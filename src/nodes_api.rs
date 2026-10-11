//! Persistent node configuration used by the Settings -> Nodes UI.
//!
//! The running poll loop intentionally remains immutable. Add/remove updates the JSON config,
//! returns to the browser, then restarts the procd service so the normal startup path rebuilds
//! every Ubus handle from the new node list.

use std::{path::Path as FsPath, sync::Arc, time::Duration};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{AppState, validate};

static CONFIG_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Debug, Clone, Serialize)]
pub struct ConfiguredNode {
    pub name: String,
    pub host: String,
    pub username: String,
    pub wired_uplink: Option<String>,
    pub gateway: bool,
    pub password_set: bool,
}

#[derive(Debug, Deserialize)]
pub struct AddNodeRequest {
    pub name: String,
    pub host: String,
    #[serde(default = "default_username")]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub wired_uplink: Option<String>,
}

fn default_username() -> String {
    "root".into()
}

fn err(code: StatusCode, message: impl Into<String>) -> Response {
    (code, Json(json!({ "error": message.into() }))).into_response()
}

fn view(
    name: &str,
    host: &str,
    username: Option<&str>,
    password: Option<&str>,
    wired_uplink: Option<&str>,
    gateway: bool,
) -> ConfiguredNode {
    ConfiguredNode {
        name: name.to_owned(),
        host: host.to_owned(),
        username: username.unwrap_or("root").to_owned(),
        wired_uplink: wired_uplink.map(str::to_owned),
        gateway,
        password_set: password.is_some_and(|p| !p.is_empty()),
    }
}

pub async fn list(State(state): State<Arc<AppState>>) -> Json<Vec<ConfiguredNode>> {
    Json(
        state
            .config
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                view(
                    &n.name,
                    &n.host,
                    n.username.as_deref(),
                    n.password.as_deref(),
                    n.wired_uplink.as_deref(),
                    i == 0,
                )
            })
            .collect(),
    )
}

pub async fn add(State(state): State<Arc<AppState>>, Json(body): Json<AddNodeRequest>) -> Response {
    if state.mock.enabled() {
        return err(
            StatusCode::CONFLICT,
            "node configuration is disabled in mock mode",
        );
    }

    let name = body.name.trim().to_owned();
    let host = body.host.trim().to_owned();
    let username = body.username.trim().to_owned();
    let wired_uplink = body
        .wired_uplink
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    if !validate::reservation_name_ok(&name) {
        return err(
            StatusCode::BAD_REQUEST,
            "name must use only letters, numbers, '-' or '_'",
        );
    }
    if !validate::ipv4_ok(&host) {
        return err(StatusCode::BAD_REQUEST, "host must be an IPv4 address");
    }
    if !validate::reservation_name_ok(&username) {
        return err(
            StatusCode::BAD_REQUEST,
            "username must use only letters, numbers, '-' or '_'",
        );
    }
    if wired_uplink.as_deref().is_some_and(|s| !iface_ok(s)) {
        return err(
            StatusCode::BAD_REQUEST,
            "wired uplink contains invalid characters",
        );
    }

    let mut node = json!({
        "name": name.clone(),
        "host": host.clone(),
        "transport": "rpcd",
        "username": username.clone(),
        "password": body.password.clone(),
    });
    if let Some(uplink) = &wired_uplink {
        node["wired_uplink"] = Value::String(uplink.clone());
    }

    let update = mutate_config(&state.config_path, |doc| {
        let Some(nodes) = doc.get_mut("nodes").and_then(Value::as_array_mut) else {
            return Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "config has no nodes array",
            ));
        };
        if nodes
            .iter()
            .any(|n| n.get("name").and_then(Value::as_str) == Some(name.as_str()))
        {
            return Err(err(
                StatusCode::CONFLICT,
                format!("a node named {name:?} already exists"),
            ));
        }
        if nodes
            .iter()
            .any(|n| n.get("host").and_then(Value::as_str) == Some(host.as_str()))
        {
            return Err(err(
                StatusCode::CONFLICT,
                format!("a node at {host} already exists"),
            ));
        }
        nodes.push(node);
        Ok(())
    })
    .await;
    if let Err(response) = update {
        return response;
    }
    schedule_restart();

    (
        StatusCode::CREATED,
        Json(view(
            &name,
            &host,
            Some(&username),
            Some(&body.password),
            wired_uplink.as_deref(),
            false,
        )),
    )
        .into_response()
}

pub async fn remove(State(state): State<Arc<AppState>>, Path(node_name): Path<String>) -> Response {
    if state.mock.enabled() {
        return err(
            StatusCode::CONFLICT,
            "node configuration is disabled in mock mode",
        );
    }
    if state
        .config
        .nodes
        .first()
        .is_some_and(|n| n.name == node_name)
    {
        return err(StatusCode::BAD_REQUEST, "the gateway cannot be removed");
    }

    let update = mutate_config(&state.config_path, |doc| {
        let Some(nodes) = doc.get_mut("nodes").and_then(Value::as_array_mut) else {
            return Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "config has no nodes array",
            ));
        };
        let before = nodes.len();
        nodes.retain(|n| n.get("name").and_then(Value::as_str) != Some(node_name.as_str()));
        if nodes.len() == before {
            return Err(err(
                StatusCode::NOT_FOUND,
                format!("unknown node {node_name:?}"),
            ));
        }
        Ok(())
    })
    .await;
    if let Err(response) = update {
        return response;
    }
    schedule_restart();
    Json(json!({ "removed": node_name })).into_response()
}

fn iface_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

async fn read_config(path: &FsPath) -> Result<Value, String> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))
}

async fn mutate_config<T>(
    path: &FsPath,
    update: impl FnOnce(&mut Value) -> Result<T, Response>,
) -> Result<T, Response> {
    let _mutation_guard = CONFIG_MUTATION_LOCK.lock().await;
    let mut doc = read_config(path)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let result = update(&mut doc)?;
    write_config(path, &doc)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(result)
}

async fn write_config(path: &FsPath, doc: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(doc).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, bytes)
        .await
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    if let Ok(meta) = tokio::fs::metadata(path).await {
        let _ = tokio::fs::set_permissions(&tmp, meta.permissions()).await;
    }
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| format!("replace {}: {e}", path.display()))
}

fn schedule_restart() {
    tokio::spawn(async {
        // Let axum flush the JSON response before procd stops this process.
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Err(e) = tokio::process::Command::new("/etc/init.d/openwrtfi")
            .arg("restart")
            .spawn()
        {
            log::error!("restart after node configuration change failed: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "owfi-node-config-{name}-{}.json",
            std::process::id()
        ))
    }

    async fn concurrent_edit(
        path: std::path::PathBuf,
        start: Arc<tokio::sync::Barrier>,
        edit: impl FnOnce(&mut Value) -> Result<(), Response> + Send + 'static,
    ) {
        start.wait().await;
        mutate_config(&path, edit).await.unwrap();
    }

    #[test]
    fn uplink_name_validation() {
        assert!(iface_ok("lan1"));
        assert!(iface_ok("br-lan.20"));
        assert!(!iface_ok("lan1; reboot"));
        assert!(!iface_ok(""));
    }

    #[tokio::test]
    async fn concurrent_adds_both_survive_config_write() {
        let path = temp_config("add-add");
        tokio::fs::write(
            &path,
            r#"{"nodes":[{"name":"gateway","host":"192.0.2.1"}],"other":"preserved"}"#,
        )
        .await
        .unwrap();
        let start = Arc::new(tokio::sync::Barrier::new(2));
        let a = tokio::spawn(concurrent_edit(path.clone(), start.clone(), |doc| {
            doc["nodes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name":"ap-a","host":"192.0.2.2"}));
            Ok(())
        }));
        let b = tokio::spawn(concurrent_edit(path.clone(), start, |doc| {
            doc["nodes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name":"ap-b","host":"192.0.2.3"}));
            Ok(())
        }));
        a.await.unwrap();
        b.await.unwrap();
        let doc = read_config(&path).await.unwrap();
        let names: Vec<_> = doc["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["name"].as_str())
            .collect();
        assert!(
            names.contains(&"ap-a") && names.contains(&"ap-b"),
            "lost concurrent add: {names:?}"
        );
        assert_eq!(doc["other"], "preserved");
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn concurrent_add_and_remove_both_survive_config_write() {
        let path = temp_config("add-remove");
        tokio::fs::write(&path, r#"{"nodes":[{"name":"gateway","host":"192.0.2.1"},{"name":"ap-old","host":"192.0.2.2"}],"other":"preserved"}"#)
            .await
            .unwrap();
        let start = Arc::new(tokio::sync::Barrier::new(2));
        let add = tokio::spawn(concurrent_edit(path.clone(), start.clone(), |doc| {
            doc["nodes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"name":"ap-new","host":"192.0.2.3"}));
            Ok(())
        }));
        let remove = tokio::spawn(concurrent_edit(path.clone(), start, |doc| {
            doc["nodes"]
                .as_array_mut()
                .unwrap()
                .retain(|n| n["name"] != "ap-old");
            Ok(())
        }));
        add.await.unwrap();
        remove.await.unwrap();
        let doc = read_config(&path).await.unwrap();
        let names: Vec<_> = doc["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["name"].as_str())
            .collect();
        assert!(
            names.contains(&"ap-new") && !names.contains(&"ap-old"),
            "lost concurrent mutation: {names:?}"
        );
        assert_eq!(doc["other"], "preserved");
        let _ = tokio::fs::remove_file(path).await;
    }
}
