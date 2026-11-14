//! ADR 2026-11-14 dispatch-admission-order・ADR 2026-11-11 dispatcher-state-ownership D3・D5: run を起こすかの判断（純粋関数）。
//!
//! [`admit`] は、dispatch_one が集めた fact（[`AdmissionInput`]）から、次に集める fact の要求・止め方・起動のどれか
//! （[`AdmissionProposal`]）を返す。admit が持つのは gate を見る順序と組み合わせだけで、個々の判定は既存の関数と owner の
//! 結果を dispatch_one が集めて入力に入れる。待ちの記録・遷移・担当の書き戻し・lease・event・spawn は dispatch_one が、
//! admit が求めたとき（または止め方・起動として返したとき）だけ今の順で行う。
//!
//! 守る順序（ADR 2026-11-14 D5。「1 本目だけ」は並列 WU の 2 本目以降で fact を求めずに通す gate）:
//!
//! - 知識整理 run の入力 → 同じ task の run（1 本目だけ）→ 並列度の枠（CoS の対話 run かは 1 本目だけ調べる）→
//!   打ち切ったばかりの run → この instance が触ってよい task か → disk の critical
//! - 担当（1 本目だけ）→ execution gate と `plan_invalid` の atomic（1 本目だけ）→ WU の gate（2 本目以降は受けた WU）
//! - 決定の待ち → 設計 review の待ち（どちらも 1 本目だけ。前が止めたら後ろを求めない）→ 木の上限 →
//!   WU の gate の種類（段の統合・最終 review・何もしない）
//! - planner run かの組み合わせと予算の上書き → 再試行の backoff（1 本目で attempts が 1 以上）→ infra の backoff（1 本目だけ。
//!   期限を過ぎていれば解除を求める）→ browser の前提（1 本目で前提が要る task だけ）
//! - 予算の上書きを task に当てる → write-set の gate → クラスタの解決（設定に無い・担当が道具を持たないなら止める）→
//!   remote ならクラスタの cooldown → 並列度 → この tick の接続の確認（切れていれば自動接続を 1 回試し、成功したら接続済みに
//!   する）→ 作業場所の dir → WU の worktree の用意の backoff（WU の run だけ）→ WU の worktree の用意（phase のある WU だけ）→
//!   disk の見込み（planner run でないときだけ）→ WU の worktree の確認と scope env（Local の WU の run だけ。用意できたら
//!   用意の失敗の記録を消す）
//!
//! 並列 WU の 2 本目以降が飛ばす gate: 同じ task の run・CoS の対話 run か・担当・execution gate・WU の gate の選択・
//! 決定と設計 review の待ち・再試行と infra の backoff・browser の前提。飛ばさない gate: 知識整理の入力・並列度の枠・
//! 打ち切ったばかりの run・eligible・disk の critical・木の上限・WU の gate の種類・予算の上書き・write-set から scope env
//! までの置き場所の gate（WU の用意に失敗したときの止め方だけが 2 本目以降かを持つ）。
//!
//! 既存の dispatcher の試験は 1 つの試験で 1 つの gate だけを止めるので、隣り合う 2 つの gate の順を入れ替えても合格する。
//! 入れ替えは `tests.rs` の、両方が止める入力で先の gate が勝つことを確かめる試験でだけ検出できる。置き場所の gate は
//! 止めたときの副作用（クラスタを使えないと記録する・WU を blocked にする・待ちを記録する）が gate ごとに違うので、先の gate が
//! 勝たないと、今は起きない副作用が起きる。
//!
//! この module は入力の値だけを読む。永続化・file・process に触れず、dispatcher の module を使わず、log も出さない（D2 の 2・3）。

use std::time::Instant;

use task_core::{
    BackoffCause, CapacityScope, ExecutionMode, GateMode, GateSource, Route, Task, TaskWait, Tier,
    TierSource, WorkUnitBudget,
};
use time::OffsetDateTime;

use crate::cluster_runtime::ClusterResolution;
use crate::disk_admission::Admission;
use crate::execution_coordination::WriteSetAdmission;

#[cfg(test)]
mod tests;

/// dispatch_one がそれまでに集めた fact。`None`（bool の欄は `false`）はまだ集めていない（admit が求めていない）。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AdmissionInput {
    /// 並列 WU の 2 本目以降で dispatch_one が受けた WU（`RunWorkUnit` を落とした値）。task は既に Running で、担当・gate・
    /// backoff は 1 本目で済んでいる。1 本目は `None`。
    pub(crate) forced_unit: Option<AdmissionUnit>,
    /// 知識整理 run の入力が揃っているか（`task_ops::knowledge_curation::inputs_ready`）。
    pub(crate) knowledge_inputs_ready: Option<bool>,
    /// この task の走っている run の数（`RunSupervisor::runs_for_task`）。
    pub(crate) task_runs: Option<usize>,
    /// CoS の対話 run か（`is_cos_task`）。
    pub(crate) cos_run: Option<bool>,
    /// 並列度の枠に空きがあるか（`RunLoad::admits`）。
    pub(crate) capacity_admits: Option<bool>,
    /// この tick で同じ task の run を打ち切ったか（`RunSupervisor::was_just_aborted`）。
    pub(crate) just_aborted: Option<bool>,
    /// この instance が触ってよい task か（`is_eligible`）。
    pub(crate) eligible: Option<bool>,
    /// disk の critical で止める run か（`held_by_disk_critical`）。
    pub(crate) disk_critical: Option<bool>,
    /// 監視している path の一覧（`disk_watch_paths`。`TaskWait::DiskCritical` の本文）。
    pub(crate) disk_watch_paths: Option<String>,
    /// 担当が決まったか（`assign_if_needed` が task を返したか。返さなければ blocked にして人に聞いた）。
    pub(crate) assigned: Option<bool>,
    /// execution gate と `plan_invalid` の atomic を当てたか（`execution_gate_if_needed`・`apply_plan_invalid_atomic`）。
    pub(crate) execution_gate_applied: bool,
    /// WU の gate（`wu_dispatch_gate` を落とした値）。
    pub(crate) unit: Option<AdmissionUnit>,
    /// `needed_before: [self]` の未回答の決定を待つか（`decision_self_hold`）。
    pub(crate) decision_self_hold: Option<bool>,
    /// 設計 review を待つか（`design_review_hold`）。
    pub(crate) design_review_hold: Option<bool>,
    /// 木の上限で止めるか（`tree_units::next_run` が run を返す gate だけ `tree_limit_hold_for`。返さなければ `false`）。
    pub(crate) tree_limit_hold: Option<bool>,
    /// planner run かの組み合わせ・予算の上書き・再試行に要る task と設定の値。
    pub(crate) task_values: Option<AdmissionTaskValues>,
    /// 再試行の backoff の期限（ready に入った時刻 + `retry_backoff`）と今の時刻。
    pub(crate) retry_backoff: Option<BackoffClock>,
    /// infra の backoff（`RunSupervisor::infra_backoff_until`。無ければ内側が `None`）と今の時刻。
    pub(crate) infra_backoff: Option<Option<BackoffClock>>,
    /// 期限を過ぎた infra の backoff を解いたか（`RunSupervisor::clear_infra_backoff`）。
    pub(crate) infra_backoff_cleared: bool,
    /// browser の前提が要る task か（`browser_prereq_applies`）。
    pub(crate) browser_prereq_needed: Option<bool>,
    /// browser の前提が無い・古いので止めたか（`browser_prereq_hold`。止めたら task を遷移させてある）。
    pub(crate) browser_prereq_hold: Option<bool>,
    /// 予算の上書きから scope env までの置き場所の fact。
    pub(crate) place: AdmissionPlace,
}

/// 置き場所の fact（予算の上書き・write-set・クラスタ・作業場所・WU の用意・disk）。`None`（bool の欄は `false`）は
/// まだ集めていない。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AdmissionPlace {
    /// run の tier と予算の上書きを task に当てたか。
    pub(crate) run_override_applied: bool,
    /// write-set の gate の結果（`admit_write_set`。`Hold` の待ちは gate が記録してある。`Admit` の予約は起動で使う）。
    pub(crate) write_set: Option<WriteSetAdmission>,
    /// `resolve_cluster` の結果を落とした値。
    pub(crate) cluster: Option<AdmissionCluster>,
    /// クラスタの cooldown の期限（`cooldown_until`）と、dispatch_one が受けた今の時刻。
    pub(crate) cluster_cooldown: Option<ClusterCooldown>,
    /// クラスタで走っている run の数（`cluster_in_use`）。
    pub(crate) cluster_in_use: Option<usize>,
    /// この tick の接続の確認の結果（`connected`。確かめていなければ内側が `None`）。
    pub(crate) cluster_connected: Option<Option<bool>>,
    /// 自動接続の結果（`try_auto_connect_cluster`。接続のフックが無ければ内側が `None`）。
    pub(crate) cluster_auto_connect: Option<Option<Result<(), String>>>,
    /// 自動接続できたクラスタを接続済みにしたか（`set_cluster_connected`）。
    pub(crate) cluster_marked_connected: bool,
    /// 作業場所の dir を解決できたか（`pin_task_root` の後に `task_workspaces_for`、無ければ `task_dir`）。
    pub(crate) task_dir_resolved: Option<bool>,
    /// WU の worktree の用意の backoff（`wu_prepare_backoff`。続いた失敗の回数と次に試してよい時刻。backoff の中で
    /// なければ内側が `None`）。
    pub(crate) wu_prepare_backoff: Option<Option<(u32, OffsetDateTime)>>,
    /// WU の worktree を用意できたか（`prepare_work_unit_workspace`）。
    pub(crate) wu_workspace_ready: Option<bool>,
    /// disk の見込みの判定（`judge_run`。判定しなければ内側が `None`）。
    pub(crate) disk: Option<Option<Admission>>,
    /// WU の worktree の確認と scope env（`wu_base::scope_env`）を用意できたか（用意できたら dispatch_one が
    /// `clear_wu_prepare_failures` で用意の失敗の記録を消してある）。
    pub(crate) wu_scope_env_ready: Option<bool>,
}

/// `resolve_cluster` の結果（クラスタの設定と作業場所の path は dispatch_one が持つ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdmissionCluster {
    /// Local の作業場所の task（`resolve_cluster` は Local の作業場所にだけこれを返す）。
    Local,
    /// remote の作業場所で、クラスタが設定にあり担当が使える（値は設定のクラスタ id・host・並列度）。
    Remote {
        id: String,
        host: String,
        concurrency: usize,
    },
    /// `[[clusters]]` にそのクラスタ id が無い。
    NotConfigured,
    /// 担当が `cluster:<id>` を持たない（ADR-0062 B1）。
    AssigneeLacksTool { cluster: String },
}

/// クラスタの cooldown の期限（無ければ `None`）と、比べる今の時刻。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClusterCooldown {
    pub(crate) until: Option<Instant>,
    pub(crate) now: Instant,
}

impl AdmissionCluster {
    /// `resolve_cluster` の結果から読む（値を写すだけで判定はしない。クラスタの設定は dispatch_one が持つ）。
    pub(crate) fn of(resolution: &ClusterResolution) -> Self {
        match resolution {
            ClusterResolution::Local => Self::Local,
            ClusterResolution::Resolved(spec, ..) => Self::Remote {
                id: spec.id.clone(),
                host: spec.host.clone(),
                concurrency: spec.concurrency,
            },
            ClusterResolution::NotConfigured => Self::NotConfigured,
            ClusterResolution::AssigneeLacksTool { cluster } => Self::AssigneeLacksTool {
                cluster: cluster.clone(),
            },
        }
    }
}

/// WU の gate（dispatcher の非公開の型）を admit の要る値に落としたもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionUnit {
    /// 計画を持たない task の run。
    Atomic,
    /// 計画の WorkUnit の run。`budget` はその WU の予算（`WorkUnitSpec.budget`）、`phased` は工程を持つ WU
    /// （自分の worktree と run の鍵を持つ v2 の WU）か。
    WorkUnit {
        budget: Option<WorkUnitBudget>,
        phased: bool,
    },
    /// planner run。`replan` は既に計画を持つ task の replan。
    Planner { replan: bool },
    /// 段の WU がすべて done で、統合を始める（run は起こさない）。
    StartIntegration,
    /// 仕事の残らない計画の最終 review を始める（run は起こさない）。
    FinalReview,
    /// この tick では何もしない。
    Skip,
}

/// planner run かの組み合わせ・予算の上書き・再試行に要る、担当と execution gate の後の task と設定の値。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdmissionTaskValues {
    /// `[execution] gate`。
    pub(crate) gate_mode: GateMode,
    /// task の `routing.execution` の mode と出所。
    pub(crate) execution: Option<(ExecutionMode, GateSource)>,
    /// `task_core::tree::is_tree_child` の結果。
    pub(crate) tree_child: bool,
    /// task の `routing.route` の経路と、shadow の記録か。
    pub(crate) route: Option<(Route, bool)>,
    /// task の `routing.tier_source`（routing が無ければ `None`）。
    pub(crate) tier_source: Option<TierSource>,
    /// task の `worker_hint.tier`。
    pub(crate) tier: Tier,
    /// task の予算の turn と壁時計（上書きの前）。
    pub(crate) budget: (u32, u64),
    /// `[execution.planner]` の tier。
    pub(crate) planner_tier: Tier,
    /// `[execution.planner]` の turn と壁時計。
    pub(crate) planner_budget: (u32, u64),
    /// task の attempts。
    pub(crate) attempts: u32,
}

impl AdmissionTaskValues {
    /// 担当と execution gate の後の task と、dispatch_one が `[execution]` から渡す gate・planner の tier と予算から読む
    /// （ADR 2026-11-14 D2。値を写すだけで判定はしない。木の子 task かは `task_core::tree::is_tree_child`）。
    pub(crate) fn read(
        task: &Task,
        gate_mode: GateMode,
        planner_tier: Tier,
        planner_budget: (u32, u64),
    ) -> Self {
        let routing = task.routing.as_ref();
        Self {
            gate_mode,
            execution: routing
                .and_then(|r| r.execution.as_ref())
                .map(|d| (d.mode, d.source)),
            tree_child: task_core::tree::is_tree_child(task),
            route: routing
                .and_then(|r| r.route.as_ref())
                .map(|r| (r.route, r.shadow)),
            tier_source: routing.map(|r| r.tier_source),
            tier: task.worker_hint.tier,
            budget: (task.budget.max_turns, task.budget.max_wall_secs),
            planner_tier,
            planner_budget,
            attempts: task.attempts,
        }
    }
}

/// backoff の期限と、比べる今の時刻（`now_utc`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BackoffClock {
    pub(crate) until: OffsetDateTime,
    pub(crate) now: OffsetDateTime,
}

/// admit の提案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdmissionProposal {
    /// 次に集める fact。dispatch_one は対応する既存の関数を呼び、結果を入力の欄に入れて admit をもう一度呼ぶ。
    Need(AdmissionFact),
    /// 起こさない。dispatch_one が止め方を適用して `Ok(false)` を返す。
    Stop(AdmissionStop),
    /// 起動へ進む。dispatch_one は lease 以降を今のとおり行う。
    Launch(AdmissionLaunch),
}

/// 次に集める fact。1 つの variant が既存の 1 つの判定か owner の 1 つの処理を指す（ADR 2026-11-14 D2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionFact {
    /// `task_ops::knowledge_curation::inputs_ready`。
    KnowledgeInputs,
    /// `RunSupervisor::runs_for_task`。
    TaskRuns,
    /// `is_cos_task`。
    CosRun,
    /// `RunLoad::admits`（`cos` は CoS の枠で数えるか）。
    Capacity { cos: bool },
    /// `RunSupervisor::was_just_aborted`。
    JustAborted,
    /// `is_eligible`。
    Eligible,
    /// `held_by_disk_critical`。
    DiskCritical,
    /// `disk_watch_paths`。
    DiskWatchPaths,
    /// `assign_if_needed`（担当を書き戻すか、候補が無ければ blocked にして人に聞く）。
    Assignee,
    /// `execution_gate_if_needed` の後に `apply_plan_invalid_atomic`（判定を task に書き戻す）。
    ExecutionGate,
    /// `wu_dispatch_gate`。
    Unit,
    /// `decision_self_hold`。
    DecisionSelfHold,
    /// `design_review_hold`（設計 review の run を起こすか判定を記録する）。
    DesignReviewHold,
    /// `tree_units::next_run` と `tree_limit_hold_for`。
    TreeLimit,
    /// task と設定の値（[`AdmissionTaskValues`]）。
    TaskValues,
    /// `retry_backoff` と `now_utc`。
    RetryBackoff,
    /// `RunSupervisor::infra_backoff_until` と `now_utc`。
    InfraBackoff,
    /// `RunSupervisor::clear_infra_backoff`（期限を過ぎた backoff を解く）。
    ClearInfraBackoff,
    /// `browser_prereq_applies`（`planner_run` は planner run か）。
    BrowserPrereq { planner_run: bool },
    /// `browser_prereq_hold`（前提が無ければ task を遷移させる）。
    BrowserPrereqHold,
    /// run の tier と予算の上書きを task に当てる（`None` は上書きしない。上書きの前の予算は dispatch_one が控える）。
    RunOverride {
        tier: Option<Tier>,
        budget: Option<(u32, u64)>,
    },
    /// `admit_write_set`（`planner_run` は planner run か。重なれば gate が待ちを記録する）。
    WriteSet { planner_run: bool },
    /// `resolve_cluster`。
    Cluster,
    /// remote のクラスタの fact。
    ClusterState(ClusterFact),
    /// `pin_task_root` の後に `task_workspaces_for`、無ければ `task_dir`。
    TaskDir,
    /// WU の run の作業場所の fact。
    WorkUnit(WorkUnitFact),
    /// `judge_run`（planner run でないときだけ）。
    Disk,
}

/// remote のクラスタの fact（`resolve_cluster` が remote のクラスタを返した後でだけ求める）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClusterFact {
    /// `cooldown_until` と今の時刻。
    Cooldown,
    /// `cluster_in_use`。
    Load,
    /// `connected`（この tick の `refresh_cluster_liveness` の結果）。
    Connected,
    /// `try_auto_connect_cluster`（試したら `note_key_auth_attempt` で記録する）。
    AutoConnect,
    /// `set_cluster_connected`（自動接続できたクラスタを接続済みにする）。
    MarkConnected,
}

/// WU の run の作業場所の fact（WU の run でだけ求める）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkUnitFact {
    /// `wu_prepare_backoff`。
    PrepareBackoff,
    /// `prepare_work_unit_workspace`（phase のある WU だけ）。
    Workspace,
    /// worktree の `ensure_blocking` と `wu_base::scope_env`（Local の作業場所だけ。用意できたら
    /// `clear_wu_prepare_failures`）。
    ScopeEnv,
}

/// 起こさないときの止め方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdmissionStop {
    /// 待ちを記録せずに見送る（遷移や待ちは、fact を集めた owner の処理が済ませている）。
    Quiet,
    /// 待ちを 1 件記録して見送る（`note_dispatch_wait`）。
    Wait(TaskWait),
    /// backoff の待ちを記録して見送る（期限の文字列化と debug log は dispatch_one）。
    Backoff {
        until: OffsetDateTime,
        cause: BackoffCause,
    },
    /// 段の統合を ready から始める（`start_integration_from_ready`）。
    StartIntegration,
    /// 最終 review を ready から始める（`start_final_review_from_ready`）。
    StartFinalReview,
    /// クラスタが設定に無い（`warn_unroutable_once` の warn・`mark_unroutable` の後に待ちを記録する）。
    ClusterNotConfigured { wait: TaskWait },
    /// 担当がクラスタの道具を持たない（`block_task_missing_cluster_tool`）。
    ClusterToolMissing { cluster: String },
    /// クラスタの待ち。`unavailable` があれば `mark_cluster_unavailable` にその理由を渡し、`waiting` なら
    /// `note_cluster_waiting`（ADR-0018 D2: 人がログインするまで進まないので待ち対象に数えない）、最後に待ちを記録する。
    Cluster {
        unavailable: Option<String>,
        waiting: bool,
        wait: TaskWait,
    },
    /// 作業場所の dir を解決できない（warn だけ）。
    NoTaskDir,
    /// WU の worktree の用意の backoff の中（debug log だけ）。
    WuPrepareBackoff {
        failures: u32,
        retry_at: OffsetDateTime,
    },
    /// WU の worktree か scope env を用意できない（`on_work_unit_prepare_failed`。`forced` は並列 WU の 2 本目以降か）。
    WuPrepareFailed { forced: bool },
}

/// 起動へ進むときの run の形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdmissionLaunch {
    /// CoS の対話 run として `max_cos_runs` で数えるか（ADR-0089。並列 WU の 2 本目以降は `false`）。
    pub(crate) cos_run: bool,
    /// 直行経路（`route = direct`・shadow でない）の task か（ADR-0124 D3・D4）。
    pub(crate) direct_route: bool,
    /// planner run か（ADR-0072 D14/D17・ADR-0074 F3・ADR-0079 D4 (1)・ADR-0124 D3 の組み合わせ）。
    pub(crate) planner_run: bool,
    /// 既に計画を持つ task の replan の planner run か。
    pub(crate) replan: bool,
    /// 人が task に `tier:frontier` を明示したか（planner run の lane の出所に使う。ADR-0074 D5.3）。
    pub(crate) planner_human_frontier: bool,
}

/// run を起こすかを決める（ADR 2026-11-14 D5 の順）。
pub(crate) fn admit(input: &AdmissionInput) -> AdmissionProposal {
    match decide(input) {
        Ok(launch) => AdmissionProposal::Launch(launch),
        Err(proposal) => proposal,
    }
}

/// 集めた fact を返す。まだ無ければ、それを求める提案を返す。
fn fact<T: Clone>(value: &Option<T>, request: AdmissionFact) -> Result<T, AdmissionProposal> {
    value.clone().ok_or(AdmissionProposal::Need(request))
}

/// 集めた fact を写さずに読む（`fact` と同じく、まだ無ければ求める）。
fn fact_ref<T>(value: &Option<T>, request: AdmissionFact) -> Result<&T, AdmissionProposal> {
    value.as_ref().ok_or(AdmissionProposal::Need(request))
}

fn stop(how: AdmissionStop) -> AdmissionProposal {
    AdmissionProposal::Stop(how)
}

fn decide(input: &AdmissionInput) -> Result<AdmissionLaunch, AdmissionProposal> {
    let cos_run = capacity_and_host(input)?;
    let unit = unit_gate(input)?;
    let (launch, run_override) = run_shape(input, unit, cos_run)?;
    backoff_and_browser(input, &launch)?;
    placement(input, unit, &launch, run_override)?;
    Ok(launch)
}

/// D5 の 1〜6: 知識整理の入力・同じ task の run・並列度の枠・打ち切ったばかりの run・eligible・disk の critical。
/// 通れば CoS の対話 run かを返す。
fn capacity_and_host(input: &AdmissionInput) -> Result<bool, AdmissionProposal> {
    let first = input.forced_unit.is_none();
    if !fact(
        &input.knowledge_inputs_ready,
        AdmissionFact::KnowledgeInputs,
    )? {
        return Err(stop(AdmissionStop::Quiet));
    }
    // ADR-0074 D1.3（Phase F2b）: 並列 WU の 2 本目以降は task が既に Running で、自分以外の run が走っていてよい。
    if first && fact(&input.task_runs, AdmissionFact::TaskRuns)? > 0 {
        return Err(stop(AdmissionStop::Quiet));
    }
    // ADR-0089（Phase R6-5）: CoS の対話 run は `max_concurrency` ではなく `max_cos_runs` で数える。
    let cos = first && fact(&input.cos_run, AdmissionFact::CosRun)?;
    if !fact(&input.capacity_admits, AdmissionFact::Capacity { cos })? {
        let scope = if cos {
            CapacityScope::Cos
        } else {
            CapacityScope::Global
        };
        return Err(stop(AdmissionStop::Wait(TaskWait::Capacity {
            scope,
            id: None,
        })));
    }
    // ADR-0044 D2: 打ち切ったばかりの run と同じ worktree に、すぐ次の run を入れない。
    if fact(&input.just_aborted, AdmissionFact::JustAborted)? {
        return Err(stop(AdmissionStop::Quiet));
    }
    // ADR-0041 D5: verify モードは煙試験だけを起こす。
    if !fact(&input.eligible, AdmissionFact::Eligible)? {
        return Err(stop(AdmissionStop::Quiet));
    }
    // ADR 2026-10-07-build-tmp-hygiene 付記 A4: critical のあいだは新しい coding run を起こさない（2 本目以降も同じ）。
    if fact(&input.disk_critical, AdmissionFact::DiskCritical)? {
        let path = fact(&input.disk_watch_paths, AdmissionFact::DiskWatchPaths)?;
        return Err(stop(AdmissionStop::Wait(TaskWait::DiskCritical { path })));
    }
    Ok(cos)
}

/// D5 の 7〜12: 担当・execution gate・WU の gate・決定と設計 review の待ち・木の上限・WU の gate の種類。
fn unit_gate(input: &AdmissionInput) -> Result<AdmissionUnit, AdmissionProposal> {
    let first = input.forced_unit.is_none();
    if first {
        // ADR-0046 D5（Phase 59）: 候補が無ければ blocked にして人に聞いた。
        if !fact(&input.assigned, AdmissionFact::Assignee)? {
            return Err(stop(AdmissionStop::Quiet));
        }
        // ADR-0072 D13（Phase E3）・ADR-0079 D9 / D7（Phase R3a）: 最初の dispatch で 1 回だけ当てる。
        if !input.execution_gate_applied {
            return Err(AdmissionProposal::Need(AdmissionFact::ExecutionGate));
        }
    }
    let unit = match input.forced_unit {
        Some(unit) => unit,
        None => fact(&input.unit, AdmissionFact::Unit)?,
    };
    // ADR-0079 D9（Phase R2b）/ D7（Phase R3a）・ADR 2026-11-07-health-review-integration D3.5: 未回答の決定か
    // 設計 review を待つ task は run を起こさない。決定の待ちが止めたら設計 review の待ちを求めない。
    if first
        && (fact(&input.decision_self_hold, AdmissionFact::DecisionSelfHold)?
            || fact(&input.design_review_hold, AdmissionFact::DesignReviewHold)?)
    {
        return Err(stop(AdmissionStop::Quiet));
    }
    if fact(&input.tree_limit_hold, AdmissionFact::TreeLimit)? {
        return Err(stop(AdmissionStop::Quiet));
    }
    match unit {
        AdmissionUnit::StartIntegration => Err(stop(AdmissionStop::StartIntegration)),
        // ADR-0074「F5-fix8 実装時の明確化」: 仕事の残っていない計画の最終レビュー（run は起こさない）。
        AdmissionUnit::FinalReview => Err(stop(AdmissionStop::StartFinalReview)),
        AdmissionUnit::Skip => Err(stop(AdmissionStop::Quiet)),
        AdmissionUnit::Atomic | AdmissionUnit::WorkUnit { .. } | AdmissionUnit::Planner { .. } => {
            Ok(unit)
        }
    }
}

/// D5 の 13: planner run かの組み合わせと、run の tier と予算の上書き（task に当てる要求）。
fn run_shape(
    input: &AdmissionInput,
    unit: AdmissionUnit,
    cos_run: bool,
) -> Result<(AdmissionLaunch, AdmissionFact), AdmissionProposal> {
    let values = fact(&input.task_values, AdmissionFact::TaskValues)?;
    // ADR-0072 D17（Phase E4）: replan の planner run。
    let replan = matches!(unit, AdmissionUnit::Planner { replan: true });
    let work_unit = match unit {
        AdmissionUnit::WorkUnit { budget, .. } => Some(budget),
        _ => None,
    };
    let decision_is_compound =
        values.execution.map(|(mode, _)| mode) == Some(ExecutionMode::Compound);
    // ADR-0074 F3: `gate = "shadow"` でも、人が `execution: compound` を明示した task は planner run に進む。
    let shadow_human_explicit_compound = values.gate_mode == GateMode::Shadow
        && decision_is_compound
        && values.execution.map(|(_, source)| source) == Some(GateSource::Human);
    // ADR-0079 D4 (1)（Phase R2a）: 木の子 task の compound は `[execution] gate` に関わらず採用する。
    let tree_child_compound = decision_is_compound && values.tree_child;
    // ADR-0124 D3: 直行経路（`route = direct`・shadow でない）は planner を挟まない。
    let direct_route = values.route == Some((Route::Direct, false));
    let planner_run = replan
        || (work_unit.is_none()
            && !direct_route
            && decision_is_compound
            && (values.gate_mode == GateMode::On
                || shadow_human_explicit_compound
                || tree_child_compound));
    // ADR-0074 D5.3（Phase F1）: 人が task に `tier:frontier` を明示していれば planner run もそれに従う。
    let planner_human_frontier =
        values.tier_source == Some(TierSource::Human) && values.tier == Tier::Frontier;
    let (tier, budget) = if planner_run {
        let tier = if planner_human_frontier {
            Tier::Frontier
        } else {
            values.planner_tier
        };
        (Some(tier), Some(values.planner_budget))
    } else if let Some(wu_budget) = work_unit {
        // ADR-0072 D18/E2 申し送り（Phase E3）: WU の予算。書かなければ `max(task の予算, 既定)`。
        let (max_turns, max_wall_secs) = values.budget;
        let turns = wu_budget.and_then(|b| b.max_turns);
        let wall = wu_budget.and_then(|b| b.max_wall_secs);
        (
            None,
            Some((
                turns.unwrap_or(max_turns.max(30)),
                wall.unwrap_or(max_wall_secs.max(1800)),
            )),
        )
    } else {
        (None, None)
    };
    let launch = AdmissionLaunch {
        cos_run,
        direct_route,
        planner_run,
        replan,
        planner_human_frontier,
    };
    Ok((launch, AdmissionFact::RunOverride { tier, budget }))
}

/// D5 の 14〜16: 再試行の backoff・infra の backoff・browser の前提（どれも 1 本目だけ）。
fn backoff_and_browser(
    input: &AdmissionInput,
    launch: &AdmissionLaunch,
) -> Result<(), AdmissionProposal> {
    if input.forced_unit.is_some() {
        return Ok(());
    }
    let values = fact(&input.task_values, AdmissionFact::TaskValues)?;
    // ADR-0010 D6（P-3）: ready に入った時刻からのバックオフ。
    if values.attempts > 0 {
        let clock = fact(&input.retry_backoff, AdmissionFact::RetryBackoff)?;
        if clock.now < clock.until {
            return Err(stop(AdmissionStop::Backoff {
                until: clock.until,
                cause: BackoffCause::Retry,
            }));
        }
    }
    // ADR-0070 D3（Phase 116）: インフラ都合の再試行のバックオフ（attempts に依らない別軸）。期限を過ぎたら解く。
    if let Some(clock) = fact(&input.infra_backoff, AdmissionFact::InfraBackoff)? {
        if clock.now < clock.until {
            return Err(stop(AdmissionStop::Backoff {
                until: clock.until,
                cause: BackoffCause::Infra,
            }));
        }
        if !input.infra_backoff_cleared {
            return Err(AdmissionProposal::Need(AdmissionFact::ClearInfraBackoff));
        }
    }
    // ADR 2026-10-08-browser-prod-enablement D2: 前提が無い・古いなら lease・worker の前に止める。
    let planner_run = launch.planner_run;
    if fact(
        &input.browser_prereq_needed,
        AdmissionFact::BrowserPrereq { planner_run },
    )? && fact(&input.browser_prereq_hold, AdmissionFact::BrowserPrereqHold)?
    {
        return Err(stop(AdmissionStop::Quiet));
    }
    Ok(())
}

/// D5 の 17〜: 予算の上書き・write-set・クラスタ・作業場所の dir・WU の用意の backoff と worktree・disk の見込み・scope env
/// （どれも並列 WU の 2 本目以降も見る）。
fn placement(
    input: &AdmissionInput,
    unit: AdmissionUnit,
    launch: &AdmissionLaunch,
    run_override: AdmissionFact,
) -> Result<(), AdmissionProposal> {
    let place = &input.place;
    // D18/D14: 上書きは write-set の gate の前に当てる（ADR-0072 D14）。
    if !place.run_override_applied {
        return Err(AdmissionProposal::Need(run_override));
    }
    // ADR-0130 D3: 同じ repo で走っている run と expected write-set が強く重なれば次 tick に回す（待ちは gate が記録する）。
    let planner_run = launch.planner_run;
    let write_set = fact_ref(&place.write_set, AdmissionFact::WriteSet { planner_run })?;
    if matches!(write_set, WriteSetAdmission::Hold { .. }) {
        return Err(stop(AdmissionStop::Quiet));
    }
    // ADR-0018・ADR-0062 B1（Phase 107）: 設定に無いクラスタは unroutable、担当が道具を持たなければ blocked にして人に聞く。
    let remote = match fact(&place.cluster, AdmissionFact::Cluster)? {
        AdmissionCluster::Local => false,
        AdmissionCluster::Remote {
            id,
            host,
            concurrency,
        } => {
            cluster_link(place, &id, &host, concurrency)?;
            true
        }
        AdmissionCluster::NotConfigured => {
            let detail = "no such cluster in the config".to_string();
            let wait = TaskWait::Unroutable { detail };
            return Err(stop(AdmissionStop::ClusterNotConfigured { wait }));
        }
        AdmissionCluster::AssigneeLacksTool { cluster } => {
            return Err(stop(AdmissionStop::ClusterToolMissing { cluster }));
        }
    };
    // ADR-0041 D1 / ADR-0043 D2: 作業場所の dir を解決できなければ起こさない。
    if !fact(&place.task_dir_resolved, AdmissionFact::TaskDir)? {
        return Err(stop(AdmissionStop::NoTaskDir));
    }
    let AdmissionUnit::WorkUnit { phased, .. } = unit else {
        return disk(place, planner_run);
    };
    let wu = AdmissionFact::WorkUnit;
    // ADR-0074「Phase F5-fix7 実装時の明確化」: 一時的な失敗の後の backoff の中は試さない。
    if let Some((failures, retry_at)) =
        fact(&place.wu_prepare_backoff, wu(WorkUnitFact::PrepareBackoff))?
    {
        return Err(stop(AdmissionStop::WuPrepareBackoff { failures, retry_at }));
    }
    let failed = AdmissionStop::WuPrepareFailed {
        forced: input.forced_unit.is_some(),
    };
    // ADR-0074 D1.2（Phase F2b）: phase のある WU は WU ごとの worktree で走る。
    if phased && !fact(&place.wu_workspace_ready, wu(WorkUnitFact::Workspace))? {
        return Err(stop(failed));
    }
    disk(place, planner_run)?;
    // worker が書く前に scope の基点を取る（remote の作業場所は取らない）。
    if !remote && !fact(&place.wu_scope_env_ready, wu(WorkUnitFact::ScopeEnv))? {
        return Err(stop(failed));
    }
    Ok(())
}

/// remote のクラスタの cooldown・並列度・接続（ADR-0018 D2・ADR-0032 D3）。止めるなら、待ち（`TaskWait::Cluster`）と、
/// クラスタを使えないと記録する理由を返す。
fn cluster_link(
    place: &AdmissionPlace,
    id: &str,
    host: &str,
    concurrency: usize,
) -> Result<(), AdmissionProposal> {
    let state = AdmissionFact::ClusterState;
    let hold = |unavailable: Option<String>, waiting: bool, detail: &str| {
        let wait = TaskWait::Cluster {
            cluster_id: id.to_string(),
            detail: detail.to_string(),
        };
        Err(stop(AdmissionStop::Cluster {
            unavailable,
            waiting,
            wait,
        }))
    };
    let cooldown = fact(&place.cluster_cooldown, state(ClusterFact::Cooldown))?;
    if cooldown.until.is_some_and(|until| cooldown.now < until) {
        return hold(None, true, "cooldown");
    }
    if fact(&place.cluster_in_use, state(ClusterFact::Load))? >= concurrency {
        return hold(None, false, "concurrency full");
    }
    if fact(&place.cluster_connected, state(ClusterFact::Connected))?.unwrap_or(false) {
        return Ok(());
    }
    // 自動接続は cooldown の中では試さない（失敗すれば cooldown が立つので、同じ tick の 2 本目は試さない）。
    match fact_ref(&place.cluster_auto_connect, state(ClusterFact::AutoConnect))? {
        Some(Ok(())) if !place.cluster_marked_connected => {
            Err(AdmissionProposal::Need(state(ClusterFact::MarkConnected)))
        }
        Some(Ok(())) => Ok(()),
        Some(Err(detail)) => {
            let detail = format!("auto-connect failed: {detail}");
            hold(Some(detail.clone()), true, &detail)
        }
        None => {
            let reason = format!("no ssh ControlMaster connection to {id} (host {host})");
            hold(Some(reason), true, "no ssh ControlMaster connection")
        }
    }
}

/// ADR 2026-11-11-disk-aware-dispatch D1: 予備を割るなら待たせる（M12。planner run は判定しない）。
fn disk(place: &AdmissionPlace, planner_run: bool) -> Result<(), AdmissionProposal> {
    if planner_run {
        return Ok(());
    }
    match fact_ref(&place.disk, AdmissionFact::Disk)? {
        Some(Admission::Wait(wait)) => Err(stop(AdmissionStop::Wait(wait.clone()))),
        Some(Admission::Go(_)) | None => Ok(()),
    }
}
