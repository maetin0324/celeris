//! `events` からの派生ビュー（DESIGN.md §5.2/§5.9, ADR-0010 D3/D5/D6/D7, ADR-0011, ADR-0013 D7）。
//!
//! 元は `task-dispatch::dispatcher` と `celerisctl` の `commands/gate.rs` にあった純粋関数をそのまま移した。
//! `task-core` の型だけを使う（`task_worker::{PriorReview, Answer}` は使わない。ワーカープロトコルの
//! 型への写像は呼び出し側 — `task-dispatch` の dispatcher や `celerisctl` の `worker.rs` — で行う）。

use std::collections::HashMap;
use std::time::Duration;

use schemars::JsonSchema;
use serde::Serialize;
use task_core::{ArtifactRef, Event, RunRole, Status, Task};

/// `prior_review_from_events` の要素。`task_worker::PriorReview` と同じ形（フィールド名も同じ）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ReviewNote {
    pub criterion: usize,
    pub pass: bool,
    pub reason: String,
}

/// `answers_from_events` の要素。`task_worker::Answer` と同じ形（フィールド名も同じ）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct AnswerNote {
    pub question: String,
    pub answer: String,
}

/// 直前のレビュー（最後に `ReviewVerdict` を記録した run の全判定）を `context.prior_review` に写す。
pub fn prior_review_from_events(events: &[(u64, Event)]) -> Vec<ReviewNote> {
    let mut by_run: HashMap<&str, Vec<ReviewNote>> = HashMap::new();
    let mut last_run: Option<&str> = None;
    for (_, ev) in events {
        if let Event::ReviewVerdict {
            run_id,
            criterion_idx,
            pass,
            reason,
        } = ev
        {
            by_run.entry(run_id.as_str()).or_default().push(ReviewNote {
                criterion: *criterion_idx,
                pass: *pass,
                reason: reason.clone(),
            });
            last_run = Some(run_id.as_str());
        }
    }
    let mut out = last_run.and_then(|r| by_run.remove(r)).unwrap_or_default();
    out.sort_by_key(|p| p.criterion);
    out
}

/// そのタスクの全 `Event::Answered` を時系列で `context.answers` に写す（ADR-0010 D3, P-10）。
pub fn answers_from_events(events: &[(u64, Event)]) -> Vec<AnswerNote> {
    events
        .iter()
        .filter_map(|(_, ev)| match ev {
            Event::Answered { question, answer } => Some(AnswerNote {
                question: question.clone(),
                answer: answer.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// 現在の試行での連続 requeue 回数（ADR-0011 D2）。`Transitioned` を新しい順に見て `requeue` を数え、
/// `dispatch` は読み飛ばし、それ以外の reason で止まる。
pub fn consecutive_requeues(events: &[(u64, Event)]) -> u32 {
    let mut n = 0;
    for (_, ev) in events.iter().rev() {
        if let Event::Transitioned { reason, .. } = ev {
            match reason.as_str() {
                "requeue" => n += 1,
                "dispatch" => {}
                _ => break,
            }
        }
    }
    n
}

/// Reviewer run の供給側失敗で延期したときの `WorkerProgress.msg` の接頭辞（ADR-0010 D5）。
pub const REVIEWER_REQUEUED_PREFIX: &str = "reviewer run requeued: ";

/// 現在の reviewing での、Reviewer run の供給側失敗による連続延期回数（ADR-0011 D2）。
/// 最後の `Transitioned`（reviewing に入った遷移）以降の延期の `WorkerProgress` を数える。
pub fn consecutive_reviewer_requeues(events: &[(u64, Event)]) -> u32 {
    let mut n = 0;
    for (_, ev) in events.iter().rev() {
        match ev {
            Event::Transitioned { .. } => break,
            Event::WorkerProgress { msg, .. } if msg.starts_with(REVIEWER_REQUEUED_PREFIX) => {
                n += 1
            }
            _ => {}
        }
    }
    n
}

/// ADR-0054 D2（Phase 113）: Reviewer run **自身のインフラ都合の失敗**（`is_error` の結果・
/// プロセス失敗・resume 拒否など。プロバイダが分類できた供給側失敗＝[`consecutive_reviewer_requeues`]
/// とは別物）で延期したときの `WorkerProgress.msg` の接頭辞。
pub const REVIEWER_INFRA_FAILURE_PREFIX: &str = "reviewer run infra failure: ";

/// 現在の reviewing での、Reviewer run 自身のインフラ都合の失敗による連続延期回数
/// （[`consecutive_reviewer_requeues`] と同じ数え方。最後の `Transitioned` 以降の
/// [`REVIEWER_INFRA_FAILURE_PREFIX`] 付き `WorkerProgress` を数える。プロバイダの供給側失敗の
/// カウンタとは別に、`[review] max_reviewer_retries` と組み合わせて使う）。
pub fn consecutive_reviewer_infra_failures(events: &[(u64, Event)]) -> u32 {
    let mut n = 0;
    for (_, ev) in events.iter().rev() {
        match ev {
            Event::Transitioned { .. } => break,
            Event::WorkerProgress { msg, .. } if msg.starts_with(REVIEWER_INFRA_FAILURE_PREFIX) => {
                n += 1
            }
            _ => {}
        }
    }
    n
}

/// ADR-0070 D3（Phase 116）: `Trigger::InfraRequeue`（`Event::Transitioned.reason ==
/// "infra_requeue"`）による、現在の試行での連続再試行回数。`consecutive_requeues` と対称
/// （新しい順に `infra_requeue` を数え、`dispatch` は読み飛ばし、それ以外の reason で止まる）。
/// `[dispatch] max_infra_retries` と組み合わせて使う。
pub fn consecutive_infra_requeues(events: &[(u64, Event)]) -> u32 {
    let mut n = 0;
    for (_, ev) in events.iter().rev() {
        if let Event::Transitioned { reason, .. } = ev {
            match reason.as_str() {
                "infra_requeue" => n += 1,
                "dispatch" => {}
                _ => break,
            }
        }
    }
    n
}

/// ADR-0070 D3: インフラ都合の再試行のバックオフ（1 回目 30 秒、2 回目 2 分、3 回目以降は 5 分固定）。
/// `n` は「これから何回目の再試行か」（`consecutive_infra_requeues` の値 + 1）。`n == 0` は 0 秒。
pub fn infra_backoff_delay(n: u32) -> Duration {
    match n {
        0 => Duration::ZERO,
        1 => Duration::from_secs(30),
        2 => Duration::from_secs(120),
        _ => Duration::from_secs(300),
    }
}

/// ADR-0072 D18（Phase E1）: 現在の試行での連続 continuation 回数（`Trigger::Continue` の
/// `reason == "continue"`）。`consecutive_requeues` / `consecutive_infra_requeues` と対称
/// （新しい順に数え、`dispatch` は読み飛ばし、それ以外の reason — `answer` を含む — で止まる。D18:
/// 「人が回答すると、その窓のカウンタは回答の時点から数え直す」）。
pub fn consecutive_continuations(events: &[(u64, Event)]) -> u32 {
    let mut n = 0;
    for (_, ev) in events.iter().rev() {
        if let Event::Transitioned { reason, .. } = ev {
            match reason.as_str() {
                "continue" => n += 1,
                // ADR-0090 D1: クラスタ job の wait とその再開は数えず、窓も切らない（`dispatch` と同じ）。
                "dispatch"
                | task_core::cluster_job::REASON_WAITING
                | task_core::cluster_job::REASON_RESUME => {}
                _ => break,
            }
        }
    }
    n
}

/// ADR-0090 D1: 進捗の比較に使う直近の checkpoint（[`latest_checkpoint`] から、クラスタ job の wait で止めた run の
/// checkpoint〈`end = waiting`〉を除いたもの）。wait の checkpoint は job を投げただけの時点なので、続きの run の
/// 進捗の基準にしない。
pub fn latest_progress_checkpoint(
    events: &[(u64, Event)],
    work_unit_id: Option<&str>,
) -> Option<task_core::Checkpoint> {
    events.iter().rev().find_map(|(_, ev)| match ev {
        Event::CheckpointSaved {
            work_unit_id: wu,
            checkpoint,
            ..
        } if wu.as_deref() == work_unit_id
            && checkpoint.end != task_core::CheckpointEnd::Waiting =>
        {
            Some((**checkpoint).clone())
        }
        _ => None,
    })
}

/// ADR-0072 D5/D8（Phase E1/E2）: 直近の `Event::CheckpointSaved` の checkpoint（無ければ `None`）。
/// `work_unit_id = None` は暗黙の WorkUnit（E1 と同じ）、`Some(id)` はその WorkUnit の run だけを対象に
/// する（E2 の申し送り: WU 単位に拡張）。
pub fn latest_checkpoint(
    events: &[(u64, Event)],
    work_unit_id: Option<&str>,
) -> Option<task_core::Checkpoint> {
    events.iter().rev().find_map(|(_, ev)| match ev {
        Event::CheckpointSaved {
            work_unit_id: wu,
            checkpoint,
            ..
        } if wu.as_deref() == work_unit_id => Some((**checkpoint).clone()),
        _ => None,
    })
}

/// ADR-0072 D18（Phase E1/E2）: 「最後の `answer` 以降」の窓の中で、連続して進捗のなかった
/// checkpoint の数（`task_core::checkpoint_shows_progress` で判定）。「回答の時点から数え直す」
/// （D18）ので、窓に入って最初の checkpoint は**窓の外の checkpoint とは比べない**（`prev = None`
/// と同じ扱い＝常に「進捗あり」からやり直す）。`no_progress_limit` と組み合わせて使う。
/// `work_unit_id` は [`latest_checkpoint`] と同じ（`None` = 暗黙の WorkUnit、`Some(id)` = その WU だけ）。
pub fn no_progress_streak(events: &[(u64, Event)], work_unit_id: Option<&str>) -> u32 {
    let mut window_start_seq: u64 = 0;
    for (seq, ev) in events {
        if let Event::Transitioned { reason, .. } = ev
            && reason == "answer"
        {
            window_start_seq = *seq;
        }
    }
    let mut streak: u32 = 0;
    let mut prev: Option<task_core::Checkpoint> = None;
    let mut baseline_reset = false;
    for (seq, ev) in events {
        if let Event::CheckpointSaved {
            work_unit_id: wu,
            checkpoint,
            ..
        } = ev
            && wu.as_deref() == work_unit_id
            // ADR-0090 D1: クラスタ job の wait で止めた run の checkpoint は数えない（基準にもしない）。
            && checkpoint.end != task_core::CheckpointEnd::Waiting
        {
            let in_window = *seq >= window_start_seq;
            if in_window && !baseline_reset {
                // 窓に入って最初の checkpoint: 窓の外の checkpoint とは比べない（D18「数え直す」）。
                prev = None;
                baseline_reset = true;
            }
            let progressed = task_core::checkpoint_shows_progress(prev.as_ref(), checkpoint);
            if in_window {
                streak = if progressed { 0 } else { streak + 1 };
            }
            prev = Some((**checkpoint).clone());
        }
    }
    streak
}

/// ADR-0070 D1（Phase 116）: `failed` の分類。`infra` はレース・切替・供給側都合、`work` はレビュー
/// 不合格やワーカー自身の明示的な失敗（人が中身を見て判断すべきもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Infra,
    Work,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureClass::Infra => "infra",
            FailureClass::Work => "work",
        }
    }
}

/// ADR-0070 D3 が最終的な失敗（`WorkerError{retryable:false}`）に付ける接頭辞
/// （ADR-0054 D2 の reviewer 側 `"reviewer infra failure ×N"` と同じ書式）。
pub const INFRA_FAILURE_MARKER: &str = "infra failure ×";
/// 供給側失敗の requeue 上限到達（ADR-0011 P-38）の文言。人の目には「レート制限が続いた」ことを
/// 意味するので、これも `infra` に分類する。
const REQUEUE_LIMIT_MARKER: &str = "requeue limit (";

/// 文字列の最初の行（前後の空白を落とす）。空なら空文字列のまま。
fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

/// ADR-0070 D1 追記（Phase 116。本番で確認: 2026-09-24 09:21Z、`/home` が満杯になり run が
/// `adapter: io error: No space left on device (os error 28)` で 2 回失敗した）: OS のディスク
/// 不足エラー文言。`std::io::Error`（`ENOSPC`）の `Display` がこの文字列を含む。ディスク残量の事前
/// チェックは別 Phase（今回は分類と文言だけ）。
const DISK_FULL_MARKER: &str = "No space left on device";

/// ディスク不足なら（`class` に関わらず）`infra` に強制し、人が読む理由の先頭に
/// 「ディスク不足: 」を足す。それ以外はそのまま返す。
fn annotate_disk_full(class: FailureClass, reason: String) -> (FailureClass, String) {
    if reason.contains(DISK_FULL_MARKER) && !reason.starts_with("ディスク不足: ") {
        (FailureClass::Infra, format!("ディスク不足: {reason}"))
    } else {
        (class, reason)
    }
}

/// ADR-0070 D1: `failed` に落ちた理由を分類し、人が読む 1 行を作る（純粋関数。`events` は `failed` の
/// タスクの全イベント、新しい順に走査する）。`Failed` でないタスクに対して呼んでも構わない
/// （その場合は最後に見つかった `WorkerFinished`/`ReviewVerdict` から意味のない分類を返すだけ）。
pub fn classify_task_failure(events: &[(u64, Event)]) -> (FailureClass, String) {
    let last_transition_reason = events.iter().rev().find_map(|(_, e)| match e {
        Event::Transitioned { reason, .. } => Some(reason.as_str()),
        _ => None,
    });
    if last_transition_reason == Some("review_fail") {
        if let Some(reason) = events.iter().rev().find_map(|(_, e)| match e {
            Event::ReviewVerdict {
                pass: false,
                reason,
                ..
            } => Some(reason.as_str()),
            _ => None,
        }) {
            return annotate_disk_full(FailureClass::Work, first_line(reason));
        }
        return (FailureClass::Work, "レビュー不合格".to_string());
    }
    for (_, e) in events.iter().rev() {
        if let Event::WorkerFinished {
            outcome,
            role: None,
            ..
        } = e
        {
            if outcome.starts_with(INFRA_FAILURE_MARKER) || outcome.contains(REQUEUE_LIMIT_MARKER) {
                return annotate_disk_full(FailureClass::Infra, first_line(outcome));
            }
            return annotate_disk_full(FailureClass::Work, first_line(outcome));
        }
    }
    (FailureClass::Work, "原因不明の失敗".to_string())
}

/// ADR-0010 D6（P-3）: `min(base·2^(attempts-1), max)`。`attempts == 0` または `base == 0` なら 0。
pub fn retry_backoff(base: Duration, max: Duration, attempts: u32) -> Duration {
    if attempts == 0 || base.is_zero() {
        return Duration::ZERO;
    }
    let factor = 1u32.checked_shl(attempts - 1).unwrap_or(u32::MAX);
    base.checked_mul(factor).unwrap_or(max).min(max)
}

/// その run で `ArtifactProduced` された成果物。
pub fn artifacts_for_run(events: &[(u64, Event)], run_id: &str) -> Vec<ArtifactRef> {
    events
        .iter()
        .filter_map(|(_, ev)| match ev {
            Event::ArtifactProduced {
                run_id: r,
                artifact,
            } if r == run_id => Some(artifact.clone()),
            _ => None,
        })
        .collect()
}

/// ADR-0067 D4: その run の `ArtifactProduced` を、`GET /tasks/{id}/artifacts/{idx}` と同じ添字
/// （`ArtifactProduced` を**全 run を通じて**出現順に 0 始まりで数えたもの。`crates/task-api/src/files.rs`
/// の `produced(rows).enumerate()` と同じ規則）付きで返す。GUI が承認画面からその場で本文を取りに行くのに使う。
pub fn artifacts_for_run_with_idx(
    events: &[(u64, Event)],
    run_id: &str,
) -> Vec<(usize, ArtifactRef)> {
    events
        .iter()
        .filter_map(|(_, ev)| match ev {
            Event::ArtifactProduced {
                run_id: r,
                artifact,
            } => Some((r, artifact)),
            _ => None,
        })
        .enumerate()
        .filter(|(_, (r, _))| *r == run_id)
        .map(|(idx, (_, artifact))| (idx, artifact.clone()))
        .collect()
}

/// `role` が Reviewer run を指すか（`None` はワーカー run。ADR-0014 D1）。
pub fn is_reviewer(role: Option<RunRole>) -> bool {
    role == Some(RunRole::Reviewer)
}

/// 最後に `WorkerStarted` した**ワーカー** run の id（Reviewer run は除く。ADR-0014 D1）。
pub fn last_run_id(events: &[(u64, Event)]) -> Option<String> {
    events.iter().rev().find_map(|(_, ev)| match ev {
        Event::WorkerStarted { run_id, role, .. } if !is_reviewer(*role) => Some(run_id.clone()),
        _ => None,
    })
}

/// ADR-0072 D5（Phase E1）: 暗黙の WorkUnit での「このタスクの中の連番」（`WorkerStarted.run_seq`
/// を DB の列としては持たず、events から数える。D5 は「WorkUnit の中の Run #n」と書いているが、
/// 暗黙の WU では「タスク内の連番」と同義）。Reviewer run は数えない（ADR-0014 D1）。`events` に
/// 最後に始まったワーカー run の `WorkerStarted` が含まれていれば、その run の番号（1 始まり）を返す。
pub fn current_run_seq(events: &[(u64, Event)]) -> u32 {
    events
        .iter()
        .filter(|(_, ev)| matches!(ev, Event::WorkerStarted { role, .. } if !is_reviewer(*role)))
        .count() as u32
}

/// `Human` criterion 用の `Approval` 子タスクの `title`（既存子の照合キーにも使う。ADR-0008 D2）。
/// ADR-0010 D8（P-35）: 試行（`attempts + 1`）を含めるので、再レビューでは新しい子が作られる。
pub fn human_approval_title(task: &Task, idx: usize) -> String {
    format!(
        "Approval needed: {} — criterion {idx} (attempt {})",
        task.title,
        task.attempts + 1
    )
}

/// 直近の `Event::ApprovalDecided` の `note` を `": <note>"` の形で返す（無ければ空文字列）。
pub fn approval_decision_note(events: &[(u64, Event)]) -> String {
    events
        .iter()
        .rev()
        .find_map(|(_, e)| match e {
            Event::ApprovalDecided { note: Some(n), .. } => Some(format!(": {n}")),
            Event::ApprovalDecided { note: None, .. } => Some(String::new()),
            _ => None,
        })
        .unwrap_or_default()
}

/// 直近の `WorkerFinished{outcome}` のうち `"question: "` で始まるものから、接頭辞を
/// 除いた質問文を取り出す（ADR-0010 D3）。`events_for` を後ろから見て最初に見つかった
/// ものを使う。無ければ空文字列。Reviewer run の `WorkerFinished` は見ない（ADR-0014 D1）。
pub fn latest_question(events: &[(u64, Event)]) -> String {
    events
        .iter()
        .rev()
        .find_map(|(_, event)| match event {
            Event::WorkerFinished { outcome, role, .. } if !is_reviewer(*role) => {
                outcome.strip_prefix("question: ").map(str::to_string)
            }
            // ADR-0021 D2: ディスパッチャが出した質問（run の終了ではない）。
            Event::QuestionRaised { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// ADR-0062 Phase 108: 直近の `Blocked` への遷移が `Trigger::Unroutable`（reason `"unroutable"`）に
/// よるものか。B1（担当に `cluster:<id>` が無い）と ADR-0046 D5（担当の候補が無い）はどちらも同じ
/// trigger を使うので区別しない。`PATCH /tasks/{id}` が `workspace`/`assignee` の変更で経路が通った
/// `blocked` タスクを `ready` に戻せるかの判定に使う（worker が聞いた質問による `blocked` とは分ける。
/// そちらは `Trigger::WorkerQuestion`、reason は `"worker_question"`）。
pub fn latest_block_is_unroutable(events: &[(u64, Event)]) -> bool {
    events
        .iter()
        .rev()
        .find_map(|(_, e)| match e {
            Event::Transitioned {
                to: Status::Blocked,
                reason,
                ..
            } => Some(reason == "unroutable"),
            _ => None,
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;
