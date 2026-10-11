//! `GET /logs?node=&lines=` — `logread -l <lines>` via the `file` ubus object's `exec` method
//! (so it works identically for the local gateway and, through rpcd, the remote APs).
//!
//! busybox `logread` lines look like:
//! `Thu Sep 24 10:00:01 2026 daemon.info hostapd: phy1-ap0: STA associated` — no hostname field
//! (that's not real syslog, just `ctime()` + facility.level + tag). Lines this parser can't
//! make sense of are dropped rather than failing the whole request.

use serde_json::json;

use crate::{
    collectors::date::{days_from_civil, month_number},
    model::{LogLevel, LogLine},
    ubus::Ubus,
};

pub async fn collect(ubus: &Ubus, node: &str, lines: u32) -> Vec<LogLine> {
    let Ok(v) = ubus
        .call(
            "file",
            "exec",
            json!({ "command": "logread", "params": ["-l", lines.to_string()] }),
        )
        .await
    else {
        return vec![];
    };
    let Some(stdout) = v["stdout"].as_str() else {
        return vec![];
    };
    stdout.lines().filter_map(|l| parse_line(node, l)).collect()
}

fn parse_line(node: &str, line: &str) -> Option<LogLine> {
    let mut it = line.splitn(7, ' ');
    let _weekday = it.next()?;
    let month = month_number(it.next()?)?;
    let day: u32 = it.next()?.parse().ok()?;
    let time = it.next()?;
    let year: i32 = it.next()?.parse().ok()?;
    let faclevel = it.next()?;
    let rest = it.next()?;

    let mut hms = time.splitn(3, ':');
    let h: i64 = hms.next()?.parse().ok()?;
    let m: i64 = hms.next()?.parse().ok()?;
    let s: i64 = hms.next()?.parse().ok()?;
    let ts = (days_from_civil(year, month, day) * 86400 + h * 3600 + m * 60 + s) as u64;

    let level = match faclevel.rsplit('.').next() {
        Some("err" | "crit" | "alert" | "emerg") => LogLevel::Err,
        Some("warn" | "warning") => LogLevel::Warn,
        _ => LogLevel::Info,
    };
    let (source, msg) = rest.split_once(": ").unwrap_or(("", rest));

    Some(LogLine {
        ts,
        node: node.to_owned(),
        level,
        source: source.to_owned(),
        msg: msg.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ubus::Ubus;

    #[test]
    fn parses_a_typical_logread_line() {
        let l = parse_line(
            "mainrouter",
            "Thu Sep 24 10:00:06 2026 kern.err mt7915e 0000:01:00.0: link down",
        )
        .unwrap();
        assert_eq!(l.node, "mainrouter");
        assert_eq!(l.level, LogLevel::Err);
        assert_eq!(l.source, "mt7915e 0000:01:00.0");
        assert_eq!(l.msg, "link down");
    }

    #[test]
    fn defaults_to_info_for_unknown_levels() {
        let l = parse_line(
            "ap-2",
            "Thu Sep 24 10:00:07 2026 daemon.info dnsmasq-dhcp[1]: DHCPACK(br-lan) 192.0.2.11",
        )
        .unwrap();
        assert_eq!(l.level, LogLevel::Info);
        assert_eq!(l.source, "dnsmasq-dhcp[1]");
    }

    #[test]
    fn garbage_line_is_skipped() {
        assert!(parse_line("ap-2", "not a logread line").is_none());
    }

    #[tokio::test]
    async fn reads_the_fixture() {
        let dir = std::path::PathBuf::from(format!(
            "{}/fixtures/mainrouter",
            env!("CARGO_MANIFEST_DIR")
        ));
        let ubus = Ubus::fixture(dir);
        let lines = collect(&ubus, "mainrouter", 200).await;
        assert!(!lines.is_empty());
        assert!(lines.iter().all(|l| l.node == "mainrouter"));
    }
}
