//! ADR-0044 D2（Phase 53）: タスク単位のコメントと、**人のコメントの効き方**（決定的）。
//!
//! | タスクの状態 | 何が起きる |
//! |---|---|
//! | `running` / `reviewing` | `Trigger::Interrupt` で `ready` に戻す（attempts 据え置き、理由 `comment`）。
//!   `Event::WorkerFinished{outcome: "interrupted: comment"}` を同じトランザクションで積む。報告は作らない |
//! | `blocked` | `answer` と同じ（ADR-0021 D2。質問への回答として渡す） |
//! | `ready` / `draft` | コメントを記録するだけ（次の run の前置きに載る） |
//! | `done` / `failed` / `cancelled` | コメントを記録するだけ（GUI が「再開」を出す。`reopen` は別の API） |
//!
//! 走っている run を実際に殺すのは**ディスパッチャ**（`abort_stale_runs`: ストア上で `running` で
//! なくなった run の `JoinHandle` を `abort()` する。tokio の `kill_on_drop` が**子プロセスに SIGKILL**
//! を送る）。**cancel と同じ経路**で、ADR-0044 D2 の「SIGTERM → `kill_grace_secs`」には**まだなって
//! いない**（cancel も昔から同じ。孫プロセスは残る。`agent-docs/progress/2026-10-02-docs-layout/refs-crates.md` の設計経緯）。
//! 打ち切ったタスクは同じ tick では dispatch し直さない（同じ worktree に 2 つの書き手を入れないため）。
//! ここは状態とコメントだけを決定的に書く。LLM は呼ばない（DESIGN 原則 1）。

use task_core::comment::{CommentAuthorKind, TaskComment};
use task_core::{Event, Status, TaskId, TaskStore, Trigger};
use time::OffsetDateTime;

use crate::error::OpsError;
use crate::gate::{self, TransitionResult};

/// 割り込みで止めた run の `WorkerFinished.outcome`（ADR-0044 D2 / D8）。
/// **`bad_news` にも `error_cooldown` にも数えない**（`classify_outcome` が `Interrupted` に分類する）。
pub const INTERRUPTED_OUTCOME: &str = "interrupted: comment";

/// 割り込みで止めた run の `outcome` の接頭辞（`view::classify_outcome` / `stats::classify_outcome` と対）。
pub const INTERRUPTED_OUTCOME_PREFIX: &str = "interrupted: ";

/// 人のコメントが何を起こしたか（ADR-0044 D2 の表）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CommentEffect {
    /// 記録しただけ（`ready` / `draft`）。
    Stored,
    /// 走っていた run を止めて `ready` に戻した（`running` / `reviewing`）。
    Interrupted,
    /// 質問への回答として渡した（`blocked`）。
    Answered,
    /// 終端なので記録しただけ。GUI は「再開」を出せる（`done` / `failed`。`cancelled` は再開できない）。
    Terminal,
}

/// `POST /tasks/{id}/comments` の結果。
#[derive(Debug, Clone, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct CommentResult {
    pub comment: TaskComment,
    pub effect: CommentEffect,
    /// 状態が動いたときだけ（`Interrupted` / `Answered`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<TransitionResult>,
    /// `Terminal` のとき、`POST /tasks/{id}/reopen` が使えるか（`cancelled` は `false`）。
    pub can_reopen: bool,
}

/// 人のコメント（ADR-0044 D2）。状態に応じて割り込み・回答・記録を決定的に行う。
pub fn post_human_comment(
    store: &dyn TaskStore,
    id: TaskId,
    body: String,
    now: OffsetDateTime,
) -> Result<CommentResult, OpsError> {
    post_human_comment_as(store, id, None, body, now)
}

/// ADR-0056 Phase 101: `post_human_comment` と同じ効き方（`ADR-0044 D2` の表）だが、
/// `author`（`CommentAuthorKind::Human` の `TaskComment.author`）を明示できる。MCP の
/// `task_comment` は `Some("mcp:<client_id>")` を渡す（`knowledge_propose` の `mcp:chatgpt` と
/// 同じ流儀）。GUI/HTTP の `POST /tasks/{id}/comments`（`post_human_comment`）は `None` のまま
/// （挙動もフィールドの見え方も、この Phase では 1 バイトも変えない）。
pub fn post_human_comment_as(
    store: &dyn TaskStore,
    id: TaskId,
    author: Option<String>,
    body: String,
    now: OffsetDateTime,
) -> Result<CommentResult, OpsError> {
    let plan = plan_human_comment(store, id, author, body, now)?;
    let outcome = store.comment_add(&plan.comment, plan.transition.clone())?;
    finish_human_comment(store, plan, outcome)
}

/// 人のコメントの書き込み計画（読むだけ）。`comment_add` に `comment` と `transition` を渡して書き、
/// 結果を [`finish_human_comment`] に渡す。CoS の操作（ADR 2026-10-05 D3）は同じ計画を監査と同じ
/// トランザクションで `SqliteStore::comment_add_tx` に渡す。
#[derive(Debug, Clone)]
pub struct HumanCommentPlan {
    pub comment: TaskComment,
    pub transition: Option<(Trigger, Vec<Event>)>,
    effect: CommentEffect,
    from: Status,
}

impl HumanCommentPlan {
    pub fn task_id(&self) -> TaskId {
        self.comment.task_id
    }
}

pub fn plan_human_comment(
    store: &dyn TaskStore,
    id: TaskId,
    author: Option<String>,
    body: String,
    now: OffsetDateTime,
) -> Result<HumanCommentPlan, OpsError> {
    TaskComment::validate_body(&body).map_err(OpsError::Validation)?;
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    let comment = TaskComment::new(
        id,
        CommentAuthorKind::Human,
        author,
        body.clone(),
        None,
        now,
    );
    let (effect, transition) = match task.status {
        // 走っている（レビュー中も含む）run を止めて `ready` に戻す。コメントと遷移は同じトランザクション。
        Status::Running | Status::Reviewing => {
            // `WorkerFinished{outcome: "interrupted: comment"}` は**いま走っているワーカー run**
            // （= リースの `worker_run_id`）にだけ付ける。`reviewing` のタスクのリースは既に外れていて、
            // 直近のワーカー run は `done: …` で終わっているので、そこに後から `interrupted` を重ねると
            // その run の記録（`outcome` / `finished_at` / `usage`）を壊す（Phase 53 の監査で発見）。
            // `reviewing` の割り込みではイベントを足さず、遷移（`Transitioned{reason:"comment"}`）だけ残す。
            let extra = match task.lease.as_ref().map(|l| l.worker_run_id.clone()) {
                Some(run_id) => vec![Event::WorkerFinished {
                    run_id,
                    outcome: INTERRUPTED_OUTCOME.to_string(),
                    usage: None,
                    role: None,
                    metrics: None,
                    end: None,
                }],
                None => Vec::new(),
            };
            (
                CommentEffect::Interrupted,
                Some((Trigger::Interrupt, extra)),
            )
        }
        // ADR-0021 D2: 質問待ちのタスクへのコメントは「回答」そのもの。
        // コメント・`Trigger::Answer`・`Event::Answered` を**同じトランザクション**で書く
        // （`gate::answer` を呼ぶと 2 つ目の tx になり、その間に blocked を抜けると
        // 「コメントだけ残って回答が消える」ので。Phase 53 の監査で発見）。
        Status::Blocked => {
            let question = crate::derive::latest_question(&store.events_for(id)?);
            let answered = Event::Answered {
                question,
                answer: body,
            };
            (
                CommentEffect::Answered,
                Some((Trigger::Answer, vec![answered])),
            )
        }
        Status::Done | Status::Failed | Status::Cancelled => (CommentEffect::Terminal, None),
        Status::Draft | Status::Ready => (CommentEffect::Stored, None),
    };
    Ok(HumanCommentPlan {
        comment,
        transition,
        effect,
        from: task.status,
    })
}

/// 書いた後の結果と後始末（`blocked` への回答は承認待ちを片付ける。別の書き込みで、冪等）。
pub fn finish_human_comment(
    store: &dyn TaskStore,
    plan: HumanCommentPlan,
    outcome: Option<task_core::transition::Outcome>,
) -> Result<CommentResult, OpsError> {
    let id = plan.comment.task_id;
    let transition = match plan.effect {
        CommentEffect::Interrupted | CommentEffect::Answered => {
            let what = if plan.effect == CommentEffect::Answered {
                "answer"
            } else {
                "interrupt"
            };
            let outcome = outcome
                .ok_or_else(|| OpsError::Validation(format!("{what} produced no outcome")))?;
            Some(TransitionResult {
                id,
                from: plan.from,
                to: outcome.next,
                reason: outcome.reason.to_string(),
                cascaded: Vec::new(),
            })
        }
        CommentEffect::Stored | CommentEffect::Terminal => None,
    };
    if plan.effect == CommentEffect::Answered {
        // GUI 監査 H2 と同じ後始末（`gate::answer` と同じ関数。別の書き込みで、冪等）。
        gate::settle_pending_approvals(store, id, &plan.comment.body)?;
    }
    Ok(CommentResult {
        effect: plan.effect,
        transition,
        can_reopen: plan.effect == CommentEffect::Terminal && plan.from != Status::Cancelled,
        comment: plan.comment,
    })
}

/// 組織の「人」（ワーカーの `{"type":"comment"}` 行、秘書・lead が委譲先に書くもの）のコメント。
/// **人を起こさない**（状態は変えない。通知は ADR-0037 の 5 種のまま）。
pub fn post_node_comment(
    store: &dyn TaskStore,
    id: TaskId,
    author: Option<String>,
    run_id: Option<String>,
    body: String,
    now: OffsetDateTime,
) -> Result<TaskComment, OpsError> {
    TaskComment::validate_body(&body).map_err(OpsError::Validation)?;
    let comment = TaskComment::new(id, CommentAuthorKind::Node, author, body, run_id, now);
    store.comment_add(&comment, None)?;
    Ok(comment)
}

/// そのタスクのコメント（古い順）。
pub fn list_comments(store: &dyn TaskStore, id: TaskId) -> Result<Vec<TaskComment>, OpsError> {
    if store.get(id)?.is_none() {
        return Err(OpsError::NotFound(id));
    }
    Ok(store.comments_for(id)?)
}

/// ADR-0044 D2: `POST /tasks/{id}/reopen`。`done` / `failed` を `ready` に戻す（attempts は 0）。
/// `cancelled` は worktree もブランチも消してあるので再開しない（409）。
pub fn reopen(
    store: &dyn TaskStore,
    id: TaskId,
    expected: Option<Status>,
) -> Result<TransitionResult, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    if let Some(exp) = expected
        && exp != task.status
    {
        return Err(OpsError::Conflict {
            expected: exp,
            actual: task.status,
        });
    }
    if !matches!(task.status, Status::Done | Status::Failed) {
        return Err(OpsError::InvalidState {
            id,
            context: format!("status={:?}", task.status),
            action: "reopened; only done or failed tasks can be reopened".to_string(),
        });
    }
    let from = task.status;
    let outcome = store.apply_transition(id, Trigger::Reopen, None)?;
    // ADR 2026-10-04 Phase 3: 再開は run の履歴を区切る。ここで届いている結果を追記する（冪等）。
    crate::routing_outcome::record_routing_outcomes(store, id)?;
    Ok(TransitionResult {
        id,
        from,
        to: outcome.next,
        reason: outcome.reason.to_string(),
        cascaded: Vec::new(),
    })
}

/// ADR-0044 D2: **直前の run を止めた人のコメント**（次の run の前置きの先頭に載せるもの）。
///
/// 決定的な規則（純粋関数）:
/// 1. 最後の `Transitioned{reason: "comment", to: ready}`（= `Interrupt`）を探す。無ければ `None`。
/// 2. それより後に「**その割り込みを受け取った run が終わった**」印があれば、もう消化済みなので `None`。
///    印は `WorkerFinished` のうち、`interrupted: ` で始まるもの（割り込みそのもの）・`requeue: `
///    （ADR-0010 P-21: 供給側の失敗。モデルは前置きを見ていない）・`lease_expired`（デーモンの死）を
///    **除いた**もの。`Transitioned{to: running}`（dispatch）や `WorkerStarted` も消化ではない
///    （割り込みの次の run は、まさにこの前置きを受け取る run だから）。
///    requeue / lease_expired を除かないと、割り込みの直後にレート制限が 1 回起きただけで
///    「前置きの先頭に載る」が消える（Phase 53 の監査で発見）。
/// 3. 残っていれば、その割り込みを起こした人のコメント = 最後の `author_kind = human` のコメント
///    （割り込みの後に人がさらに書き足していれば、その最新のもの。どちらも人が読ませたい文なので
///    先頭に出してよい、という判断）。
pub fn interrupting_comment<'a>(
    events: &[(u64, Event)],
    comments: &'a [TaskComment],
) -> Option<&'a TaskComment> {
    let idx = events.iter().rposition(|(_, e)| {
        matches!(e, Event::Transitioned { reason, to, .. } if reason == "comment" && *to == Status::Ready)
    })?;
    let consumed = events[idx + 1..].iter().any(
        |(_, e)| matches!(e, Event::WorkerFinished { outcome, .. } if consumes_interrupt(outcome)),
    );
    if consumed {
        return None;
    }
    comments
        .iter()
        .rev()
        .find(|c| c.author_kind == CommentAuthorKind::Human)
}

/// `WorkerFinished.outcome` が「割り込みを受け取った run が実際に走って終わった」印か（純粋関数）。
/// 割り込みそのもの・供給側の requeue・リース切れは**走っていない**ので印にならない。
fn consumes_interrupt(outcome: &str) -> bool {
    !outcome.starts_with(INTERRUPTED_OUTCOME_PREFIX)
        && !outcome.starts_with("requeue: ")
        && outcome != "lease_expired"
}

/// ADR-0054 Phase 113 D3: そのタスクが `failed` になった直近の遷移が `Trigger::ReviewFail`
/// （`Event::Transitioned{reason: "review_fail", to: Failed}`）か（純粋関数）。`ReviewFail` は
/// `task.status == Reviewing` のときしか発火せず（`transition.rs`）、`Reviewing` に入るのは実装 run が
/// `done`（`Trigger::WorkerDone`）した後だけなので、これが `true` なら「実装は最後まで通ったが、
/// review の判定（reviewer 条件・command 条件・human 条件のどれか）が不合格だった」ことを意味する。
/// 実装 run 自体が供給側失敗の requeue 上限や `max_retries` の枯渇で `failed` になった場合
/// （`Trigger::Requeue`/`WorkerError` 経由）はこの条件を満たさない（reopen で仕切り直すべき）。
pub fn review_fail_is_the_only_reason_for_failure(events: &[(u64, Event)]) -> bool {
    events.iter().rev().find_map(|(_, e)| match e {
        Event::Transitioned { reason, .. } => Some(reason.as_str()),
        _ => None,
    }) == Some("review_fail")
}

/// 既存成果を部署のレビュアーで再判定する。新しい実装runは作らない。
///
/// ADR-0054 Phase 113 D3 追記: `done` からに加えて、`failed` からも許す。ただし
/// [`review_fail_is_the_only_reason_for_failure`] が `true` のとき（＝直前の実装 run は `done` で、
/// review 判定だけが不合格だった。それ以外の理由で `failed` になったタスク、例えば実装 run 自体が
/// 供給側失敗の requeue 上限に達した場合は対象外）だけに限る。`task_core::transition::transition`
/// （純粋関数、event 履歴を持たない）は `Status::Failed` からの遷移そのものは許しているので、
/// ここで event 履歴を見て絞り込む。人の承認（`Check::Human`）が既に `Done` の `Approval` 子タスクで
/// 承認済みなら、`attempts` を変えずに再判定することで（`human_approval_title` が `attempts` を含むため）
/// ディスパッチャの `resolve_human_approvals` が同じ子タスクを見つけて再利用する（人に二度承認させない。
/// D2 と同じ考え方）。
/// ADR-0070 D2（Phase 116）: [`rereview`] が検証する条件を、状態を変えずに真偽値として返す
/// （純粋関数）。一覧・受信箱・タスク詳細が「再レビュー」ボタンを出してよいかを判定するのに使う
/// （`rereview` 自身の詳細なエラー文言はここでは作らない。二重実装を避けるため、条件は
/// `rereview` と同じものをここに集約した）。
pub fn can_rereview(task: &task_core::Task, events: &[(u64, Event)]) -> bool {
    if !task
        .acceptance
        .iter()
        .any(|c| matches!(c.check, task_core::Check::Reviewer))
        || task_core::support_kind(task).is_some()
    {
        return false;
    }
    match task.status {
        Status::Done => true,
        Status::Failed => review_fail_is_the_only_reason_for_failure(events),
        _ => false,
    }
}

pub fn rereview(
    store: &dyn TaskStore,
    id: TaskId,
    expected: Option<Status>,
) -> Result<TransitionResult, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    if let Some(exp) = expected
        && exp != task.status
    {
        return Err(OpsError::Conflict {
            expected: exp,
            actual: task.status,
        });
    }
    if !task
        .acceptance
        .iter()
        .any(|c| matches!(c.check, task_core::Check::Reviewer))
        || task_core::support_kind(&task).is_some()
    {
        return Err(OpsError::Validation(
            "reviewer条件を持つ通常タスクだけを再レビューできます".into(),
        ));
    }
    if task.status == Status::Failed {
        let events = store.events_for(id)?;
        if !review_fail_is_the_only_reason_for_failure(&events) {
            return Err(OpsError::Validation(
                "failedなタスクを再レビューできるのは、直前の実装runがdoneでreview判定だけが\
                 不合格だった場合だけです（それ以外の理由でfailedになった場合はreopenしてください）"
                    .into(),
            ));
        }
    }
    let outcome = store.apply_transition(id, Trigger::Rereview, None)?;
    Ok(TransitionResult {
        id,
        from: task.status,
        to: outcome.next,
        reason: outcome.reason.into(),
        cascaded: Vec::new(),
    })
}

#[cfg(test)]
mod tests;
