//! Reusable, validated UCI operations over the caller's session-scoped ubus handle.
//!
//! This module deliberately does not apply or confirm changes. Callers own the transaction
//! policy (including rpcd rollback) after composing these primitives.

use std::{
    collections::{BTreeMap, HashSet},
    fmt,
};

use serde_json::{Map, Value, json};

use crate::ubus::Ubus;

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // List values are part of the generic engine contract; current pages use scalars.
pub enum UciScalar {
    String(String),
    Bool(bool),
    Integer(i64),
}

impl UciScalar {
    fn to_wire(&self) -> Value {
        match self {
            Self::String(value) => Value::String(value.clone()),
            Self::Bool(value) => Value::String(if *value { "1" } else { "0" }.into()),
            Self::Integer(value) => Value::String(value.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UciValue {
    String(String),
    Bool(bool),
    Integer(i64),
    #[allow(dead_code)] // Used by upcoming UCI-backed feature pages; kept available now by design.
    List(Vec<UciScalar>),
}

impl UciValue {
    /// Convert to ubus UCI's value shape. Scalars become strings; lists stay arrays because
    /// UCI represents repeated options as list values.
    pub fn to_wire(&self) -> Value {
        match self {
            Self::String(value) => Value::String(value.clone()),
            Self::Bool(value) => Value::String(if *value { "1" } else { "0" }.into()),
            Self::Integer(value) => Value::String(value.to_string()),
            Self::List(values) => Value::Array(values.iter().map(UciScalar::to_wire).collect()),
        }
    }

    pub fn as_text(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Bool(value) => if *value { "1" } else { "0" }.into(),
            Self::Integer(value) => value.to_string(),
            Self::List(values) => {
                Value::Array(values.iter().map(UciScalar::to_wire).collect()).to_string()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub kind: String,
    pub anonymous: bool,
    pub index: Option<i64>,
    pub options: BTreeMap<String, Value>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.options.get(key).and_then(Value::as_str)
    }

    pub fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some("1" | "on" | "true" | "yes"))
    }

    pub fn parse<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.get(key).and_then(|value| value.parse().ok())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidIdentifier {
        field: &'static str,
        value: String,
    },
    #[allow(dead_code)]
    // Currently reached by ordering validation, whose first UI consumer is later.
    InvalidValue(String),
    Transport(String),
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier { field, value } => {
                write!(f, "invalid UCI {field} identifier {value:?}")
            }
            Self::InvalidValue(message) => write!(f, "invalid UCI value: {message}"),
            Self::Transport(message) => write!(f, "ubus UCI call failed: {message}"),
            Self::Protocol(message) => write!(f, "invalid ubus UCI response: {message}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn plain_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn section_name(value: &str) -> bool {
    if plain_name(value) {
        return true;
    }
    let Some(rest) = value.strip_prefix('@') else {
        return false;
    };
    let Some((kind, index)) = rest.split_once('[') else {
        return false;
    };
    let Some(index) = index.strip_suffix(']') else {
        return false;
    };
    type_name(kind) && !index.is_empty() && index.parse::<i64>().is_ok()
}

fn type_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn identifier(field: &'static str, value: &str) -> Result<()> {
    let valid = match field {
        "type" => type_name(value),
        "section" => section_name(value),
        "config" | "name" | "option" => plain_name(value),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidIdentifier {
            field,
            value: value.into(),
        })
    }
}

/// Validate an option name before a caller builds an edit preview.
pub fn validate_option_name(value: &str) -> Result<()> {
    identifier("option", value)
}

fn options(values: &BTreeMap<String, UciValue>) -> Result<Map<String, Value>> {
    let mut wire = Map::new();
    for (key, value) in values {
        identifier("option", key)?;
        wire.insert(key.clone(), value.to_wire());
    }
    Ok(wire)
}

fn transport<T>(result: crate::ubus::Result<T>) -> Result<T> {
    result.map_err(Error::Transport)
}

/// Read a config, retaining UCI section order and list-valued options.
pub async fn read(ubus: &Ubus, config: &str) -> Result<Vec<Section>> {
    identifier("config", config)?;
    let reply = transport(ubus.call("uci", "get", json!({"config": config})).await)?;
    let values = reply
        .get("values")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Protocol("get response has no values object".into()))?;
    let mut sections = Vec::with_capacity(values.len());
    for (name, raw) in values {
        if identifier("section", name).is_err() {
            continue;
        }
        let Some(fields) = raw.as_object() else {
            continue;
        };
        let Some(kind) = fields.get(".type").and_then(Value::as_str) else {
            continue;
        };
        if identifier("type", kind).is_err() {
            continue;
        }
        let anonymous = fields
            .get(".anonymous")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let index = fields.get(".index").and_then(Value::as_i64);
        let mut section_options = BTreeMap::new();
        for (key, value) in fields {
            if key.starts_with('.') {
                continue;
            }
            if identifier("option", key).is_err() {
                continue;
            }
            section_options.insert(key.clone(), value.clone());
        }
        sections.push(Section {
            name: name.clone(),
            kind: kind.into(),
            anonymous,
            index,
            options: section_options,
        });
    }
    sections.sort_by(|a, b| a.index.cmp(&b.index).then_with(|| a.name.cmp(&b.name)));
    Ok(sections)
}

/// Create a section and return its assigned name.
pub async fn create(
    ubus: &Ubus,
    config: &str,
    kind: &str,
    name: Option<&str>,
    values: &BTreeMap<String, UciValue>,
) -> Result<String> {
    identifier("config", config)?;
    identifier("type", kind)?;
    if let Some(name) = name {
        identifier("name", name)?;
    }
    let mut args = json!({"config": config, "type": kind, "values": options(values)?});
    if let Some(name) = name {
        args["name"] = Value::String(name.into());
    }
    let reply = transport(ubus.call("uci", "add", args).await)?;
    let name = reply
        .get("section")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Protocol("add response has no section name".into()))?;
    identifier("section", name)?;
    Ok(name.into())
}

/// Set or replace options on an existing section.
pub async fn update(
    ubus: &Ubus,
    config: &str,
    section: &str,
    values: &BTreeMap<String, UciValue>,
) -> Result<()> {
    identifier("config", config)?;
    identifier("section", section)?;
    let values = options(values)?;
    transport(
        ubus.call(
            "uci",
            "set",
            json!({"config": config, "section": section, "values": values}),
        )
        .await,
    )?;
    Ok(())
}

/// Delete a whole section, or one option when `option` is supplied.
#[allow(dead_code)] // Core API required by the project item; no current page deletes UCI state yet.
pub async fn delete(ubus: &Ubus, config: &str, section: &str, option: Option<&str>) -> Result<()> {
    identifier("config", config)?;
    identifier("section", section)?;
    if let Some(option) = option {
        identifier("option", option)?;
    }
    let mut args = json!({"config": config, "section": section});
    if let Some(option) = option {
        args["option"] = Value::String(option.into());
    }
    transport(ubus.call("uci", "delete", args).await)?;
    Ok(())
}

/// Apply an explicit section order. rpcd moves the listed sections to positions 0..N-1.
#[allow(dead_code)] // Core API required by the project item; ordered editors are follow-up cards.
pub async fn order(ubus: &Ubus, config: &str, sections: &[String]) -> Result<()> {
    identifier("config", config)?;
    if sections.is_empty() {
        return Err(Error::InvalidValue(
            "order requires at least one section".into(),
        ));
    }
    let mut seen = HashSet::new();
    for section in sections {
        identifier("section", section)?;
        if !seen.insert(section.as_str()) {
            return Err(Error::InvalidValue(format!(
                "order contains duplicate section {section:?}"
            )));
        }
    }
    transport(
        ubus.call(
            "uci",
            "order",
            json!({"config": config, "sections": sections}),
        )
        .await,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::*;

    fn fixture_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "openwrtfi-uci-engine-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn typed_values_use_uci_wire_conventions() {
        let values = UciValue::List(vec![
            UciScalar::String("lan".into()),
            UciScalar::Bool(false),
            UciScalar::Integer(-7),
        ]);
        assert_eq!(values.to_wire(), json!(["lan", "0", "-7"]));
        assert_eq!(UciValue::Bool(true).to_wire(), json!("1"));
        assert_eq!(UciValue::Integer(42).to_wire(), json!("42"));
    }

    #[test]
    fn identifiers_reject_injection_and_reserved_option_names() {
        assert!(identifier("config", "wireless_2").is_ok());
        assert!(identifier("name", "_guest").is_ok());
        assert!(identifier("type", "wifi-iface").is_ok());
        assert!(identifier("section", "@wifi-iface[0]").is_ok());
        for invalid in ["", ".", "../wireless", "wireless.foo", "a b", "-start"] {
            assert!(identifier("config", invalid).is_err(), "{invalid:?}");
        }
        assert!(identifier("option", "bad-key").is_err());
        assert!(identifier("option", ".type").is_err());
    }

    #[test]
    fn option_conversion_validates_every_key() {
        let mut values = BTreeMap::new();
        values.insert("ssid".into(), UciValue::String("home".into()));
        assert_eq!(
            Value::Object(options(&values).unwrap()),
            json!({"ssid": "home"})
        );
        values.insert("bad.key".into(), UciValue::Bool(true));
        assert!(options(&values).is_err());
    }

    #[tokio::test]
    async fn crud_and_order_use_rpcd_method_names() {
        let dir = fixture_dir("methods");
        fs::write(dir.join("uci.add.network.json"), r#"{"section":"cfg01"}"#).unwrap();
        fs::write(dir.join("uci.set.network.json"), "{}").unwrap();
        fs::write(dir.join("uci.delete.network.json"), "{}").unwrap();
        fs::write(dir.join("uci.order.network.json"), "{}").unwrap();
        let ubus = Ubus::fixture(dir.clone());

        let mut values = BTreeMap::new();
        values.insert("proto".into(), UciValue::String("static".into()));
        assert_eq!(
            create(&ubus, "network", "interface", None, &values)
                .await
                .unwrap(),
            "cfg01"
        );
        update(&ubus, "network", "lan", &values).await.unwrap();
        delete(&ubus, "network", "lan", Some("proto"))
            .await
            .unwrap();
        order(&ubus, "network", &["lan".into(), "wan".into()])
            .await
            .unwrap();

        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn read_skips_invalid_sections_and_options_without_losing_valid_data() {
        let dir = fixture_dir("tolerant-read");
        fs::write(
            dir.join("uci.get.wireless.json"),
            r#"{
                "values": {
                    "radio0": {
                        ".type": "wifi-device",
                        ".index": 0,
                        "channel": "auto",
                        "bad.option": "ignored"
                    },
                    "bad.section": {
                        ".type": "wifi-device",
                        ".index": 1
                    },
                    "missing_type": {
                        ".index": 2,
                        "ssid": "ignored"
                    },
                    "not_object": "ignored",
                    "default_radio0": {
                        ".type": "wifi-iface",
                        ".index": 3,
                        "ssid": "Home"
                    }
                }
            }"#,
        )
        .unwrap();
        let ubus = Ubus::fixture(dir.clone());

        let sections = read(&ubus, "wireless").await.unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "radio0");
        assert_eq!(sections[0].get("channel"), Some("auto"));
        assert!(!sections[0].options.contains_key("bad.option"));
        assert_eq!(sections[1].name, "default_radio0");
        assert_eq!(sections[1].get("ssid"), Some("Home"));

        fs::remove_dir_all(dir).unwrap();
    }
}
