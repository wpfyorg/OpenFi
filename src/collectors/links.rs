//! Mesh peers (802.11s), the wired uplink (ap-2's `lan1`), and the WAN link (gateway only).
//!
//! Mesh peers are identified by MAC: `iwinfo assoclist` on the mesh iface returns the peer's
//! *own* radio MAC, so we first collect every node's `phy1-mesh0` MAC (via `network.device
//! status`) into a lookup table, then map each assoclist entry back to a node name.

use std::collections::HashMap;

use serde_json::json;

use crate::{
    config::NodeConfig,
    model::{Link, LinkKind, Quality},
    ubus::Ubus,
};

const MESH_IFACE: &str = "phy1-mesh0";

/// Every node's mesh-iface MAC, lowercased, so `collect` can turn a peer MAC into a node name.
pub async fn mesh_mac_table(nodes: &[(NodeConfig, Ubus)]) -> HashMap<String, String> {
    let mut table = HashMap::new();
    for (nc, ubus) in nodes {
        if let Ok(v) = ubus
            .call("network.device", "status", json!({ "name": MESH_IFACE }))
            .await
            && let Some(mac) = v["macaddr"].as_str()
        {
            table.insert(mac.to_lowercase(), nc.name.clone());
        }
    }
    table
}

pub async fn collect(
    ubus: &Ubus,
    nc: &NodeConfig,
    gateway_name: &str,
    is_gateway: bool,
    mesh_macs: &HashMap<String, String>,
    wan_up: bool,
) -> Vec<Link> {
    let mut links = vec![];

    if let Ok(v) = ubus
        .call("iwinfo", "assoclist", json!({ "device": MESH_IFACE }))
        .await
    {
        for r in v["results"].as_array().into_iter().flatten() {
            let Some(mac) = r["mac"].as_str() else {
                continue;
            };
            let peer = mesh_macs
                .get(&mac.to_lowercase())
                .cloned()
                .unwrap_or_else(|| mac.to_lowercase());
            let signal_dbm = r["signal"].as_i64().map(|s| s as i32);
            let quality = signal_dbm
                .map(Quality::from_mesh_signal)
                .unwrap_or(Quality::Weak);
            links.push(Link {
                kind: LinkKind::Mesh,
                peer,
                signal_dbm,
                rx_mbps: kbit_to_mbps(r["rx"]["rate"].as_u64()),
                tx_mbps: kbit_to_mbps(r["tx"]["rate"].as_u64()),
                quality,
            });
        }
    }

    if let Some(iface) = &nc.wired_uplink
        && let Ok(v) = ubus
            .call("network.device", "status", json!({ "name": iface }))
            .await
        && v["up"].as_bool().unwrap_or(false)
    {
        // Verified on mainrouter (2026-09-24): `network.device status {"name":"lan1"}` reports
        // `"speed": "1000F"` — a string, the negotiated Mbps followed by a duplex letter
        // (`F`ull/`H`alf), not a bare number.
        let speed = parse_speed(&v["speed"]);
        links.push(Link {
            kind: LinkKind::Wired,
            peer: gateway_name.to_owned(),
            signal_dbm: None,
            rx_mbps: speed,
            tx_mbps: speed,
            quality: Quality::Strong,
        });
    }

    if is_gateway && wan_up {
        links.push(Link {
            kind: LinkKind::Wan,
            peer: "ONT".into(),
            signal_dbm: None,
            rx_mbps: None,
            tx_mbps: None,
            quality: Quality::Strong,
        });
    }

    links
}

/// iwinfo reports PHY rates in kbit/s; the contract wants Mbps.
fn kbit_to_mbps(kbit: Option<u64>) -> Option<u32> {
    kbit.map(|k| (k / 1000) as u32)
}

/// `network.device status`'s `speed` field: a bare number on some drivers, but on this board's
/// DSA switch ports a string like `"1000F"` (Mbps + duplex letter, `F`ull/`H`alf) — verified on
/// mainrouter (2026-09-24). Accepts either shape; leading digits are the Mbps value.
fn parse_speed(v: &serde_json::Value) -> Option<u32> {
    if let Some(n) = v.as_u64() {
        return Some(n as u32);
    }
    let s = v.as_str()?;
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_string_speed_with_duplex_letter() {
        assert_eq!(parse_speed(&json!("1000F")), Some(1000));
        assert_eq!(parse_speed(&json!("100H")), Some(100));
    }

    #[test]
    fn parses_bare_numeric_speed() {
        assert_eq!(parse_speed(&json!(1000)), Some(1000));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_speed(&json!("unknown")), None);
        assert_eq!(parse_speed(&json!(null)), None);
    }
}
