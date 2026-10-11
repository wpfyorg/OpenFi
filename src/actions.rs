//! `POST /clients/{mac}/{kick,reserve,whitelist}`, `POST /nodes/{node}/{reboot,wifi/reload}`,
//! `POST /nodes/{node}/radios/{radio}` — the single-node write actions from `docs/api.md`'s
//! Actions table. The multi-node WiFi rollout ("apply plan") is `src/plan.rs`.
//!
//! Every action here runs against the *caller's own* session-scoped `Ubus` (`Session.node_ubus`
//! — see `session.rs`), writes exactly one `audit.rs` entry (success or failure), and — in mock
//! mode — also mutates `AppState.mock` so the next snapshot visibly reflects it (whitelist
//! toggle, radio disabled), per `docs/api.md`'s "Mock mode" note.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    AppState,
    audit::{AuditEntry, AuditResult},
    collectors::uci,
    session::Session,
    validate,
};

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn audit(
    state: &AppState,
    session: &Session,
    action: &str,
    target: &str,
    result: Result<&str, &str>,
) {
    let (result, detail) = match result {
        Ok(d) => (AuditResult::Ok, d.to_owned()),
        Err(d) => (AuditResult::Error, d.to_owned()),
    };
    state
        .audit
        .record(AuditEntry {
            ts: now(),
            user: session.user.clone(),
            action: action.to_owned(),
            target: target.to_owned(),
            result,
            detail,
        })
        .await;
}

/// Find which node the client is currently associated to (from the live snapshot) and, if it's
/// WiFi, which `hostapd.<iface>` object it's on — kick needs the iface, not just the node.
async fn find_wifi_client(state: &AppState, mac: &str) -> Option<(String, String)> {
    let snap = state.snapshot.read().await;
    let c = snap.clients.iter().find(|c| c.mac == mac)?;
    if c.wired || !c.online {
        return None;
    }
    let node = c.node.clone()?;
    let ssid = c.ssid.clone()?;
    let node_snap = snap.nodes.iter().find(|n| n.name == node)?;
    let radio = node_snap
        .radios
        .iter()
        .find(|r| r.ssids.iter().any(|s| s == &ssid))?;
    let iface = radio.iface.clone()?;
    Some((node, iface))
}

// The `Response` error variant is large (it's the same type as the success path elsewhere in
// this file), but boxing it would just move the cost to every call site's `match`.
#[allow(clippy::result_large_err)]
fn node_ubus<'a>(
    state: &AppState,
    session: &'a Session,
    node: &str,
) -> Result<&'a crate::ubus::Ubus, Response> {
    if session.read_only.contains(node) {
        return Err(err(
            StatusCode::FORBIDDEN,
            format!("{node} is read-only for this session (its rpcd login failed at sign-in)"),
        ));
    }
    session.node_ubus.get(node).ok_or_else(|| {
        let known: Vec<&str> = state.nodes.iter().map(|(n, _)| n.name.as_str()).collect();
        err(
            StatusCode::BAD_REQUEST,
            format!("unknown node {node:?} (known nodes: {known:?})"),
        )
    })
}

// -------------------------------------------------------------------------------------------
// POST /clients/{mac}/kick
// -------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct KickBody {
    pub ban_secs: Option<u64>,
}

pub async fn kick(
    State(state): State<Arc<AppState>>,
    axum::Extension(session): axum::Extension<Arc<Session>>,
    Path(mac): Path<String>,
    body: Option<Json<KickBody>>,
) -> Response {
    let mac = validate::mac_lower(&mac);
    if !validate::mac_ok(&mac) {
        return err(StatusCode::BAD_REQUEST, "invalid MAC address");
    }
    let ban_secs = body.map(|b| b.0.ban_secs.unwrap_or(0)).unwrap_or(0);

    let Some((node, iface)) = find_wifi_client(&state, &mac).await else {
        audit(
            &state,
            &session,
            "kick",
            &mac,
            Err("not associated to any AP"),
        )
        .await;
        return err(
            StatusCode::NOT_FOUND,
            "client is not currently associated over WiFi",
        );
    };
    let ubus = match node_ubus(&state, &session, &node) {
        Ok(u) => u,
        Err(resp) => {
            audit(
                &state,
                &session,
                "kick",
                &mac,
                Err("node is read-only for this session"),
            )
            .await;
            return resp;
        }
    };

    // Mock mode: fixture `Ubus` calls just read static JSON off disk, and there's nothing
    // meaningful to fake for a `hostapd del_client` reply — treat it as an immediate success
    // instead, per `docs/api.md`'s "Mock mode" note ("writes succeed without touching any
    // router").
    let result = if state.mock.enabled() {
        Ok(json!({}))
    } else {
        ubus.call(
            &format!("hostapd.{iface}"),
            "del_client",
            json!({
                "addr": mac,
                "reason": 5,
                "deauth": true,
                "ban_time": ban_secs * 1000,
            }),
        )
        .await
    };

    match result {
        Ok(_) => {
            audit(
                &state,
                &session,
                "kick",
                &mac,
                Ok(&format!("via {node}/{iface}, ban_secs={ban_secs}")),
            )
            .await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => {
            audit(&state, &session, "kick", &mac, Err(&e)).await;
            err(StatusCode::BAD_GATEWAY, e)
        }
    }
}

// -------------------------------------------------------------------------------------------
// POST /clients/{mac}/reserve
// -------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ReserveBody {
    pub ip: String,
    pub name: String,
}

pub async fn reserve(
    State(state): State<Arc<AppState>>,
    axum::Extension(session): axum::Extension<Arc<Session>>,
    Path(mac): Path<String>,
    Json(body): Json<ReserveBody>,
) -> Response {
    let mac = validate::mac_lower(&mac);
    if !validate::mac_ok(&mac) {
        return err(StatusCode::BAD_REQUEST, "invalid MAC address");
    }
    if !validate::ip_in_lan(&body.ip) {
        return err(StatusCode::BAD_REQUEST, "ip must be in 192.168.1.0/24");
    }
    if !validate::reservation_name_ok(&body.name) {
        return err(
            StatusCode::BAD_REQUEST,
            "name must be 1-63 chars, [A-Za-z0-9_-]",
        );
    }
    let warning = validate::ip_in_dhcp_pool(&body.ip)
        .then(|| "warning: this IP is inside the dynamic DHCP pool (.100-.239)".to_owned());

    // DHCP config lives on the gateway only.
    let gateway = state
        .nodes
        .first()
        .map(|(n, _)| n.name.clone())
        .unwrap_or_default();
    let ubus = match node_ubus(&state, &session, &gateway) {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    let target = format!("{mac} -> {} ({})", body.ip, body.name);
    // Mock mode: fixtures have no `uci add`/`apply`/`confirm` replies to read — treat it as an
    // immediate success (see the kick handler's identical note).
    let outcome: Result<(), String> = if state.mock.enabled() {
        Ok(())
    } else {
        async {
            uci::add(
                ubus,
                "dhcp",
                "host",
                json!({ "mac": mac, "ip": body.ip, "name": body.name }),
            )
            .await?;
            uci::apply_confirm(ubus, 30).await
        }
        .await
    };

    match outcome {
        Ok(()) => {
            audit(
                &state,
                &session,
                "reserve",
                &target,
                Ok(warning.as_deref().unwrap_or("ok")),
            )
            .await;
            Json(json!({ "ok": true, "warning": warning })).into_response()
        }
        Err(e) => {
            audit(&state, &session, "reserve", &target, Err(&e)).await;
            err(StatusCode::BAD_GATEWAY, e)
        }
    }
}

// -------------------------------------------------------------------------------------------
// POST /clients/{mac}/whitelist
// -------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct WhitelistBody {
    pub on: bool,
}

pub async fn whitelist(
    State(state): State<Arc<AppState>>,
    axum::Extension(session): axum::Extension<Arc<Session>>,
    Path(mac): Path<String>,
    Json(body): Json<WhitelistBody>,
) -> Response {
    let mac = validate::mac_lower(&mac);
    if !validate::mac_ok(&mac) {
        return err(StatusCode::BAD_REQUEST, "invalid MAC address");
    }
    let gateway = state
        .nodes
        .first()
        .map(|(n, _)| n.name.clone())
        .unwrap_or_default();
    let ubus = match node_ubus(&state, &session, &gateway) {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let state_str = if body.on { "on" } else { "off" };
    let target = format!("{mac} -> {state_str}");

    // Mock mode: no fixture models `qos-tiers`'s reply — succeed immediately (see the kick
    // handler's identical note).
    let result = if state.mock.enabled() {
        Ok(json!({}))
    } else {
        ubus.call(
            "file",
            "exec",
            json!({ "command": "/usr/sbin/qos-tiers", "params": ["whitelist", mac, state_str] }),
        )
        .await
    };

    match result {
        Ok(_) => {
            state.mock.set_whitelist(&mac, body.on).await;
            audit(&state, &session, "whitelist", &target, Ok("ok")).await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => {
            audit(&state, &session, "whitelist", &target, Err(&e)).await;
            err(StatusCode::BAD_GATEWAY, e)
        }
    }
}

// -------------------------------------------------------------------------------------------
// POST /nodes/{node}/reboot
// -------------------------------------------------------------------------------------------

pub async fn reboot(
    State(state): State<Arc<AppState>>,
    axum::Extension(session): axum::Extension<Arc<Session>>,
    Path(node): Path<String>,
) -> Response {
    let ubus = match node_ubus(&state, &session, &node) {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    // Mock mode: don't actually try to exec a fixture file for this — just record it. A real
    // reboot obviously also never returns a normal ubus reply (the box goes down mid-call).
    let result = if state.mock.enabled() {
        Ok(())
    } else {
        ubus.call("system", "reboot", json!({})).await.map(|_| ())
    };
    match result {
        Ok(()) => {
            audit(&state, &session, "reboot", &node, Ok("ok")).await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => {
            audit(&state, &session, "reboot", &node, Err(&e)).await;
            err(StatusCode::BAD_GATEWAY, e)
        }
    }
}

// -------------------------------------------------------------------------------------------
// POST /nodes/{node}/wifi/reload
// -------------------------------------------------------------------------------------------

pub async fn wifi_reload(
    State(state): State<Arc<AppState>>,
    axum::Extension(session): axum::Extension<Arc<Session>>,
    Path(node): Path<String>,
) -> Response {
    let ubus = match node_ubus(&state, &session, &node) {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    // `network.wireless reload` restarts just the wifi stack (hostapd/wpa_supplicant/mesh),
    // which is what LuCI's Wireless page's "Restart" button does — a plain `network reload`
    // would also cycle wired interfaces/DHCP for no reason. Documented here since docs/api.md
    // leaves the exact call up to the implementation ("network.wireless reload").
    let result = if state.mock.enabled() {
        Ok(json!({}))
    } else {
        ubus.call("network.wireless", "reload", json!({})).await
    };
    match result {
        Ok(_) => {
            audit(&state, &session, "wifi_reload", &node, Ok("ok")).await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => {
            audit(&state, &session, "wifi_reload", &node, Err(&e)).await;
            err(StatusCode::BAD_GATEWAY, e)
        }
    }
}

// -------------------------------------------------------------------------------------------
// POST /nodes/{node}/radios/{radio}
// -------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct RadioBody {
    pub disabled: Option<bool>,
    pub channel: Option<ChannelValue>,
    pub txpower_dbm: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ChannelValue {
    Auto(AutoTag),
    Number(u32),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoTag {
    Auto,
}

pub async fn set_radio(
    State(state): State<Arc<AppState>>,
    axum::Extension(session): axum::Extension<Arc<Session>>,
    Path((node, radio)): Path<(String, String)>,
    Json(body): Json<RadioBody>,
) -> Response {
    let ubus = match node_ubus(&state, &session, &node) {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    let is_5ghz = {
        let snap = state.snapshot.read().await;
        snap.nodes
            .iter()
            .find(|n| n.name == node)
            .and_then(|n| n.radios.iter().find(|r| r.name == radio))
            .map(|r| matches!(r.band, crate::model::Band::Ghz5))
            .unwrap_or(false)
    };

    let mut values = serde_json::Map::new();
    if let Some(disabled) = body.disabled {
        values.insert("disabled".into(), json!(if disabled { "1" } else { "0" }));
    }
    match &body.channel {
        Some(ChannelValue::Auto(_)) => {
            values.insert("channel".into(), json!("auto"));
        }
        Some(ChannelValue::Number(ch)) => {
            if !validate::channel_ok(is_5ghz, *ch) {
                return err(
                    StatusCode::BAD_REQUEST,
                    "channel out of range for this band",
                );
            }
            values.insert("channel".into(), json!(ch.to_string()));
        }
        None => {}
    }
    if let Some(dbm) = body.txpower_dbm {
        if !validate::txpower_ok(dbm) {
            return err(StatusCode::BAD_REQUEST, "txpower_dbm must be 0-30");
        }
        values.insert("txpower".into(), json!(dbm.to_string()));
    }
    if values.is_empty() {
        return err(StatusCode::BAD_REQUEST, "no fields to change");
    }

    let channel_num = match &body.channel {
        Some(ChannelValue::Number(c)) => Some(*c),
        _ => None,
    };
    let target = format!("{node}/{radio}");
    // Mock mode: same as `reserve` — no fixture models `uci set`/`apply`/`confirm` replies.
    let outcome: Result<(), String> = if state.mock.enabled() {
        Ok(())
    } else {
        async {
            uci::set(ubus, "wireless", &radio, &values).await?;
            uci::apply_confirm(ubus, 30).await
        }
        .await
    };

    match outcome {
        Ok(()) => {
            state
                .mock
                .set_radio(&node, &radio, body.disabled, channel_num, body.txpower_dbm)
                .await;
            audit(&state, &session, "radio_set", &target, Ok("ok")).await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => {
            audit(&state, &session, "radio_set", &target, Err(&e)).await;
            err(StatusCode::BAD_GATEWAY, e)
        }
    }
}
