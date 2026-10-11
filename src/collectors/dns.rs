//! AdGuard Home `/control/stats` → `Dns`. Shape is AdGuard's real (documented) API:
//! <https://github.com/AdguardTeam/AdGuardHome/wiki/API>. `avg_processing_time` is seconds;
//! the contract wants milliseconds. `top_blocked_domains`/`top_queried_domains` are arrays of
//! single-key objects (`{"domain.com": 123}`), and `dns_queries` is already oldest-first.

use serde_json::Value;

use crate::{
    config::Config,
    http,
    model::{Dns, TopBlocked},
};

pub async fn collect(cfg: &Config) -> Option<Dns> {
    let raw = fetch(cfg).await?;
    Some(parse(&raw))
}

async fn fetch(cfg: &Config) -> Option<Value> {
    if let Some(path) = &cfg.dns_fixture {
        let bytes = tokio::fs::read(path).await.ok()?;
        return serde_json::from_slice(&bytes).ok();
    }
    let ag = cfg.adguard.as_ref()?;
    let host = ag.url.trim_start_matches("http://").trim_end_matches('/');
    let auth = match (&ag.username, &ag.password) {
        (Some(u), Some(p)) => Some(format!("Basic {}", base64(format!("{u}:{p}").as_bytes()))),
        _ => None,
    };
    let headers: Vec<(&str, &str)> = auth
        .as_deref()
        .map(|a| vec![("Authorization", a)])
        .unwrap_or_default();
    match http::request("GET", host, "/control/stats", &headers, None).await {
        Ok((200, body)) => serde_json::from_slice(&body).ok(),
        Ok((code, _)) => {
            log::debug!("adguard HTTP {code}");
            None
        }
        Err(e) => {
            log::debug!("adguard: {e}");
            None
        }
    }
}

fn parse(v: &Value) -> Dns {
    let top_blocked = v["top_blocked_domains"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let (domain, count) = entry.as_object()?.iter().next()?;
            Some(TopBlocked {
                domain: domain.clone(),
                count: count.as_u64().unwrap_or(0),
            })
        })
        .collect();
    let hourly_queries = v["dns_queries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect();
    Dns {
        queries: v["num_dns_queries"].as_u64().unwrap_or(0),
        blocked: v["num_blocked_filtering"].as_u64().unwrap_or(0),
        avg_ms: v["avg_processing_time"].as_f64().unwrap_or(0.0) * 1000.0,
        top_blocked,
        hourly_queries,
    }
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_adguard_shape() {
        let v: Value =
            serde_json::from_str(include_str!("../../fixtures/adguard-stats.json")).unwrap();
        let dns = parse(&v);
        assert_eq!(dns.queries, 48213);
        assert_eq!(dns.blocked, 6104);
        assert!((dns.avg_ms - 3.7).abs() < 0.001);
        assert_eq!(dns.top_blocked.len(), 5);
        assert_eq!(dns.top_blocked[0].domain, "doubleclick.net");
        assert_eq!(dns.top_blocked[0].count, 812);
        assert_eq!(dns.hourly_queries.len(), 24);
    }
}
