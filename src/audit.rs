//! In-memory audit log, and its on-disk mirror (`/tmp/openwrtfi-audit.log`, JSON lines, capped
//! at 1000). Every action handler (`src/actions.rs`, `src/plan.rs`, `src/shell.rs`) calls
//! `record()` exactly once per write — success or failure — and never with a raw secret in
//! `detail` (see `mask_secret`/`mask_uci_line`).

use std::collections::VecDeque;

use serde::Serialize;
use tokio::sync::Mutex;

/// In-memory cap (`GET /audit` never needs more than this many rows at once).
const MAX_ENTRIES: usize = 500;
/// On-disk cap, per the contract ("capped at 1000").
const MAX_LOG_LINES: usize = 1000;

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct AuditEntry {
    pub ts: u64,
    pub user: String,
    pub action: String,
    pub target: String,
    pub result: AuditResult,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditResult {
    Ok,
    Error,
}

pub struct Audit {
    entries: Mutex<VecDeque<AuditEntry>>,
    disk_write: Mutex<()>,
    log_path: std::path::PathBuf,
}

impl Default for Audit {
    fn default() -> Self {
        Self::new("/tmp/openwrtfi-audit.log".into())
    }
}

impl Audit {
    pub fn new(log_path: std::path::PathBuf) -> Self {
        Self {
            entries: Mutex::new(load_entries(&log_path)),
            disk_write: Mutex::new(()),
            log_path,
        }
    }

    pub async fn record(&self, entry: AuditEntry) {
        {
            let mut entries = self.entries.lock().await;
            entries.push_front(entry.clone());
            entries.truncate(MAX_ENTRIES);
        }
        self.append_to_disk(&entry).await;
    }

    /// Newest first, per the contract.
    pub async fn list(&self) -> Vec<AuditEntry> {
        self.entries.lock().await.iter().cloned().collect()
    }

    /// Appends one JSON line to `log_path`, then truncates the file to the last `MAX_LOG_LINES`
    /// lines if it's grown past that — a simple read-modify-write rather than a real log
    /// rotator, which is fine at this cadence (one action write, at most every few seconds).
    async fn append_to_disk(&self, entry: &AuditEntry) {
        let _guard = self.disk_write.lock().await;
        let Ok(line) = serde_json::to_string(entry) else {
            return;
        };
        let existing = tokio::fs::read_to_string(&self.log_path)
            .await
            .unwrap_or_default();
        let mut lines: VecDeque<&str> = existing.lines().collect();
        lines.push_back(&line);
        while lines.len() > MAX_LOG_LINES {
            lines.pop_front();
        }
        let mut out = lines.iter().fold(String::new(), |mut acc, l| {
            acc.push_str(l);
            acc.push('\n');
            acc
        });
        if out.is_empty() {
            out.push_str(&line);
            out.push('\n');
        }
        let _ = tokio::fs::write(&self.log_path, out).await;
    }
}

/// Restore valid entries in newest-first order. A malformed or truncated JSONL row is skipped
/// independently, so it cannot hide later valid rows. Only the newest in-memory window is kept.
fn load_entries(log_path: &std::path::Path) -> VecDeque<AuditEntry> {
    let Ok(contents) = std::fs::read_to_string(log_path) else {
        return VecDeque::new();
    };

    let mut entries = VecDeque::new();
    for line in contents.lines() {
        let Ok(entry) = serde_json::from_str::<AuditEntry>(line) else {
            continue;
        };
        entries.push_front(entry);
        entries.truncate(MAX_ENTRIES);
    }
    entries
}

/// Masks a secret value for an audit `detail`/diff line — never the raw WiFi key or password,
/// just enough to show *that* it changed.
pub fn mask_secret(_value: &str) -> &'static str {
    "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"
}

/// `uci set <config>.<section>.<option>='<value>'`-style diff line, masking the value when
/// `option` is one that carries a secret (`key`, `password`).
pub fn mask_uci_line(config: &str, section: &str, option: &str, value: &str) -> String {
    let shown = if matches!(option, "key" | "password") {
        mask_secret(value)
    } else {
        value
    };
    format!("{config}.{section}.{option}='{shown}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("owfi-audit-test-{name}-{}.log", std::process::id()))
    }

    fn entry(ts: u64) -> AuditEntry {
        AuditEntry {
            ts,
            user: "root".into(),
            action: "kick".into(),
            target: "aa:bb".into(),
            result: AuditResult::Ok,
            detail: String::new(),
        }
    }

    #[tokio::test]
    async fn records_newest_first_and_caps_in_memory_length() {
        let path = temp_log("caps");
        let audit = Audit::new(path.clone());
        for i in 0..(MAX_ENTRIES + 10) {
            audit.record(entry(i as u64)).await;
        }
        let list = audit.list().await;
        assert_eq!(list.len(), MAX_ENTRIES);
        assert_eq!(list[0].ts, (MAX_ENTRIES + 9) as u64);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn persists_json_lines_to_disk() {
        let path = temp_log("jsonl");
        let audit = Audit::new(path.clone());
        audit.record(entry(1)).await;
        audit.record(entry(2)).await;
        let text = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let parsed: AuditEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.ts, 1);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn restores_capped_newest_first_history_after_restart() {
        let path = temp_log("restart");
        let _ = std::fs::remove_file(&path);
        {
            let audit = Audit::new(path.clone());
            for i in 0..(MAX_ENTRIES + 10) {
                let mut entry = entry(i as u64);
                entry.detail = mask_uci_line("wireless", "default_radio1", "key", "secret");
                audit.record(entry).await;
            }
        }

        // Malformed rows should be skipped individually, including a truncated final row.
        let contents = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<_> = contents.lines().collect();
        lines.insert(1, "not-json");
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"ts\":999")
            .unwrap();

        let restarted = Audit::new(path.clone());
        let list = restarted.list().await;
        assert_eq!(list.len(), MAX_ENTRIES);
        assert_eq!(list[0].ts, (MAX_ENTRIES + 9) as u64);
        assert_eq!(list[MAX_ENTRIES - 1].ts, 10);
        assert!(list[0].detail.contains("••••••••"));
        assert!(!list[0].detail.contains("secret"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn concurrent_records_all_survive_disk_reload() {
        let path = temp_log("concurrent");
        let _ = std::fs::remove_file(&path);
        let audit = std::sync::Arc::new(Audit::new(path.clone()));
        let start = std::sync::Arc::new(tokio::sync::Barrier::new(201));
        let mut tasks = Vec::new();
        for i in 0..200u64 {
            let audit = audit.clone();
            let start = start.clone();
            tasks.push(tokio::spawn(async move {
                let mut entry = entry(i);
                entry.target = format!("device-{i}");
                start.wait().await;
                audit.record(entry).await;
            }));
        }
        start.wait().await;
        for task in tasks {
            task.await.unwrap();
        }

        let text = tokio::fs::read_to_string(&path).await.unwrap();
        let persisted: Vec<AuditEntry> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let targets: std::collections::HashSet<_> = persisted
            .iter()
            .map(|entry| entry.target.as_str())
            .collect();
        assert_eq!(persisted.len(), 200);
        assert_eq!(targets.len(), 200);

        let reloaded: Vec<AuditEntry> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(reloaded.len(), 200);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn disk_log_is_capped_at_1000_lines() {
        let path = temp_log("cap1000");
        let audit = Audit::new(path.clone());
        for i in 0..1010u64 {
            audit.record(entry(i)).await;
        }
        let text = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), MAX_LOG_LINES);
        // Oldest lines were dropped, newest kept.
        let first: AuditEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.ts, 1010 - MAX_LOG_LINES as u64);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn secrets_are_masked_never_shown_raw() {
        assert_eq!(
            mask_secret("super-secret-key"),
            "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}"
        );
        assert!(!mask_secret("super-secret-key").contains("super-secret-key"));
    }

    #[test]
    fn uci_line_masks_key_and_password_but_not_other_options() {
        let masked = mask_uci_line("wireless", "default_radio1", "key", "hunter2");
        assert!(!masked.contains("hunter2"));
        assert!(masked.contains("\u{2022}"));

        let plain = mask_uci_line("wireless", "default_radio1", "ssid", "MyWiFi");
        assert!(plain.contains("MyWiFi"));
    }
}
