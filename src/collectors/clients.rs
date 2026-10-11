use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use serde_json::json;

use crate::{collectors::uci, model::Client, ubus::Ubus};

/// dnsmasq lease file: `<expiry> <mac> <ip> <hostname> <client-id>`.
pub async fn read_leases(path: &Path) -> Vec<Client> {
    let Ok(text) = tokio::fs::read_to_string(path).await else {
        return vec![];
    };
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 4).then(|| Client {
                mac: f[1].to_lowercase(),
                ip: Some(f[2].to_owned()),
                hostname: (f[3] != "*").then(|| f[3].to_owned()),
                ..Default::default()
            })
        })
        .collect()
}

/// A wired-looking (ARP-visible) neighbor: `(mac, ip, reachable)`. `reachable` is the ARP
/// `0x2` "complete" flag; `0x0`/incomplete entries are stale cache misses, not live devices.
pub struct ArpEntry {
    pub mac: String,
    pub ip: String,
    pub reachable: bool,
}

/// `/proc/net/arp` via the `file` ubus object (works locally and through rpcd, so it also
/// covers ap-2's directly-wired ports). Assumes `file read` returns `{"data": "<contents>"}` —
/// unverified against the real rpcd `file` plugin; see plan.md.
pub async fn read_arp(ubus: &Ubus) -> Vec<ArpEntry> {
    let Ok(v) = ubus
        .call("file", "read", json!({ "path": "/proc/net/arp" }))
        .await
    else {
        return vec![];
    };
    let Some(text) = v["data"].as_str() else {
        return vec![];
    };
    text.lines()
        .skip(1) // header: "IP address  HW type  Flags  HW address  Mask  Device"
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 4 {
                return None;
            }
            let flags = u32::from_str_radix(f[2].trim_start_matches("0x"), 16).unwrap_or(0);
            Some(ArpEntry {
                ip: f[0].to_owned(),
                mac: f[3].to_lowercase(),
                reachable: flags & 0x2 != 0,
            })
        })
        .collect()
}

/// `luci-rpc getHostHints`: friendlier per-MAC names (including ones set by hand in LuCI),
/// keyed by MAC. Only worth calling on the gateway, which has the full DHCP/host picture.
pub async fn read_host_hints(ubus: &Ubus) -> HashMap<String, String> {
    let Ok(v) = ubus.call("luci-rpc", "getHostHints", json!({})).await else {
        return HashMap::new();
    };
    let Some(obj) = v.as_object() else {
        return HashMap::new();
    };
    obj.iter()
        .filter_map(|(mac, hint)| {
            let name = hint["name"]
                .as_str()
                .or_else(|| hint["hostname"].as_str())?;
            Some((mac.to_lowercase(), name.to_owned()))
        })
        .collect()
}

/// Static DHCP reservations (`uci show dhcp`, `config host` sections) — gateway only.
pub async fn read_reserved(ubus: &Ubus) -> HashSet<String> {
    uci::get(ubus, "dhcp")
        .await
        .into_iter()
        .filter(|s| s.kind == "host")
        .filter_map(|s| s.get("mac").map(|m| m.to_lowercase()))
        .collect()
}

/// `/etc/device-qos-whitelist`: `<MAC> [label]` per line, comments/blank lines ignored.
pub async fn read_whitelist(ubus: &Ubus) -> HashSet<String> {
    let Ok(v) = ubus
        .call(
            "file",
            "read",
            json!({ "path": "/etc/device-qos-whitelist" }),
        )
        .await
    else {
        return HashSet::new();
    };
    let Some(text) = v["data"].as_str() else {
        return HashSet::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_lowercase)
        .collect()
}

/// Tracks the last time each MAC was seen online, across polls (in-memory only — a restart
/// resets it, which just means `last_seen` reads `null` until the next sighting).
#[derive(Default)]
pub struct Tracker {
    last_seen: HashMap<String, u64>,
}

impl Tracker {
    /// Join DHCP leases, WiFi stations, and ARP-visible wired neighbors by MAC, then layer on
    /// hostnames, reservations, and the QoS whitelist. Any MAC associated to a hostapd iface is
    /// WiFi; everything else that showed up in ARP is "wired" per the contract's definition.
    #[allow(clippy::too_many_arguments)]
    pub fn merge(
        &mut self,
        ts: u64,
        leases: Vec<Client>,
        stations: Vec<Client>,
        arp: Vec<(String, Vec<ArpEntry>)>,
        host_hints: &HashMap<String, String>,
        reserved: &HashSet<String>,
        whitelisted: &HashSet<String>,
    ) -> Vec<Client> {
        let mut by_mac: HashMap<String, Client> =
            leases.into_iter().map(|c| (c.mac.clone(), c)).collect();

        for s in stations {
            by_mac
                .entry(s.mac.clone())
                .and_modify(|c| {
                    c.node = s.node.clone();
                    c.ssid = s.ssid.clone();
                    c.band = s.band;
                    c.signal_dbm = s.signal_dbm;
                    c.rx_rate = s.rx_rate;
                    c.tx_rate = s.tx_rate;
                    c.online = true;
                    c.wired = false;
                })
                .or_insert(s);
        }

        for (node, entries) in arp {
            for e in entries {
                if !e.reachable {
                    continue;
                }
                let c = by_mac.entry(e.mac.clone()).or_insert_with(|| Client {
                    mac: e.mac.clone(),
                    ..Default::default()
                });
                if c.ip.is_none() {
                    c.ip = Some(e.ip.clone());
                }
                // A station already associated over WiFi is not wired, even if some stale ARP
                // entry (from before it roamed onto WiFi) still names it reachable.
                if !c.online {
                    c.online = true;
                    c.wired = true;
                    c.node = Some(node.clone());
                }
            }
        }

        for c in by_mac.values_mut() {
            if c.hostname.is_none()
                && let Some(name) = host_hints.get(&c.mac)
            {
                c.hostname = Some(name.clone());
            }
            c.reserved = reserved.contains(&c.mac);
            c.whitelisted = whitelisted.contains(&c.mac);
            if c.online {
                self.last_seen.insert(c.mac.clone(), ts);
            }
            c.last_seen = self.last_seen.get(&c.mac).copied();
        }

        let mut out: Vec<Client> = by_mac.into_values().collect();
        out.sort_by(|a, b| {
            b.online
                .cmp(&a.online)
                .then_with(|| a.hostname.cmp(&b.hostname))
        });
        out
    }

    /// Promote wired clients reported online by qos-status, whose neighbor state can be
    /// inconclusive. Called after `merge` so the tracker can retain the observation timestamp.
    pub fn promote_qos_online(
        &mut self,
        clients: &mut [Client],
        qos_online: &HashMap<String, bool>,
        gateway_name: &str,
        ts: u64,
    ) {
        for client in clients {
            if !client.online && qos_online.get(&client.mac) == Some(&true) {
                client.online = true;
                client.wired = true;
                if client.node.is_none() {
                    client.node = Some(gateway_name.to_owned());
                }
                self.last_seen.insert(client.mac.clone(), ts);
                client.last_seen = Some(ts);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qos_only_online_client_gets_current_poll_as_last_seen() {
        let mut tracker = Tracker::default();
        let mut clients = vec![Client {
            mac: "aa:bb:cc:dd:ee:ff".into(),
            ..Default::default()
        }];
        let qos_online = HashMap::from([("aa:bb:cc:dd:ee:ff".to_owned(), true)]);

        tracker.promote_qos_online(&mut clients, &qos_online, "gateway", 42_000);

        assert!(clients[0].online);
        assert!(clients[0].wired);
        assert_eq!(clients[0].node.as_deref(), Some("gateway"));
        assert_eq!(clients[0].last_seen, Some(42_000));
    }
}
