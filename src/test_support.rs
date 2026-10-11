//! Test-only helpers for building a minimal `AppState` without going through `main()`. Used by
//! unit tests in `plan.rs`/`shell.rs`/`actions.rs` that need a real `Arc<AppState>` for its audit
//! log and plan/session bookkeeping, but don't want to spin up the poll loop or a listener.

use std::sync::Arc;

use tokio::sync::{RwLock, watch};

use crate::{
    AppState, audit::Audit, config::Config, model::Snapshot, plan::PlanStore, session::SessionStore,
};

pub fn minimal_app_state() -> Arc<AppState> {
    let mut config = Config::default();
    config.use_fixtures(std::path::PathBuf::from(format!(
        "{}/fixtures",
        env!("CARGO_MANIFEST_DIR")
    )));
    let nodes = config
        .nodes
        .iter()
        .map(|n| (n.clone(), config.ubus_for(n)))
        .collect();
    let log_path = std::env::temp_dir().join(format!("owfi-test-audit-{}.log", std::process::id()));
    let (_tick_tx, tick_rx) = watch::channel(0u64);
    Arc::new(AppState {
        config,
        config_path: std::env::temp_dir()
            .join(format!("owfi-test-config-{}.json", std::process::id())),
        nodes,
        snapshot: RwLock::new(Arc::new(Snapshot::default())),
        history_7d: RwLock::new(vec![]),
        daily_usage: RwLock::new(vec![]),
        audit: Audit::new(log_path),
        sessions: SessionStore::default(),
        plans: PlanStore::default(),
        mock: crate::mock::MockState::new(true),
        shell_tokens: Default::default(),
        tick: tick_rx,
    })
}
