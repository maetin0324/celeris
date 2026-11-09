//! ADR 2026-11-08-health-enforcement-loop D1.2・D2.2・D3.2・D3.4: WU の check と段の統合が必須検査の列に足す health の
//! 項目（`cmd` は [`HEALTH_CHECK_ID`]）。
//!
//! 両方の経路は `dispatcher::phase_integration::run_integration_checks` を通り、そこがこの module の [`evaluate`] を
//! 1 回呼ぶ（経路ごとに検査器を起こさない）。[`evaluate`] は経路共通の入口 [`gate::health_gate`] を呼び、扱い
//! （[`HealthGateDecision`]）を検査の列の 1 行（`pass`・`reason`）に写す。写しの規則はここだけに置く:
//!
//! - Pass・PolicyOnly: 合格（PolicyOnly は WU・段の統合では先へ進める。D3.1）。
//! - Repair: 不合格。reason は rule ごとの `health:<rule_id>:<hash>[,…]（evidence: …）` を `; ` で並べた文で、
//!   `task_core::classify_review_failure` が `RepairClass::Health` に分ける（D3.2）。class の数え方は
//!   [`repair_buckets`]（`health:<rule_id>`）。
//! - Error（Unavailable を含む）: 不合格。reason は `health_check_error: <理由>`（[`check_error`] が読む）。repair に回さず、
//!   経路は task を Blocked にする（D3.4）。
//! - NotApplicable: 列に足さない（event には理由を持つ証跡を残す）。
//!
//! 修復の後の再評価は、経路が新しい head で [`evaluate`] を呼び直す（前の証跡を使わない。ここは何も覚えない）。

use std::path::PathBuf;

use task_core::RepoId;
use task_core::required_checks::health::{HEALTH_CHECK_ID, HealthEvidence};

use super::gate::{self, BundledChecker, HealthGateDecision, HealthGateRequest};
use super::input::HEALTH_INPUT_TIMEOUT;

/// D3.4: Error・Unavailable の項目の reason の先頭の語（`health_check_error: <理由>`）。
pub const HEALTH_CHECK_ERROR: &str = "health_check_error";

/// health を評価する 1 つの repo の作業ツリーと範囲（経路が組み立てる。D1.2 の表）。
#[derive(Debug, Clone)]
pub struct HealthTarget {
    pub repo_id: RepoId,
    /// git と検査器を実行する作業ツリー（WU は WU の作業ツリー、段の統合は統合後の Task の作業ツリー）。
    pub worktree: PathBuf,
    /// WU は `wu.base_commit`、段の統合は段の基点。
    pub base: String,
    /// WU は WU の HEAD、段の統合は統合の HEAD（証跡には解決した sha が残る）。
    pub head: String,
    /// release に同梱した検査器（経路の外側で読んだ値）。
    pub bundled: Option<BundledChecker>,
}

/// 検査の列に足す health の 1 行と、event に残す証跡。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthItem {
    /// 列に足すか（NotApplicable は `false`。event には `evidence` を残す）。
    pub counted: bool,
    pub pass: bool,
    /// Pass は `Some(0)`、Repair は `Some(1)`、Error と NotApplicable は `None`。
    pub exit: Option<i32>,
    pub reason: String,
    pub evidence: HealthEvidence,
}

/// D1.2: `target` を経路共通の入口で 1 回評価し、列の 1 行に写す（blocking。git と検査器の子 process を起こす）。
pub fn evaluate(target: &HealthTarget) -> HealthItem {
    let outcome = gate::health_gate(&HealthGateRequest {
        repo_id: target.repo_id,
        worktree: &target.worktree,
        base: &target.base,
        head: &target.head,
        integrated_by_celeris: true,
        bundled: target.bundled.as_ref(),
        timeout: HEALTH_INPUT_TIMEOUT,
    });
    let evidence = outcome.evidence().clone();
    let (counted, pass, exit, reason) = match outcome.decision() {
        HealthGateDecision::NotApplicable => (
            false,
            true,
            None,
            format!(
                "health: not applicable: {}",
                evidence.reason.as_deref().unwrap_or_default()
            ),
        ),
        HealthGateDecision::Pass => (true, true, Some(0), "health: pass".to_string()),
        HealthGateDecision::PolicyOnly => (
            true,
            true,
            Some(0),
            format!(
                "health: {}",
                evidence.reason.as_deref().unwrap_or("policy_only")
            ),
        ),
        HealthGateDecision::Repair(repairs) => (
            true,
            false,
            Some(1),
            repairs
                .iter()
                .map(|r| r.reason.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ),
        HealthGateDecision::Error { reason } => {
            (true, false, None, format!("{HEALTH_CHECK_ERROR}: {reason}"))
        }
    };
    HealthItem {
        counted,
        pass,
        exit,
        reason,
        evidence,
    }
}

/// 列の 1 行が health の Error（D3.4）なら、その reason（`health_check_error: <理由>`）。
pub fn check_error<'a>(cmd: &str, pass: bool, reason: &'a str) -> Option<&'a str> {
    let rest = reason.strip_prefix(HEALTH_CHECK_ERROR)?;
    (cmd == HEALTH_CHECK_ID && !pass && rest.starts_with(':')).then_some(reason)
}

/// D3.2・D4.1: health の Repair の reason（[`evaluate`] が作る）から、`max_repairs_per_class` を数える rule ごとの
/// bucket（`health:<rule_id>`、重複なし・順は reason のまま）。health の Repair でなければ空。
pub fn repair_buckets(reason: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in reason.split("; ") {
        let Some(item) = part.strip_prefix(task_core::execution::HEALTH_REPAIR_REASON_PREFIX)
        else {
            continue;
        };
        let first = item.split([',', '（']).next().unwrap_or_default();
        if let Some((rule_id, _hash)) = first.rsplit_once(':') {
            let bucket = format!("health:{rule_id}");
            if !out.contains(&bucket) {
                out.push(bucket);
            }
        }
    }
    out
}

/// WU の check の列に足す health の項目の check（`WorkUnitCheckFinished` の cmd と不合格の記録に使う）。
pub fn work_unit_check() -> task_core::WorkUnitCheck {
    task_core::WorkUnitCheck {
        cmd: HEALTH_CHECK_ID.to_string(),
        expect_exit: 0,
        scope: false,
    }
}

/// `dispatcher::phase_integration::run_integration_checks` の結果。`health` は列に足す health の行（`(pass, reason)`。NotApplicable・対象外は `None`）。
pub(crate) struct IntegrationChecks {
    pub results: Vec<(bool, String)>,
    pub health: Option<(bool, String)>,
}

/// health の項目を 1 回評価し、開始・終了の event（`health` 欄に証跡）を残す（`(work_unit_id, key, run_id)` の
/// `run_id` が `Some` なら葉の WU の check の event、`None` なら段の統合の event）。
pub(crate) async fn run_item(
    store: &dyn task_core::TaskStore,
    task_id: task_core::TaskId,
    (work_unit_id, key, run_id): (&str, &str, Option<&str>),
    target: HealthTarget,
    index: u32,
    total: u32,
) -> Option<(bool, String)> {
    let cmd = task_core::required_checks::health::HEALTH_CHECK_ID.to_string();
    let started_at = now_rfc3339();
    let started = match run_id {
        Some(run_id) => task_core::Event::WorkUnitCheckStarted {
            work_unit_id: work_unit_id.to_string(),
            key: key.to_string(),
            run_id: run_id.to_string(),
            index,
            total,
            cmd: cmd.clone(),
            log_path: String::new(),
            started_at,
        },
        None => task_core::Event::IntegrationCheckStarted {
            work_unit_id: work_unit_id.to_string(),
            key: key.to_string(),
            index,
            total,
            cmd: cmd.clone(),
            log_path: String::new(),
            started_at,
        },
    };
    if let Err(e) = store.append_event(task_id, &started) {
        tracing::warn!(%task_id, work_unit = %key, error = %e, "could not record the health check start");
    }
    let clock = std::time::Instant::now();
    let item = match tokio::task::spawn_blocking(move || evaluate(&target)).await {
        Ok(item) => item,
        // 評価の thread が落ちた: 合格にせず Error として止める（D3.4）。
        Err(e) => {
            let reason = format!("{HEALTH_CHECK_ERROR}: health evaluation task failed: {e}");
            return Some((false, reason));
        }
    };
    let duration_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
    let evidence = Some(Box::new(item.evidence.clone()));
    let finished = match run_id {
        Some(run_id) => task_core::Event::WorkUnitCheckFinished {
            work_unit_id: work_unit_id.to_string(),
            key: key.to_string(),
            run_id: run_id.to_string(),
            index,
            total,
            cmd,
            pass: item.pass,
            exit: item.exit,
            timed_out: false,
            duration_ms,
            health: evidence,
        },
        None => task_core::Event::IntegrationCheckFinished {
            work_unit_id: work_unit_id.to_string(),
            key: key.to_string(),
            index,
            total,
            cmd,
            pass: item.pass,
            exit: item.exit,
            timed_out: false,
            duration_ms,
            health: evidence,
        },
    };
    if let Err(e) = store.append_event(task_id, &finished) {
        tracing::warn!(%task_id, work_unit = %key, error = %e, "could not record the health check finish");
    }
    item.counted.then_some((item.pass, item.reason))
}

/// ADR 2026-11-08-health-enforcement-loop D1.2: 段 `phase` の基点（health の base）。その段の葉・子の unit のうち
/// 最も前（`seq`）の unit が切られた commit（`base_commit`）。段の unit は段の始まりの Task のブランチから切られる
/// ので、統合と修復を繰り返しても同じ基点になる。どの unit も基点を持たなければ `None`。
fn phase_base_commit(units: &[task_core::WorkUnitRow], phase: &str) -> Option<String> {
    units
        .iter()
        .filter(|u| {
            u.phase.as_deref() == Some(phase)
                && !matches!(
                    u.kind,
                    task_core::WorkUnitKind::Integrate | task_core::WorkUnitKind::Repair
                )
        })
        .filter_map(|u| u.base_commit.clone().map(|base| (u.seq, base)))
        .min_by_key(|(seq, _)| *seq)
        .map(|(_, base)| base)
}

/// `dir` の HEAD（統合の前。段の unit が基点を持たないときの health の base）。読めなければ `HEAD`（評価で base を解決
/// できなければ証跡は Error になり、合格にしない）。
fn git_head(dir: &std::path::Path) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "HEAD".to_string())
}

/// health の証跡の repo の id。案件の repo の参照（`task.repos` の同じ名前）があればその id。参照の無い作業場所
/// （task の `repos` が空の単一の作業場所）は固定の id（全桁 0）にする（作業場所は 1 つなので取り違えない）。
pub(crate) fn health_repo_id(repo_id: Option<task_core::RepoId>) -> task_core::RepoId {
    repo_id.unwrap_or(task_core::RepoId(ulid::Ulid::nil()))
}

/// 段の統合の health の `(作業ツリー, repo の id, base)`（base は段の基点、無ければ統合の前の HEAD）。
pub(crate) fn phase_base(
    units: &[task_core::WorkUnitRow],
    phase: &str,
    dir: &std::path::Path,
    repo_id: Option<RepoId>,
) -> (PathBuf, RepoId, String) {
    let base = phase_base_commit(units, phase).unwrap_or_else(|| git_head(dir));
    (dir.to_path_buf(), health_repo_id(repo_id), base)
}

/// 段の統合の health の対象（[`phase_base`] の `(作業ツリー, repo の id, base)` と統合の HEAD。空なら `HEAD`）。
pub(crate) fn at_head(
    (worktree, repo_id, base): (PathBuf, RepoId, String),
    head: &str,
    bundled: Option<BundledChecker>,
) -> HealthTarget {
    let head = if head.is_empty() { "HEAD" } else { head };
    HealthTarget {
        repo_id,
        worktree,
        base,
        head: head.to_string(),
        bundled,
    }
}

/// WU の check の health の対象（base は `wu.base_commit`、head は WU の作業ツリー `worktree` の HEAD）。基点の無い WU
/// （Task の作業ツリーで走る repair など）は `None`（段の統合で評価する）。repo の id は作業場所の先頭の git の repo の参照。
pub(crate) fn work_unit_target(
    task: &task_core::Task,
    workspaces: Option<task_worker::TaskWorkspaces>,
    base_commit: Option<&str>,
    worktree: &std::path::Path,
) -> Option<HealthTarget> {
    let repo_id = workspaces.and_then(|ws| {
        let repo = ws.repos.into_iter().find(|r| r.is_git())?;
        let reference = task.repos.iter().find(|r| r.name == repo.name)?;
        Some(reference.repo_id)
    });
    Some(HealthTarget {
        repo_id: health_repo_id(repo_id),
        worktree: worktree.to_path_buf(),
        base: base_commit?.to_string(),
        head: "HEAD".to_string(),
        bundled: gate::bundled_checker().cloned(),
    })
}

/// D3.4: WU の check の列（`checks` と `results` は同じ順）の health の項目が Error なら、run の結果を質問
/// （`health_check_error: <理由>`。worker が `Done` で返した usage は落とさない）にすり替えた結果。
pub(crate) fn error_question(
    checks: &[task_core::WorkUnitCheck],
    results: &[(bool, String)],
    result: &Result<task_worker::RunOutcome, task_worker::AdapterError>,
) -> Option<Result<task_worker::RunOutcome, task_worker::AdapterError>> {
    let text = checks
        .iter()
        .zip(results)
        .find_map(|(c, (pass, reason))| check_error(&c.cmd, *pass, reason))?
        .to_string();
    let usage = match result {
        Ok(task_worker::RunOutcome {
            terminal: task_worker::Terminal::Done { usage, .. },
            ..
        }) => *usage,
        _ => None,
    };
    Some(Ok(task_worker::RunOutcome {
        terminal: task_worker::Terminal::Question { text, usage },
        exit_code: None,
    }))
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "check_item_tests.rs"]
mod tests;
