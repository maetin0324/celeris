//! Reviewer（DESIGN §5.7, ADR-0005 D5, ADR-0007 D4/D5）。
//!
//! - `Command` はワーカーの自己申告を信じず、ワークスペースで実際に再実行する。
//! - `ArtifactExists` はファイル存在と sha256。
//! - `Plan` kind は追加で `artifacts/plan.json` を `PlanOutput` として決定的に検証する（暗黙の条件、
//!   `criterion_idx = acceptance.len()`）。
//! - `Reviewer` は、決定的条件が全て pass のときだけ、ワーカーアダプタ経由の **別 run**（合成した `Review` kind
//!   タスク + `context.review`）を起動し、`artifacts/review.json` の判定を採る。**このモジュールは LLM を呼ばない**
//!   （アダプタに run を依頼するだけ。どのアダプタ／プロバイダを使うかはディスパッチャが tick で決める）。
//! - `Human` は、ディスパッチャが `Approval` 子タスクの結果を `ReviewExtras::human` として渡す
//!   (ADR-0008 D2)。このモジュール自身はストアに触れない。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use task_core::plan::{PlanLimits, PlanOutput, parse_and_validate};
use task_core::{
    ArtifactRef, Check, GenreSpec, Status, Task, TaskId, TaskKind, Tier, Usage, WorkerHint,
};
use task_worker::artifact::sha256_file;

use crate::policy::ProviderOutcome;
use task_worker::protocol::{ReviewCheckResult, ReviewDecision};
use task_worker::{
    Answer, EventSink, Evidence, PROTOCOL_VERSION, ReviewOutput, ReviewRequest, RunContext,
    RunLimits, RunRequest, Terminal, WorkerAdapter, Workspace,
};

/// 条件 1 件の判定結果。`Event::ReviewVerdict` にそのまま写す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub criterion_idx: usize,
    pub pass: bool,
    pub reason: String,
    /// ADR-0072 D16（Phase E4）: `Check::Reviewer` の不合格に reviewer 自身が添えた repair のヒント
    /// （`review.json` の `repair`）。決定的条件（`Command`/`ArtifactExists`/…）の verdict では常に `None`。
    pub repair_hint: Option<task_core::execution::ReviewRepairHint>,
}

/// 対象 run の `done` の内容（`Reviewer` run に `context.review` として渡す。ADR-0007 D5）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewSubject {
    pub summary: String,
    pub evidence: Vec<Evidence>,
}

/// `Reviewer` 条件のためにディスパッチャが選んだ run（ADR-0007 D5 1./3.）。
pub struct ReviewerRun {
    pub node: Option<task_worker::NodeContext>,
    pub profile: Option<task_core::EffectiveProfile>,
    /// Skills mounted by the subject task's assignee for review (ADR-0122 D4).
    pub skills: Vec<task_worker::protocol::SkillMount>,
    pub adapter: Arc<dyn WorkerAdapter>,
    pub run_id: String,
    pub limits: RunLimits,
    pub sink: Box<dyn EventSink>,
    /// 合成 `Review` タスクの `worker_hint`（設定 `[reviewer]`。ADR-0010 D9）。
    pub hint: WorkerHint,
    /// Phase 38（ADR-0028 追記）: レビュー対象のタスクの分野の manifest（ディスパッチャが `[[genres]]` と
    /// `[[roles]]` から決定的に組む）。ハーネスで動く分野なら、レビュアーのプロンプトに「成果物の名前は
    /// 固定」の規約が出る（`claude_code::harness_artifacts_section_for_review`）。
    pub subject_genre: Option<task_worker::GenreContext>,
    /// ADR-0054 D1（Phase 67）: 部署の根ノード（engineering/research/operations）の**継続セッション**
    /// （`kind = lead`）。対象タスクに部署が無い、またはアダプタが継続に対応しないときは `None`
    /// （前置きは Phase 66 までとバイト単位で同じ）。ディスパッチャが `resolve_node_session` で決める。
    pub session: Option<task_worker::protocol::SessionHandle>,
    /// ADR-0054 D1: `session` が継続中（`resume = true`）なら前回以降の差分、新規（`resume = false`）で
    /// 要約が要るときは前セッションの要約。どちらでもなければ空。
    pub session_diff: Vec<String>,
}

/// `Plan` kind の検証パラメータ（ADR-0007 D2/D4, ADR-0028 D3）。
#[derive(Debug, Clone)]
pub struct PlanCheck {
    /// その Plan 自身を含む祖先 Plan の数。
    pub depth: u32,
    pub limits: PlanLimits,
    /// ADR-0028 D3: `PlanOutput.tasks[].genre` / `role` の整合検証に使う（`[[genres]]`）。
    pub genres: Vec<GenreSpec>,
    /// ADR-0043 D2: その案件のリポジトリの名前（計画の `tasks[].repos` の検証に使う）。
    /// 案件にリポジトリが無ければ空で、そのとき `repos` を書いた計画は差し戻される。
    pub repos: Vec<String>,
}

/// `Reviewer` run が供給側の失敗で判定できなかったこと（ADR-0010 D5, P-29）。
///
/// Phase 113（ADR-0054 追記）: `outcome` を `Option` にした。`Some` は `classify_provider_failure`/
/// `provider_failure_outcome` が分類できた、プロバイダ・アカウントの cooldown に値する失敗
/// （Throttled/Exhausted/AuthFailed。従来どおりディスパッチャが `self.policy.report`/
/// `record_account_failure` を呼ぶ）。`None` は分類できなかった「reviewer run 自身のインフラ都合の
/// 失敗」（`is_error` の結果・プロセス失敗・resume 拒否・分類できなかったレート制限文言など。ADR-0054
/// Phase 113 D2）で、こちらはプロバイダ/アカウントを cooldown にする理由にはしない（成果物の問題でも
/// プロバイダの供給側の問題でもなく、たまたまこの run が失敗しただけなので）。どちらも判定は無効で、
/// ディスパッチャは遷移を適用せず `reviewing` のまま延期する点は変わらない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerProviderFailure {
    pub outcome: Option<ProviderOutcome>,
    pub message: String,
}

/// `review_task` の結果。
/// ADR-0061（Phase 104）: `ReviewerRunRecord` が `Eq` を落とした（`Usage.cost_usd: Option<f64>`）
/// ので、それを含むこの構造体も `Eq` を落とした。
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewOutcome {
    pub verdicts: Vec<Verdict>,
    /// `Plan` kind で検証に通った場合の出力（子タスクの生成に使う）。
    pub plan: Option<PlanOutput>,
    /// `Some` のとき判定は無効。ディスパッチャは遷移を適用せず `reviewing` のまま延期する（attempts を消費しない）。
    pub provider_failure: Option<ReviewerProviderFailure>,
    /// Reviewer run を起動した場合の、その run 自身の結果（ADR-0014 D1。ディスパッチャが `WorkerFinished{role: reviewer}` にする）。
    pub reviewer_run: Option<ReviewerRunRecord>,
    /// ADR-0118 D4 付記: 検査中に HEAD か target が review 前同期の snapshot から動いた。判定は無効で、
    /// ディスパッチャは attempts を消費せず再 sync → 再 check → 再 review に回す（上限は D4）。
    pub target_stale: Option<TargetStale>,
}

/// ADR-0118 D4 付記: 検査後に review snapshot が変わったリポジトリ（`ReviewTargetAdvanced` に残す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetStale {
    pub repo_id: task_core::RepoId,
    pub reviewed_sha: String,
    pub target_sha: String,
    pub reason: String,
}

/// Reviewer run 自身の終わり方（ADR-0014 D1）。
/// ADR-0061（Phase 104）: `Usage.cost_usd: Option<f64>` を追加したので `Eq` は落とした
/// （`f64` は `Eq` を持てない。`PartialEq` は変わらず使える）。
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewerRunRecord {
    pub run_id: String,
    /// `WorkerFinished.outcome` と同じ接頭辞の規則（`done: ` / `question: ` / `error(retryable=…): ` / `requeue: `）。
    pub outcome: String,
    /// `done` の `usage`（それ以外は `None`）。
    pub usage: Option<Usage>,
}

impl ReviewOutcome {
    pub fn all_pass(&self) -> bool {
        self.verdicts.iter().all(|v| v.pass)
    }
}

/// `Check::Human` の criterion idx ごとに解決した `(pass, reason)`（ADR-0008 D2）。
pub type HumanVerdicts = HashMap<usize, (bool, String)>;

/// 成果物ディレクトリの中のファイル名（ADR-0036 D2: 置き場はタスクごとの `artifacts_dir`）。
pub const PLAN_FILE_NAME: &str = "plan.json";
pub const REVIEW_FILE_NAME: &str = "review.json";

const REASON_TAIL: usize = 1024;

fn tail(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut start = s.len() - n;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// `task` に `Check::Reviewer` の条件があるか（ディスパッチャがレビュー run の要否を決めるのに使う）。
pub fn needs_reviewer_run(task: &Task) -> bool {
    task.acceptance
        .iter()
        .any(|c| matches!(c.check, Check::Reviewer))
}

/// `Reviewer` run に使う `WorkerHint`（DESIGN §5.7: `Standard` tier）。
pub fn reviewer_hint() -> WorkerHint {
    WorkerHint {
        tier: Tier::Standard,
        adapter: None,
    }
}

/// `review_task` の Phase 5 追加入力（ADR-0007）。
#[derive(Default)]
pub struct ReviewExtras {
    /// 対象 run の `done` の内容（`Reviewer` run に渡す）。
    pub subject: ReviewSubject,
    /// `task.kind == Plan` のときだけ `Some`。
    pub plan: Option<PlanCheck>,
    /// `Reviewer` 条件があり、ディスパッチャが run を用意できたときだけ `Some`。
    pub reviewer: Option<ReviewerRun>,
    /// `Check::Human` の各 criterion idx について、ディスパッチャが `Approval` 子タスクの終端状態から
    /// 解決した `(pass, reason)`（ADR-0008 D2）。呼び出し元は `task.acceptance` の全 `Human` criterion が
    /// 解決済みのときだけ `review_task` を呼ぶ想定（未解決分を待つ間はレビュー全体を延期する）。
    pub human: HumanVerdicts,
    /// ADR-0016 D3 / M4: 集約 run のレビューなら true。暗黙の条件「`<artifacts_dir>/summary.md` が存在する」を
    /// `criterion_idx = acceptance.len()`（Plan の暗黙条件があればその次）に加える。
    pub aggregate: bool,
    /// ADR-0043 D4: 先頭のリポジトリの `.config/celeris/workspace.toml` の `[commands] check`。
    /// **タスクの `acceptance` に `Check::Command` が 1 つも無いときだけ**、暗黙の条件として
    /// `exit 0` を期待して実行する（タスクに明示があればそれが勝つ）。空なら何もしない。
    ///
    /// ADR-0046 D4（Phase 59）: `mode = prototype` のタスクではディスパッチャがこれを**空にする**
    /// （「明示の受け入れ条件だけ。リポジトリの check は使わない」）。
    pub repo_checks: Vec<String>,
    /// ADR-0046 D4（Phase 59）: `mode = research` のタスクだけ true。暗黙の条件として
    /// 「結果に出典（`sources`）か計測の記録がある」を足す（[`research_evidence`] が決定的に判定する）。
    pub research: bool,
    /// ADR-0117 D1: 対象 task と祖先で回答済みの人の決定（ディスパッチャが store から集める）。
    /// reviewer run の `ReviewRequest.decisions` にそのまま渡す。
    pub decisions: Vec<ReviewDecision>,
    /// ADR-0117 D1: 対象 task と祖先の `question` への人の回答（`ReviewRequest.answers`）。
    pub answers: Vec<Answer>,
}

/// ADR-0117 D2: `ReviewCheckResult.reason` に残す末尾の文字数。
const CHECK_REASON_TAIL_CHARS: usize = 1000;

fn tail_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        return s.to_string();
    }
    s.chars().skip(count - n).collect()
}

/// ADR-0117 D2: reviewer より先に出た決定的な verdict を `ReviewCheckResult` に写す。
/// `implicit` は暗黙の条件（plan / aggregate / repo check / research）の criterion idx ごとの `(kind, cmd)`。
fn deterministic_check_results(
    task: &Task,
    verdicts: &[Verdict],
    implicit: &HashMap<usize, (&'static str, Option<String>)>,
) -> Vec<ReviewCheckResult> {
    verdicts
        .iter()
        .map(|v| {
            let (criterion, kind, cmd) = match task.acceptance.get(v.criterion_idx) {
                Some(c) => {
                    let (kind, cmd) = match &c.check {
                        Check::Command { cmd, .. } => ("command", Some(cmd.clone())),
                        Check::ArtifactExists { .. } => ("artifact_exists", None),
                        Check::KnowledgePage { .. } => ("knowledge_page", None),
                        Check::Human => ("human", None),
                        Check::Reviewer => ("reviewer", None),
                    };
                    (Some(v.criterion_idx), kind, cmd)
                }
                None => match implicit.get(&v.criterion_idx) {
                    Some((kind, cmd)) => (None, *kind, cmd.clone()),
                    None => (None, "implicit", None),
                },
            };
            ReviewCheckResult {
                criterion,
                kind: kind.to_string(),
                cmd,
                pass: v.pass,
                reason: tail_chars(&v.reason, CHECK_REASON_TAIL_CHARS),
            }
        })
        .collect()
}

/// ADR-0046 D4（Phase 59）: `mode = research` の暗黙の条件。**決定的**（LLM は使わない）。
///
/// `<artifacts_dir>/result.json` を読み、次のどれかがあれば合格:
/// - `sources` が非空の配列（トップレベル、または `done` の下）
/// - `measurements` が非空の配列（計測の記録。同上）
/// - `<artifacts_dir>/sources.json` が非空の配列（PaperQA2 / Local Deep Research が書くファイル）
pub fn research_evidence(artifacts_dir: &Path, artifacts_rel: &str) -> (bool, String) {
    let result_path = artifacts_dir.join("result.json");
    let result_rel = format!("{artifacts_rel}/result.json");
    if let Ok(text) = std::fs::read_to_string(&result_path)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
    {
        for key in ["sources", "measurements"] {
            if let Some(n) = non_empty_array_len(&value, key) {
                return (
                    true,
                    format!("{result_rel}: {key} に {n} 件（mode = research）"),
                );
            }
            if let Some(done) = value.get("done")
                && let Some(n) = non_empty_array_len(done, key)
            {
                return (
                    true,
                    format!("{result_rel}: done.{key} に {n} 件（mode = research）"),
                );
            }
        }
    }
    let sources_path = artifacts_dir.join("sources.json");
    if let Ok(text) = std::fs::read_to_string(&sources_path)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
    {
        let n = match &value {
            serde_json::Value::Array(items) => items.len(),
            other => other
                .get("sources")
                .and_then(|s| s.as_array())
                .map(|a| a.len())
                .unwrap_or(0),
        };
        if n > 0 {
            return (
                true,
                format!("{artifacts_rel}/sources.json: 出典 {n} 件（mode = research）"),
            );
        }
    }
    (
        false,
        format!(
            "mode = research だが、結果に出典も計測の記録も無い（{result_rel} の `sources` / `measurements`、             または {artifacts_rel}/sources.json のどれかに 1 件以上必要）"
        ),
    )
}

fn non_empty_array_len(value: &serde_json::Value, key: &str) -> Option<usize> {
    let items = value.get(key)?.as_array()?;
    if items.is_empty() {
        None
    } else {
        Some(items.len())
    }
}

/// ADR-0074 D6.1（Phase F1）: 2 倍の timeout の上限（1,800 秒。無限ループの検査で無駄になる時間を
/// 抑える。U-F9）。
const REVIEW_TIMEOUT_RETRY_CAP_SECS: u64 = 1800;

/// D6.2: `git merge-base --is-ancestor <ref> HEAD` の `<ref>`（決定的な字句解析。cmd に
/// `merge-base` と `--is-ancestor` の両方を含むときだけ、`--is-ancestor` の次のトークンを返す）。
fn merge_base_ancestor_ref(cmd: &str) -> Option<String> {
    let lower = cmd.to_lowercase();
    if !lower.contains("merge-base") || !lower.contains("--is-ancestor") {
        return None;
    }
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let idx = tokens.iter().position(|&t| t == "--is-ancestor")?;
    tokens.get(idx + 1).map(|s| s.trim_matches('"').to_string())
}

/// ADR-0079 D6（Phase R1c）: 木の子 task の最終レビューで「既定のブランチ」と読む ref。子の検査では
/// これらを親のブランチに置き換える（子は main ではなく親のブランチに取り込まれる）。
const DEFAULT_BRANCH_REFS: &[&str] = &[
    "main",
    "master",
    "origin/main",
    "origin/master",
    "refs/heads/main",
    "refs/heads/master",
    "origin/HEAD",
];

/// ADR-0079 D6: 子の最終レビューの検査で置き換えた先（見出しの文言にも使う）。
pub const TREE_CHILD_REVIEW_HEADING: &str = "## 取り込み先（ADR-0079 D6）";

/// ADR-0079 D6（Phase R1c）: `cmd` の `merge-base --is-ancestor <ref>` の `<ref>` が既定のブランチ
/// （[`DEFAULT_BRANCH_REFS`]、引用符付きも）なら `parent_branch` に置き換える（すべての出現。決定的な字句の
/// 置き換えで、他の部分は 1 バイトも変えない）。当たらなければ `None`。
pub fn rebase_merge_base_ref(cmd: &str, parent_branch: &str) -> Option<String> {
    let lower = cmd.to_lowercase();
    if !lower.contains("merge-base") || !lower.contains("--is-ancestor") {
        return None;
    }
    const FLAG: &str = "--is-ancestor";
    let mut out = String::with_capacity(cmd.len() + parent_branch.len());
    let mut rest = cmd;
    let mut changed = false;
    while let Some(pos) = rest.find(FLAG) {
        let after_flag = pos + FLAG.len();
        out.push_str(&rest[..after_flag]);
        let tail = &rest[after_flag..];
        let ws_len = tail.len() - tail.trim_start().len();
        let token_start = &tail[ws_len..];
        let token_len = token_start
            .find(char::is_whitespace)
            .unwrap_or(token_start.len());
        let token = &token_start[..token_len];
        out.push_str(&tail[..ws_len]);
        let bare = token.trim_matches(|c| c == '"' || c == '\'');
        if ws_len > 0 && DEFAULT_BRANCH_REFS.contains(&bare) {
            out.push_str(parent_branch);
            changed = true;
        } else {
            out.push_str(token);
        }
        rest = &token_start[token_len..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// ADR-0079 D6（Phase R1c）: 最終レビューに渡す対象の task。木の子 task（`tree.parent_unit` を持つ）なら、
/// - `Check::Command` の `merge-base --is-ancestor <main 等>` を親のブランチ `parent_branch` に置き換え
///   （[`rebase_merge_base_ref`]。条件の数と順は変えないので `criterion_idx` はそのまま）、
/// - 目的（reviewer run の前置きに出る）の末尾に、取り込み先と差分の基点を固定の書式で足す
///   （[`TREE_CHILD_REVIEW_HEADING`]。main が進んでいても不合格の理由にしない）。
///
/// 親のブランチは段階の途中では動かない（動くのは段階末尾の統合だけ）ので、子の最終レビュー中に基点が
/// ずれない（人の決定 4）。木の子でなければ `task` をそのまま返す（root は今どおり main と比べる）。
pub fn tree_child_review_view(task: Task, branch_prefix: &str) -> Task {
    review_view(task, branch_prefix, None)
}

/// ADR-0079 R5b-fix2: remote workspace の木の子の取り込み先の注記（ブランチ統合が無いので親のブランチの
/// 行は出さない）。
pub const TREE_CHILD_REMOTE_NOTE: &str =
    "remote workspace: ブランチ統合なし。検査はクラスタ側の worktree（remote-exec）で行う";

/// 最終レビューに渡す対象の task（[`tree_child_review_view`] に remote の扱いを足したもの）。
///
/// - 木の子で local: 従来どおり（`merge-base` の相手を親のブランチに置き換え、取り込み先を足す）。
/// - 木の子で remote（`remote` が `Some` か、`task.workspace` が `Remote`）: ADR-0079 D5 のとおり
///   remote は merge しないので、親のブランチ・基点の行は出さず [`TREE_CHILD_REMOTE_NOTE`] を足す
///   （`merge-base` も書き換えない。クラスタ側に親のブランチは無い）。R5b-fix2。
/// - `remote` が `Some` なら（木の子かどうかに関わらず）、worker と同じ `.celeris/remote-exec` の
///   指示を reviewer 向けの文面で足す（`task_worker::remote_exec_reviewer_instructions`）。
pub fn review_view(
    task: Task,
    branch_prefix: &str,
    remote: Option<&task_worker::SshSettings>,
) -> Task {
    let is_remote =
        remote.is_some() || matches!(task.workspace, task_core::WorkspaceSpec::Remote { .. });
    let mut view = match task_core::tree::parent_branch(&task, branch_prefix) {
        None => task,
        Some(_) if is_remote => {
            let mut view = task;
            view.objective.push_str(&format!(
                "\n\n{TREE_CHILD_REVIEW_HEADING}\n\
                 この task は親 task の計画の段階の子 task。{TREE_CHILD_REMOTE_NOTE}。\
                 main や親 task のブランチとの関係は不合格の理由にしない。\n"
            ));
            view
        }
        Some(parent_branch) => {
            let base = task_core::tree::child_base_commit(&task)
                .map(|sha| sha.chars().take(12).collect::<String>())
                .unwrap_or_else(|| "-".to_string());
            let mut view = task;
            for criterion in &mut view.acceptance {
                if let Check::Command { cmd, .. } = &mut criterion.check
                    && let Some(rewritten) = rebase_merge_base_ref(cmd, &parent_branch)
                {
                    *cmd = rewritten;
                }
            }
            view.objective.push_str(&format!(
                "\n\n{TREE_CHILD_REVIEW_HEADING}\n\
                 この task は親 task の計画の段階の子 task。成果は親のブランチ `{parent_branch}` に親の段階末尾の統合で\
                 取り込まれ、main には直接取り込まれない。差分の基点は `{base}`（親の段階の基点）、merge-base / \
                 fast-forward の相手は `{parent_branch}`。main が進んでいることは不合格の理由にしない。\n"
            ));
            view
        }
    };
    if let Some(settings) = remote {
        view.objective
            .push_str(&task_worker::remote_exec_reviewer_instructions(settings));
    }
    view
}

/// 1 回だけ command を実行して判定する（`review_task`/`run_work_unit_checks` の元の判定ロジック）。
/// `label` は理由の頭に付ける接頭辞（`workspace.toml` の check だけ `"workspace.toml check: "`。
/// それ以外は空文字で、既存の文面と 1 バイトも変わらない）。
async fn exec_check_once(
    workspace: &dyn Workspace,
    cmd: &str,
    expect_exit: i32,
    timeout: Duration,
    label: &str,
) -> CheckOutcome {
    match workspace.exec(cmd, timeout).await {
        Err(e) => CheckOutcome {
            pass: false,
            reason: format!("exec failed: {e}"),
            exit: None,
            timed_out: false,
        },
        Ok(r) if r.timed_out => CheckOutcome {
            pass: false,
            reason: format!("command timed out after {}s: {cmd}", timeout.as_secs()),
            exit: None,
            timed_out: true,
        },
        Ok(r) => {
            let pass = r.exit == Some(expect_exit);
            CheckOutcome {
                pass,
                reason: format!(
                    "{label}cmd={cmd:?} exit={:?} expected={expect_exit} stdout_tail={:?} stderr_tail={:?}",
                    r.exit,
                    tail(&r.stdout_tail, REASON_TAIL),
                    tail(&r.stderr_tail, REASON_TAIL)
                ),
                exit: r.exit,
                timed_out: false,
            }
        }
    }
}

/// 決定的な検査 1 件の結果（判定と判定文に、最後に走らせたコマンドの exit・timeout を添える）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckOutcome {
    pub pass: bool,
    pub reason: String,
    pub exit: Option<i32>,
    pub timed_out: bool,
}

/// ADR-0074 D6.1/D6.2（Phase F1）: 決定的な検査を実行し、2 種類の技術的な不合格を daemon が
/// その場で直してから最終判定する（LLM は使わない）。
///
/// - **timeout**（`command timed out after` で不合格）: 同じコマンドを 1 回だけ、timeout を 2 倍
///   （上限 [`REVIEW_TIMEOUT_RETRY_CAP_SECS`]）にして再実行する。通れば判定を差し替える。それでも
///   timeout なら、2 倍の timeout を含む理由文言をそのまま返す（`execution::classify_review_failure`
///   が `review_timeout` の repair に倒す）。
/// - **`merge-base --is-ancestor <ref> HEAD` の不成立**: `git merge --no-edit <ref>` を 1 回試す。
///   衝突なく合流できれば同じ検査を再実行して判定を差し替える。衝突（または merge 自体の失敗）なら
///   `git merge --abort` して元の不合格のまま返す（`classify_review_failure` が `merge_base` の
///   repair に倒す）。
async fn exec_check_with_repair_retries(
    workspace: &dyn Workspace,
    cmd: &str,
    expect_exit: i32,
    timeout: Duration,
    label: &str,
) -> (bool, String) {
    let o = exec_check_outcome(workspace, cmd, expect_exit, timeout, label).await;
    (o.pass, o.reason)
}

/// [`exec_check_with_repair_retries`] と同じ判定（再実行・merge-base の修復を含む）で、exit・timeout も返す。
pub(crate) async fn exec_check_outcome(
    workspace: &dyn Workspace,
    cmd: &str,
    expect_exit: i32,
    timeout: Duration,
    label: &str,
) -> CheckOutcome {
    let first = exec_check_once(workspace, cmd, expect_exit, timeout, label).await;
    if first.pass {
        return first;
    }
    if first.reason.starts_with("command timed out after") {
        let doubled_secs = timeout
            .as_secs()
            .saturating_mul(2)
            .min(REVIEW_TIMEOUT_RETRY_CAP_SECS);
        if doubled_secs > timeout.as_secs() {
            return exec_check_once(
                workspace,
                cmd,
                expect_exit,
                Duration::from_secs(doubled_secs),
                label,
            )
            .await;
        }
        return first;
    }
    if let Some(ancestor_ref) = merge_base_ancestor_ref(cmd) {
        let merge_cmd = format!("git merge --no-edit {ancestor_ref}");
        let merged_cleanly = matches!(
            workspace.exec(&merge_cmd, timeout).await,
            Ok(r) if r.exit == Some(0)
        );
        if merged_cleanly {
            return exec_check_once(workspace, cmd, expect_exit, timeout, label).await;
        }
        // 衝突、または merge 自体が失敗した: 作業ツリーを元に戻し、元の不合格を返す
        // （repair WU に任せる。ここでは設計判断をしない）。
        let _ = workspace.exec("git merge --abort", timeout).await;
    }
    first
}

pub const SUMMARY_FILE_NAME: &str = "summary.md";

/// `task.acceptance` を順に判定する。`produced` はその run の `ArtifactProduced`（名前の照合に使う）。
/// `artifacts_dir` はそのタスクの成果物ディレクトリ（ADR-0036 D1。ディスパッチャが決める）。
/// `plan.json` / `review.json` / `summary.md` と `Check::ArtifactExists` の既定のパスはここを基準にする。
pub async fn review_task(
    task: &Task,
    workspace: &dyn Workspace,
    workspace_dir: &Path,
    artifacts_dir: &Path,
    produced: &[ArtifactRef],
    command_timeout: Duration,
    extras: ReviewExtras,
) -> ReviewOutcome {
    let artifacts_rel = task_core::artifacts::rel_from(workspace_dir, artifacts_dir);
    let ReviewExtras {
        subject,
        plan,
        reviewer,
        human,
        aggregate,
        repo_checks,
        research,
        decisions,
        answers,
    } = extras;
    let mut implicit: HashMap<usize, (&'static str, Option<String>)> = HashMap::new();
    let subject = &subject;
    let mut verdicts = Vec::with_capacity(task.acceptance.len() + 1);
    let mut reviewer_criteria: Vec<usize> = Vec::new();
    for (idx, criterion) in task.acceptance.iter().enumerate() {
        let (pass, reason) = match &criterion.check {
            Check::Command { cmd, expect_exit } => {
                exec_check_with_repair_retries(workspace, cmd, *expect_exit, command_timeout, "")
                    .await
            }
            Check::ArtifactExists { name } => {
                let rel = produced
                    .iter()
                    .rev()
                    .find(|a| &a.name == name)
                    .map(|a| a.path.clone())
                    .unwrap_or_else(|| format!("{artifacts_rel}/{name}"));
                let full = workspace_dir.join(&rel);
                if full.is_file() {
                    match sha256_file(&full) {
                        Ok(sha) => (true, format!("path={rel} sha256={sha}")),
                        Err(e) => (false, format!("path={rel} unreadable: {e}")),
                    }
                } else {
                    (false, format!("artifact {name:?} not found at {rel}"))
                }
            }
            // ADR-0067 D2: 知識ベースのページ参照。実在の検証は組み立て時にはしない（`ArtifactExists`
            // と違い、人が確認する時点で GUI がリンクを 404 で示せば気付ける。ADR-0067 §2 却下した案）。
            Check::KnowledgePage { path } => (
                true,
                format!("knowledge base page: {path} (existence not checked automatically)"),
            ),
            Check::Reviewer => {
                // 決定的条件の結果を見てから判定する（後段）。
                reviewer_criteria.push(idx);
                continue;
            }
            Check::Human => match human.get(&idx) {
                Some((pass, reason)) => (*pass, reason.clone()),
                // 呼び出し元（dispatcher::spawn_review）は Human criterion が全て解決してから呼ぶので
                // 通常到達しない防御的フォールバック（ADR-0008 D2）。
                None => (false, "human approval state missing".to_string()),
            },
        };
        verdicts.push(Verdict {
            criterion_idx: idx,
            pass,
            reason,
            repair_hint: None,
        });
    }

    // ADR-0007 D4: Plan kind は暗黙の条件「`<artifacts_dir>/plan.json` が PlanOutput として妥当」を追加する。
    let mut plan_output = None;
    if let Some(check) = &plan {
        let (pass, reason, parsed) = check_plan_file(artifacts_dir, &artifacts_rel, check);
        plan_output = parsed;
        implicit.insert(task.acceptance.len(), ("plan", None));
        verdicts.push(Verdict {
            criterion_idx: task.acceptance.len(),
            pass,
            reason,
            repair_hint: None,
        });
    }

    // ADR-0016 D3 / M4: 集約 run は `<artifacts_dir>/summary.md` を作っていなければならない（暗黙の条件）。
    if aggregate {
        let idx = task.acceptance.len() + usize::from(plan.is_some());
        let full = artifacts_dir.join(SUMMARY_FILE_NAME);
        let summary_rel = format!("{artifacts_rel}/{SUMMARY_FILE_NAME}");
        let (pass, reason) = if full.is_file() {
            match sha256_file(&full) {
                Ok(sha) => (true, format!("path={summary_rel} sha256={sha}")),
                Err(e) => (false, format!("path={summary_rel} unreadable: {e}")),
            }
        } else {
            (
                false,
                format!("aggregate run did not produce {summary_rel}"),
            )
        };
        implicit.insert(idx, ("aggregate_summary", None));
        verdicts.push(Verdict {
            criterion_idx: idx,
            pass,
            reason,
            repair_hint: None,
        });
    }

    // ADR-0043 D4: タスクが自分で検査コマンドを書いていないときだけ、リポジトリの
    // `[commands] check` を暗黙の条件として足す（タスクの `acceptance` の明示が勝つ）。
    let task_has_command = task
        .acceptance
        .iter()
        .any(|c| matches!(c.check, Check::Command { .. }));
    if !repo_checks.is_empty() && !task_has_command {
        let base = task.acceptance.len() + usize::from(plan.is_some()) + usize::from(aggregate);
        for (n, cmd) in repo_checks.iter().enumerate() {
            let (pass, reason) = exec_check_with_repair_retries(
                workspace,
                cmd,
                0,
                command_timeout,
                "workspace.toml check: ",
            )
            .await;
            implicit.insert(base + n, ("repo_check", Some(cmd.clone())));
            verdicts.push(Verdict {
                criterion_idx: base + n,
                pass,
                reason,
                repair_hint: None,
            });
        }
    }

    // ADR-0046 D4（Phase 59）: `mode = research` は「結果に出典か計測の記録があること」を暗黙の条件に足す。
    if research {
        let idx = task.acceptance.len()
            + usize::from(plan.is_some())
            + usize::from(aggregate)
            + if task
                .acceptance
                .iter()
                .any(|c| matches!(c.check, Check::Command { .. }))
            {
                0
            } else {
                repo_checks.len()
            };
        let (pass, reason) = research_evidence(artifacts_dir, &artifacts_rel);
        implicit.insert(idx, ("research_evidence", None));
        verdicts.push(Verdict {
            criterion_idx: idx,
            pass,
            reason,
            repair_hint: None,
        });
    }

    // ADR-0007 D5 2./3.: 決定的条件が全 pass のときだけ LLM レビュー run を起動する。
    let mut provider_failure = None;
    let mut reviewer_run = None;
    if !reviewer_criteria.is_empty() {
        let deterministic_ok = verdicts.iter().all(|v| v.pass);
        let reviewer_verdicts = if !deterministic_ok {
            reviewer_criteria
                .iter()
                .map(|&idx| Verdict {
                    criterion_idx: idx,
                    pass: false,
                    reason: "not evaluated: a deterministic check failed".to_string(),
                    repair_hint: None,
                })
                .collect()
        } else {
            match reviewer {
                None => reviewer_criteria
                    .iter()
                    .map(|&idx| Verdict {
                        criterion_idx: idx,
                        pass: false,
                        reason: "not evaluated: no reviewer run was available".to_string(),
                        repair_hint: None,
                    })
                    .collect(),
                Some(run) => {
                    let human_context = ReviewHumanContext {
                        decisions,
                        answers,
                        checks: deterministic_check_results(task, &verdicts, &implicit),
                    };
                    let (result, record) = run_reviewer(
                        task,
                        workspace_dir,
                        artifacts_dir,
                        &artifacts_rel,
                        produced,
                        subject,
                        &reviewer_criteria,
                        human_context,
                        run,
                    )
                    .await;
                    reviewer_run = Some(record);
                    match result {
                        Ok(v) => v,
                        Err(pf) => {
                            // ADR-0010 D5（P-29）: 判定そのものが無効。遷移はディスパッチャが適用しない。
                            provider_failure = Some(pf);
                            Vec::new()
                        }
                    }
                }
            }
        };
        verdicts.extend(reviewer_verdicts);
    }

    verdicts.sort_by_key(|v| v.criterion_idx);
    ReviewOutcome {
        verdicts,
        plan: plan_output,
        provider_failure,
        reviewer_run,
        target_stale: None,
    }
}

/// ADR-0072 D14/D6・E4 (g): WU の決定的な `checks`（`Check::Command` と同じ意味論の `Command` だけ）を
/// 実行する。`review_task` の `Check::Command` 分岐と同じ判定（`workspace.exec` でワークスペースの中で
/// 実際に再実行し、exit を比較する）を再利用する。呼び出し側（`dispatcher.rs`）は、1 つでも
/// `pass = false` があれば、その run を `RunEnd::Failed{retryable: true}`（WU の retry）として扱う。
pub async fn run_work_unit_checks(
    workspace: &dyn Workspace,
    checks: &[task_core::WorkUnitCheck],
    command_timeout: Duration,
) -> Vec<(bool, String)> {
    let mut out = Vec::with_capacity(checks.len());
    for c in checks {
        let (pass, reason) =
            exec_check_with_repair_retries(workspace, &c.cmd, c.expect_exit, command_timeout, "")
                .await;
        out.push((pass, reason));
    }
    out
}

fn check_plan_file(
    artifacts_dir: &Path,
    artifacts_rel: &str,
    check: &PlanCheck,
) -> (bool, String, Option<PlanOutput>) {
    let path = artifacts_dir.join(PLAN_FILE_NAME);
    let rel = format!("{artifacts_rel}/{PLAN_FILE_NAME}");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return (false, format!("{rel} not found or unreadable: {e}"), None),
    };
    match parse_and_validate(
        &text,
        check.depth,
        &check.limits,
        &check.genres,
        &check.repos,
    ) {
        Ok(plan) => {
            let n = plan.tasks.len();
            (
                true,
                format!("{rel} is a valid PlanOutput with {n} tasks"),
                Some(plan),
            )
        }
        Err(e) => (false, format!("{rel}: {e}"), None),
    }
}

/// 合成した `Review` kind のタスク（永続化しない。ADR-0007 D5 3.）。
pub fn synthetic_review_task(subject_task: &Task, run_id: &str, hint: &WorkerHint) -> Task {
    let now = time::OffsetDateTime::now_utc();
    Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: Some(subject_task.id),
        kind: TaskKind::Review,
        title: format!("Review: {}", subject_task.title),
        objective: subject_task.objective.clone(),
        acceptance: subject_task.acceptance.clone(),
        inputs: vec![],
        depends_on: vec![],
        status: Status::Running,
        priority: subject_task.priority,
        worker_hint: hint.clone(),
        workspace: subject_task.workspace.clone(),
        budget: subject_task.budget,
        // ADR-0046 D4（Phase 59）: 合成したレビューのタスクは対象タスクの進め方を継ぐ
        // （前置きに mode の規則が出る）。
        skills: Vec::new(),
        mode: subject_task.mode,
        attempts: 0,
        lease: Some(task_core::Lease {
            worker_run_id: run_id.to_string(),
            expires_at: now + time::Duration::seconds(subject_task.budget.max_wall_secs as i64),
        }),
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        // ADR-0033 D2（監査 D-3）: 派生タスクは親の案件・途中目標・担当を継ぐ。
        project_id: subject_task.project_id,
        milestone_id: subject_task.milestone_id,
        assignee: subject_task.assignee.clone(),
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

/// ADR-0117 D1/D2: `ReviewRequest` に載せる人の決定・回答と、先に出た決定的 check の結果。
struct ReviewHumanContext {
    decisions: Vec<ReviewDecision>,
    answers: Vec<Answer>,
    checks: Vec<ReviewCheckResult>,
}

/// Reviewer run を実行し、判定と、その run 自身の結果（`WorkerFinished` 用。ADR-0014 D1）を返す。
#[allow(clippy::too_many_arguments)]
async fn run_reviewer(
    task: &Task,
    workspace_dir: &Path,
    artifacts_dir: &Path,
    artifacts_rel: &str,
    produced: &[ArtifactRef],
    subject: &ReviewSubject,
    criteria: &[usize],
    human_context: ReviewHumanContext,
    run: ReviewerRun,
) -> (
    Result<Vec<Verdict>, ReviewerProviderFailure>,
    ReviewerRunRecord,
) {
    let mut record = ReviewerRunRecord {
        run_id: run.run_id.clone(),
        outcome: String::new(),
        usage: None,
    };
    let result = run_reviewer_inner(
        task,
        workspace_dir,
        artifacts_dir,
        artifacts_rel,
        produced,
        subject,
        criteria,
        human_context,
        run,
        &mut record,
    )
    .await;
    (result, record)
}

#[allow(clippy::too_many_arguments)]
async fn run_reviewer_inner(
    task: &Task,
    workspace_dir: &Path,
    artifacts_dir: &Path,
    artifacts_rel: &str,
    produced: &[ArtifactRef],
    subject: &ReviewSubject,
    criteria: &[usize],
    human_context: ReviewHumanContext,
    run: ReviewerRun,
    record: &mut ReviewerRunRecord,
) -> Result<Vec<Verdict>, ReviewerProviderFailure> {
    let fail_all = |reason: String| -> Result<Vec<Verdict>, ReviewerProviderFailure> {
        Ok(criteria
            .iter()
            .map(|&idx| Verdict {
                criterion_idx: idx,
                pass: false,
                reason: reason.clone(),
                repair_hint: None,
            })
            .collect())
    };
    let subject_genre = run.subject_genre.clone();
    let review_path = artifacts_dir.join(REVIEW_FILE_NAME);
    let review_rel = format!("{artifacts_rel}/{REVIEW_FILE_NAME}");
    // 前回のレビュー run の出力を今回の結果と誤読しない（ADR-0007 D1）。
    let _ = std::fs::remove_file(&review_path);

    let mut review_task = synthetic_review_task(task, &run.run_id, &run.hint);
    if let Some(node) = &run.node {
        review_task.assignee = Some(node.id.clone());
    }
    let req = RunRequest {
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: review_task,
        workspace: workspace_dir.to_path_buf(),
        // ADR-0041 D1: Reviewer run は判定だけで編集しないので、worktree ではなく対象タスクの
        // ディレクトリ（成果物と `tree/` がある場所）で動かす。
        work_dir: None,
        // ADR-0036 D2: レビューは対象タスクの成果物についての判定なので、対象タスクの成果物ディレクトリを使う
        // （`review.json` もその中に書かせる）。
        artifacts_dir: artifacts_dir.to_path_buf(),
        context: RunContext {
            node: run.node.clone(),
            profile: run.profile.clone(),
            skills: run.skills.clone(),
            prior_review: vec![],
            inputs: produced.to_vec(),
            // ADR-0117 D1: `RunContext.answers` は worker run 用。人の回答は `ReviewRequest.answers` に載せる。
            answers: vec![],
            review: Some(ReviewRequest {
                summary: subject.summary.clone(),
                evidence: subject.evidence.clone(),
                criteria: criteria.to_vec(),
                decisions: human_context.decisions,
                answers: human_context.answers,
                checks: human_context.checks,
            }),
            role: None,
            children: Vec::new(),
            // ADR-0027 D1: Reviewer run は委譲しない（M8 相当）。
            available_genres: Vec::new(),
            // Phase 38（ADR-0028 追記）: 対象タスクの分野の manifest だけは渡す（ハーネスで動く分野の
            // 成果物の名前は固定で、`papers.json` は検索コーパスであって答えではない、を伝えるため）。
            subject_genre: subject_genre.clone(),
            // ADR-0054 D1（Phase 67）: 部署の根ノードの継続セッション（`kind = lead`）。部署が無い・
            // アダプタが対応しない run では `None` / 空（Phase 66 までとバイト単位で同じ）。
            session: run.session.clone(),
            session_diff: run.session_diff.clone(),
            // ADR-0033 D4 / D6: Reviewer run は「人」ではなく独立した判定なので、役職・記憶・やり取り・
            // 組織図は渡さない（判定は成果物と条件だけで決める）。
            ..RunContext::default()
        },
    };
    let tag = format!("reviewer({})", run.run_id);
    run.sink
        .progress(&format!("started (adapter={})", run.adapter.id()));
    let outcome = match run
        .adapter
        .run(req, &run.run_id, run.limits, run.sink.as_ref())
        .await
    {
        Ok(o) => o,
        Err(e) => {
            // ADR-0054 D2（Phase 113）: アダプタが分類できた供給側失敗（Throttled/Exhausted/
            // AuthFailed）はこれまでどおり `Some(outcome)`。分類できなかったもの（`Io`/`Serde`/
            // `Other`。spawn 前の UUID 拒否や、プロセス起動そのものの失敗を含む）も、もう
            // `fail_all` しない — 「reviewer run 自身のインフラ都合の失敗」として `None` で返し、
            // ディスパッチャに `max_reviewer_retries` までのやり直しを任せる。
            run.sink.progress(&format!("adapter error: {e}"));
            let outcome = crate::dispatcher::provider_failure_outcome(&e);
            record.outcome = format!("requeue: adapter: {e}");
            return Err(ReviewerProviderFailure {
                outcome,
                message: format!("{tag}: {e}"),
            });
        }
    };
    match outcome.terminal {
        Terminal::Done { summary, usage, .. } => {
            run.sink.progress(&format!("done: {summary}"));
            record.outcome = format!("done: {summary}");
            record.usage = usage;
        }
        Terminal::Question { text } => {
            run.sink.progress(&format!("question: {text}"));
            record.outcome = format!("question: {text}");
            return fail_all(format!(
                "{tag}: reviewer asked a question instead of judging: {text}"
            ));
        }
        Terminal::Error { message, retryable } => {
            run.sink
                .progress(&format!("error(retryable={retryable}): {message}"));
            // ADR-0054 D2（Phase 113。実機障害 2026-09-23、タスク 01M35X86XTK84F97QW0CN5PGMR）:
            // `is_error` の結果・プロセス失敗・resume 拒否・レート制限文言はここに来る
            // （`terminal_from_result`/`(None, None)` クラッシュ分類が `Terminal::Error` にする）。
            // `retryable` なら「判定できなかっただけ」として `fail_all` せず、ディスパッチャに
            // `max_reviewer_retries` までのやり直しを任せる。`retryable = false`（`paperqa`/
            // `subprocess` の「やり直しても直らない」設定不備等）は従来どおり即座に不合格。
            if retryable {
                record.outcome = format!("requeue: reviewer run failed: {message}");
                return Err(ReviewerProviderFailure {
                    outcome: None,
                    message: format!("{tag}: reviewer run failed: {message}"),
                });
            }
            record.outcome = format!("error(retryable=false): {message}");
            return fail_all(format!("{tag}: reviewer run failed: {message}"));
        }
        // ADR-0072 D7（Phase E1）: reviewer run は continuation の対象外（graceful yield の前置きも
        // 付けない。D10）。同じ claude-code の実行経路を通るので理論上は起こりうるが、起きたときは
        // 「判定できなかっただけ」として reviewer のやり直し（`max_reviewer_retries`）に任せる
        // （`retryable = true` の `Terminal::Error` と同じ扱い）。
        Terminal::BudgetExhausted {
            kind,
            message,
            usage,
        } => {
            run.sink
                .progress(&format!("budget exhausted ({kind:?}): {message}"));
            record.outcome = format!("requeue: reviewer run failed: budget exhausted: {message}");
            record.usage = usage;
            return Err(ReviewerProviderFailure {
                outcome: None,
                message: format!("{tag}: reviewer run failed: budget exhausted: {message}"),
            });
        }
        Terminal::Yielded { usage, .. } => {
            run.sink.progress("yielded instead of judging");
            record.outcome = "requeue: reviewer run yielded instead of judging".to_string();
            record.usage = usage;
            return Err(ReviewerProviderFailure {
                outcome: None,
                message: format!("{tag}: reviewer run yielded instead of judging"),
            });
        }
        // ADR-0090: reviewer run は job を待たない（wait を書いても判定の代わりにはならない）。やり直しに任せる。
        Terminal::Waiting { usage, .. } => {
            run.sink
                .progress("asked for a cluster job wait instead of judging");
            record.outcome =
                "requeue: reviewer run asked for a cluster job wait instead of judging".to_string();
            record.usage = usage;
            return Err(ReviewerProviderFailure {
                outcome: None,
                message: format!(
                    "{tag}: reviewer run asked for a cluster job wait instead of judging"
                ),
            });
        }
    }
    let text = match std::fs::read_to_string(&review_path) {
        Ok(t) => t,
        Err(e) => return fail_all(format!("{tag}: {review_rel} not found or unreadable: {e}")),
    };
    let output: ReviewOutput = match serde_json::from_str(&text) {
        Ok(o) => o,
        Err(e) => {
            return fail_all(format!(
                "{tag}: {review_rel} is not a valid ReviewOutput: {e}"
            ));
        }
    };
    Ok(criteria
        .iter()
        .map(
            |&idx| match output.verdicts.iter().rev().find(|v| v.criterion == idx) {
                Some(v) => Verdict {
                    criterion_idx: idx,
                    pass: v.pass,
                    reason: format!("{tag}: {}", v.reason),
                    // ADR-0072 D16（Phase E4）: reviewer 自身が申告した repair ヒントをそのまま写す
                    // （`pass = true` のときは呼び出し側〈`classify_review_failure`〉が使わない）。
                    repair_hint: v.repair.as_ref().map(|r| {
                        task_core::execution::ReviewRepairHint {
                            scope: r.scope.clone(),
                            class: r.class.clone(),
                        }
                    }),
                },
                None => Verdict {
                    criterion_idx: idx,
                    pass: false,
                    reason: format!("{tag}: no verdict for criterion {idx} in {review_rel}"),
                    repair_hint: None,
                },
            },
        )
        .collect())
}

#[cfg(test)]
mod tests;
