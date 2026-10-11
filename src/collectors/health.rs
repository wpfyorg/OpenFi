//! Rolls the snapshot up into one `ok`/`degraded`/`down` status with human reasons.
//!
//! Thresholds: the gateway offline or the WAN down are outages (`down`); an AP offline, a mesh
//! link weaker than -78 dBm (the contract's own "weak" cutoff), a sensor within 10°C of its
//! hot/critical trip point, or RAM over 85% used are all `degraded`. The 10°C and 85% figures
//! aren't in the contract — they're a reasonable-looking guess, called out here so they're easy
//! to tune once the real router shows what "about to matter" looks like.

use crate::model::{Health, HealthStatus, LinkKind, Node, Quality, Role};

const TEMP_WARN_MARGIN_C: f32 = 10.0;
const MEM_USED_WARN_FRACTION: f64 = 0.85;

pub fn compute(nodes: &[Node], wan_up: bool) -> Health {
    let mut reasons = vec![];
    let mut down = false;
    let mut degraded = false;

    if !wan_up {
        reasons.push("WAN is down".to_owned());
        down = true;
    }

    for n in nodes {
        if !n.online {
            reasons.push(format!("{} is offline", n.name));
            if n.role == Role::Gateway {
                down = true;
            } else {
                degraded = true;
            }
            continue;
        }
        for l in &n.links {
            if l.kind == LinkKind::Mesh && l.quality == Quality::Weak {
                reasons.push(format!(
                    "weak mesh link {} \u{2192} {} ({} dBm)",
                    n.name,
                    l.peer,
                    l.signal_dbm
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "?".into())
                ));
                degraded = true;
            }
        }
        for t in &n.temps {
            if let Some(crit) = t.crit
                && t.celsius >= crit - TEMP_WARN_MARGIN_C
            {
                reasons.push(format!(
                    "{} {} near critical temperature ({:.0}\u{b0}C, crit {:.0}\u{b0}C)",
                    n.name, t.label, t.celsius, crit
                ));
                degraded = true;
            }
        }
        if n.mem_total > 0 {
            let used = 1.0 - n.mem_available as f64 / n.mem_total as f64;
            if used > MEM_USED_WARN_FRACTION {
                reasons.push(format!("{} RAM at {:.0}%", n.name, used * 100.0));
                degraded = true;
            }
        }
    }

    let status = if down {
        HealthStatus::Down
    } else if degraded {
        HealthStatus::Degraded
    } else {
        HealthStatus::Ok
    };
    Health { status, reasons }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Link, Temp};

    fn ok_node(name: &str, role: Role) -> Node {
        Node {
            name: name.into(),
            role,
            online: true,
            mem_total: 1000,
            mem_available: 500,
            ..Default::default()
        }
    }

    #[test]
    fn all_good_is_ok() {
        let h = compute(&[ok_node("mainrouter", Role::Gateway)], true);
        assert_eq!(h.status, HealthStatus::Ok);
        assert!(h.reasons.is_empty());
    }

    #[test]
    fn wan_down_is_down() {
        let h = compute(&[ok_node("mainrouter", Role::Gateway)], false);
        assert_eq!(h.status, HealthStatus::Down);
        assert!(h.reasons.iter().any(|r| r.contains("WAN")));
    }

    #[test]
    fn gateway_offline_is_down_but_ap_offline_is_degraded() {
        let mut gw_down = ok_node("mainrouter", Role::Gateway);
        gw_down.online = false;
        assert_eq!(compute(&[gw_down], true).status, HealthStatus::Down);

        let mut ap_down = ok_node("ap-2", Role::Ap);
        ap_down.online = false;
        assert_eq!(compute(&[ap_down], true).status, HealthStatus::Degraded);
    }

    #[test]
    fn weak_mesh_link_degrades() {
        let mut n = ok_node("ap-2", Role::Ap);
        n.links.push(Link {
            kind: LinkKind::Mesh,
            peer: "mainrouter".into(),
            signal_dbm: Some(-82),
            rx_mbps: None,
            tx_mbps: None,
            quality: Quality::Weak,
        });
        let h = compute(&[n], true);
        assert_eq!(h.status, HealthStatus::Degraded);
        assert!(h.reasons[0].contains("weak mesh link"));
    }

    #[test]
    fn near_critical_temp_degrades() {
        let mut n = ok_node("mainrouter", Role::Gateway);
        n.temps.push(Temp {
            label: "CPU".into(),
            celsius: 118.0,
            crit: Some(125.0),
        });
        assert_eq!(compute(&[n], true).status, HealthStatus::Degraded);
    }

    #[test]
    fn high_ram_degrades() {
        let mut n = ok_node("mainrouter", Role::Gateway);
        n.mem_total = 1000;
        n.mem_available = 100;
        assert_eq!(compute(&[n], true).status, HealthStatus::Degraded);
    }
}
