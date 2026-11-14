//! Read and guarded restoration for masked UCI history captured by WiFi plan applies.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

use crate::{
    AppState, advanced_uci,
    audit::{AuditEntry, AuditResult, ConfigChange},
    safe_apply,
    session::Session,
    uci_engine::{self, UciValue},
};

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

/// Check whether every change in an audit entry can be safely restored.
fn restorable(entry: &AuditEntry) -> bool {
    let section_restore = entry.config_changes.iter().any(|c| c.restore_section);
    !entry.config_changes.is_empty()
        && entry.result == AuditResult::Ok
        && entry.config_changes.iter().all(|c| c.restorable)
        && (!section_restore
            || entry
                .config_changes
                .iter()
                .all(|c| c.restore_section && c.before.is_none() && c.after.is_some()))
        && entry
            .config_changes
            .iter()
            .map(|c| c.node.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len()
            == 1
}

/// Confirm a created section still contains exactly the values recorded in history.
fn created_section_matches(changes: &[ConfigChange], sections: &[uci_engine::Section]) -> bool {
    let Some(first) = changes.first() else {
        return false;
    };
    if changes.is_empty()
        || changes.iter().any(|c| {
            !c.restore_section
                || c.node != first.node
                || c.config != first.config
                || c.section != first.section
                || c.before.is_some()
        })
    {
        return false;
    }
    let expected: std::collections::BTreeMap<_, _> = changes
        .iter()
        .filter_map(|c| c.after.as_ref().map(|v| (c.option.as_str(), v.as_str())))
        .collect();
    let Some(section) = sections.iter().find(|s| s.name == first.section) else {
        return false;
    };
    section.options.len() == expected.len()
        && expected.iter().all(|(option, value)| {
            section.options.get(*option).is_some_and(|current| {
                current
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| current.to_string())
                    == *value
            })
        })
}

/// Compare a current option value with the post-change value stored in history.
fn matches_after(current: Option<String>, expected: &Option<String>) -> bool {
    &current == expected
}

/// Add a success or failure entry for a configuration restore attempt.
async fn record_restore_result(
    state: &AppState,
    session: &Session,
    node: &str,
    result: AuditResult,
    detail: &str,
) {
    state
        .audit
        .record(AuditEntry {
            id: String::new(),
            ts: now(),
            user: session.user.clone(),
            action: "config_restore".into(),
            target: node.into(),
            result,
            detail: detail.into(),
            config_changes: vec![],
        })
        .await;
}

/// Return audit entries that contain structured configuration changes.
pub async fn list(State(state): State<Arc<AppState>>) -> Json<Vec<AuditEntry>> {
    Json(
        state
            .audit
            .list()
            .await
            .into_iter()
            .filter(|entry| !entry.config_changes.is_empty())
            .collect(),
    )
}

/// Restore only scalar, non-secret values, and only if every current value still matches the
/// state this history row recorded. This avoids overwriting subsequent operator changes.
pub async fn restore(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Path(id): Path<String>,
) -> Response {
    let Some(entry) = state.audit.list().await.into_iter().find(|e| e.id == id) else {
        return error(StatusCode::NOT_FOUND, "unknown configuration history entry");
    };
    if entry.config_changes.is_empty() || entry.config_changes.iter().any(|c| !c.restorable) {
        return error(
            StatusCode::CONFLICT,
            "history entry contains unsupported or secret values",
        );
    }
    if entry.result != AuditResult::Ok {
        return error(
            StatusCode::CONFLICT,
            "only successful changes can be restored",
        );
    }
    let nodes: std::collections::HashSet<_> = entry
        .config_changes
        .iter()
        .map(|c| c.node.as_str())
        .collect();
    if nodes.len() != 1 {
        return error(StatusCode::CONFLICT, "history entry spans multiple nodes");
    }
    if entry.config_changes.iter().any(|c| c.restore_section) && !restorable(&entry) {
        return error(
            StatusCode::CONFLICT,
            "history entry cannot safely restore this section",
        );
    }
    debug_assert!(restorable(&entry));
    let node = *nodes.iter().next().unwrap();
    if session.read_only.contains(node) {
        return error(
            StatusCode::FORBIDDEN,
            format!("{node} is read-only for this session"),
        );
    }
    let Some(ubus) = session.node_ubus.get(node) else {
        return error(
            StatusCode::FORBIDDEN,
            format!("no write session for {node}"),
        );
    };
    let configs = entry
        .config_changes
        .iter()
        .map(|change| change.config.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if configs
        .iter()
        .any(|config| !advanced_uci::config_allowed(config))
    {
        return error(
            StatusCode::FORBIDDEN,
            "history entry references a UCI config outside the editable allowlist",
        );
    }
    for config in &configs {
        for function in ["read", "write"] {
            match ubus.has_access("uci", config, function).await {
                Ok(true) => {}
                Ok(false) => {
                    return error(
                        StatusCode::FORBIDDEN,
                        format!("rpcd ACL denies uci:{config}.{function}"),
                    );
                }
                Err(e) => {
                    return error(
                        StatusCode::FORBIDDEN,
                        format!("could not verify rpcd ACL for uci:{config}.{function}: {e}"),
                    );
                }
            }
        }
    }
    let staged_configs = configs.iter().copied().collect::<Vec<_>>();

    let result = safe_apply::run(
        ubus,
        &staged_configs,
        30,
        Duration::from_secs(3),
        || async {
            // Validate against one locked snapshot per config. Keep these reads inside `run` so
            // no other UCI transaction can change state between validation and staging.
            let mut configs = BTreeMap::new();
            for diff in &entry.config_changes {
                if !configs.contains_key(&diff.config) {
                    let sections = uci_engine::read(ubus, &diff.config)
                        .await
                        .map_err(|_| "__restore_read_error__".to_owned())?;
                    configs.insert(diff.config.clone(), sections);
                }
            }

            if entry.config_changes[0].restore_section {
                let sections = &configs[&entry.config_changes[0].config];
                if !created_section_matches(&entry.config_changes, sections) {
                    return Err(
                        "__restore_conflict__:created section changed since this history entry"
                            .to_owned(),
                    );
                }
            }

            for diff in &entry.config_changes {
                let sections = &configs[&diff.config];
                let current = sections
                    .iter()
                    .find(|s| s.name == diff.section)
                    .and_then(|s| s.options.get(&diff.option))
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string())
                    });
                if !matches_after(current, &diff.after) {
                    return Err(format!(
                        "__restore_conflict__:{} changed since this history entry",
                        diff.option
                    ));
                }
            }

            for diff in &entry.config_changes {
                if diff.restore_section {
                    continue;
                }
                if let Some(before) = &diff.before {
                    let mut values = BTreeMap::new();
                    values.insert(diff.option.clone(), UciValue::String(before.clone()));
                    uci_engine::update(ubus, &diff.config, &diff.section, &values)
                        .await
                        .map_err(|e| e.to_string())?;
                } else {
                    uci_engine::delete(ubus, &diff.config, &diff.section, Some(&diff.option))
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
            if entry.config_changes[0].restore_section {
                let first = &entry.config_changes[0];
                uci_engine::delete(ubus, &first.config, &first.section, None)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        },
        || async { safe_apply::board_reachable(ubus).await },
    )
    .await;
    if let Err(safe_apply::Error::Stage(stage_error)) = &result {
        if stage_error == "__restore_read_error__" {
            return error(StatusCode::BAD_GATEWAY, "could not read current UCI state");
        }
        if let Some(message) = stage_error.strip_prefix("__restore_conflict__:") {
            return error(StatusCode::CONFLICT, message);
        }
    }
    if let Err(e) = result {
        record_restore_result(
            &state,
            &session,
            node,
            AuditResult::Error,
            "restore failed; rollback is automatic when armed",
        )
        .await;
        return error(StatusCode::BAD_GATEWAY, e.to_string());
    }
    record_restore_result(
        &state,
        &session,
        node,
        AuditResult::Ok,
        &format!("restored history entry {id}"),
    )
    .await;
    Json(json!({"ok": true})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::ConfigChange;

    /// Create a successful one-option history entry for restore tests.
    fn sample() -> AuditEntry {
        AuditEntry {
            id: "history-1".into(),
            ts: 1,
            user: "root".into(),
            action: "plan_apply_confirmed".into(),
            target: "ap-2".into(),
            result: AuditResult::Ok,
            detail: "confirmed".into(),
            config_changes: vec![ConfigChange {
                node: "ap-2".into(),
                config: "wireless".into(),
                section: "main".into(),
                option: "ssid".into(),
                before: Some("old".into()),
                after: Some("new".into()),
                restorable: true,
                restore_section: false,
            }],
        }
    }

    #[test]
    /// Checks failed, secret, and cross-node entries are not restorable.
    fn rejects_failed_secret_and_cross_node_history() {
        let mut entry = sample();
        assert!(restorable(&entry));
        entry.result = AuditResult::Error;
        assert!(!restorable(&entry));
        entry.result = AuditResult::Ok;
        entry.config_changes[0].restorable = false;
        assert!(!restorable(&entry));
        entry.config_changes[0].restorable = true;
        let mut second = entry.config_changes[0].clone();
        second.node = "gateway".into();
        entry.config_changes.push(second);
        assert!(!restorable(&entry));
    }

    #[test]
    /// Checks restore matching rejects values changed after the recorded operation.
    fn stale_state_does_not_match_recorded_after_value() {
        assert!(matches_after(Some("new".into()), &Some("new".into())));
        assert!(!matches_after(Some("later".into()), &Some("new".into())));
        assert!(!matches_after(None, &Some("new".into())));
    }

    #[test]
    /// Checks reservation removal requires an exact match of the created section.
    fn reservation_restore_requires_exact_section_contents() {
        let changes = [
            ("mac", "aa:bb:cc:dd:ee:ff"),
            ("ip", "192.168.1.20"),
            ("name", "desktop"),
        ]
        .into_iter()
        .map(|(option, value)| ConfigChange {
            node: "gateway".into(),
            config: "dhcp".into(),
            section: "cfg123".into(),
            option: option.into(),
            before: None,
            after: Some(value.into()),
            restorable: true,
            restore_section: true,
        })
        .collect::<Vec<_>>();
        let section = uci_engine::Section {
            name: "cfg123".into(),
            kind: "host".into(),
            anonymous: true,
            index: Some(0),
            options: [
                (
                    "mac".into(),
                    serde_json::Value::String("aa:bb:cc:dd:ee:ff".into()),
                ),
                (
                    "ip".into(),
                    serde_json::Value::String("192.168.1.20".into()),
                ),
                ("name".into(), serde_json::Value::String("desktop".into())),
            ]
            .into_iter()
            .collect(),
        };
        assert!(created_section_matches(
            &changes,
            std::slice::from_ref(&section)
        ));
        let mut changed = section.clone();
        changed
            .options
            .insert("name".into(), serde_json::Value::String("renamed".into()));
        assert!(!created_section_matches(&changes, &[changed]));
        let mut extended = section;
        extended
            .options
            .insert("tag".into(), serde_json::Value::String("manual".into()));
        assert!(!created_section_matches(&changes, &[extended]));
    }
}
