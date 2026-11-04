//! Read-only per-node inventory. A failed probe contributes no capability, so callers can
//! safely hide controls when rpcd ACLs or firmware omit an object.

use serde::Serialize;
use serde_json::Value;

use crate::ubus::Ubus;

#[derive(Debug, Default, Serialize)]
pub struct Capabilities {
    pub packages: Vec<String>,
    pub ubus_objects: Vec<UbusObject>,
    pub protocols: Vec<String>,
    pub services: Vec<String>,
    /// Stable UI action identifiers, present only when the backing method/config was found.
    pub actions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UbusObject {
    pub name: String,
    pub methods: Vec<String>,
}

pub async fn discover(ubus: &Ubus, gateway: bool, shell_enabled: bool) -> Capabilities {
    let mut result = Capabilities::default();
    let Ok(objects) = ubus.list("*").await else {
        return result;
    };
    result.ubus_objects = objects
        .iter()
        .map(|name| UbusObject {
            name: name.clone(),
            methods: Vec::new(),
        })
        .collect();

    let mut system_board = false;
    for (object, action) in [
        ("system", "node.reboot"),
        ("network.wireless", "wifi.reload"),
        ("uci", "radio.configure"),
    ] {
        if let Ok(methods) = ubus.methods(object).await {
            if let Some(entry) = result
                .ubus_objects
                .iter_mut()
                .find(|item| item.name == object)
            {
                entry.methods = methods.clone();
            }
            let supported = match action {
                "node.reboot" => {
                    system_board = methods.iter().any(|method| method == "board");
                    methods.iter().any(|method| method == "reboot")
                }
                "wifi.reload" => methods.iter().any(|method| method == "reload"),
                _ => ["set", "apply", "confirm"]
                    .iter()
                    .all(|required| methods.iter().any(|method| method == required)),
            };
            if supported {
                result.actions.push(action.into());
            }
        }
    }
    if result
        .actions
        .iter()
        .any(|action| action == "radio.configure")
    {
        if let Ok(wireless) = ubus
            .call("uci", "get", serde_json::json!({"config":"wireless"}))
            .await
        {
            if wireless.get("values").and_then(Value::as_object).is_none() {
                result.actions.retain(|action| action != "radio.configure");
            }
        } else {
            result.actions.retain(|action| action != "radio.configure");
        }
    }
    if !system_board {
        result.actions.retain(|action| action != "node.reboot");
    }

    result.packages = installed_packages(ubus).await.unwrap_or_default();
    result.protocols = protocols_for(&result.packages);
    if let Ok(services) = ubus.call("service", "list", serde_json::json!({})).await
        && let Some(map) = services.as_object()
    {
        result.services = map.keys().cloned().collect();
    }
    if gateway && shell_enabled {
        result.actions.push("shell.open".into());
    }
    result.actions.sort();
    result
}

async fn installed_packages(ubus: &Ubus) -> Option<Vec<String>> {
    let apk_database = ubus.read_file("/lib/apk/db/installed").await.ok();
    if let Some(packages) = select_package_database(apk_database.as_deref(), None) {
        return Some(packages);
    }
    let opkg_database = ubus.read_file("/usr/lib/opkg/status").await.ok();
    select_package_database(None, opkg_database.as_deref())
}

fn select_package_database(apk: Option<&str>, opkg: Option<&str>) -> Option<Vec<String>> {
    apk.and_then(parse_apk_packages).or_else(|| {
        opkg.map(parse_opkg_packages)
            .filter(|packages| !packages.is_empty())
    })
}

/// APK's installed database has one `P:<name>` record per installed package.
/// `None` means the readable content was malformed/empty, so legacy fallback may be tried.
fn parse_apk_packages(database: &str) -> Option<Vec<String>> {
    let mut packages = database
        .lines()
        .filter_map(|line| line.strip_prefix("P:"))
        .filter(|name| valid_name(name))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    packages.sort();
    packages.dedup();
    (!packages.is_empty()).then_some(packages)
}

fn parse_opkg_packages(status: &str) -> Vec<String> {
    let mut packages = status
        .split("\n\n")
        .filter_map(|entry| {
            let installed = entry
                .lines()
                .any(|line| line.trim() == "Status: install user installed");
            let name = entry
                .lines()
                .find_map(|line| line.strip_prefix("Package: "))?;
            (installed && valid_name(name)).then(|| name.to_owned())
        })
        .collect::<Vec<_>>();
    packages.sort();
    packages.dedup();
    packages
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'+'))
}

fn protocols_for(packages: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    for (package, protocol) in [
        ("ppp", "ppp"),
        ("ppp-mod-pppoe", "pppoe"),
        ("wireguard-tools", "wireguard"),
        ("openvpn-openssl", "openvpn"),
        ("l2tp", "l2tp"),
        ("ocserv", "openconnect"),
    ] {
        if packages.iter().any(|installed| installed == package) {
            found.push(protocol.into());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::{parse_apk_packages, parse_opkg_packages, protocols_for, select_package_database};

    #[test]
    fn package_parser_keeps_only_installed_valid_entries() {
        let text = "Package: ppp\nStatus: install user installed\n\nPackage: wireguard-tools\nStatus: deinstall user not-installed\n\nPackage: bad name\nStatus: install user installed\n";
        assert_eq!(parse_opkg_packages(text), vec!["ppp"]);
        assert_eq!(protocols_for(&parse_opkg_packages(text)), vec!["ppp"]);
    }

    #[test]
    fn apk_installed_database_detects_package_set_and_protocols() {
        let database = "C:Q1abc\nP:ppp\nV:2.5.0-r1\n\nC:Q1def\nP:ppp-mod-pppoe\nV:2.5.0-r1\n\nP:wireguard-tools\nV:1.0-r1\n";
        let packages = parse_apk_packages(database).unwrap();
        assert_eq!(packages, vec!["ppp", "ppp-mod-pppoe", "wireguard-tools"]);
        assert_eq!(protocols_for(&packages), vec!["ppp", "pppoe", "wireguard"]);
    }

    #[test]
    fn no_readable_package_database_produces_no_inventory() {
        let detected = select_package_database(None, None);
        assert!(detected.is_none());
        assert!(protocols_for(&detected.unwrap_or_default()).is_empty());
    }

    #[test]
    fn malformed_apk_database_uses_opkg_fallback() {
        let opkg = "Package: ppp-mod-pppoe\nStatus: install user installed\n";
        let packages = select_package_database(Some("not an apk db"), Some(opkg)).unwrap();
        assert_eq!(protocols_for(&packages), vec!["pppoe"]);
    }

    #[tokio::test]
    async fn missing_fixture_probes_do_not_advertise_actions() {
        let ubus = crate::ubus::Ubus::fixture(std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/mainrouter"
        )));
        let capabilities = super::discover(&ubus, false, false).await;
        assert!(capabilities.actions.is_empty());
        assert!(capabilities.packages.is_empty());
        assert!(capabilities.services.is_empty());
        assert!(
            capabilities
                .ubus_objects
                .iter()
                .all(|object| object.methods.is_empty())
        );
    }
}
