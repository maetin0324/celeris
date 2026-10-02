//! 決定的ディスパッチャ（DESIGN §5.2, ADR-0005 D4–D6）。
//!
//! 1 tick の手順:
//! 1. 終了したワーカー／レビューの結果を取り込み、状態遷移をストアに書く
//! 2. 期限切れリースを回収（`running → ready|failed`、`LeaseExpired`）
//! 3. 自分が起動した run のうち、ストア上で既に `running` でない／run_id が変わったものを強制終了（cancel 等）
//! 4. `reviewing` なのに判定中でないタスクのレビューを開始（再起動後の復旧、または前 tick で `Reviewer` run の
//!    枠が無く見送ったもの）
//! 5. `ready_tasks` を `priority DESC, created_at ASC` で取り、`ProviderPolicy` と並列度上限に従って dispatch
//!
//! Phase 5（ADR-0007）: `Reviewer` 条件を持つタスクのレビューは、`Standard` tier のプロバイダをここで選び
//! （並列度の枠も実行中 run と共有する）、`review.rs` がアダプタ経由で別 run を起動する。`Plan` kind の
//! レビューが通れば `TaskStore::complete_plan` で子タスクを挿入する。
//!
//! **LLM 呼び出しはここに書かない。** 判断は全て設定・状態機械・ストアのクエリで決まる。
//!
//! ## module map（ADR-0082）
//!
//! この facade に残すもの: `Dispatcher` struct と private な補助型（`RunEntry`・`ReviewEntry`・
//! `Completion` など）、公開の設定型、`new` と setter/getter、`tick`（段階の順序。`disk_ready` は drain
//! より前）・`drain_completions`・`dispatch_ready`・`is_idle`、`mod` 宣言と明示した `pub use`。依存の向きは L0（この facade の型と free helper）
//! ← L1 ← L2 ← L3 ← L4 ← `tick`。横の呼び出しは `worker_finish` → `phase_integration` の 1 本だけ。
//!
//! | モジュール | 責務 | 層 |
//! |---|---|---|
//! | `cluster` | cluster / tunnel の接続・生存確認・probe | L1 |
//! | `housekeeping` | disk guard・scratch pool の GC・後片付け | L1 |
//! | `snapshot` | デーモン状態の snapshot の組み立てと公開 | L1 |
//! | `quota_book` | quota の見積りと release | L1 |
//! | `sinks` | run 途中のイベントの sink（`StoreSink`・`ReviewerSink`） | L1 |
//! | `provider_select` | provider / account の選択と cooldown | L1 |
//! | `workspaces` | 作業場所・worktree・container の準備 | L1 |
//! | `run_context` | run の文脈（extras・session・knowledge・skills） | L1 |
//! | `worker_task` | worker 本体（free fn の `run_worker`。`Dispatcher` に依存しない） | L1 |
//! | `work_units` | WorkUnit の gate・準備・並列・checks | L2 |
//! | `tree_units` | 木の子 task の gate・liveness・一括作成 | L2 |
//! | `child_tasks` | 委譲した子と承認の子 | L2 |
//! | `review_spawn` | review run の起動（per-task lock は verdict の保存まで） | L2 |
//! | `worker_finish` | worker run の終了処理 | L3 |
//! | `planner_flow` | planner の結果の採用と replan | L3 |
//! | `review_verdict` | review の判定の適用と repair | L3 |
//! | `phase_integration` | 工程の統合と途中報（phase report） | L3 |
//! | `cluster_job_wait` | クラスタ job の durable wait の poll と続きの run（ADR-0090） | L3 |
//! | `leases` | lease の回収・abort・orphan・drain | L3 |
//! | `dispatch_run` | run の起動（`dispatch_one`・`spawn_worker`）と担当・lane の決定 | L4 |

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use task_core::plan::{PlanLimits, PlanOutput, materialize_logging};
use task_core::report::{HEADLINE_MAX_CHARS, first_line, truncate_chars};
use task_core::{
    AccountAdapter, ArtifactRef, Check, DelegateTask, DelegationLimits, Event, GenreSpec,
    ListFilter, ListOrder, NodeSession, NotificationKind, OnChildFailure, OrgKind, ProjectId,
    ProjectStatus, RateLimitObservation, RoleSpec, RunRole, SessionKind, Status, StoreError, Task,
    TaskId, TaskKind, TaskStore, Tier, Trigger, WorkspaceMode, WorkspaceSpec, support_kind,
};
use task_ops::daemon::{
    AccountCooldownLive, AccountLive, AccountUsageLive, ClusterLive, CooldownView, DaemonSnapshot,
    InFlight, InFlightKind, ProviderCheckView, ProviderLive, TunnelForwardLive,
};
use task_ops::delegate::{pending_children, plan_delegation};
use task_ops::derive::{
    AnswerNote, INFRA_FAILURE_MARKER, REVIEWER_INFRA_FAILURE_PREFIX, REVIEWER_REQUEUED_PREFIX,
    ReviewNote, answers_from_events, approval_decision_note, artifacts_for_run,
    consecutive_continuations, consecutive_infra_requeues, consecutive_requeues,
    consecutive_reviewer_infra_failures, consecutive_reviewer_requeues, current_run_seq,
    human_approval_title, infra_backoff_delay, last_run_id, latest_checkpoint, no_progress_streak,
    prior_review_from_events, retry_backoff,
};
use task_worker::{
    ActiveProjectContext, AdapterError, Answer, ChildSummary, CommentContext,
    ConversationAddressee, ConversationTurn, EventSink, GenreContext, LocalWorkspace,
    MemoryContext, MemoryDir, MilestoneBrief, MilestoneReviewContext, MilestoneTaskResult,
    NodeContext, OrgNodeContext, PROTOCOL_VERSION, PriorReview, Reachability, RecentWork,
    RoleContext, RunContext, RunLimits, RunOutcome, RunRequest, SshSettings, SshWorkspace,
    SyncMode, Terminal, WorkerAdapter, WorkerMessage, Workspace, WorkspaceError,
    control_master_alive_blocking, remote_dir_is_resolved, remote_exec_instructions,
    resolve_remote_dir,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::accounts::{
    AccountBook, AccountCandidate, AccountCheckRecord, AccountCooldownReason, AccountDir,
    ExcludedReason, ObservationSource, cooldown_for_failure, evaluate, scan_accounts,
    select_account,
};
use crate::policy::{
    AdapterId, CooldownReason, ProviderId, ProviderOutcome, ProviderPolicy, Selection,
};

// ADR-0082: 責務別の子モジュール（層は L1 ← L2 ← L3 ← L4 ← tick）。
mod cluster_job_wait;
pub use cluster_job_wait::{ClusterJobPollRequest, ClusterJobPoller, ssh_cluster_job_poller};
mod child_tasks;
mod cluster;
mod dispatch_run;
/// ADR-0098（Phase R7-10）: worker の run が宣言した後続 task（`followups.json`）。
mod followups;
mod housekeeping;
mod leases;
mod phase_integration;
mod planner_flow;
mod provider_select;
mod quota_book;
mod review_spawn;
mod review_verdict;
mod run_context;
mod sinks;
mod snapshot;
mod tree_units;
use sinks::{ReviewerSink, StoreSink};
mod work_units;
#[cfg(test)]
use work_units::previous_check_failure_lines;
use work_units::{WorkUnitCheckFailure, WorkUnitCheckRun};
mod worker_finish;
mod worker_task;
mod workspaces;

pub use cluster::{
    ClusterCommandProbe, ClusterConnector, ClusterForwardSpec, ClusterLivenessProbe,
    ClusterMasterExit, ClusterMasterWatcher, ClusterSpec, DEFAULT_TUNNEL_PROBE_INTERVAL_SECS,
    TUNNEL_PROBE_MAX_INTERVAL_SECS, TunnelEvent, TunnelEventKind, TunnelForwardEnsurer,
    TunnelListenerProbe, TunnelProbe,
};
pub use provider_select::provider_failure_outcome;
pub use snapshot::SnapshotPublisher;

#[cfg(test)]
use cluster::{CLUSTER_LIVENESS_INTERVAL, next_key_auth_backoff, next_probe_interval_secs};
use cluster::{
    ClusterConnChange, ClusterConnState, ClusterDisconnectInfo, ClusterResolution,
    ForwardObservation, TargetProbeState, run_cluster_hooks_off_async,
};
use leases::wall_ms_since;
#[cfg(test)]
use planner_flow::{
    REJECTED_PLAN_FILE, planner_blocked_question, planner_rejections_since_last_plan,
    planner_retry_message,
};
use provider_select::{
    account_cooldown_reason_name, cooldown_reason_name, excluded_reason_name,
    provider_failure_reason,
};
use worker_finish::{
    build_continuation_context, finish_reviewer_run_index, set_worker_finished_end,
    worker_finished_usage,
};
#[cfg(test)]
use worker_task::push_remote_after_run;
use worker_task::{LeaseRenewal, run_worker};
use workspaces::{
    CONTAINER_PROBE_TIMEOUT, downgraded_remote_workspace, remote_mode_omitted,
    workspace_error_to_adapter, worktree_marker,
};

/// ADR-0079 付記「R6-1」D4: 終端の task の `running` のままの `runs` 行を照合する間隔（秒）。
pub const RUNS_RECONCILE_INTERVAL_SECS: i64 = 600;

/// これを超えた tick は段階ごとの所要時間を `warn` で出す（ADR-0015 D2）。
/// ADR-0079 D10（Phase R3b）: 木の生存確認の間隔（秒。tick ごとに全節点の events を読まない）。
const LIVENESS_CHECK_INTERVAL_SECS: i64 = 30;

/// ADR-0072 D14: planner の試行の上限（「1 回だけ再試行」。窓は直近の `ExecutionPlanned` から）。
const MAX_PLANNER_ATTEMPTS: usize = 2;

use task_ops::tree_plan::{TreePlanOutcome, is_tree_plan_limit_error, relaxed_tree_plan_limits};

/// ADR-0079 D7（Phase R3a）: worker が `result.json` の `decisions` で出した決定の要求を記録する材料
/// （`Dispatcher::worker_decisions`）。
struct WorkerDecisions {
    /// `DecisionRequested`（path 付き）と、止めた unit の `WorkUnitTransitioned{reason: decision}`、捨てた要素・
    /// 束ねた旨の進行の 1 行。
    events: Vec<Event>,
    /// 決定が指した他の unit（`pending` / `ready`）の `blocked(decision)` の行。
    held_rows: Vec<task_core::WorkUnitRow>,
    /// `needed_before: self` の決定がある（leaf なら done にせず止める、atomic なら節点を止める）。
    self_hold: bool,
    /// 記録する決定の数（束ねた後）。
    count: usize,
}

const SLOW_TICK: Duration = Duration::from_secs(1);

/// tick の中の 1 段階がこれを超えたら `warn`（ADR-0015 D2。遅いのが DB かファイルかを切り分ける）。
const SLOW_STEP: Duration = Duration::from_millis(500);

/// ADR-0070 D5（Phase 116）: `StoreSink::heartbeat` が `renew_lease` の DB busy/locked をリトライする回数。
const RENEW_LEASE_RETRIES: u32 = 3;
/// ADR-0070 D5: 上のリトライの間隔。
const RENEW_LEASE_RETRY_DELAY: Duration = Duration::from_millis(100);

fn log_slow_step(step: &'static str, started: Instant) {
    let elapsed = started.elapsed();
    if elapsed >= SLOW_STEP {
        tracing::warn!(
            step,
            duration_ms = elapsed.as_millis() as u64,
            "slow dispatcher step"
        );
    }
}

/// 未作成の作業場所は最も近い既存の親 filesystem を測る。statvfs は symlink も解決する。
fn free_disk_mb(path: &Path) -> Result<u64, String> {
    let existing = path
        .ancestors()
        .find(|p| p.exists())
        .ok_or_else(|| format!("{}: no existing ancestor", path.display()))?;
    let stat =
        nix::sys::statvfs::statvfs(existing).map_err(|e| format!("{}: {e}", existing.display()))?;
    Ok(stat.blocks_available().saturating_mul(stat.fragment_size()) / (1024 * 1024))
}

/// ADR-0075（Phase G1）: dispatcher が持つ scratch pool の GC の状態（プロセス内メモリだけ。DB に書かない）。
#[derive(Default)]
struct ScratchState {
    /// 測定スレッドの結果（パス → サイズ）。
    sizes: crate::scratch_gc::SizeCache,
    /// 削除スレッド・測定スレッドが動いている間は `true`（重ねて起こさない）。
    removing: Arc<std::sync::atomic::AtomicBool>,
    measuring: Arc<std::sync::atomic::AtomicBool>,
    last_measure: Option<Instant>,
    /// 直近の走査で P3 だった target（run の開始時の adopt の候補）。
    candidates: Vec<task_worker::scratch::AdoptCandidate>,
    /// `DaemonSnapshot.scratch`。
    view: Option<task_ops::daemon::ScratchStatus>,
    last_gc: Option<task_ops::daemon::ScratchGcView>,
    /// journal の遷移（watermark の到達・解除）を 1 回だけ出すための直前の値。
    pressure: Option<task_worker::scratch::Pressure>,
    /// 実効上限の縮小を 1 回だけ出すための直前の値（GiB）。
    effective_warned_gib: Option<u64>,
    /// ディスク不足の通知の本文に足す P0 の一覧（直近の走査）。
    pinned_summary: Option<String>,
    /// この tick で緊急 GC を回した（通常の `scratch_gc` phase を重ねない）。
    ran_this_tick: bool,
}

/// ADR-0075 D3: run の `CARGO_TARGET_DIR` をどこから取るか（`run_worker` に渡す）。
#[derive(Debug, Clone)]
enum CargoTargetPlan {
    /// `[workspace] shared_build_cache = false`（与えない）。
    None,
    /// ADR-0066 D1 / F5-fix（`[scratch]` が無効）: `<build_cache_dir>/cargo/<repo-key>[/wu-<id>]`。
    Legacy(PathBuf),
    /// scratch pool（`<scratch>/targets/<owner>/target`。adopt の候補つき）。
    Scratch {
        settings: Box<task_worker::scratch::ScratchSettings>,
        candidates: Vec<task_worker::scratch::AdoptCandidate>,
    },
}

/// ADR-0075 D3: run の開始時の割り当て（lease の作成と adopt）。commit の距離もここで計算する（tick では計算しない）。
fn allocate_scratch_target(
    settings: &task_worker::scratch::ScratchSettings,
    candidates: &[task_worker::scratch::AdoptCandidate],
    owner: &task_worker::scratch::Owner,
    repo: &task_worker::TaskRepo,
    work_unit_key: Option<String>,
) -> PathBuf {
    let pool = settings.pool();
    let base = repo.worktree.as_ref().map(|w| w.base.sha.clone());
    let checkout = task_worker::scratch::checkout_time(&repo.dir);
    let source = repo.source.clone();
    let distance = |c: &task_worker::scratch::AdoptCandidate| match (&base, &c.base_commit) {
        (Some(a), Some(b)) => crate::scratch_gc::commit_distance(&source, a, b),
        _ => None,
    };
    let req = task_worker::scratch::AllocateRequest {
        owner,
        repo_path: &repo.source,
        base_commit: base.clone(),
        work_unit_key,
        checkout,
        candidates,
        distance: &distance,
        adopt: settings.adopt,
        max_distance: settings.adopt_max_distance,
    };
    match task_worker::scratch::allocate(&pool, &req) {
        Ok(a) => {
            if let Some(from) = &a.adopted_from {
                tracing::info!(owner = %owner, adopted_from = %from, target = %a.target_dir.display(), "scratch: adopted a warm target (ADR-0075 D3)");
            }
            a.target_dir
        }
        Err(e) => {
            // lease を作れなくてもパスは与える（worktree の直下に target を作らせない）。
            tracing::warn!(owner = %owner, error = %e, "scratch: could not write the lease; using the target path without it");
            pool.target_dir(owner)
        }
    }
}

/// ADR-0041 D5: この celeris が**面倒を見てよいタスク**の述語。`None`（既定）は「全部」＝従来どおり。
///
/// 検証（`--mode verify`）の celeris は、ここに「`genre = "smoke"` で、かつアダプタが `fake`」を渡す。
/// dispatch だけでなく、**ストア上の他のタスクの状態を変えうる経路すべて**（期限切れリースの回収、
/// `reviewing` の拾い上げ）で同じ述語を使う。手元で起こした run の後始末はこの述語に関係なく続ける
/// （自分が起こしたものは必ず自分が畳む）。
pub type TaskFilter = Arc<dyn Fn(&Task) -> bool + Send + Sync>;

/// RFC 3339 の文字列（デーモンのスナップショット用）。書式化に失敗することは実質無いが、その場合は空文字列。
/// ADR-0074 §4（Phase F2b）: 工程ごとの merge の repair の上限。
const MAX_MERGE_REPAIRS_PER_PHASE: usize = 2;

/// ADR-0074 D1.5（Phase F2）: v2 の Task の lease の保持者（run ではなく工程）の接頭辞。
const PHASE_LEASE_PREFIX: &str = "phase:";

/// Phase F5-fix2: 完了の確定に失敗した WU の run を戻すときの `WorkUnitTransitioned.reason`。
const FINALISE_FAILED_REASON: &str = "finalise_failed";

/// ADR-0074 D1.5: Task の lease の保持者が工程（`phase:<plan_id>:<phase>:<ulid>`）か。
fn is_phase_lease_holder(holder: &str) -> bool {
    holder.starts_with(PHASE_LEASE_PREFIX)
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

/// ADR-0074 D2.3（Phase F3 途中確認）: `WorkUnitRow.created_at`/`updated_at`（RFC 3339 の文字列）を
/// 壁時計の計算のために読む。読めなければ `None`（決定的な組み立てを諦め、その分は 0m 扱いになる）。
fn parse_rfc3339(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

use crate::review::{
    HumanVerdicts, PLAN_FILE_NAME, PlanCheck, ReviewExtras, ReviewOutcome, ReviewSubject,
    ReviewerRun, Verdict, needs_reviewer_run, review_task,
};

/// `task_ops::derive::ReviewNote` をワーカープロトコルの `task_worker::PriorReview` に写す
/// （ADR-0013 D7: task-ops は task_worker に依存しないため、この写像は dispatcher 側で行う）。
fn to_prior_review(notes: Vec<ReviewNote>) -> Vec<PriorReview> {
    notes
        .into_iter()
        .map(|n| PriorReview {
            criterion: n.criterion,
            pass: n.pass,
            reason: n.reason,
        })
        .collect()
}

/// `task_ops::derive::AnswerNote` をワーカープロトコルの `task_worker::Answer` に写す。
fn to_answers(notes: Vec<AnswerNote>) -> Vec<Answer> {
    notes
        .into_iter()
        .map(|n| Answer {
            question: n.question,
            answer: n.answer,
        })
        .collect()
}

/// ADR-0024/0025: `[accounts]` があるときのプール実行時設定（`celeris::config::AccountsConfig` の写し）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountsRuntimeConfig {
    /// ADR-0025 D1: アダプタごとの根ディレクトリ。`<root>/<id>/` が 1 アカウント。どちらか一方だけでもよい。
    pub roots: HashMap<AccountAdapter, PathBuf>,
    pub max_runs_per_account: usize,
    /// D6 の確認に使うモデル（celeris 側が使う。ディスパッチャ自身は確認を行わない。claude-code のみ）。
    pub check_model: String,
    /// 供給側失敗でアカウントを cooldown にするときのフォールバック秒数（= `error_cooldown_secs`）。
    pub fallback_cooldown_secs: u64,
}

impl AccountsRuntimeConfig {
    pub fn root_for(&self, adapter: AccountAdapter) -> Option<&PathBuf> {
        self.roots.get(&adapter)
    }
}

/// ディスパッチャの設定（`config.toml` から組み立てる。ADR-0005 D7）。
#[derive(Debug, Clone)]
pub struct DispatchConfig {
    pub delivery: task_ops::delivery::DeliveryPolicy,
    /// 全体の並列度上限。
    pub max_concurrency: usize,
    /// ADR-0002 D7: リース ttl = `max_wall_secs` + この猶予。
    pub lease_grace: Duration,
    /// ADR-0003 D4。
    pub idle_timeout: Duration,
    /// ADR-0003 D4。
    pub kill_grace: Duration,
    /// `Command` チェック 1 件あたりの上限。
    pub review_timeout: Duration,
    /// `WorkspaceSpec::Local` の相対パスの基準。
    pub workspace_root: PathBuf,
    /// DESIGN §4.2 `plan.auto_accept`: Plan の子を `draft` のまま置く（false）か、親 `done` と同一トランザクションで
    /// `ready` にする（true）か（ADR-0002 D6, ADR-0007 D3）。
    pub plan_auto_accept: bool,
    /// ADR-0010 D6（P-3）: attempts > 0 の ready タスクは `updated_at + min(base·2^(attempts-1), max)` まで dispatch しない。
    /// `base = 0` で無効。
    pub retry_backoff_base: Duration,
    pub retry_backoff_max: Duration,
    /// ADR-0010 D9（P-30）: `Reviewer` run の `pick` と合成 `Review` タスクの `worker_hint`。
    /// ADR-0069 Phase 118 D4: `tier` は他に何も分からないときの既定値（後方互換）で、実際の lane は
    /// `pick_reviewer` が `reviewer_tier_override` / 部署の `profile.review_tier` / worker lane から
    /// 動的に決める。
    pub reviewer_hint: task_core::WorkerHint,
    /// ADR-0069 Phase 118 D4: `[reviewer] tier` が明示されているときだけ `Some`（設定の優先順位で
    /// worker lane 一致の既定より強いが、部署の `profile.review_tier` には負ける）。
    pub reviewer_tier_override: Option<task_core::Tier>,
    /// ADR-0018: `WorkspaceSpec::Remote{cluster}` が指すクラスタ。キーは `cluster` の名前。
    pub clusters: HashMap<String, ClusterSpec>,
    /// ADR-0018 D2: 多重接続が無いクラスタを、この時間だけ dispatch の対象から外す。
    pub cluster_cooldown: Duration,
    /// ADR-0011（P-38）: 同じ試行での連続 requeue の上限。達したら供給側失敗を通常の失敗（attempts 消費）として扱う。
    pub max_requeues: u32,
    /// ADR-0054 D2（Phase 113）: `[review] max_reviewer_retries`。Reviewer run **自身のインフラ都合の
    /// 失敗**（`is_error` の結果・プロセス失敗・resume 拒否など。プロバイダが分類できた供給側失敗の
    /// `max_requeues` とは別軸）で reviewing を延期できる連続回数の上限。達したら「判定できなかった」
    /// を「reviewer infra failure ×N」として不合格にする（`fail_all` はしない。人の承認・command・
    /// artifact_exists の結果は保持する）。既定 3。
    pub max_reviewer_retries: u32,
    /// ADR-0070 D3（Phase 116）: `[dispatch] max_infra_retries`。ワーカー run **自身のインフラ都合の
    /// 失敗**（lease 失効・切替による中断・result.json 不在・セッション再開拒否・レート制限・DB busy。
    /// `provider_failure_outcome` が分類できない `Err`）で `attempts` を消費せず再試行できる連続回数の
    /// 上限（`consecutive_infra_requeues`）。達したら `WorkerError{retryable:false}` で
    /// `"infra failure ×N: …"` として打ち切る（`Trigger::LeaseExpired` は使わなくなった。
    /// `reclaim_expired_leases` もこの上限を通す）。既定 5。
    pub max_infra_retries: u32,
    /// 新規 run の開始を許す各 filesystem の最小空き容量 (MiB)。0 で無効。
    pub min_free_disk_mb: u64,
    /// ADR-0016 D1: `[[roles]]`。run 開始時に `RunContext.role`（指示文）を載せ、委譲された子の既定に使う。
    pub roles: Vec<RoleSpec>,
    /// ADR-0027 D1: `[[genres]]`。委譲の分野解決（`default_role` の既定の穴埋め）と、委譲できる run に渡す
    /// `RunContext.available_genres` に使う。
    pub genres: Vec<GenreSpec>,
    /// ADR-0016 D2: 委譲の上限（1 run の件数・木の深さ・木の run 数）。
    pub delegation: DelegationLimits,
    /// ADR-0024: `[accounts]` が設定されていればプール選択を有効にする。
    pub accounts: Option<AccountsRuntimeConfig>,
    /// ADR-0033 D6（Phase 24）: `[memory] dir`（絶対パス）。`None` なら記憶を読まないし書かない。
    pub memory_dir: Option<PathBuf>,
    /// ADR-0041 D1 / ADR-0042 D3: ローカルの worktree のブランチ接頭辞
    /// （`[workspace] worktree_branch_prefix`、既定 `celeris/`）。
    pub worktree_branch_prefix: String,
    /// ADR-0041 D1: `[selfdeploy] releases_dir`。その**親**の `current/manifest.json` が読めれば、
    /// 本番の sha を worktree の base の候補にする。`None` なら base は常に `main`（か `HEAD`）。
    pub releases_dir: Option<PathBuf>,
    /// ADR-0043 D3（Phase 56）: `[containers]`。コンテナ実行の runtime・既定のイメージ・ビルドの置き場。
    pub containers: ContainersRuntimeConfig,
    // ---- ADR-0047（Phase 61）: 知識ベース。ここから ----
    /// ADR-0047 D1 / D2: `[knowledge]`。正本の置き場と既定のマウント。
    pub knowledge: KnowledgeRuntimeConfig,
    // ---- ADR-0047（Phase 61）: ここまで ----
    /// ADR-0054 D1（Phase 67）: `[sessions] rollover_tokens`。CoS の対話・部門長のレビュー run の
    /// 継続セッションで、`approx_tokens`（run の usage の累計）がこれを超えたら次の run から
    /// 新しいセッションにする（要約を前置きに）。既定 400,000（`celeris::config` 側の既定値と同じ）。
    pub session_rollover_tokens: u64,
    /// ADR-0066 D1（Phase 110b）: `[workspace] shared_build_cache`（既定 true）。ローカルの git
    /// worktree のホスト実行に `CARGO_TARGET_DIR` を与えるかどうか。
    pub shared_build_cache: bool,
    /// ADR-0066 D1: `[workspace] build_cache_dir`（既定 `~/.local/celeris/build-cache`）。
    pub build_cache_dir: PathBuf,
    /// ADR-0075（Phase G1）: `[scratch]`。有効なら `CARGO_TARGET_DIR` は `<scratch>/targets/<owner>/target`
    /// （`build_cache_dir` を使わない）。無効（設定・NFS 上）なら ADR-0066 D1 / F5-fix の挙動。
    pub scratch: task_worker::scratch::ScratchSettings,
    /// ADR-0066 D2（Phase 110b）: `[workspace] prune_after_secs`（既定 86400、`0` で無効）。終端に
    /// なってからこの秒数経った作業場所から、ビルド生成物だけを刈る。
    pub workspace_prune_after_secs: u64,
    /// ADR-0072 D18（Phase E1）: `[execution]`。continuation の可否と上限。
    pub execution: ExecutionConfig,
}

/// ADR-0072 D18（Phase E1）: `[execution]`。continuation（予算切れ・yield の続き）の可否と上限。
/// E2 以降の欄（`max_work_units` 等）は ExecutionPlan/WorkUnit と一緒に導入する（今回は範囲外）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionConfig {
    /// `[execution] continuation`（既定 `true`）。`false` なら E1 の continuation を無効にし、
    /// 予算切れ・yield を従来どおり `WorkerError{retryable:true}` として扱う（ADR-0072 §6 (f)）。
    pub continuation: bool,
    /// `[execution] max_continuations_per_work_unit`（既定 3）。暗黙の WorkUnit では 1 タスクの
    /// continuation の合計回数（1 つの WU の Run は最大 `continuation + 1` 回）。
    pub max_continuations_per_work_unit: u32,
    /// `[execution] no_progress_limit`（既定 2）。進捗なしの continuation が連続この回数で
    /// `blocked` にする。
    pub no_progress_limit: u32,
    /// ADR-0072 D13（Phase E3）: `[execution] gate`。既定 `shadow`。
    pub gate: task_core::GateMode,
    /// ADR-0072 D14（Phase E3）: `[execution.planner]`。
    pub planner: task_core::PlannerConfig,
    /// ADR-0072 D16/D18（Phase E4）: Task ごとの repair の上限（既定 3）。
    pub max_repairs: u32,
    /// ADR-0072 D16/D18（Phase E4）: 同じ `RepairClass::bucket()` の repair の上限（既定 2）。
    pub max_repairs_per_class: u32,
    /// ADR-0072 D17/D18（Phase E4）: Task ごとの replan（計画の版の更新）の上限（既定 3）。
    pub max_replans: u32,
    /// ADR-0074 D5.2（Phase F1）: `[execution] work_unit_lane_cap`。既定 `task`。
    pub work_unit_lane_cap: task_core::WorkUnitLaneCap,
    /// ADR-0074 D1.1/§4（Phase F2b）: `[execution] parallel`。`true` で planner に v2
    /// （`celeris.execution-plan/2`）を出させる。既定 `false`（v1 のまま）。
    pub parallel: bool,
    /// ADR-0074 D1.3/§4（Phase F2b）: `[execution] max_parallel_work_units`。Task ごとの同時 WU 数の
    /// 上限（既定 3、上限 6）。
    pub max_parallel_work_units: usize,
    /// Phase F5-fix3: planner の計画の検証・採用に使う上限（ADR-0072 D18 / ADR-0074 §4）。planner の
    /// プロンプトにもこの値をそのまま出す（検証と文面の出どころを 1 つにする）。config.toml の欄は無く、
    /// 常に `ExecutionLimits::default()`（テストが既定と違う値を挿す）。
    pub limits: task_core::ExecutionLimits,
    /// ADR-0089（Phase R6-5）: `[execution] max_cos_runs`（既定 2）。`max_concurrency` とプールの
    /// `concurrency` から外す CoS の対話 run の同時数の絶対上限。`0` で例外を無効にする。
    pub max_cos_runs: usize,
}

/// ADR-0074 §4: `max_parallel_work_units` の上限。
pub const MAX_PARALLEL_WORK_UNITS_CAP: usize = 6;

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            continuation: true,
            max_continuations_per_work_unit: 3,
            no_progress_limit: 2,
            gate: task_core::GateMode::default(),
            planner: task_core::PlannerConfig::default(),
            max_repairs: 3,
            max_repairs_per_class: 2,
            max_replans: 3,
            work_unit_lane_cap: task_core::WorkUnitLaneCap::default(),
            parallel: false,
            max_parallel_work_units: 3,
            limits: task_core::ExecutionLimits::default(),
            max_cos_runs: crate::capacity::DEFAULT_MAX_COS_RUNS,
        }
    }
}

/// `[knowledge]`（ADR-0047 D1 / D2。Phase 61）。
#[derive(Debug, Clone, Default)]
pub struct KnowledgeRuntimeConfig {
    /// 正本の置き場（絶対パス。既定 `~/.local/share/celeris/knowledge`）。**celeris は作らない**。
    pub root: PathBuf,
    /// 実効 profile（ADR-0046 D1）が何も言わないときに全ノードが継ぐマウント。
    pub default_mounts: Vec<task_core::KnowledgeMount>,
    /// ADR-0052 D1（Phase 64）: `[knowledge.langmem].base_url`。知識整理タスクを dispatch する直前に
    /// `GET <base_url>/models` を当てる。`None` なら検査しない（＝従来どおり `langmem` で走らせる）。
    /// ADR-0132 D4: 通常は celeris の llm-proxy を指す。検査するのは proxy の到達性で、proxy の先の
    /// Qwen の生死ではない（Qwen が落ちても proxy が Claude / GPT の cheap に倒すので `langmem` のまま）。
    pub langmem_base_url: Option<String>,
    /// Phase 65b: `[knowledge.langmem].api_key_secret` から解決した平文のトークン（`[secrets] dir`
    /// が無い・見つからない等なら `None`）。到達性の probe が `Authorization: Bearer` に使う
    /// （`llm-proxy` のように `/v1/models` が認証を要求する上流を指したときのため）。**値はログに出さない**。
    pub langmem_api_key: Option<String>,
    /// ADR-0052 D2（Phase 64）: `knowledge` ハーネスの `fallback`（倒す先の tier）。`None` は
    /// 「倒さない」（`fallback = false` か、そもそも `knowledge` ハーネスが無い）。
    pub fallback_tier: Option<Tier>,
}

/// `[containers]`（ADR-0043 D3 / ADR-0042 D3）。
#[derive(Debug, Clone)]
pub struct ContainersRuntimeConfig {
    /// `runtime = "auto" | "podman" | "docker"`（既定 `auto` = podman を先に試す）。
    pub preference: task_worker::RuntimePreference,
    /// `[container] image` も `dockerfile` も無いときのイメージ（既定 `celeris-worker:latest`）。
    pub image_default: String,
    /// Dockerfile からビルドしたイメージの作業場所（既定 `~/.local/celeris/containers`）。
    pub build_dir: PathBuf,
    /// 1 回のビルドの上限（既定 1800 秒）。
    pub build_timeout: Duration,
}

impl Default for ContainersRuntimeConfig {
    fn default() -> Self {
        Self {
            preference: task_worker::RuntimePreference::Auto,
            image_default: task_worker::container::DEFAULT_IMAGE.to_string(),
            build_dir: PathBuf::from("."),
            build_timeout: Duration::from_secs(task_worker::container::DEFAULT_BUILD_TIMEOUT_SECS),
        }
    }
}

/// ADR-0043 D3（Phase 56）: 1 タスク分の実行環境の判断（ディスパッチャが dispatch のときに決める）。
#[derive(Debug, Clone)]
pub enum ContainerDecision {
    /// 従来どおりホストで走らせる。
    Host,
    /// コンテナが要るのに runtime が使えない → run を始めず `blocked` にして人に聞く。
    Unavailable { question: String },
    /// コンテナで走らせる（イメージの用意は `run_worker` が run の直前にやる）。
    Container(Box<ContainerRun>),
}

/// コンテナで走らせるときの一式（ADR-0043 D3）。
#[derive(Debug, Clone)]
pub struct ContainerRun {
    /// コンテナの形。`image` はイメージを決めた後に埋める。
    pub plan: task_worker::ContainerPlan,
    pub image: task_worker::ImageSource,
    pub image_default: String,
    pub build_root: PathBuf,
    pub build_timeout: Duration,
    /// コンテナを要求したリポジトリの名前（人に見せる文面に出す）。
    pub repo: String,
}

/// 1 tick の要約（ログとテスト用）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickReport {
    pub reclaimed: usize,
    pub dispatched: usize,
    pub finished: usize,
    pub reviewed: usize,
    pub in_flight: usize,
    /// 実行中／判定中が無く、`ready_tasks` も空で、DB に `running`/`reviewing` が無い。
    pub idle: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error(transparent)]
    Store(#[from] StoreError),
}

enum Completion {
    Worker {
        task_id: TaskId,
        run_id: String,
        provider: ProviderId,
        result: Result<RunOutcome, AdapterError>,
    },
    Review {
        task_id: TaskId,
        run_id: String,
        outcome: ReviewOutcome,
    },
    /// ADR-0072 D14/D6・E4 (g): WU の決定的な `checks`（`Command`）の実行が終わった。`on_worker_finished`
    /// が「run は `Terminal::Done` で終わったが、この WU にはまだ確かめていない `checks` がある」と
    /// 判定したときだけ起きる（`checks` が無ければ従来どおり `Completion::Worker` の経路をそのまま通る）。
    WorkUnitChecks {
        task_id: TaskId,
        run_id: String,
        account: Option<String>,
        account_adapter: Option<AccountAdapter>,
        run_since: Option<OffsetDateTime>,
        provider: ProviderId,
        result: Box<Result<RunOutcome, AdapterError>>,
        /// `(pass, reason)` の 1 件ずつ（`review::run_work_unit_checks` の結果そのまま）。
        check_results: Vec<(bool, String)>,
        /// ADR-0079 付記 R7-5 D1: 走らせた checks（`check_results` と同じ順）と、走らせた所。
        checks: Vec<task_core::WorkUnitCheck>,
        check_cwd: PathBuf,
    },
    /// ADR-0074 D1.4（Phase F2b）: 工程の統合（葉の merge と検査の再実行）が終わった。
    Integration {
        task_id: TaskId,
        work_unit_id: String,
        result: Box<Result<IntegrationRun, String>>,
    },
}

/// ADR-0074 D1.5（Phase F2）: `running` の鍵。`work_unit` は v2 の並列 WU の run だけ `Some(work_units.id)`
/// （atomic・planner・v1 の WU の run は `None`。v1 の挙動は 1 バイトも変えない）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RunKey {
    task: TaskId,
    work_unit: Option<String>,
}

struct RunEntry {
    run_id: String,
    provider: ProviderId,
    handle: JoinHandle<()>,
    /// dispatch した時刻（デーモンのスナップショット用。ADR-0013 D4）。
    since: OffsetDateTime,
    /// ADR-0018: コマンドを実行するクラスタ（ローカル実行なら `None`）。並列度の会計に使う。
    cluster: Option<String>,
    /// ADR-0024 D2/D3: プールから選んだアカウント（プールを使わないプロバイダなら `None`）。
    account: Option<String>,
    /// ADR-0025 D1: `account` が属するアダプタ（`account` が `None` なら `None`）。
    account_adapter: Option<AccountAdapter>,
    /// Phase 55/56 の合流（ADR-0044 P55-4 / ADR-0043 P56-7）: この run をコンテナで走らせているなら、
    /// ラベルでコンテナを止める口。`killpg` はコンテナの中の PID 名前空間には届かないので、
    /// `stop_run` がこれを `task_worker::kill_tree_with` に渡す。ホスト実行なら `None`。
    container: Option<Arc<dyn task_worker::ContainerStopper>>,
    /// ADR-0089（Phase R6-5）: CoS の対話 run か（`max_concurrency`・プールの `concurrency` に数えない）。
    cos: bool,
}

/// Phase 33: `recent_work_of` が `list_page` から読む候補の上限（裏方タスクを除いた後に
/// `RECENT_WORK_LIMIT` 件へ絞るための余裕）。
const RECENT_WORK_SCAN: usize = 100;
/// Phase 33（ADR-0033 D4 追記）: `context.recent_work` に渡す件数の上限。
const RECENT_WORK_LIMIT: usize = 10;

/// ADR-0048 D3（Phase 60b）: CoS の対話 run に渡す進行中の案件の件数の上限。
const ACTIVE_PROJECTS_SCAN: usize = 200;

/// Phase 41（ADR-0038 D1）: レビューの前置きに載せる、その途中目標の仕事の件数の上限。
const MILESTONE_REVIEW_TASK_LIMIT: usize = 20;
/// Phase 41: 途中目標の仕事を探すときに `list_page` から読む候補の上限。
const MILESTONE_REVIEW_TASK_SCAN: usize = 500;
/// Phase 41（ADR-0038 D1）: 1 件の仕事から載せる成果物の抜粋の字数（決定的に切る）。
const MILESTONE_REVIEW_EXCERPT_CHARS: usize = 4_000;
/// Phase 41（ADR-0038 D1）: 抜粋する成果物の名前（この順に見る）。
const MILESTONE_REVIEW_ARTIFACTS: [&str; 2] = ["answer.md", "report.md"];

/// Phase 33: その run が残した成果物の名前（`ArtifactProduced` から。重複は除く、順は登場順）。
fn artifact_names_of(events: &[(u64, Event)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, e) in events {
        if let Event::ArtifactProduced { artifact, .. } = e
            && !out.contains(&artifact.name)
        {
            out.push(artifact.name.clone());
        }
    }
    out
}

/// Phase 33（ADR-0033 D4 追記）: 終端タスクの短い要約（対話 run が「あなたの直近の仕事」に出す 1 行）。
/// `done` なら直近の `WorkerFinished` の `summary` の 1 行目、`failed` なら直近のレビュー不合格の理由か
/// 直近のワーカーのエラー（どちらが後かはイベント順で決まる）、それも無ければ `Failed` への遷移理由、
/// `blocked` なら直近の質問。Phase 25 の報告の文面の組み立て（`task_core::report`）をそのまま流用する
/// （`first_line` / `truncate_chars`）。LLM は使わない（DESIGN 原則 1）。
fn recent_work_outcome(task: &Task, events: &[(u64, Event)]) -> Option<String> {
    match task.status {
        Status::Done => events.iter().rev().find_map(|(_, e)| match e {
            Event::WorkerFinished {
                outcome,
                role: None,
                ..
            } => outcome
                .strip_prefix("done: ")
                .map(|s| truncate_chars(first_line(s), HEADLINE_MAX_CHARS)),
            _ => None,
        }),
        Status::Failed => events
            .iter()
            .rev()
            .find_map(|(_, e)| match e {
                Event::ReviewVerdict {
                    pass: false,
                    reason,
                    ..
                } => Some(truncate_chars(reason, HEADLINE_MAX_CHARS)),
                Event::WorkerFinished {
                    outcome,
                    role: None,
                    ..
                } => outcome
                    .strip_prefix("error(retryable=")
                    .and_then(|rest| rest.split_once("): "))
                    .map(|(_, message)| truncate_chars(message, HEADLINE_MAX_CHARS)),
                _ => None,
            })
            .or_else(|| {
                events.iter().rev().find_map(|(_, e)| match e {
                    Event::Transitioned {
                        to: Status::Failed,
                        reason,
                        ..
                    } => Some(reason.clone()),
                    _ => None,
                })
            }),
        Status::Blocked => events.iter().rev().find_map(|(_, e)| match e {
            Event::QuestionRaised { text, .. } => Some(truncate_chars(text, HEADLINE_MAX_CHARS)),
            Event::WorkerFinished {
                outcome,
                role: None,
                ..
            } => outcome
                .strip_prefix("question: ")
                .map(|s| truncate_chars(s, HEADLINE_MAX_CHARS)),
            _ => None,
        }),
        _ => None,
    }
}

/// ADR-0074 D1.4（Phase F2b）: 走っている工程の統合。
struct IntegrationEntry {
    work_unit_id: String,
    handle: JoinHandle<()>,
}

/// ADR-0072 D14/D6・E4 (g) / Phase F5-fix2: WU の `checks` を走らせている run（`spawn_work_unit_checks`
/// が spawn した検査。run 自身は `running` から既に外れている）。`in_flight` に数え（draining の
/// インスタンスが検査の途中で exit して完了を失わないため）、lease の照合では「生きている run」と
/// みなす。
struct CheckingEntry {
    task_id: TaskId,
    handle: JoinHandle<()>,
}

/// ADR-0074 D1.4: 工程の統合（spawn した git 操作と検査）の結果。
#[derive(Debug, Clone, Default)]
struct IntegrationRun {
    merged: Vec<crate::integration::Merged>,
    head: String,
    conflict: Option<crate::integration::Conflict>,
    /// `(cmd, pass, summary)`。
    checks: Vec<(String, bool, String)>,
}

/// ADR-0074 D1.2（Phase F2b）: v2 の Task を並列でどう走らせるか。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParallelMode {
    /// 工程の中で同時に走らせてよい WU の数。
    limit: usize,
    /// WU ごとの worktree を切るか（並列 1 に倒したときは Task の worktree を共有する）。
    worktrees: bool,
    /// 並列 1 に倒した理由（D1.2。`WorkUnitsSerialized` に残す）。
    fallback: Option<String>,
}

/// ADR-0074 D1.2: WU の run のために用意した作業場所。
struct WorkUnitWorkspace {
    workspaces: task_worker::TaskWorkspaces,
    branch: String,
    base: String,
    artifacts_dir: PathBuf,
}

/// ADR-0074「Phase F5-fix7 実装時の明確化」: WU の worktree を用意できなかった理由。
/// `permanent` は時間では直らないもの（依存先の成果が解決できない・Task ブランチが無い）で、1 回目で
/// WU を blocked にする。それ以外（git の錠・EBUSY・DB の一時的な失敗など）は
/// [`MAX_WU_PREPARE_ATTEMPTS`] 回までバックオフしてやり直し、使い切ったら blocked にする。
#[derive(Debug, Clone, PartialEq, Eq)]
struct WuPrepareError {
    message: String,
    permanent: bool,
}

impl WuPrepareError {
    fn permanent(message: String) -> Self {
        WuPrepareError {
            message,
            permanent: true,
        }
    }

    fn transient(message: String) -> Self {
        WuPrepareError {
            message,
            permanent: false,
        }
    }
}

impl std::fmt::Display for WuPrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// ADR-0074「Phase F5-fix7 実装時の明確化」: 一時的な失敗で WU の worktree の用意をやり直す上限
/// （連続。この回数目の失敗で blocked にする）。
const MAX_WU_PREPARE_ATTEMPTS: u32 = 5;

/// 一時的な失敗の `n` 回目（1 始まり）の後に待つ時間（2 s, 4 s, 8 s, 16 s …、上限 60 s）。
fn wu_prepare_backoff(n: u32) -> time::Duration {
    time::Duration::seconds(2i64.saturating_pow(n.min(6)).min(60))
}

/// 同じ WU の worktree の用意が連続して失敗した回数と、次に試してよい時刻（プロセス内メモリのみ）。
#[derive(Debug, Clone, Copy)]
struct WuPrepareFailures {
    count: u32,
    retry_at: OffsetDateTime,
}

/// ADR-0016 M5: 子待ちの親について覚えておくもの。
struct AwaitingChildren {
    run_id: String,
    plan: Option<PlanOutput>,
}

/// ADR-0072 D15（Phase E2）: `dispatch_ready` が計画のある Task について何をすべきか
/// （`wu_dispatch_gate`）。
enum WuDispatchGate {
    /// 計画を持たない Task（暗黙の WorkUnit）。従来どおり。
    Atomic,
    /// この WorkUnit の run を起こす。
    RunWorkUnit(Box<task_core::WorkUnitRow>),
    /// ADR-0072 D17（Phase E4）: 最初の計画作成（E3。`current_wu` が無い gate/replan 前の分岐）とは
    /// 別に、replan の planner run を起こす（`replan` は常に `true`。既存の `is_planner_dispatch` と
    /// 同じ扱いで dispatch する）。
    RunPlanner { replan: bool },
    /// ADR-0074 D1.4/D1.7（Phase F2b）: 工程の WU がすべて done で、統合がまだ（再起動の照合で
    /// pending に戻った等）。Task の lease（工程の保持者）を取り、統合を走らせる。
    StartIntegration(Box<task_core::WorkUnitRow>),
    /// ADR-0074「F5-fix8 実装時の明確化」: 有効な計画に仕事が残っていない（WU がすべて done、または WU が
    /// 1 つも無い）のに、その版がまだ最終レビューを受けていない（replan で何も足さなかった版など）。run を
    /// 起こさずに `Trigger::PlanComplete`（`ready → reviewing`）で最終レビューに出す。
    FinalReview,
    /// この tick では何もしない（`Stuck`（replan の余地なし）／replan の上限に到達）。
    Skip,
}

/// run 開始時に決める、ワーカーに渡す追加の文脈（ADR-0016 D1 / D3, ADR-0027 D1）。
#[derive(Debug, Default)]
struct RunExtras {
    role: Option<RoleContext>,
    children: Vec<ChildSummary>,
    /// ADR-0027 D1: 委譲できる run（`build_execute_prompt` を使う run）にだけ非空。
    available_genres: Vec<GenreContext>,
    /// ADR-0033 D4: `task.assignee` の組織ノード（担当が無いタスクでは `None`）。
    node: Option<NodeContext>,
    /// ADR-0033 D6: `[memory]` を設定し、担当が決まっている run にだけ載る長期記憶。
    memory: Option<MemoryContext>,
    /// ADR-0033 D4: 担当のノードとのこの案件での直近のやり取り（古い順）。
    conversation: Vec<ConversationTurn>,
    /// ADR-0033 D5（Phase 26）: 担当宛て + 全員向けの永続の認可（`standing_rules`。無ければ空）。
    standing_rules: Vec<String>,
    /// ADR-0033 D4: 分解・委譲できる run に渡す組織図。
    organization: Vec<OrgNodeContext>,
    /// ADR-0033 D4（Phase 28）: 対話用タスクの run だけ `Some`（相手が秘書かそれ以外か）。
    conversation_addressee: Option<ConversationAddressee>,
    /// Phase 30（ADR-0033 D4 追記）: 対話 run で、担当のノードが**自分の仕事の分野**（`node.genre`）を
    /// 持つときだけ `Some`。対話そのものは常に対話用分野で走る（`task.genre`）が、その人が自分の得意分野を
    /// 知って答えられるように、前置きに「仕事で使う道具」として渡す（実機の事故の再発防止:
    /// 検索ハーネスの genre を持つノードに話しかけても、その分野の run にはしない）。
    work_genre: Option<GenreContext>,
    /// Phase 33（ADR-0033 D4 追記。実機の事故の再発防止）: 対話 run にだけ、担当の直近の仕事
    /// （最大 10 件、更新の新しい順。案件を選んでいる対話ならその案件のものを先に）。
    recent_work: Vec<RecentWork>,
    /// Phase 41（ADR-0038 D1）: **途中目標レビューの対話 run** にだけ、その途中目標とそこまでの成果。
    milestone_review: Option<MilestoneReviewContext>,
    /// Phase 43（ADR-0039 D3）: 案件が作業場所を決めている run にだけ、その場所を説明する 1 行。
    workspace_note: Option<String>,
    /// ADR-0044 D2（Phase 53）: そのタスクのコメント（最新 20 件、古い順）。
    comments: Vec<CommentContext>,
    /// ADR-0044 D2: 直前の run を止めた人のコメント（あれば前置きの先頭に「人からの割り込み」として出る）。
    interrupt: Option<String>,
    /// ADR-0046 D1（Phase 59）: 担当ノードの実効 profile ＋ タスクの上書き。profile を 1 つも書いて
    /// いない組織では `None`（前置きは Phase 58 までとバイト単位で同じ）。
    profile: Option<task_core::EffectiveProfile>,
    /// ADR-0046 D4（Phase 59）: 既定（`production`）以外の進め方のときだけ `Some`。
    mode: Option<task_core::TaskMode>,
    /// ADR-0047 D2（Phase 61）: マウントされた知識の索引（本文は入れない）。
    knowledge: Option<task_worker::protocol::KnowledgeContext>,
    /// ADR-0048 D3（Phase 60b）: **CoS の対話 run** にだけ渡す、進行中の案件と途中目標。
    active_projects: Vec<ActiveProjectContext>,
    /// ADR-0059 D6（Phase 99）: **CoS の対話 run** にだけ渡す `[[clusters]]` の一覧
    /// （id・接続状態・実効 work_dir）。
    clusters: Vec<task_worker::ClusterContext>,
    /// ADR-0052 D2（Phase 64）: `langmem` の接続先に届かず、tier `cheap` の汎用ハーネスへ倒した run。
    /// 前置き（`role`）と予算をこの値で上書きする。通常の run では `None`。
    knowledge_fallback: Option<KnowledgeFallbackRun>,
    /// ADR-0054 D1（Phase 67）: 継続セッションの手がかり（CoS の対話・部門長のレビュー run だけ `Some`）。
    session: Option<task_worker::protocol::SessionHandle>,
    /// ADR-0054 D1（Phase 67）: `session.resume = true` のときだけ、前回の run 以降の差分（箇条書き）。
    session_diff: Vec<String>,
    /// ADR-0056 D3（Phase 79）: 担当ノードの実効 profile が継いだ skill mount のうち、KB に実在した
    /// もの（`RunContext.skills` にそのまま乗る）。
    skills: Vec<task_worker::protocol::SkillMount>,
    /// ADR-0056 D3（Phase 79）: mount 名にあったが KB に無かった skill（`run_ready` 相当の呼び出し元が
    /// `status` の進行イベントを 1 行出す。`RunContext` には乗らない）。
    missing_skills: Vec<String>,
    /// ADR-0072 D9/D21（Phase E2）: 計画のある Task の WorkUnit の run にだけ `Some`
    /// （`RunContext.work_unit` にそのまま乗る）。
    work_unit: Option<task_worker::protocol::WorkUnitPromptContext>,
    /// ADR-0072 D9（Phase E2）: この run が WU の continuation なら、events からではなく
    /// `runs` 索引から組み立てた続きの文脈（`run_worker` は events から求める代わりにこれを使う）。
    continuation_override: Option<task_worker::ContinuationContext>,
    /// ADR-0072 D13/D14（Phase E3）: task-local な planner run にだけ `Some`
    /// （`RunContext.execution_planner` にそのまま乗る）。
    execution_planner: Option<task_worker::protocol::ExecutionPlannerContext>,
    /// ADR-0072 D14（Phase E4b 項目3）: planner run にだけ `Some(self.config.execution.planner.
    /// permission_mode)`。`RunContext` には乗らない（ワーカーへの文脈ではなく、`run_worker` が
    /// `adapter.with_permission_mode` で実際の CLI 引数を上書きするためだけの、dispatcher 内部の
    /// 配線）。
    planner_permission_mode: Option<String>,
    /// ADR-0074 D1.2（Phase F2b）: v2 の WU の run の成果物の置き場（`<task_dir>/wu/<key>/artifacts`。
    /// 並列の WU が同じ `artifacts/checkpoint.json` を上書きしないため）。`runs/` は Task のものを共有する。
    artifacts_dir_override: Option<PathBuf>,
    /// ADR-0074 F5-fix / ADR-0075 D3: 自分の worktree で走る v2 の WU の run だけ `Some((work_unit_id, key))`。
    /// `run_worker` が `CARGO_TARGET_DIR` を WU ごとにする（scratch なら owner `task-<id>/wu-<id>`、無効なら
    /// `<repo-key>/wu-<id>`。兄弟 WU と target を共有しない）。
    cargo_target_work_unit: Option<(String, String)>,
    /// ADR-0079 D7（Phase R3a）: 木の節点の worker の run（planner でない）だけ `true`（`result.json` の
    /// `decisions` で人への決定の要求を出せることを前置きで伝える）。
    decision_requests: bool,
    /// ADR-0074「R7-11 実装時の明確化」: この run の実効の予算（planner なら `[execution.planner]`、WU なら D18、
    /// 知識整理のフォールバックなら ADR-0052 の値、それ以外は task の予算）。`run_worker` は DB から読み直した
    /// 写しの `budget` をこれで置き換える（`max_turns` が `RunRequest.task.budget` → `--max-turns` に届くように）。
    budget: Option<task_core::Budget>,
}

struct ReviewEntry {
    /// Keep ownership until the verdict transaction has completed, across daemon handoff.
    _review_lock: Arc<std::fs::File>,
    handle: JoinHandle<()>,
    /// `Reviewer` run を起動する場合に選んだプロバイダ（並列度の枠を消費する）。
    provider: Option<ProviderId>,
    /// レビューを延期（Reviewer run の供給側失敗）するときに次 tick へ持ち越す `done` の内容。
    subject: ReviewSubject,
    /// レビュー対象の run（デーモンのスナップショット用）。
    run_id: String,
    /// ADR-0018: 判定コマンドを実行するクラスタ（ローカルなら `None`）。
    cluster: Option<String>,
    /// Reviewer run を起動した場合のその run の id（スナップショットの `in_flight` 用。ADR-0014 D1）。
    review_run_id: Option<String>,
    since: OffsetDateTime,
    /// ADR-0024 D2/D3: プールから選んだアカウント（プールを使わない、または Reviewer run 自体を起動しない場合は `None`）。
    account: Option<String>,
    /// ADR-0025 D1: `account` が属するアダプタ（`account` が `None` なら `None`）。
    account_adapter: Option<AccountAdapter>,
}

pub struct Dispatcher {
    store: Arc<dyn TaskStore>,
    policy: Box<dyn ProviderPolicy>,
    models: HashMap<ProviderId, String>,
    /// プロバイダ（= アカウント）ごとのアダプタのインスタンス（ADR-0012 D1）。
    adapters: HashMap<ProviderId, Arc<dyn WorkerAdapter>>,
    config: DispatchConfig,
    #[cfg(test)]
    test_now: Option<Arc<StdMutex<OffsetDateTime>>>,
    #[cfg(test)]
    test_policy_clock: Option<Arc<StdMutex<Instant>>>,
    /// ADR-0074 D1.5（Phase F2）: 鍵は (task, WU)。Task 単位の問いは `running_for_task`。
    running: HashMap<RunKey, RunEntry>,
    reviewing: HashMap<TaskId, ReviewEntry>,
    /// レビューを開始できなかった（`Reviewer` run の枠が無い）タスクの `done` 内容。次 tick で使う。
    pending_subjects: HashMap<TaskId, ReviewSubject>,
    /// ADR-0070 D3（Phase 116）: `Trigger::InfraRequeue` で `Ready` に戻したタスクの再 dispatch を
    /// バックオフさせる（`task_ops::derive::infra_backoff_delay`）。プロセス内メモリのみ（DB に
    /// 永続化しない。dispatcher の再起動で猶予は失われるが、上限判定自体は `consecutive_infra_requeues`
    /// が events から数え直すので安全側。ADR-0070 §2 D3 参照）。
    infra_backoff: HashMap<TaskId, OffsetDateTime>,
    /// ADR-0074「Phase F5-fix7 実装時の明確化」: WU（id）の worktree の用意の一時的な失敗の回数と
    /// 次に試してよい時刻。成功・blocked にしたら消す。プロセス内メモリのみ（再起動で数え直す）。
    wu_prepare_failures: HashMap<String, WuPrepareFailures>,
    /// ADR-0079 D10（Phase R3b）: 理由なく止まっている（`LivenessClass::Unexplained`）と最初に見た木の節点と、その時の
    /// 節点の最後の event の seq（seq が変われば数え直す）。プロセス内メモリのみ（再起動で数え直す = 安全側）。
    stall_watch: HashMap<TaskId, (u64, OffsetDateTime)>,
    /// ADR-0079 D10（Phase R3b）: 最後に生存確認をした時刻（[`LIVENESS_CHECK_INTERVAL_SECS`] ごと）。
    liveness_checked_at: Option<OffsetDateTime>,
    /// ADR-0079 付記「R6-1」D4: 最後に終端の task の `runs` 索引の `running` の行を照合した時刻（起動後の最初の
    /// tick と [`RUNS_RECONCILE_INTERVAL_SECS`] ごと）。
    runs_reconciled_at: Option<OffsetDateTime>,
    disk_low: bool,
    /// ADR-0074 F5-fix: 終端の WU の target を消す別スレッドが動いている間は `true`（重ねて起こさない）。
    removing_build_caches: Arc<std::sync::atomic::AtomicBool>,
    /// ADR-0075（Phase G1）: scratch pool の GC の状態（測定の cache・削除 / 測定スレッドの印・adopt の候補・観測値）。
    scratch: ScratchState,
    disk_ready: bool,
    /// 「設定に合うプロバイダが無い」警告を出した（連続 tick で繰り返さない）タスク（ADR-0012 D2）。
    warned_unroutable: std::collections::HashSet<TaskId>,
    /// ADR-0062 B1（Phase 107）: 「担当が cluster:<id> を持たない」警告を出した（連続 tick で
    /// 繰り返さない）タスク。担当・道具が変わって使えるようになれば `task_may_use_cluster` が消す。
    warned_cluster_tool: std::collections::HashSet<TaskId>,
    /// この tick で `NoMatchingProvider` だった ready タスク（`is_idle` で待ち対象から外す。ADR-0012 D2）。
    unroutable: std::collections::HashSet<TaskId>,
    /// ADR-0044 D2（Phase 53）: **この tick で run を打ち切った**タスク。次の tick まで dispatch しない。
    /// 打ち切りは `handle.abort()`（= 子プロセスへの SIGKILL）で、**孫プロセスは即死しない**ので、
    /// 同じ tick で同じ worktree に次の run を入れると 2 つの書き手が重なる（Phase 53 の監査で発見）。
    just_aborted: std::collections::HashSet<TaskId>,
    /// ADR-0018 D2（監査 4-1）: この tick でクラスタの多重接続が無い／cooldown 中のため待っている ready タスク。「人のログイン待ち」で
    /// 経路なし（`unroutable`）とは別物。`is_idle` の待ち対象から外すだけで、スナップショットには出さない（受信箱の (d) が知らせる）。
    cluster_waiting: std::collections::HashSet<TaskId>,
    /// 人間の承認待ちで延期中の reviewing タスク（`is_idle` 判定用。ADR-0010 D8）。
    awaiting_human: std::collections::HashSet<TaskId>,
    /// ADR-0016 D2 / M5: レビューは全 pass だが、委譲した子が終端になるのを待っている reviewing タスク。
    /// 値はその run の id と、Plan kind なら検証済みの plan（子が終わってから `complete_plan` する）。
    awaiting_children: HashMap<TaskId, AwaitingChildren>,
    /// ADR-0074 D1.4（Phase F2b）: 工程の統合を走らせている Task（spawn した git 操作と検査）。
    /// 再起動の照合（D1.7）は「running の統合 WU で、ここに無いもの」を pending に戻す。
    integrating: HashMap<TaskId, IntegrationEntry>,
    /// Phase F5-fix2: WU の `checks` を走らせている run（キーは run id）。
    checking: HashMap<String, CheckingEntry>,
    /// Phase F5-fix6: 居なくなったデーモンの run（孤児）を lease 失効を待たずに回収する（`None` = 無効）。
    orphan_takeover: Option<crate::orphan::OrphanTakeover>,
    tx: mpsc::UnboundedSender<Completion>,
    rx: mpsc::UnboundedReceiver<Completion>,
    /// ADR-0018 D2: 多重接続が無いクラスタの cooldown（この時刻まで dispatch しない）。
    cluster_cooldown: HashMap<String, Instant>,
    /// ADR-0018 D2: 直近の `ssh -O check` の結果（クラスタ id → 多重接続があるか）。`refresh_cluster_liveness` が埋める。
    cluster_connected: HashMap<String, bool>,
    /// ADR-0023 D1: 最後に `ssh -O check` を回した時刻（`CLUSTER_LIVENESS_INTERVAL` に 1 回だけ回す）。
    last_cluster_liveness: Option<Instant>,
    /// ADR-0053 Phase 84b: `refresh_cluster_liveness` が使う「master 生存」判定フック。既定は本物の
    /// `control_master_alive_blocking`（`ssh -O check`）。テストは `set_cluster_liveness_probe` で
    /// 偽物に差し替え、実機の ssh 状態に依存しないようにする。
    cluster_liveness_probe: ClusterLivenessProbe,
    /// tick の回数（スナップショット用）。
    ticks: u64,
    publisher: Option<SnapshotPublisher>,
    /// ADR-0024 D2: `account_pool = true` のプロバイダ id。`reload_providers` で差し替える。
    account_pool_providers: std::collections::HashSet<ProviderId>,
    /// ADR-0024 D4 / ADR-0025 D1: アダプタごとのアカウントの観測値・cooldown・確認の帳簿（設定された根ディレクトリの
    /// アダプタだけキーを持つ）。実行中の run のシンクとも共有する。reload では差し替えない（設定ファイルの
    /// 再読込では消えない観測値）。
    account_books: HashMap<AccountAdapter, Arc<StdMutex<AccountBook>>>,
    /// ADR-0024 D1: 選択のたびにディレクトリを読み直さないよう、tick につき高々 1 回だけスキャンする（アダプタごと）。
    accounts_scan_cache: HashMap<AccountAdapter, Vec<AccountDir>>,
    /// ADR-0024 D5/D7 / ADR-0025 D5: celeris（GUI の管理 API）が進行中のログイン中継を持っているアカウント
    /// （キーは `"<adapter>:<id>"`。同じ id でもアダプタが違えば別のログインとして扱う）。
    login_pending_accounts: std::collections::HashSet<String>,
    /// ADR-0043 D3（Phase 56）: 起動時に調べたコンテナ runtime（観測値）。celeris が
    /// `detect_container_runtime()` を呼んで埋める。埋まっていなければ「使えない」と同じ扱いで、
    /// コンテナが要るタスクは `blocked` になる。
    container_probe: task_worker::RuntimeProbe,
    /// ADR-0032 D3: `auth = "publickey"` のクラスタに自動で接続を張るフック。`None` なら自動接続しない
    /// （celeris 側が `set_cluster_connector` で挿す。未設定＝従来どおりの挙動）。
    cluster_connector: Option<ClusterConnector>,
    /// ADR-0062 A（Phase 107）: master 越しの実通信で生存を確定させるフック（`ssh -o BatchMode=yes
    /// <host> -- true`）。`None` なら実通信の probe はしない（`-O check` だけの従来どおり）。
    cluster_command_probe: Option<ClusterCommandProbe>,
    /// ADR-0062 A: クラスタごとに最後に実通信 probe を行った時刻（`liveness_probe_secs` の間引きに使う）。
    last_cluster_command_probe: HashMap<String, Instant>,
    /// ADR-0090 D2: クラスタ job の poll のフック（`None` なら poll しない。celeris が本物を挿す）。
    cluster_job_poller: Option<ClusterJobPoller>,
    /// ADR-0090 D2: 走っている poll（wait id → 結果の受け口）。結果は次の tick 以降に拾う。
    cluster_job_polls: HashMap<
        String,
        std::sync::mpsc::Receiver<Result<task_worker::RemoteCommandOutput, String>>,
    >,
    /// ADR-0078: クラスタごとの接続の帳簿（遷移・probe・鍵認証の再接続・回数）。
    cluster_conn: HashMap<String, ClusterConnState>,
    /// ADR-0078 D5: 直前の tick の開始時刻と、その前の tick との間隔（`cluster ssh master lost` の
    /// `last_tick_gap_ms`。tick の停止による切断〈ADR-0078 C3〉を見分ける）。
    last_tick_started: Option<Instant>,
    last_tick_gap_ms: u64,
    /// ADR-0062 A: 明示的な切断を経ずに接続が失われたクラスタの詳細（exit code・stderr の末尾）。
    /// `mark_cluster_unavailable` が拾って報告に足し、`Event::ClusterMasterExited` を 1 回だけ残す
    /// （`reported`）。接続が戻れば消す。
    cluster_disconnect_info: HashMap<String, ClusterDisconnectInfo>,
    /// ADR-0062 A: celeris が保持している master（`Child`）の終了を検出するフック。`None` なら見ない
    /// （celeris 側が `set_cluster_master_watcher` で挿す）。
    cluster_master_watcher: Option<ClusterMasterWatcher>,
    /// ADR-0032 D4/D5: GUI 発の接続（`POST /clusters/{id}/connect`）が進行中のクラスタ id
    /// （celeris が `set_cluster_connect_pending` で反映する。D3 の自動接続とは別物）。
    connect_pending_clusters: std::collections::HashSet<String>,
    /// 壁時計の Unix 秒（テストで差し替えられるようにした関数。既定は実時刻）。
    now_unix_fn: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// ADR-0040 D4（Phase 47）: 新しい仕事を始めてよいか。`false`（= draining）のときは
    /// `dispatch_ready` も `recover_reviews` も動かさず、**手元の run とレビューの面倒だけ見続ける**
    /// （完了の記録、リースの更新、`aggregate` / `child_failed` の後処理は通常どおり動く）。
    accepting_new_work: bool,
    /// ADR-0041 D1 / ADR-0043 D2: この celeris が用意したローカルの作業場所（1 つ以上のリポジトリ）。
    /// **終端では消さない**（ADR-0043 D2 の改定。差分を見るために残す）。消すのは**中止**（cancel）
    /// のときだけで、worktree とブランチを消す。再起動では失われる（残った worktree は人が
    /// `git worktree remove` する。PROGRESS の未解決）。
    task_workspaces: HashMap<TaskId, task_worker::TaskWorkspaces>,
    /// ADR-0041 D5: 面倒を見てよいタスクの述語（`None` なら全部）。`--mode verify` の煙試験でだけ使う。
    eligible: Option<TaskFilter>,
    /// ADR-0052 D1（Phase 64）: 知識整理タスクを dispatch する直前に当てる到達性の検査
    /// （既定は `task_worker::probe_models`。テストは `set_knowledge_probe` で差し替える）。
    /// **LLM は呼ばない**（`GET <base_url>/models` の 1 回だけ）。
    knowledge_probe: KnowledgeProbe,
    /// ADR-0052 D1: 検査の結果のキャッシュ（`base_url` → (いつ調べたか, 結果)）。60 秒。
    knowledge_probe_cache: HashMap<String, (Instant, Reachability)>,
    /// ADR-0053 D3（Phase 66）: `[[clusters]].forwards` を(再)確立するフック。`None` なら何もしない。
    tunnel_forward_ensurer: Option<TunnelForwardEnsurer>,
    /// ADR-0053 D3 / Phase 85: forward の target（先方）の健康を見るフック。`None` なら常に「不健全」扱い。
    tunnel_probe: Option<TunnelProbe>,
    /// ADR-0053 Phase 85: forward のリスナーの有無を見るフック。`None` なら常に「無い」扱い
    /// （安全側のデフォルト。`tunnel_forward_ensurer` に(再)確立を試みさせる。Phase 66 までと同じ保守的な
    /// 挙動）。
    tunnel_listener_probe: Option<TunnelListenerProbe>,
    /// ADR-0053 D3 / Phase 85: forward ごとの直近の観測（キーは `tunnel_key`）。listener と target の
    /// 健康を別々に持つ（`ForwardObservation`）。
    tunnel_state: HashMap<String, ForwardObservation>,
    /// ADR-0066 D3（Phase 110b）: forward ごとの target probe の直近の結果（キーは `tunnel_key`）。
    /// **専用スレッド（`ensure_tunnel_prober_started`）が書き、`refresh_one_forward` は読むだけ**
    /// （tick の同期経路から HTTP probe を外すため）。listener が初めて有りになった forward は、
    /// ここにまだ記録が無い間だけ `refresh_one_forward` が 1 回だけ同期に probe して種を蒔く。
    tunnel_probe_state: Arc<std::sync::Mutex<HashMap<String, TargetProbeState>>>,
    /// ADR-0066 D3: 専用スレッドを起こしたら `Some`（二重に起こさない）。`Drop` でスレッドに停止を伝える。
    tunnel_prober_stop: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// ADR-0053 D3: 「TOTP ログインが要る」と判定済みのクラスタ id（重複通知を防ぐ。master が戻れば消す）。
    cluster_login_needed: std::collections::HashSet<String>,
    /// ADR-0053 D3: 直近のトンネル状態遷移（Console / cluster API 向け。`take_tunnel_events` で取り出す）。
    tunnel_events: std::collections::VecDeque<TunnelEvent>,
    /// ADR-0053 D3: 最後にトンネルの生存を見た時刻（`refresh_cluster_liveness` と同じ間隔で間引く）。
    last_cluster_tunnel_refresh: Option<Instant>,
    /// ADR-0074 D4（Phase F3 quota）: run の重なりを追跡し、measured/apportioned/pending を決定する
    /// （プロセス内メモリのみ。`AccountBook` とは別軸で、replay の対象外）。
    quota_activity: crate::accounts::QuotaActivity,
    /// ADR-0074 D4.2（Phase F3 quota）: `estimated` の較正材料（source × 窓ごとの直近 `measured`）。
    quota_calibration: crate::accounts::QuotaCalibrationBook,
}

impl Drop for Dispatcher {
    /// ADR-0066 D3: 専用の target-probe スレッド（起こしていれば）に停止を伝える。tick を止めない設計と
    /// 同じ理由で、ここでも join はしない（スレッドは次の周回〈`TUNNEL_PROBER_STEP`〉で自分から終わる）。
    fn drop(&mut self) {
        if let Some(stop) = &self.tunnel_prober_stop {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// ADR-0052 D1（Phase 65b で `bearer_token` を追加）: 到達性の検査のフック（差し替えられるようにして
/// ある。既定は本物の HTTP GET）。第 2 引数は `[knowledge.langmem].api_key_secret` から解決した
/// 平文のトークン（`llm-proxy` のように `/v1/models` が認証を要求する上流のため。値はログに出さない）。
pub type KnowledgeProbe = Arc<dyn Fn(&str, Option<&str>) -> Reachability + Send + Sync>;

/// ADR-0052 D2: フォールバックする run に載せる上書き（`run_extras` の結果に混ぜる）。
#[derive(Debug, Clone)]
struct KnowledgeFallbackRun {
    /// 倒した先のアダプタ（`WorkerStarted.adapter` と同じ。ログと進行イベントに出す）。
    adapter: String,
    /// 前置き（`task_worker::knowledge_fallback_instructions`）。
    instructions: String,
    /// ADR-0052 D2 の予算（`max_turns = 8` / `max_wall_secs = 600`）。
    budget: task_core::Budget,
}

/// ADR-0052 D2: フォールバック run の予算。
const KNOWLEDGE_FALLBACK_MAX_TURNS: u32 = 8;
const KNOWLEDGE_FALLBACK_MAX_WALL_SECS: u64 = 600;
/// ADR-0052 D2: フォールバック run が書く候補ファイル（ワーカーから見た位置）。
const KNOWLEDGE_CANDIDATES_REL: &str = "artifacts/knowledge-candidates.json";

fn real_now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

impl Dispatcher {
    fn now_utc(&self) -> OffsetDateTime {
        #[cfg(test)]
        if let Some(clock) = &self.test_now {
            return *clock.lock().expect("test clock mutex");
        }
        OffsetDateTime::now_utc()
    }

    fn monotonic_now(&self) -> Instant {
        #[cfg(test)]
        if let Some(clock) = &self.test_policy_clock {
            return *clock.lock().expect("test monotonic clock mutex");
        }
        Instant::now()
    }
    /// `models` は provider id → `WorkerStarted.model` に記録するモデル名。`account_pool_providers` は
    /// `account_pool = true` のプロバイダ id（ADR-0024 D2）。
    pub fn new(
        store: Arc<dyn TaskStore>,
        policy: Box<dyn ProviderPolicy>,
        models: HashMap<ProviderId, String>,
        adapters: HashMap<ProviderId, Arc<dyn WorkerAdapter>>,
        account_pool_providers: std::collections::HashSet<ProviderId>,
        config: DispatchConfig,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        // ADR-0024 D4 / ADR-0025 D1: `<root>/.celeris-usage.json` から観測値・cooldown を読む（無ければ空から始める）。
        // アダプタごとに別の根ディレクトリ・別の帳簿（アカウントの記録はそのアダプタの中で閉じる）。
        let account_books: HashMap<AccountAdapter, Arc<StdMutex<AccountBook>>> =
            match &config.accounts {
                Some(accounts) => accounts
                    .roots
                    .iter()
                    .map(|(adapter, root)| {
                        (
                            *adapter,
                            Arc::new(StdMutex::new(AccountBook::load(
                                &root.join(".celeris-usage.json"),
                            ))),
                        )
                    })
                    .collect(),
                None => HashMap::new(),
            };
        // ADR-0075 D1: scratch を NFS 上で無効化したときは起動ログに理由を出す（従来の build_cache_dir に戻る）。
        if let Some(reason) = &config.scratch.disabled_reason {
            tracing::warn!(%reason, "scratch pool disabled");
        } else if config.scratch.enabled && config.shared_build_cache {
            tracing::info!(dir = %config.scratch.dir.display(), "scratch pool enabled (ADR-0075)");
            // ADR-0075 D4（Phase G2）: sccache を配線するか（run ごとにも確かめる。ここは起動ログだけ）。
            let state = task_worker::scratch::resolve_sccache(
                &config.scratch,
                task_worker::scratch::server_listening,
            );
            match state.reason() {
                None => tracing::info!(
                    port = config.scratch.sccache.server_port,
                    binary = %config.scratch.sccache.binary.display(),
                    "sccache L1 wired into cargo runs (ADR-0075 D4)"
                ),
                Some(reason) => tracing::info!(
                    state = state.label(),
                    reason,
                    "sccache L1 not wired; runs use plain cargo (ADR-0075 D4)"
                ),
            }
        }
        Self {
            store,
            policy,
            models,
            adapters,
            config,
            #[cfg(test)]
            test_now: None,
            #[cfg(test)]
            test_policy_clock: None,
            running: HashMap::new(),
            reviewing: HashMap::new(),
            pending_subjects: HashMap::new(),
            infra_backoff: HashMap::new(),
            wu_prepare_failures: HashMap::new(),
            stall_watch: HashMap::new(),
            liveness_checked_at: None,
            runs_reconciled_at: None,
            disk_low: false,
            removing_build_caches: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            scratch: ScratchState::default(),
            disk_ready: true,
            warned_unroutable: std::collections::HashSet::new(),
            warned_cluster_tool: std::collections::HashSet::new(),
            just_aborted: std::collections::HashSet::new(),
            unroutable: std::collections::HashSet::new(),
            cluster_waiting: std::collections::HashSet::new(),
            awaiting_human: std::collections::HashSet::new(),
            awaiting_children: HashMap::new(),
            integrating: HashMap::new(),
            checking: HashMap::new(),
            orphan_takeover: None,
            tx,
            rx,
            cluster_cooldown: HashMap::new(),
            cluster_connected: HashMap::new(),
            last_cluster_liveness: None,
            cluster_liveness_probe: Arc::new(|ssh_command: &[String], host: &str| {
                control_master_alive_blocking(ssh_command, host)
            }),
            ticks: 0,
            publisher: None,
            account_pool_providers,
            account_books,
            accounts_scan_cache: HashMap::new(),
            login_pending_accounts: std::collections::HashSet::new(),
            container_probe: task_worker::RuntimeProbe::default(),
            cluster_connector: None,
            cluster_command_probe: None,
            last_cluster_command_probe: HashMap::new(),
            cluster_job_poller: None,
            cluster_job_polls: HashMap::new(),
            cluster_conn: HashMap::new(),
            last_tick_started: None,
            last_tick_gap_ms: 0,
            cluster_disconnect_info: HashMap::new(),
            cluster_master_watcher: None,
            connect_pending_clusters: std::collections::HashSet::new(),
            now_unix_fn: Arc::new(real_now_unix),
            accepting_new_work: true,
            task_workspaces: HashMap::new(),
            eligible: None,
            knowledge_probe: Arc::new(|base_url, bearer_token| {
                task_worker::probe_models(base_url, task_worker::PROBE_TIMEOUT, bearer_token)
            }),
            knowledge_probe_cache: HashMap::new(),
            tunnel_forward_ensurer: None,
            tunnel_probe: None,
            tunnel_listener_probe: None,
            tunnel_state: HashMap::new(),
            tunnel_probe_state: Arc::new(std::sync::Mutex::new(HashMap::new())),
            tunnel_prober_stop: None,
            cluster_login_needed: std::collections::HashSet::new(),
            tunnel_events: std::collections::VecDeque::new(),
            last_cluster_tunnel_refresh: None,
            quota_activity: crate::accounts::QuotaActivity::new(),
            quota_calibration: crate::accounts::QuotaCalibrationBook::new(),
        }
    }

    /// ADR-0052 D1: 到達性の検査を差し替える（テストは偽のローカルサーバも起こさずに済ませる）。
    pub fn set_knowledge_probe(&mut self, probe: KnowledgeProbe) {
        self.knowledge_probe = probe;
        self.knowledge_probe_cache.clear();
    }

    /// ADR-0052 D1: `[knowledge.langmem].base_url` の到達性（60 秒キャッシュ）。
    /// `base_url` が無ければ「検査できない」= [`Reachability::Unknown`]。
    fn knowledge_reachability(&mut self, now: Instant) -> Reachability {
        let Some(base_url) = self.config.knowledge.langmem_base_url.clone() else {
            return Reachability::Unknown {
                reason: "[knowledge.langmem].base_url が無い".to_string(),
            };
        };
        if let Some((checked_at, cached)) = self.knowledge_probe_cache.get(&base_url)
            && now.duration_since(*checked_at) < task_worker::PROBE_CACHE_TTL
        {
            return cached.clone();
        }
        let token = self.config.knowledge.langmem_api_key.clone();
        let started = Instant::now();
        let outcome = (self.knowledge_probe)(&base_url, token.as_deref());
        log_slow_step("knowledge_probe", started);
        tracing::debug!(%base_url, ?outcome, "knowledge: probed the langmem endpoint");
        self.knowledge_probe_cache
            .insert(base_url, (now, outcome.clone()));
        outcome
    }

    /// ADR-0043 D3（Phase 56）: 起動時にコンテナ runtime を調べる（`podman info` → `docker info`）。
    /// 結果はログと `GET /daemon` に出る。**呼ばなければコンテナが要るタスクは `blocked`** になる
    /// （テストは `set_container_probe` で差し替える。`cargo test` は runtime を起こさない）。
    pub fn detect_container_runtime(&mut self) {
        let probe = task_worker::container::detect(
            self.config.containers.preference,
            CONTAINER_PROBE_TIMEOUT,
        );
        match probe.runtime {
            Some(rt) => tracing::info!(
                runtime = rt.as_str(),
                preference = %probe.preference,
                image_default = %self.config.containers.image_default,
                "container runtime detected"
            ),
            None => tracing::warn!(
                preference = %probe.preference,
                detail = %probe.summary(),
                "no container runtime; tasks that need one will be blocked"
            ),
        }
        self.container_probe = probe;
    }

    /// 調べた結果を差し替える（celeris の起動経路とテスト用）。
    pub fn set_container_probe(&mut self, probe: task_worker::RuntimeProbe) {
        self.container_probe = probe;
    }

    /// 起動時に調べたコンテナ runtime（スナップショット用）。
    pub fn container_probe(&self) -> &task_worker::RuntimeProbe {
        &self.container_probe
    }

    /// ADR-0040 D4（Phase 47）: 新しい仕事を始めるのをやめる／再開する。`false` にすると
    /// `dispatch_ready`（ready なタスクの起動）と `recover_reviews`（他のインスタンスが抱えている
    /// かもしれない reviewing の拾い上げ）を止める。**手元の run とレビューはそのまま面倒を見る**。
    pub fn set_accepting_new_work(&mut self, accepting: bool) {
        self.accepting_new_work = accepting;
    }

    /// Phase F5-fix6: 孤児の回収を有効にする（celeris が `daemon_instances` の自分の行を持つときに
    /// 渡す。判定の定義は `crate::orphan`）。
    pub fn set_orphan_takeover(&mut self, takeover: crate::orphan::OrphanTakeover) {
        self.orphan_takeover = Some(takeover);
    }

    /// ADR-0041 D5: 面倒を見てよいタスクを絞る（`--mode verify` の煙試験）。呼ばなければ従来どおり全部。
    /// 絞られたタスクは **ready のまま放置**され、リースの回収もレビューの拾い上げも起きない。
    pub fn set_eligible_tasks(&mut self, eligible: TaskFilter) {
        self.eligible = Some(eligible);
    }

    /// そのタスクをこのインスタンスが触ってよいか（述語が無ければ常に真）。
    fn is_eligible(&self, task: &Task) -> bool {
        self.eligible.as_ref().is_none_or(|f| f(task))
    }

    /// 手元で動いている run とレビューの数（ADR-0040 D4 の drain の判定に使う）。
    ///
    /// Phase F5-fix2: run が終わった後の WU の `checks`（`checking`）と工程の統合（`integrating`）も
    /// 数える。数えないと、draining のインスタンスは run の完了を受けて検査を spawn した直後に
    /// 「手元が 0」と判断して exit し、検査の完了（`Completion::WorkUnitChecks`）ごと失う。
    /// 本番（dogfood 4 回目の `gate` WU）では、その run は `running` のまま検査前に延ばした lease
    /// （`review_timeout × (2n+1) + lease_grace`）が切れるまで放置され、`lease expired` で requeue された。
    pub fn in_flight(&self) -> usize {
        self.running.len() + self.reviewing.len() + self.checking.len() + self.integrating.len()
    }

    pub fn set_delivery_policy(&mut self, policy: task_ops::delivery::DeliveryPolicy) {
        self.config.delivery = policy;
    }

    pub fn config(&self) -> &DispatchConfig {
        &self.config
    }

    /// ADR-0033 D3: tick ループ（celeris）が報告の圧縮のためにストアを読む。ディスパッチャ自身の
    /// 判断には使わない（ここから LLM を呼ぶこともない）。
    pub fn store(&self) -> Arc<dyn TaskStore> {
        Arc::clone(&self.store)
    }

    /// テスト用: 壁時計の Unix 秒を差し替える（ADR-0024 D3 のスコア計算・cooldown の期限に使う）。
    pub fn set_now_unix_fn(&mut self, f: Arc<dyn Fn() -> i64 + Send + Sync>) {
        self.now_unix_fn = f;
    }

    /// ADR-0013 D4: tick ごとにデーモンのスナップショットを `watch` に送るようにする。
    pub fn set_snapshot_publisher(&mut self, publisher: SnapshotPublisher) {
        self.publisher = Some(publisher);
    }

    /// ADR-0017 M2: `POST /api/v1/reload` — 稼働中のプロバイダ選定・アダプタ一式を丸ごと差し替える。
    /// 実行中の run はそれぞれ差し替え前のアダプタの `Arc` を既に掴んでいるので影響を受けない（D1）。
    /// `AccountBook`（観測値・cooldown）は reload では差し替えない（ADR-0024 D4: 設定ではなく観測値なので）。
    pub fn reload_providers(
        &mut self,
        policy: Box<dyn ProviderPolicy>,
        models: HashMap<ProviderId, String>,
        adapters: HashMap<ProviderId, Arc<dyn WorkerAdapter>>,
        account_pool_providers: std::collections::HashSet<ProviderId>,
    ) {
        self.policy = policy;
        self.models = models;
        self.adapters = adapters;
        self.account_pool_providers = account_pool_providers;
    }

    /// Phase 44（実機 2026-09-18）: `POST /reload` で `[[roles]]` / `[[genres]]` / `[delegation]` も読み直す。
    /// `reload_providers` とは別トランザクション（呼び出し側が両方呼ぶ）。実行中の run はそれぞれ `spawn_worker`
    /// 時点でこれらの値の写しを既に掴んでいるので、反映されるのは**次に起動する run から**
    /// （委譲で作られる子の budget を含む）。
    pub fn reload_config(
        &mut self,
        roles: Vec<RoleSpec>,
        genres: Vec<GenreSpec>,
        delegation: DelegationLimits,
    ) {
        self.config.roles = roles;
        self.config.genres = genres;
        self.config.delegation = delegation;
    }

    // ---- ADR-0024/0025: celeris（GUI の管理 API）が使うアカウント操作 ----

    /// そのアダプタ・アカウントで走っている run（ワーカー run + Reviewer run）の数。
    pub fn account_in_use(&self, adapter: AccountAdapter, id: &str) -> usize {
        let matches = |a: &Option<AccountAdapter>, acct: &Option<String>| {
            *a == Some(adapter) && acct.as_deref() == Some(id)
        };
        self.running
            .values()
            .filter(|e| matches(&e.account_adapter, &e.account))
            .count()
            + self
                .reviewing
                .values()
                .filter(|e| matches(&e.account_adapter, &e.account))
                .count()
    }

    /// ADR-0025 D1: `login_pending_accounts` のキー（同じ id でもアダプタが違えば別のログインとして扱う）。
    fn login_pending_key(adapter: AccountAdapter, id: &str) -> String {
        format!("{adapter}:{id}")
    }

    /// D7: 進行中のログイン中継の有無を記録する（celeris の `HashMap<String, LoginSession>` と対）。
    pub fn set_account_login_pending(&mut self, adapter: AccountAdapter, id: &str, pending: bool) {
        let key = Self::login_pending_key(adapter, id);
        if pending {
            self.login_pending_accounts.insert(key);
        } else {
            self.login_pending_accounts.remove(&key);
        }
    }

    /// ログイン中継の実行中は定期確認しない。
    pub fn account_login_pending(&self, adapter: AccountAdapter, id: &str) -> bool {
        self.login_pending_accounts
            .contains(&Self::login_pending_key(adapter, id))
    }

    /// ADR-0053 D1（Phase 65）: `llm-proxy` が同じアカウントプールの cooldown・観測値を共有するための
    /// アクセサでもある（CLI ワーカーの dispatch と**同じ帳簿**を返す。別の写しを作らない）。
    /// そのアダプタの `[accounts]` 根が設定されていなければ `None`。
    pub fn account_book(&self, adapter: AccountAdapter) -> Option<Arc<StdMutex<AccountBook>>> {
        self.account_books.get(&adapter).cloned()
    }

    /// D6: 手動確認の結果を `AccountBook` に記録して保存する（`source = "check"`）。
    pub fn record_account_check(
        &mut self,
        adapter: AccountAdapter,
        id: &str,
        result: &str,
        detail: Option<String>,
        observation: Option<RateLimitObservation>,
    ) {
        let now = (self.now_unix_fn)();
        let Some(book) = self.account_book(adapter) else {
            return;
        };
        let Ok(mut book) = book.lock() else { return };
        let has_observation = observation.is_some();
        if let Some(obs) = observation {
            book.record_observation(id, obs, ObservationSource::Check);
        }
        match result {
            "ok" => {
                book.clear_cooldown(id);
                if adapter == AccountAdapter::Codex && !has_observation {
                    book.clear_observation(id);
                }
            }
            "auth_failed" | "throttled" => {
                let reason = if result == "auth_failed" {
                    AccountCooldownReason::AuthFailed
                } else {
                    AccountCooldownReason::Throttled
                };
                let cooldown = cooldown_for_failure(
                    book.state(id),
                    reason,
                    now,
                    self.config
                        .accounts
                        .as_ref()
                        .map_or(300, |c| c.fallback_cooldown_secs),
                );
                book.set_cooldown(id, cooldown, now);
            }
            _ => {} // 通信失敗だけでは前回の観測を消さない。
        }
        book.record_check(
            id,
            AccountCheckRecord {
                at: now,
                result: result.to_string(),
                detail,
            },
        );
        if let Err(e) = book.save() {
            tracing::warn!(account_id = %id, %adapter, error = %e, "failed to save account book after check");
        }
    }

    /// D5 `DELETE /accounts/{id}`: 帳簿からもこのアカウントの記録を消す（ディレクトリの移動は celeris/task-api が行う）。
    pub fn remove_account_book_entry(&mut self, adapter: AccountAdapter, id: &str) {
        let Some(book) = self.account_book(adapter) else {
            return;
        };
        let Ok(mut book) = book.lock() else { return };
        book.remove(id);
        if let Err(e) = book.save() {
            tracing::warn!(account_id = %id, %adapter, error = %e, "failed to save account book after removal");
        }
    }

    /// ADR-0017 M4: 次 tick のスナップショットに乗るプロバイダ一覧を差し替える（`reload_providers` とあわせて呼ぶ）。
    /// `set_snapshot_publisher` より前（`publisher` が無い状態）で呼んでも無害（何もしない）。
    pub fn set_snapshot_providers(&mut self, providers: Vec<ProviderLive>) {
        if let Some(publisher) = self.publisher.as_mut() {
            // ADR-0022 D2: 消えた id の確認記録は落とし、残った id の記録は保つ。
            let ids: std::collections::HashSet<&str> =
                providers.iter().map(|p| p.id.as_str()).collect();
            publisher
                .provider_checks
                .retain(|id, _| ids.contains(id.as_str()));
            publisher.providers = providers;
        }
    }

    /// ADR-0022 D2: 疎通確認の結果をスナップショットに載せる（DB には書かない）。次の tick から `GET /providers` に出る。
    pub fn set_provider_check(&mut self, provider_id: &str, check: ProviderCheckView) {
        if let Some(publisher) = self.publisher.as_mut() {
            publisher
                .provider_checks
                .insert(provider_id.to_string(), check);
        }
    }

    /// 1 tick。tokio ランタイム内から呼ぶ（ワーカーとレビューを `tokio::spawn` する）。
    pub fn tick(&mut self) -> Result<TickReport, DispatchError> {
        self.ticks += 1;
        let tick_started = Instant::now();
        if let Some(prev) = self.last_tick_started.replace(tick_started) {
            self.last_tick_gap_ms =
                u64::try_from(tick_started.duration_since(prev).as_millis()).unwrap_or(u64::MAX);
        }
        // ADR-0024 D1: 選択のたびに読み直さないよう、スキャンは tick ごとに高々 1 回（このキャッシュを毎 tick 捨てる）。
        self.accounts_scan_cache.clear();
        let now = (self.now_unix_fn)();
        for book in self.account_books.values() {
            if let Ok(mut book) = book.lock() {
                book.clear_expired(now);
            }
        }
        // drain_completions からも reviewer run が起動されるため、完了処理より先に判定する。
        self.disk_ready = !self.accepting_new_work || self.check_disk_space();
        let mut report = TickReport::default();
        // ADR-0015 D2: 遅い tick の内訳を出せるよう、段階ごとに所要時間を測る。
        let started = Instant::now();
        let mut at = Instant::now();
        let lap = |at: &mut Instant| {
            let d = at.elapsed().as_millis() as u64;
            *at = Instant::now();
            d
        };
        let (finished, reviewed) = self.drain_completions()?;
        let drain_ms = lap(&mut at);
        report.finished = finished;
        report.reviewed = reviewed;
        self.settle_awaiting_children()?;
        // ADR-0079 §7 R1b: 木の照合（子の状態の写しと、ready の kind task の unit からの子 task の生成）。
        self.reconcile_tree_units()?;
        // ADR-0079 D10（Phase R3b）: 木の生存確認（理由なく止まっている節点に `StallDetected` と障害通知を 1 回）。
        if let Err(e) = self.check_tree_liveness() {
            tracing::warn!(error = %e, "tree liveness check failed (ADR-0079 D10)");
        }
        // Phase F7: 認可元のタスクが終端のまま未決で残った認可の要求を閉じる（照合。通常は遷移が閉じる）。
        if let Err(e) = crate::approvals::withdraw_stale_approvals(
            self.store.as_ref(),
            OffsetDateTime::now_utc(),
        ) {
            tracing::warn!(error = %e, "failed to withdraw stale approvals");
        }
        // ADR-0080 D4: 期限の過ぎた browser の wait を一度だけ終端化する（起動直後の最初の tick が
        // 再起動時の照合を兼ねる）。人待ちの task は lease を持たないので worker slot は使っていない。
        match task_ops::browser::expire_due(self.store.as_ref(), OffsetDateTime::now_utc()) {
            Ok(expired) => {
                for w in expired {
                    tracing::info!(
                        task_id = %w.task_id,
                        wait_id = %w.wait_id,
                        reason = w.reason.as_str(),
                        "browser wait expired"
                    );
                }
            }
            Err(e) => tracing::warn!(error = %e, "failed to expire browser waits"),
        }
        report.reclaimed = self.reclaim_expired_leases()?;
        // ADR-0074 D1.7（Phase F2b）: v2 の Task の照合（WU の lease 切れ・何も走っていない Running）。
        self.reconcile_parallel_tasks()?;
        let reclaim_ms = lap(&mut at);
        self.abort_stale_runs()?;
        // ADR-0079 付記「R6-1」D4: 終端の task の `running` のままの `runs` 行を閉じる（起動時と定期）。
        self.reconcile_terminal_runs();
        // ADR-0043 D2: **中止**されたタスクの worktree とブランチを消す（終端〈done / failed〉では消さない）。
        self.cleanup_cancelled_worktrees()?;
        // ADR-0075 D2（Phase G1）: scratch pool の semantic GC（`scratch_gc` phase。rename まで、削除と測定は別スレッド）。
        // scratch が無効なら ADR-0074 F5-fix: 終端になった WU の target（WU ごとの `CARGO_TARGET_DIR`）を消す。
        if self.scratch_active() {
            if !self.scratch.ran_this_tick {
                self.scratch_gc(false);
            }
        } else {
            self.cleanup_work_unit_build_caches();
            self.scratch.view = self.config.shared_build_cache.then(|| {
                crate::scratch_gc::disabled_status(
                    &self.config.scratch,
                    std::time::SystemTime::now(),
                )
            });
        }
        self.scratch.ran_this_tick = false;
        let abort_ms = lap(&mut at);
        // ADR-0066 D2（Phase 110b）: 終端になってから `prune_after_secs` 経った作業場所から、ビルド
        // 生成物だけを刈る（1 tick に最大 1 か所。探すところまでは軽いので同期、削除は別スレッド）。
        self.prune_one_workspace();
        let prune_ms = lap(&mut at);
        // ADR-0040 D4: draining のインスタンスは新しい仕事を始めない（拾い上げも dispatch もしない）。
        // 手元の run とレビューの完了・リース更新・後処理は上の `drain_completions` 以下でそのまま動く。
        if self.accepting_new_work && self.disk_ready {
            self.recover_reviews()?;
        }
        let recover_ms = lap(&mut at);
        // Phase 66b: `ssh` を呼ぶ・ネストしたランタイムを `block_on` しうるので、async ワーカーから逃がす
        // （`run_cluster_hooks_off_async` の説明を参照）。
        run_cluster_hooks_off_async(|| self.refresh_cluster_liveness());
        // ADR-0062 A: `try_wait` は非ブロッキングなので、他のフックと違いスレッドを逃がす必要は無い。
        self.refresh_cluster_master_exits();
        // ADR-0090 D2: クラスタ job の durable wait の poll（ssh は OS スレッドに逃がし、終わった結果だけを拾う）。
        if let Err(e) = self.poll_cluster_job_waits() {
            tracing::warn!(error = %e, "cluster job wait poll failed (ADR-0090)");
        }
        let cluster_ms = lap(&mut at);
        run_cluster_hooks_off_async(|| self.refresh_cluster_tunnels());
        let tunnel_ms = lap(&mut at);
        report.dispatched = if self.accepting_new_work && self.disk_ready {
            self.dispatch_ready()?
        } else {
            0
        };
        let dispatch_ms = lap(&mut at);
        report.in_flight = self.in_flight();
        report.idle = self.is_idle()?;
        let idle_ms = lap(&mut at);
        self.publish_snapshot();
        if started.elapsed() >= SLOW_TICK {
            tracing::warn!(
                total_ms = started.elapsed().as_millis() as u64,
                drain_ms,
                reclaim_ms,
                abort_ms,
                prune_ms,
                recover_ms,
                cluster_ms,
                tunnel_ms,
                dispatch_ms,
                idle_ms,
                "slow tick phases"
            );
        }
        Ok(report)
    }

    fn drain_completions(&mut self) -> Result<(usize, usize), DispatchError> {
        let mut finished = 0;
        let mut reviewed = 0;
        while let Ok(c) = self.rx.try_recv() {
            match c {
                Completion::Worker {
                    task_id,
                    run_id,
                    provider,
                    result,
                } => {
                    // Phase F5-fix2: 完了の確定に失敗しても（DB・作業ツリーの一時的な失敗など）、
                    // 残りの完了の処理は続け、この run は「インフラ都合の失敗」として記録する
                    // （`?` で tick ごと抜けると、受信済みの完了が消えて run が `running` のまま残る）。
                    if let Err(e) =
                        self.on_worker_finished(task_id, run_id.clone(), provider, result)
                    {
                        self.record_finalisation_failure(task_id, &run_id, &e);
                    }
                    finished += 1;
                }
                Completion::Review {
                    task_id,
                    run_id,
                    outcome,
                } => {
                    self.on_review_finished(task_id, run_id, outcome)?;
                    reviewed += 1;
                }
                Completion::WorkUnitChecks {
                    task_id,
                    run_id,
                    account,
                    account_adapter,
                    run_since,
                    provider,
                    result,
                    check_results,
                    checks,
                    check_cwd,
                } => {
                    self.checking.remove(&run_id);
                    if let Err(e) = self.on_work_unit_checks_finished(
                        task_id,
                        run_id.clone(),
                        account,
                        account_adapter,
                        run_since,
                        provider,
                        *result,
                        WorkUnitCheckRun {
                            checks,
                            results: check_results,
                            cwd: check_cwd,
                        },
                    ) {
                        self.record_finalisation_failure(task_id, &run_id, &e);
                    }
                    finished += 1;
                }
                Completion::Integration {
                    task_id,
                    work_unit_id,
                    result,
                } => {
                    self.on_integration_finished(task_id, &work_unit_id, *result)?;
                }
            }
        }
        Ok((finished, reviewed))
    }

    /// 実行中の run と、プロバイダを使っているレビュー run の合計（並列度の分母）。
    /// ADR-0089（Phase R6-5）: CoS の対話 run は数えない（`cos_in_flight` で別に数える）。
    fn workers_in_flight(&self) -> usize {
        self.running.values().filter(|e| !e.cos).count()
            + self
                .reviewing
                .values()
                .filter(|e| e.provider.is_some())
                .count()
    }

    /// ADR-0089（Phase R6-5）: 走っている CoS の対話 run の数。
    fn cos_in_flight(&self) -> usize {
        self.running.values().filter(|e| e.cos).count()
    }

    /// ADR-0089（Phase R6-5）: 並列度の会計（`crate::capacity::RunLoad`）。
    fn run_load(&self) -> crate::capacity::RunLoad {
        crate::capacity::RunLoad {
            workers_in_flight: self.workers_in_flight(),
            max_concurrency: self.config.max_concurrency,
            cos_in_flight: self.cos_in_flight(),
            max_cos_runs: self.config.execution.max_cos_runs,
        }
    }

    /// ADR-0089 規則 1（Phase R6-5）: この task の run が CoS の対話 run か（`crate::capacity::is_cos_run`）。
    /// `max_cos_runs = 0` なら例外を無効にする（常に `false`）。対話でなければ組織を引かない。
    fn is_cos_task(&self, task: &Task) -> Result<bool, DispatchError> {
        if self.config.execution.max_cos_runs == 0 || !task_core::is_conversation(task) {
            return Ok(false);
        }
        Ok(crate::capacity::is_cos_run(task, &self.store.org_list()?))
    }

    /// ADR-0089 規則 2 / 3（Phase R6-5）: `cos` の run にとってそのプロバイダが満杯か。
    fn provider_full(&self, provider: &ProviderId, cos: bool) -> bool {
        crate::capacity::provider_full(
            cos,
            self.account_pool_providers.contains(provider),
            self.provider_in_use(provider),
            self.provider_in_use_cos(provider),
            self.policy.concurrency_limit(provider.clone()),
        )
    }

    /// ADR-0089（Phase R6-5）: そのプロバイダで走っている CoS の対話 run の数（`GET /providers` の `in_use_cos`）。
    fn provider_in_use_cos(&self, provider: &ProviderId) -> usize {
        self.running
            .values()
            .filter(|e| e.cos && &e.provider == provider)
            .count()
    }

    /// そのプロバイダで走っている run の数。ADR-0089（Phase R6-5）: CoS の対話 run は数えない
    /// （`provider_in_use_cos`）。
    fn provider_in_use(&self, provider: &ProviderId) -> usize {
        self.running
            .values()
            .filter(|e| !e.cos && &e.provider == provider)
            .count()
            + self
                .reviewing
                .values()
                .filter(|e| e.provider.as_ref() == Some(provider))
                .count()
    }

    /// ADR-0052 D1: このタスクが「知識整理 run（`langmem` 固定）」で、かつ接続先に届かないなら、
    /// 倒す理由（人が読む 1 行）を返す。それ以外は `None`（＝従来どおり `langmem` で走らせる）。
    ///
    /// 検査は `base_url` ごとに 60 秒キャッシュするので、tick ごとには叩かない。
    fn knowledge_fallback_reason(&mut self, task: &Task, now: Instant) -> Option<String> {
        if task.worker_hint.adapter.as_deref() != Some(task_worker::LangMemAdapter::ID) {
            return None;
        }
        if support_kind(task) != Some("knowledge") {
            return None;
        }
        // `fallback = false`（または `knowledge` ハーネスが無い）なら倒さない。
        self.config.knowledge.fallback_tier?;
        self.knowledge_reachability(now)
            .should_fall_back()
            .map(str::to_string)
    }

    fn dispatch_ready(&mut self) -> Result<usize, DispatchError> {
        self.unroutable.clear();
        self.cluster_waiting.clear();
        // ADR-0089（Phase R6-5）: 非 CoS の枠が埋まっていても、CoS の対話 run の枠が空いていれば走査する。
        if !self.run_load().any_slot() {
            return Ok(0);
        }
        // 上位から見て見送りが続いても後続を試せるよう、窓は広めに取る。
        let window = self.ready_window();
        let ready_started = Instant::now();
        let candidates = self.store.ready_tasks(window)?;
        log_slow_step("ready_tasks", ready_started);
        let now = self.monotonic_now();
        let mut dispatched = 0;
        // この tick で並列度の上限に達していると分かったプロバイダ（tick 内では空きが増えないので共有する）。
        let mut full: std::collections::HashSet<ProviderId> = std::collections::HashSet::new();
        for task in candidates {
            let load = self.run_load();
            if !load.any_slot() {
                break;
            }
            // ADR-0089: 非 CoS の枠が無いときは対話用タスク（CoS の候補）だけを見る（判定は dispatch_one）。
            if !load.admits(false) && !task_core::is_conversation(&task) {
                continue;
            }
            if self.dispatch_one(task, None, &mut full, now)? {
                dispatched += 1;
            }
        }
        // ADR-0074 D1.3 3.（Phase F2b）: 公平性。Ready の Task の 1 本目を先に起こし、残りの枠を
        // 「Running で、現在の工程に runnable な WU を持つ Task」の 2 本目以降に回す（作成順）。
        dispatched += self.dispatch_parallel_work_units(&mut full, now)?;
        Ok(dispatched)
    }

    fn is_idle(&self) -> Result<bool, DispatchError> {
        if !self.running.is_empty() || !self.reviewing.is_empty() {
            return Ok(false);
        }
        if !self.store.list(Some(Status::Running))?.is_empty() {
            return Ok(false);
        }
        // ADR-0010 D8: 人間の承認待ちで延期中の reviewing は、人間が操作しない限り進まないので idle とみなす。
        if self.store.list(Some(Status::Reviewing))?.iter().any(|t| {
            !self.awaiting_human.contains(&t.id) && !self.awaiting_children.contains_key(&t.id)
        }) {
            return Ok(false);
        }
        // ADR-0012 D2（P-33）: 設定に合うプロバイダが無い ready タスクは、設定を直さない限り進まないので待ち対象から外す。
        // 窓いっぱいに返ってきた場合は窓の外に実行可能なタスクが残りうるので idle にしない（次 tick で窓が広がる）。
        let window = self.ready_window();
        let ready = self.store.ready_tasks(window)?;
        if ready.len() >= window {
            return Ok(false);
        }
        // ADR-0041 D5: 面倒を見ないタスク（verify の非 `smoke`）は、このインスタンスでは決して進まないので
        // 待ち対象に数えない。
        Ok(ready.iter().all(|t| {
            !self.is_eligible(t)
                || self.unroutable.contains(&t.id)
                || self.cluster_waiting.contains(&t.id)
        }))
    }
}

/// ADR-0016 M4: イベント列に集約遷移（`Transitioned{reason: "aggregate"}`）があるか。以後の run は集約 run。
fn has_aggregate_transition(events: &[(u64, Event)]) -> bool {
    events
        .iter()
        .any(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "aggregate"))
}

/// ADR-0021 D1: イベント列に子の失敗による遷移（`Transitioned{reason: "child_failed"}`）があるか。
/// 以後の run は「子が失敗した後のやり直し」なので、子の結果（`context.children`）を渡す。
fn has_child_failed_transition(events: &[(u64, Event)]) -> bool {
    events
        .iter()
        .any(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == Trigger::ChildFailed.name()))
}

/// task-ops の読み取りエラーをディスパッチャのエラーに写す（検証以外の失敗は来ない想定）。
fn ops_to_store(e: task_ops::OpsError) -> DispatchError {
    match e {
        task_ops::OpsError::Store(inner) => DispatchError::Store(inner),
        other => DispatchError::Store(StoreError::Invalid(other.to_string())),
    }
}

/// Phase F5-fix6: 孤児の回収で残す `WorkerFinished.outcome` の理由（`interrupted: <これ> (run_id=…)`）。
const ORPHAN_WHY: &str = "orphan_takeover: the daemon holding this run is gone";

/// Phase F5-fix6: SIGTERM / SIGINT で止まるデーモンが手元の run を止めたときの理由。
const SHUTDOWN_WHY: &str = "daemon shutdown (SIGTERM/SIGINT)";

/// デーモン再起動後の復旧用: `runs/<run_id>/result.json`（`fake`/`run_subprocess` が書く終端メッセージ）から
/// `done` の内容を復元する。無ければ空（ADR-0007 D5）。
/// Phase F5-fix2（P-F5-3 の result.json の部分）: `runs/<run_id>/result.json`（アダプタが終端を
/// 正規化して書く `WorkerMessage`。P-26 / ADR-0010 D10）に残った終端を `Terminal` に戻す。
/// 無い・読めない・終端でない・供給側の失敗（`provider_failure` 付きの `error`。cooldown の判断に
/// アダプタの文脈が要る）は `None`（呼び出し側は従来どおり lease 切れとして扱う）。
fn terminal_from_run_dir(dir: &std::path::Path, run_id: &str) -> Option<Terminal> {
    let path = dir.join("runs").join(run_id).join("result.json");
    let text = std::fs::read_to_string(path).ok()?;
    // ADR-0090 D1: `{"type":"wait",...}` の行は `result.json` と同じ検証で `Terminal::Waiting` に戻す。
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim())
        && value.get("type").and_then(|t| t.as_str()) == Some("wait")
    {
        let usage = value
            .get("usage")
            .and_then(|u| serde_json::from_value::<task_core::Usage>(u.clone()).ok());
        return task_worker::adapter::wait_terminal(&value, usage);
    }
    match serde_json::from_str::<WorkerMessage>(text.trim()).ok()? {
        WorkerMessage::Done {
            summary,
            evidence,
            usage,
        } => Some(Terminal::Done {
            summary,
            evidence,
            usage,
        }),
        WorkerMessage::Question { text } => Some(Terminal::Question { text }),
        WorkerMessage::Error {
            message,
            retryable,
            provider_failure: None,
        } => Some(Terminal::Error { message, retryable }),
        WorkerMessage::Yielded { checkpoint, usage } => {
            Some(Terminal::Yielded { checkpoint, usage })
        }
        WorkerMessage::BudgetExhausted {
            kind,
            message,
            usage,
        } => Some(Terminal::BudgetExhausted {
            kind,
            message,
            usage,
        }),
        _ => None,
    }
}

fn subject_from_run_dir(dir: &std::path::Path, run_id: &str) -> ReviewSubject {
    let path = dir.join("runs").join(run_id).join("result.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return ReviewSubject::default();
    };
    match serde_json::from_str::<WorkerMessage>(&text) {
        Ok(WorkerMessage::Done {
            summary, evidence, ..
        }) => ReviewSubject { summary, evidence },
        _ => ReviewSubject::default(),
    }
}

#[cfg(test)]
mod tests;
