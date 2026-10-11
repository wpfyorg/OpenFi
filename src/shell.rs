//! The browser shell — off by default (`docs/api.md`'s "Browser shell" section, plan.md's
//! "Browser shell (opt-in, off by default)"). `POST /shell/token` is a step-up re-auth that
//! mints a one-time, 60-second token bound to the caller's session; `GET /shell/{node}` upgrades
//! to a WebSocket proxied to `ttyd` at `127.0.0.1:7681` on that node — gateway only for now
//! (APs get a clear 501, per the contract's own todo in plan.md).
//!
//! Every hard gate from the contract is enforced server-side, in this order, before the
//! upgrade: `shell.enabled` in config, HTTPS-or-loopback, a valid session, `Origin` matching
//! `Host`, and a valid unused token. This binary has no TLS, so in practice the HTTPS-or-
//! loopback gate only ever passes for loopback — i.e. only through an SSH tunnel, exactly as
//! plan.md documents. Runtime limits (10 min idle / 60 min hard cap) are enforced here, not by
//! ttyd. Keystrokes are never logged — only open/close events go to the audit log.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Extension, Json,
    extract::{
        ConnectInfo, Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::RwLock;

use crate::{
    AppState,
    audit::{AuditEntry, AuditResult},
    session::{Session, SessionToken},
};

const TOKEN_TTL_SECS: u64 = 60;
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const HARD_CAP: Duration = Duration::from_secs(60 * 60);
const TTYD_ADDR: &str = "127.0.0.1:7681";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

struct TokenEntry {
    session_token: String,
    expires: u64,
    used: bool,
}

#[derive(Default)]
pub struct ShellTokens(RwLock<Vec<(String, TokenEntry)>>);

impl ShellTokens {
    async fn issue(&self, session_token: &str) -> String {
        let token = crate::session::random_token().await;
        let mut v = self.0.write().await;
        v.retain(|(_, e)| e.expires > now()); // opportunistic sweep
        v.push((
            token.clone(),
            TokenEntry {
                session_token: session_token.to_owned(),
                expires: now() + TOKEN_TTL_SECS,
                used: false,
            },
        ));
        token
    }

    /// Consumes the token if it's valid, unexpired, unused, and bound to `session_token`.
    async fn consume(&self, token: &str, session_token: &str) -> bool {
        let mut v = self.0.write().await;
        let Some((_, entry)) = v.iter_mut().find(|(t, _)| t == token) else {
            return false;
        };
        if entry.used || entry.expires <= now() || entry.session_token != session_token {
            return false;
        }
        entry.used = true;
        true
    }
}

async fn audit(state: &AppState, user: &str, action: &str, node: &str, detail: &str) {
    state
        .audit
        .record(AuditEntry {
            ts: now(),
            user: user.to_owned(),
            action: action.to_owned(),
            target: node.to_owned(),
            result: AuditResult::Ok,
            detail: detail.to_owned(),
        })
        .await;
}

// -------------------------------------------------------------------------------------------
// POST /shell/token
// -------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub password: String,
}

/// Step-up re-auth: re-validates `password` against the gateway's rpcd (or, in mock mode, the
/// fixed `root`/`mock` pair) even though the caller already has a valid dashboard session —
/// opening a root shell shouldn't be possible just by riding a stolen cookie.
pub async fn issue_token(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Extension(SessionToken(session_token)): Extension<SessionToken>,
    Json(body): Json<TokenRequest>,
) -> Response {
    if !state.config.shell.enabled {
        return err(
            StatusCode::FORBIDDEN,
            "the browser shell is disabled (shell.enabled=false)",
        );
    }
    let ok = if state.mock.enabled() {
        session.user == "root" && body.password == "mock"
    } else {
        let gw = state.nodes.first();
        let Some((gw_nc, _)) = gw else {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "no gateway configured");
        };
        let ubus = crate::session::write_ubus_for(gw_nc, true, &session.user, &body.password);
        ubus.ensure_session().await.is_ok()
    };
    if !ok {
        return err(StatusCode::UNAUTHORIZED, "wrong password");
    }
    let token = state.shell_tokens.issue(&session_token).await;
    Json(json!({ "token": token, "expires_in": TOKEN_TTL_SECS })).into_response()
}

// -------------------------------------------------------------------------------------------
// GET /shell/{node}
// -------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ShellQuery {
    pub token: Option<String>,
}

fn origin_matches_host(headers: &HeaderMap) -> bool {
    let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    // Origin is "<scheme>://<host>[:<port>]"; compare just the host[:port] part.
    let origin_host = origin.split("://").nth(1).unwrap_or(origin);
    origin_host == host
}

// Every parameter is an axum extractor; there's no natural way to group them further.
#[allow(clippy::too_many_arguments)]
pub async fn shell_ws(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Extension(SessionToken(session_token)): Extension<SessionToken>,
    Path(node): Path<String>,
    Query(q): Query<ShellQuery>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !state.config.shell.enabled {
        return err(
            StatusCode::FORBIDDEN,
            "the browser shell is disabled (shell.enabled=false)",
        );
    }
    // No TLS in this binary (see the module doc comment), so this only ever passes for
    // loopback — i.e. through an SSH tunnel.
    if !peer.ip().is_loopback() {
        return err(
            StatusCode::FORBIDDEN,
            "shell access requires HTTPS or a loopback connection (use an SSH tunnel)",
        );
    }
    if !origin_matches_host(&headers) {
        return err(StatusCode::FORBIDDEN, "Origin does not match Host");
    }
    let Some(token) = &q.token else {
        return err(StatusCode::FORBIDDEN, "missing ?token=");
    };
    if !state.shell_tokens.consume(token, &session_token).await {
        return err(
            StatusCode::FORBIDDEN,
            "invalid, expired, or already-used token",
        );
    }
    let gateway_name = state
        .nodes
        .first()
        .map(|(n, _)| n.name.clone())
        .unwrap_or_default();
    if node != gateway_name && node != "gateway" {
        return err(
            StatusCode::NOT_IMPLEMENTED,
            "the browser shell only supports the gateway node for now",
        );
    }

    let user = session.user.clone();
    let mock = state.mock.enabled();
    audit(&state, &user, "shell_open", &node, "opened").await;
    ws.on_upgrade(move |socket| async move {
        let opened = std::time::Instant::now();
        if mock {
            run_echo(socket).await;
        } else {
            run_ttyd_proxy(socket).await;
        }
        audit(
            &state,
            &user,
            "shell_close",
            &node,
            &format!("closed after {:?}", opened.elapsed()),
        )
        .await;
    })
}

/// Mock mode: "proxy to a fake echo backend so the UI can be tested" (docs/api.md). No real
/// terminal, no real credentials — just bounces whatever the client sends back at it, subject to
/// the same idle/hard-cap limits as the real proxy.
async fn run_echo(mut socket: WebSocket) {
    let deadline = tokio::time::sleep(HARD_CAP);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            msg = tokio::time::timeout(IDLE_TIMEOUT, socket.recv()) => {
                match msg {
                    Ok(Some(Ok(Message::Text(t)))) => {
                        if socket.send(Message::Text(t)).await.is_err() { break; }
                    }
                    Ok(Some(Ok(Message::Binary(b)))) => {
                        if socket.send(Message::Binary(b)).await.is_err() { break; }
                    }
                    Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) | Err(_) => break,
                    _ => {}
                }
            }
        }
    }
}

/// Relays the browser's WebSocket to ttyd's own WebSocket at `127.0.0.1:7681/ws`, enforcing the
/// idle/hard-cap limits from `docs/api.md`/plan.md server-side (ttyd itself has no such limits).
async fn run_ttyd_proxy(mut socket: WebSocket) {
    let ttyd = match tokio_tungstenite::connect_async(format!("ws://{TTYD_ADDR}/ws")).await {
        Ok((s, _)) => s,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    format!("openwrtfi: could not reach ttyd at {TTYD_ADDR}: {e}").into(),
                ))
                .await;
            return;
        }
    };
    let (mut ttyd_tx, mut ttyd_rx) = ttyd.split();
    let deadline = tokio::time::sleep(HARD_CAP);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => break,
            from_browser = tokio::time::timeout(IDLE_TIMEOUT, socket.recv()) => {
                match from_browser {
                    Ok(Some(Ok(Message::Text(t)))) => {
                        if ttyd_tx.send(tokio_tungstenite::tungstenite::Message::Text(t.as_str().into())).await.is_err() { break; }
                    }
                    Ok(Some(Ok(Message::Binary(b)))) => {
                        if ttyd_tx.send(tokio_tungstenite::tungstenite::Message::Binary(b)).await.is_err() { break; }
                    }
                    Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) | Err(_) => break,
                    _ => {}
                }
            }
            from_ttyd = ttyd_rx.next() => {
                match from_ttyd {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                        if socket.send(Message::Text(t.as_str().into())).await.is_err() { break; }
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(b))) => {
                        if socket.send(Message::Binary(b)).await.is_err() { break; }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
        }
    }
    let _ = ttyd_tx.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn token_is_one_time_use() {
        let store = ShellTokens::default();
        let token = store.issue("sess1").await;
        assert!(store.consume(&token, "sess1").await);
        assert!(!store.consume(&token, "sess1").await); // already used
    }

    #[tokio::test]
    async fn token_is_bound_to_the_issuing_session() {
        let store = ShellTokens::default();
        let token = store.issue("sess1").await;
        assert!(!store.consume(&token, "sess2").await);
    }

    #[tokio::test]
    async fn expired_token_is_rejected() {
        let store = ShellTokens::default();
        let token = store.issue("sess1").await;
        {
            let mut v = store.0.write().await;
            v.iter_mut().find(|(t, _)| t == &token).unwrap().1.expires = now() - 1;
        }
        assert!(!store.consume(&token, "sess1").await);
    }

    #[test]
    fn origin_must_match_host() {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::HOST, "127.0.0.1:8088".parse().unwrap());
        h.insert(
            axum::http::header::ORIGIN,
            "http://127.0.0.1:8088".parse().unwrap(),
        );
        assert!(origin_matches_host(&h));

        h.insert(
            axum::http::header::ORIGIN,
            "http://evil.example:8088".parse().unwrap(),
        );
        assert!(!origin_matches_host(&h));
    }

    #[test]
    fn missing_origin_or_host_fails_closed() {
        let mut h = HeaderMap::new();
        assert!(!origin_matches_host(&h));
        h.insert(axum::http::header::HOST, "127.0.0.1:8088".parse().unwrap());
        assert!(!origin_matches_host(&h));
    }
}
