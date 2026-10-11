//! `GET /wifi/networks`, built fresh from `uci get wireless` on every node (not part of the
//! poll snapshot, since it's read rarely and must always reflect the config actions write to).
//!
//! Networks are grouped by SSID (mesh ifaces group by `mesh_id` instead, since they have no
//! SSID) across every node and every radio. `purpose`/`security`/roaming flags are read off the
//! *first* matching wifi-iface section found (config order: mainrouter, then ap-2, then ap-4) —
//! the same logical network is expected to be configured identically everywhere, which is what
//! the "apply plan" (a later phase) enforces.
//!
//! Assumption, unverified: whether `usteer` actually manages a given SSID isn't a `wireless`
//! uci option at all (it's driven by a separate `usteer` config's interface list), so `usteer`
//! here is approximated as "true for every non-backhaul network" — good enough to render the
//! toggle in the UI, but worth wiring to the real `usteer` config once its shape is captured.

use std::collections::HashMap;

use crate::{
    collectors::uci::{self, Section},
    config::NodeConfig,
    model::{Band, BandBroadcast, Client, Pmf, Purpose, Roaming, Security, WifiNetwork},
    ubus::Ubus,
};

struct NodeWireless {
    node: String,
    sections: Vec<Section>,
}

pub async fn collect(nodes: &[(NodeConfig, Ubus)], clients: &[Client]) -> Vec<WifiNetwork> {
    let mut per_node = vec![];
    for (nc, ubus) in nodes {
        per_node.push(NodeWireless {
            node: nc.name.clone(),
            sections: uci::get(ubus, "wireless").await,
        });
    }

    // ssid/mesh_id -> (id, purpose, locked, security, roaming section, bands seen)
    struct Group<'a> {
        id: String,
        mode: &'a str,
        encryption: Option<&'a str>,
        roaming_section: &'a Section,
        bands: Vec<Band>,
    }
    let mut groups: HashMap<String, Group> = HashMap::new();
    let mut order = vec![]; // first-seen order, for stable output

    for nw in &per_node {
        let device_band: HashMap<&str, Band> = nw
            .sections
            .iter()
            .filter(|s| s.kind == "wifi-device")
            .map(|s| {
                let band = if s.get("band") == Some("5g") {
                    Band::Ghz5
                } else {
                    Band::Ghz24
                };
                (s.name.as_str(), band)
            })
            .collect();
        for s in nw.sections.iter().filter(|s| s.kind == "wifi-iface") {
            let key = match s.get("mode") {
                Some("mesh") => s.get("mesh_id"),
                _ => s.get("ssid"),
            };
            let Some(key) = key else { continue };
            let Some(&band) = s.get("device").and_then(|d| device_band.get(d)) else {
                continue;
            };
            let g = groups.entry(key.to_owned()).or_insert_with(|| {
                order.push(key.to_owned());
                Group {
                    id: s.name.clone(),
                    mode: s.get("mode").unwrap_or("ap"),
                    encryption: s.get("encryption"),
                    roaming_section: s,
                    bands: vec![],
                }
            });
            if !g.bands.contains(&band) {
                g.bands.push(band);
            }
        }
    }

    order
        .into_iter()
        .filter_map(|ssid| {
            let g = groups.get(&ssid)?;
            let purpose = purpose_of(g.mode, &ssid);
            let locked = purpose == Purpose::Backhaul;
            let broadcast = per_node
                .iter()
                .map(|nw| (nw.node.clone(), broadcast_for(&nw.sections, &ssid)))
                .collect();
            let clients_on = clients
                .iter()
                .filter(|c| c.ssid.as_deref() == Some(ssid.as_str()))
                .count();
            Some(WifiNetwork {
                id: g.id.clone(),
                ssid: ssid.clone(),
                purpose,
                locked,
                security: security_of(g.encryption),
                bands: g.bands.clone(),
                broadcast,
                roaming: roaming_of(g.roaming_section, !locked),
                clients: clients_on,
            })
        })
        .collect()
}

fn purpose_of(mode: &str, ssid: &str) -> Purpose {
    if mode == "mesh" {
        return Purpose::Backhaul;
    }
    let lower = ssid.to_lowercase();
    if lower.contains("legacy") || lower.contains("iot") {
        Purpose::Iot
    } else {
        Purpose::Primary
    }
}

fn security_of(encryption: Option<&str>) -> Security {
    match encryption.and_then(|e| e.split('+').next()) {
        Some("sae-mixed") => Security::SaeMixed,
        Some("psk2") => Security::Psk2,
        Some("sae") => Security::Sae,
        _ => Security::None,
    }
}

fn roaming_of(s: &Section, usteer_guess: bool) -> Roaming {
    Roaming {
        ft: s.flag("ieee80211r"),
        ft_over_ds: s.flag("ft_over_ds"),
        mobility_domain: s.get("mobility_domain").map(str::to_owned),
        rrm: s.flag("ieee80211k"),
        btm: s.flag("ieee80211v"),
        pmf: match s.get("ieee80211w") {
            Some("2") => Pmf::Required,
            Some("1") => Pmf::Optional,
            _ => Pmf::Off,
        },
        usteer: usteer_guess,
    }
}

/// Whether this node broadcasts `ssid` on each band: `None` = that band's radio is absent or
/// disabled on this node; `Some(bool)` = radio present and enabled, broadcasting or not.
fn broadcast_for(sections: &[Section], ssid: &str) -> BandBroadcast {
    let mut out = BandBroadcast::default();
    for dev in sections.iter().filter(|s| s.kind == "wifi-device") {
        let band = if dev.get("band") == Some("5g") {
            Band::Ghz5
        } else {
            Band::Ghz24
        };
        let value = if dev.flag("disabled") {
            None
        } else {
            let on = sections.iter().any(|s| {
                s.kind == "wifi-iface"
                    && s.get("device") == Some(&dev.name)
                    && matches!(s.get("mode"), Some("ap") | Some("mesh"))
                    && matches!(s.get("ssid").or_else(|| s.get("mesh_id")), Some(v) if v == ssid)
            });
            Some(on)
        };
        match band {
            Band::Ghz24 => out.ghz24 = value,
            Band::Ghz5 => out.ghz5 = value,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    async fn nodes() -> Vec<(NodeConfig, Ubus)> {
        let mut cfg = Config::default();
        cfg.use_fixtures(std::path::PathBuf::from(format!(
            "{}/fixtures",
            env!("CARGO_MANIFEST_DIR")
        )));
        cfg.nodes
            .iter()
            .map(|n| (n.clone(), cfg.ubus_for(n)))
            .collect()
    }

    #[tokio::test]
    async fn groups_by_ssid_across_nodes() {
        let nodes = nodes().await;
        let networks = collect(&nodes, &[]).await;
        let names: Vec<&str> = networks.iter().map(|n| n.ssid.as_str()).collect();
        assert!(names.contains(&"Example-WiFi-13"));
        assert!(names.contains(&"Example-Legacy"));
        assert!(networks.iter().any(|n| n.purpose == Purpose::Backhaul));
    }

    #[tokio::test]
    async fn primary_network_is_dual_band_and_unlocked() {
        let nodes = nodes().await;
        let networks = collect(&nodes, &[]).await;
        let primary = networks
            .iter()
            .find(|n| n.ssid == "Example-WiFi-13")
            .unwrap();
        assert_eq!(primary.purpose, Purpose::Primary);
        assert!(!primary.locked);
        assert_eq!(primary.security, Security::SaeMixed);
        assert!(primary.bands.contains(&Band::Ghz24));
        assert!(primary.bands.contains(&Band::Ghz5));
        assert!(primary.roaming.ft);
    }

    #[tokio::test]
    async fn backhaul_is_locked_and_ap4_radio0_disabled_shows_null_not_false() {
        let nodes = nodes().await;
        let networks = collect(&nodes, &[]).await;
        let backhaul = networks
            .iter()
            .find(|n| n.purpose == Purpose::Backhaul)
            .unwrap();
        assert!(backhaul.locked);
        assert_eq!(backhaul.security, Security::Sae);

        let primary = networks
            .iter()
            .find(|n| n.ssid == "Example-WiFi-13")
            .unwrap();
        let ap4 = &primary.broadcast["ap-4"];
        assert_eq!(ap4.ghz24, None, "ap-4's radio0 is disabled, not just off");
        assert_eq!(ap4.ghz5, Some(true));
    }

    #[tokio::test]
    async fn legacy_ssid_is_iot_purpose() {
        let nodes = nodes().await;
        let networks = collect(&nodes, &[]).await;
        let legacy = networks
            .iter()
            .find(|n| n.ssid == "Example-Legacy")
            .unwrap();
        assert_eq!(legacy.purpose, Purpose::Iot);
        assert_eq!(legacy.security, Security::Psk2);
    }
}
