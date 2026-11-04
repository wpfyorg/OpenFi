//! Read compatibility wrapper for collectors plus the existing rollback-safe apply helper.
//!
//! Generic UCI CRUD/order/type validation lives in `crate::uci_engine`; collectors keep this
//! narrow wrapper so existing read paths can continue treating an unavailable config as empty.

use std::time::Duration;

use serde_json::json;

use crate::{
    ubus::{self, Ubus},
    uci_engine,
};

pub use crate::uci_engine::Section;

/// All sections of a config, in the order the daemon returned them (uci preserves file order).
pub async fn get(ubus: &Ubus, config: &str) -> Vec<Section> {
    uci_engine::read(ubus, config).await.unwrap_or_default()
}

/// The rollback-safe apply/confirm dance from `docs/api.md`: `uci apply {rollback:true,
/// timeout}`, wait for the change to actually land, re-reach the node with a fresh call, then
/// `uci confirm` — or, if the node didn't answer, do nothing and let rpcd's own timer revert it.
/// `is_reachable` lets callers substitute extra checks (the multi-node plan's gateway nuance:
/// every AP must still be reachable before the gateway itself is confirmed).
pub async fn apply_then<F, Fut>(ubus: &Ubus, timeout_secs: u32, is_reachable: F) -> ubus::Result<()>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    ubus.call(
        "uci",
        "apply",
        json!({ "rollback": true, "timeout": timeout_secs }),
    )
    .await?;
    // Give the change (and, for wireless, the radio restart) a moment to actually take effect
    // before probing for reachability.
    tokio::time::sleep(Duration::from_secs(3)).await;
    if is_reachable().await {
        ubus.call("uci", "confirm", json!({})).await?;
        Ok(())
    } else {
        Err("node unreachable after apply; rpcd will roll the change back".to_owned())
    }
}

/// `apply_then` with the default reachability probe: a fresh `system board` call on the same
/// (session-scoped) `Ubus` handle.
pub async fn apply_confirm(ubus: &Ubus, timeout_secs: u32) -> ubus::Result<()> {
    apply_then(ubus, timeout_secs, || async {
        ubus.call("system", "board", json!({})).await.is_ok()
    })
    .await
}
