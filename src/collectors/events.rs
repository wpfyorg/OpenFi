//! Derives the events feed from diffs between consecutive polls (client joined/left/roamed,
//! node up/down, WAN up/down). "action" events come from `src/audit.rs` write handlers later;
//! nothing here produces that kind yet.
//!
//! usteer's own `get_clients`/`remote_hosts` would give a more authoritative roam signal (it
//! knows *why* a client moved — RSSI, load, or a forced kick), but the client-node/band diff
//! below already captures the same moves and needs no extra polling, so usteer is left for a
//! later pass if the plain diff proves too noisy.

use std::collections::{HashMap, VecDeque};

use crate::model::{Band, Client, Event, EventKind, Node};

const MAX_EVENTS: usize = 50;

#[derive(Clone)]
struct ClientState {
    online: bool,
    wired: bool,
    node: Option<String>,
    band: Option<Band>,
}

#[derive(Default)]
pub struct Tracker {
    events: VecDeque<Event>,
    clients: HashMap<String, ClientState>,
    nodes: HashMap<String, bool>,
    wan_up: Option<bool>,
    /// Suppresses events on the very first poll, where "state changed" just means "we now know
    /// the state" rather than anything actually happening.
    primed: bool,
}

impl Tracker {
    pub fn update(
        &mut self,
        ts: u64,
        nodes: &[Node],
        clients: &[Client],
        wan_up: bool,
    ) -> Vec<Event> {
        let push = |events: &mut VecDeque<Event>,
                    kind: EventKind,
                    text: String,
                    node: Option<String>,
                    mac: Option<String>| {
            events.push_front(Event {
                ts,
                kind,
                text,
                node,
                mac,
            });
            events.truncate(MAX_EVENTS);
        };

        if self.primed {
            for n in nodes {
                match self.nodes.get(&n.name) {
                    Some(true) if !n.online => {
                        push(
                            &mut self.events,
                            EventKind::NodeDown,
                            format!("{} went offline", n.name),
                            Some(n.name.clone()),
                            None,
                        );
                    }
                    Some(false) if n.online => {
                        push(
                            &mut self.events,
                            EventKind::NodeUp,
                            format!("{} back online", n.name),
                            Some(n.name.clone()),
                            None,
                        );
                    }
                    _ => {}
                }
            }
            match self.wan_up {
                Some(true) if !wan_up => push(
                    &mut self.events,
                    EventKind::WanDown,
                    "WAN link down".into(),
                    None,
                    None,
                ),
                Some(false) if wan_up => push(
                    &mut self.events,
                    EventKind::WanUp,
                    "WAN link restored".into(),
                    None,
                    None,
                ),
                _ => {}
            }
            for c in clients {
                let name = c.hostname.clone().unwrap_or_else(|| c.mac.clone());
                let Some(prev) = self.clients.get(&c.mac) else {
                    if c.online {
                        push(
                            &mut self.events,
                            EventKind::Joined,
                            format!("{name} joined"),
                            c.node.clone(),
                            Some(c.mac.clone()),
                        );
                    }
                    continue;
                };
                if prev.online && !c.online {
                    push(
                        &mut self.events,
                        EventKind::Left,
                        format!("{name} left the network"),
                        prev.node.clone(),
                        Some(c.mac.clone()),
                    );
                } else if !prev.online && c.online {
                    push(
                        &mut self.events,
                        EventKind::Joined,
                        format!("{name} joined"),
                        c.node.clone(),
                        Some(c.mac.clone()),
                    );
                } else if prev.online
                    && c.online
                    && !c.wired
                    && !prev.wired
                    && prev.node.is_some()
                    && c.node.is_some()
                    && prev.node != c.node
                {
                    push(
                        &mut self.events,
                        EventKind::Roam,
                        format!(
                            "{name} moved from {} to {}",
                            prev.node.as_deref().unwrap_or("?"),
                            c.node.as_deref().unwrap_or("?")
                        ),
                        c.node.clone(),
                        Some(c.mac.clone()),
                    );
                } else if prev.online
                    && c.online
                    && !c.wired
                    && prev.node == c.node
                    && prev.band.is_some()
                    && c.band.is_some()
                    && prev.band != c.band
                {
                    push(
                        &mut self.events,
                        EventKind::BandSteer,
                        format!(
                            "{name} switched to {} on {}",
                            band_label(c.band),
                            c.node.as_deref().unwrap_or("?")
                        ),
                        c.node.clone(),
                        Some(c.mac.clone()),
                    );
                }
            }
        }

        self.nodes = nodes.iter().map(|n| (n.name.clone(), n.online)).collect();
        self.wan_up = Some(wan_up);
        self.clients = clients
            .iter()
            .map(|c| {
                (
                    c.mac.clone(),
                    ClientState {
                        online: c.online,
                        wired: c.wired,
                        node: c.node.clone(),
                        band: c.band,
                    },
                )
            })
            .collect();
        self.primed = true;

        self.events.iter().cloned().collect()
    }
}

fn band_label(b: Option<Band>) -> &'static str {
    match b {
        Some(Band::Ghz24) => "2.4 GHz",
        Some(Band::Ghz5) => "5 GHz",
        None => "WiFi",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(mac: &str, node: &str, online: bool) -> Client {
        Client {
            mac: mac.into(),
            node: Some(node.into()),
            online,
            hostname: Some("Phone".into()),
            ..Default::default()
        }
    }

    #[test]
    fn first_poll_is_silent() {
        let mut t = Tracker::default();
        let events = t.update(1000, &[], &[client("aa:bb", "ap-2", true)], true);
        assert!(events.is_empty());
    }

    #[test]
    fn detects_join_and_leave() {
        let mut t = Tracker::default();
        t.update(1000, &[], &[client("aa:bb", "ap-2", true)], true);

        let events = t.update(1010, &[], &[client("aa:bb", "ap-2", false)], true);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Left);

        let events = t.update(1020, &[], &[client("aa:bb", "ap-2", true)], true);
        assert_eq!(events[0].kind, EventKind::Joined);
        // Newest first, and the earlier "left" is still there.
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].kind, EventKind::Left);
    }

    #[test]
    fn detects_roam_between_nodes() {
        let mut t = Tracker::default();
        t.update(1000, &[], &[client("aa:bb", "ap-2", true)], true);
        let events = t.update(1010, &[], &[client("aa:bb", "ap-4", true)], true);
        assert_eq!(events[0].kind, EventKind::Roam);
        assert!(events[0].text.contains("ap-2"));
        assert!(events[0].text.contains("ap-4"));
    }

    #[test]
    fn detects_wan_down() {
        let mut t = Tracker::default();
        t.update(1000, &[], &[], true);
        let events = t.update(1010, &[], &[], false);
        assert_eq!(events[0].kind, EventKind::WanDown);
    }

    #[test]
    fn caps_at_50_events() {
        let mut t = Tracker::default();
        t.update(0, &[], &[client("aa:bb", "ap-2", true)], true);
        for i in 0..60 {
            let online = i % 2 == 0;
            t.update(1000 + i, &[], &[client("aa:bb", "ap-2", online)], true);
        }
        let events = t.update(2000, &[], &[client("aa:bb", "ap-2", true)], true);
        assert!(events.len() <= 50);
    }
}
