//! Mock-mode UI feedback: `docs/api.md`'s "Mock mode" note says writes "mutate in-memory mock
//! state where it is cheap ... so the UI visibly reacts." Fixture `Ubus` calls just read static
//! JSON off disk, so a whitelist toggle or radio change wouldn't otherwise show up in the next
//! snapshot — this holds the small set of overrides `collectors::mod` folds in every poll.
//! A no-op (and practically free) when not in mock mode.

use std::collections::HashMap;

use tokio::sync::RwLock;

use crate::model::{Client, Node};

#[derive(Clone, Copy, Default)]
struct RadioOverride {
    disabled: Option<bool>,
    channel: Option<u32>,
    txpower_dbm: Option<i32>,
}

pub struct MockState {
    enabled: bool,
    whitelist: RwLock<HashMap<String, bool>>,
    radios: RwLock<HashMap<(String, String), RadioOverride>>,
}

impl MockState {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            whitelist: RwLock::new(HashMap::new()),
            radios: RwLock::new(HashMap::new()),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub async fn set_whitelist(&self, mac: &str, on: bool) {
        if self.enabled {
            self.whitelist.write().await.insert(mac.to_owned(), on);
        }
    }

    pub async fn set_radio(
        &self,
        node: &str,
        radio: &str,
        disabled: Option<bool>,
        channel: Option<u32>,
        txpower_dbm: Option<i32>,
    ) {
        if !self.enabled {
            return;
        }
        let mut radios = self.radios.write().await;
        let entry = radios
            .entry((node.to_owned(), radio.to_owned()))
            .or_default();
        if let Some(d) = disabled {
            entry.disabled = Some(d);
        }
        if let Some(c) = channel {
            entry.channel = Some(c);
        }
        if let Some(t) = txpower_dbm {
            entry.txpower_dbm = Some(t);
        }
    }

    pub async fn apply(&self, nodes: &mut [Node], clients: &mut [Client]) {
        if !self.enabled {
            return;
        }
        let whitelist = self.whitelist.read().await;
        for c in clients.iter_mut() {
            if let Some(&w) = whitelist.get(&c.mac) {
                c.whitelisted = w;
            }
        }
        let radios = self.radios.read().await;
        for n in nodes.iter_mut() {
            for r in &mut n.radios {
                let Some(ov) = radios.get(&(n.name.clone(), r.name.clone())) else {
                    continue;
                };
                if let Some(d) = ov.disabled {
                    r.disabled = d;
                }
                if r.disabled {
                    r.iface = None;
                    r.ssids.clear();
                    r.channel = None;
                    r.htmode = None;
                    r.txpower_dbm = None;
                    r.clients = 0;
                    continue;
                }
                if let Some(c) = ov.channel {
                    r.channel = Some(c);
                }
                if let Some(t) = ov.txpower_dbm {
                    r.txpower_dbm = Some(t);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Band, Radio};

    fn node_with_radio() -> Node {
        Node {
            name: "ap-2".into(),
            radios: vec![Radio {
                name: "radio1".into(),
                iface: Some("phy1-ap0".into()),
                band: Band::Ghz5,
                disabled: false,
                ssids: vec!["Example-Primary".into()],
                channel: Some(36),
                htmode: Some("HE80".into()),
                txpower_dbm: Some(20),
                noise_dbm: None,
                clients: 3,
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn disabled_when_not_in_mock_mode() {
        let mock = MockState::new(false);
        mock.set_whitelist("aa:bb", true).await;
        let mut clients = vec![Client {
            mac: "aa:bb".into(),
            ..Default::default()
        }];
        let mut nodes = vec![];
        mock.apply(&mut nodes, &mut clients).await;
        assert!(!clients[0].whitelisted);
    }

    #[tokio::test]
    async fn whitelist_override_reflected_in_next_apply() {
        let mock = MockState::new(true);
        mock.set_whitelist("aa:bb", true).await;
        let mut clients = vec![Client {
            mac: "aa:bb".into(),
            whitelisted: false,
            ..Default::default()
        }];
        let mut nodes = vec![];
        mock.apply(&mut nodes, &mut clients).await;
        assert!(clients[0].whitelisted);
    }

    #[tokio::test]
    async fn disabling_a_radio_clears_its_live_fields() {
        let mock = MockState::new(true);
        mock.set_radio("ap-2", "radio1", Some(true), None, None)
            .await;
        let mut nodes = vec![node_with_radio()];
        let mut clients = vec![];
        mock.apply(&mut nodes, &mut clients).await;
        let r = &nodes[0].radios[0];
        assert!(r.disabled);
        assert!(r.iface.is_none());
        assert!(r.ssids.is_empty());
        assert_eq!(r.channel, None);
    }

    #[tokio::test]
    async fn channel_and_txpower_overrides_apply_when_enabled() {
        let mock = MockState::new(true);
        mock.set_radio("ap-2", "radio1", None, Some(149), Some(17))
            .await;
        let mut nodes = vec![node_with_radio()];
        let mut clients = vec![];
        mock.apply(&mut nodes, &mut clients).await;
        let r = &nodes[0].radios[0];
        assert!(!r.disabled);
        assert_eq!(r.channel, Some(149));
        assert_eq!(r.txpower_dbm, Some(17));
    }
}
