//! Read compatibility wrapper for collectors plus the existing rollback-safe apply helper.
//!
//! Generic UCI CRUD/order/type validation lives in `crate::uci_engine`; collectors keep this
//! narrow wrapper so existing read paths can continue treating an unavailable config as empty.

use crate::{ubus::Ubus, uci_engine};

pub use crate::uci_engine::Section;

/// All sections of a config, in the order the daemon returned them (uci preserves file order).
pub async fn get(ubus: &Ubus, config: &str) -> Vec<Section> {
    match uci_engine::read(ubus, config).await {
        Ok(sections) => sections,
        Err(e) => {
            log::warn!("uci read {config}: {e}; using empty configuration");
            Vec::new()
        }
    }
}
