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
    /// Format a transaction failure without exposing the raw UCI diff.
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
    staged_configs: &[&str],
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
    if staged_configs.is_empty() || staged_configs.iter().any(|config| config.is_empty()) {
        return Err(Error::Stage("no staged UCI config names provided".into()));
    }

    if let Err(error) = stage().await {
        revert_staged(ubus, staged_configs).await;
        return Err(Error::Stage(error));
    }

    let diff = match ubus.call("uci", "changes", json!({})).await {
        Ok(diff) => diff,
        Err(error) => {
            revert_staged(ubus, staged_configs).await;
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
        revert_staged(ubus, staged_configs).await;
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

/// Revert every config with a staged delta on a transaction failure.
///
/// rpcd's `uci revert` requires `config`; without it the call is rejected and the staged delta
/// stays in the session, where the next transaction's `uci changes` reports it and its `uci apply`
/// commits it. The declared configs are always reverted — even when `uci changes` cannot be read —
/// and any additional staged config rpcd reports is reverted too, so no leftover delta survives.
async fn revert_staged(ubus: &Ubus, declared: &[&str]) {
    let staged = ubus.call("uci", "changes", json!({})).await.ok();
    for config in revert_targets(declared, staged.as_ref()) {
        let _ = ubus
            .call("uci", "revert", json!({ "config": config }))
            .await;
    }
}

/// Union of the caller's declared configs and the configs rpcd still reports as staged.
fn revert_targets(declared: &[&str], staged: Option<&Value>) -> Vec<String> {
    let mut targets: Vec<String> = declared.iter().map(|config| (*config).to_owned()).collect();
    let reported = staged
        .and_then(|diff| diff.get("changes"))
        .and_then(Value::as_object);
    if let Some(reported) = reported {
        for config in reported.keys() {
            if !targets.iter().any(|target| target == config) {
                targets.push(config.clone());
            }
        }
    }
    targets
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

    /// Create a clean temporary fixture directory for a transaction test.
    fn fixture_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "openwrtfi-safe-apply-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write one rpcd response fixture into the transaction test directory.
    fn fixture(dir: &Path, file: &str, contents: &str) {
        fs::write(dir.join(file), contents).unwrap();
    }

    /// Run a transaction with an empty staging closure and chosen reachability result.
    async fn run_test(ubus: &Ubus, config: &str, reachable: bool) -> Result<Report, Error> {
        run(
            ubus,
            &[config],
            30,
            Duration::ZERO,
            || async { Ok(()) },
            || async move { reachable },
        )
        .await
    }

    #[tokio::test]
    /// Checks apply and confirm happen only after a nonempty diff and successful reachability.
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
        let result = run_test(&Ubus::fixture(dir.clone()), "network", true)
            .await
            .unwrap();
        assert_eq!(result.state, State::Confirmed);
        assert_eq!(result.diff["changes"]["network"][0][0], "set");
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    /// Checks an empty diff skips apply and confirm calls.
    async fn empty_diff_does_not_apply_or_confirm() {
        let dir = fixture_dir("empty");
        fixture(&dir, "uci.changes.json", r#"{"changes":{}}"#);
        let result = run_test(&Ubus::fixture(dir.clone()), "network", true)
            .await
            .unwrap();
        assert_eq!(result.state, State::NoChanges);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    /// Checks an unreachable node leaves rpcd rollback armed.
    async fn unreachable_node_keeps_rpcd_rollback_armed() {
        let dir = fixture_dir("unreachable");
        fixture(
            &dir,
            "uci.changes.json",
            r#"{"changes":{"wireless":[["set","radio0","channel","6"]]}}"#,
        );
        fixture(&dir, "uci.apply.json", "{}");
        assert!(matches!(
            run_test(&Ubus::fixture(dir.clone()), "wireless", false).await,
            Err(Error::RollbackArmed { .. })
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    /// Checks staging errors stop the transaction and revert staged changes.
    async fn stage_failure_stops_transaction_and_reverts_staged_delta() {
        let dir = fixture_dir("stage-failure");
        fixture(&dir, "uci.revert.network.json", "{}");
        fixture(&dir, "uci.revert.wireless.json", "{}");
        let result = run(
            &Ubus::fixture(dir.clone()),
            &["network", "wireless"],
            30,
            Duration::ZERO,
            || async { Err("permission denied".to_owned()) },
            || async { true },
        )
        .await;
        assert_eq!(result, Err(Error::Stage("permission denied".into())));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    /// Checks cleanup reverts the declared configs plus any stale staged config rpcd reports.
    fn revert_targets_cover_declared_and_stale_staged_configs() {
        let staged = json!({
            "changes": {
                "wireless": [["set", "radio0", "channel", "6"]],
                "network": [],
            }
        });
        // Declared configs come first, then anything else rpcd still reports as staged.
        assert_eq!(
            revert_targets(&["wireless"], Some(&staged)),
            vec!["wireless".to_owned(), "network".to_owned()]
        );
        // An unreadable `uci changes` still reverts the declared configs.
        assert_eq!(revert_targets(&["dhcp"], None), vec!["dhcp".to_owned()]);
        assert!(revert_targets(&[], Some(&json!({}))).is_empty());
    }

    #[tokio::test]
    /// Checks invalid config names are rejected before staging begins.
    async fn missing_config_names_do_not_start_staging() {
        let dir = fixture_dir("missing-config-names");
        let stage_started = std::sync::atomic::AtomicBool::new(false);
        let result = run(
            &Ubus::fixture(dir.clone()),
            &[],
            30,
            Duration::ZERO,
            || async {
                stage_started.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
            || async { true },
        )
        .await;
        assert!(matches!(result, Err(Error::Stage(_))));
        assert!(!stage_started.load(std::sync::atomic::Ordering::SeqCst));
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    /// Checks a denied diff request stops before apply and reverts staged changes.
    async fn diff_denial_stops_before_apply_and_reverts_staged_delta() {
        let dir = fixture_dir("diff-denied");
        fixture(&dir, "uci.revert.network.json", "{}");
        let result = run_test(&Ubus::fixture(dir.clone()), "network", true).await;
        assert!(matches!(result, Err(Error::Diff(_))));
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    /// Checks apply and confirm denials are returned as transaction failures.
    async fn apply_or_confirm_denial_never_reports_success() {
        let apply_dir = fixture_dir("apply-denied");
        fixture(
            &apply_dir,
            "uci.changes.json",
            r#"{"changes":{"dhcp":[["set","lan","ignore","1"]]}}"#,
        );
        fixture(&apply_dir, "uci.revert.dhcp.json", "{}");
        assert!(matches!(
            run_test(&Ubus::fixture(apply_dir.clone()), "dhcp", true).await,
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
            run_test(&Ubus::fixture(confirm_dir.clone()), "dhcp", true).await,
            Err(Error::Confirm(_))
        ));
        fs::remove_dir_all(confirm_dir).unwrap();
    }
}
