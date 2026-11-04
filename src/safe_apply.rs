//! Shared UCI transaction flow for connectivity-sensitive configuration changes.
//!
//! Stage changes, capture rpcd's UCI diff, arm rpcd rollback, probe reachability, then confirm.
//! If the probe fails, leave the rollback timer armed. rpcd owns the actual timeout and revert.

use std::{future::Future, sync::OnceLock, time::Duration};

use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::ubus::Ubus;

static TRANSACTION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    NoChanges,
    Confirmed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub state: State,
    /// Raw rpcd `uci changes` response. Treat values as secrets; redact before display or audit.
    pub diff: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Stage(String),
    Diff(String),
    Apply(String),
    RollbackArmed { diff: Value },
    Confirm(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stage(e) => write!(f, "UCI staging failed: {e}"),
            Self::Diff(e) => write!(f, "UCI diff failed: {e}"),
            Self::Apply(e) => write!(f, "UCI rollback-armed apply failed: {e}"),
            Self::RollbackArmed { .. } => write!(
                f,
                "node unreachable after apply; rpcd rollback remains armed"
            ),
            Self::Confirm(e) => write!(f, "UCI confirm failed; rpcd rollback remains armed: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Execute one serialized UCI transaction. `stage` must use this same session-scoped handle.
/// The lock prevents this framework's own concurrent callers from mixing staged UCI deltas.
pub async fn run<S, SFut, R, RFut>(
    ubus: &Ubus,
    timeout_secs: u32,
    settle: Duration,
    stage: S,
    is_reachable: R,
) -> Result<Report, Error>
where
    S: FnOnce() -> SFut,
    SFut: Future<Output = Result<(), String>>,
    R: FnOnce() -> RFut,
    RFut: Future<Output = bool>,
{
    let _guard = TRANSACTION_LOCK.get_or_init(|| Mutex::new(())).lock().await;

    if let Err(error) = stage().await {
        let _ = ubus.call("uci", "revert", json!({})).await;
        return Err(Error::Stage(error));
    }

    let diff = match ubus.call("uci", "changes", json!({})).await {
        Ok(diff) => diff,
        Err(error) => {
            let _ = ubus.call("uci", "revert", json!({})).await;
            return Err(Error::Diff(error));
        }
    };
    let has_changes = diff.get("changes").is_some_and(|changes| match changes {
        Value::Array(items) => !items.is_empty(),
        Value::Object(items) => !items.is_empty(),
        _ => false,
    });
    if !has_changes {
        return Ok(Report {
            state: State::NoChanges,
            diff,
        });
    }

    if let Err(error) = ubus
        .call(
            "uci",
            "apply",
            json!({ "rollback": true, "timeout": timeout_secs }),
        )
        .await
    {
        let _ = ubus.call("uci", "revert", json!({})).await;
        return Err(Error::Apply(error));
    }

    tokio::time::sleep(settle).await;
    if !is_reachable().await {
        return Err(Error::RollbackArmed { diff });
    }

    ubus.call("uci", "confirm", json!({}))
        .await
        .map_err(Error::Confirm)?;
    Ok(Report {
        state: State::Confirmed,
        diff,
    })
}

/// Default reachability probe using a fresh request on the same authenticated session.
pub async fn board_reachable(ubus: &Ubus) -> bool {
    ubus.call("system", "board", json!({})).await.is_ok()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::*;

    fn fixture_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "openwrtfi-safe-apply-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture(dir: &Path, file: &str, contents: &str) {
        fs::write(dir.join(file), contents).unwrap();
    }

    async fn run_test(ubus: &Ubus, reachable: bool) -> Result<Report, Error> {
        run(
            ubus,
            30,
            Duration::ZERO,
            || async { Ok(()) },
            || async move { reachable },
        )
        .await
    }

    #[tokio::test]
    async fn confirms_only_after_diff_apply_and_reachability() {
        let dir = fixture_dir("confirmed");
        fixture(
            &dir,
            "uci.changes.json",
            r#"{"changes":{"network":[["set","lan","ipaddr","192.0.2.1"]]}}"#,
        );
        fixture(&dir, "uci.apply.json", "{}");
        fixture(&dir, "system.board.json", "{}");
        fixture(&dir, "uci.confirm.json", "{}");
        let result = run_test(&Ubus::fixture(dir.clone()), true).await.unwrap();
        assert_eq!(result.state, State::Confirmed);
        assert_eq!(result.diff["changes"]["network"][0][0], "set");
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn empty_diff_does_not_apply_or_confirm() {
        let dir = fixture_dir("empty");
        fixture(&dir, "uci.changes.json", r#"{"changes":{}}"#);
        let result = run_test(&Ubus::fixture(dir.clone()), true).await.unwrap();
        assert_eq!(result.state, State::NoChanges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn unreachable_node_keeps_rpcd_rollback_armed() {
        let dir = fixture_dir("unreachable");
        fixture(
            &dir,
            "uci.changes.json",
            r#"{"changes":{"wireless":[["set","radio0","channel","6"]]}}"#,
        );
        fixture(&dir, "uci.apply.json", "{}");
        assert!(matches!(
            run_test(&Ubus::fixture(dir.clone()), false).await,
            Err(Error::RollbackArmed { .. })
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn stage_failure_stops_transaction_and_reverts_staged_delta() {
        let dir = fixture_dir("stage-failure");
        fixture(&dir, "uci.revert.json", "{}");
        let result = run(
            &Ubus::fixture(dir.clone()),
            30,
            Duration::ZERO,
            || async { Err("permission denied".to_owned()) },
            || async { true },
        )
        .await;
        assert_eq!(result, Err(Error::Stage("permission denied".into())));
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn diff_denial_stops_before_apply_and_reverts_staged_delta() {
        let dir = fixture_dir("diff-denied");
        fixture(&dir, "uci.revert.json", "{}");
        let result = run_test(&Ubus::fixture(dir.clone()), true).await;
        assert!(matches!(result, Err(Error::Diff(_))));
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn apply_or_confirm_denial_never_reports_success() {
        let apply_dir = fixture_dir("apply-denied");
        fixture(
            &apply_dir,
            "uci.changes.json",
            r#"{"changes":{"dhcp":[["set","lan","ignore","1"]]}}"#,
        );
        fixture(&apply_dir, "uci.revert.json", "{}");
        assert!(matches!(
            run_test(&Ubus::fixture(apply_dir.clone()), true).await,
            Err(Error::Apply(_))
        ));
        fs::remove_dir_all(apply_dir).unwrap();

        let confirm_dir = fixture_dir("confirm-denied");
        fixture(
            &confirm_dir,
            "uci.changes.json",
            r#"{"changes":{"dhcp":[["set","lan","ignore","1"]]}}"#,
        );
        fixture(&confirm_dir, "uci.apply.json", "{}");
        fixture(&confirm_dir, "system.board.json", "{}");
        assert!(matches!(
            run_test(&Ubus::fixture(confirm_dir.clone()), true).await,
            Err(Error::Confirm(_))
        ));
        fs::remove_dir_all(confirm_dir).unwrap();
    }
}
