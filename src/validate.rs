//! Input validators shared by the action handlers (`src/actions.rs`, `src/plan.rs`). Hand-rolled
//! (no `regex` crate — see the binary size budget in plan.md).

/// Strict `aa:bb:cc:dd:ee:ff` — six colon-separated hex octets, either case. This is deliberately
/// tighter than what the kernel/hostapd accept (no `-` separators, no bare 12-hex-digit form),
/// since it's only ever used to build a `hostapd`/`qos-tiers` argument, not to parse arbitrary
/// input from the router.
pub fn mac_ok(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Lowercases a MAC that's already passed `mac_ok`, for consistent lookups/logging.
pub fn mac_lower(s: &str) -> String {
    s.to_lowercase()
}

/// A dotted-quad IPv4 address (no CIDR, no leading zeros ambiguity — each octet 0-255, parsed
/// with `u8::from_str` which already rejects leading zeros like "01").
pub fn ipv4_ok(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| p.parse::<u8>().is_ok() && (p.len() == 1 || !p.starts_with('0')))
}

/// Whether `ip` falls inside `192.168.1.0/24` — the LAN subnet this controller manages
/// reservations on (see `AGENTS.md`/plan.md's topology). Hardcoded rather than read from `uci
/// show network` because a reservation write happens before we'd trust a freshly-read LAN
/// config anyway; revisit if this controller ever needs to run on a differently-addressed LAN.
pub fn ip_in_lan(ip: &str) -> bool {
    ipv4_ok(ip) && ip.starts_with("192.168.1.")
}

/// The DHCP dynamic pool, `.100`-`.239` (see `AGENTS.md`). A reservation *inside* this range
/// isn't rejected outright (a device might already be leased there and just needs pinning), but
/// callers should surface it as a warning — see `actions::reserve`.
pub fn ip_in_dhcp_pool(ip: &str) -> bool {
    let Some(last) = ip.rsplit('.').next().and_then(|s| s.parse::<u16>().ok()) else {
        return false;
    };
    ip_in_lan(ip) && (100..=239).contains(&last)
}

/// `^[A-Za-z0-9_-]{1,63}$` — a DHCP host name / reservation label. Not a full RFC 1123 hostname
/// validator (no dots — reservation names are a single label), just enough to keep it out of
/// `uci add dhcp host`'s `name` option safely.
pub fn reservation_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// WiFi channel ranges. `"auto"` (channel 0 in uci) is handled by the caller before this is
/// reached. 2.4 GHz: 1-14 (regulatory domains vary, but nothing legitimate is outside this).
/// 5 GHz: the union of UNII-1 through UNII-4 channel numbers in common use (36-165).
pub fn channel_ok(band_5ghz: bool, channel: u32) -> bool {
    if band_5ghz {
        (36..=165).contains(&channel)
    } else {
        (1..=14).contains(&channel)
    }
}

/// Reasonable txpower bounds in dBm for this hardware (MT7915, see plan.md) — 0 disables the
/// radio's own attenuation (max power), 30 dBm (1 W) is above what's legal almost anywhere, so
/// anything past it is certainly a typo.
pub fn txpower_ok(dbm: i32) -> bool {
    (0..=30).contains(&dbm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_validation() {
        assert!(mac_ok("aa:bb:cc:dd:ee:ff"));
        assert!(mac_ok("AA:BB:CC:DD:EE:FF"));
        assert!(!mac_ok("aabbccddeeff"));
        assert!(!mac_ok("aa:bb:cc:dd:ee"));
        assert!(!mac_ok("aa-bb-cc-dd-ee-ff"));
        assert!(!mac_ok("aa:bb:cc:dd:ee:gg"));
        assert!(!mac_ok("aa:bb:cc:dd:ee:ff; rm -rf /"));
    }

    #[test]
    fn ipv4_validation() {
        assert!(ipv4_ok("192.0.2.43"));
        assert!(ipv4_ok("0.0.0.0"));
        assert!(ipv4_ok("255.255.255.255"));
        assert!(!ipv4_ok("192.168.68"));
        assert!(!ipv4_ok("192.0.2.256"));
        assert!(!ipv4_ok("192.168.068.42")); // leading zero
        assert!(!ipv4_ok("192.0.2.43.1"));
        assert!(!ipv4_ok("192.0.2.43; rm -rf /"));
    }

    #[test]
    fn lan_and_pool_ranges() {
        assert!(ip_in_lan("192.168.1.43"));
        assert!(!ip_in_lan("198.51.100.42"));
        assert!(ip_in_dhcp_pool("192.168.1.101"));
        assert!(ip_in_dhcp_pool("192.168.1.239"));
        assert!(!ip_in_dhcp_pool("192.168.1.99"));
        assert!(!ip_in_dhcp_pool("192.168.1.240"));
        assert!(!ip_in_dhcp_pool("198.51.100.150")); // not even in the LAN
    }

    #[test]
    fn reservation_names() {
        assert!(reservation_name_ok("client10"));
        assert!(reservation_name_ok("client13_2"));
        assert!(!reservation_name_ok(""));
        assert!(!reservation_name_ok("has spaces"));
        assert!(!reservation_name_ok("semi;colon"));
        assert!(!reservation_name_ok(&"a".repeat(64)));
    }

    #[test]
    fn channel_ranges() {
        assert!(channel_ok(false, 1));
        assert!(channel_ok(false, 14));
        assert!(!channel_ok(false, 0));
        assert!(!channel_ok(false, 15));
        assert!(channel_ok(true, 36));
        assert!(channel_ok(true, 165));
        assert!(!channel_ok(true, 14));
        assert!(!channel_ok(true, 200));
    }

    #[test]
    fn txpower_range() {
        assert!(txpower_ok(0));
        assert!(txpower_ok(30));
        assert!(!txpower_ok(-1));
        assert!(!txpower_ok(31));
    }
}
