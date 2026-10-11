//! Per-radio metadata: `uci get wireless` for the static config (band, channel, htmode,
//! txpower, disabled, SSIDs) plus `iwinfo info` on the radio's AP iface for the live values
//! (actual operating channel/txpower can differ from the uci config, and uci has no `noise`
//! option at all). Client counts are filled in by `mod.rs` once station lists are known.

use serde_json::json;

use crate::{
    collectors::uci,
    model::{Band, Radio},
    ubus::Ubus,
};

pub async fn collect(ubus: &Ubus) -> Vec<Radio> {
    let sections = uci::get(ubus, "wireless").await;
    let devices = sections.iter().filter(|s| s.kind == "wifi-device");
    let mut radios = vec![];
    for dev in devices {
        let band = match dev.get("band") {
            Some("5g") => Band::Ghz5,
            _ => Band::Ghz24, // uci default when unset is "2g" on every board we target
        };
        let disabled = dev.flag("disabled");

        // AP-mode ifaces on this radio (mesh ifaces don't have an SSID or show up in hostapd).
        let ap_ifaces: Vec<_> = sections
            .iter()
            .filter(|s| s.kind == "wifi-iface" && s.get("device") == Some(&dev.name))
            .filter(|s| s.get("mode") == Some("ap"))
            .collect();
        let ssids: Vec<String> = ap_ifaces
            .iter()
            .filter_map(|s| s.get("ssid").map(str::to_owned))
            .collect();
        // The primary iface (first in file order) stands in for the radio's live iwinfo values;
        // every AP-mode VAP on a radio shares the same channel/txpower/noise.
        let iface = ap_ifaces
            .first()
            .and_then(|s| s.get("ifname"))
            .map(str::to_owned);

        let mut channel = dev.parse::<u32>("channel");
        let mut htmode = dev.get("htmode").map(str::to_owned);
        let mut txpower_dbm = dev.parse::<i32>("txpower");
        let mut noise_dbm = None;
        if !disabled
            && let Some(ifname) = &iface
            && let Ok(info) = ubus
                .call("iwinfo", "info", json!({ "device": ifname }))
                .await
        {
            channel = info["channel"].as_u64().map(|c| c as u32).or(channel);
            htmode = info["htmode"].as_str().map(str::to_owned).or(htmode);
            txpower_dbm = info["txpower"].as_i64().map(|t| t as i32).or(txpower_dbm);
            noise_dbm = info["noise"].as_i64().map(|n| n as i32);
        }

        radios.push(Radio {
            name: dev.name.clone(),
            iface: if disabled { None } else { iface },
            band,
            disabled,
            ssids: if disabled { vec![] } else { ssids },
            channel: if disabled { None } else { channel },
            htmode: if disabled { None } else { htmode },
            txpower_dbm: if disabled { None } else { txpower_dbm },
            noise_dbm,
            clients: 0,
        });
    }
    radios
}
