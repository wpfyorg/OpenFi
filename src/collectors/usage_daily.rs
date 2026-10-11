//! `GET /usage/daily` — our own daily rollup, kept in RAM and persisted to
//! `daily_usage_file` (default `/etc/openwrtfi/daily.json`).
//!
//! Verified on mainrouter (2026-09-24): `/etc/usage-history/<YYYY-MM>.csv` is a **per-device,
//! per-month** accounting file (`mac,name,ip,dl_bytes,ul_bytes`) written by `net-account` — it
//! has no daily rows at all, and there is no other daily source anywhere on the router. So this
//! binary keeps its own running total of *today's* WAN rx/tx bytes (from the same WAN
//! interface-counter deltas `collectors::traffic` samples) and a count of distinct devices seen
//! online today, rolls that over into a completed `DailyUsage` row at local midnight (see
//! `Config.local_utc_offset_secs` — there's no timezone database in this binary), and keeps the
//! last 90 rolled-over days.
//!
//! Flash wear: `maybe_save` only actually writes `daily_usage_file` if at least an hour has
//! passed since the last write (or it's never been written), same spirit as the 24h/7d history
//! ring in `collectors::traffic`. The caller is also expected to call `save` once on shutdown.
//! Completed days and the in-progress day's totals, device set, and WAN counter baseline are
//! persisted so a service restart can continue the current day without losing or double-counting
//! traffic. Older files containing only completed days remain readable.
//!
//! Per the contract, days before this binary was ever running not just show as zero — they're
//! simply absent from `rows()`, and the UI shows an empty state for them.

use std::collections::{HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{collectors::date::day_key, model::DailyUsage};

/// Keep at most this many completed days (a little over the 90-day window the contract's own
/// default range never actually asks for, but +30 gives headroom for `?days=` overrides).
const MAX_DAYS: usize = 90;

pub struct DailyTracker {
    utc_offset_secs: i64,
    /// Completed days only, oldest first, capped to `MAX_DAYS`.
    days: VecDeque<DailyUsage>,
    /// The day the in-progress totals below belong to; `None` until the first `tick`.
    current_day: Option<String>,
    rx_bytes: u64,
    tx_bytes: u64,
    devices_seen: HashSet<String>,
    /// Last WAN cumulative counters seen, to compute this poll's delta.
    last_wan: Option<(u64, u64)>,
    dirty: bool,
    last_saved: std::time::Instant,
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    days: Vec<DailyUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<PersistedCurrent>,
}

#[derive(Serialize, Deserialize)]
struct PersistedCurrent {
    day: String,
    rx_bytes: u64,
    tx_bytes: u64,
    devices_seen: Vec<String>,
    last_wan: Option<(u64, u64)>,
}

impl DailyTracker {
    pub async fn load(path: &std::path::Path, utc_offset_secs: i64) -> Self {
        let persisted = match tokio::fs::read(path).await {
            Ok(bytes) => serde_json::from_slice::<Persisted>(&bytes).unwrap_or_default(),
            Err(_) => Persisted::default(),
        };
        let current = persisted.current;
        Self {
            utc_offset_secs,
            days: persisted.days.into(),
            current_day: current.as_ref().map(|c| c.day.clone()),
            rx_bytes: current.as_ref().map_or(0, |c| c.rx_bytes),
            tx_bytes: current.as_ref().map_or(0, |c| c.tx_bytes),
            devices_seen: current
                .as_ref()
                .map(|c| c.devices_seen.iter().cloned().collect())
                .unwrap_or_default(),
            last_wan: current.and_then(|c| c.last_wan),
            dirty: false,
            last_saved: std::time::Instant::now(),
        }
    }

    /// Called once per poll with the WAN device's cumulative rx/tx byte counters (since boot)
    /// and every currently-online client MAC. Rolls the previous day into `days` if the day has
    /// changed since the last call.
    pub fn tick<'a>(
        &mut self,
        ts: u64,
        wan_rx_bytes: u64,
        wan_tx_bytes: u64,
        online_macs: impl Iterator<Item = &'a str>,
    ) {
        let day = day_key(ts, self.utc_offset_secs);
        if self.current_day.as_deref().is_some_and(|d| d != day) {
            self.roll_over(day.clone());
        }
        self.current_day.get_or_insert_with(|| day.clone());

        if let Some((lrx, ltx)) = self.last_wan {
            if wan_rx_bytes >= lrx {
                self.rx_bytes += wan_rx_bytes - lrx;
            }
            if wan_tx_bytes >= ltx {
                self.tx_bytes += wan_tx_bytes - ltx;
            }
        }
        self.last_wan = Some((wan_rx_bytes, wan_tx_bytes));
        self.devices_seen.extend(online_macs.map(str::to_owned));
        self.dirty = true;
    }

    fn roll_over(&mut self, new_day: String) {
        if let Some(day) = self.current_day.take() {
            self.days.push_back(DailyUsage {
                day,
                rx_bytes: self.rx_bytes,
                tx_bytes: self.tx_bytes,
                devices: self.devices_seen.len() as u32,
            });
            while self.days.len() > MAX_DAYS {
                self.days.pop_front();
            }
            self.dirty = true;
        }
        self.current_day = Some(new_day);
        self.rx_bytes = 0;
        self.tx_bytes = 0;
        self.devices_seen.clear();
    }

    /// Completed days plus today's in-progress totals, oldest first — callers slice this down
    /// to whatever `?days=` window they need (see `api::usage_daily`). Days this tracker never
    /// saw (before install, or lost to a gap) are simply absent, not zero-filled.
    pub fn rows(&self) -> Vec<DailyUsage> {
        let mut out: Vec<DailyUsage> = self.days.iter().cloned().collect();
        if let Some(day) = &self.current_day {
            out.push(DailyUsage {
                day: day.clone(),
                rx_bytes: self.rx_bytes,
                tx_bytes: self.tx_bytes,
                devices: self.devices_seen.len() as u32,
            });
        }
        out
    }

    /// Writes `daily_usage_file` only if dirty and it's been at least an hour since the last
    /// write — flash wear. Call `save` directly (bypassing this guard) on shutdown.
    pub async fn maybe_save(&mut self, path: &std::path::Path) {
        if self.dirty && self.last_saved.elapsed() >= std::time::Duration::from_secs(3600) {
            self.save(path).await;
        }
    }

    pub async fn save(&mut self, path: &std::path::Path) {
        let p = Persisted {
            days: self.days.iter().cloned().collect(),
            current: self.current_day.as_ref().map(|day| PersistedCurrent {
                day: day.clone(),
                rx_bytes: self.rx_bytes,
                tx_bytes: self.tx_bytes,
                devices_seen: self.devices_seen.iter().cloned().collect(),
                last_wan: self.last_wan,
            }),
        };
        if let Ok(bytes) = serde_json::to_vec(&p) {
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            if tokio::fs::write(path, bytes).await.is_ok() {
                self.dirty = false;
                self.last_saved = std::time::Instant::now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::date::days_from_civil;

    fn ts(y: i32, m: u32, d: u32, hms: (u32, u32, u32)) -> u64 {
        days_from_civil(y, m, d) as u64 * 86400
            + hms.0 as u64 * 3600
            + hms.1 as u64 * 60
            + hms.2 as u64
    }

    #[tokio::test]
    async fn accumulates_within_a_day_and_rolls_over_at_midnight() {
        let mut t = DailyTracker::load(std::path::Path::new("/nonexistent"), 0).await;
        t.tick(
            ts(2026, 9, 24, (0, 0, 0)),
            1_000,
            200,
            ["aa:bb"].into_iter(),
        );
        t.tick(
            ts(2026, 9, 24, (12, 0, 0)),
            5_000,
            1_200,
            ["aa:bb", "cc:dd"].into_iter(),
        );
        let rows = t.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].day, "2026-09-24");
        assert_eq!(rows[0].rx_bytes, 4_000);
        assert_eq!(rows[0].tx_bytes, 1_000);
        assert_eq!(rows[0].devices, 2);

        // Next day: yesterday becomes a completed row, today starts fresh.
        t.tick(
            ts(2026, 9, 25, (0, 30, 0)),
            5_500,
            1_300,
            ["aa:bb"].into_iter(),
        );
        let rows = t.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].day, "2026-09-24");
        assert_eq!(rows[0].rx_bytes, 4_000); // unchanged, sealed
        assert_eq!(rows[1].day, "2026-09-25");
        assert_eq!(rows[1].rx_bytes, 500); // fresh delta from the day's first sample
    }

    #[tokio::test]
    async fn wan_counter_reset_does_not_go_negative() {
        let mut t = DailyTracker::load(std::path::Path::new("/nonexistent"), 0).await;
        t.tick(
            ts(2026, 9, 24, (0, 0, 0)),
            10_000,
            2_000,
            std::iter::empty(),
        );
        // Counter reset (WAN iface restart / reboot): smaller than last sample.
        t.tick(ts(2026, 9, 24, (0, 1, 0)), 500, 100, std::iter::empty());
        let rows = t.rows();
        assert_eq!(rows[0].rx_bytes, 0);
        assert_eq!(rows[0].tx_bytes, 0);
    }

    #[tokio::test]
    async fn wan_counter_directions_reset_independently() {
        async fn sample_pair(after_rx: u64, after_tx: u64) -> DailyUsage {
            let mut tracker = DailyTracker::load(std::path::Path::new("/nonexistent"), 0).await;
            tracker.tick(
                ts(2026, 9, 24, (0, 0, 0)),
                10_000,
                2_000,
                std::iter::empty(),
            );
            tracker.tick(
                ts(2026, 9, 24, (0, 1, 0)),
                after_rx,
                after_tx,
                std::iter::empty(),
            );
            tracker.rows().remove(0)
        }

        let neither = sample_pair(10_100, 2_200).await;
        assert_eq!((neither.rx_bytes, neither.tx_bytes), (100, 200));

        let rx_reset = sample_pair(500, 2_300).await;
        assert_eq!((rx_reset.rx_bytes, rx_reset.tx_bytes), (0, 300));

        let tx_reset = sample_pair(10_300, 100).await;
        assert_eq!((tx_reset.rx_bytes, tx_reset.tx_bytes), (300, 0));

        let both_reset = sample_pair(500, 100).await;
        assert_eq!((both_reset.rx_bytes, both_reset.tx_bytes), (0, 0));
    }

    #[tokio::test]
    async fn caps_history_at_90_days() {
        let mut t = DailyTracker::load(std::path::Path::new("/nonexistent"), 0).await;
        for i in 0..100 {
            let (y, m, d) =
                crate::collectors::date::civil_from_days(days_from_civil(2026, 1, 1) + i as i64);
            t.tick(
                ts(y, m, d, (0, 0, 0)),
                i as u64 * 100,
                i as u64 * 10,
                std::iter::empty(),
            );
        }
        // 100 ticks = 99 day-boundaries crossed = 99 completed rows, capped to 90, plus today.
        assert!(t.rows().len() <= MAX_DAYS + 1);
    }

    #[tokio::test]
    async fn loads_legacy_completed_days_only_format() {
        let dir = std::env::temp_dir().join(format!("owfi-daily-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("daily.json");

        tokio::fs::write(
            &path,
            r#"{"days":[{"day":"2026-09-24","rx_bytes":1000,"tx_bytes":200,"devices":2}]}"#,
        )
        .await
        .unwrap();

        let loaded = DailyTracker::load(&path, 0).await;
        let rows = loaded.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].day, "2026-09-24");
        assert_eq!(rows[0].rx_bytes, 1_000);
        assert_eq!(rows[0].tx_bytes, 200);
        assert_eq!(rows[0].devices, 2);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn save_and_load_preserves_current_day_and_wan_baseline() {
        let dir = std::env::temp_dir().join(format!("owfi-daily-restart-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("daily.json");

        let mut tracker = DailyTracker::load(&path, 0).await;
        tracker.tick(ts(2026, 9, 24, (0, 0, 0)), 1_000, 200, std::iter::empty());
        tracker.tick(
            ts(2026, 9, 24, (12, 0, 0)),
            1_500,
            450,
            ["aa:bb", "cc:dd"].into_iter(),
        );
        tracker.save(&path).await;

        let mut loaded = DailyTracker::load(&path, 0).await;
        loaded.tick(
            ts(2026, 9, 24, (13, 0, 0)),
            1_700,
            550,
            ["aa:bb"].into_iter(),
        );
        let rows = loaded.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].day, "2026-09-24");
        assert_eq!(rows[0].rx_bytes, 700);
        assert_eq!(rows[0].tx_bytes, 350);
        assert_eq!(rows[0].devices, 2);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
