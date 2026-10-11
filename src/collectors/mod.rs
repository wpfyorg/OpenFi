//! Poll loop: every `poll_secs`, query all nodes concurrently and publish a new snapshot.

mod clients;
pub(crate) mod date;
pub mod dns;
mod events;
mod health;
mod links;
mod logs;
mod nlbw;
mod qos;
mod radios;
mod system;
mod traffic;
pub(crate) mod uci;
mod usage_daily;
mod wifi;
pub mod wifi_networks;

pub use logs::collect as collect_logs;
pub use traffic::wan_ip_gateway;

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::future::join_all;
use tokio::sync::watch;

use crate::{
    AppState,
    model::{Role, Snapshot},
};

/// How often the 24h/7d history ring is flushed to `history_file` (every 150 polls = 5 min at
/// the default 2s `poll_secs`) — frequent enough to survive most restarts, rare enough to spare
/// the router's flash.
const SAVE_HISTORY_EVERY_N_POLLS: u64 = 150;

fn merged_whitelist(
    file_whitelisted: &std::collections::HashSet<String>,
    qos_whitelisted: Option<&HashMap<String, bool>>,
) -> std::collections::HashSet<String> {
    let mut whitelisted = file_whitelisted.clone();
    if let Some(qos_whitelisted) = qos_whitelisted {
        for (mac, enabled) in qos_whitelisted {
            if *enabled {
                whitelisted.insert(mac.clone());
            } else {
                whitelisted.remove(mac);
            }
        }
    }
    whitelisted
}

pub async fn run(
    state: Arc<AppState>,
    tick: watch::Sender<u64>,
    mut shutdown: watch::Receiver<bool>,
) {
    let cfg = &state.config;
    let nodes = &state.nodes;
    let gateway = nodes
        .first()
        .map(|(_, u)| u.clone())
        .expect("at least one node configured");
    let gateway_name = nodes
        .first()
        .map(|(n, _)| n.name.clone())
        .unwrap_or_default();

    let mut traffic = traffic::Tracker::load(&cfg.history_file).await;
    let mut daily =
        usage_daily::DailyTracker::load(&cfg.daily_usage_file, cfg.local_utc_offset_secs).await;
    let mut nlbw = nlbw::Tracker::new();
    let mut clients_tracker = clients::Tracker::default();
    let mut events_tracker = events::Tracker::default();
    let mut interval = tokio::time::interval(Duration::from_secs(cfg.poll_secs.max(1)));
    let mut n = 0u64;

    // Cached between the "slow" (~every 10th poll) refreshes below.
    let mut host_hints = HashMap::new();
    let mut reserved = Default::default();
    let mut file_whitelisted = Default::default();
    let mut qos_collected: Option<qos::QosCollected> = None;

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            res = shutdown.changed() => {
                if res.is_err() || *shutdown.borrow() {
                    log::info!("poll loop shutting down, flushing history");
                    traffic.save(&cfg.history_file).await;
                    daily.save(&cfg.daily_usage_file).await;
                    return;
                }
            }
        }
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let polled = join_all(nodes.iter().enumerate().map(|(i, (nc, ubus))| async move {
            let role = if i == 0 { Role::Gateway } else { Role::Ap };
            let mut node = system::collect(nc, ubus, role).await;
            let (stations, iface_counts) = if node.online {
                node.radios = radios::collect(ubus).await;
                wifi::collect(ubus, &nc.name).await
            } else {
                (vec![], HashMap::new())
            };
            for r in &mut node.radios {
                if let Some(iface) = &r.iface {
                    r.clients = iface_counts.get(iface).copied().unwrap_or(0);
                }
            }
            let arp = clients::read_arp(ubus).await;
            (node, stations, arp)
        }))
        .await;

        let (ip, gw, wan_up) = wan_ip_gateway(&gateway).await;
        let mut wan = traffic.sample(&gateway, &cfg.wan_device, ts).await;
        wan.ip = ip;
        wan.gateway = gw;

        let mesh_macs = links::mesh_mac_table(nodes).await;
        let leases = clients::read_leases(&cfg.dhcp_leases).await;

        let mut nodes_out = Vec::with_capacity(polled.len());
        let mut all_stations = Vec::new();
        let mut arp_by_node = Vec::new();
        for (i, (mut node, stations, arp)) in polled.into_iter().enumerate() {
            let (nc, ubus) = &nodes[i];
            if node.online {
                node.links =
                    links::collect(ubus, nc, &gateway_name, i == 0, &mesh_macs, wan_up).await;
            }
            arp_by_node.push((node.name.clone(), arp));
            all_stations.extend(stations);
            nodes_out.push(node);
        }

        // Slow-changing extras only every ~10 polls.
        let prev = state.snapshot.read().await.clone();
        let dns = if n.is_multiple_of(10) {
            dns::collect(cfg).await
        } else {
            prev.dns.clone()
        };
        if n.is_multiple_of(10) {
            host_hints = clients::read_host_hints(&gateway).await;
            reserved = clients::read_reserved(&gateway).await;
            file_whitelisted = clients::read_whitelist(&gateway).await;
            qos_collected = qos::collect(cfg).await.or(qos_collected);
        }
        let qos = qos_collected.as_ref().map(|q| q.qos.clone());

        let whitelisted = merged_whitelist(
            &file_whitelisted,
            qos_collected.as_ref().map(|q| &q.whitelisted),
        );

        let mut clients = clients_tracker.merge(
            ts,
            leases,
            all_stations,
            arp_by_node,
            &host_hints,
            &reserved,
            &whitelisted,
        );

        // qos-status's `devices[].online` comes from net-account's own neighbor tracking, which
        // is more authoritative than a single ARP snapshot for wired devices whose ARP entry is
        // ambiguous (STALE/DELAY/PROBE, or aged out of the cache entirely) — see collectors::qos.
        if let Some(q) = &qos_collected {
            clients_tracker.promote_qos_online(&mut clients, &q.online, &gateway_name, ts);
        }

        // nlbwmon itself only refreshes every ~30s, so there's no point asking more often
        // than that (~15 polls at the default 2s poll_secs).
        let usage = if n.is_multiple_of(15) {
            nlbw.sample(cfg, ts, &clients)
                .await
                .or_else(|| prev.usage.clone())
        } else {
            prev.usage.clone()
        };

        // Mock mode: fold in any action-driven overrides (whitelist toggle, radio disabled/
        // channel/txpower) so the UI visibly reacts even though fixture `Ubus` calls just read
        // static JSON — see `mock.rs`. A no-op outside mock mode.
        state.mock.apply(&mut nodes_out, &mut clients).await;

        let events = events_tracker.update(ts, &nodes_out, &clients, wan_up);
        let health = health::compute(&nodes_out, wan_up);

        let snap = Snapshot {
            ts,
            wan,
            nodes: nodes_out,
            clients,
            usage,
            dns,
            qos,
            events,
            health,
        };
        daily.tick(
            ts,
            snap.wan.rx_bytes,
            snap.wan.tx_bytes,
            snap.clients
                .iter()
                .filter(|c| c.online)
                .map(|c| c.mac.as_str()),
        );
        *state.daily_usage.write().await = daily.rows();

        *state.snapshot.write().await = Arc::new(snap);
        *state.history_7d.write().await = traffic.history(true);

        n += 1;
        if n.is_multiple_of(SAVE_HISTORY_EVERY_N_POLLS) {
            traffic.save(&cfg.history_file).await;
        }
        daily.maybe_save(&cfg.daily_usage_file).await;
        let _ = tick.send(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qos_whitelist_state_overrides_legacy_file_when_present() {
        let file = ["file-true", "file-false", "file-only"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let qos = HashMap::from([
            ("file-true".to_owned(), false),
            ("file-false".to_owned(), true),
        ]);

        let merged = merged_whitelist(&file, Some(&qos));

        assert!(!merged.contains("file-true"));
        assert!(merged.contains("file-false"));
        assert!(merged.contains("file-only"));
    }

    #[test]
    fn file_whitelist_is_used_when_qos_has_no_entry() {
        let file = ["legacy-only".to_owned()].into_iter().collect();
        let qos = HashMap::from([("other-device".to_owned(), false)]);

        assert!(merged_whitelist(&file, Some(&qos)).contains("legacy-only"));
    }

    #[test]
    fn qos_true_whitelists_a_mac_missing_from_the_legacy_file() {
        let file = std::collections::HashSet::new();
        let qos = HashMap::from([("qos-only".to_owned(), true)]);

        assert!(merged_whitelist(&file, Some(&qos)).contains("qos-only"));
    }
}
