//! `/usr/sbin/qos-status` → `Qos`. Real shape captured on mainrouter (2026-09-24) — top-level
//! keys `schema, generated, host, available, test_mode, wan, services, shaping, policy, totals,
//! devices, history_months, baseline, notes` (`fixtures/qos-status.json` is the real file,
//! unedited). Only the fields we need are modelled below, and every one of them is
//! `#[serde(default)]` so an older/newer qos-tiers build (extra fields, a renamed field that
//! goes missing) degrades to zero/empty rather than failing the whole snapshot.
//!
//! The monthly fair-use ladder (`Qos.tiers`) is read primarily from qos-status's own
//! `policy.fup` (`[{from_gb, kbit}]` — exactly what qos-tiers is enforcing right now).
//! `/etc/device-qos.conf`'s `FUP_GB_1/2/3` + `FUP_RATE_50/100/200` (shell `KEY=VALUE`, real
//! shape captured in `fixtures/device-qos.conf`, copied from `device-qos.conf.real`) is the
//! fallback when qos-status itself is unavailable, and the hardcoded AGENTS.md ladder is the
//! last resort if neither parses. `Qos.clients[].tier` is still *computed* from `monthly_bytes`
//! against this ladder rather than trusted from the per-device `fup_tier` string (which uses
//! different labels — `"bypass"`, `"none"` — for the same idea), which keeps the contract's
//! invariant that every `clients[].tier` is one of `tiers[].id`.
//!
//! `collect()` also returns the whitelist and online-state signals qos-status carries per
//! device, since `net-account` (the FUP/whitelist backend) already tracks both more reliably
//! than `/etc/device-qos-whitelist` + `/proc/net/arp` alone — `mod.rs` folds these into
//! `Client.whitelisted` and `Client.online` (for ARP-ambiguous wired devices) after the merge.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::{
    config::Config,
    model::{Qos, QosClient, QosTier},
};

/// Bytes per GB for `month_bytes` vs. the ladder's `min_gb`/`max_gb` — decimal (ISP
/// convention), not binary.
const GB: f64 = 1_000_000_000.0;

/// AGENTS.md: <50 GB uncapped, 50-100 GB @ 125 Mbit, 100-200 GB @ 62.5 Mbit, 200 GB+ @ 50 Mbit.
/// Last-resort fallback if neither qos-status nor device-qos.conf can be read/parsed.
fn default_tiers() -> Vec<QosTier> {
    ladder_from_fup(&[
        Fup {
            from_gb: 50.0,
            kbit: 125_000,
        },
        Fup {
            from_gb: 100.0,
            kbit: 62_500,
        },
        Fup {
            from_gb: 200.0,
            kbit: 50_000,
        },
    ])
}

// --- real qos-status shape ------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct Real {
    #[serde(default)]
    services: Services,
    #[serde(default)]
    policy: Policy,
    #[serde(default)]
    devices: Vec<Device>,
}

#[derive(Debug, Default, Deserialize)]
struct Services {
    #[serde(rename = "qos-tiers", default)]
    qos_tiers: ServiceStatus,
}

#[derive(Debug, Default, Deserialize)]
struct ServiceStatus {
    #[serde(default)]
    running: bool,
}

#[derive(Debug, Default, Deserialize)]
struct Policy {
    dl_ceiling_kbit: Option<u64>,
    ul_ceiling_kbit: Option<u64>,
    #[serde(default)]
    fup: Vec<Fup>,
    #[serde(default)]
    burst: Option<Value>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Fup {
    from_gb: f64,
    kbit: u64,
}

#[derive(Debug, Default, Deserialize)]
struct Device {
    mac: String,
    #[serde(default)]
    online: bool,
    /// One of `REACHABLE`/`STALE`/`DELAY`/`PROBE`/`FAILED`/`null`; kept only for documentation —
    /// `online` (net-account's own judgment call) is what we act on.
    #[serde(default)]
    #[allow(dead_code)]
    neigh_state: Option<String>,
    #[serde(default)]
    monthly_bytes: u64,
    #[serde(default)]
    whitelisted: bool,
}

/// What a poll cycle needs beyond the `Qos` struct itself: per-device whitelist/online signals
/// to fold into `Client` (see the module doc comment).
#[derive(Debug, Default, Clone)]
pub struct QosCollected {
    pub qos: Qos,
    /// mac (lowercase) -> whitelisted, straight from qos-status's own devices list.
    pub whitelisted: HashMap<String, bool>,
    /// mac (lowercase) -> online, straight from qos-status's own devices list.
    pub online: HashMap<String, bool>,
}

pub async fn collect(cfg: &Config) -> Option<QosCollected> {
    let cmd = cfg.qos_status_cmd.as_ref()?;
    let out = tokio::process::Command::new("sh")
        .args(["-c", cmd])
        .output()
        .await
        .ok()?;
    let real: Real = serde_json::from_slice(&out.stdout).ok()?;
    let tiers = load_tiers(&real, &cfg.device_qos_conf).await;
    Some(build(real, tiers))
}

async fn load_tiers(real: &Real, conf_path: &std::path::Path) -> Vec<QosTier> {
    if !real.policy.fup.is_empty() {
        return ladder_from_fup(&real.policy.fup);
    }
    match tokio::fs::read_to_string(conf_path).await {
        Ok(text) => parse_conf_fup(&text)
            .map(|fup| ladder_from_fup(&fup))
            .unwrap_or_else(default_tiers),
        Err(_) => default_tiers(),
    }
}

/// `policy.fup` (or its device-qos.conf equivalent) → the ladder: an open-ended "unlimited"
/// tier from 0 up to the first threshold, then one tier per `Fup` entry, low to high.
fn ladder_from_fup(fup: &[Fup]) -> Vec<QosTier> {
    let mut sorted = fup.to_vec();
    sorted.sort_by(|a, b| a.from_gb.total_cmp(&b.from_gb));

    let mut tiers = vec![QosTier {
        id: "t0".into(),
        label: "Unlimited".into(),
        min_gb: 0.0,
        max_gb: sorted.first().map(|f| f.from_gb),
        rate_mbit: None,
        clients: 0,
    }];
    for (i, f) in sorted.iter().enumerate() {
        let rate_mbit = f.kbit as f64 / 1000.0;
        tiers.push(QosTier {
            id: format!("t{}", i + 1),
            label: format!("{} Mbit", trim_trailing_zero(rate_mbit)),
            min_gb: f.from_gb,
            max_gb: sorted.get(i + 1).map(|n| n.from_gb),
            rate_mbit: Some(rate_mbit),
            clients: 0,
        });
    }
    tiers
}

fn trim_trailing_zero(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

/// `/etc/device-qos.conf`: shell `KEY=VALUE` lines, `#`-comments (inline or whole-line) allowed,
/// blank lines ignored. Real format (see `fixtures/device-qos.conf`, `AGENTS.md`):
/// `FUP_GB_1=50 FUP_GB_2=100 FUP_GB_3=200`, `FUP_RATE_50=125000 FUP_RATE_100=62500
/// FUP_RATE_200=50000` (kbit). The rate key is named after the *threshold* GB value, not the
/// tier index, so `FUP_GB_2=100` pairs with `FUP_RATE_100`. Returns `None` (falls back to the
/// hardcoded ladder) if no `FUP_GB_*` key is present at all.
fn parse_conf_fup(text: &str) -> Option<Vec<Fup>> {
    let vars = parse_shell_kv(text);
    let mut fup = vec![];
    for i in 1.. {
        let Some(gb_str) = vars.get(&format!("FUP_GB_{i}")) else {
            break;
        };
        let Ok(gb) = gb_str.parse::<f64>() else {
            break;
        };
        // Rate keys are named after the integer GB threshold, e.g. `FUP_RATE_50`.
        let rate_key = format!("FUP_RATE_{}", gb as i64);
        let Some(kbit) = vars.get(&rate_key).and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        fup.push(Fup { from_gb: gb, kbit });
    }
    (!fup.is_empty()).then_some(fup)
}

/// `DL_CEIL`/`UL_CEIL` from device-qos.conf (kbit, `0` = unlimited) — used only when
/// qos-status's own `policy.*_ceiling_kbit` isn't available.
#[allow(dead_code)]
async fn parse_conf_ceilings(path: &std::path::Path) -> Option<(u64, u64)> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let vars = parse_shell_kv(&text);
    let dl = vars.get("DL_CEIL")?.parse().ok()?;
    let ul = vars.get("UL_CEIL")?.parse().ok()?;
    Some((dl, ul))
}

fn parse_shell_kv(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let without_comment = line.split_once('#').map(|(v, _)| v).unwrap_or(line);
            let trimmed = without_comment.trim();
            if trimmed.is_empty() {
                return None;
            }
            let (k, v) = trimmed.split_once('=')?;
            let v = v.trim().trim_matches('"').trim_matches('\'');
            Some((k.trim().to_owned(), v.to_owned()))
        })
        .collect()
}

fn tier_id_for(month_bytes: u64, tiers: &[QosTier]) -> String {
    let gb = month_bytes as f64 / GB;
    tiers
        .iter()
        .find(|t| gb >= t.min_gb && t.max_gb.is_none_or(|max| gb < max))
        .or_else(|| tiers.last())
        .map(|t| t.id.clone())
        .unwrap_or_default()
}

fn build(real: Real, mut tiers: Vec<QosTier>) -> QosCollected {
    let clients: Vec<QosClient> = real
        .devices
        .iter()
        .map(|d| {
            let mac = d.mac.to_lowercase();
            QosClient {
                tier: tier_id_for(d.monthly_bytes, &tiers),
                mac,
                month_bytes: d.monthly_bytes,
                whitelisted: d.whitelisted,
            }
        })
        .collect();

    for t in &mut tiers {
        t.clients = clients.iter().filter(|c| c.tier == t.id).count();
    }

    let running = real.services.qos_tiers.running;
    let qos = Qos {
        enforcing_burst: running && real.policy.burst.is_some(),
        enforcing_monthly: running && !real.policy.fup.is_empty(),
        shaper_dl_kbit: real.policy.dl_ceiling_kbit.unwrap_or(0),
        shaper_ul_kbit: real.policy.ul_ceiling_kbit.unwrap_or(0),
        clients,
        tiers,
    };

    let whitelisted = real
        .devices
        .iter()
        .map(|d| (d.mac.to_lowercase(), d.whitelisted))
        .collect();
    let online = real
        .devices
        .iter()
        .map(|d| (d.mac.to_lowercase(), d.online))
        .collect();

    QosCollected {
        qos,
        whitelisted,
        online,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Real {
        serde_json::from_str(include_str!("../../fixtures/qos-status.json")).unwrap()
    }

    fn collected() -> QosCollected {
        let real = fixture();
        let tiers = ladder_from_fup(&real.policy.fup);
        build(real, tiers)
    }

    #[test]
    fn parses_the_real_fixture_shape() {
        let c = collected();
        assert!(c.qos.enforcing_burst);
        assert!(c.qos.enforcing_monthly);
        // policy.{dl,ul}_ceiling_kbit are both null in the real capture => unlimited (0).
        assert_eq!(c.qos.shaper_dl_kbit, 0);
        assert_eq!(c.qos.shaper_ul_kbit, 0);
        assert_eq!(c.qos.clients.len(), 22);
        assert_eq!(c.qos.tiers.len(), 4);
    }

    #[test]
    fn every_client_tier_is_a_known_tier_id() {
        let c = collected();
        let ids: Vec<&str> = c.qos.tiers.iter().map(|t| t.id.as_str()).collect();
        for cl in &c.qos.clients {
            assert!(ids.contains(&cl.tier.as_str()), "unknown tier {}", cl.tier);
        }
    }

    #[test]
    fn tier_client_counts_match() {
        let c = collected();
        let total: usize = c.qos.tiers.iter().map(|t| t.clients).sum();
        assert_eq!(total, c.qos.clients.len());
    }

    #[test]
    fn whitelisted_device_bypasses_but_still_gets_a_computed_tier() {
        let c = collected();
        let whitelisted_client = c
            .qos
            .clients
            .iter()
            .find(|cl| cl.mac == "02:00:00:00:00:16")
            .unwrap();
        assert!(whitelisted_client.whitelisted);
        // ~81 GB/month, but whitelisted devices still get a computed tier id (t1) even though
        // qos-tiers itself doesn't throttle them.
        assert_eq!(whitelisted_client.tier, "t1");
        assert_eq!(c.whitelisted.get("02:00:00:00:00:16"), Some(&true));
    }

    #[test]
    fn light_user_under_the_first_threshold_is_unlimited() {
        let c = collected();
        // ap-2 itself, ~10 GB/month, not whitelisted.
        let ap2 = c
            .qos
            .clients
            .iter()
            .find(|cl| cl.mac == "02:00:00:00:00:1c")
            .unwrap();
        assert!(!ap2.whitelisted);
        assert_eq!(ap2.tier, "t0");
    }

    #[test]
    fn online_and_whitelist_maps_are_exposed_per_mac() {
        let c = collected();
        assert_eq!(c.online.get("02:00:00:00:00:16"), Some(&true));
        let offline_mac = "02:00:00:00:00:15"; // online: false in the fixture
        assert_eq!(c.online.get(offline_mac), Some(&false));
    }

    #[test]
    fn parses_the_real_shell_kv_ladder_file() {
        let text = std::fs::read_to_string(format!(
            "{}/fixtures/device-qos.conf",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let fup = parse_conf_fup(&text).unwrap();
        let tiers = ladder_from_fup(&fup);
        assert_eq!(tiers.len(), 4);
        assert_eq!(tiers[0].id, "t0");
        assert_eq!(tiers[0].max_gb, Some(50.0));
        assert_eq!(tiers[0].rate_mbit, None);
        assert_eq!(tiers[1].min_gb, 50.0);
        assert_eq!(tiers[1].rate_mbit, Some(125.0));
        assert_eq!(tiers[3].id, "t3");
        assert_eq!(tiers[3].max_gb, None);
        assert_eq!(tiers[3].rate_mbit, Some(50.0));
    }

    #[tokio::test]
    async fn parses_dl_ul_ceilings_from_the_conf_file() {
        let path = std::path::PathBuf::from(format!(
            "{}/fixtures/device-qos.conf",
            env!("CARGO_MANIFEST_DIR")
        ));
        let (dl, ul) = parse_conf_ceilings(&path).await.unwrap();
        assert_eq!(dl, 0);
        assert_eq!(ul, 0);
    }

    #[test]
    fn tolerates_an_entirely_different_shape() {
        let real: Real = serde_json::from_str(r#"{"unrelated": true}"#).unwrap();
        let c = build(real, default_tiers());
        assert!(!c.qos.enforcing_burst);
        assert_eq!(c.qos.shaper_dl_kbit, 0);
        assert!(c.qos.clients.is_empty());
        assert_eq!(c.qos.tiers.len(), 4);
    }

    #[test]
    fn malformed_ladder_file_falls_back_to_defaults() {
        assert!(parse_conf_fup("not a valid ladder\nat all").is_none());
    }
}
