//! The multi-node WiFi change ("apply plan") — `docs/api.md`'s Actions section:
//! `POST /wifi/networks/{id}/plan`, `POST /wifi/plans/{id}/apply`, `GET /wifi/plans/{id}`.
//!
//! A plan is computed (diffed, masked, ordered) synchronously on `POST .../plan`; `POST
//! .../apply` starts a background rollout, one node at a time — `uci set` → `uci apply
//! {rollback:true, timeout:30}` → wait → re-reach the node → `uci confirm`, or leave it to
//! rpcd's own revert. The gateway nuance from the contract: since the controller *runs on* the
//! gateway, reaching it at `127.0.0.1` proves nothing about whether it's still reachable from
//! the LAN, so every AP must still answer before the gateway itself is confirmed
//! (`gateway_may_confirm`).

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{Mutex, RwLock};

use crate::{
    AppState,
    audit::{self, AuditEntry, AuditResult, ConfigChange},
    config::NodeConfig,
    model::{Plan, PlanDiff, PlanRunState, PlanStatus, StepState, StepStatus},
    safe_apply,
    session::Session,
    ubus::Ubus,
    uci_engine::{self, Section, UciValue},
};

fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Real rollouts wait a few seconds for a uci change to land before probing reachability; tests
/// use a few milliseconds so the state machine can be exercised without a multi-second sleep per
/// node.
fn settle_delay() -> Duration {
    if cfg!(test) {
        Duration::from_millis(5)
    } else {
        Duration::from_secs(3)
    }
}

/// Mock mode "simulate the progression with realistic delays" (docs/api.md): ~2s applying, ~3s
/// confirming. Tests use milliseconds.
fn mock_applying_delay() -> Duration {
    if cfg!(test) {
        Duration::from_millis(5)
    } else {
        Duration::from_secs(2)
    }
}
fn mock_confirming_delay() -> Duration {
    if cfg!(test) {
        Duration::from_millis(5)
    } else {
        Duration::from_secs(3)
    }
}

// -------------------------------------------------------------------------------------------
// Request body
// -------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct BandBroadcastReq {
    #[serde(rename = "2.4")]
    pub ghz24: Option<bool>,
    #[serde(rename = "5")]
    pub ghz5: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RoamingReq {
    pub ft: Option<bool>,
    pub ft_over_ds: Option<bool>,
    pub mobility_domain: Option<String>,
    pub rrm: Option<bool>,
    pub btm: Option<bool>,
    /// "off" | "optional" | "required".
    pub pmf: Option<String>,
    /// Not written anywhere yet: usteer's interface list lives in a separate `usteer` uci
    /// config with a shape this codebase hasn't verified against the real router (see
    /// `collectors::wifi_networks`'s own doc comment on the same limitation for the read side).
    /// Accepted here so the request body round-trips, but it's a no-op.
    #[allow(dead_code)]
    pub usteer: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlanRequest {
    pub ssid: Option<String>,
    pub key: Option<String>,
    /// "sae-mixed" | "psk2" | "sae" | "none" — written verbatim to uci's `encryption` option.
    pub security: Option<String>,
    pub broadcast: Option<HashMap<String, BandBroadcastReq>>,
    pub roaming: Option<RoamingReq>,
}

// -------------------------------------------------------------------------------------------
// Plan computation
// -------------------------------------------------------------------------------------------

/// One uci section to change on one node, plus which node it's on.
#[derive(Clone)]
struct NodeChange {
    node: String,
    section: String,
    values: BTreeMap<String, UciValue>,
    before: BTreeMap<String, Option<serde_json::Value>>,
}

/// Convert planned wireless updates into masked, structured audit changes.
fn audit_changes(changes: &[NodeChange]) -> Vec<ConfigChange> {
    changes
        .iter()
        .flat_map(|c| {
            c.values.iter().map(|(option, after)| {
                let before = c.before.get(option).cloned().flatten();
                let secret = matches!(
                    option.as_str(),
                    "key" | "password" | "psk" | "private_key" | "auth_token"
                );
                let show = |v: Option<serde_json::Value>| {
                    v.map(|v| {
                        if secret {
                            audit::mask_secret("").to_owned()
                        } else if let Some(s) = v.as_str() {
                            s.to_owned()
                        } else {
                            v.to_string()
                        }
                    })
                };
                ConfigChange {
                    node: c.node.clone(),
                    config: "wireless".into(),
                    section: c.section.clone(),
                    option: option.clone(),
                    before: show(before.clone()),
                    after: show(Some(after.to_wire())),
                    restorable: !secret
                        && before
                            .as_ref()
                            .is_none_or(|v| !v.is_array() && !v.is_object())
                        && !matches!(after, UciValue::List(_)),
                    restore_section: false,
                }
            })
        })
        .collect()
}

/// Locates every wifi-iface section (across every node) belonging to the network named by
/// `network_id`: first finds `network_id`'s own `ssid`/`mesh_id` (its "group key"), then finds
/// every section on every node sharing that same key, band by band (mirrors
/// `collectors::wifi_networks`'s own grouping, which this can't reuse directly since it also
/// needs each section's `device`/band, not just the group summary).
async fn discover(
    nodes: &[(NodeConfig, Ubus)],
    network_id: &str,
) -> Option<(HashMap<String, Vec<Section>>, HashMap<String, Vec<String>>)> {
    let mut per_node = HashMap::new();
    for (nc, ubus) in nodes {
        let sections = match uci_engine::read(ubus, "wireless").await {
            Ok(sections) => sections,
            Err(e) => {
                log::warn!(
                    "wifi plan discovery: uci read wireless on node {} failed: {e}; using empty configuration",
                    nc.name
                );
                Vec::new()
            }
        };
        per_node.insert(nc.name.clone(), sections);
    }

    let key = per_node.values().find_map(|sections| {
        sections
            .iter()
            .find(|s| s.kind == "wifi-iface" && s.name == network_id)
            .and_then(|s| s.get("mesh_id").or_else(|| s.get("ssid")))
            .map(str::to_owned)
    })?;

    let mut present = HashMap::new();
    for (node, sections) in &per_node {
        let matches: Vec<String> = sections
            .iter()
            .filter(|s| s.kind == "wifi-iface")
            .filter(|s| s.get("mesh_id").or_else(|| s.get("ssid")) == Some(key.as_str()))
            .map(|s| s.name.clone())
            .collect();
        if !matches.is_empty() {
            present.insert(node.clone(), matches);
        }
    }
    if present.is_empty() {
        None
    } else {
        Some((per_node, present))
    }
}

fn device_band(sections: &[Section], device: &str) -> Option<bool> {
    sections
        .iter()
        .find(|s| s.kind == "wifi-device" && s.name == device)
        .map(|d| d.get("band") == Some("5g"))
}

fn pmf_value(s: &str) -> Option<&'static str> {
    match s {
        "off" => Some("0"),
        "optional" => Some("1"),
        "required" => Some("2"),
        _ => None,
    }
}

/// Builds the per-(node, section) `uci set` values for the requested change. Fields that apply
/// network-wide (ssid/key/security/roaming) are written to every matching section on every
/// node; `broadcast` targets one specific band's section per node.
fn build_changes(
    per_node_sections: &HashMap<String, Vec<Section>>,
    present: &HashMap<String, Vec<String>>,
    req: &PlanRequest,
) -> Vec<NodeChange> {
    let mut by_node_section: HashMap<(String, String), BTreeMap<String, UciValue>> = HashMap::new();

    for (node, section_names) in present {
        for section in section_names {
            let values = by_node_section
                .entry((node.clone(), section.clone()))
                .or_default();
            if let Some(ssid) = &req.ssid {
                values.insert("ssid".into(), UciValue::String(ssid.clone()));
            }
            if let Some(key) = &req.key {
                values.insert("key".into(), UciValue::String(key.clone()));
            }
            if let Some(sec) = &req.security {
                values.insert("encryption".into(), UciValue::String(sec.clone()));
            }
            if let Some(roaming) = &req.roaming {
                if let Some(ft) = roaming.ft {
                    values.insert("ieee80211r".into(), UciValue::Bool(ft));
                }
                if let Some(v) = roaming.ft_over_ds {
                    values.insert("ft_over_ds".into(), UciValue::Bool(v));
                }
                if let Some(md) = &roaming.mobility_domain {
                    values.insert("mobility_domain".into(), UciValue::String(md.clone()));
                }
                if let Some(v) = roaming.rrm {
                    values.insert("ieee80211k".into(), UciValue::Bool(v));
                }
                if let Some(v) = roaming.btm {
                    values.insert("ieee80211v".into(), UciValue::Bool(v));
                }
                if let Some(pmf) = roaming.pmf.as_deref().and_then(pmf_value) {
                    values.insert("ieee80211w".into(), UciValue::String(pmf.into()));
                }
            }
        }
    }

    if let Some(broadcast) = &req.broadcast {
        for (node, bands) in broadcast {
            let Some(section_names) = present.get(node) else {
                continue;
            };
            let Some(sections) = per_node_sections.get(node) else {
                continue;
            };
            for section_name in section_names {
                let Some(section) = sections.iter().find(|s| s.name == *section_name) else {
                    continue;
                };
                let Some(device) = section.get("device") else {
                    continue;
                };
                let Some(is_5ghz) = device_band(sections, device) else {
                    continue;
                };
                let want = if is_5ghz { bands.ghz5 } else { bands.ghz24 };
                if let Some(on) = want {
                    by_node_section
                        .entry((node.clone(), section_name.clone()))
                        .or_default()
                        .insert("disabled".into(), UciValue::Bool(!on));
                }
            }
        }
    }

    by_node_section
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|((node, section), values)| {
            let before = per_node_sections
                .get(&node)
                .and_then(|sections| sections.iter().find(|s| s.name == section))
                .map(|old| {
                    values
                        .keys()
                        .map(|key| (key.clone(), old.options.get(key).cloned()))
                        .collect()
                })
                .unwrap_or_default();
            NodeChange {
                node,
                section,
                values,
                before,
            }
        })
        .collect()
}

/// APs first — mesh-only APs (no wired uplink) before wired ones — gateway last, per the
/// contract. Only nodes actually carrying this network are included.
fn order_nodes(
    nodes: &[(NodeConfig, Ubus)],
    present: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let Some((gateway, _)) = nodes.first() else {
        return vec![];
    };
    let mut mesh_only = vec![];
    let mut wired = vec![];
    for (nc, _) in nodes.iter().skip(1) {
        if !present.contains_key(&nc.name) {
            continue;
        }
        if nc.wired_uplink.is_none() {
            mesh_only.push(nc.name.clone());
        } else {
            wired.push(nc.name.clone());
        }
    }
    mesh_only.extend(wired);
    if present.contains_key(&gateway.name) {
        mesh_only.push(gateway.name.clone());
    }
    mesh_only
}

fn diff_lines(order: &[String], changes: &[NodeChange]) -> Vec<PlanDiff> {
    order
        .iter()
        .map(|node| {
            let mut lines: Vec<String> = changes
                .iter()
                .filter(|c| &c.node == node)
                .flat_map(|c| {
                    c.values.iter().map(move |(k, v)| {
                        audit::mask_uci_line("wireless", &c.section, k, &v.as_text())
                    })
                })
                .collect();
            lines.sort();
            PlanDiff {
                node: node.clone(),
                lines,
            }
        })
        .collect()
}

// -------------------------------------------------------------------------------------------
// Execution
// -------------------------------------------------------------------------------------------

#[derive(Clone)]
enum Exec {
    /// `fail` = true simulates this node going unreachable after apply (mock-mode testing hook,
    /// `docs/api.md`: "a special SSID value like fail-ap-2").
    Mock {
        fail: bool,
    },
    Real {
        ubus: Ubus,
    },
}

#[derive(Clone)]
struct StepExec {
    node: String,
    is_gateway: bool,
    changes: Vec<NodeChange>,
    exec: Exec,
}

pub struct PlanRecord {
    network: String,
    status: PlanStatus,
    diff: Vec<PlanDiff>,
    steps_exec: Vec<StepExec>,
    started: bool,
}

#[derive(Default)]
pub struct PlanStore {
    plans: RwLock<HashMap<String, Arc<RwLock<PlanRecord>>>>,
    pub running: Mutex<Option<String>>,
}

impl PlanStore {
    async fn get(&self, id: &str) -> Option<Arc<RwLock<PlanRecord>>> {
        self.plans.read().await.get(id).cloned()
    }
}

/// The gateway nuance from `docs/api.md`: the controller runs *on* the gateway, so its own
/// `127.0.0.1` reachability check proves nothing about the LAN side — every AP must still be
/// reachable before the gateway itself gets confirmed.
fn gateway_may_confirm(self_reachable: bool, all_aps_reachable: bool) -> bool {
    self_reachable && all_aps_reachable
}

/// Store one plan action outcome and its structured UCI changes.
async fn record_audit(
    state: &AppState,
    user: &str,
    action: &str,
    node: &str,
    ok: bool,
    detail: &str,
    config_changes: Vec<ConfigChange>,
) {
    state
        .audit
        .record(AuditEntry {
            id: String::new(),
            ts: now(),
            user: user.to_owned(),
            action: action.to_owned(),
            target: node.to_owned(),
            result: if ok {
                AuditResult::Ok
            } else {
                AuditResult::Error
            },
            detail: detail.to_owned(),
            config_changes,
        })
        .await;
}

async fn set_status(
    record: &Arc<RwLock<PlanRecord>>,
    node: &str,
    status: StepState,
    deadline: Option<u64>,
    error: Option<String>,
) {
    let mut r = record.write().await;
    if let Some(s) = r.status.steps.iter_mut().find(|s| s.node == node) {
        s.status = status;
        s.deadline = deadline;
        s.error = error;
    }
}

/// Apply rollout steps in order and confirm each node only while reachability checks pass.
async fn run_rollout(state: Arc<AppState>, user: String, record: Arc<RwLock<PlanRecord>>) {
    let steps = record.read().await.steps_exec.clone();
    let mut failed = false;
    let mut confirmed_real: Vec<StepExec> = vec![];

    for step in &steps {
        if failed {
            set_status(&record, &step.node, StepState::Skipped, None, None).await;
            continue;
        }
        set_status(
            &record,
            &step.node,
            StepState::Applying,
            Some(now() + 30),
            None,
        )
        .await;
        // `Failed`: the change itself couldn't even be attempted/applied (bad uci call, rpcd
        // error). `RolledBack`: the change *was* applied but the node went unreachable before
        // it could be confirmed, so rpcd's own timer reverted it — a distinct, expected outcome
        // the contract calls out separately.
        let outcome: Result<(), (StepState, String)> = match &step.exec {
            Exec::Mock { fail } => {
                tokio::time::sleep(mock_applying_delay()).await;
                set_status(
                    &record,
                    &step.node,
                    StepState::Confirming,
                    Some(now() + 3),
                    None,
                )
                .await;
                tokio::time::sleep(mock_confirming_delay()).await;
                if *fail {
                    Err((
                        StepState::RolledBack,
                        "simulated: node unreachable after apply".to_owned(),
                    ))
                } else {
                    Ok(())
                }
            }
            Exec::Real { ubus } => safe_apply::run(
                ubus,
                &["wireless"],
                30,
                settle_delay(),
                || async {
                    match ubus.has_access("uci", "wireless", "write").await {
                        Ok(true) => {}
                        Ok(false) => {
                            return Err(format!(
                                "rpcd ACL denies uci:wireless.write on {}",
                                step.node
                            ));
                        }
                        Err(e) => {
                            return Err(format!("could not verify rpcd ACL on {}: {e}", step.node));
                        }
                    }
                    for change in &step.changes {
                        uci_engine::update(ubus, "wireless", &change.section, &change.values)
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(())
                },
                || async {
                    set_status(
                        &record,
                        &step.node,
                        StepState::Confirming,
                        Some(now() + 5),
                        None,
                    )
                    .await;
                    let self_ok = safe_apply::board_reachable(ubus).await;
                    let mut aps_ok = true;
                    if step.is_gateway {
                        for prior in &confirmed_real {
                            if prior.is_gateway {
                                continue;
                            }
                            if let Exec::Real { ubus: prior_ubus } = &prior.exec
                                && !safe_apply::board_reachable(prior_ubus).await
                            {
                                aps_ok = false;
                                break;
                            }
                        }
                    }
                    if step.is_gateway {
                        gateway_may_confirm(self_ok, aps_ok)
                    } else {
                        self_ok
                    }
                },
            )
            .await
            .map(|_| ())
            .map_err(|e| {
                let state = if matches!(e, safe_apply::Error::RollbackArmed { .. }) {
                    StepState::RolledBack
                } else {
                    StepState::Failed
                };
                (state, e.to_string())
            }),
        };

        match outcome {
            Ok(()) => {
                set_status(&record, &step.node, StepState::Confirmed, None, None).await;
                record_audit(
                    &state,
                    &user,
                    "plan_apply_confirmed",
                    &step.node,
                    true,
                    "confirmed",
                    audit_changes(&step.changes),
                )
                .await;
                confirmed_real.push(step.clone());
            }
            Err((status, e)) => {
                let action = if status == StepState::RolledBack {
                    "plan_apply_rolled_back"
                } else {
                    "plan_apply_failed"
                };
                set_status(&record, &step.node, status, None, Some(e.clone())).await;
                record_audit(
                    &state,
                    &user,
                    action,
                    &step.node,
                    false,
                    &e,
                    audit_changes(&step.changes),
                )
                .await;
                failed = true;
            }
        }
    }

    {
        let mut r = record.write().await;
        r.status.state = if failed {
            PlanRunState::Failed
        } else {
            PlanRunState::Done
        };
    }
    *state.plans.running.lock().await = None;
}

// -------------------------------------------------------------------------------------------
// HTTP handlers
// -------------------------------------------------------------------------------------------

/// Validate a requested network update and return a preview with an opaque plan ID.
pub async fn create_plan(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Path(network_id): Path<String>,
    body: Option<Json<PlanRequest>>,
) -> Response {
    let req = body.map(|b| b.0).unwrap_or_default();
    let Some((per_node_sections, present)) = discover(&state.nodes, &network_id).await else {
        return err(
            StatusCode::NOT_FOUND,
            format!("unknown wifi network {network_id:?}"),
        );
    };

    {
        let snap = state.snapshot.read().await;
        for node in present.keys() {
            if let Some(n) = snap.nodes.iter().find(|n| &n.name == node)
                && !n.online
            {
                return err(StatusCode::CONFLICT, format!("{node} is offline"));
            }
        }
    }

    let changes = build_changes(&per_node_sections, &present, &req);
    if changes.is_empty() {
        return err(StatusCode::BAD_REQUEST, "no fields to change");
    }
    let order = order_nodes(&state.nodes, &present);
    let diff = diff_lines(&order, &changes);

    let mock = state.mock.enabled();
    let fail_target = req
        .ssid
        .as_deref()
        .and_then(|s| s.strip_prefix("fail-"))
        .map(str::to_owned);
    let gateway_name = state
        .nodes
        .first()
        .map(|(n, _)| n.name.clone())
        .unwrap_or_default();

    let mut steps_exec = vec![];
    for node in &order {
        let node_changes: Vec<NodeChange> = changes
            .iter()
            .filter(|c| &c.node == node)
            .cloned()
            .collect();
        let exec = if mock {
            Exec::Mock {
                fail: fail_target.as_deref() == Some(node.as_str()),
            }
        } else {
            if session.read_only.contains(node) {
                return err(
                    StatusCode::FORBIDDEN,
                    format!("{node} is read-only for this session"),
                );
            }
            let Some(ubus) = session.node_ubus.get(node) else {
                return err(
                    StatusCode::BAD_REQUEST,
                    format!("no write session for {node}"),
                );
            };
            match ubus.has_access("uci", "wireless", "write").await {
                Ok(true) => {}
                Ok(false) => {
                    return err(
                        StatusCode::FORBIDDEN,
                        format!("rpcd ACL denies uci:wireless.write on {node}"),
                    );
                }
                Err(e) => {
                    return err(
                        StatusCode::FORBIDDEN,
                        format!("could not verify rpcd ACL on {node}: {e}"),
                    );
                }
            }
            Exec::Real { ubus: ubus.clone() }
        };
        steps_exec.push(StepExec {
            node: node.clone(),
            is_gateway: node == &gateway_name,
            changes: node_changes,
            exec,
        });
    }

    let id = crate::session::random_token().await[..16].to_owned();
    let status = PlanStatus {
        id: id.clone(),
        state: PlanRunState::Running,
        steps: order
            .iter()
            .map(|n| StepStatus {
                node: n.clone(),
                status: StepState::Queued,
                deadline: None,
                error: None,
            })
            .collect(),
    };
    let record = PlanRecord {
        network: network_id.clone(),
        status,
        diff: diff.clone(),
        steps_exec,
        started: false,
    };
    state
        .plans
        .plans
        .write()
        .await
        .insert(id.clone(), Arc::new(RwLock::new(record)));

    Json(Plan {
        id,
        network: network_id,
        order,
        diff,
    })
    .into_response()
}

/// Start applying a previously created plan, rejecting duplicate or unknown runs.
pub async fn apply_plan(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<Arc<Session>>,
    Path(id): Path<String>,
) -> Response {
    let Some(record_arc) = state.plans.get(&id).await else {
        return err(StatusCode::NOT_FOUND, "unknown plan");
    };

    let real_steps = {
        let record = record_arc.read().await;
        record
            .steps_exec
            .iter()
            .filter_map(|step| match &step.exec {
                Exec::Real { ubus } => Some((step.node.clone(), ubus.clone())),
                Exec::Mock { .. } => None,
            })
            .collect::<Vec<_>>()
    };
    for (node, ubus) in real_steps {
        match ubus.has_access("uci", "wireless", "write").await {
            Ok(true) => {}
            Ok(false) => {
                return err(
                    StatusCode::FORBIDDEN,
                    format!("rpcd ACL denies uci:wireless.write on {node}"),
                );
            }
            Err(e) => {
                return err(
                    StatusCode::FORBIDDEN,
                    format!("could not verify rpcd ACL on {node}: {e}"),
                );
            }
        }
    }

    {
        let mut running = state.plans.running.lock().await;
        if running.is_some() {
            return err(StatusCode::CONFLICT, "another plan is already running");
        }
        let mut record = record_arc.write().await;
        if record.started {
            return err(StatusCode::CONFLICT, "plan already applied");
        }
        let snap = state.snapshot.read().await;
        for step in &record.status.steps {
            if let Some(n) = snap.nodes.iter().find(|n| n.name == step.node)
                && !n.online
            {
                return err(StatusCode::CONFLICT, format!("{} is offline", step.node));
            }
        }
        record.started = true;
        *running = Some(id.clone());
    }

    let (network, order, diff) = {
        let r = record_arc.read().await;
        (
            r.network.clone(),
            r.status
                .steps
                .iter()
                .map(|s| s.node.clone())
                .collect::<Vec<_>>(),
            r.diff.clone(),
        )
    };
    tokio::spawn(run_rollout(
        state.clone(),
        session.user.clone(),
        record_arc.clone(),
    ));

    (
        StatusCode::ACCEPTED,
        Json(Plan {
            id,
            network,
            order,
            diff,
        }),
    )
        .into_response()
}

pub async fn get_plan(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.plans.get(&id).await {
        Some(record) => Json(record.read().await.status.clone()).into_response(),
        None => err(StatusCode::NOT_FOUND, "unknown plan"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(node: &str, is_gateway: bool, exec: Exec) -> StepExec {
        StepExec {
            node: node.to_owned(),
            is_gateway,
            changes: vec![],
            exec,
        }
    }

    fn record_with(steps: Vec<StepExec>) -> Arc<RwLock<PlanRecord>> {
        let status = PlanStatus {
            id: "p1".into(),
            state: PlanRunState::Running,
            steps: steps
                .iter()
                .map(|s| StepStatus {
                    node: s.node.clone(),
                    status: StepState::Queued,
                    deadline: None,
                    error: None,
                })
                .collect(),
        };
        Arc::new(RwLock::new(PlanRecord {
            network: "net".into(),
            status,
            diff: vec![],
            steps_exec: steps,
            started: true,
        }))
    }

    fn test_state() -> Arc<AppState> {
        // A minimal AppState just for run_rollout's audit + plans.running bookkeeping.
        crate::test_support::minimal_app_state()
    }

    #[tokio::test]
    async fn order_puts_mesh_only_aps_first_then_wired_then_gateway() {
        let mut cfg = crate::config::Config::default();
        cfg.use_fixtures(std::path::PathBuf::from(format!(
            "{}/fixtures",
            env!("CARGO_MANIFEST_DIR")
        )));
        let nodes: Vec<(NodeConfig, Ubus)> = cfg
            .nodes
            .iter()
            .map(|n| (n.clone(), cfg.ubus_for(n)))
            .collect();
        let present: HashMap<String, Vec<String>> = nodes
            .iter()
            .map(|(n, _)| (n.name.clone(), vec!["x".into()]))
            .collect();
        let order = order_nodes(&nodes, &present);
        assert_eq!(order, vec!["ap-4", "ap-2", "mainrouter"]);
    }

    #[test]
    fn diff_masks_the_key_but_not_the_ssid() {
        let changes = vec![NodeChange {
            node: "ap-2".into(),
            section: "default_radio1".into(),
            before: BTreeMap::new(),
            values: {
                let mut m = BTreeMap::new();
                m.insert("ssid".into(), UciValue::String("NewWifi".into()));
                m.insert("key".into(), UciValue::String("supersecret".into()));
                m
            },
        }];
        let diff = diff_lines(&["ap-2".to_owned()], &changes);
        let lines = &diff[0].lines;
        assert!(lines.iter().any(|l| l.contains("NewWifi")));
        assert!(
            lines
                .iter()
                .any(|l| l.contains('\u{2022}') && !l.contains("supersecret"))
        );
    }

    #[test]
    /// Checks history masks secrets and permits restoration only for scalar public values.
    fn history_masks_secrets_and_marks_only_scalar_public_values_restorable() {
        let changes = vec![NodeChange {
            node: "ap-2".into(),
            section: "wifi_main".into(),
            before: BTreeMap::from([
                ("ssid".into(), Some(serde_json::Value::String("Old".into()))),
                (
                    "key".into(),
                    Some(serde_json::Value::String("old-secret".into())),
                ),
            ]),
            values: BTreeMap::from([
                ("ssid".into(), UciValue::String("New".into())),
                ("key".into(), UciValue::String("new-secret".into())),
            ]),
        }];
        let rows = audit_changes(&changes);
        let ssid = rows.iter().find(|r| r.option == "ssid").unwrap();
        assert_eq!(ssid.before.as_deref(), Some("Old"));
        assert_eq!(ssid.after.as_deref(), Some("New"));
        assert!(ssid.restorable);
        let key = rows.iter().find(|r| r.option == "key").unwrap();
        assert_eq!(key.before.as_deref(), Some("••••••••"));
        assert_eq!(key.after.as_deref(), Some("••••••••"));
        assert!(!key.restorable);
        assert!(!serde_json::to_string(&rows).unwrap().contains("secret"));
    }

    #[test]
    fn gateway_confirm_requires_both_self_and_ap_reachability() {
        assert!(gateway_may_confirm(true, true));
        assert!(!gateway_may_confirm(true, false));
        assert!(!gateway_may_confirm(false, true));
    }

    #[tokio::test]
    async fn mock_rollout_succeeds_when_no_node_is_marked_to_fail() {
        let steps = vec![
            step("ap-4", false, Exec::Mock { fail: false }),
            step("ap-2", false, Exec::Mock { fail: false }),
            step("mainrouter", true, Exec::Mock { fail: false }),
        ];
        let record = record_with(steps);
        run_rollout(test_state(), "root".into(), record.clone()).await;
        let status = record.read().await.status.clone();
        assert_eq!(status.state, PlanRunState::Done);
        assert!(
            status
                .steps
                .iter()
                .all(|s| s.status == StepState::Confirmed)
        );
    }

    #[tokio::test]
    async fn mock_rollout_rolls_back_the_failing_node_and_skips_the_rest() {
        let steps = vec![
            step("ap-4", false, Exec::Mock { fail: false }),
            step("ap-2", false, Exec::Mock { fail: true }),
            step("mainrouter", true, Exec::Mock { fail: false }),
        ];
        let record = record_with(steps);
        run_rollout(test_state(), "root".into(), record.clone()).await;
        let status = record.read().await.status.clone();
        assert_eq!(status.state, PlanRunState::Failed);
        assert_eq!(status.steps[0].status, StepState::Confirmed);
        assert_eq!(status.steps[1].status, StepState::RolledBack);
        assert!(status.steps[1].error.is_some());
        assert_eq!(status.steps[2].status, StepState::Skipped);
    }
}
