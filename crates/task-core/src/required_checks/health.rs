//! ADR 2026-11-08-health-enforcement-loop D1.1・D2.1〜D2.3: repo health を当てるかの判定と、結果を候補に結び付ける
//! 証跡の型・指紋。
//!
//! - [`repo_facts`] が repo の事実（Git 管理か・Celeris が統合する作業ツリーか・base に検査器があるか・同梱の検査器）を
//!   集め、[`applicability`] だけが当てるかを決める。どちらも案件名・repo 名・`self_improve`・selfdeploy・task の mode を
//!   入力に持たない（J1）。読めない事実は [`HealthApplicability::Unavailable`]（fail closed）。
//! - 検査器の所在（[`CHECKER_DIR`]・[`CHECKER_SCRIPT`]・[`CHECKER_SIDE_FILES`]）と base に検査器があるかの判定
//!   （[`base_has_file`]・[`base_checker`]）の正本はここ。dispatcher の起動の口もこれを使う。
//! - [`HealthEvidence`] は `PhaseCheckResult.health`・`WorkUnitCheckFinished.health`・`IntegrationCheckFinished.health`
//!   に入る証跡。検査器の版は [`checker_sha256`]、policy は [`policy_sha256`] で Celeris 側が計算する（report/1 と
//!   検査器は変えない。D2.3）。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::RepoId;

/// 必須検査の列での health の項目の `cmd`（D2.2）。
pub const HEALTH_CHECK_ID: &str = "celeris:repo-health/1";
/// 検査器の dir（repo 相対）。base から取り出すときも、release に同梱するときもこの配置。
pub const CHECKER_DIR: &str = "scripts/dev/repo-health";
/// 起こす script（repo 相対）。base にこれがあれば base の検査器を使う（D1.4）。
pub const CHECKER_SCRIPT: &str = "scripts/dev/repo-health/repo_health.py";
/// 検査器が同じ checkout から読む size の report（repo 相対）。無い base もある。
pub const SOURCE_SIZE_REPORT: &str = "scripts/dev/source-size-report.py";
/// 検査器の rule arch-map.* が同じ checkout から import する地図の検査器（repo 相対。`rules/arch_map.py` の `CHECKER`）。
/// 無い base もある。
pub const ARCH_MAP_CHECKER: &str = "scripts/dev/check-architecture-map.py";
/// 検査器と一緒に取り出す file（repo 相対）。検査器が自分の dir の外（`<検査器の dir>/..`）から読む file は全部ここに
/// 置く。抜けると base の検査器を一時 dir に出したときだけ、その rule が `checker_missing` の unsupported になる
/// （ADR 2026-11-08-health-enforcement-loop 付記 2026-11-10 hotfix）。
pub const CHECKER_SIDE_FILES: &[&str] = &[SOURCE_SIZE_REPORT, ARCH_MAP_CHECKER];
/// repo の profile の dir（repo 相対）。無ければ組み込み generic で評価する（契約 D2）。
pub const PROFILES_DIR: &str = "scripts/dev/repo-health/profiles";
/// [`HealthEvidence::blocking`] に残す fingerprint の数の上限（D2.1。超えた数は `reason` に書く）。
pub const MAX_BLOCKING: usize = 50;

/// 検査器の出どころ（D1.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckerSource {
    /// 評価する repo の base の commit にある検査器。
    Base,
    /// Celeris の release に同梱した検査器（`<release>/repo-health/`）。
    Bundled,
}

/// 検査器の版。`sha256` は [`checker_sha256`] の値で、同じ commit の検査器なら base でも同梱でも同じ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckerId {
    pub source: CheckerSource,
    pub sha256: String,
}

/// [`applicability`] の入力。repo の事実だけを持つ（J1。案件・repo 名・mode の欄を足さない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthRepoFacts {
    /// 作業ツリーが Git 管理（`git rev-parse --show-toplevel` が通る）。`false` は Git 管理でないと確かめられた
    /// ときだけ（[`repo_facts`]。確かめられない失敗は `unreadable`）。
    pub git: bool,
    /// Celeris が branch を統合する作業ツリー（ssh の remote workspace は false。ADR-0079 D5）。
    pub integrated_by_celeris: bool,
    /// base の commit にある検査器（無いと確かめられたら `None`）。
    pub base_checker: Option<CheckerId>,
    /// Celeris の release に同梱した検査器（release dir に無ければ `None`）。
    pub bundled_checker: Option<CheckerId>,
    /// 事実を読めなかった理由（git を起動できない、base の有無を確かめられない、など）。`Some` なら他の欄は読めた
    /// 所までで、当てるかを決められない。
    pub unreadable: Option<String>,
}

/// health を当てない理由（D1.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotApplicableReason {
    /// Git 管理でない作業場所。
    NotAGitRepo,
    /// Celeris が統合しない remote workspace（候補の SHA が無く、止める統合が Celeris に無い）。
    RemoteWorkspace,
}

impl NotApplicableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            NotApplicableReason::NotAGitRepo => "not_a_git_repo",
            NotApplicableReason::RemoteWorkspace => "remote_workspace",
        }
    }
}

/// 当てるべきだが評価できない理由（D1.1）。合格にしない（J4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnavailableReason {
    /// base にも同梱にも検査器が無い。
    CheckerMissing,
    /// 事実を読めなかった（理由の文を持つ）。
    FactsUnreadable(String),
}

impl std::fmt::Display for UnavailableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnavailableReason::CheckerMissing => f.write_str("checker_missing"),
            UnavailableReason::FactsUnreadable(why) => write!(f, "facts_unreadable:{why}"),
        }
    }
}

/// [`applicability`] の結果（D1.1・D1.2）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthApplicability {
    /// health の項目を必須検査の列に足し、`checker` で評価する。
    Applies { checker: CheckerId },
    /// health の項目を足さない（理由は証跡に残す）。
    NotApplicable { reason: NotApplicableReason },
    /// health の項目を `error` の結果として足す（合格にしない）。
    Unavailable { reason: UnavailableReason },
}

impl HealthApplicability {
    /// 証跡に残す理由の文（`Applies` は `None`）。
    pub fn reason(&self) -> Option<String> {
        match self {
            HealthApplicability::Applies { .. } => None,
            HealthApplicability::NotApplicable { reason } => Some(reason.as_str().to_string()),
            HealthApplicability::Unavailable { reason } => Some(reason.to_string()),
        }
    }
}

/// repo health を当てるか（D1.1）。純粋関数で、全経路がこれを呼ぶ（J1）。
///
/// 順序: remote workspace → 読めない事実 → Git 管理でない → base の検査器 → 同梱の検査器 → どちらも無い。
/// 読めない事実は同梱の検査器があっても `Unavailable`（base の検査器が無いと確かめられないので、base を優先する
/// J3 を守れない）。
pub fn applicability(facts: &HealthRepoFacts) -> HealthApplicability {
    if !facts.integrated_by_celeris {
        return HealthApplicability::NotApplicable {
            reason: NotApplicableReason::RemoteWorkspace,
        };
    }
    if let Some(why) = &facts.unreadable {
        return HealthApplicability::Unavailable {
            reason: UnavailableReason::FactsUnreadable(why.clone()),
        };
    }
    if !facts.git {
        return HealthApplicability::NotApplicable {
            reason: NotApplicableReason::NotAGitRepo,
        };
    }
    match facts
        .base_checker
        .as_ref()
        .or(facts.bundled_checker.as_ref())
    {
        Some(checker) => HealthApplicability::Applies {
            checker: checker.clone(),
        },
        None => HealthApplicability::Unavailable {
            reason: UnavailableReason::CheckerMissing,
        },
    }
}

/// git が「Git 管理でない」と答えたときの stderr の語句。`git rev-parse` が作業ツリーを親 dir まで探して見つから
/// なかったときの 2 つの文（`not a git repository (or any of the parent directories): .git` と、file system の境界で
/// 止まった `not a git repository (or any parent up to mount point …)`）だけに合う。壊れた gitfile・`GIT_DIR` の
/// 誤り（`not a git repository: '<path>'`）・所有者の検査（`dubious ownership`）・時間切れ・起動失敗には合わない。
/// git の文が翻訳される locale でも合わないが、その場合は `unreadable` になり合格にはならない（fail closed）。
const NOT_A_GIT_REPO_STDERR: &str = "not a git repository (or any ";

/// [`applicability`] の事実を集める（D1.1）。`git` は repo で git を実行して stdout を返す closure
/// （[`super::candidate_required_checks`] と同じ形。非 0 exit は stderr を、時間切れ・起動失敗はその理由を `Err` に
/// 入れる）。`bundled` は release に同梱した検査器（source は [`CheckerSource::Bundled`]）。remote workspace
/// （`integrated_by_celeris == false`）では git を呼ばない（手元に統合する作業ツリーが無い）。
///
/// `git rev-parse --show-toplevel` の失敗は、git が「Git 管理でない」と答えたとき（[`NOT_A_GIT_REPO_STDERR`]）だけ
/// `git = false` にする。それ以外の失敗（時間切れ・起動失敗・壊れた作業ツリーなど）は Git 管理でないと確かめられて
/// いないので `unreadable` に理由を書く（fail closed）。読めない事実は全て `unreadable` に理由を書く。
pub fn repo_facts(
    integrated_by_celeris: bool,
    base_sha: &str,
    bundled: Option<&CheckerId>,
    mut git: impl FnMut(&[&str]) -> Result<String, String>,
) -> HealthRepoFacts {
    let mut facts = HealthRepoFacts {
        git: false,
        integrated_by_celeris,
        base_checker: None,
        bundled_checker: bundled.cloned(),
        unreadable: None,
    };
    if !integrated_by_celeris {
        return facts;
    }
    if let Err(error) = git(&["rev-parse", "--show-toplevel"]) {
        if !error.contains(NOT_A_GIT_REPO_STDERR) {
            facts.unreadable = Some(format!(
                "git rev-parse --show-toplevel を読めない: {}",
                error.trim()
            ));
        }
        return facts;
    }
    facts.git = true;
    match base_checker(base_sha, &mut git) {
        Ok(found) => facts.base_checker = found,
        Err(error) => facts.unreadable = Some(error),
    }
    facts
}

/// base の commit `base_sha` に `path` があるか。無いと確かめられたときだけ `Ok(false)`（`cat-file` の失敗だけでは
/// 無いのか壊れているのか分からないので、`ls-tree` で base を一覧でき、その path が無いことを確かめる）。
pub fn base_has_file(
    base_sha: &str,
    path: &str,
    mut git: impl FnMut(&[&str]) -> Result<String, String>,
) -> Result<bool, String> {
    let object = format!("{base_sha}:{path}");
    if git(&["cat-file", "-e", &object]).is_ok() {
        return Ok(true);
    }
    let listed = git(&["ls-tree", "--name-only", base_sha, "--", path])
        .map_err(|e| format!("{object} の有無を確かめられない: {e}"))?;
    if listed.trim().is_empty() {
        Ok(false)
    } else {
        Err(format!("{object} は一覧にあるが読めない"))
    }
}

/// base の commit にある検査器の版（無いと確かめられたら `None`）。式は [`checker_sha256`] で、同梱の版と同じ。
pub fn base_checker(
    base_sha: &str,
    mut git: impl FnMut(&[&str]) -> Result<String, String>,
) -> Result<Option<CheckerId>, String> {
    if !base_has_file(base_sha, CHECKER_SCRIPT, &mut git)? {
        return Ok(None);
    }
    let tree = object_id(base_sha, CHECKER_DIR, &mut git)?;
    let side = if base_has_file(base_sha, SOURCE_SIZE_REPORT, &mut git)? {
        Some(object_id(base_sha, SOURCE_SIZE_REPORT, &mut git)?)
    } else {
        None
    };
    Ok(Some(CheckerId {
        source: CheckerSource::Base,
        sha256: checker_sha256(&tree, side.as_deref()),
    }))
}

/// `git rev-parse --verify <base>:<path>` の object id（hex でなければ拒否）。
fn object_id(
    base_sha: &str,
    path: &str,
    mut git: impl FnMut(&[&str]) -> Result<String, String>,
) -> Result<String, String> {
    let object = format!("{base_sha}:{path}");
    let id = git(&["rev-parse", "--verify", &object])
        .map_err(|e| format!("{object} の object id を読めない: {e}"))?;
    let id = id.trim();
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{object} の object id が壊れている: {id:?}"));
    }
    Ok(id.to_string())
}

/// health の項目の判定（D2.1・D3.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthVerdict {
    /// report の exit 0。
    Pass,
    /// exit 1 で、policy の file だけの変更ではない。
    Fail,
    /// exit 2・report が無い・時間切れ・壊れた report・評価できない（`Unavailable`）。合格にしない（J4）。
    Error,
    /// exit 1 で、fail 級の finding が policy の file の変更だけ（G2 D6.2）。
    PolicyOnly,
    /// 適用判定が `NotApplicable`（`reason` に理由）。
    NotApplicable,
}

/// health の結果を候補に結び付ける証跡（D2.1）。束縛は `(repo_id, base_sha, head_sha, head_tree, policy_sha256,
/// checker.sha256)` と `report_sha256`（J2）。作れなかった値（report の無い Error など）は空文字列か `None`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthEvidence {
    pub repo_id: RepoId,
    /// 評価の base の commit。
    pub base_sha: String,
    /// 評価した候補の commit。
    pub head_sha: String,
    /// `git rev-parse <head_sha>^{tree}`。
    pub head_tree: String,
    /// [`policy_sha256`] の値。
    pub policy_sha256: String,
    /// 評価に使った検査器（`NotApplicable` と、検査器の無い `Error` は `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checker: Option<CheckerId>,
    /// report の file の byte 列の sha256（report が無ければ空）。
    pub report_sha256: String,
    /// report の `tool.version`（report が無ければ空）。
    pub tool_version: String,
    pub verdict: HealthVerdict,
    /// fail 級の finding の fingerprint（sort 済み、[`MAX_BLOCKING`] 件まで）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocking: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 同じ tree の別の commit の証跡を写したときの元の `head_sha`（D2.5）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reused_from: Option<String>,
    /// report の `summary.reviewable`。false（dirty な作業ツリー）の証跡は再利用と delivery に使わない（D2.7）。
    pub reviewable: bool,
}

/// Blocking finding の説明。fail 級かどうかの正本は `HealthEvidence.blocking` だけ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HealthBlockingDetail {
    pub fingerprint: String,
    pub rule_id: String,
    pub paths: Vec<String>,
    /// report に message が無ければ空文字列。
    #[serde(default)]
    pub message: String,
}

/// 検査器の版（D2.3）: domain `celeris.repo-health.checker/1` で、[`CHECKER_DIR`] の tree id と
/// [`SOURCE_SIZE_REPORT`] の blob id（無ければ `absent`）を長さ付きで入れた sha256 hex。base の版は
/// `git rev-parse <base>:<path>` の値、同梱の版は `CHECKER_TREE` の値を渡す。
pub fn checker_sha256(checker_tree: &str, source_size_report_blob: Option<&str>) -> String {
    domain_sha256(
        "celeris.repo-health.checker/1",
        [
            CHECKER_DIR,
            checker_tree,
            SOURCE_SIZE_REPORT,
            source_size_report_blob.unwrap_or("absent"),
        ],
    )
}

/// [`policy_sha256`] の入力（D2.3）。report/1 の欄と base の profile の tree id。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyIdentity<'a> {
    /// report の `policy.version`。
    pub policy_version: &'a str,
    /// report の `policy.source`。
    pub policy_source: &'a str,
    /// report の `inputs.profile.resolved`。
    pub profile_resolved: &'a str,
    /// report の `inputs.profile.source`。
    pub profile_source: &'a str,
    /// base の [`PROFILES_DIR`] の tree id（無ければ `None` で、`builtin` として入れる）。
    pub profiles_tree: Option<&'a str>,
    /// report の `policy.files[].base_blob`（report の順。`null` は `None`）。
    pub base_blobs: &'a [Option<String>],
}

/// policy の指紋（D2.3）: domain `celeris.repo-health.policy/1` で、[`PolicyIdentity`] の欄を順に長さ付きで入れた
/// sha256 hex。policy は base 側で評価されるので、同じ base なら同じ値になる。
pub fn policy_sha256(policy: &PolicyIdentity<'_>) -> String {
    let head = [
        policy.policy_version,
        policy.policy_source,
        policy.profile_resolved,
        policy.profile_source,
        policy.profiles_tree.unwrap_or("builtin"),
    ];
    let blobs = policy
        .base_blobs
        .iter()
        .map(|blob| blob.as_deref().unwrap_or("absent"));
    domain_sha256(
        "celeris.repo-health.policy/1",
        head.into_iter().chain(blobs),
    )
}

/// `domain` と長さ付きの `fields` の sha256 hex（区切りの曖昧さが無い形）。
pub(super) fn domain_sha256<'a>(domain: &str, fields: impl IntoIterator<Item = &'a str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    for field in fields {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "health_tests.rs"]
mod tests;
