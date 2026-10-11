//! Session store, cookie plumbing, login rate limiting, and the auth/CSRF middleware.
//!
//! A session token is 32 random bytes from `/dev/urandom`, hex-encoded (no `rand` crate — see
//! `random_token`). Sessions live only in memory (`SessionStore`) and expire after 30 minutes of
//! idle time (`touch`, called by the auth middleware on every authenticated request).
//!
//! Per `docs/api.md`'s Auth section: writes to any node go through *that user's own* rpcd
//! session (so rpcd ACLs and uci's session-scoped staging apply), not the read-only polling
//! credentials in `Config`. `Session.node_ubus` holds one such `Ubus` per node, built at login
//! time from the submitted username/password (`write_ubus_for`) — separate from
//! `AppState.nodes`, which are the poll loop's own read-only handles.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Write as _,
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, RwLock},
};

use crate::{config::NodeConfig, ubus::Ubus};

pub const COOKIE_NAME: &str = "owfi_session";
/// The raw session-cookie value, inserted into request extensions by `auth_mw` alongside the
/// resolved `Arc<Session>`, for the handful of handlers that need it directly (`shell.rs`'s
/// step-up token, which is bound to the *token string*, not the session object).
#[derive(Clone)]
pub struct SessionToken(pub String);
const IDLE_SECS: u64 = 30 * 60;
const MAX_LOGIN_FAILS: usize = 5;
const LOGIN_WINDOW_SECS: u64 = 60;

pub struct Session {
    pub user: String,
    pub created: u64,
    last_seen: AtomicU64,
    /// Per-node, session-scoped write handles (see the module doc comment). Only nodes the user
    /// successfully authenticated to are present.
    pub node_ubus: HashMap<String, Ubus>,
    /// Nodes present in `node_ubus`'s *config* but whose AP login failed — read-only for this
    /// session; actions against them should be refused with a clear error rather than silently
    /// falling back to a different transport.
    pub read_only: HashSet<String>,
}

impl Session {
    pub fn touch(&self) {
        self.last_seen.store(now_secs(), Ordering::Relaxed);
    }

    pub fn expires_at(&self) -> u64 {
        self.last_seen.load(Ordering::Relaxed) + IDLE_SECS
    }
}

#[derive(Default)]
pub struct SessionStore {
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    fails: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl SessionStore {
    pub async fn create(
        &self,
        user: String,
        node_ubus: HashMap<String, Ubus>,
        read_only: HashSet<String>,
    ) -> String {
        let token = random_token().await;
        let now = now_secs();
        let session = Arc::new(Session {
            user,
            created: now,
            last_seen: AtomicU64::new(now),
            node_ubus,
            read_only,
        });
        self.sessions.write().await.insert(token.clone(), session);
        token
    }

    /// Looks up a session by token, expiring (and removing) it if idle too long. Touches
    /// `last_seen` on a hit, per the 30-minute idle-expiry rule.
    pub async fn touch(&self, token: &str) -> Option<Arc<Session>> {
        {
            let sessions = self.sessions.read().await;
            let s = sessions.get(token)?;
            if now_secs().saturating_sub(s.last_seen.load(Ordering::Relaxed)) <= IDLE_SECS {
                s.touch();
                return Some(s.clone());
            }
        }
        self.sessions.write().await.remove(token);
        None
    }

    pub async fn remove(&self, token: &str) -> Option<Arc<Session>> {
        self.sessions.write().await.remove(token)
    }

    /// `Err(retry_after_secs)` if this IP has already hit the failure cap in the current
    /// 60-second window; call before attempting credential validation so a client that's
    /// already rate-limited doesn't get to try (and doesn't reset its own window).
    pub async fn login_attempt_allowed(&self, ip: IpAddr) -> Result<(), u64> {
        let mut fails = self.fails.lock().await;
        let now = Instant::now();
        if let Some(q) = fails.get_mut(&ip) {
            q.retain(|t| now.duration_since(*t) < Duration::from_secs(LOGIN_WINDOW_SECS));
            if q.len() >= MAX_LOGIN_FAILS {
                let oldest = q.front().copied().unwrap_or(now);
                let elapsed = now.duration_since(oldest).as_secs();
                return Err(LOGIN_WINDOW_SECS.saturating_sub(elapsed).max(1));
            }
        }
        Ok(())
    }

    pub async fn note_login_failure(&self, ip: IpAddr) {
        self.fails
            .lock()
            .await
            .entry(ip)
            .or_default()
            .push_back(Instant::now());
    }

    pub async fn note_login_success(&self, ip: IpAddr) {
        self.fails.lock().await.remove(&ip);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 32 random bytes from `/dev/urandom`, hex-encoded — no `rand` crate (see the size budget in
/// plan.md). `/dev/urandom` never blocks once seeded, which is always true long before a router
/// finishes booting network services, so this is a plain async read, not a fallback chain.
pub async fn random_token() -> String {
    let mut f = tokio::fs::File::open("/dev/urandom")
        .await
        .expect("open /dev/urandom");
    let mut buf = [0u8; 32];
    f.read_exact(&mut buf).await.expect("read /dev/urandom");
    let mut s = String::with_capacity(64);
    for b in buf {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// A per-node, session-scoped `Ubus` for *write* calls, built from the credentials the user just
/// logged in with — never the read-only polling credentials in `Config`. The gateway is always
/// reached over the controller-owned rpcd bridge at `127.0.0.1:8089` (not the local `ubus` CLI,
/// even though the poll loop may use `Cli` for it), so `uci apply {rollback:true}` is scoped to
/// this session, and so the password never touches argv/`ps`. Mock nodes (fixtures configured)
/// ignore credentials entirely, same as the read-only poll-side handles, since there's nothing
/// to authenticate against.
pub fn write_ubus_for(nc: &NodeConfig, is_gateway: bool, username: &str, password: &str) -> Ubus {
    if let Some(dir) = &nc.fixtures {
        return Ubus::fixture(dir.clone());
    }
    let host = if is_gateway {
        "127.0.0.1:8089"
    } else {
        nc.host.as_str()
    };
    Ubus::rpcd(
        host.to_owned(),
        Some(username.to_owned()),
        Some(password.to_owned()),
    )
}

pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|kv| {
        let kv = kv.trim();
        let (k, v) = kv.split_once('=')?;
        (k == name).then(|| v.to_owned())
    })
}

pub fn set_cookie_header(token: &str) -> String {
    format!("{COOKIE_NAME}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={IDLE_SECS}")
}

pub fn clear_cookie_header() -> String {
    format!("{COOKIE_NAME}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
}

/// The CSRF guard: every non-GET request needs `X-OpenWrtFi: 1`. A pure function so it's
/// trivially unit-testable without spinning up a router.
pub fn has_csrf_header(headers: &HeaderMap) -> bool {
    headers
        .get("X-OpenWrtFi")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "1")
}

fn json_err(code: StatusCode, msg: &str) -> Response {
    (code, axum::Json(json!({ "error": msg }))).into_response()
}

/// The auth + CSRF middleware, wrapping every `/api/v1` route: `POST /auth/login` and
/// `GET /health` skip the session check (but *not* the CSRF header check — the contract doesn't
/// exempt login from it); every other non-GET needs `X-OpenWrtFi: 1`, and every other route
/// needs a live session cookie. On success, both the resolved session and the raw cookie value
/// are placed in request extensions for handlers to pick up.
pub async fn auth_mw(
    State(state): State<Arc<crate::AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_owned();
    // Static assets and the SPA shell (everything outside `/api/v1`) need no session — a
    // logged-out browser still has to be able to load the login page's JS.
    if !path.starts_with("/api/v1/") {
        return next.run(req).await;
    }
    let is_login = path == "/api/v1/auth/login" && req.method() == Method::POST;
    let is_health = path == "/api/v1/health";

    if req.method() != Method::GET && !has_csrf_header(req.headers()) {
        return json_err(StatusCode::FORBIDDEN, "missing X-OpenWrtFi header");
    }
    if is_login || is_health {
        return next.run(req).await;
    }

    let Some(token) = cookie(req.headers(), COOKIE_NAME) else {
        return json_err(StatusCode::UNAUTHORIZED, "missing session cookie");
    };
    let Some(session) = state.sessions.touch(&token).await else {
        return json_err(StatusCode::UNAUTHORIZED, "session expired or invalid");
    };
    req.extensions_mut().insert(session);
    req.extensions_mut().insert(SessionToken(token));
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[tokio::test]
    async fn create_and_touch_round_trips() {
        let store = SessionStore::default();
        let token = store
            .create("root".into(), HashMap::new(), HashSet::new())
            .await;
        let s = store.touch(&token).await.unwrap();
        assert_eq!(s.user, "root");
        assert_eq!(token.len(), 64); // 32 bytes, hex-encoded
    }

    #[tokio::test]
    async fn unknown_token_is_none() {
        let store = SessionStore::default();
        assert!(store.touch("deadbeef").await.is_none());
    }

    #[tokio::test]
    async fn expired_session_is_removed_on_touch() {
        let store = SessionStore::default();
        let token = store
            .create("root".into(), HashMap::new(), HashSet::new())
            .await;
        {
            let sessions = store.sessions.read().await;
            let s = sessions.get(&token).unwrap();
            // Force last_seen far enough in the past to be past the 30-minute idle window.
            s.last_seen
                .store(now_secs() - IDLE_SECS - 1, Ordering::Relaxed);
        }
        assert!(store.touch(&token).await.is_none());
        // And it's actually gone, not just reported expired once.
        assert!(!store.sessions.read().await.contains_key(&token));
    }

    #[tokio::test]
    async fn touching_resets_the_idle_clock() {
        let store = SessionStore::default();
        let token = store
            .create("root".into(), HashMap::new(), HashSet::new())
            .await;
        {
            let sessions = store.sessions.read().await;
            sessions
                .get(&token)
                .unwrap()
                .last_seen
                .store(now_secs() - IDLE_SECS + 5, Ordering::Relaxed);
        }
        // Still within the window, so this succeeds and bumps last_seen back to "now".
        assert!(store.touch(&token).await.is_some());
        let expires_soon = store.touch(&token).await.unwrap().expires_at();
        assert!(expires_soon >= now_secs() + IDLE_SECS - 2);
    }

    #[tokio::test]
    async fn logout_removes_the_session() {
        let store = SessionStore::default();
        let token = store
            .create("root".into(), HashMap::new(), HashSet::new())
            .await;
        assert!(store.remove(&token).await.is_some());
        assert!(store.touch(&token).await.is_none());
    }

    #[tokio::test]
    async fn login_rate_limit_kicks_in_after_five_failures() {
        let store = SessionStore::default();
        let ip: IpAddr = "10.0.0.5".parse().unwrap();
        for _ in 0..5 {
            assert!(store.login_attempt_allowed(ip).await.is_ok());
            store.note_login_failure(ip).await;
        }
        let err = store.login_attempt_allowed(ip).await.unwrap_err();
        assert!((1..=LOGIN_WINDOW_SECS).contains(&err));
    }

    #[tokio::test]
    async fn login_rate_limit_is_per_ip() {
        let store = SessionStore::default();
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        for _ in 0..5 {
            store.note_login_failure(a).await;
        }
        assert!(store.login_attempt_allowed(a).await.is_err());
        assert!(store.login_attempt_allowed(b).await.is_ok());
    }

    #[tokio::test]
    async fn a_success_clears_the_failure_count() {
        let store = SessionStore::default();
        let ip: IpAddr = "10.0.0.9".parse().unwrap();
        for _ in 0..4 {
            store.note_login_failure(ip).await;
        }
        store.note_login_success(ip).await;
        // Only 4 recorded before the reset, so still allowed either way, but confirm the
        // counter actually cleared rather than just happening to be under the cap.
        for _ in 0..4 {
            assert!(store.login_attempt_allowed(ip).await.is_ok());
            store.note_login_failure(ip).await;
        }
        assert!(store.login_attempt_allowed(ip).await.is_ok());
    }

    #[test]
    fn csrf_header_must_be_exactly_1() {
        let mut h = HeaderMap::new();
        assert!(!has_csrf_header(&h));
        h.insert("X-OpenWrtFi", HeaderValue::from_static("1"));
        assert!(has_csrf_header(&h));
        h.insert("X-OpenWrtFi", HeaderValue::from_static("true"));
        assert!(!has_csrf_header(&h));
    }

    #[test]
    fn cookie_parsing_finds_the_named_cookie_among_others() {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("foo=bar; owfi_session=abc123; baz=qux"),
        );
        assert_eq!(cookie(&h, COOKIE_NAME).as_deref(), Some("abc123"));
        assert_eq!(cookie(&h, "missing"), None);
    }

    #[test]
    fn random_tokens_are_64_hex_chars_and_not_repeated() {
        // Sanity check on the format only — actual randomness needs /dev/urandom at runtime.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (a, b) = rt.block_on(async { (random_token().await, random_token().await) });
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
