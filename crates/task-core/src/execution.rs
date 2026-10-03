//! ADR-0072（Task execution decomposition）Phase E1: Run lifecycle / checkpoint / continuation。
//!
//! 純粋なデータ定義と純粋関数だけを置く（I/O・LLM 呼び出し・プロセス起動はしない。ADR-0001 D2）。
//! git の読み取りや `checkpoint.json` の読み込みは `task-dispatch::checkpoint`（I/O 層）が行い、
//! ここに集めた事実（[`MechanicalCheckpoint`]）と worker の申告（[`WorkerCheckpointInput`]）を
//! [`merge_checkpoint`] で決定的に合成する（ADR-0072 D8）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// D8: checkpoint の JSON schema 版。
pub const CHECKPOINT_SCHEMA: &str = "celeris.checkpoint/1";
/// D8: checkpoint 全体の上限（16 KiB）。
pub const CHECKPOINT_MAX_BYTES: usize = 16 * 1024;
/// D8: 配列 1 つあたりの上限（30 件）。
pub const CHECKPOINT_MAX_ITEMS: usize = 30;
/// D8: 文字列 1 つあたりの上限（500 文字）。
pub const CHECKPOINT_MAX_STRING_CHARS: usize = 500;

// ---------------------------------------------------------------------------
// D7: Run の終わり方
// ---------------------------------------------------------------------------

/// D7: 予算切れの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BudgetKind {
    Turns,
    WallClock,
    Context,
}

/// D7: harness / 供給側都合の失敗の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessErrorClass {
    Supply,
    Infra,
    LeaseExpired,
    IdleTimeout,
}

/// D7: `task_core::execution::RunEnd`（`WorkerFinished.end` に入れる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEnd {
    Completed,
    Yielded,
    BudgetExhausted {
        kind: BudgetKind,
    },
    Question,
    Failed {
        retryable: bool,
    },
    HarnessError {
        class: HarnessErrorClass,
    },
    Cancelled,
    /// ADR-0090 D1/D4: `result.json` の `wait`（クラスタ job の終了待ち）で終わった。run は閉じるが task / unit は
    /// 終わっていない（job が終われば continuation の run が続きをやる）。continuation の回数・進捗なしの窓・
    /// attempts には数えない（[`RunEnd::is_continuable`] は `false`）。
    Waiting,
}

impl RunEnd {
    /// この Run の終わり方が continuation（D9/D11）の対象か（予算切れ、または自己申告の yield）。
    pub fn is_continuable(self) -> bool {
        matches!(self, RunEnd::BudgetExhausted { .. } | RunEnd::Yielded)
    }

    /// D8 の checkpoint の `end` 欄に写す（continuation 対象でなければ `None`）。
    pub fn as_checkpoint_end(self) -> Option<CheckpointEnd> {
        match self {
            RunEnd::Completed => Some(CheckpointEnd::Completed),
            RunEnd::Yielded => Some(CheckpointEnd::Yielded),
            RunEnd::BudgetExhausted { .. } => Some(CheckpointEnd::BudgetExhausted),
            RunEnd::Waiting => Some(CheckpointEnd::Waiting),
            _ => None,
        }
    }

    /// ADR-0090 D1: checkpoint を合成して残す終わり方か（continuation の対象〈予算切れ・yield〉と、
    /// クラスタ job の wait）。
    pub fn saves_checkpoint(self) -> bool {
        self.is_continuable() || self == RunEnd::Waiting
    }
}

// ---------------------------------------------------------------------------
// D6: Trigger::Continue の `why`
// ---------------------------------------------------------------------------

/// D6: `Trigger::Continue{why}` の種類。E1 で使うのは [`ContinueWhy::Continue`] だけ（予算切れ /
/// yield の続き）。他は E2 以降の配線先（`advance` = WU 完了で次へ、`work_unit_retry` = WU の
/// retry、`planned` = planner の採用、`replan` = 再計画）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContinueWhy {
    Continue,
    Advance,
    WorkUnitRetry,
    Planned,
    Replan,
}

impl ContinueWhy {
    /// `Event::Transitioned.reason` に入る静的な名前（D6 の表）。
    pub fn name(self) -> &'static str {
        match self {
            ContinueWhy::Continue => "continue",
            ContinueWhy::Advance => "advance",
            ContinueWhy::WorkUnitRetry => "work_unit_retry",
            ContinueWhy::Planned => "planned",
            ContinueWhy::Replan => "replan",
        }
    }
}

// ---------------------------------------------------------------------------
// D8: checkpoint
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointEnd {
    Completed,
    Yielded,
    BudgetExhausted,
    /// ADR-0090 D1: クラスタ job の wait で止めた run の checkpoint（進捗なしの窓の比較から外す）。
    Waiting,
}

/// checkpoint を合成した出所（D8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointSource {
    Worker,
    Yield,
    Mechanical,
    Merged,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointDecision {
    pub what: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointFileChange {
    pub path: String,
    /// `"added" | "modified" | "deleted"`（自由記述。worker の申告も daemon の git 検出もここに入る）。
    pub change: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointTestRun {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointKnownFailure {
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointArtifactRef {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepoState {
    pub branch: String,
    pub base: String,
    pub head: String,
    pub uncommitted: bool,
    pub diff_stat: String,
}

/// D8: daemon が確定させた checkpoint（`celeris.checkpoint/1`）。`CheckpointSaved` イベントと
/// `runs/<run_id>/checkpoint.json` に残す。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Checkpoint {
    pub schema: String,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_unit: Option<String>,
    pub run_id: String,
    pub run_seq: u32,
    pub end: CheckpointEnd,
    pub source: CheckpointSource,
    #[serde(default)]
    pub completed: Vec<String>,
    #[serde(default)]
    pub remaining: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<CheckpointDecision>,
    #[serde(default)]
    pub files_changed: Vec<CheckpointFileChange>,
    #[serde(default)]
    pub tests_run: Vec<CheckpointTestRun>,
    #[serde(default)]
    pub known_failures: Vec<CheckpointKnownFailure>,
    #[serde(default)]
    pub artifact_refs: Vec<CheckpointArtifactRef>,
    pub next_action: String,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_issue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_state: Option<RepoState>,
    #[serde(default)]
    pub recent_activity: Vec<String>,
    pub created_at: String,
}

/// ワーカーが `<artifacts_dir>/checkpoint.json` に書く、または `result.json` の `yield` に添える
/// 申告（D8 の意味の欄だけ）。**寛容に読む**（`deny_unknown_fields` にしない。未知の欄は捨てる）。
/// JSON として読めない・配列であるべき欄が配列でない等の**型の不一致は schema 違反**として扱い、
/// 呼び出し側（`task-dispatch::checkpoint`）は `None` として [`merge_checkpoint`] に渡す。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct WorkerCheckpointInput {
    #[serde(default)]
    pub completed: Vec<String>,
    #[serde(default)]
    pub remaining: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<CheckpointDecision>,
    #[serde(default)]
    pub files_changed: Vec<CheckpointFileChange>,
    #[serde(default)]
    pub tests_run: Vec<CheckpointTestRun>,
    #[serde(default)]
    pub known_failures: Vec<CheckpointKnownFailure>,
    #[serde(default)]
    pub artifact_refs: Vec<CheckpointArtifactRef>,
    #[serde(default)]
    pub next_action: Option<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub plan_issue: Option<String>,
}

/// worker の checkpoint.json（文字列）を寛容に読む。JSON として不正、または既知の欄の型が
/// 合わなければ `None`（D8: 「schema 違反のときは mechanical だけで作る」）。
pub fn parse_worker_checkpoint(text: &str) -> Option<WorkerCheckpointInput> {
    serde_json::from_str(text).ok()
}

/// daemon が git などから決定的に集めた「事実」（D8 の `mechanical`）。I/O は
/// `task-dispatch::checkpoint` が行い、ここには結果だけを渡す。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MechanicalCheckpoint {
    pub repo_state: Option<RepoState>,
    pub files_changed: Vec<CheckpointFileChange>,
    pub tests_run: Vec<CheckpointTestRun>,
    pub recent_activity: Vec<String>,
}

/// [`merge_checkpoint`] が確定値を組み立てるのに要る、run に固有の情報。
#[derive(Debug, Clone, PartialEq)]
pub struct CheckpointContext {
    pub task_id: String,
    /// E1 では常に `None`（暗黙の WorkUnit）。E2 以降で WorkUnit の id を持つ。
    pub work_unit: Option<String>,
    pub run_id: String,
    pub run_seq: u32,
    pub end: CheckpointEnd,
    pub created_at: String,
}

const NO_CHECKPOINT_REMAINING: &str = "（checkpoint がありません）元の目的を続けてください";
const NO_CHECKPOINT_NEXT_ACTION: &str = "git diff で現状を確認して続きから";

/// D8 の合成規則: 意味の欄は worker を採り、事実の欄（`files_changed` のパス集合）は mechanical を
/// 正として worker の注記を path で添える。`tests_run` は和集合（コマンドで重複を除く）。worker が
/// 無い・schema 違反なら mechanical だけ（`source = mechanical`、`remaining`/`next_action` は
/// 既定文言）。合成後は [`truncate_checkpoint`] で 16 KiB に切り詰める。
pub fn merge_checkpoint(
    worker: Option<WorkerCheckpointInput>,
    mechanical: MechanicalCheckpoint,
    ctx: CheckpointContext,
) -> Checkpoint {
    let source = if worker.is_some() {
        CheckpointSource::Merged
    } else {
        CheckpointSource::Mechanical
    };

    let (
        completed,
        remaining,
        decisions,
        known_failures,
        artifact_refs,
        next_action,
        open_questions,
        plan_issue,
        worker_files,
    ) = match &worker {
        Some(w) => (
            w.completed.clone(),
            w.remaining.clone(),
            w.decisions.clone(),
            w.known_failures.clone(),
            w.artifact_refs.clone(),
            w.next_action
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| NO_CHECKPOINT_NEXT_ACTION.to_string()),
            w.open_questions.clone(),
            w.plan_issue.clone(),
            w.files_changed.clone(),
        ),
        None => (
            Vec::new(),
            vec![NO_CHECKPOINT_REMAINING.to_string()],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            NO_CHECKPOINT_NEXT_ACTION.to_string(),
            Vec::new(),
            None,
            Vec::new(),
        ),
    };

    // 事実の欄は mechanical を正とし、worker の注記を path で添える。
    let files_changed: Vec<CheckpointFileChange> = mechanical
        .files_changed
        .into_iter()
        .map(|mut f| {
            if let Some(note) = worker_files
                .iter()
                .find(|wf| wf.path == f.path)
                .and_then(|wf| wf.note.clone())
            {
                f.note = Some(note);
            }
            f
        })
        .collect();

    // tests_run は和集合（コマンドで重複を除く。mechanical を先に、worker の未出のコマンドを足す）。
    let mut tests_run = mechanical.tests_run;
    if let Some(w) = &worker {
        for t in &w.tests_run {
            if !tests_run.iter().any(|e| e.command == t.command) {
                tests_run.push(t.clone());
            }
        }
    }

    let mut checkpoint = Checkpoint {
        schema: CHECKPOINT_SCHEMA.to_string(),
        task_id: ctx.task_id,
        work_unit: ctx.work_unit,
        run_id: ctx.run_id,
        run_seq: ctx.run_seq,
        end: ctx.end,
        source,
        completed,
        remaining,
        decisions,
        files_changed,
        tests_run,
        known_failures,
        artifact_refs,
        next_action,
        open_questions,
        plan_issue,
        repo_state: mechanical.repo_state,
        recent_activity: mechanical.recent_activity,
        created_at: ctx.created_at,
    };
    truncate_checkpoint(&mut checkpoint);
    checkpoint
}

fn truncate_string(s: &mut String) {
    if s.chars().count() > CHECKPOINT_MAX_STRING_CHARS {
        let truncated: String = s.chars().take(CHECKPOINT_MAX_STRING_CHARS).collect();
        *s = format!("{truncated}…");
    }
}

fn truncate_strings(v: &mut [String]) {
    for s in v.iter_mut() {
        truncate_string(s);
    }
}

fn pop_one<T>(v: &mut Vec<T>) -> bool {
    v.pop().is_some()
}

fn checkpoint_byte_len(cp: &Checkpoint) -> usize {
    serde_json::to_vec(cp).map(|v| v.len()).unwrap_or(0)
}

/// D8: 配列 30 件・文字列 500 文字の上限を適用し、それでも全体が 16 KiB を超えるなら決定的に
/// 縮める（`recent_activity` → `tests_run` → `known_failures` → `files_changed` → `decisions` →
/// `artifact_refs` → `open_questions` → `remaining` → `completed` の順に末尾から落とし、
/// それでも収まらなければ `next_action` を空にする）。
pub fn truncate_checkpoint(cp: &mut Checkpoint) {
    truncate_string(&mut cp.next_action);
    if let Some(pi) = &mut cp.plan_issue {
        truncate_string(pi);
    }
    cp.completed.truncate(CHECKPOINT_MAX_ITEMS);
    truncate_strings(&mut cp.completed);
    cp.remaining.truncate(CHECKPOINT_MAX_ITEMS);
    truncate_strings(&mut cp.remaining);
    cp.open_questions.truncate(CHECKPOINT_MAX_ITEMS);
    truncate_strings(&mut cp.open_questions);
    cp.recent_activity.truncate(CHECKPOINT_MAX_ITEMS);
    truncate_strings(&mut cp.recent_activity);

    cp.decisions.truncate(CHECKPOINT_MAX_ITEMS);
    for d in &mut cp.decisions {
        truncate_string(&mut d.what);
        truncate_string(&mut d.why);
    }
    cp.files_changed.truncate(CHECKPOINT_MAX_ITEMS);
    for f in &mut cp.files_changed {
        truncate_string(&mut f.path);
        truncate_string(&mut f.change);
        if let Some(n) = &mut f.note {
            truncate_string(n);
        }
    }
    cp.tests_run.truncate(CHECKPOINT_MAX_ITEMS);
    for t in &mut cp.tests_run {
        truncate_string(&mut t.command);
        if let Some(s) = &mut t.summary {
            truncate_string(s);
        }
    }
    cp.known_failures.truncate(CHECKPOINT_MAX_ITEMS);
    for k in &mut cp.known_failures {
        truncate_string(&mut k.what);
        if let Some(d) = &mut k.detail {
            truncate_string(d);
        }
    }
    cp.artifact_refs.truncate(CHECKPOINT_MAX_ITEMS);
    for a in &mut cp.artifact_refs {
        truncate_string(&mut a.path);
    }
    if let Some(rs) = &mut cp.repo_state {
        truncate_string(&mut rs.branch);
        truncate_string(&mut rs.base);
        truncate_string(&mut rs.head);
        truncate_string(&mut rs.diff_stat);
    }

    // 全体の上限（16 KiB）。決定的な優先順位で末尾から落とす。
    while checkpoint_byte_len(cp) > CHECKPOINT_MAX_BYTES {
        let shrank = pop_one(&mut cp.recent_activity)
            || pop_one(&mut cp.tests_run)
            || pop_one(&mut cp.known_failures)
            || pop_one(&mut cp.files_changed)
            || pop_one(&mut cp.decisions)
            || pop_one(&mut cp.artifact_refs)
            || pop_one(&mut cp.open_questions)
            || pop_one(&mut cp.remaining)
            || pop_one(&mut cp.completed);
        if shrank {
            continue;
        }
        if !cp.next_action.is_empty() {
            cp.next_action.clear();
            continue;
        }
        // これ以上縮められない（すべて空でもまだ超過。理論上到達しない — schema/task_id/run_id 等の
        // 固定欄だけで 16 KiB を超えることはない）。無限ループを避けて諦める。
        break;
    }
}

/// D18: 進捗の定義。次のどれかが起きていれば進捗あり: `files_changed` のパス集合か HEAD が変わる /
/// `completed` が増える / `remaining` が減る。前の checkpoint が無ければ（この WU/Task で最初の
/// checkpoint）常に進捗ありとする。
pub fn checkpoint_shows_progress(prev: Option<&Checkpoint>, next: &Checkpoint) -> bool {
    let Some(prev) = prev else {
        return true;
    };
    let prev_paths: std::collections::BTreeSet<&str> =
        prev.files_changed.iter().map(|f| f.path.as_str()).collect();
    let next_paths: std::collections::BTreeSet<&str> =
        next.files_changed.iter().map(|f| f.path.as_str()).collect();
    if prev_paths != next_paths {
        return true;
    }
    let prev_head = prev.repo_state.as_ref().map(|r| r.head.as_str());
    let next_head = next.repo_state.as_ref().map(|r| r.head.as_str());
    if prev_head != next_head {
        return true;
    }
    if next.completed.len() > prev.completed.len() {
        return true;
    }
    if next.remaining.len() < prev.remaining.len() {
        return true;
    }
    false
}

/// D7: context 超過の実機の文言は未確認（ADR-0072 §7 U1）。既知の候補の字句で判定する
/// （分類できなければ従来どおり字句判定なし = 呼び出し側が安全側の `WorkerError`/`Failed` に倒す）。
/// `retry_policy::is_budget_outcome` と各アダプタの終端判定の両方から使う単一の語彙源。
pub fn looks_like_context_exceeded(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "prompt is too long",
        "context window",
        "context_length_exceeded",
        "context length exceeded",
        "context_length",
        "maximum context length",
        "exceeds the model's context",
        "exceeds context limit",
    ]
    .iter()
    .any(|w| t.contains(w))
}

/// 生成したスキーマ（`docs/protocol/checkpoint.schema.json`。`UPDATE_SCHEMA=1` で再生成）。
pub fn schema_value() -> serde_json::Value {
    let schema = schemars::schema_for!(Checkpoint);
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

// ---------------------------------------------------------------------------
// ADR-0072 D16（Phase E4）: reviewer repair（不合格を局所的に修復する）
// ---------------------------------------------------------------------------

/// D16: 最終レビューで不合格になった 1 条件の、分類に要る最小限の情報。`pass = false` の
/// `Verdict` だけを渡す想定（呼び出し側が `task.acceptance[criterion_idx].check` を添える）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedCheck {
    pub check: crate::model::Check,
    pub reason: String,
    /// `Check::Reviewer` の不合格に添えられた `review.json` の `repair` ヒント（D16: `{"scope":"local",
    /// "class":"format|lint|test|doc|other","hint":"…"}`）。それ以外の `check` では常に `None`。
    pub repair_hint: Option<ReviewRepairHint>,
}

/// `review.json` の `repair` ヒント（`scope`/`class` だけを使う。`hint` の自由記述は呼び出し側の
/// ログ用で分類には使わない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRepairHint {
    pub scope: String,
    pub class: String,
}

/// D16: `reviewer_local` の repair WU の予算を決めるための、内側の（サブ）分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewerRepairKind {
    Format,
    Lint,
    Test,
    Other,
}

impl ReviewerRepairKind {
    fn parse(class: &str) -> Self {
        match class {
            "format" => ReviewerRepairKind::Format,
            "lint" => ReviewerRepairKind::Lint,
            "test" => ReviewerRepairKind::Test,
            _ => ReviewerRepairKind::Other,
        }
    }
}

/// D16 の分類表: `format` / `lint` / `test_small` / `reviewer_local` / `merge_base`。ADR-0074 D6.1/D6.2
/// （Phase F1）で `review_timeout` を足し、`merge_base` を Task 内部の最終レビュー（`Check::Command`
/// の `cmd` が `merge-base --is-ancestor` を含む場合）でも返せるようにした。配送
/// （`crates/celeris/src/delivery.rs`）自身の技術的な失敗は今までどおり、この関数を経由せず直接
/// `RepairClass::MergeBase`/`Format` を組み立てる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairClass {
    Format,
    Lint,
    TestSmall,
    ReviewerLocal(ReviewerRepairKind),
    MergeBase,
    /// ADR-0074 D6.1（Phase F1）: 決定的な検査が `command timed out after` で不合格になり、
    /// daemon が 2 倍の timeout で 1 回再実行してもなお timeout だった。
    ReviewTimeout,
    /// ADR-0074 D1.4（Phase F2b）: 工程の統合で WU のブランチの merge が衝突した（repair WU
    /// `merge-<phase>-<key>` が Task の worktree で merge し、衝突だけを解消する）。
    MergeConflict,
    /// ADR-0120 D2: review 前の target 同期（rebase）が衝突した。repair WU
    /// `integration-repair-<n>` が成果を保ったまま rebase を完了させる。
    IntegrationConflict,
}

impl RepairClass {
    /// `repairs` の「同じ class は 2 回まで」の集計に使うキー（`reviewer_local` はサブ分類を問わず
    /// 1 つのバケット）。
    pub fn bucket(self) -> &'static str {
        match self {
            RepairClass::Format => "format",
            RepairClass::Lint => "lint",
            RepairClass::TestSmall => "test_small",
            RepairClass::ReviewerLocal(_) => "reviewer_local",
            RepairClass::MergeBase => "merge_base",
            RepairClass::ReviewTimeout => "review_timeout",
            RepairClass::MergeConflict => "merge_conflict",
            RepairClass::IntegrationConflict => INTEGRATION_REPAIR_BUCKET,
        }
    }

    /// D16 の表の repair WU の予算（`max_turns`, `max_wall_secs`）。
    pub fn budget(self) -> (u32, u64) {
        match self {
            RepairClass::Format => (12, 600),
            RepairClass::Lint => (20, 1200),
            RepairClass::TestSmall
            | RepairClass::MergeBase
            | RepairClass::MergeConflict
            | RepairClass::IntegrationConflict => (30, 1800),
            RepairClass::ReviewerLocal(kind) => match kind {
                ReviewerRepairKind::Format => (12, 600),
                ReviewerRepairKind::Lint => (20, 1200),
                ReviewerRepairKind::Test | ReviewerRepairKind::Other => (30, 1800),
            },
            // ADR-0074 D6.1: max_turns 20、wall 1200（lane は cheap。呼び出し側が features で決める）。
            RepairClass::ReviewTimeout => (20, 1200),
        }
    }
}

/// ADR-0074 D6.2（Phase F1）: `Event::RepairScheduled.origin`（repair WU を起こした場所）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepairOrigin {
    /// Task 内部の最終レビュー不合格（`try_review_repair`。D16）。
    Review,
    /// 工程末尾の統合後の検査不合格（F2 の並列 WU。F1 では発生しない）。
    Integration,
    /// 配送前の準備ゲート・取り込みの技術的失敗（`crates/celeris/src/delivery.rs`）。
    Delivery,
    /// planner が replan で自ら `kind = repair` の WU を書いた（class は復元できないので
    /// `"planner"` 固定。E6 の `repairs_by_class: unknown` の解消）。
    Planner,
}

impl RepairOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            RepairOrigin::Review => "review",
            RepairOrigin::Integration => "integration",
            RepairOrigin::Delivery => "delivery",
            RepairOrigin::Planner => "planner",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "review" => Some(RepairOrigin::Review),
            "integration" => Some(RepairOrigin::Integration),
            "delivery" => Some(RepairOrigin::Delivery),
            "planner" => Some(RepairOrigin::Planner),
            _ => None,
        }
    }
}

/// D16: 修復できる不合格かどうかの判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairDecision {
    Repairable(RepairClass),
    /// 修復できない不合格が 1 つでもある、または修復できる種類が混在している
    /// （`Check::Human` の不合格を含む場合も、この列挙値になる）。
    Substantive,
}

const FMT_COMMAND_WORDS: [&str; 7] = [
    "cargo fmt",
    "rustfmt",
    "prettier",
    "biome format",
    "gofmt",
    "black",
    "ruff format",
];
const LINT_COMMAND_WORDS: [&str; 7] = [
    "clippy",
    "eslint",
    "biome check",
    "biome lint",
    "ruff check",
    "tsc",
    "typecheck",
];
const TEST_COMMAND_WORDS: [&str; 5] = ["cargo test", "pnpm test", "vitest", "pytest", "go test"];

/// `reason`（コマンドの stdout/stderr tail を含む）から、失敗したテストの件数を読む。
/// `test result: FAILED. N passed; M failed`、`M failed` のような字句を探す（決定的な字句判定。
/// D16）。見つからなければ `None`（test_small と判定できない = substantive に倒れる）。
fn failed_test_count(reason: &str) -> Option<u32> {
    let bytes = reason.as_bytes();
    let needle = b" failed";
    let mut best: Option<u32> = None;
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            // needle の直前にある数字の連続を後ろ向きに読む。
            let mut j = i;
            while j > 0 && bytes[j - 1].is_ascii_digit() {
                j -= 1;
            }
            if j < i
                && let Ok(n) = reason[j..i].parse::<u32>()
            {
                best = Some(n);
            }
        }
        i += 1;
    }
    best
}

/// D6.1（Phase F1）: `crate::dispatch`（呼び出し側）が同じコマンドを 2 倍の timeout で 1 回だけ
/// 再実行してもなお付ける固定文言（`review.rs:295, 407, 531` と同じ字句）。
const COMMAND_TIMEOUT_PREFIX: &str = "command timed out after";
/// D6.2（Phase F1）: Task 内部の最終レビューでの merge-base 不成立（E6 の criterion 5 の形）。
const MERGE_BASE_ANCESTOR_WORDS: [&str; 2] = ["merge-base", "--is-ancestor"];

fn classify_command(cmd: &str, reason: &str) -> Option<RepairClass> {
    // ADR-0074 D6.1: reason（daemon が既に 2 倍の timeout で再試行した後の結果）が timeout を
    // 示していれば、cmd の中身に関わらず review_timeout（fmt/lint/test のコマンドが遅いだけの
    // 環境要因を、コードの再実装ではなく検査環境の再現で直す性質のものとして別扱いする）。
    if reason.starts_with(COMMAND_TIMEOUT_PREFIX) {
        return Some(RepairClass::ReviewTimeout);
    }
    let c = cmd.to_lowercase();
    // ADR-0074 D6.2: `merge-base --is-ancestor` の不成立（Task 内部の最終レビュー段階。配送段階の
    // `[delivery-repair]` は別経路のまま）。
    if MERGE_BASE_ANCESTOR_WORDS.iter().all(|w| c.contains(w)) {
        return Some(RepairClass::MergeBase);
    }
    if FMT_COMMAND_WORDS.iter().any(|w| c.contains(w)) {
        return Some(RepairClass::Format);
    }
    if LINT_COMMAND_WORDS.iter().any(|w| c.contains(w)) {
        return Some(RepairClass::Lint);
    }
    if TEST_COMMAND_WORDS.iter().any(|w| c.contains(w)) {
        return match failed_test_count(reason) {
            Some(n) if (1..=3).contains(&n) => Some(RepairClass::TestSmall),
            _ => None,
        };
    }
    None
}

/// D16「reason が fmt / clippy の語だけを指す（字句の予備判定）」: 明示の `repair` ヒントが無い
/// `Check::Reviewer` の不合格に対する、決定的な字句フォールバック。
///
/// 字句を見る前に ULID の形の語（`reviewer(<run_id>): ` の接頭辞や `.taskd/artifacts/<task_id>/`
/// の id）を外す。Crockford base32 の ULID は `F`・`M`・`T` を含みうるので、id の一部の `FMT` を
/// `fmt` の語と誤読して、中身の不合格を format の repair に倒していた（nextest の gate で
/// `rereview_from_failed_reuses_the_approved_human_child_and_only_reruns_the_reviewer` が約 0.04 % で
/// `Done` になった原因。agent-docs/progress/2026-10-02-docs-layout/refs-crates.md「SD-2 追記」）。`L`・`I`・`O` は ULID に出ないので
/// `lint` / `clippy` / `format` は id から生じないが、同じ理由で一律に外す。
fn classify_reviewer_reason(reason: &str) -> Option<RepairClass> {
    let r = without_ulid_tokens(reason).to_lowercase();
    let mentions_fmt = r.contains("fmt") || r.contains("format");
    let mentions_lint = r.contains("clippy") || r.contains("lint");
    if mentions_fmt && !mentions_lint {
        return Some(RepairClass::ReviewerLocal(ReviewerRepairKind::Format));
    }
    if mentions_lint && !mentions_fmt {
        return Some(RepairClass::ReviewerLocal(ReviewerRepairKind::Lint));
    }
    None
}

/// `s` から ULID の形の語（英数字の連続で、26 文字すべてが Crockford base32 の大文字・数字、
/// 先頭が `0`〜`7`）を取り除いた文字列。区切り（英数字以外）はそのまま残す。
fn without_ulid_tokens(s: &str) -> String {
    fn is_ulid(tok: &str) -> bool {
        tok.len() == 26
            && tok.as_bytes()[0] <= b'7'
            && tok
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'A'..=b'H' | b'J' | b'K' | b'M' | b'N' | b'P'..=b'T' | b'V'..=b'Z'))
    }
    let mut out = String::with_capacity(s.len());
    let mut token_start: Option<usize> = None;
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_alphanumeric() {
            token_start.get_or_insert(i);
            continue;
        }
        if let Some(start) = token_start.take()
            && !is_ulid(&s[start..i])
        {
            out.push_str(&s[start..i]);
        }
        out.push(ch);
    }
    if let Some(start) = token_start
        && !is_ulid(&s[start..])
    {
        out.push_str(&s[start..]);
    }
    out
}

fn classify_one(f: &FailedCheck) -> Option<RepairClass> {
    match &f.check {
        crate::model::Check::Command { cmd, .. } => classify_command(cmd, &f.reason),
        crate::model::Check::Reviewer => match &f.repair_hint {
            Some(hint) if hint.scope == "local" => Some(RepairClass::ReviewerLocal(
                ReviewerRepairKind::parse(&hint.class),
            )),
            Some(_) => None,
            None => classify_reviewer_reason(&f.reason),
        },
        // ArtifactExists / KnowledgePage / Human: D16 の表に無い = 修復できない。
        _ => None,
    }
}

/// D16: 不合格の verdict をすべて見て、**全部が同じ修復できる class** のときだけ
/// `RepairDecision::Repairable` にする（1 つでも修復できない、または class が混在すれば
/// `Substantive`）。`failing` は空を渡さない（呼び出し側は不合格が無ければそもそも呼ばない）。
pub fn classify_review_failure(failing: &[FailedCheck]) -> RepairDecision {
    let mut classes = failing.iter().map(classify_one);
    let Some(Some(first)) = classes.next() else {
        return RepairDecision::Substantive;
    };
    for c in classes {
        match c {
            Some(c) if c.bucket() == first.bucket() => {}
            _ => return RepairDecision::Substantive,
        }
    }
    RepairDecision::Repairable(first)
}

/// ADR-0074 付記（2026-10-02）: repair に渡す task / 段階の許可範囲。空なら `build_repair_objective`
/// は `None` と同じ出力になる（呼び出し側が範囲を集められないときはそのまま空を渡せばよい）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepairScope {
    pub allowed_paths: Vec<String>,
    pub scope_checks: Vec<String>,
}

impl RepairScope {
    fn is_empty(&self) -> bool {
        self.allowed_paths.is_empty() && self.scope_checks.is_empty()
    }
}

/// D16: repair WU の objective（**最小の context**。元の実装の context は作り直さない）。決定的な
/// 文字列合成のみ（I/O は無い）。`failing_details` は `Verdict.reason`（cmd / exit / stdout・stderr の
/// tail をそのまま含む）を 1 件 1 行ずつ渡す。`diff_stat`（`git diff --stat` の要約）は任意。
/// `scope`（ADR-0074 付記）が `Some` かつ空でなければ、許可範囲と範囲外差分の検査を objective に足す。
/// `None` または空なら、従来の出力と 1 バイトも変わらない。
pub fn build_repair_objective(
    class: RepairClass,
    failing_details: &[String],
    task_title: &str,
    task_objective: &str,
    diff_stat: Option<&str>,
    scope: Option<&RepairScope>,
) -> String {
    let objective_preview: String = task_objective.chars().take(600).collect();
    let mut s = String::new();
    s.push_str(
        "次の検査が失敗した。失敗を直すことだけをせよ。設計や他のコードは変えるな。\
直した後に同じコマンドを実行して exit を確かめよ。\n\n",
    );
    s.push_str(&format!("## 分類\n{}\n\n", class.bucket()));
    s.push_str("## 失敗した検査\n");
    for d in failing_details {
        s.push_str(&format!("- {d}\n"));
    }
    s.push_str(&format!(
        "\n## 対象タスク（参考。全文ではない）\n- title: {task_title}\n- objective（先頭 600 文字）: {objective_preview}\n"
    ));
    if let Some(stat) = diff_stat {
        s.push_str(&format!("\n## git diff --stat\n{stat}\n"));
    }
    if let Some(scope) = scope
        && !scope.is_empty()
    {
        if !scope.allowed_paths.is_empty() {
            s.push_str("\n## 変更してよい範囲\n");
            for p in &scope.allowed_paths {
                s.push_str(&format!("- {p}\n"));
            }
        }
        if !scope.scope_checks.is_empty() {
            s.push_str("\n## 範囲外差分の検査\n");
            for c in &scope.scope_checks {
                s.push_str(&format!("- {c}\n"));
            }
            s.push_str("直した後にこれも実行して exit を確かめよ。\n");
        }
        s.push_str(
            "\n失敗の原因が許可範囲の外にある（例: web/ だけの task での crates/ の flaky test・\
環境依存のテスト）なら、範囲外のファイルを変えるな。result.json に \
`{\"yield\":{\"plan_issue\":\"<何が範囲外のどこで落ちたか>\"}}` を書いて終えよ。\n",
        );
    }
    s
}

// ---------------------------------------------------------------------------
// ADR-0120: review 前同期の衝突を成果保持型 IntegrationRepair で解消する
// ---------------------------------------------------------------------------

/// ADR-0120 D2/D4: integration repair WU の bucket（`RepairClass::IntegrationConflict.bucket()`、
/// `RepairScheduled.class`、`WorkUnitTransitioned.reason`）。
pub const INTEGRATION_REPAIR_BUCKET: &str = "integration_repair";
/// ADR-0120 D2: integration repair WU の title（`repair (<bucket>): …` の字句規則で bucket を読み戻せる）。
pub const INTEGRATION_REPAIR_TITLE: &str = "repair (integration_repair): target 同期の衝突解消";

/// ADR-0120 D2: `n` 件目（1 始まり）の integration repair WU の key（`integration-repair-<n>`）。
pub fn integration_repair_key(n: u32) -> String {
    format!("integration-repair-{n}")
}

/// ADR-0120 D4: その WU が integration repair か（`kind = repair` かつ title の bucket が
/// `integration_repair`）。回数はこれに当たる work_units の行数で数える（専用の欄を足さない）。
pub fn is_integration_repair_unit(kind: crate::WorkUnitKind, title: &str) -> bool {
    kind == crate::WorkUnitKind::Repair
        && title
            .strip_prefix("repair (")
            .and_then(|rest| rest.split(')').next())
            == Some(INTEGRATION_REPAIR_BUCKET)
}

/// ADR-0120 D4: task の work_units（`(kind, title)` の列）のうち integration repair の数。
/// 人の `Rereview`・`Reopen` でも数え直さない（task の寿命で数える）。
pub fn count_integration_repairs<'a>(
    units: impl IntoIterator<Item = (crate::WorkUnitKind, &'a str)>,
) -> u32 {
    let n = units
        .into_iter()
        .filter(|(kind, title)| is_integration_repair_unit(*kind, title))
        .count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// ADR-0120 D5: `conflict_files` の正規化（空行を除き、重複を除いてパス順に並べる）。
pub fn normalize_conflict_files(files: &[String]) -> Vec<String> {
    let set: std::collections::BTreeSet<&str> = files
        .iter()
        .map(|f| f.trim())
        .filter(|f| !f.is_empty())
        .collect();
    set.into_iter().map(str::to_string).collect()
}

/// ADR-0120 D4/D5: `Event::IntegrationRepairExhausted.reason`（固定値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationRepairExhaustReason {
    /// D4.1: 起票しようとした時点で上限（`MAX_INTEGRATION_REPAIRS`）に達していた。
    LimitReached,
    /// D4.2: 修復 WU が `plan_issue` で終えた。
    PlanIssue,
    /// D4.3: 修復 WU が `failed` になった。
    WorkUnitFailed,
    /// D4.3: 修復 WU が `budget_exhausted` のまま再開できない。
    BudgetExhausted,
    /// D4.4: 成果の保持が確認できない（祖先関係・dirty・rebase 進行中）。
    ResultUntrusted,
    /// D4.5: 衝突時の `rebase --abort` が失敗した。
    AbortFailed,
    /// D4.5: worktree が消えた、remote/shared workspace に変わった。
    WorktreeUnavailable,
}

impl IntegrationRepairExhaustReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LimitReached => "limit_reached",
            Self::PlanIssue => "plan_issue",
            Self::WorkUnitFailed => "work_unit_failed",
            Self::BudgetExhausted => "budget_exhausted",
            Self::ResultUntrusted => "result_untrusted",
            Self::AbortFailed => "abort_failed",
            Self::WorktreeUnavailable => "worktree_unavailable",
        }
    }
}

/// ADR-0120 D5: 最後の integration repair の結末。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationRepairState {
    Scheduled,
    Resolved,
    Exhausted,
}

/// ADR-0120 D5: events から組み立てた task の integration repair の現在の状況（`TaskDetail` の
/// 投影の材料。`max_attempts` は呼び出し側が `MAX_INTEGRATION_REPAIRS` を足す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRepairStatus {
    pub state: IntegrationRepairState,
    pub work_unit_id: Option<String>,
    pub attempt: u32,
    pub repo_id: crate::RepoId,
    /// 対応する scheduled event の値（scheduled の無い exhausted では `None`）。
    pub target_ref: Option<String>,
    /// resolved は再同期時、それ以外は最後の event の値。
    pub target_sha: String,
    pub before_sha: Option<String>,
    pub conflict_files: Vec<String>,
    pub reason: Option<IntegrationRepairExhaustReason>,
    pub rollback_to_sha: Option<String>,
    pub fallback: Option<bool>,
}

/// ADR-0120 D5: 起票時の snapshot（WU 応答の `integration_repair` 欄の材料）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRepairSnapshot {
    pub repo_id: crate::RepoId,
    pub target_ref: String,
    pub target_sha: String,
    pub before_sha: String,
    pub conflict_files: Vec<String>,
    pub attempt: u32,
}

/// ADR-0120 D5: `work_unit_id` ごとの起票時 snapshot（`IntegrationRepairScheduled` だけから読む。
/// WU の kind・title から推測しない）。
pub fn integration_repair_snapshots(
    events: &[crate::Event],
) -> std::collections::BTreeMap<String, IntegrationRepairSnapshot> {
    let mut out = std::collections::BTreeMap::new();
    for e in events {
        if let crate::Event::IntegrationRepairScheduled {
            work_unit_id,
            repo_id,
            target_ref,
            target_sha,
            before_sha,
            conflict_files,
            attempt,
            ..
        } = e
        {
            out.insert(
                work_unit_id.clone(),
                IntegrationRepairSnapshot {
                    repo_id: *repo_id,
                    target_ref: target_ref.clone(),
                    target_sha: target_sha.clone(),
                    before_sha: before_sha.clone(),
                    conflict_files: conflict_files.clone(),
                    attempt: *attempt,
                },
            );
        }
    }
    out
}

/// ADR-0120 D5: 最後の integration repair event と、対応する scheduled event から現在の状況を
/// 決定的に組み立てる（`events` は古い順）。履歴が無ければ `None`。
pub fn integration_repair_status(events: &[crate::Event]) -> Option<IntegrationRepairStatus> {
    let snapshots = integration_repair_snapshots(events);
    let last = events.iter().rev().find(|e| {
        matches!(
            e,
            crate::Event::IntegrationRepairScheduled { .. }
                | crate::Event::IntegrationRepairResolved { .. }
                | crate::Event::IntegrationRepairExhausted { .. }
        )
    })?;
    let snap = |wu: Option<&String>| wu.and_then(|id| snapshots.get(id));
    let status = match last {
        crate::Event::IntegrationRepairScheduled {
            work_unit_id,
            repo_id,
            target_ref,
            target_sha,
            before_sha,
            conflict_files,
            attempt,
            ..
        } => IntegrationRepairStatus {
            state: IntegrationRepairState::Scheduled,
            work_unit_id: Some(work_unit_id.clone()),
            attempt: *attempt,
            repo_id: *repo_id,
            target_ref: Some(target_ref.clone()),
            target_sha: target_sha.clone(),
            before_sha: Some(before_sha.clone()),
            conflict_files: conflict_files.clone(),
            reason: None,
            rollback_to_sha: None,
            fallback: None,
        },
        crate::Event::IntegrationRepairResolved {
            work_unit_id,
            repo_id,
            target_sha,
            attempt,
            ..
        } => {
            let s = snap(Some(work_unit_id));
            IntegrationRepairStatus {
                state: IntegrationRepairState::Resolved,
                work_unit_id: Some(work_unit_id.clone()),
                attempt: *attempt,
                repo_id: *repo_id,
                target_ref: s.map(|s| s.target_ref.clone()),
                target_sha: target_sha.clone(),
                before_sha: s.map(|s| s.before_sha.clone()),
                conflict_files: s.map(|s| s.conflict_files.clone()).unwrap_or_default(),
                reason: None,
                rollback_to_sha: None,
                fallback: None,
            }
        }
        crate::Event::IntegrationRepairExhausted {
            work_unit_id,
            repo_id,
            target_sha,
            before_sha,
            attempt,
            reason,
            rollback_to_sha,
            fallback,
        } => {
            let s = snap(work_unit_id.as_ref());
            IntegrationRepairStatus {
                state: IntegrationRepairState::Exhausted,
                work_unit_id: work_unit_id.clone(),
                attempt: *attempt,
                repo_id: *repo_id,
                target_ref: s.map(|s| s.target_ref.clone()),
                target_sha: target_sha.clone(),
                before_sha: Some(before_sha.clone()),
                conflict_files: s.map(|s| s.conflict_files.clone()).unwrap_or_default(),
                reason: Some(*reason),
                rollback_to_sha: rollback_to_sha.clone(),
                fallback: Some(*fallback),
            }
        }
        _ => return None,
    };
    Some(status)
}

/// ADR-0120 D2: integration repair WU の objective（決定的な文字列合成のみ。I/O は無い）。
/// `build_repair_objective` は再利用しない（検査の失敗ではなく、入力も違うため）。
/// `scope` が `Some` かつ空でなければ、`build_repair_objective` と同じ文面で許可範囲と範囲外の
/// 扱いを足す。
pub fn build_integration_repair_objective(
    target_ref: &str,
    target_sha: &str,
    before_sha: &str,
    conflict_files: &[String],
    task_title: &str,
    task_objective: &str,
    scope: Option<&RepairScope>,
) -> String {
    let objective_preview: String = task_objective.chars().take(600).collect();
    let mut s = String::new();
    s.push_str(&format!(
        "review 前の target 同期が衝突した。task の成果を保ったまま、`git rebase {target_sha}` を\
完了させよ。衝突の解消以外の変更をするな。\n\n"
    ));
    s.push_str(&format!(
        "## 分類\n{INTEGRATION_REPAIR_BUCKET}\n\n## 同期先\n- target_ref: {target_ref}\n- target_sha: {target_sha}\n\n"
    ));
    s.push_str(&format!(
        "## 衝突前の HEAD\n- before_sha: {before_sha}\n\
`git reset --hard`・`git checkout -- .`・`push --force` で成果を捨てるな。やり直すときは \
`git rebase --abort` で before_sha に戻れ。\n\n"
    ));
    s.push_str("## 衝突したファイル\n");
    for f in normalize_conflict_files(conflict_files) {
        s.push_str(&format!("- {f}\n"));
    }
    s.push_str(&format!(
        "\n## 解消の方針\n\
- 両側の意図を残せ。target 側の変更を消すな。\n\
- 解消後に `git rebase --continue` で rebase を完了せよ。\n\
- 次の検査を自分で実行して exit 0 を確かめよ:\n\
  - `git merge-base --is-ancestor {target_sha} HEAD`\n\
  - `test -z \"$(git status --porcelain)\"`\n"
    ));
    s.push_str(&format!(
        "\n## 対象タスク（参考。全文ではない）\n- title: {task_title}\n- objective（先頭 600 文字）: {objective_preview}\n"
    ));
    if let Some(scope) = scope
        && !scope.is_empty()
    {
        if !scope.allowed_paths.is_empty() {
            s.push_str("\n## 変更してよい範囲\n");
            for p in &scope.allowed_paths {
                s.push_str(&format!("- {p}\n"));
            }
        }
        if !scope.scope_checks.is_empty() {
            s.push_str("\n## 範囲外差分の検査\n");
            for c in &scope.scope_checks {
                s.push_str(&format!("- {c}\n"));
            }
            s.push_str("解消した後にこれも実行して exit を確かめよ。\n");
        }
    }
    s.push_str(
        "\n解消できない（両側の意図が矛盾する・設計判断が要る・許可範囲の外の衝突）なら、\
`git rebase --abort` で before_sha に戻してから、result.json に \
`{\"yield\":{\"plan_issue\":\"<どのファイルで何が矛盾したか>\"}}` を書いて終えよ。\n",
    );
    s
}

#[cfg(test)]
#[path = "execution/tests.rs"]
mod tests;
