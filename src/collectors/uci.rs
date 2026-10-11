//! Thin wrapper over the `uci` ubus object's `get` method, shared by the radios and
//! wifi-networks collectors (both need to walk `/etc/config/wireless`, and the DHCP static
//! leases collector needs `/etc/config/dhcp`).
//!
//! `ubus call uci get '{"config": "wireless"}'` returns `{"values": {<section>: {".name",
//! ".type", ".anonymous", <option>: <value>, ...}}}`. Every option value is a string (uci has
//! no native types), so callers parse booleans as `"1"`/`"0"` and numbers with `str::parse`.
//!
//! The `add`/`set`/`apply_confirm` functions below are the write side, shared by `actions.rs`
//! and `plan.rs`: every config change goes `uci set`/`uci add` → `uci apply {rollback:true,
//! timeout}` → the caller re-reaches the node → `uci confirm` (or lets it auto-revert). They
//! take the caller's own session-scoped `Ubus` (see `session.rs`), so `uci`'s pending-change
//! staging and the rollback timer are scoped to that rpcd session, per `docs/api.md`.

use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::ubus::{self, Ubus};

pub struct Section {
    pub name: String,
    /// uci section type, e.g. "wifi-device", "wifi-iface", "host".
    pub kind: String,
    fields: Map<String, Value>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(Value::as_str)
    }

    /// uci's boolean convention: option present and truthy ("1", "on", "true", "yes").
    pub fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some("1" | "on" | "true" | "yes"))
    }

    pub fn parse<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.get(key).and_then(|s| s.parse().ok())
    }
}

/// All sections of a config, in the order the daemon returned them (uci preserves file order).
pub async fn get(ubus: &Ubus, config: &str) -> Vec<Section> {
    let Ok(v) = ubus.call("uci", "get", json!({ "config": config })).await else {
        return vec![];
    };
    let Some(values) = v["values"].as_object() else {
        return vec![];
    };
    let mut sections: Vec<Section> = values
        .iter()
        .filter_map(|(name, fields)| {
            let fields = fields.as_object()?.clone();
            let kind = fields.get(".type")?.as_str()?.to_owned();
            Some(Section {
                name: name.clone(),
                kind,
                fields,
            })
        })
        .collect();
    sections.sort_by(|a, b| {
        let ia = a.fields.get(".index").and_then(Value::as_i64).unwrap_or(0);
        let ib = b.fields.get(".index").and_then(Value::as_i64).unwrap_or(0);
        ia.cmp(&ib).then_with(|| a.name.cmp(&b.name))
    });
    sections
}

/// `uci set <config>.<section>.<option>=<value>` for every key in `values` — one ubus call per
/// option, since `uci set`'s ubus method takes a single `option`/`value` pair (unlike `uci add`,
/// which takes a whole `values` object).
pub async fn set(
    ubus: &Ubus,
    config: &str,
    section: &str,
    values: &Map<String, Value>,
) -> ubus::Result<()> {
    for (option, value) in values {
        ubus.call(
            "uci",
            "set",
            json!({ "config": config, "section": section, "values": { option: value } }),
        )
        .await?;
    }
    Ok(())
}

/// `uci add <config> <type>` with initial `values`, returning the new (anonymous) section name.
pub async fn add(ubus: &Ubus, config: &str, kind: &str, values: Value) -> ubus::Result<String> {
    let v = ubus
        .call(
            "uci",
            "add",
            json!({ "config": config, "type": kind, "values": values }),
        )
        .await?;
    v.get("section")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "uci add: response had no \"section\"".to_owned())
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
