//! nlbwmon per-client / per-application traffic accounting (gateway only).
//!
//! nlbwmon refreshes its own counters every ~30s regardless of how often we ask, so `mod.rs`
//! calls `Tracker::sample` far less often than the 2s poll (see `NLBW_EVERY_N_POLLS`). We read
//! two groupings: `mac` for per-client totals, and `mac,layer7` for the top apps per client and
//! the network-wide app mix. Output is columnar (`{"columns": [...], "data": [[...], ...]}`)
//! and column order differs per grouping, so `parse_columnar` indexes by name.

use std::{collections::HashMap, time::Instant};

use serde_json::{Map, Value, json};

use crate::{
    config::Config,
    model::{AppUsage, Client, ClientUsage, Usage},
};

/// Keep the client list small enough that the SSE snapshot stays a few KB.
const MAX_CLIENTS: usize = 50;
/// Apps beyond this rank are folded into a single "Other" row.
const MAX_APPS: usize = 12;

#[derive(Default)]
pub struct Tracker {
    /// mac -> (rx_bytes, tx_bytes) at the last sample, to derive rates from the delta.
    last: HashMap<String, (u64, u64)>,
    last_ts: u64,
    /// Mock mode only: elapsed time drives `animate` so fixture counters grow like the real thing.
    started: Option<Instant>,
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            started: Some(Instant::now()),
            ..Default::default()
        }
    }

    /// Fetch `mac` and `mac,layer7`, join with the already-merged client list for
    /// hostname/IP, and derive rates. Returns None if nlbw is disabled or unreachable
    /// (the caller falls back to the previous snapshot's usage).
    pub async fn sample(&mut self, cfg: &Config, ts: u64, clients: &[Client]) -> Option<Usage> {
        let mac_rows = parse_columnar(&self.fetch(cfg, "mac").await?);
        let app_rows = self
            .fetch(cfg, "mac,layer7")
            .await
            .map(|v| parse_columnar(&v))
            .unwrap_or_default();
        Some(self.apply(&mac_rows, &app_rows, clients, ts))
    }

    async fn fetch(&self, cfg: &Config, group: &str) -> Option<Value> {
        if let Some(dir) = &cfg.nlbw_fixtures {
            let path = dir.join(format!("nlbw.{group}.json"));
            let bytes = tokio::fs::read(&path).await.ok()?;
            let mut v: Value = serde_json::from_slice(&bytes).ok()?;
            if let Some(started) = self.started {
                animate(&mut v, started.elapsed().as_secs_f64());
            }
            return Some(v);
        }
        let cmd = cfg.nlbw_cmd.as_ref()?;
        let out = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{cmd} -c json -g {group}"))
            .output()
            .await
            .ok()?;
        serde_json::from_slice(&out.stdout).ok()
    }

    /// Pure aggregation step (no I/O), so it's easy to unit test with hand-built or
    /// fixture-loaded rows.
    fn apply(
        &mut self,
        mac_rows: &[Map<String, Value>],
        app_rows: &[Map<String, Value>],
        clients: &[Client],
        ts: u64,
    ) -> Usage {
        // Per-client top apps and the network-wide app mix both come from `mac,layer7`.
        let mut per_client_apps: HashMap<String, HashMap<String, u64>> = HashMap::new();
        let mut app_totals: HashMap<String, (u64, u64, u64)> = HashMap::new();
        for row in app_rows {
            let mac = str_col(row, "mac").unwrap_or_default().to_lowercase();
            let (rx, tx, conns) = (
                u64_col(row, "rx_bytes"),
                u64_col(row, "tx_bytes"),
                u64_col(row, "conns"),
            );
            let name = app_name(row);
            if !mac.is_empty() {
                *per_client_apps
                    .entry(mac)
                    .or_default()
                    .entry(name.clone())
                    .or_insert(0) += rx + tx;
            }
            let e = app_totals.entry(name).or_insert((0, 0, 0));
            e.0 += rx;
            e.1 += tx;
            e.2 += conns;
        }

        let dt = ts.saturating_sub(self.last_ts).max(1);
        let mut usage_clients = Vec::new();
        for row in mac_rows {
            let mac = str_col(row, "mac").unwrap_or_default().to_lowercase();
            if mac.is_empty() {
                continue;
            }
            let (rx, tx, conns) = (
                u64_col(row, "rx_bytes"),
                u64_col(row, "tx_bytes"),
                u64_col(row, "conns"),
            );
            // No previous sample, or a counter reset (reboot / new nlbwmon month) → no rate
            // yet rather than a bogus negative one; it'll show up on the next sample.
            let (rx_bps, tx_bps) = match self.last.get(&mac) {
                Some(&(lrx, ltx)) if rx >= lrx && tx >= ltx => {
                    ((rx - lrx) * 8 / dt, (tx - ltx) * 8 / dt)
                }
                _ => (0, 0),
            };
            self.last.insert(mac.clone(), (rx, tx));

            let mut top_apps: Vec<(String, u64)> = per_client_apps
                .get(&mac)
                .map(|m| m.iter().map(|(n, b)| (n.clone(), *b)).collect())
                .unwrap_or_default();
            top_apps.sort_by_key(|(_, bytes)| std::cmp::Reverse(*bytes));
            top_apps.truncate(3);

            let known = clients.iter().find(|c| c.mac == mac);
            usage_clients.push(ClientUsage {
                mac,
                hostname: known.and_then(|c| c.hostname.clone()),
                ip: known.and_then(|c| c.ip.clone()),
                rx_bytes: rx,
                tx_bytes: tx,
                conns,
                rx_bps,
                tx_bps,
                top_apps: top_apps.into_iter().map(|(n, _)| n).collect(),
            });
        }
        self.last_ts = ts;

        let total_rx_bytes = usage_clients
            .iter()
            .fold(0u64, |sum, c| sum.saturating_add(c.rx_bytes));
        let total_tx_bytes = usage_clients
            .iter()
            .fold(0u64, |sum, c| sum.saturating_add(c.tx_bytes));
        let client_count = usage_clients.len() as u64;
        usage_clients
            .sort_by_key(|c| std::cmp::Reverse(u128::from(c.rx_bytes) + u128::from(c.tx_bytes)));
        usage_clients.truncate(MAX_CLIENTS);

        let mut apps: Vec<AppUsage> = app_totals
            .into_iter()
            .map(|(name, (rx_bytes, tx_bytes, conns))| AppUsage {
                name,
                rx_bytes,
                tx_bytes,
                conns,
            })
            .collect();
        apps.sort_by_key(|a| std::cmp::Reverse(a.rx_bytes + a.tx_bytes));
        if apps.len() > MAX_APPS {
            let rest = apps.split_off(MAX_APPS);
            let other = rest.into_iter().fold(
                AppUsage {
                    name: "Other".into(),
                    ..Default::default()
                },
                |mut acc, a| {
                    acc.rx_bytes += a.rx_bytes;
                    acc.tx_bytes += a.tx_bytes;
                    acc.conns += a.conns;
                    acc
                },
            );
            apps.push(other);
        }

        Usage {
            total_rx_bytes,
            total_tx_bytes,
            client_count,
            clients: usage_clients,
            apps,
        }
    }
}

/// nlbwmon's `-c json` output: `{"columns": [...], "data": [[...], ...]}`. Column order
/// depends on the `-g` grouping, so callers index by name rather than position.
fn parse_columnar(v: &Value) -> Vec<Map<String, Value>> {
    let cols: Vec<&str> = v["columns"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    v["data"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let r = r.as_array()?;
                    Some(
                        cols.iter()
                            .map(|c| c.to_string())
                            .zip(r.iter().cloned())
                            .collect(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn str_col<'a>(row: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

fn u64_col(row: &Map<String, Value>, key: &str) -> u64 {
    row.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Human-friendly app name: nlbwmon's layer7 guess if it has one, else `PROTO/port`
/// (layer7 is often null — unclassified traffic, or protocols like plain IP/ICMP).
fn app_name(row: &Map<String, Value>) -> String {
    if let Some(l7) = str_col(row, "layer7") {
        return l7.to_owned();
    }
    let proto = str_col(row, "proto").unwrap_or("?");
    let port = row.get("port").and_then(Value::as_u64).unwrap_or(0);
    format!("{proto}/{port}")
}

/// Mock mode: nudge rx_bytes/tx_bytes/conns upward so a couple of samples show non-zero
/// rates, the same trick as `ubus::animate` for the hostapd/network.device fixtures. Growth
/// is proportional to each row's own total (so heavy clients still look heavy) with a floor
/// so quiet ones aren't stuck at exactly zero rate.
fn animate(v: &mut Value, secs: f64) {
    let idx = |name: &str| {
        v["columns"]
            .as_array()
            .and_then(|c| c.iter().position(|x| x.as_str() == Some(name)))
    };
    let (rx_i, tx_i, conn_i) = (idx("rx_bytes"), idx("tx_bytes"), idx("conns"));
    let Some(rows) = v["data"].as_array_mut() else {
        return;
    };
    for row in rows {
        let Some(arr) = row.as_array_mut() else {
            continue;
        };
        for i in [rx_i, tx_i].into_iter().flatten() {
            if let Some(n) = arr.get(i).and_then(Value::as_u64) {
                let rate = (n as f64 * 0.00003).max(300.0);
                arr[i] = json!(n + (rate * secs) as u64);
            }
        }
        if let Some(i) = conn_i
            && let Some(n) = arr.get(i).and_then(Value::as_u64)
        {
            arr[i] = json!(n + (secs / 8.0) as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(group: &str) -> Value {
        let path = format!(
            "{}/fixtures/nlbw/nlbw.{group}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn parses_mac_grouping_by_column_name() {
        let rows = parse_columnar(&fixture("mac"));
        assert_eq!(rows.len(), 23);
        let first = &rows[0];
        assert_eq!(str_col(first, "mac"), Some("02:00:00:00:00:16"));
        assert_eq!(u64_col(first, "rx_bytes"), 59561532778);
        assert_eq!(u64_col(first, "tx_bytes"), 10549839598);
        assert_eq!(u64_col(first, "conns"), 105335);
    }

    #[test]
    fn parses_mac_layer7_grouping_with_reordered_columns() {
        // `-g mac,layer7` puts proto/port first, unlike `-g mac` — parsing by name must
        // not care about the difference.
        let rows = parse_columnar(&fixture("mac,layer7"));
        assert_eq!(rows.len(), 163);
        let first = &rows[0];
        assert_eq!(str_col(first, "mac"), Some("02:00:00:00:00:16"));
        assert_eq!(str_col(first, "proto"), Some("TCP"));
        assert_eq!(u64_col(first, "port"), 443);
        assert_eq!(str_col(first, "layer7"), Some("HTTPS"));
    }

    #[test]
    fn null_layer7_falls_back_to_proto_port() {
        let rows = parse_columnar(&fixture("layer7"));
        let ip_row = rows
            .iter()
            .find(|r| str_col(r, "proto") == Some("IP"))
            .unwrap();
        assert_eq!(str_col(ip_row, "layer7"), None);
        assert_eq!(app_name(ip_row), "IP/0");
        let https_row = rows
            .iter()
            .find(|r| str_col(r, "proto") == Some("TCP"))
            .unwrap();
        assert_eq!(app_name(https_row), "HTTPS");
    }

    #[test]
    fn aggregates_real_fixtures_into_usage() {
        let mac_rows = parse_columnar(&fixture("mac"));
        let app_rows = parse_columnar(&fixture("mac,layer7"));
        let mut t = Tracker::default();
        let usage = t.apply(&mac_rows, &app_rows, &[], 1000);

        assert_eq!(usage.clients.len(), 23);
        // Sorted by total bytes, descending.
        assert!(
            usage
                .clients
                .windows(2)
                .all(|w| (w[0].rx_bytes + w[0].tx_bytes) >= (w[1].rx_bytes + w[1].tx_bytes))
        );
        let top = &usage.clients[0];
        assert_eq!(top.mac, "02:00:00:00:00:16");
        assert!(!top.top_apps.is_empty());
        assert_eq!(top.top_apps[0], "HTTPS");
        // First sample ever: no rate yet.
        assert_eq!((top.rx_bps, top.tx_bps), (0, 0));

        // App mix: at most MAX_APPS + 1 ("Other") rows, sorted descending.
        assert!(usage.apps.len() <= MAX_APPS + 1);
        assert!(
            usage
                .apps
                .windows(2)
                .all(|w| (w[0].rx_bytes + w[0].tx_bytes) >= (w[1].rx_bytes + w[1].tx_bytes))
        );
        assert!(usage.apps.iter().any(|a| a.name == "HTTPS"));
    }

    #[test]
    fn usage_totals_include_clients_beyond_the_display_limit() {
        let mac_rows: Vec<Map<String, Value>> = (0..120)
            .map(|i| {
                [
                    (
                        "mac".into(),
                        json!(format!("02:00:00:00:{:02x}:{:02x}", i / 256, i % 256)),
                    ),
                    ("rx_bytes".into(), json!(i + 1)),
                    ("tx_bytes".into(), json!((i + 1) * 2)),
                    ("conns".into(), json!(1)),
                ]
                .into_iter()
                .collect()
            })
            .collect();
        let mut tracker = Tracker::default();
        let usage = tracker.apply(&mac_rows, &[], &[], 1000);
        let serialized = serde_json::to_value(&usage).unwrap();

        assert_eq!(serialized["client_count"], 120);
        assert_eq!(serialized["total_rx_bytes"], 7_260);
        assert_eq!(serialized["total_tx_bytes"], 14_520);
        assert_eq!(usage.clients.len(), MAX_CLIENTS);
        assert!(
            usage
                .clients
                .windows(2)
                .all(|w| { (w[0].rx_bytes + w[0].tx_bytes) >= (w[1].rx_bytes + w[1].tx_bytes) })
        );
    }

    #[test]
    fn ranks_clients_without_overflowing_total_bytes() {
        let row = |mac: &str, rx: u64, tx: u64| -> Map<String, Value> {
            [
                ("mac".into(), json!(mac)),
                ("rx_bytes".into(), json!(rx)),
                ("tx_bytes".into(), json!(tx)),
                ("conns".into(), json!(1)),
            ]
            .into_iter()
            .collect()
        };
        let rows = [
            // Both totals exceed u64::MAX; put the lower total first to expose a
            // saturating-sort tie that would preserve the wrong order.
            row("02:00:00:00:00:01", u64::MAX, 1),
            row("02:00:00:00:00:02", u64::MAX, 100),
            row("02:00:00:00:00:03", 0, 100),
        ];

        let usage = Tracker::default().apply(&rows, &[], &[], 1000);

        assert_eq!(usage.clients[0].mac, "02:00:00:00:00:02");
        assert_eq!(usage.clients[1].mac, "02:00:00:00:00:01");
        assert_eq!(usage.clients[2].mac, "02:00:00:00:00:03");
    }

    #[test]
    fn joins_hostname_and_ip_from_clients() {
        let mac_rows = parse_columnar(&fixture("mac"));
        let clients = vec![Client {
            mac: "02:00:00:00:00:16".into(),
            ip: Some("192.0.2.250".into()),
            hostname: Some("test-device".into()),
            ..Default::default()
        }];
        let mut t = Tracker::default();
        let usage = t.apply(&mac_rows, &[], &clients, 1000);
        let c = usage
            .clients
            .iter()
            .find(|c| c.mac == "02:00:00:00:00:16")
            .unwrap();
        assert_eq!(c.hostname.as_deref(), Some("test-device"));
        assert_eq!(c.ip.as_deref(), Some("192.0.2.250"));
        let unknown = usage.clients.iter().find(|c| c.hostname.is_none()).unwrap();
        assert_eq!(unknown.hostname, None);
    }

    #[test]
    fn rate_is_delta_over_time_between_samples() {
        let mut t = Tracker::default();
        let row = |rx: u64, tx: u64| -> Map<String, Value> {
            [
                ("mac".into(), json!("aa:bb:cc:dd:ee:ff")),
                ("rx_bytes".into(), json!(rx)),
                ("tx_bytes".into(), json!(tx)),
                ("conns".into(), json!(1)),
            ]
            .into_iter()
            .collect()
        };
        let first = t.apply(&[row(1_000, 500)], &[], &[], 1000);
        assert_eq!((first.clients[0].rx_bps, first.clients[0].tx_bps), (0, 0));

        // 10s later, +12_500 rx bytes and +1_250 tx bytes -> 10_000 bps / 1_000 bps.
        let second = t.apply(&[row(13_500, 1_750)], &[], &[], 1010);
        assert_eq!(
            (second.clients[0].rx_bps, second.clients[0].tx_bps),
            (10_000, 1_000)
        );
    }

    #[test]
    fn counter_reset_does_not_go_negative() {
        let mut t = Tracker::default();
        let row = |rx: u64, tx: u64| -> Map<String, Value> {
            [
                ("mac".into(), json!("aa:bb:cc:dd:ee:ff")),
                ("rx_bytes".into(), json!(rx)),
                ("tx_bytes".into(), json!(tx)),
                ("conns".into(), json!(1)),
            ]
            .into_iter()
            .collect()
        };
        t.apply(&[row(1_000_000, 500_000)], &[], &[], 1000);
        // New nlbwmon month (or a nlbwmon restart) resets the counters to near zero.
        let after_reset = t.apply(&[row(100, 50)], &[], &[], 1030);
        assert_eq!(
            (after_reset.clients[0].rx_bps, after_reset.clients[0].tx_bps),
            (0, 0)
        );
        // The next sample after that measures cleanly from the new baseline.
        let next = t.apply(&[row(3_100, 350)], &[], &[], 1060);
        assert_eq!((next.clients[0].rx_bps, next.clients[0].tx_bps), (800, 80));
    }
}
