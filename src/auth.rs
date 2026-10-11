//! `POST /auth/login`, `GET /auth/me`, `POST /auth/logout` — see `docs/api.md`'s Auth section
//! and `session.rs` for the session store / cookie / rate-limit mechanics this wires together.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    AppState,
    session::{self, Session, SessionToken},
};

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Mock mode: only `root`/`mock` succeeds (docs/api.md). Real mode: `session login` against the
/// gateway's own rpcd through the private loopback bridge at `127.0.0.1:8089` (never the `ubus`
/// CLI — that would put the password in argv/`ps`), then best-effort the same against every AP;
/// an AP failure doesn't fail login, it just marks that node read-only for the session
/// (`docs/api.md`'s Auth section).
pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<LoginRequest>,
) -> Response {
    let ip = peer.ip();
    if let Err(retry_after) = state.sessions.login_attempt_allowed(ip).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after.to_string())],
            Json(json!({ "error": "too many failed logins, try again later" })),
        )
            .into_response();
    }

    let (node_ubus, read_only) = if state.mock.enabled() {
        if body.username != "root" || body.password != "mock" {
            state.sessions.note_login_failure(ip).await;
            return err(StatusCode::UNAUTHORIZED, "invalid credentials");
        }
        // Fixture `Ubus` handles never fail to authenticate, so every configured node is
        // writable in mock mode — reuse the poll loop's own handles directly.
        let map: HashMap<String, crate::ubus::Ubus> = state
            .nodes
            .iter()
            .map(|(n, u)| (n.name.clone(), u.clone()))
            .collect();
        (map, HashSet::new())
    } else {
        let Some((gw_nc, _)) = state.nodes.first() else {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "no gateway configured");
        };
        let gw_ubus = session::write_ubus_for(gw_nc, true, &body.username, &body.password);
        if let Err(e) = gw_ubus.ensure_session().await {
            if e.ends_with(": login failed") {
                state.sessions.note_login_failure(ip).await;
                return err(StatusCode::UNAUTHORIZED, "invalid credentials");
            }
            log::error!("login: gateway rpcd authentication unavailable: {e}");
            return err(
                StatusCode::BAD_GATEWAY,
                "router authentication service unavailable",
            );
        }
        let mut map = HashMap::new();
        map.insert(gw_nc.name.clone(), gw_ubus);
        let mut read_only = HashSet::new();
        for (nc, _) in state.nodes.iter().skip(1) {
            let ap_ubus = session::write_ubus_for(nc, false, &body.username, &body.password);
            if ap_ubus.ensure_session().await.is_ok() {
                map.insert(nc.name.clone(), ap_ubus);
            } else {
                log::warn!(
                    "login: {} rpcd login failed for {}, marking read-only",
                    nc.name,
                    body.username
                );
                read_only.insert(nc.name.clone());
            }
        }
        (map, read_only)
    };

    state.sessions.note_login_success(ip).await;
    let token = state
        .sessions
        .create(body.username.clone(), node_ubus, read_only)
        .await;
    (
        StatusCode::OK,
        [(header::SET_COOKIE, session::set_cookie_header(&token))],
        Json(json!({ "user": body.username })),
    )
        .into_response()
}

pub async fn me(Extension(session): Extension<Arc<Session>>) -> Response {
    Json(json!({ "user": session.user, "expires": session.expires_at() })).into_response()
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Extension(SessionToken(token)): Extension<SessionToken>,
) -> Response {
    for ubus in session.node_ubus.values() {
        ubus.destroy_session().await;
    }
    state.sessions.remove(&token).await;
    (
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, session::clear_cookie_header())],
    )
        .into_response()
}
