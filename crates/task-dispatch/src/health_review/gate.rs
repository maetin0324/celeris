//! ADR 2026-11-08-health-enforcement-loop D1.2・D1.3・D2.3・D3.1・D3.2: repo health の経路共通の入口と実行の口。
//!
//! - [`health_gate`]: 全経路（review・WU の check・段の統合・再利用・同期・修復の後・delivery）が呼ぶ唯一の入口。
//!   `repo_facts` → `applicability` → `run_health_check` の順に呼び、結果を [`HealthGateOutcome`]（NotApplicable・
//!   Unavailable・Evaluated）に分ける。経路の扱いは [`HealthGateOutcome::decision`]（[`HealthGateDecision`]）が決め、
//!   Fail は rule ごとの [`HealthRepair`]（reason `health:<rule_id>:<hash>[,…]` と evidence の path、bucket
//!   `health:<rule_id>`）になる。同梱の検査器は引数（[`HealthGateRequest::bundled`]）で受け、process の
//!   [`bundled_checker`] を読まない。
//! - [`with_applicable_checker`]: 検査器の選び方の唯一の口。`repo_facts` → `applicability` を呼び、`Applies` なら
//!   `CheckerId` の出どころ（base・同梱）を `health_review::policy::CheckerSite` に写して（写しはこの file の
//!   `checker_site` だけ）`health_review::policy::with_base_checker` で検査器を出す。[`health_gate`] と G2 の 3 か所
//!   （review 方針の `load_review_policy`、G2 の材料と policy の関門が読む policy の状態。`health_review::input`）が
//!   呼ぶ。`NotApplicable` は当てず、`Unavailable` は合格にしない（呼び出し側が理由を残す）。
//! - [`check_with`]: 選んだ検査器で `base..head` を 1 回評価し、[`HealthEvidence`] を作り（D2.3 の値、D3.1 の判定）、
//!   評価で読んだ report も返す。`run_health_check`（[`health_gate`] が呼ぶ）と G2 の材料が共有し、同じ candidate で
//!   検査器（report）を 2 回起こさない。
//! - [`run_report`]: 検査器（[`BaseChecker`]）で `repo_health.py --repo --base --head --policy-version 1 --out` を 1 回
//!   起こし、schema を確かめて [`RepoHealthReport`] に読む（[`check_with`] だけが呼ぶ）。
//! - [`configure_bundled_checker`]・[`bundled_checker`]: release に同梱した検査器（`<release>/repo-health/`。D1.4）の所在を
//!   process に 1 回設定し、読む（読むのは daemon の起動と経路の外側の 1 か所だけ）。環境変数での上書きは置かない。
//!
//! policy の file だけの変更（PolicyOnly）かは、report を作った同じ検査器の `health.engine.policy_review_files` が
//! 決める（`scripts/dev/check-health.sh` と同じ呼び方）。Rust に同じ判定を書かない。LLM は呼ばない（I4）。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use task_core::RepoId;
use task_core::required_checks::health::{
    CHECKER_SCRIPT, CheckerId, CheckerSource, HealthApplicability, HealthEvidence, HealthVerdict,
    MAX_BLOCKING, NotApplicableReason, PROFILES_DIR, PolicyIdentity, UnavailableReason,
    applicability, base_has_file, checker_sha256, policy_sha256, repo_facts,
};
use task_worker::protocol::HealthFindingCounts;

use super::policy::{BaseChecker, CheckerSite, Scratch, git_stdout, with_base_checker};

/// `repo_health.py --policy-version` に渡す版。
pub const POLICY_VERSION: &str = "1";
/// 読む report の schema。
pub const REPORT_SCHEMA: &str = "celeris.repo-health.report/1";
/// 同梱の dir で検査器の版を書いた file（release.sh の `sd_bundle_repo_health` が書く。`repo-health <tree id>` と
/// `source-size-report <blob id>` の行）。
pub const CHECKER_TREE_FILE: &str = "CHECKER_TREE";

/// report（`celeris.repo-health.report/1`）のうち読む欄。
#[derive(Debug, Deserialize)]
pub struct RepoHealthReport {
    pub schema: String,
    #[serde(default)]
    pub tool: RepoHealthTool,
    #[serde(default)]
    pub inputs: RepoHealthInputs,
    pub summary: RepoHealthSummary,
    #[serde(default)]
    pub rules: Vec<RepoHealthRule>,
    #[serde(default)]
    pub findings: Vec<RepoHealthFinding>,
    pub policy: RepoHealthPolicy,
}

#[derive(Debug, Default, Deserialize)]
pub struct RepoHealthTool {
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct RepoHealthInputs {
    #[serde(default)]
    pub profile: RepoHealthProfile,
}

/// report の `inputs.profile`。
#[derive(Debug, Default, Deserialize)]
pub struct RepoHealthProfile {
    #[serde(default)]
    pub resolved: String,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Deserialize)]
pub struct RepoHealthSummary {
    pub verdict: String,
    pub reviewable: bool,
    pub counts: HealthFindingCounts,
}

#[derive(Debug, Default, Deserialize)]
pub struct RepoHealthRule {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Deserialize)]
pub struct RepoHealthFinding {
    pub rule_id: String,
    pub fingerprint: String,
    pub severity: String,
    pub delta: String,
    #[serde(default)]
    pub exception: Option<serde_json::Value>,
    #[serde(default)]
    pub evidence: Vec<RepoHealthEvidence>,
}

#[derive(Debug, Deserialize)]
pub struct RepoHealthEvidence {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RepoHealthPolicy {
    /// report の `policy.version`。
    #[serde(default)]
    pub version: String,
    pub source: String,
    pub changed: bool,
    /// report の `policy.files`（file ごとの base と head の blob）。
    #[serde(default, rename = "files")]
    pub blobs: Vec<PolicyBlobs>,
}

#[derive(Debug, Deserialize)]
pub struct PolicyBlobs {
    pub path: String,
    #[serde(default)]
    pub base_blob: Option<String>,
    #[serde(default)]
    pub head_blob: Option<String>,
}

/// [`run_report`] の 1 回の実行の結果。
#[derive(Debug)]
pub(crate) struct ReportRun {
    /// `repo_health.py` の exit code（signal で終われば `None`）。
    pub code: Option<i32>,
    /// report の file の byte 列（[`HealthEvidence::report_sha256`] の元）。
    pub bytes: Vec<u8>,
    pub report: RepoHealthReport,
}

/// [`run_report`] が report を読めなかった理由。表示は理由の文だけ（G2 の材料の理由の文と同じ）。
#[derive(Debug)]
pub(crate) enum ReportError {
    /// 検査器を起こせない・時間切れ。
    NotRun(String),
    /// report の file が無い。
    Missing(String),
    /// report が JSON でない・schema が違う。
    Invalid(String),
}

impl ReportError {
    /// [`HealthEvidence::reason`] の先頭の語。
    fn token(&self) -> &'static str {
        match self {
            ReportError::NotRun(_) => "checker_not_run",
            ReportError::Missing(_) => "report_missing",
            ReportError::Invalid(_) => "report_invalid",
        }
    }
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReportError::NotRun(why) | ReportError::Missing(why) | ReportError::Invalid(why) => {
                f.write_str(why)
            }
        }
    }
}

/// `checker` で repo health を 1 回走らせ（`--repo <repo> --base <base> --head <head> --policy-version 1 --out <out>`）、
/// report を読む。exit 1（fail）・2（error）でも report があれば読む。`out` の dir は作り、前の file は消す。
pub(crate) fn run_report(
    checker: &BaseChecker,
    repo: &Path,
    base: &str,
    head: &str,
    out: &Path,
) -> Result<ReportRun, ReportError> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| ReportError::NotRun(format!("report の dir を作れない: {e}")))?;
    }
    let _ = std::fs::remove_file(out);
    let (repo_arg, out_arg) = (repo.to_string_lossy(), out.to_string_lossy());
    let ran = checker
        .run(
            &[
                "--repo",
                &repo_arg,
                "--base",
                base,
                "--head",
                head,
                "--policy-version",
                POLICY_VERSION,
                "--out",
                &out_arg,
            ],
            "repo_health.py",
        )
        .map_err(ReportError::NotRun)?;
    let bytes = std::fs::read(out).map_err(|e| {
        ReportError::Missing(format!(
            "repo_health.py が report を書かなかった（exit {:?}: {}）: {e}",
            ran.code,
            ran.why()
        ))
    })?;
    let report: RepoHealthReport = serde_json::from_slice(&bytes)
        .map_err(|e| ReportError::Invalid(format!("report を読めない: {e}")))?;
    if report.schema != REPORT_SCHEMA {
        return Err(ReportError::Invalid(format!(
            "report の schema が {REPORT_SCHEMA} でない: {:?}",
            report.schema
        )));
    }
    Ok(ReportRun {
        code: ran.code,
        bytes,
        report,
    })
}

/// release に同梱した検査器（D1.4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledChecker {
    /// `<release>/repo-health`（base から出したときと同じ repo 相対の配置）。
    pub dir: PathBuf,
    /// `CHECKER_TREE` から [`checker_sha256`] で計算した版（source は [`CheckerSource::Bundled`]）。
    pub id: CheckerId,
}

static BUNDLED: OnceLock<BundledChecker> = OnceLock::new();

/// 同梱の検査器の所在を process に 1 回設定する（daemon の起動で呼ぶ）。`dir` の `CHECKER_TREE` を読んで版を計算する。
/// 別の dir・版で 2 回目を呼ぶと拒否する（同じ値なら設定済みの値を返す）。
pub fn configure_bundled_checker(dir: &Path) -> Result<&'static BundledChecker, String> {
    let found = read_bundled_checker(dir)?;
    let set = BUNDLED.get_or_init(|| found.clone());
    if *set != found {
        return Err(format!(
            "同梱の検査器は既に {} に設定されている",
            set.dir.display()
        ));
    }
    Ok(set)
}

/// 設定した同梱の検査器（未設定なら `None`）。
pub fn bundled_checker() -> Option<&'static BundledChecker> {
    BUNDLED.get()
}

/// `dir` の同梱の検査器を読む（`CHECKER_TREE` と `repo_health.py` が要る）。
pub fn read_bundled_checker(dir: &Path) -> Result<BundledChecker, String> {
    if !dir.join(CHECKER_SCRIPT).is_file() {
        return Err(format!(
            "同梱の検査器 {} が無い",
            dir.join(CHECKER_SCRIPT).display()
        ));
    }
    let path = dir.join(CHECKER_TREE_FILE);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{} を読めない: {e}", path.display()))?;
    let (mut tree, mut side) = (None, None);
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (key, id) = line.split_once(' ').unwrap_or((line, ""));
        let id = id.trim();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("{} の行が壊れている: {line:?}", path.display()));
        }
        match key {
            "repo-health" => tree = Some(id.to_string()),
            "source-size-report" => side = Some(id.to_string()),
            _ => return Err(format!("{} の行が未知: {line:?}", path.display())),
        }
    }
    let tree = tree.ok_or_else(|| format!("{} に repo-health の行が無い", path.display()))?;
    Ok(BundledChecker {
        dir: dir.to_path_buf(),
        id: CheckerId {
            source: CheckerSource::Bundled,
            sha256: checker_sha256(&tree, side.as_deref()),
        },
    })
}

/// D1.2・D1.3: 経路共通の入口 [`health_gate`] の入力。経路は repo の事実だけを渡す（案件名・repo 名・task の mode は
/// 持たない。J1）。
#[derive(Debug, Clone, Copy)]
pub struct HealthGateRequest<'a> {
    pub repo_id: RepoId,
    /// 評価する repo の作業ツリー（git をここで実行する）。
    pub worktree: &'a Path,
    /// 評価の base（commit の sha か ref。証跡には解決した sha を残す）。
    pub base: &'a str,
    /// 評価する候補（commit の sha か ref）。
    pub head: &'a str,
    /// Celeris が branch を統合する作業ツリーか（ssh の remote workspace は false。ADR-0079 D5）。
    pub integrated_by_celeris: bool,
    /// release に同梱した検査器（daemon が起動で読んだ値を経路の外側から渡す。無ければ `None`）。
    pub bundled: Option<&'a BundledChecker>,
    /// 事実の読み取り・検査器・policy の判定の全体の上限（経路は `HEALTH_INPUT_TIMEOUT` を渡す。D4.2）。
    pub timeout: Duration,
}

/// [`health_gate`] の結果（D1.2）。どの場合も [`HealthGateOutcome::evidence`] に証跡がある。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthGateOutcome {
    /// 適用判定が NotApplicable: 必須検査の列に health の項目を足さない。`evidence` は理由を残す証跡（verdict
    /// NotApplicable）。
    NotApplicable {
        reason: NotApplicableReason,
        evidence: HealthEvidence,
    },
    /// 適用判定が Unavailable: health の項目を verdict Error の結果として足す（合格にしない。J4）。
    Unavailable {
        reason: UnavailableReason,
        evidence: HealthEvidence,
    },
    /// 検査器で評価した（verdict は Pass・PolicyOnly・Fail・Error。D3.1）。
    Evaluated(HealthRun),
}

/// 検査器での 1 回の評価（`run_health_check` の結果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthRun {
    pub evidence: HealthEvidence,
    /// verdict が Fail のときの fail 級の finding（`evidence.blocking` と同じ fingerprint・同じ順）。ほかは空。
    pub failures: Vec<HealthFailure>,
}

/// Fail の fail 級の finding 1 件（report の finding のうち、同じ検査器の `is_fail_grade` が選んだもの）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthFailure {
    pub rule_id: String,
    /// report/1 の fingerprint（schema で `<rule_id>:<hash>`）。
    pub fingerprint: String,
    /// finding の evidence の path（重複なし・sort 済み）。
    pub paths: Vec<String>,
}

/// 経路が health の項目をどう扱うか（D3.1〜D3.4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthGateDecision {
    /// 項目を足さない（NotApplicable）。
    NotApplicable,
    /// 合格（exit 0）。
    Pass,
    /// policy の file だけの変更（G2 D6.2）。WU の check・段の統合は合格扱いで進め、task の review は G2 の
    /// `policy_review_gate` が人の承認を求める。
    PolicyOnly,
    /// 統合を止めて既存の repair へ回す（D3.2）。rule ごとに 1 件（rule_id の順）。
    Repair(Vec<HealthRepair>),
    /// 合格にせず、repair にも回さない（Error・Unavailable。D3.4 で Blocked と受信箱）。`reason` は証跡の理由。
    Error { reason: String },
}

/// D3.2: Fail を既存の repair（review は `FailedCheck`、WU・段の統合は既存の repair）に回す材料。1 rule に 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthRepair {
    pub rule_id: String,
    /// `health:<rule_id>:<hash>[,…]`（fingerprint ごと）の後に evidence の path を付けた文。
    pub reason: String,
    /// この rule の finding の evidence の path（重複なし・sort 済み）。
    pub paths: Vec<String>,
}

impl HealthRepair {
    /// `RepairClass` の bucket（`max_repairs_per_class` を rule ごとに数える鍵。D3.2・D4.1）。
    pub fn bucket(&self) -> String {
        format!("health:{}", self.rule_id)
    }
}

/// D3.2: fail 級の finding を rule ごとの [`HealthRepair`] にする（rule_id の順、rule の中は fingerprint の順）。reason の
/// 項目は `health:<fingerprint>`（report/1 の schema で fingerprint は `<rule_id>:<hash>` なので `health:<rule_id>:<hash>`）。
pub fn health_repairs(failures: &[HealthFailure]) -> Vec<HealthRepair> {
    let mut by_rule: std::collections::BTreeMap<&str, Vec<&HealthFailure>> = Default::default();
    for failure in failures {
        by_rule.entry(&failure.rule_id).or_default().push(failure);
    }
    by_rule
        .into_iter()
        .map(|(rule_id, mut found)| {
            found.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
            let items: Vec<String> = found
                .iter()
                .map(|f| format!("health:{}", f.fingerprint))
                .collect();
            let paths: Vec<String> = found
                .iter()
                .flat_map(|f| f.paths.iter().cloned())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let reason = if paths.is_empty() {
                items.join(",")
            } else {
                format!("{}（evidence: {}）", items.join(","), paths.join(", "))
            };
            HealthRepair {
                rule_id: rule_id.to_string(),
                reason,
                paths,
            }
        })
        .collect()
}

impl HealthGateOutcome {
    /// 証跡（NotApplicable も理由を持つ証跡を返す）。
    pub fn evidence(&self) -> &HealthEvidence {
        match self {
            HealthGateOutcome::NotApplicable { evidence, .. }
            | HealthGateOutcome::Unavailable { evidence, .. } => evidence,
            HealthGateOutcome::Evaluated(run) => &run.evidence,
        }
    }

    /// 必須検査の列に足す health の項目（NotApplicable は `None`。D1.2）。
    pub fn item(&self) -> Option<&HealthEvidence> {
        match self {
            HealthGateOutcome::NotApplicable { .. } => None,
            other => Some(other.evidence()),
        }
    }

    /// 経路の扱い（D3.1〜D3.4）。Pass だけが合格で、Error・Unavailable は合格にも repair にもしない（J4・D3.4）。
    pub fn decision(&self) -> HealthGateDecision {
        let run = match self {
            HealthGateOutcome::NotApplicable { .. } => return HealthGateDecision::NotApplicable,
            HealthGateOutcome::Unavailable { evidence, .. } => {
                return HealthGateDecision::Error {
                    reason: evidence.reason.clone().unwrap_or_default(),
                };
            }
            HealthGateOutcome::Evaluated(run) => run,
        };
        match run.evidence.verdict {
            HealthVerdict::Pass => HealthGateDecision::Pass,
            HealthVerdict::PolicyOnly => HealthGateDecision::PolicyOnly,
            HealthVerdict::Fail => HealthGateDecision::Repair(health_repairs(&run.failures)),
            HealthVerdict::Error | HealthVerdict::NotApplicable => HealthGateDecision::Error {
                reason: run.evidence.reason.clone().unwrap_or_default(),
            },
        }
    }
}

/// D1.2・D1.3: 経路共通の入口。`repo_facts` → `applicability` → `run_health_check` の順に呼び、結果を分ける。
/// `request.timeout` は事実の読み取りと評価の全体の上限。同梱の検査器は `request.bundled` だけを使う。LLM は呼ばない。
pub fn health_gate(request: &HealthGateRequest<'_>) -> HealthGateOutcome {
    let deadline = Instant::now() + test_timeout::cap(request.worktree, request.timeout);
    let blank = |verdict, reason: String| HealthEvidence {
        repo_id: request.repo_id,
        base_sha: request.base.to_string(),
        head_sha: request.head.to_string(),
        head_tree: String::new(),
        policy_sha256: String::new(),
        checker: None,
        report_sha256: String::new(),
        tool_version: String::new(),
        verdict,
        blocking: Vec::new(),
        reason: Some(reason),
        reused_from: None,
        reviewable: false,
    };
    let applies = applicable(
        request.worktree,
        request.base,
        request.integrated_by_celeris,
        request.bundled,
        deadline,
    );
    match applies {
        HealthApplicability::NotApplicable { reason } => HealthGateOutcome::NotApplicable {
            evidence: blank(HealthVerdict::NotApplicable, reason.as_str().to_string()),
            reason,
        },
        HealthApplicability::Unavailable { reason } => HealthGateOutcome::Unavailable {
            evidence: blank(HealthVerdict::Error, reason.to_string()),
            reason,
        },
        HealthApplicability::Applies { checker } => HealthGateOutcome::Evaluated(run_health_check(
            request.repo_id,
            request.worktree,
            request.base,
            request.head,
            &checker,
            request.bundled,
            deadline.saturating_duration_since(Instant::now()),
        )),
    }
}

/// D1.1: `worktree` の `base` の事実（`repo_facts`。git は締め切りまで）で `applicability` を呼ぶ。
pub(crate) fn applicable(
    worktree: &Path,
    base: &str,
    integrated_by_celeris: bool,
    bundled: Option<&BundledChecker>,
    deadline: Instant,
) -> HealthApplicability {
    let facts = repo_facts(
        integrated_by_celeris,
        base,
        bundled.map(|b| &b.id),
        |args| git_stdout(worktree, args, deadline),
    );
    applicability(&facts)
}

/// [`with_applicable_checker`] の結果（D1.1・D1.2）。
#[derive(Debug)]
pub(crate) enum CheckerChoice<T> {
    /// 当てない（Git 管理でない作業場所・remote workspace）。
    NotApplicable(NotApplicableReason),
    /// 当てるべきだが評価できない（`checker_missing`・`facts_unreadable:<理由>`）。合格にしない（J4）。
    Unavailable(UnavailableReason),
    /// 適用判定が選んだ検査器で `run` を呼んだ。`Err` は検査器を出せない・`run` が失敗した理由。
    Ran(Result<T, String>),
}

/// D1.3: 検査器の選び方の唯一の口。`worktree` の `base` で `repo_facts` → `applicability` を呼び、`Applies` なら選んだ
/// 検査器（base の版を一時 dir に出したもの、または `bundled` の dir）と `CheckerId` を `run` に渡す。同梱の検査器は
/// `bundled`（呼び出し側が渡す）だけを使う。`deadline` は事実の読み取り・検査器を出す・`run` の全体の締め切り。
pub(crate) fn with_applicable_checker<T>(
    worktree: &Path,
    base: &str,
    integrated_by_celeris: bool,
    bundled: Option<&BundledChecker>,
    deadline: Instant,
    run: impl FnOnce(&BaseChecker, &CheckerId) -> Result<T, String>,
) -> CheckerChoice<T> {
    match applicable(worktree, base, integrated_by_celeris, bundled, deadline) {
        HealthApplicability::NotApplicable { reason } => CheckerChoice::NotApplicable(reason),
        HealthApplicability::Unavailable { reason } => CheckerChoice::Unavailable(reason),
        HealthApplicability::Applies { checker } => CheckerChoice::Ran(with_checker(
            worktree,
            base,
            &checker,
            bundled,
            deadline,
            |c| run(c, &checker),
        )),
    }
}

/// D1.3・D1.4: `CheckerId` の出どころを起こす検査器の場所に写す（写しはここだけ）。同梱は `bundled` の版が
/// 適用判定と同じときだけ使う。
fn checker_site<'a>(
    checker: &CheckerId,
    bundled: Option<&'a BundledChecker>,
) -> Result<CheckerSite<'a>, String> {
    match checker.source {
        CheckerSource::Base => Ok(CheckerSite::Base),
        CheckerSource::Bundled => match bundled {
            Some(bundled) if bundled.id == *checker => Ok(CheckerSite::Bundled(&bundled.dir)),
            Some(_) => Err("checker_mismatch: 渡された同梱の検査器の版が適用判定と違う".into()),
            None => Err("checker_missing: 同梱の検査器が渡されていない".into()),
        },
    }
}

/// 適用判定が返した `checker` を出して `run` に渡す（base の版は戻ると消す）。
fn with_checker<T>(
    repo: &Path,
    base: &str,
    checker: &CheckerId,
    bundled: Option<&BundledChecker>,
    deadline: Instant,
    run: impl FnOnce(&BaseChecker) -> Result<T, String>,
) -> Result<T, String> {
    let site = checker_site(checker, bundled)?;
    with_base_checker(repo, base, site, deadline, run)?
        .ok_or_else(|| "checker_missing: base に検査器が無い（適用判定と食い違う）".to_string())
}

/// D1.3: `repo` の `base..head` を `checker`（適用判定が返したもの）で 1 回評価し、証跡を作る。`timeout` は全体
/// （git・検査器・policy の判定）の上限。同梱の検査器は `bundled`（[`health_gate`] が受けた値）だけを使う。評価できなければ
/// verdict は [`HealthVerdict::Error`] で、`reason` に理由（`checker_not_run`・`report_missing`・`report_invalid`・
/// `unknown`・`unsupported` など）を書く（J4）。経路は直接呼ばず [`health_gate`] を通す。
fn run_health_check(
    repo_id: RepoId,
    repo: &Path,
    base: &str,
    head: &str,
    checker: &CheckerId,
    bundled: Option<&BundledChecker>,
    timeout: Duration,
) -> HealthRun {
    let deadline = Instant::now() + timeout;
    let checked = with_checker(repo, base, checker, bundled, deadline, |c| {
        Ok(check_with(c, checker, repo_id, repo, head, None, deadline))
    });
    match checked {
        Ok(checked) => checked.run,
        Err(why) => {
            let mut run = HealthRun {
                evidence: error_evidence(repo_id, base, head, checker),
                failures: Vec::new(),
            };
            run.evidence.reason = Some(why);
            run
        }
    }
}

/// 評価の前の証跡（verdict は Error。読めた値から順に書く）。
fn error_evidence(repo_id: RepoId, base: &str, head: &str, checker: &CheckerId) -> HealthEvidence {
    HealthEvidence {
        repo_id,
        base_sha: base.to_string(),
        head_sha: head.to_string(),
        head_tree: String::new(),
        policy_sha256: String::new(),
        checker: Some(checker.clone()),
        report_sha256: String::new(),
        tool_version: String::new(),
        verdict: HealthVerdict::Error,
        blocking: Vec::new(),
        reason: None,
        reused_from: None,
        reviewable: false,
    }
}

/// [`check_with`] の結果: 証跡と、評価で読んだ report（G2 の材料が同じ report を使う）。
#[derive(Debug)]
pub(crate) struct CheckedRun {
    pub run: HealthRun,
    /// 読めた report。検査器を起こせない・report が無い・壊れている（または report を作る前に止まった）なら理由。
    /// report を読めた後の判定（policy の判定など）が失敗しても report は返す（`run` は Error）。
    pub report: Result<RepoHealthReport, String>,
}

/// D1.3・D2.3・D3.1: `c`（適用判定が選んだ `checker` を出したもの）で `repo` の `c.sha..head` を 1 回評価する。
/// report は `report_out`（無ければ一時 dir）に書く。`run_health_check` と G2 の材料（`health_review::input`）が共有する
/// 唯一の評価で、検査器の起動（`run_report`）は 1 回。
pub(crate) fn check_with(
    c: &BaseChecker,
    checker: &CheckerId,
    repo_id: RepoId,
    repo: &Path,
    head: &str,
    report_out: Option<&Path>,
    deadline: Instant,
) -> CheckedRun {
    let mut run = HealthRun {
        evidence: error_evidence(repo_id, &c.sha, head, checker),
        failures: Vec::new(),
    };
    let mut report = None;
    let evaluated = evaluate(c, repo, report_out, deadline, &mut run, &mut report);
    if let Err(why) = &evaluated {
        run.evidence.verdict = HealthVerdict::Error;
        run.evidence.reason = Some(why.clone());
        run.failures.clear();
    }
    let report = report.unwrap_or_else(|| Err(evaluated.err().unwrap_or_default()));
    CheckedRun { run, report }
}

/// [`check_with`] の本体。読めた値から順に `run` に書き、検査器を起こしたら report（読めなければ理由の文。証跡の
/// `reason` と違い先頭の語を付けない）を `report` に置く。評価できなければ理由を返す。
fn evaluate(
    c: &BaseChecker,
    repo: &Path,
    report_out: Option<&Path>,
    deadline: Instant,
    run: &mut HealthRun,
    report: &mut Option<Result<RepoHealthReport, String>>,
) -> Result<(), String> {
    let evidence = &mut run.evidence;
    let git = |args: &[&str]| git_stdout(repo, args, deadline);
    let head_commit = format!("{}^{{commit}}", evidence.head_sha);
    let head_sha = git(&["rev-parse", "--verify", &head_commit]).map_err(|e| {
        format!(
            "head_unreadable: {} を commit に解決できない: {e}",
            evidence.head_sha
        )
    })?;
    evidence.head_sha = head_sha.trim().to_string();
    let head_tree = format!("{}^{{tree}}", evidence.head_sha);
    evidence.head_tree = git(&["rev-parse", "--verify", &head_tree])
        .map_err(|e| format!("head_unreadable: {head_tree} を読めない: {e}"))?
        .trim()
        .to_string();
    let scratch = Scratch::create("celeris-health-gate")?;
    let out = report_out.map_or_else(|| scratch.0.join("report.json"), Path::to_path_buf);
    let ran = match run_report(c, repo, &evidence.base_sha, &evidence.head_sha, &out) {
        Ok(ran) => ran,
        Err(e) => {
            *report = Some(Err(e.to_string()));
            return Err(format!("{}: {e}", e.token()));
        }
    };
    let judged = judge(c, repo, deadline, run, &ran, &out, &scratch);
    *report = Some(Ok(ran.report));
    judged
}

/// 検査器が書いた report（`out`）から D2.3 の値と D3.1 の判定を `run` に書く（Fail は fail 級の finding も。D3.2）。
fn judge(
    c: &BaseChecker,
    repo: &Path,
    deadline: Instant,
    run: &mut HealthRun,
    ran: &ReportRun,
    out: &Path,
    scratch: &Scratch,
) -> Result<(), String> {
    let evidence = &mut run.evidence;
    let report = &ran.report;
    evidence.report_sha256 = sha256_hex(&ran.bytes);
    evidence.tool_version = report.tool.version.clone();
    evidence.reviewable = report.summary.reviewable;
    evidence.policy_sha256 = report_policy_sha256(repo, &evidence.base_sha, report, deadline)?;
    match ran.code {
        Some(0) => evidence.verdict = HealthVerdict::Pass,
        Some(1) => {
            let changed = changed_paths(repo, &evidence.base_sha, &evidence.head_sha, deadline)?;
            let judged = policy_review(c, out, &changed, &scratch.0)?;
            let mut blocking = judged.blocking;
            blocking.sort();
            let over = blocking.len().saturating_sub(MAX_BLOCKING);
            blocking.truncate(MAX_BLOCKING);
            evidence.blocking = blocking;
            let more = (over > 0).then(|| format!("（ほかに fail 級の finding が {over} 件）"));
            match judged.policy_files.filter(|files| !files.is_empty()) {
                Some(files) => {
                    evidence.verdict = HealthVerdict::PolicyOnly;
                    evidence.reason = Some(format!("policy_only: {}", files.join(", ")));
                }
                None if evidence.blocking.is_empty() => {
                    return Err(
                        "report_invalid: exit 1 なのに fail 級の finding が無い（修復する対象が無い）".into(),
                    );
                }
                None => {
                    evidence.verdict = HealthVerdict::Fail;
                    evidence.reason = more;
                    run.failures = failures(report, &evidence.blocking)?;
                }
            }
        }
        Some(2) => {
            evidence.verdict = HealthVerdict::Error;
            evidence.reason = Some(error_reason(report));
        }
        other => {
            evidence.verdict = HealthVerdict::Error;
            evidence.reason = Some(format!("unexpected_exit: repo_health.py の exit {other:?}"));
        }
    }
    Ok(())
}

/// `blocking`（同じ検査器の `is_fail_grade` が選んだ fingerprint）の report の finding（`blocking` の順）。report に無い
/// fingerprint は壊れた report（fail closed）。
fn failures(report: &RepoHealthReport, blocking: &[String]) -> Result<Vec<HealthFailure>, String> {
    blocking
        .iter()
        .map(|fp| {
            let f = report
                .findings
                .iter()
                .find(|f| &f.fingerprint == fp)
                .ok_or_else(|| {
                    format!("report_invalid: fail 級の {fp} が report の findings に無い")
                })?;
            Ok(HealthFailure {
                rule_id: f.rule_id.clone(),
                fingerprint: f.fingerprint.clone(),
                paths: f
                    .evidence
                    .iter()
                    .filter_map(|e| e.path.clone())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            })
        })
        .collect()
}

/// exit 2 の理由: 必須 rule のうち `error`・`unknown`・`unsupported` のもの（`<status>:<rule id>(<reason>)`）。
fn error_reason(report: &RepoHealthReport) -> String {
    let rules: Vec<String> = report
        .rules
        .iter()
        .filter(|r| r.required && matches!(r.status.as_str(), "error" | "unknown" | "unsupported"))
        .map(|r| match r.reason.as_str() {
            "" => format!("{}:{}", r.status, r.id),
            why => format!("{}:{}({why})", r.status, r.id),
        })
        .collect();
    if rules.is_empty() {
        format!("{}: repo_health.py の exit 2", report.summary.verdict)
    } else {
        rules.join(", ")
    }
}

/// D2.3 の policy の指紋: report の `policy.version`・`policy.source`・`inputs.profile` と、base の profile の dir の
/// tree id（無ければ builtin）と `policy.files[].base_blob`。
fn report_policy_sha256(
    repo: &Path,
    base: &str,
    report: &RepoHealthReport,
    deadline: Instant,
) -> Result<String, String> {
    let git = |args: &[&str]| git_stdout(repo, args, deadline);
    let profiles_tree = if base_has_file(base, PROFILES_DIR, git)? {
        let object = format!("{base}:{PROFILES_DIR}");
        Some(
            git(&["rev-parse", "--verify", &object])
                .map_err(|e| format!("{object} の tree id を読めない: {e}"))?
                .trim()
                .to_string(),
        )
    } else {
        None
    };
    let base_blobs: Vec<Option<String>> = report
        .policy
        .blobs
        .iter()
        .map(|f| f.base_blob.clone())
        .collect();
    Ok(policy_sha256(&PolicyIdentity {
        policy_version: &report.policy.version,
        policy_source: &report.policy.source,
        profile_resolved: &report.inputs.profile.resolved,
        profile_source: &report.inputs.profile.source,
        profiles_tree: profiles_tree.as_deref(),
        base_blobs: &base_blobs,
    }))
}

/// `base..head` の変更 path（`git diff --no-renames --name-only -z`。検査器の snapshot と同じ引数）。
fn changed_paths(
    repo: &Path,
    base: &str,
    head: &str,
    deadline: Instant,
) -> Result<Vec<String>, String> {
    let names = git_stdout(
        repo,
        &[
            "diff",
            "--no-renames",
            "--name-only",
            "-z",
            "--no-ext-diff",
            base,
            head,
        ],
        deadline,
    )
    .map_err(|e| format!("changed_unreadable: {base}..{head} の変更 path を読めない: {e}"))?;
    Ok(names
        .split('\0')
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect())
}

/// 同じ検査器の `health.engine` の判定。
#[derive(Deserialize)]
struct PolicyReview {
    /// `policy_review_files` の値（policy の file だけの変更でなければ `None`）。
    policy_files: Option<Vec<String>>,
    /// `is_fail_grade` の finding の fingerprint。
    blocking: Vec<String>,
}

/// report を作った検査器の `health.engine.policy_review_files(report, changed)` と、`is_fail_grade` の finding の
/// fingerprint を読む（`scripts/dev/check-health.sh` と同じく `python3 -I` で検査器の dir を `sys.path` に入れる）。
const POLICY_REVIEW_PY: &str = r#"import json
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, sys.argv[1])
from health import engine  # noqa: E402

with open(sys.argv[2], encoding="utf-8") as fh:
    report = json.load(fh)
with open(sys.argv[3], encoding="utf-8") as fh:
    changed = json.load(fh)
files = engine.policy_review_files(report, changed)
blocking = sorted(f["fingerprint"] for f in report["findings"] if engine.is_fail_grade(f))
json.dump({"policy_files": files, "blocking": blocking}, sys.stdout)
"#;

fn policy_review(
    c: &BaseChecker,
    report: &Path,
    changed: &[String],
    scratch: &Path,
) -> Result<PolicyReview, String> {
    let changed_file = scratch.join("changed.json");
    let encoded = serde_json::to_vec(changed)
        .map_err(|e| format!("policy_review_failed: 変更 path を書けない: {e}"))?;
    std::fs::write(&changed_file, encoded)
        .map_err(|e| format!("policy_review_failed: 変更 path を書けない: {e}"))?;
    let dir = c.checker_dir();
    let (dir_arg, report_arg, changed_arg) = (
        dir.to_string_lossy(),
        report.to_string_lossy(),
        changed_file.to_string_lossy(),
    );
    let ran = c
        .python(
            &["-c", POLICY_REVIEW_PY, &dir_arg, &report_arg, &changed_arg],
            "health.engine.policy_review_files",
        )
        .map_err(|e| format!("policy_review_failed: {e}"))?;
    if !ran.ok {
        return Err(format!(
            "policy_review_failed: health.engine.policy_review_files が失敗: {}",
            ran.why()
        ));
    }
    serde_json::from_str(&ran.stdout)
        .map_err(|e| format!("policy_review_failed: 判定の出力を読めない: {e}"))
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "gate_tests.rs"]
pub(crate) mod tests;

/// 試験専用の口（D4.2）: 経路は `HEALTH_INPUT_TIMEOUT`（600 秒）を渡すので、時間切れの試験は登録した作業ツリーの下の
/// 評価だけ上限を縮める（終わらない stub と組み合わせ、600 秒待たずに時間切れを作る）。鍵は作業ツリーの path なので、
/// 同じ process の他の試験の評価には効かない。本番の build では上限を変えない。
#[cfg(test)]
pub(crate) mod test_timeout {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    static CAPS: Mutex<Vec<(PathBuf, Duration)>> = Mutex::new(Vec::new());

    /// `root` の下の作業ツリーの評価の上限を `timeout` 以下にする。
    pub(crate) fn cap_under(root: &Path, timeout: Duration) {
        CAPS.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((root.to_path_buf(), timeout));
    }

    pub(super) fn cap(worktree: &Path, timeout: Duration) -> Duration {
        let caps = CAPS.lock().unwrap_or_else(|e| e.into_inner());
        (caps.iter())
            .filter(|(root, _)| worktree.starts_with(root))
            .map(|(_, t)| *t)
            .fold(timeout, Duration::min)
    }
}

#[cfg(not(test))]
mod test_timeout {
    pub(super) fn cap(_: &std::path::Path, timeout: std::time::Duration) -> std::time::Duration {
        timeout
    }
}
