//! OpenWrtFi — a UniFi-style dashboard for OpenWrt.
//!
//! One static binary runs on the gateway (the "controller"). It polls every
//! node over ubus — locally via the `ubus` CLI, remotely via rpcd's JSON-RPC
//! endpoint that LuCI already exposes — so the APs need nothing installed.

mod actions;
mod api;
mod audit;
mod auth;
mod collectors;
mod config;
mod diag;
mod http;
mod mock;
mod model;
mod nodes_api;
mod plan;
mod session;
mod shell;
#[cfg(test)]
mod test_support;
mod ubus;
mod validate;
mod web;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use argh::FromArgs;
use log::info;
use tokio::sync::{RwLock, watch};

use crate::{
    audit::Audit,
    config::{Config, NodeConfig},
    mock::MockState,
    model::{DailyUsage, RatePoint, Snapshot},
    plan::PlanStore,
    session::SessionStore,
    shell::ShellTokens,
    ubus::Ubus,
};

#[derive(FromArgs)]
/// UniFi-style dashboard for OpenWrt.
struct Args {
    /// address to listen on (default 0.0.0.0:8088)
    #[argh(option, default = "\"0.0.0.0:8088\".parse().unwrap()")]
    listen: SocketAddr,

    /// path to the JSON config (default /etc/openwrtfi.json; built-in defaults if missing)
    #[argh(option, default = "PathBuf::from(\"/etc/openwrtfi.json\")")]
    config: PathBuf,

    /// serve canned data from this fixtures dir instead of talking to ubus (for dev on a laptop)
    #[argh(option)]
    mock: Option<PathBuf>,
}

pub struct AppState {
    pub config: Config,
    /// Source JSON file used for persistent node add/remove operations from the UI.
    pub config_path: PathBuf,
    /// Every node with its (already session-caching, for rpcd) `Ubus` handle — built once so
    /// on-demand read handlers (`/wifi/networks`, `/logs`, `/diag`) share the poll loop's rpcd
    /// sessions instead of re-authenticating every request.
    pub nodes: Vec<(NodeConfig, Ubus)>,
    pub snapshot: RwLock<Arc<Snapshot>>,
    /// 7-day / 3h-bucket history, refreshed each poll alongside the snapshot (the 24h series
    /// lives on `Snapshot.wan.history` instead, since `/history?range=24h` and the dashboard
    /// both just want "the current snapshot's history").
    pub history_7d: RwLock<Vec<RatePoint>>,
    /// The daily usage rollup (`collectors::usage_daily::DailyTracker`), refreshed each poll;
    /// `GET /usage/daily` slices this down to the requested window.
    pub daily_usage: RwLock<Vec<DailyUsage>>,
    pub audit: Audit,
    /// Logged-in sessions and the login rate limiter (`src/session.rs`).
    pub sessions: SessionStore,
    /// Multi-node WiFi rollouts in flight or completed (`src/plan.rs`).
    pub plans: PlanStore,
    /// Mock-mode-only write-action overrides so the UI visibly reacts (`src/mock.rs`); a no-op
    /// outside `--mock`.
    pub mock: MockState,
    /// One-time browser-shell step-up tokens (`src/shell.rs`).
    pub shell_tokens: ShellTokens,
    /// Bumped after every poll; SSE subscribers wake on change.
    pub tick: watch::Receiver<u64>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Args = argh::from_env();

    let mock = args.mock.is_some();
    let config_path = args.config.clone();
    let mut config = Config::load(&config_path);
    if let Some(dir) = args.mock {
        config.use_fixtures(dir);
    }

    let nodes = config
        .nodes
        .iter()
        .map(|n| (n.clone(), config.ubus_for(n)))
        .collect();
    let audit = Audit::new(config.audit_log.clone());

    let (tick_tx, tick_rx) = watch::channel(0u64);
    let state = Arc::new(AppState {
        config,
        config_path,
        nodes,
        snapshot: RwLock::new(Arc::new(Snapshot::default())),
        history_7d: RwLock::new(vec![]),
        daily_usage: RwLock::new(vec![]),
        audit,
        sessions: SessionStore::default(),
        plans: PlanStore::default(),
        mock: MockState::new(mock),
        shell_tokens: ShellTokens::default(),
        tick: tick_rx,
    });

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let poll_handle = tokio::spawn(collectors::run(state.clone(), tick_tx, shutdown_rx));

    let app = api::router(state.clone());
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .expect("bind");
    info!("openwrtfi listening on http://{}", args.listen);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    })
    .await
    .expect("server");
    // Give the poll loop a chance to flush the history/daily-usage files before the process
    // exits (bounded, so a stuck task can't hang shutdown forever).
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), poll_handle).await;
}
