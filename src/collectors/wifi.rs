use std::collections::HashMap;

use serde_json::json;

use crate::{
    model::{Band, Client},
    ubus::Ubus,
};

/// Associated stations from every `hostapd.<iface>` object on the node, keyed by iface for the
/// radios collector to fold into `Radio.clients`. Mesh peers (802.11s) are not in hostapd — see
/// `collectors::links`.
pub async fn collect(ubus: &Ubus, node: &str) -> (Vec<Client>, HashMap<String, usize>) {
    let ifaces = ubus.list("hostapd.*").await.unwrap_or_default();
    let mut stations = vec![];
    let mut counts = HashMap::new();
    for iface in ifaces {
        let Ok(v) = ubus.call(&iface, "get_clients", json!({})).await else {
            continue;
        };
        let clients = v["clients"].as_object().cloned().unwrap_or_default();
        let status = ubus.call(&iface, "get_status", json!({})).await.ok();
        let ssid = status
            .as_ref()
            .and_then(|s| s["ssid"].as_str())
            .map(str::to_owned);
        let freq = status.as_ref().and_then(|s| s["freq"].as_u64());
        let band = freq.map(|f| if f < 3000 { Band::Ghz24 } else { Band::Ghz5 });
        let iface_name = iface.trim_start_matches("hostapd.").to_owned();
        counts.insert(iface_name.clone(), clients.len());
        for (mac, c) in clients {
            stations.push(Client {
                mac: mac.to_lowercase(),
                node: Some(node.to_owned()),
                online: true,
                wired: false,
                ssid: ssid.clone(),
                band,
                signal_dbm: c["signal"].as_i64().map(|s| s as i32),
                rx_rate: c["rate"]["rx"].as_u64(),
                tx_rate: c["rate"]["tx"].as_u64(),
                ..Default::default()
            });
        }
    }
    (stations, counts)
}
