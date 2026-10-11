use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Query, State},
    http::{Request, StatusCode, Uri, header},
    middleware,
    middleware::Next,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio_stream::wrappers::WatchStream;

use crate::{
    AppState, actions, auth, collectors,
    diag::{self, DiagRequest},
    model::RatePoint,
    nodes_api, plan, session, shell, web,
};

/// Every `/api/v1` route (except `POST /auth/login` and `GET /health`) needs a session cookie;
/// every non-GET also needs `X-OpenWrtFi: 1` — enforced by `session::auth_mw`, applied last (see
/// its own comment on why `.layer()` must come after every `.route()`/`.fallback()` call) so it
/// also covers the `fallback` route below, which is how an unmatched `/api/v1/...` path still
/// gets a 401 before it gets a 404.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/health", get(|| async { "ok" }))
        .route("/api/v1/snapshot", get(snapshot))
        .route("/api/v1/stream", get(stream))
        .route("/api/v1/history", get(history))
        .route("/api/v1/usage/daily", get(usage_daily))
        .route("/api/v1/wifi/networks", get(wifi_networks))
        .route("/api/v1/logs", get(logs))
        .route("/api/v1/audit", get(audit))
        .route("/api/v1/diag", post(diag_handler))
        // Persistent controller node configuration
        .route("/api/v1/nodes", get(nodes_api::list).post(nodes_api::add))
        .route("/api/v1/nodes/{node}/remove", post(nodes_api::remove))
        // Auth
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/me", get(auth::me))
        .route("/api/v1/auth/logout", post(auth::logout))
        // Actions
        .route("/api/v1/clients/{mac}/kick", post(actions::kick))
        .route("/api/v1/clients/{mac}/reserve", post(actions::reserve))
        .route("/api/v1/clients/{mac}/whitelist", post(actions::whitelist))
        .route("/api/v1/nodes/{node}/reboot", post(actions::reboot))
        .route(
            "/api/v1/nodes/{node}/wifi/reload",
            post(actions::wifi_reload),
        )
        .route(
            "/api/v1/nodes/{node}/radios/{radio}",
            post(actions::set_radio),
        )
        // Multi-node WiFi plan
        .route("/api/v1/wifi/networks/{id}/plan", post(plan::create_plan))
        .route("/api/v1/wifi/plans/{id}/apply", post(plan::apply_plan))
        .route("/api/v1/wifi/plans/{id}", get(plan::get_plan))
        // Browser shell
        .route("/api/v1/shell/token", post(shell::issue_token))
        .route("/api/v1/shell/{node}", get(shell::shell_ws))
        .fallback(fallback)
        .layer(middleware::from_fn(normalize_api_errors))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            session::auth_mw,
        ))
        .with_state(state)
}

/// Normalize framework-generated API rejections (which occur before a handler can format an
/// error) to the documented JSON shape. Handler responses and non-API SPA responses pass through.
async fn normalize_api_errors(request: Request<Body>, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let response = next.run(request).await;
    if !is_api || !response.status().is_client_error() {
        return response;
    }
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
    {
        return response;
    }

    let status = response.status();
    let (mut parts, body) = response.into_parts();
    let message = to_bytes(body, 64 * 1024)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_default();
    let message = if message.is_empty() {
        status
            .canonical_reason()
            .unwrap_or("request failed")
            .to_owned()
    } else {
        message
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    let body = serde_json::to_vec(&json!({ "error": message })).unwrap_or_default();
    Response::from_parts(parts, Body::from(body))
}

/// Unmatched `/api/*` → `404 {"error": "not found"}` (never the SPA's `index.html`); anything
/// else falls back to the embedded static assets / SPA shell.
async fn fallback(uri: Uri) -> Response {
    if uri.path().starts_with("/api/") {
        (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
    } else {
        web::static_handler(uri).await
    }
}

fn bad_request(msg: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": msg.into() })),
    )
        .into_response()
}

async fn snapshot(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    Json((**s.snapshot.read().await).clone())
}

/// Server-sent events: one full snapshot per poll. Snapshots are a few KB, so
/// deltas aren't worth the complexity yet.
async fn stream(
    State(s): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let st = s.clone();
    let events = WatchStream::new(s.tick.clone()).then(move |_| {
        let st = st.clone();
        async move {
            let snap = st.snapshot.read().await.clone();
            Ok(Event::default()
                .event("snapshot")
                .json_data(&*snap)
                .unwrap_or_default())
        }
    });
    Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[derive(Deserialize)]
struct HistoryQuery {
    range: Option<String>,
}

/// `?range=24h` (default) reads the current snapshot's own 24h/30-min series (already kept in
/// RAM for the dashboard); `?range=7d` reads the separate 3h-bucket ring.
async fn history(State(s): State<Arc<AppState>>, Query(q): Query<HistoryQuery>) -> Response {
    match q.range.as_deref() {
        Some("7d") => Json(s.history_7d.read().await.clone()).into_response(),
        Some("24h") | None => {
            let points: Vec<RatePoint> = s.snapshot.read().await.wan.history.clone();
            Json(points).into_response()
        }
        Some(other) => bad_request(format!("unknown range {other:?}, want \"24h\" or \"7d\"")),
    }
}

#[derive(Deserialize)]
struct UsageDailyQuery {
    days: Option<u32>,
}

/// Slices the in-memory daily rollup (`AppState.daily_usage`, kept fresh by
/// `collectors::usage_daily::DailyTracker`) down to the requested window. Days before this
/// binary was ever running (or otherwise missing) are simply absent — the contract's "empty
/// state", not a zero-filled row.
async fn usage_daily(State(s): State<Arc<AppState>>, Query(q): Query<UsageDailyQuery>) -> Response {
    let days = q.days.unwrap_or(30).max(1) as i64;
    let today_ts = s.snapshot.read().await.ts;
    let earliest =
        collectors::date::window_start_day(today_ts, days as u32, s.config.local_utc_offset_secs);
    let rows: Vec<_> = s
        .daily_usage
        .read()
        .await
        .iter()
        .filter(|r| r.day.as_str() >= earliest.as_str())
        .cloned()
        .collect();
    Json(rows).into_response()
}

async fn wifi_networks(State(s): State<Arc<AppState>>) -> Response {
    let clients = s.snapshot.read().await.clients.clone();
    Json(collectors::wifi_networks::collect(&s.nodes, &clients).await).into_response()
}

#[derive(Deserialize)]
struct LogsQuery {
    node: Option<String>,
    lines: Option<u32>,
}

async fn logs(State(s): State<Arc<AppState>>, Query(q): Query<LogsQuery>) -> Response {
    let Some(gateway_name) = s.nodes.first().map(|(n, _)| n.name.clone()) else {
        return bad_request("no nodes configured");
    };
    let node_name = q.node.unwrap_or(gateway_name);
    let Some((_, ubus)) = s.nodes.iter().find(|(n, _)| n.name == node_name) else {
        return bad_request(format!("unknown node {node_name:?}"));
    };
    let lines = q.lines.unwrap_or(200).clamp(1, 5000);
    Json(collectors::collect_logs(ubus, &node_name, lines).await).into_response()
}

async fn audit(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    Json(s.audit.list().await)
}

async fn diag_handler(State(s): State<Arc<AppState>>, Json(req): Json<DiagRequest>) -> Response {
    if !diag::valid_host(&req.host) {
        return bad_request("host must match ^[A-Za-z0-9.:-]{1,253}$");
    }
    let gateway_name = s.nodes.first().map(|(n, _)| n.name.as_str()).unwrap_or("");
    if let Some(node) = &req.node
        && node != gateway_name
        && node != "gateway"
    {
        return bad_request(format!(
            "diagnostics only run on the gateway ({gateway_name}) for now"
        ));
    }
    Json(diag::run(req.tool, &req.host).await).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn unauthenticated_unmatched_api_path_is_401_not_404() {
        let state = crate::test_support::minimal_app_state();
        let app = router(state);
        let req = axum::http::Request::builder()
            .uri("/api/v1/this-route-does-not-exist")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_unmatched_api_path_is_404_json_not_the_spa() {
        let state = crate::test_support::minimal_app_state();
        let token = state
            .sessions
            .create("root".into(), Default::default(), Default::default())
            .await;
        let app = router(state);
        let req = axum::http::Request::builder()
            .uri("/api/v1/this-route-does-not-exist")
            .header(axum::http::header::COOKIE, format!("owfi_session={token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "not found");
    }

    #[tokio::test]
    async fn non_api_unmatched_path_still_falls_back_to_the_spa_shell() {
        let state = crate::test_support::minimal_app_state();
        let app = router(state);
        let req = axum::http::Request::builder()
            .uri("/some/client-side/route")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // No auth required outside /api/v1, and it's the SPA handler (index.html, or its
        // "UI not built" notice when web/dist is empty), never the API's JSON 404.
        assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap_or("").to_owned());
        assert!(!ct.unwrap_or_default().contains("application/json"));
    }

    #[tokio::test]
    async fn health_and_login_need_no_session_but_login_still_needs_the_csrf_header() {
        let state = crate::test_support::minimal_app_state();
        let app = router(state);

        let health_req = axum::http::Request::builder()
            .uri("/api/v1/health")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(health_req).await.unwrap().status(),
            StatusCode::OK
        );

        // POST without X-OpenWrtFi: 1 is rejected before the session/credential check runs.
        let login_req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"username":"root","password":"mock"}"#))
            .unwrap();
        assert_eq!(
            app.oneshot(login_req).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn daily_usage_window_uses_configured_local_day_near_midnight() {
        let mut state = crate::test_support::minimal_app_state();
        Arc::get_mut(&mut state)
            .expect("unique test state")
            .config
            .local_utc_offset_secs = 5 * 3600 + 1800;
        let day = collectors::date::days_from_civil(2026, 9, 24);
        let local_midnight_utc = (day * 86400 - state.config.local_utc_offset_secs) as u64;
        *state.snapshot.write().await = Arc::new(crate::model::Snapshot {
            ts: local_midnight_utc + 1,
            ..Default::default()
        });
        *state.daily_usage.write().await = vec![
            crate::model::DailyUsage {
                day: "2026-09-23".into(),
                rx_bytes: 10,
                tx_bytes: 0,
                devices: 0,
            },
            crate::model::DailyUsage {
                day: "2026-09-24".into(),
                rx_bytes: 20,
                tx_bytes: 0,
                devices: 0,
            },
        ];
        let token = state
            .sessions
            .create("root".into(), Default::default(), Default::default())
            .await;
        let app = router(state);
        let req = axum::http::Request::builder()
            .uri("/api/v1/usage/daily?days=1")
            .header(axum::http::header::COOKIE, format!("owfi_session={token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let rows: Vec<crate::model::DailyUsage> = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.day.as_str()).collect::<Vec<_>>(),
            ["2026-09-24"]
        );
    }

    #[tokio::test]
    async fn api_extractor_rejections_are_json_and_keep_their_status() {
        let state = crate::test_support::minimal_app_state();
        let token = state
            .sessions
            .create("root".into(), Default::default(), Default::default())
            .await;
        let app = router(state);
        let cases = [
            ("{", Some("application/json"), StatusCode::BAD_REQUEST),
            (
                r#"{"tool":"ping","host":123}"#,
                Some("application/json"),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                r#"{"tool":"ping","host":"127.0.0.1"}"#,
                Some("text/plain"),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                r#"{"tool":"ping","host":"127.0.0.1"}"#,
                None,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
        ];
        for (body, content_type, expected_status) in cases {
            let mut builder = axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/diag")
                .header(axum::http::header::COOKIE, format!("owfi_session={token}"))
                .header("X-OpenWrtFi", "1");
            if let Some(content_type) = content_type {
                builder = builder.header(axum::http::header::CONTENT_TYPE, content_type);
            }
            let req = builder.body(Body::from(body)).unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), expected_status);
            assert!(
                resp.headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("application/json")
            );
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(value["error"].as_str().is_some());
        }
    }

    #[tokio::test]
    async fn existing_api_errors_remain_json_and_spa_fallback_is_unchanged() {
        let state = crate::test_support::minimal_app_state();
        let token = state
            .sessions
            .create("root".into(), Default::default(), Default::default())
            .await;
        let app = router(state);

        let cases = [
            ("/api/v1/missing", None, None, StatusCode::UNAUTHORIZED),
            (
                "/api/v1/diag",
                Some("POST"),
                Some((None, "{}")),
                StatusCode::FORBIDDEN,
            ),
            (
                "/api/v1/missing",
                Some("GET"),
                Some((Some("X-OpenWrtFi"), "")),
                StatusCode::NOT_FOUND,
            ),
            (
                "/api/v1/nodes",
                Some("POST"),
                Some((Some("X-OpenWrtFi"), r#"{"name":"ap-x","host":"192.0.2.2"}"#)),
                StatusCode::CONFLICT,
            ),
        ];
        for (uri, method, write, expected_status) in cases {
            let mut builder = axum::http::Request::builder().uri(uri);
            if let Some(method) = method {
                builder = builder.method(method);
            }
            if uri != "/api/v1/missing" || expected_status != StatusCode::UNAUTHORIZED {
                builder =
                    builder.header(axum::http::header::COOKIE, format!("owfi_session={token}"));
            }
            let req = if let Some((csrf_header, body)) = write {
                if let Some(name) = csrf_header {
                    builder = builder.header(name, "1");
                }
                builder
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap()
            } else {
                builder.body(Body::empty()).unwrap()
            };
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), expected_status);
            assert!(
                resp.headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("application/json")
            );
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(value["error"].as_str().is_some());
        }

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/client-side/path")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            !resp
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
    }
}
