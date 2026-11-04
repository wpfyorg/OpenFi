//! Controlled advanced UCI editor. It edits options on existing sections only; section identity,
//! type, and order remain owned by the router. Writes use the caller's session and rpcd rollback.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    AppState,
    audit::{self, AuditEntry, AuditResult, ConfigChange},
    safe_apply,
    session::Session,
    uci_engine::{self, Section, UciScalar, UciValue},
};

#[derive(Debug, Deserialize)]
pub struct EditRequest {
    pub sections: BTreeMap<String, BTreeMap<String, Value>>,
}

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

async fn require_uci_acl(
    ubus: &crate::ubus::Ubus,
    config: &str,
    function: &str,
) -> Result<(), String> {
    match ubus.has_access("uci", config, function).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("rpcd ACL denies uci:{config}.{function}")),
        Err(e) => Err(format!(
            "could not verify rpcd ACL for uci:{config}.{function}: {e}"
        )),
    }
}

#[allow(clippy::result_large_err)]
fn node_handle<'a>(
    state: &AppState,
    session: &'a Session,
    node: &str,
) -> Result<&'a crate::ubus::Ubus, Response> {
    if session.read_only.contains(node) {
        return Err(err(
            StatusCode::FORBIDDEN,
            format!("{node} is read-only for this session"),
        ));
    }
    session.node_ubus.get(node).ok_or_else(|| {
        if state.nodes.iter().any(|(n, _)| n.name == node) {
            err(
                StatusCode::FORBIDDEN,
                format!("no write session for {node}"),
            )
        } else {
            err(StatusCode::BAD_REQUEST, format!("unknown node {node:?}"))
        }
    })
}

fn to_uci(value: &Value) -> Result<UciValue, String> {
    match value {
        Value::String(v) => Ok(UciValue::String(v.clone())),
        Value::Bool(v) => Ok(UciValue::Bool(*v)),
        Value::Number(v) => v
            .as_i64()
            .map(UciValue::Integer)
            .ok_or_else(|| "only signed integer UCI numbers are supported".into()),
        Value::Array(values) => values
            .iter()
            .map(|v| match v {
                Value::String(s) => Ok(UciScalar::String(s.clone())),
                Value::Bool(b) => Ok(UciScalar::Bool(*b)),
                Value::Number(n) => n
                    .as_i64()
                    .map(UciScalar::Integer)
                    .ok_or_else(|| "only signed integer list values are supported".into()),
                _ => Err("UCI lists may contain only strings, booleans, or integers".into()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(UciValue::List),
        _ => Err("UCI values must be strings, booleans, integers, or scalar lists".into()),
    }
}

fn diff(
    sections: &[Section],
    requested: &BTreeMap<String, BTreeMap<String, Value>>,
) -> Result<Vec<(String, String, Option<UciValue>)>, String> {
    if requested.len() != sections.len()
        || sections.iter().any(|s| !requested.contains_key(&s.name))
    {
        return Err("section names are fixed; include every current section without renaming or removing it".into());
    }
    let mut changes = Vec::new();
    for section in sections {
        let target = &requested[&section.name];
        for (key, value) in target {
            uci_engine::validate_option_name(key).map_err(|e| e.to_string())?;
            if audit::is_sensitive_uci_option(key)
                && section.options.contains_key(key)
                && *value == Value::String(audit::mask_secret("").into())
            {
                continue;
            }
            let typed = to_uci(value)?;
            if section.options.get(key) != Some(&typed.to_wire()) {
                changes.push((section.name.clone(), key.clone(), Some(typed)));
            }
        }
        for key in section
            .options
            .keys()
            .filter(|key| !target.contains_key(*key))
        {
            changes.push((section.name.clone(), key.clone(), None));
        }
    }
    Ok(changes)
}

fn redacted_options(options: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    options
        .iter()
        .map(|(key, value)| {
            let shown = if audit::is_sensitive_uci_option(key) {
                Value::String(audit::mask_secret("").into())
            } else {
                value.clone()
            };
            (key.clone(), shown)
        })
        .collect()
}

fn scrub_sensitive_values(message: &str, changes: &[(String, String, Option<UciValue>)]) -> String {
    let mut secrets = Vec::new();
    for (_, option, value) in changes {
        if !audit::is_sensitive_uci_option(option) {
            continue;
        }
        if let Some(value) = value {
            match value {
                UciValue::String(s) if !s.is_empty() => secrets.push(s.clone()),
                UciValue::List(items) => {
                    secrets.extend(items.iter().filter_map(|item| match item {
                        UciScalar::String(s) if !s.is_empty() => Some(s.clone()),
                        _ => None,
                    }))
                }
                _ => {}
            }
        }
    }
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    secrets
        .into_iter()
        .fold(message.to_owned(), |safe, secret| {
            safe.replace(&secret, audit::mask_secret(""))
        })
}

fn display_diff(config: &str, changes: &[(String, String, Option<UciValue>)]) -> Vec<String> {
    changes
        .iter()
        .map(|(section, key, value)| match value {
            Some(value) if audit::is_sensitive_uci_option(key) => {
                audit::mask_uci_line(config, section, key, &value.as_text())
            }
            Some(value) => format!("{config}.{section}.{key} = {}", value.to_wire()),
            None => format!("{config}.{section}.{key} (delete)"),
        })
        .collect()
}

fn history_changes(
    node: &str,
    config: &str,
    sections: &[Section],
    changes: &[(String, String, Option<UciValue>)],
) -> Vec<ConfigChange> {
    changes
        .iter()
        .map(|(section, option, after)| {
            let before = sections
                .iter()
                .find(|current| current.name == *section)
                .and_then(|current| current.options.get(option));
            let secret = audit::is_sensitive_uci_option(option);
            let show_before = before.map(|value| {
                if secret {
                    audit::mask_secret("").to_owned()
                } else if let Some(value) = value.as_str() {
                    value.to_owned()
                } else {
                    value.to_string()
                }
            });
            let show_after = after.as_ref().map(|value| {
                if secret {
                    audit::mask_secret("").to_owned()
                } else {
                    value.as_text()
                }
            });
            let before_scalar = before
                .is_none_or(|value| value.is_string() || value.is_boolean() || value.is_number());
            let after_scalar = after
                .as_ref()
                .is_none_or(|value| !matches!(value, UciValue::List(_)));
            ConfigChange {
                node: node.into(),
                config: config.into(),
                section: section.clone(),
                option: option.clone(),
                before: show_before,
                after: show_after,
                restorable: !secret && before_scalar && after_scalar,
                restore_section: false,
            }
        })
        .collect()
}

async fn record(
    state: &AppState,
    session: &Session,
    action: &str,
    target: &str,
    result: Result<&str, &str>,
    config_changes: Vec<ConfigChange>,
) {
    let (result, detail) = match result {
        Ok(s) => (AuditResult::Ok, s),
        Err(s) => (AuditResult::Error, s),
    };
    state
        .audit
        .record(AuditEntry {
            id: String::new(),
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            user: session.user.clone(),
            action: action.into(),
            target: target.into(),
            result,
            detail: detail.into(),
            config_changes,
        })
        .await;
}

pub async fn read_config(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Path((node, config)): Path<(String, String)>,
) -> Response {
    let ubus = match node_handle(&state, &session, &node) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if let Err(message) = require_uci_acl(ubus, &config, "read").await {
        return err(StatusCode::FORBIDDEN, message);
    }
    match uci_engine::read(ubus, &config).await {
        Ok(sections) => {
            let safe_sections: BTreeMap<_, _> = sections
                .iter()
                .map(|section| (section.name.clone(), redacted_options(&section.options)))
                .collect();
            Json(json!({"node": node, "config": config, "sections": safe_sections})).into_response()
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

pub async fn preview(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Path((node, config)): Path<(String, String)>,
    Json(body): Json<EditRequest>,
) -> Response {
    let ubus = match node_handle(&state, &session, &node) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if let Err(message) = require_uci_acl(ubus, &config, "read").await {
        return err(StatusCode::FORBIDDEN, message);
    }
    let sections = match uci_engine::read(ubus, &config).await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    match diff(&sections, &body.sections) {
        Ok(changes) => Json(
            json!({"node": node, "config": config, "changes": display_diff(&config, &changes)}),
        )
        .into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

pub async fn apply(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Path((node, config)): Path<(String, String)>,
    Json(body): Json<EditRequest>,
) -> Response {
    let ubus = match node_handle(&state, &session, &node) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if let Err(message) = require_uci_acl(ubus, &config, "read").await {
        return err(StatusCode::FORBIDDEN, message);
    }
    if let Err(message) = require_uci_acl(ubus, &config, "write").await {
        return err(StatusCode::FORBIDDEN, message);
    }
    if state.mock.enabled() {
        return err(
            StatusCode::CONFLICT,
            "advanced UCI editing is disabled in mock mode",
        );
    }
    let sections = match uci_engine::read(ubus, &config).await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    let changes = match diff(&sections, &body.sections) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    if changes.is_empty() {
        return err(StatusCode::BAD_REQUEST, "no UCI changes to apply");
    }
    let lines = display_diff(&config, &changes);
    let config_changes = history_changes(&node, &config, &sections, &changes);
    let target = format!("{node}:{config}");
    let result = safe_apply::run(
        ubus,
        30,
        Duration::from_secs(3),
        || async {
            for (section, option, value) in &changes {
                match value {
                    Some(value) => uci_engine::update(
                        ubus,
                        &config,
                        section,
                        &BTreeMap::from([(option.clone(), value.clone())]),
                    )
                    .await
                    .map_err(|e| e.to_string())?,
                    None => uci_engine::delete(ubus, &config, section, Some(option))
                        .await
                        .map_err(|e| e.to_string())?,
                }
            }
            Ok::<(), String>(())
        },
        || async { safe_apply::board_reachable(ubus).await },
    )
    .await;
    match result {
        Ok(_) => {
            let detail = lines.join("; ");
            record(
                &state,
                &session,
                "uci_advanced_apply",
                &target,
                Ok(&detail),
                config_changes,
            )
            .await;
            Json(json!({"ok": true, "changes": lines})).into_response()
        }
        Err(e) => {
            let safe_error = scrub_sensitive_values(&e.to_string(), &changes);
            record(
                &state,
                &session,
                "uci_advanced_apply",
                &target,
                Err(&safe_error),
                config_changes,
            )
            .await;
            err(StatusCode::BAD_GATEWAY, safe_error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn section(options: serde_json::Value) -> Section {
        Section {
            name: "main".into(),
            kind: "service".into(),
            anonymous: false,
            index: Some(0),
            options: options
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn diff_validates_fixed_sections_and_supported_uci_values() {
        let current = vec![section(json!({"enabled":"0", "hosts":["lan", "guest"]}))];
        let target = BTreeMap::from([(
            "main".into(),
            BTreeMap::from([
                ("enabled".into(), json!(true)),
                ("hosts".into(), json!(["lan", "guest"])),
            ]),
        )]);
        let changes = diff(&current, &target).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, "main");
        assert_eq!(changes[0].1, "enabled");
        assert!(diff(&current, &BTreeMap::new()).is_err());
        let invalid = BTreeMap::from([(
            "main".into(),
            BTreeMap::from([("bad.key".into(), json!(1))]),
        )]);
        assert!(diff(&current, &invalid).is_err());
        let nested = BTreeMap::from([(
            "main".into(),
            BTreeMap::from([("x".into(), json!({"nested":true}))]),
        )]);
        assert!(diff(&current, &nested).is_err());
    }

    #[test]
    fn secret_values_are_masked_in_preview_and_audit_diff() {
        for option in ["key", "password", "psk", "private_key", "auth_token"] {
            let changes = vec![(
                "wireless".into(),
                option.into(),
                Some(UciValue::String("secret-value".into())),
            )];
            let lines = display_diff("wireless", &changes);
            assert!(lines[0].contains("••••••••"));
            assert!(!lines[0].contains("secret-value"));
        }
    }

    #[test]
    fn read_redacts_secret_options_and_mask_round_trips_as_unchanged() {
        let options = BTreeMap::from([
            ("psk".into(), json!("raw-secret")),
            ("auth_token".into(), json!("token-value")),
            ("ssid".into(), json!("home")),
        ]);
        let safe = redacted_options(&options);
        assert_eq!(safe["psk"], json!("••••••••"));
        assert_eq!(safe["auth_token"], json!("••••••••"));
        assert_eq!(safe["ssid"], json!("home"));

        let current = vec![section(json!({"psk":"raw-secret"}))];
        let request = BTreeMap::from([(
            "main".into(),
            BTreeMap::from([("psk".into(), json!("••••••••"))]),
        )]);
        assert!(diff(&current, &request).unwrap().is_empty());
    }

    #[test]
    fn staged_errors_scrub_submitted_secret_values() {
        let changes = vec![(
            "main".into(),
            "auth_token".into(),
            Some(UciValue::String("token-value".into())),
        )];
        let safe = scrub_sensitive_values("router rejected token-value", &changes);
        assert_eq!(safe, "router rejected ••••••••");
    }

    #[test]
    fn history_masks_secrets_and_only_marks_scalar_changes_restorable() {
        let current = vec![section(json!({
            "ssid":"old",
            "psk":"old-secret",
            "dns":["1.1.1.1", "8.8.8.8"]
        }))];
        let changes = vec![
            (
                "main".into(),
                "ssid".into(),
                Some(UciValue::String("new".into())),
            ),
            (
                "main".into(),
                "psk".into(),
                Some(UciValue::String("new-secret".into())),
            ),
            ("main".into(), "dns".into(), None),
        ];
        let history = history_changes("gateway", "wireless", &current, &changes);
        assert_eq!(history[0].before.as_deref(), Some("old"));
        assert_eq!(history[0].after.as_deref(), Some("new"));
        assert!(history[0].restorable);
        assert_eq!(history[1].before.as_deref(), Some("••••••••"));
        assert_eq!(history[1].after.as_deref(), Some("••••••••"));
        assert!(!history[1].restorable);
        assert!(!history[2].restorable);
    }

    #[tokio::test]
    async fn refuses_read_only_sessions_and_mock_mode_apply() {
        let state = crate::test_support::minimal_app_state();
        let node = state.nodes[0].0.name.clone();
        let session_token = state
            .sessions
            .create(
                "tester".into(),
                HashMap::new(),
                HashSet::from([node.clone()]),
            )
            .await;
        let session = state.sessions.touch(&session_token).await.unwrap();
        let response = apply(
            State(state.clone()),
            Extension(session),
            Path((node.clone(), "system".into())),
            Json(EditRequest {
                sections: BTreeMap::new(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let handle = state.nodes[0].1.clone();
        let session_token = state
            .sessions
            .create(
                "tester".into(),
                HashMap::from([(node.clone(), handle)]),
                HashSet::new(),
            )
            .await;
        let session = state.sessions.touch(&session_token).await.unwrap();
        let response = apply(
            State(state),
            Extension(session),
            Path((node, "system".into())),
            Json(EditRequest {
                sections: BTreeMap::new(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn config_read_response_redacts_sensitive_values() {
        let state = crate::test_support::minimal_app_state();
        let (node_config, handle) = &state.nodes[0];
        let node = node_config.name.clone();
        let token = state
            .sessions
            .create(
                "tester".into(),
                HashMap::from([(node.clone(), handle.clone())]),
                HashSet::new(),
            )
            .await;
        let session = state.sessions.touch(&token).await.unwrap();
        let response = read_config(
            State(state),
            Extension(session),
            Path((node, "wireless".into())),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!body.contains("REDACTED"));
        assert!(body.contains("••••••••"));
    }
}
