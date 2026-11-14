//! ADR 2026-11-11 dispatcher-state-ownership D3・D4: run の終わりの判断（純粋関数）。
//!
//! D4 の wu-failure の共有の関数 [`replan_or_ask`] を置く。自動 replan をするか人に聞くかの上限の判定は、
//! WU の失敗（worker 完了）・終わらない WU（遅延 WU の照合）・統合の失敗・仕事の残らない計画の gate・木の liveness の
//! 事実のすべてがこの 1 つで行う。replan の実行・event の追記・質問の文・決定の要求は各経路に残す。
//!
//! D4 の run-close の共有の関数 [`run_end_record`] を置く。run の終了記録（`WorkerFinished`）の組み立てだけを共有し、
//! lease の検証・二重の終了記録の確認・状態遷移・transaction の境界は閉じる経路ごとに残す。
//!
//! D3 の [`settle`] を置く。worker の run の終わり方（結果の分類・`Trigger`・outcome の文・`RunEnd`・infra 再試行・
//! requeue の上限・cluster job の wait・browser の前提の block・WU の check の不合格）を、worker 完了の経路が読んだ値
//! （[`RunFinishInput`]）から提案（[`RunFinishProposal`]）として返す。部をまたぐ質問の置き換え（[`questions_of_run`]）と
//! 木の token 予算切れ（[`stop_at_token_budget`]）は、WU と atomic task の分岐の後に決まる trigger を見るので別の関数にする。
//! 提案の適用（backoff・quota・cooldown・遷移と events の追記）は worker 完了の経路と owner の method が今と同じ順で行う。
//!
//! WU の run の判断（[`settle_work_unit`]。WU の遷移は `execution_scheduler` の遷移の関数を呼ぶ）は `work_unit` に置く。
//! WU の失敗の自動 replan か質問は [`settle_work_unit_failure`] が [`replan_or_ask`] で決める。
//!
//! atomic task の run の続け方（[`settle_task_run`]。continuation・子 task 待ち・進捗なしの連続・固い上限の質問）と、
//! v2 の WU の行を書いた後の段の進み方（[`phase_step_after_write`]。段の状態は `execution_scheduler` の段の関数）は
//! `task_run` に置く。
//!
//! この module は入力の値だけを読む。永続化・file・process に触れず、`dispatcher` module を使わない（D2 の 3）。

use std::time::Duration;

use task_core::browser_prerequisite::BrowserPrerequisiteCode;
use task_core::cluster_job::ClusterJobWait;
use task_core::{BudgetKind, Event, RunEnd, Trigger, Usage};
use task_worker::{AdapterError, Evidence, RunOutcome, Terminal};

use crate::policy::ProviderOutcome;

// WU の run の continuation の入力（[`WorkUnitRunInput`] の欄）。型の正本は `execution_scheduler` で、ここは同じ型を
// 再公開するだけで写さない。worker 完了の経路はこの名前で入力を作り、WU の run の入口を run_settlement だけにする。
pub(crate) use crate::execution_scheduler::ContinuationInputs;

mod task_run;
pub(crate) use task_run::{
    PhaseStep, TaskRunInput, hold_task_for_worker_decisions, phase_step_after_write,
    settle_task_run, task_checkpoint_end, task_reads_worker_decisions,
};
mod work_unit;
pub(crate) use work_unit::{
    WorkUnitFailureStep, WorkUnitRunInput, hold_for_worker_decisions, integration_repair_exhausted,
    settle_work_unit, settle_work_unit_failure, work_unit_run_end, work_unit_saves_checkpoint,
};

/// [`replan_or_ask`] の入力。各経路が読んだ値を詰める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReplanLimits {
    /// 同じ unit・同じ失敗の署名の自動 replan が進展なしに続いた回数（`task_ops::plan_gate::stalled_replans` の結果）。
    /// 失敗の署名を持たない経路（仕事の残らない計画の gate・木の liveness）は 0。
    pub stalled: u32,
    /// `[execution] max_replans` に数える replan の回数（`task_ops::plan_gate::counted_replans`。人の replan は数えない）。
    pub replans: u32,
    /// 節点の replan の上限（`[execution] max_replans` に、`limit:max_replans` の回答で上げた分を足した値）。
    pub max_replans: u32,
    /// replan を禁止する理由。無ければ `None`。
    pub ban: Option<ReplanBan>,
}

impl ReplanLimits {
    /// 経路が読んだ task の events と節点の上限から入力を作る。`failure` は (unit の key, 失敗の署名)。失敗の署名を
    /// 持たない経路（仕事の残らない計画の gate・木の liveness・planner の文脈）は `None` で、`stalled` は 0。
    pub(crate) fn from_events(
        events: &[(u64, task_core::Event)],
        failure: Option<(&str, &str)>,
        max_replans: u32,
        ban: Option<ReplanBan>,
    ) -> Self {
        Self {
            stalled: failure.map_or(0, |(key, signature)| {
                task_ops::plan_gate::stalled_replans(events, key, signature)
            }),
            replans: task_ops::plan_gate::counted_replans(events),
            max_replans,
            ban,
        }
    }
}

/// replan を禁止する理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplanBan {
    /// health の Error・同じ指摘の往復・health の上限の使い切りで統合が止まった
    /// （ADR 2026-11-08-health-enforcement-loop D3.4・D4.3・D4.4。`health_review::check_item::stops_without_replan`）。
    HealthStop,
}

/// [`replan_or_ask`] が返す判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplanOrAsk {
    /// 自動 replan をする。
    Replan,
    /// replan せず人に聞く（質問か、木の節点では `limit:max_replans` の決定の要求。どちらにするかは経路が決める）。
    Ask(AskReason),
}

/// 人に聞く理由。数（`stalled`・`replans`）は入力の [`ReplanLimits`] にある。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AskReason {
    /// 同じ失敗の自動 replan が進展なしに `task_ops::plan_gate::STALLED_REPLAN_LIMIT` 回以上続いた
    /// （ADR 2026-10-22-stalled-auto-replan D4。`max_replans` に余りがあっても止める）。
    Stalled,
    /// replan を禁止する理由がある（[`ReplanBan`]）。
    Banned,
    /// replan の上限を使い切った（ADR-0079 付記「R6-1」D3・D9）。
    Exhausted,
}

/// D4 wu-failure: 自動 replan をするか、人に聞くか。
///
/// 理由の優先は stalled → 禁止 → 上限の使い切り。stalled を先に見るのは、統合の失敗が health で禁止されていても
/// 進展の無い繰り返しなら stalled の質問文を出す今の挙動を保つため。
pub(crate) fn replan_or_ask(limits: ReplanLimits) -> ReplanOrAsk {
    if limits.stalled >= task_ops::plan_gate::STALLED_REPLAN_LIMIT {
        return ReplanOrAsk::Ask(AskReason::Stalled);
    }
    if limits.ban.is_some() {
        return ReplanOrAsk::Ask(AskReason::Banned);
    }
    if limits.replans < limits.max_replans {
        ReplanOrAsk::Replan
    } else {
        ReplanOrAsk::Ask(AskReason::Exhausted)
    }
}

/// D4 run-close: run の終了記録（`Event::WorkerFinished`）を組み立てる。
///
/// 入力は run id・outcome の文・usage・role（`None` はワーカー run）・metrics・終端。終端は呼ぶ経路が明示し、
/// この関数は補わない（`None` を渡せば `None` のまま）。返すのは event だけで、追記・lease の検証・二重の終了記録の
/// 確認・状態遷移・transaction の境界は呼ぶ経路が持つ（run を閉じる関数にしない）。
pub(crate) fn run_end_record(
    run_id: &str,
    outcome: String,
    usage: Option<task_core::Usage>,
    role: Option<task_core::RunRole>,
    metrics: Option<task_core::RunMetrics>,
    end: Option<task_core::RunEnd>,
) -> task_core::Event {
    task_core::Event::WorkerFinished {
        run_id: run_id.to_string(),
        outcome,
        usage,
        role,
        metrics,
        end,
    }
}

/// [`settle`] の入力。worker 完了の経路が永続化から読んだ値と、dispatcher の側の判定（供給側の失敗の分類・cluster job の
/// wait の検証・infra 再試行の待ち時間）の結果を詰める。
pub(crate) struct RunFinishInput<'a> {
    /// worker の結果（`Err` はアダプタか供給側の失敗）。
    pub result: Result<RunOutcome, AdapterError>,
    /// この task の events（連続 requeue 数と部をまたぐ質問を数える）。
    pub events: &'a [(u64, Event)],
    pub run_id: &'a str,
    /// 対話の task の run（`Question` を `Done` として扱う。ADR-0033 D4 / Phase 28）。
    pub conversation: bool,
    /// WU の run（browser の前提の block は atomic の run だけ）。
    pub work_unit: bool,
    /// `Err` の供給側の失敗の分類（分類できなければ `None`）。
    pub supply_failure: Option<ProviderOutcome>,
    /// `Err` の文が台帳由来の browser の前提の失敗なら、その code。
    pub browser_prereq: Option<BrowserPrerequisiteCode>,
    /// `Terminal::Waiting` の申告を検証した結果（ADR-0090 D1/D2）。他の終わり方では `None`。
    pub cluster_wait: Option<Result<ClusterJobWait, String>>,
    pub max_requeues: u32,
    /// 分類できない失敗が今回で何回目の infra 再試行か（連続 infra 再試行数 + 1）と、D4 retry-backoff の共有の関数
    /// `run_supervisor::infra_retry_after` が返した待ち時間（上限を超えたら `None`）。
    pub infra_retry: (u32, Option<Duration>),
    /// WU の checks の不合格で `Terminal::Done` を `Terminal::Error` にすり替えた run の、worker の usage と不合格の要約
    /// （ADR-0079 付記 R7-5）。
    pub check_failure: Option<(Option<Usage>, String)>,
}

/// [`settle`] の提案。worker 完了の経路はこれを WU と atomic task の分岐に渡し、同じ transaction の events と owner の
/// method への指示にする。
#[derive(Debug)]
pub(crate) struct RunFinishProposal {
    pub trigger: Trigger,
    /// `WorkerFinished.outcome` の文。
    pub outcome: String,
    /// この run の usage（check の不合格ですり替えた run は worker が返した usage）。
    pub usage: Option<Usage>,
    /// quota と cooldown の記録に渡す結果。
    pub provider_outcome: ProviderOutcome,
    /// 構造化した終わり方（ADR-0072 D7）。分類できない供給側・インフラの失敗は `None`。
    pub run_end: Option<RunEnd>,
    /// reviewer に渡す done の内容（要約と証拠）。done でない終わり方は空。
    pub done_summary: String,
    pub done_evidence: Vec<Evidence>,
    /// `Terminal::Yielded` と `Terminal::Waiting` の生の checkpoint（ADR-0072 D9）。
    pub yield_checkpoint: Option<serde_json::Value>,
    /// 開く cluster job の wait（ADR-0090 D1/D2）。
    pub cluster_wait: Option<ClusterJobWait>,
    /// browser の前提の block の event（ADR 2026-10-08-browser-prod-enablement D2）。
    pub browser_prereq: Option<Event>,
    /// `Trigger::InfraRequeue` のときの (n 回目, 待ち時間)。`RunSupervisor` の infra backoff に渡す（ADR-0070 D3）。
    pub infra_requeue: Option<(u32, Duration)>,
    /// worker が `result.json` を置き違えた run（WU の失敗から自動 replan を起こさない）。
    pub misplaced: bool,
    /// 部をまたぐ委譲の質問（1 件の部またぎにつき 1 件。同じ文面は 1 回だけ。ADR-0033 D4 / Phase 27）。
    pub cross_department: Vec<String>,
    /// outcome の文に足す check の不合格の要約（無ければ空。ADR-0079 付記 R7-5 D2）。
    pub check_failure_suffix: String,
}

impl RunFinishProposal {
    /// 終わり方だけを決めた提案。他の欄は空で、[`settle`] と分類の関数が埋める。
    fn ended(
        trigger: Trigger,
        outcome: String,
        usage: Option<Usage>,
        run_end: Option<RunEnd>,
    ) -> Self {
        Self {
            trigger,
            outcome,
            usage,
            provider_outcome: ProviderOutcome::Ok,
            run_end,
            done_summary: String::new(),
            done_evidence: Vec::new(),
            yield_checkpoint: None,
            cluster_wait: None,
            browser_prereq: None,
            infra_requeue: None,
            misplaced: false,
            cross_department: Vec::new(),
            check_failure_suffix: String::new(),
        }
    }
}

/// D3: worker の run の終わり方を決める。
pub(crate) fn settle(input: RunFinishInput<'_>) -> RunFinishProposal {
    let misplaced = matches!(&input.result, Ok(RunOutcome {
        terminal: Terminal::Error { message, retryable: false, .. }, ..
    }) if message.starts_with(task_worker::result_location::MISPLACED_RESULT_MARKER));
    let mut proposal = match input.result {
        Ok(outcome) => settle_terminal(outcome.terminal, input.conversation, input.cluster_wait),
        Err(e) => settle_adapter_failure(
            &e,
            input.supply_failure,
            task_ops::derive::consecutive_requeues(input.events) < input.max_requeues,
            input.max_requeues,
            (!input.work_unit).then_some(input.browser_prereq).flatten(),
            input.infra_retry,
        ),
    };
    proposal.misplaced = misplaced;
    proposal.cross_department = cross_department_questions(input.events, input.run_id);
    // ADR-0079 付記 R7-5 D4: checks の不合格ですり替えた run も、worker が返した usage を残す。
    if let Some((usage, summary)) = input.check_failure {
        proposal.usage = proposal.usage.or(usage);
        proposal.check_failure_suffix = format!(": {summary}");
    }
    proposal
}

/// worker が返した終わり方（`Ok(RunOutcome)`）の分類。
fn settle_terminal(
    terminal: Terminal,
    conversation: bool,
    checked_wait: Option<Result<ClusterJobWait, String>>,
) -> RunFinishProposal {
    match terminal {
        Terminal::Done {
            summary,
            usage,
            evidence,
        } => {
            let mut p = RunFinishProposal::ended(
                Trigger::WorkerDone,
                format!("done: {summary}"),
                usage,
                Some(RunEnd::Completed),
            );
            p.done_summary = summary;
            p.done_evidence = evidence;
            p
        }
        // ADR-0033 D4 / Phase 28: 対話 run は `Question` を出さない。人に聞きたいことは返事に書けばよいので、そのまま
        // `Done` 扱いにする（`approvals` の行は作らない）。
        Terminal::Question { text, usage } if conversation => {
            let mut p = RunFinishProposal::ended(
                Trigger::WorkerDone,
                format!("done: {text}"),
                usage,
                Some(RunEnd::Completed),
            );
            p.done_summary = text;
            p
        }
        Terminal::Question { text, usage } => RunFinishProposal::ended(
            Trigger::WorkerQuestion,
            format!("question: {text}"),
            usage,
            Some(RunEnd::Question),
        ),
        // ADR-0072 D7: 二重の安全網。構造化されていない `Error` でも、予算切れの語彙なら `BudgetExhausted` にする。
        // ADR 2026-10-20-mlsys-main2-fixes D5: 失敗までに観測できた usage を残す（無ければ欠測のまま）。
        Terminal::Error {
            message,
            retryable,
            usage,
        } => {
            let end = retryable
                .then(|| classify_budget_kind_from_text(&message))
                .flatten()
                .map_or(RunEnd::Failed { retryable }, |kind| {
                    RunEnd::BudgetExhausted { kind }
                });
            RunFinishProposal::ended(
                Trigger::WorkerError { retryable },
                format!("error(retryable={retryable}): {message}"),
                usage,
                Some(end),
            )
        }
        // ADR-0072 D9/D10: graceful yield。trigger と outcome の文は仮の値で、continuation の判定が確定させる。
        Terminal::Yielded { checkpoint, usage } => {
            let mut p = RunFinishProposal::ended(
                Trigger::WorkerError { retryable: true },
                "error(retryable=true): yielded".to_string(),
                usage,
                Some(RunEnd::Yielded),
            );
            p.yield_checkpoint = Some(checkpoint);
            p
        }
        // ADR-0090 D1: クラスタ job の終了待ち。検証に通れば run を `waiting` で閉じる。通らなければ retryable な失敗。
        Terminal::Waiting {
            checkpoint, usage, ..
        } => match checked_wait
            .unwrap_or_else(|| Err("the cluster job wait was not validated".to_string()))
        {
            Ok(wait) => {
                let line = format!(
                    "waiting: クラスタ {} の {} job の終了を待ちます: {}",
                    wait.cluster,
                    wait.scheduler.as_str(),
                    wait.jobs.join(" ")
                );
                let mut p = RunFinishProposal::ended(
                    Trigger::ClusterJobWait,
                    line,
                    usage,
                    Some(RunEnd::Waiting),
                );
                p.yield_checkpoint = checkpoint;
                p.cluster_wait = Some(wait);
                p
            }
            Err(reason) => RunFinishProposal::ended(
                Trigger::WorkerError { retryable: true },
                format!("error(retryable=true): invalid cluster job wait: {reason}"),
                usage,
                Some(RunEnd::Failed { retryable: true }),
            ),
        },
        // ADR-0072 D7: turn / wall-clock / context の上限に当たった。usage を運ぶ。
        Terminal::BudgetExhausted {
            kind,
            message,
            usage,
        } => RunFinishProposal::ended(
            Trigger::WorkerError { retryable: true },
            format!("error(retryable=true): budget exhausted ({kind:?}): {message}"),
            usage,
            Some(RunEnd::BudgetExhausted { kind }),
        ),
    }
}

/// アダプタか供給側の失敗（`Err(AdapterError)`）の分類。`below_requeue_limit` は連続 requeue 数が上限未満か、
/// `browser_prereq` は atomic の run の browser の前提の失敗の code（WU の run は `None`）。
fn settle_adapter_failure(
    e: &AdapterError,
    supply_failure: Option<ProviderOutcome>,
    below_requeue_limit: bool,
    max_requeues: u32,
    browser_prereq: Option<BrowserPrerequisiteCode>,
    (infra_n, infra_delay): (u32, Option<Duration>),
) -> RunFinishProposal {
    match (supply_failure, browser_prereq) {
        // ADR-0010 D5（P-21）: 供給側失敗は attempts を消費せず requeue し、プロバイダを cooldown にする。
        // ADR 2026-10-17: 認証失敗は上限に数えない（アカウントは routing から外れ、再ログインか別アカウントを待つ）。
        (Some(po), _) => {
            let requeue = matches!(po, ProviderOutcome::AuthFailed) || below_requeue_limit;
            let mut p = if requeue {
                RunFinishProposal::ended(
                    Trigger::Requeue,
                    format!("requeue: adapter: {e}"),
                    None,
                    None,
                )
            } else {
                // ADR-0011（P-38）: 同じ試行での連続 requeue が上限に達したら、通常の失敗として attempts を消費する。
                RunFinishProposal::ended(
                    Trigger::WorkerError { retryable: true },
                    format!(
                        "error(retryable=true): requeue limit ({max_requeues}) reached: adapter: {e}"
                    ),
                    None,
                    None,
                )
            };
            p.provider_outcome = po;
            p
        }
        // ADR 2026-10-08-browser-prod-enablement D2: 台帳由来の失敗を返した atomic の run は infra 再試行に回さず止める。
        (None, Some(code)) => {
            let mut p = RunFinishProposal::ended(
                Trigger::BrowserPrereqBlock,
                format!("browser_prerequisite: {}: adapter: {e}", code.as_str()),
                None,
                None,
            );
            p.browser_prereq = Some(Event::BrowserPrerequisiteBlocked {
                code,
                message: code.message().to_string(),
            });
            p
        }
        // ADR-0070 D3（Phase 116）: プロバイダが分類できない失敗は「インフラ都合」として attempts を消費せず、
        // `max_infra_retries` までバックオフして再試行する。上限を超えたら `WorkerError{retryable:false}` で打ち切り、
        // `"infra failure ×N"` を付ける（D1 の失敗分類がこの接頭辞を見る）。
        (None, None) => match infra_delay {
            Some(delay) => {
                let mut p = RunFinishProposal::ended(
                    Trigger::InfraRequeue,
                    format!("infra_requeue: adapter: {e}"),
                    None,
                    None,
                );
                p.infra_requeue = Some((infra_n, delay));
                p
            }
            None => RunFinishProposal::ended(
                Trigger::WorkerError { retryable: false },
                format!(
                    "{}{infra_n}: adapter: {e}",
                    task_ops::derive::INFRA_FAILURE_MARKER
                ),
                None,
                None,
            ),
        },
    }
}

/// ADR-0033 D4（Phase 24 / Phase 27）: この run の途中で「部をまたぐ委譲」を止めたときに残った質問
/// （`QuestionRaised`。1 件の部またぎにつき 1 件。同じ文面は 1 回だけ）。
fn cross_department_questions(events: &[(u64, Event)], run_id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, e) in events {
        if let Event::QuestionRaised { run_id: r, text } = e
            && r == run_id
            && !out.contains(text)
        {
            out.push(text.clone());
        }
    }
    out
}

/// ADR-0072 D7（Phase E1）: 「二重の安全網」。構造化した `Terminal::BudgetExhausted` を返さない古い経路・アダプタのために、
/// `Terminal::Error{retryable:true}` の文を字句判定する（`task_core::is_budget_outcome` と同じ語彙。分類できなければ `None`）。
fn classify_budget_kind_from_text(message: &str) -> Option<BudgetKind> {
    let m = message.to_lowercase();
    if task_core::looks_like_context_exceeded(&m) {
        Some(BudgetKind::Context)
    } else if m.contains("max_turns") || m.contains("max turns") || m.contains("turn limit") {
        Some(BudgetKind::Turns)
    } else if m.contains("wall-clock") || m.contains("wall clock") || m.contains("budget") {
        Some(BudgetKind::WallClock)
    } else {
        None
    }
}

/// [`questions_of_run`] の提案。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RunQuestions {
    /// 人に見せる質問（`approvals` の行の単位）。
    pub questions: Vec<String>,
    /// 部をまたぐ質問で run の終わり方を `Trigger::WorkerQuestion` に置き換えるときの outcome の文。置き換えたら
    /// reviewer に渡す done の内容は空にする。
    pub replaced_outcome: Option<String>,
}

/// ADR-0033 D4: 部をまたぐ委譲の質問は、run の自己申告の終わり方より優先する（子は作られていない）。
/// Phase 27: 人に見せる質問は 1 件の部またぎにつき 1 つ。`trigger` と `outcome` は WU と atomic task の分岐の後の値。
pub(crate) fn questions_of_run(
    trigger: &Trigger,
    outcome: &str,
    cross_department: &[String],
) -> RunQuestions {
    let asked = matches!(trigger, Trigger::WorkerQuestion);
    let mut questions: Vec<String> = Vec::new();
    if asked {
        questions.push(
            outcome
                .strip_prefix("question: ")
                .unwrap_or(outcome)
                .to_string(),
        );
    }
    let replaced_outcome = (!cross_department.is_empty() && !asked)
        .then(|| format!("question: {}", cross_department.join("\n")));
    questions.extend(cross_department.iter().cloned());
    RunQuestions {
        questions,
        replaced_outcome,
    }
}

/// ADR 2026-10-20-mlsys-main2-fixes D1-4: この trigger の後に（人の操作なしで）次の run か reviewer が起きるか。
/// `WorkerError{retryable:true}` は attempts が残れば retry になるので含める（残っていなければ failed で同じ終端）。
pub(crate) fn trigger_starts_another_run(trigger: &Trigger) -> bool {
    matches!(
        trigger,
        Trigger::WorkerDone
            | Trigger::Continue { .. }
            | Trigger::WorkerError { retryable: true }
            | Trigger::Requeue
            | Trigger::InfraRequeue
    )
}

/// [`stop_at_token_budget`] の入力。
#[derive(Debug, Clone, Copy)]
pub(crate) struct TreeTokens {
    /// task 木のこの run 以外の消費。
    pub prior: u64,
    /// この run の usage。
    pub run: Option<Usage>,
    /// 木の `max_tokens`。
    pub max: u64,
}

/// [`stop_at_token_budget`] の提案。適用する経路は trigger と outcome の文を置き換え、質問と infra の backoff を捨てる。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TokenBudgetStop {
    pub trigger: Trigger,
    pub outcome: String,
    pub used: u64,
    pub max: u64,
}

/// ADR 2026-10-20-mlsys-main2-fixes D1-4: 次の run（continuation・retry・requeue・次の unit・replan）や reviewer を起こす
/// 終わり方でも、task 木の消費（この run を含む）が `max_tokens` に達していたら起こさず、task を failed にする（人には聞かない）。
/// 兄弟の unit が走っている間（`stays_running`。task を遷移させない）は最後の 1 本で決めるので止めない。
/// 木の消費は止める候補のときだけ読めばよいので、`tokens` は呼ぶ側が [`needs_tree_tokens`] を見てから読む。
pub(crate) fn stop_at_token_budget(tokens: TreeTokens) -> Option<TokenBudgetStop> {
    let used = tokens.prior + tokens.run.map(|u| u.budget_tokens()).unwrap_or(0);
    (used >= tokens.max).then(|| TokenBudgetStop {
        trigger: Trigger::TokenBudgetExhausted,
        outcome: token_budget_outcome(used, tokens.max),
        used,
        max: tokens.max,
    })
}

/// 木の token 予算を見る終わり方か（task を遷移させ、次の run か reviewer を起こす）。
pub(crate) fn needs_tree_tokens(trigger: &Trigger, stays_running: bool) -> bool {
    !stays_running && trigger_starts_another_run(trigger)
}

/// `WorkerFinished.outcome` / 遷移の理由に書く文（`token_budget_exhausted: <used>/<max>`）。
pub(crate) fn token_budget_outcome(used: u64, max: u64) -> String {
    format!("token_budget_exhausted: {used}/{max}")
}

/// ADR-0072 D9（Phase E1）: `WorkerFinished.outcome` に載せる、人が読む 1 行の終わり方の説明。
pub(crate) fn describe_run_end(end: RunEnd) -> String {
    match end {
        RunEnd::Completed => "completed".to_string(),
        RunEnd::Yielded => "yielded".to_string(),
        RunEnd::BudgetExhausted { kind } => {
            let k = match kind {
                BudgetKind::Turns => "turns",
                BudgetKind::WallClock => "wall_clock",
                BudgetKind::Tokens => "tokens",
                BudgetKind::Context => "context",
            };
            format!("budget_exhausted({k})")
        }
        RunEnd::Question => "question".to_string(),
        RunEnd::Failed { retryable } => format!("failed(retryable={retryable})"),
        RunEnd::HarnessError { class } => format!("harness_error({class:?})"),
        RunEnd::Cancelled => "cancelled".to_string(),
        RunEnd::Waiting => "waiting(cluster_jobs)".to_string(),
    }
}

#[cfg(test)]
mod tests;
