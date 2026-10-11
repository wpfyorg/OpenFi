use serde_json::{Value, json};

use crate::{
    config::NodeConfig,
    model::{Node, Role, Temp},
    ubus::Ubus,
};

/// `system board` + `system info` — available on every OpenWrt node.
pub async fn collect(nc: &NodeConfig, ubus: &Ubus, role: Role) -> Node {
    let mut node = Node {
        name: nc.name.clone(),
        host: nc.host.clone(),
        role,
        ..Default::default()
    };
    let info = match ubus.call("system", "info", json!({})).await {
        Ok(v) => v,
        Err(e) => {
            node.error = Some(e);
            return node;
        }
    };
    node.online = true;
    node.uptime = info["uptime"].as_u64().unwrap_or(0);
    if let Some(l) = info["load"].as_array() {
        // Kernel load averages are fixed-point * 65536.
        for (i, v) in l.iter().take(3).enumerate() {
            node.load[i] = v.as_f64().unwrap_or(0.0) as f32 / 65536.0;
        }
    }
    let mem = &info["memory"];
    node.mem_total = mem["total"].as_u64().unwrap_or(0);
    node.mem_available = mem["available"].as_u64().unwrap_or_else(|| {
        mem["free"].as_u64().unwrap_or(0)
            + mem["buffered"].as_u64().unwrap_or(0)
            + mem["cached"].as_u64().unwrap_or(0)
    });
    // Verified on mainrouter (2026-09-24): `system info`'s `root` object is
    // `{"total","free","used","avail"}` in **KiB** (`ubus call system info` →
    // `"root":{"total":103216,"free":78848,"used":24368,"avail":74012}`), not bytes — the
    // contract's `overlay_used`/`overlay_total` are bytes, so multiply by 1024 here.
    node.overlay_total = info["root"]["total"].as_u64().map(|kib| kib * 1024);
    node.overlay_used = info["root"]["used"].as_u64().map(|kib| kib * 1024);

    if let Ok(board) = ubus.call("system", "board", json!({})).await {
        node.model = board["model"].as_str().map(str::to_owned);
        node.firmware = board["release"]["description"].as_str().map(str::to_owned);
    }
    // luci-app-temp-status is on every node; it wraps /sys/class/hwmon with trip points.
    if let Ok(v) = ubus.call("luci.temp-status", "getSensors", json!({})).await {
        node.temps = temps(&v);
    }
    node
}

/// Flatten `getSensors` ({"sensors":{"0":[{title, sources:[{temp, tpoints}]}]}}), millidegrees → °C.
fn temps(v: &Value) -> Vec<Temp> {
    let mut out = vec![];
    for group in v["sensors"]
        .as_object()
        .into_iter()
        .flat_map(|o| o.values())
    {
        for sensor in group.as_array().into_iter().flatten() {
            let title = sensor["title"].as_str().unwrap_or("sensor");
            for src in sensor["sources"].as_array().into_iter().flatten() {
                let Some(t) = src["temp"].as_f64() else {
                    continue;
                };
                let crit = src["tpoints"]
                    .as_object()
                    .into_iter()
                    .flat_map(|o| o.values())
                    .filter(|p| matches!(p["type"].as_str(), Some("hot" | "critical")))
                    .filter_map(|p| p["temp"].as_f64())
                    .reduce(f64::min);
                out.push(Temp {
                    label: label(title),
                    celsius: (t / 1000.0) as f32,
                    crit: crit.map(|c| (c / 1000.0) as f32),
                });
            }
        }
    }
    out
}

fn label(title: &str) -> String {
    match title {
        "cpu_thermal" | "cpu-thermal" => "CPU".into(),
        // MT7915 on this board: phy0 = 2.4 GHz, phy1 = 5 GHz.
        "mt7915_phy0" => "WiFi 2.4 GHz".into(),
        "mt7915_phy1" => "WiFi 5 GHz".into(),
        t => t.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_get_sensors() {
        let v: Value = serde_json::from_str(include_str!(
            "../../fixtures/mainrouter/luci.temp-status.getSensors.json"
        ))
        .unwrap();
        let t = temps(&v);
        assert_eq!(
            t.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(),
            ["CPU", "WiFi 2.4 GHz", "WiFi 5 GHz"]
        );
        assert_eq!(t[0].crit, Some(120.0));
        assert!((t[2].celsius - 73.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn overlay_usage_is_converted_from_kib_to_bytes() {
        let nc = NodeConfig {
            name: "mainrouter".into(),
            host: "127.0.0.1".into(),
            transport: "local".into(),
            username: None,
            password: None,
            wired_uplink: None,
            fixtures: Some(std::path::PathBuf::from(format!(
                "{}/fixtures/mainrouter",
                env!("CARGO_MANIFEST_DIR")
            ))),
        };
        let ubus = Ubus::fixture(nc.fixtures.clone().unwrap());
        let node = collect(&nc, &ubus, Role::Gateway).await;
        // fixture root block (real shape): total 103216 KiB, used 24368 KiB.
        assert_eq!(node.overlay_total, Some(103216 * 1024));
        assert_eq!(node.overlay_used, Some(24368 * 1024));
    }
}
