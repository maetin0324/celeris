//! ADR 2026-11-08-health-enforcement-loop D1.3・D2.3・D3.1: repo health の実行の口。
//!
//! - [`run_report`]: 検査器（[`BaseChecker`]）で `repo_health.py --repo --base --head --policy-version 1 --out` を 1 回
//!   起こし、schema を確かめて [`RepoHealthReport`] に読む。G2 の材料（`health_review::input`）と [`run_health_check`] が
//!   共有する唯一の起動。
//! - [`run_health_check`]: 適用判定（`task_core::required_checks::health::applicability`）が返した検査器で 1 回評価し、
//!   [`HealthEvidence`] を作る（D2.3 の値、D3.1 の判定）。検査器の有無は調べ直さず、`CheckerId` の出どころ（base・同梱）
//!   をそのまま使う。起動は `health_review::policy::with_base_checker` の 1 つの口を通る。
//! - [`configure_bundled_checker`]・[`bundled_checker`]: release に同梱した検査器（`<release>/repo-health/`。D1.4）の所在を
//!   process に 1 回設定し、読む。環境変数での上書きは置かない。
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
    CHECKER_SCRIPT, CheckerId, CheckerSource, HealthBlockingDetail, HealthEvidence, HealthVerdict,
    MAX_BLOCKING, PROFILES_DIR, PolicyIdentity, base_has_file, checker_sha256, policy_sha256,
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
    /// 判定根拠（[`HealthBlockingDetail::message`] に写す）。
    #[serde(default)]
    pub rationale: String,
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

/// D1.3: `repo` の `base..head` を `checker`（適用判定が返したもの）で 1 回評価し、証跡を作る。`timeout` は全体
/// （git・検査器・policy の判定）の上限。評価できなければ verdict は [`HealthVerdict::Error`] で、`reason` に理由
/// （`checker_not_run`・`report_missing`・`report_invalid`・`unknown`・`unsupported` など）を書く（J4）。
/// [`run_health_check_with_details`] の説明を捨てる形。
pub fn run_health_check(
    repo_id: RepoId,
    repo: &Path,
    base: &str,
    head: &str,
    checker: &CheckerId,
    timeout: Duration,
) -> HealthEvidence {
    run_health_check_with_details(repo_id, repo, base, head, checker, timeout).0
}

/// [`run_health_check`] と同じ 1 回の評価で、証跡と、`blocking` の fingerprint ごとの説明（同じ report の
/// finding の rule_id・evidence の path・rationale。`blocking` の順）を返す。棚卸し（D6）の起票の文面に使う。
/// 説明は判定の正本ではない（fail 級かは `blocking` だけが決める）。
pub fn run_health_check_with_details(
    repo_id: RepoId,
    repo: &Path,
    base: &str,
    head: &str,
    checker: &CheckerId,
    timeout: Duration,
) -> (HealthEvidence, Vec<HealthBlockingDetail>) {
    let mut evidence = HealthEvidence {
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
    };
    let mut details = Vec::new();
    let deadline = Instant::now() + timeout;
    if let Err(why) = evaluate(repo, checker, deadline, &mut evidence, &mut details) {
        evidence.verdict = HealthVerdict::Error;
        evidence.reason = Some(why);
    }
    (evidence, details)
}

/// [`run_health_check`] の本体。読めた値から順に `evidence` に書き、評価できなければ理由を返す。
fn evaluate(
    repo: &Path,
    checker: &CheckerId,
    deadline: Instant,
    evidence: &mut HealthEvidence,
    details: &mut Vec<HealthBlockingDetail>,
) -> Result<(), String> {
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
    let site = match checker.source {
        CheckerSource::Base => CheckerSite::Base,
        CheckerSource::Bundled => match bundled_checker() {
            Some(bundled) if bundled.id == *checker => CheckerSite::Bundled(&bundled.dir),
            Some(_) => {
                return Err("checker_mismatch: 設定した同梱の検査器の版が適用判定と違う".into());
            }
            None => return Err("checker_missing: 同梱の検査器が設定されていない".into()),
        },
    };
    let base = evidence.base_sha.clone();
    let ran = with_base_checker(repo, &base, site, deadline, |c| {
        evidence.base_sha = c.sha.clone();
        judge(c, repo, deadline, evidence, details)
    })?;
    ran.ok_or_else(|| "checker_missing: base に検査器が無い（適用判定と食い違う）".to_string())
}

/// 検査器で report を作り、D2.3 の値と D3.1 の判定を `evidence` に、`blocking` の説明を同じ report から
/// `details` に書く。
fn judge(
    c: &BaseChecker,
    repo: &Path,
    deadline: Instant,
    evidence: &mut HealthEvidence,
    details: &mut Vec<HealthBlockingDetail>,
) -> Result<(), String> {
    let scratch = Scratch::create("celeris-health-gate")?;
    let out = scratch.0.join("report.json");
    let ran = run_report(c, repo, &evidence.base_sha, &evidence.head_sha, &out)
        .map_err(|e| format!("{}: {e}", e.token()))?;
    let report = &ran.report;
    evidence.report_sha256 = sha256_hex(&ran.bytes);
    evidence.tool_version = report.tool.version.clone();
    evidence.reviewable = report.summary.reviewable;
    evidence.policy_sha256 = report_policy_sha256(repo, &evidence.base_sha, report, deadline)?;
    match ran.code {
        Some(0) => evidence.verdict = HealthVerdict::Pass,
        Some(1) => {
            let changed = changed_paths(repo, &evidence.base_sha, &evidence.head_sha, deadline)?;
            let judged = policy_review(c, &out, &changed, &scratch.0)?;
            let mut blocking = judged.blocking;
            blocking.sort();
            let over = blocking.len().saturating_sub(MAX_BLOCKING);
            blocking.truncate(MAX_BLOCKING);
            *details = blocking_details(report, &blocking);
            evidence.blocking = blocking;
            let more = (over > 0).then(|| format!("（ほかに fail 級の finding が {over} 件）"));
            match judged.policy_files.filter(|files| !files.is_empty()) {
                Some(files) => {
                    evidence.verdict = HealthVerdict::PolicyOnly;
                    evidence.reason = Some(format!("policy_only: {}", files.join(", ")));
                }
                None => {
                    evidence.verdict = HealthVerdict::Fail;
                    evidence.reason = more;
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

/// `blocking` の fingerprint の説明（同じ report の finding。evidence の path は重複を除いて出た順）。
fn blocking_details(report: &RepoHealthReport, blocking: &[String]) -> Vec<HealthBlockingDetail> {
    blocking
        .iter()
        .filter_map(|fingerprint| {
            let finding = report
                .findings
                .iter()
                .find(|f| &f.fingerprint == fingerprint)?;
            let mut paths: Vec<String> = Vec::new();
            for path in finding.evidence.iter().filter_map(|e| e.path.as_ref()) {
                if !paths.contains(path) {
                    paths.push(path.clone());
                }
            }
            Some(HealthBlockingDetail {
                fingerprint: fingerprint.clone(),
                rule_id: finding.rule_id.clone(),
                paths,
                message: finding.rationale.clone(),
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

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "gate_tests.rs"]
pub(crate) mod tests;
