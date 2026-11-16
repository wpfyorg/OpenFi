//! Controlled advanced UCI editor. It edits options on existing sections only; section identity,
//! type, and order remain owned by the router. Writes use the caller's session and rpcd rollback.

use std::{
    collections::{BTreeMap, hash_map::RandomState},
    hash::{BuildHasher, Hasher},
    sync::{Arc, OnceLock},
    time::Duration,
};

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

type ConfigSnapshot = BTreeMap<String, BTreeMap<String, Value>>;

const STALE_CONFIG_MESSAGE: &str = "configuration changed since it was loaded; reload and retry";
const EDITABLE_CONFIGS: &[&str] = &["wireless", "dhcp", "system"];

/// Return whether a UCI config is exposed through the guarded advanced editor.
pub(crate) fn config_allowed(config: &str) -> bool {
    EDITABLE_CONFIGS.contains(&config)
}

/// Reject UCI configs that are outside the dashboard's bounded editable set.
fn require_allowed_config(config: &str) -> Result<(), Response> {
    if config_allowed(config) {
        Ok(())
    } else {
        Err(err(
            StatusCode::FORBIDDEN,
            format!("UCI config {config:?} is not available in the advanced editor"),
        ))
    }
}

#[derive(Debug, Deserialize)]
pub struct EditRequest {
    /// Opaque token from the preceding `GET`; it changes whenever any value, including a masked
    /// credential, changes on the node.
    pub revision: String,
    pub base: ConfigSnapshot,
    pub sections: ConfigSnapshot,
}

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

/// Check whether the caller's rpcd session may invoke a UCI method for this config.
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
/// Resolve a writable node handle, rejecting read-only or unknown nodes.
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

/// Convert a JSON scalar or scalar list into the corresponding UCI value.
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

/// Validate a complete section snapshot and produce only changed UCI options.
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

/// Replace values for sensitive options with the fixed audit mask.
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

/// Convert live UCI sections into the same secret-masked snapshot returned to clients.
fn redacted_snapshot(sections: &[Section]) -> ConfigSnapshot {
    sections
        .iter()
        .map(|section| (section.name.clone(), redacted_options(&section.options)))
        .collect()
}

/// Rebuild the option-only section view needed to diff a client base snapshot.
fn snapshot_sections(snapshot: &ConfigSnapshot) -> Vec<Section> {
    snapshot
        .iter()
        .map(|(name, options)| Section {
            name: name.clone(),
            kind: String::new(),
            anonymous: false,
            index: None,
            options: options.clone(),
        })
        .collect()
}

/// Reject a request if the live config no longer matches the client's loaded snapshot.
fn validate_base_snapshot(current: &[Section], base: &ConfigSnapshot) -> Result<(), String> {
    if redacted_snapshot(current) == *base {
        Ok(())
    } else {
        Err(STALE_CONFIG_MESSAGE.into())
    }
}

/// Opaque conflict token for a live config snapshot.
///
/// The masked `base` snapshot cannot reveal a credential change, so the token hashes the full
/// snapshot — secrets included — with a process-random key: it changes whenever any value changes,
/// and reveals nothing about the values it covers.
fn revision_token(sections: &[Section]) -> String {
    static KEYS: OnceLock<RandomState> = OnceLock::new();
    let mut hasher = KEYS.get_or_init(RandomState::new).build_hasher();
    for section in sections {
        hasher.write(section.name.as_bytes());
        hasher.write_u8(0);
        for (key, value) in &section.options {
            hasher.write(key.as_bytes());
            hasher.write_u8(0);
            hasher.write(value.to_string().as_bytes());
            hasher.write_u8(0);
        }
    }
    format!("{:016x}", hasher.finish())
}

/// Reject a request whose revision token no longer matches the live config.
fn validate_revision(current: &[Section], revision: &str) -> Result<(), String> {
    if revision_token(current) == revision {
        Ok(())
    } else {
        Err(STALE_CONFIG_MESSAGE.into())
    }
}

/// Remove submitted secret values from an error string before returning it to the client.
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

/// Render changed options as readable lines while masking sensitive values.
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

/// Build secret-masked history records and mark only supported scalar changes restorable.
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

/// Store one advanced-editor result and its structured configuration changes.
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

/// Read a node's UCI config, returning sensitive options in masked form.
pub async fn read_config(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Path((node, config)): Path<(String, String)>,
) -> Response {
    if let Err(response) = require_allowed_config(&config) {
        return response;
    }
    let ubus = match node_handle(&state, &session, &node) {
        Ok(u) => u,
        Err(r) => return r,
    };
    if let Err(message) = require_uci_acl(ubus, &config, "read").await {
        return err(StatusCode::FORBIDDEN, message);
    }
    match uci_engine::read(ubus, &config).await {
        Ok(sections) => {
            let revision = revision_token(&sections);
            let safe_sections = redacted_snapshot(&sections);
            Json(json!({
                "node": node,
                "config": config,
                "revision": revision,
                "sections": safe_sections,
            }))
            .into_response()
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

/// Preview the option changes requested for a node's UCI config.
pub async fn preview(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Path((node, config)): Path<(String, String)>,
    Json(body): Json<EditRequest>,
) -> Response {
    if let Err(response) = require_allowed_config(&config) {
        return response;
    }
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
    if let Err(message) = validate_revision(&sections, &body.revision) {
        return err(StatusCode::CONFLICT, message);
    }
    if let Err(message) = validate_base_snapshot(&sections, &body.base) {
        return err(StatusCode::CONFLICT, message);
    }
    match diff(&sections, &body.sections) {
        Ok(changes) => Json(
            json!({"node": node, "config": config, "changes": display_diff(&config, &changes)}),
        )
        .into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e),
    }
}

/// Apply validated UCI changes with rpcd rollback and audit the result.
pub async fn apply(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Path((node, config)): Path<(String, String)>,
    Json(body): Json<EditRequest>,
) -> Response {
    if let Err(response) = require_allowed_config(&config) {
        return response;
    }
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
    let base_sections = snapshot_sections(&body.base);
    let changes = match diff(&base_sections, &body.sections) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    if changes.is_empty() {
        return err(StatusCode::BAD_REQUEST, "no UCI changes to apply");
    }
    let lines = display_diff(&config, &changes);
    let config_changes = history_changes(&node, &config, &base_sections, &changes);
    let target = format!("{node}:{config}");
    let result = safe_apply::run(
        ubus,
        &[config.as_str()],
        30,
        Duration::from_secs(3),
        || async {
            let current = uci_engine::read(ubus, &config)
                .await
                .map_err(|e| e.to_string())?;
            // Checked inside the transaction so a credential changed behind the mask is caught
            // before any staged write can land on top of it.
            validate_revision(&current, &body.revision)?;
            validate_base_snapshot(&current, &body.base)?;
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
            let status = if matches!(
                &e,
                safe_apply::Error::Stage(message) if message == STALE_CONFIG_MESSAGE
            ) {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_GATEWAY
            };
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
            err(status, safe_error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// Build a named wireless section from a JSON option object for editor tests.
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
    /// Checks the editor exposes only the bounded config set granted by the packaged admin ACL.
    fn editable_config_allowlist_rejects_sensitive_and_unmanaged_configs() {
        for config in ["wireless", "dhcp", "system"] {
            assert!(config_allowed(config));
        }
        for config in ["rpcd", "firewall", "network", "dropbear"] {
            assert!(!config_allowed(config));
        }
    }

    #[test]
    /// Checks fixed section names and supported scalar/list UCI values.
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
    /// Checks any visible config change since load invalidates the client base snapshot.
    fn stale_base_snapshot_is_rejected_before_staging() {
        let loaded = vec![section(json!({
            "enabled":"0",
            "description":"old",
            "psk":"secret"
        }))];
        let base = redacted_snapshot(&loaded);

        assert!(validate_base_snapshot(&loaded, &base).is_ok());
        assert!(
            validate_base_snapshot(
                &[section(json!({
                    "enabled":"0",
                    "description":"changed elsewhere",
                    "psk":"secret"
                }))],
                &base,
            )
            .is_err()
        );
    }

    #[test]
    /// Checks preview and audit output mask secret values.
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
    /// Checks reads redact secrets and an unchanged mask is treated as unchanged input.
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
    /// Checks the opaque revision token tracks every value, including masked credentials.
    fn revision_token_changes_when_a_masked_credential_changes() {
        let before = vec![section(json!({"ssid":"home","psk":"first-secret"}))];
        let after = vec![section(json!({"ssid":"home","psk":"second-secret"}))];
        assert_eq!(revision_token(&before), revision_token(&before));
        assert_ne!(revision_token(&before), revision_token(&after));
        assert!(validate_revision(&before, &revision_token(&before)).is_ok());
        assert_eq!(
            validate_revision(&before, &revision_token(&after)).unwrap_err(),
            STALE_CONFIG_MESSAGE
        );
        assert!(!revision_token(&before).contains("first-secret"));
    }

    #[test]
    /// Checks UCI errors do not return submitted secret values.
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
    /// Checks history masks secrets and limits restoration to supported scalar changes.
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
    /// Checks read-only sessions and mock mode cannot apply advanced UCI edits.
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
                revision: String::new(),
                base: BTreeMap::new(),
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
                revision: String::new(),
                base: BTreeMap::new(),
                sections: BTreeMap::new(),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    /// Checks a config read response contains redacted sensitive option values.
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
