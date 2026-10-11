use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    model::{RatePoint, Wan},
    ubus::Ubus,
};

/// 30-minute buckets → 48 points of 24h history.
const BUCKET_24H_SECS: u64 = 1800;
const LEN_24H: usize = 48;
/// 3-hour buckets → 56 points of 7-day history.
const BUCKET_7D_SECS: u64 = 3 * 3600;
const LEN_7D: usize = 56;

/// A fixed-size, bucketed ring of rate samples. Two of these (24h/7d) share the same input
/// samples but average them over different bucket widths.
#[derive(Default)]
struct Ring {
    bucket: Option<(u64, u64, u64, u32)>, // start, rx_sum, tx_sum, samples
    history: VecDeque<RatePoint>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct PersistedBucket {
    start: u64,
    rx_sum: u64,
    tx_sum: u64,
    samples: u32,
}

impl Ring {
    fn add(&mut self, bucket_secs: u64, len: usize, ts: u64, rx: u64, tx: u64) {
        let start = ts - ts % bucket_secs;
        // Older versions persisted the in-progress bucket as an average with no sample count.
        // If it matches this sample's bucket, discard it: its contribution cannot be weighted
        // correctly. Explicit active buckets restored from sums/count are kept above this path.
        if self.bucket.is_none() && self.history.back().is_some_and(|p| p.ts == start) {
            self.history.retain(|point| point.ts != start);
        }
        match &mut self.bucket {
            Some(b) if b.0 == start => {
                b.1 += rx;
                b.2 += tx;
                b.3 += 1;
            }
            _ => {
                if let Some((s, r, t, n)) = self.bucket.take() {
                    self.history.push_back(RatePoint {
                        ts: s,
                        rx_bps: r / n as u64,
                        tx_bps: t / n as u64,
                    });
                    while self.history.len() > len {
                        self.history.pop_front();
                    }
                }
                self.bucket = Some((start, rx, tx, 1));
            }
        }
    }

    fn points(&self) -> Vec<RatePoint> {
        let mut out: Vec<RatePoint> = self.history.iter().copied().collect();
        if let Some((s, r, t, n)) = self.bucket {
            out.push(RatePoint {
                ts: s,
                rx_bps: r / n as u64,
                tx_bps: t / n as u64,
            });
        }
        out
    }

    /// Bytes implied by each bucket's average rate times the time it covers: full width for
    /// completed buckets, only the elapsed part (up to `now`) for the in-progress one.
    fn window_bytes(&self, bucket_secs: u64, now: u64) -> (u64, u64) {
        self.points().iter().fold((0u64, 0u64), |(rx, tx), p| {
            let secs = now.saturating_sub(p.ts).min(bucket_secs);
            (rx + p.rx_bps * secs / 8, tx + p.tx_bps * secs / 8)
        })
    }

    fn load(points: Vec<RatePoint>, len: usize, active: Option<PersistedBucket>) -> Self {
        let mut by_timestamp = BTreeMap::new();
        for point in points {
            by_timestamp.insert(point.ts, point);
        }
        let mut history: VecDeque<RatePoint> = by_timestamp.into_values().collect();
        while history.len() > len {
            history.pop_front();
        }
        Self {
            bucket: active.map(|b| (b.start, b.rx_sum, b.tx_sum, b.samples.max(1))),
            history,
        }
    }

    fn completed_points(&self) -> Vec<RatePoint> {
        self.history.iter().copied().collect()
    }

    fn persisted_bucket(&self) -> Option<PersistedBucket> {
        self.bucket
            .map(|(start, rx_sum, tx_sum, samples)| PersistedBucket {
                start,
                rx_sum,
                tx_sum,
                samples,
            })
    }
}

#[derive(Default)]
pub struct Tracker {
    last: Option<(u64, u64, u64)>, // ts, rx, tx
    ring_24h: Ring,
    ring_7d: Ring,
    last_rate: (u64, u64),
}

/// What's persisted to `history_file` across restarts. Older versions stored the active bucket
/// as an average point in the history vectors; new versions store its sums and weight separately.
#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    #[serde(rename = "24h")]
    h24: Vec<RatePoint>,
    #[serde(rename = "7d")]
    d7: Vec<RatePoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_24h: Option<PersistedBucket>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_7d: Option<PersistedBucket>,
}

impl Tracker {
    pub async fn load(path: &Path) -> Self {
        let Ok(bytes) = tokio::fs::read(path).await else {
            return Self::default();
        };
        let Ok(p) = serde_json::from_slice::<Persisted>(&bytes) else {
            return Self::default();
        };
        Self {
            ring_24h: Ring::load(p.h24, LEN_24H, p.active_24h),
            ring_7d: Ring::load(p.d7, LEN_7D, p.active_7d),
            ..Default::default()
        }
    }

    pub async fn save(&self, path: &Path) {
        let p = Persisted {
            h24: self.ring_24h.completed_points(),
            d7: self.ring_7d.completed_points(),
            active_24h: self.ring_24h.persisted_bucket(),
            active_7d: self.ring_7d.persisted_bucket(),
        };
        if let Ok(bytes) = serde_json::to_vec(&p) {
            let _ = tokio::fs::write(path, bytes).await;
        }
    }

    pub async fn sample(&mut self, ubus: &Ubus, device: &str, ts: u64) -> Wan {
        let stats = ubus
            .call("network.device", "status", json!({ "name": device }))
            .await
            .ok()
            .map(|v| v["statistics"].clone());
        let (rx, tx) = match &stats {
            Some(s) => (
                s["rx_bytes"].as_u64().unwrap_or(0),
                s["tx_bytes"].as_u64().unwrap_or(0),
            ),
            None => return self.wan(0, 0),
        };
        if let Some((lts, lrx, ltx)) = self.last {
            let dt = ts.saturating_sub(lts).max(1);
            // Counter reset (reboot / iface restart) → skip this interval.
            if rx >= lrx && tx >= ltx {
                self.last_rate = ((rx - lrx) * 8 / dt, (tx - ltx) * 8 / dt);
                self.ring_24h.add(
                    BUCKET_24H_SECS,
                    LEN_24H,
                    ts,
                    self.last_rate.0,
                    self.last_rate.1,
                );
                self.ring_7d.add(
                    BUCKET_7D_SECS,
                    LEN_7D,
                    ts,
                    self.last_rate.0,
                    self.last_rate.1,
                );
            }
        }
        self.last = Some((ts, rx, tx));
        self.wan(rx, tx)
    }

    /// `ip`/`gateway` come from a separate object (`network.interface.wan`); the caller
    /// (`collectors::run`) fetches those and merges them into the returned `Wan`.
    fn wan(&self, rx_bytes: u64, tx_bytes: u64) -> Wan {
        // "Now" is the latest sample's timestamp; without one the in-progress bucket counts as empty.
        let now = self.last.map(|(ts, _, _)| ts).unwrap_or(0);
        let (window_rx_bytes, window_tx_bytes) = self.ring_24h.window_bytes(BUCKET_24H_SECS, now);
        Wan {
            ip: None,
            gateway: None,
            rx_bps: self.last_rate.0,
            tx_bps: self.last_rate.1,
            rx_bytes,
            tx_bytes,
            window_rx_bytes,
            window_tx_bytes,
            history: self.ring_24h.points(),
        }
    }

    pub fn history(&self, range_7d: bool) -> Vec<RatePoint> {
        if range_7d {
            self.ring_7d.points()
        } else {
            self.ring_24h.points()
        }
    }
}

/// `network.interface.wan` — a distinct ubus object (not `network.interface` + an arg) on
/// stock OpenWrt. ip = first configured IPv4 address; gateway = the default route's nexthop.
pub async fn wan_ip_gateway(ubus: &Ubus) -> (Option<String>, Option<String>, bool) {
    let Ok(v) = ubus
        .call("network.interface.wan", "status", json!({}))
        .await
    else {
        return (None, None, false);
    };
    let up = v["up"].as_bool().unwrap_or(false);
    let ip = v["ipv4-address"][0]["address"].as_str().map(str::to_owned);
    let gateway = v["route"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|r| r["target"].as_str() == Some("0.0.0.0"))
        .and_then(|r| r["nexthop"].as_str())
        .map(str::to_owned);
    (ip, gateway, up)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_bytes_counts_only_elapsed_time_of_the_open_bucket() {
        let mut ring = Ring::default();
        // Two full 30-min buckets at 1,000,000 bps rx / 200,000 bps tx, then an in-progress
        // bucket that is only 600 s old.
        ring.add(BUCKET_24H_SECS, LEN_24H, 0, 1_000_000, 200_000);
        ring.add(
            BUCKET_24H_SECS,
            LEN_24H,
            BUCKET_24H_SECS,
            1_000_000,
            200_000,
        );
        ring.add(
            BUCKET_24H_SECS,
            LEN_24H,
            BUCKET_24H_SECS * 2,
            1_000_000,
            200_000,
        );
        let (rx, tx) = ring.window_bytes(BUCKET_24H_SECS, BUCKET_24H_SECS * 2 + 600);
        assert_eq!(rx, 1_000_000 * (BUCKET_24H_SECS * 2 + 600) / 8);
        assert_eq!(tx, 200_000 * (BUCKET_24H_SECS * 2 + 600) / 8);
    }

    #[test]
    fn wan_merges_window_bytes_from_the_ring() {
        let mut t = Tracker::default();
        t.ring_24h
            .add(BUCKET_24H_SECS, LEN_24H, 0, 1_000_000, 200_000);
        t.last = Some((BUCKET_24H_SECS, 0, 0)); // the bucket has fully elapsed
        let wan = t.wan(0, 0);
        assert_eq!(wan.window_rx_bytes, 1_000_000 * BUCKET_24H_SECS / 8);
        assert_eq!(wan.window_tx_bytes, 200_000 * BUCKET_24H_SECS / 8);
    }

    #[tokio::test]
    async fn repeated_same_bucket_restarts_do_not_duplicate_history_timestamps() {
        let dir = std::env::temp_dir().join(format!("owfi-traffic-restart-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("history.json");

        let mut tracker = Tracker::default();
        tracker.ring_24h.add(BUCKET_24H_SECS, LEN_24H, 1000, 80, 40);
        tracker.ring_7d.add(BUCKET_7D_SECS, LEN_7D, 1000, 80, 40);

        for _ in 0..10 {
            tracker.save(&path).await;
            tracker = Tracker::load(&path).await;
            tracker.ring_24h.add(BUCKET_24H_SECS, LEN_24H, 1001, 80, 40);
            tracker.ring_7d.add(BUCKET_7D_SECS, LEN_7D, 1001, 80, 40);
        }

        let h24 = tracker.history(false);
        let d7 = tracker.history(true);
        assert_eq!(h24.iter().filter(|p| p.ts == 0).count(), 1);
        assert_eq!(d7.iter().filter(|p| p.ts == 0).count(), 1);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn resumed_active_bucket_preserves_sample_weighting() {
        let dir = std::env::temp_dir().join(format!("owfi-traffic-weight-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("history.json");

        let mut tracker = Tracker::default();
        for _ in 0..100 {
            tracker
                .ring_24h
                .add(BUCKET_24H_SECS, LEN_24H, 1000, 1000, 500);
            tracker.ring_7d.add(BUCKET_7D_SECS, LEN_7D, 1000, 1000, 500);
        }
        tracker.save(&path).await;

        let mut restored = Tracker::load(&path).await;
        restored
            .ring_24h
            .add(BUCKET_24H_SECS, LEN_24H, 1001, 101_000, 50_500);
        restored
            .ring_7d
            .add(BUCKET_7D_SECS, LEN_7D, 1001, 101_000, 50_500);

        let h24 = restored.history(false);
        let d7 = restored.history(true);
        let expected = ((100 * 1000 + 101_000) / 101, (100 * 500 + 50_500) / 101);
        assert_eq!((h24[0].rx_bps, h24[0].tx_bps), expected);
        assert_eq!((d7[0].rx_bps, d7[0].tx_bps), expected);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn legacy_active_bucket_discards_unweighted_average_on_first_sample() {
        let dir = std::env::temp_dir().join(format!(
            "owfi-traffic-legacy-active-weight-{}",
            std::process::id()
        ));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("history.json");
        let legacy = Persisted {
            h24: vec![RatePoint {
                ts: 0,
                rx_bps: 1_000,
                tx_bps: 2_000,
            }],
            d7: vec![RatePoint {
                ts: 0,
                rx_bps: 1_000,
                tx_bps: 2_000,
            }],
            ..Default::default()
        };
        tokio::fs::write(&path, serde_json::to_vec(&legacy).unwrap())
            .await
            .unwrap();

        let mut tracker = Tracker::load(&path).await;
        tracker
            .ring_24h
            .add(BUCKET_24H_SECS, LEN_24H, 1_001, 101_000, 202_000);
        tracker
            .ring_7d
            .add(BUCKET_7D_SECS, LEN_7D, 1_001, 101_000, 202_000);

        for (name, points) in [
            ("24h", tracker.history(false)),
            ("7d", tracker.history(true)),
        ] {
            let matching: Vec<_> = points.iter().filter(|point| point.ts == 0).collect();
            assert_eq!(matching.len(), 1, "{name} bucket timestamp duplicated");
            assert_eq!(
                (matching[0].rx_bps, matching[0].tx_bps),
                (101_000, 202_000),
                "{name} legacy average was assigned an invented weight"
            );
        }

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn loads_existing_completed_history_format() {
        let dir = std::env::temp_dir().join(format!("owfi-traffic-legacy-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("history.json");
        let stored = Persisted {
            h24: vec![RatePoint {
                ts: 1800,
                rx_bps: 10,
                tx_bps: 20,
            }],
            d7: vec![RatePoint {
                ts: 10800,
                rx_bps: 30,
                tx_bps: 40,
            }],
            ..Default::default()
        };
        tokio::fs::write(&path, serde_json::to_vec(&stored).unwrap())
            .await
            .unwrap();

        let tracker = Tracker::load(&path).await;
        let h24 = tracker.history(false);
        let d7 = tracker.history(true);
        assert_eq!(h24.len(), 1);
        assert_eq!((h24[0].ts, h24[0].rx_bps, h24[0].tx_bps), (1800, 10, 20));
        assert_eq!(d7.len(), 1);
        assert_eq!((d7[0].ts, d7[0].rx_bps, d7[0].tx_bps), (10800, 30, 40));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn load_deduplicates_timestamps_before_newer_history_points() {
        let dir =
            std::env::temp_dir().join(format!("owfi-traffic-load-dedupe-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("history.json");
        let point = |ts, rx_bps, tx_bps| RatePoint { ts, rx_bps, tx_bps };
        let legacy = Persisted {
            h24: vec![
                point(3600, 30, 40),
                point(3600, 35, 45),
                point(3600, 40, 50),
                point(5400, 70, 80),
            ],
            d7: vec![
                point(0, 30, 40),
                point(0, 35, 45),
                point(0, 40, 50),
                point(10800, 70, 80),
            ],
            ..Default::default()
        };
        tokio::fs::write(&path, serde_json::to_vec(&legacy).unwrap())
            .await
            .unwrap();

        let tracker = Tracker::load(&path).await;
        let h24 = tracker.history(false);
        let d7 = tracker.history(true);
        assert_eq!(h24.len(), 2);
        assert_eq!((h24[0].ts, h24[0].rx_bps, h24[0].tx_bps), (3600, 40, 50));
        assert_eq!((h24[1].ts, h24[1].rx_bps, h24[1].tx_bps), (5400, 70, 80));
        assert_eq!(d7.len(), 2);
        assert_eq!((d7[0].ts, d7[0].rx_bps, d7[0].tx_bps), (0, 40, 50));
        assert_eq!((d7[1].ts, d7[1].rx_bps, d7[1].tx_bps), (10800, 70, 80));

        let capped = Ring::load(
            vec![
                point(0, 10, 10),
                point(0, 20, 20),
                point(1800, 30, 30),
                point(3600, 40, 40),
            ],
            2,
            None,
        )
        .points();
        assert_eq!(capped.len(), 2);
        assert_eq!((capped[0].ts, capped[0].rx_bps), (1800, 30));
        assert_eq!((capped[1].ts, capped[1].rx_bps), (3600, 40));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn migrates_legacy_active_bucket_without_duplicate_timestamp() {
        let dir = std::env::temp_dir().join(format!("owfi-traffic-migrate-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("history.json");
        let legacy = Persisted {
            h24: vec![
                RatePoint {
                    ts: 1800,
                    rx_bps: 10,
                    tx_bps: 20,
                },
                RatePoint {
                    ts: 3600,
                    rx_bps: 30,
                    tx_bps: 40,
                },
            ],
            d7: vec![RatePoint {
                ts: 0,
                rx_bps: 30,
                tx_bps: 40,
            }],
            ..Default::default()
        };
        tokio::fs::write(&path, serde_json::to_vec(&legacy).unwrap())
            .await
            .unwrap();

        let mut tracker = Tracker::load(&path).await;
        tracker.ring_24h.add(BUCKET_24H_SECS, LEN_24H, 3601, 50, 60);
        tracker.ring_7d.add(BUCKET_7D_SECS, LEN_7D, 3601, 50, 60);
        assert_eq!(
            tracker
                .history(false)
                .iter()
                .filter(|p| p.ts == 3600)
                .count(),
            1
        );
        assert_eq!(
            tracker.history(true).iter().filter(|p| p.ts == 0).count(),
            1
        );

        tracker.ring_24h.add(BUCKET_24H_SECS, LEN_24H, 5400, 70, 80);
        tracker.ring_7d.add(BUCKET_7D_SECS, LEN_7D, 10800, 70, 80);
        tracker.save(&path).await;

        let loaded = Tracker::load(&path).await;
        assert_eq!(
            loaded
                .history(false)
                .iter()
                .filter(|p| p.ts == 3600)
                .count(),
            1
        );
        assert_eq!(loaded.history(true).iter().filter(|p| p.ts == 0).count(), 1);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn collapses_repeated_legacy_active_bucket_entries_across_restart() {
        let dir =
            std::env::temp_dir().join(format!("owfi-traffic-duplicates-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("history.json");
        let point = |ts, rx_bps, tx_bps| RatePoint { ts, rx_bps, tx_bps };
        let legacy = Persisted {
            h24: vec![
                point(1800, 10, 20),
                point(3600, 30, 40),
                point(3600, 35, 45),
                point(3600, 40, 50),
            ],
            d7: vec![point(0, 30, 40), point(0, 35, 45), point(0, 40, 50)],
            ..Default::default()
        };
        tokio::fs::write(&path, serde_json::to_vec(&legacy).unwrap())
            .await
            .unwrap();

        let mut tracker = Tracker::load(&path).await;
        tracker.ring_24h.add(BUCKET_24H_SECS, LEN_24H, 3601, 50, 60);
        tracker.ring_7d.add(BUCKET_7D_SECS, LEN_7D, 3601, 50, 60);
        assert_eq!(
            tracker
                .history(false)
                .iter()
                .filter(|p| p.ts == 3600)
                .count(),
            1
        );
        assert_eq!(
            tracker.history(true).iter().filter(|p| p.ts == 0).count(),
            1
        );
        let resumed_24h = tracker
            .history(false)
            .into_iter()
            .find(|p| p.ts == 3600)
            .unwrap();
        assert_eq!((resumed_24h.rx_bps, resumed_24h.tx_bps), (50, 60));
        let resumed_7d = tracker
            .history(true)
            .into_iter()
            .find(|p| p.ts == 0)
            .unwrap();
        assert_eq!((resumed_7d.rx_bps, resumed_7d.tx_bps), (50, 60));

        tracker.save(&path).await;
        let loaded = Tracker::load(&path).await;
        assert_eq!(
            loaded
                .history(false)
                .iter()
                .filter(|p| p.ts == 3600)
                .count(),
            1
        );
        assert_eq!(loaded.history(true).iter().filter(|p| p.ts == 0).count(), 1);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
