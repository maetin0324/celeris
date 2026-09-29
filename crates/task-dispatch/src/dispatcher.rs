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
mod child_tasks;
mod cluster;
mod housekeeping;
mod leases;
mod planner_flow;
mod provider_select;
mod quota_book;
mod review_spawn;
mod review_verdict;
mod run_context;
mod snapshot;
mod tree_units;
mod work_units;
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

/// ADR-0074 D2.3（Phase F3 途中確認）: `PhaseReport` を人が読める Markdown にする（決定的。LLM 不使用）。
/// `Event::PhaseReported.report` と同じ内容を `artifacts/phase-reports/<n>-<phase>.md` にも残す。
fn render_phase_report_markdown(report: &task_core::PhaseReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# 途中報告: {} ({})\n\n",
        report.phase_title, report.phase
    ));
    if !report.phases_done.is_empty() {
        out.push_str("## 済んだ工程\n\n");
        for line in &report.phases_done {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    if !report.work_units.is_empty() {
        out.push_str("## この工程の WU\n\n");
        for line in &report.work_units {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    // ADR-0079 D11（Phase R4a）: 子 task の要約。
    if !report.child_units.is_empty() {
        out.push_str("## この段階の子 task\n\n");
        for line in &report.child_units {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    if !report.integration.is_empty() {
        out.push_str("## 統合\n\n");
        for line in &report.integration {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    if !report.diff_stat.is_empty() {
        out.push_str("## 差分\n\n```\n");
        for line in &report.diff_stat {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("```\n\n");
    }
    out.push_str("## 次の工程\n\n");
    match &report.next_phase {
        Some(p) => {
            out.push_str(&format!("- {p}\n"));
            for wu in &report.next_phase_work_units {
                out.push_str(&format!("  - {wu}\n"));
            }
        }
        None => out.push_str("- (この工程が最後。最終レビューへ)\n"),
    }
    out.push('\n');
    out.push_str(&format!("## quota\n\n{}\n\n", report.quota_summary));
    if !report.artifact_paths.is_empty() {
        out.push_str("## 成果物\n\n");
        for p in &report.artifact_paths {
            out.push_str(&format!("- {p}\n"));
        }
    }
    out
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

/// run 途中のイベントをストアに追記するシンク。ワーカーの出力（heartbeat）があればリースを延長する（ADR-0010 D7）。
struct StoreSink {
    store: Arc<dyn TaskStore>,
    task_id: TaskId,
    run_id: String,
    /// 延長後の ttl（`idle_timeout + lease_grace`）。
    lease_ttl: Duration,
    /// 延長の最小間隔（`lease_grace / 2`）。延長後の期限は常にアダプタの無出力タイムアウトより後になる。
    renew_every: Duration,
    last_renew: std::sync::Mutex<Instant>,
    /// ADR-0016 D2: 委譲の検証に使う `[[roles]]` と上限、この run で既に受け入れた件数。
    roles: Vec<RoleSpec>,
    /// ADR-0027 D1: 委譲の分野解決・検証に使う `[[genres]]`。
    genres: Vec<GenreSpec>,
    delegation: DelegationLimits,
    delegated_this_run: std::sync::atomic::AtomicUsize,
    /// ADR-0024 D4 / ADR-0025 D1: このアカウント（プールを使わなければ `None`）と、そのアダプタの観測値を記録する帳簿
    /// （呼び出し側があらかじめアダプタで解決して渡す）。
    account: Option<String>,
    account_book: Option<Arc<StdMutex<AccountBook>>>,
    /// ADR-0054 D1（Phase 67）: この run が継続セッションの対象なら `(node_id, kind, project_id)`。
    /// `session_established` / `session_resume_failed` がこれを使って `node_sessions` を書く。
    /// 継続セッションの対象でない run では `None`（両方 no-op）。
    session_key: Option<(String, task_core::SessionKind, Option<ProjectId>)>,
}

impl StoreSink {
    fn note(&self, msg: String) {
        let ev = Event::worker_progress(self.run_id.clone(), msg);
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record delegation note");
        }
    }

    /// ADR-0016 D2 / M2 / M6: 提案を検証し、通ったものだけ子として挿入する。拒否理由は `WorkerProgress` に残し、run は失敗させない。
    fn delegate_impl(&self, tasks: &[DelegateTask]) -> Result<(), String> {
        let parent = self
            .store
            .get(self.task_id)
            .map_err(|e| format!("store: {e}"))?
            .ok_or_else(|| "task vanished".to_string())?;
        let ours = parent.status == Status::Running
            && parent.lease.as_ref().map(|l| l.worker_run_id.as_str())
                == Some(self.run_id.as_str());
        if !ours {
            return Err("task is no longer running under this run".to_string());
        }
        // ADR-0033 D4 / Phase 28: 対話 run は返事だけをする。委譲は受け付けず、理由を `progress` に残す
        // （実機で秘書が返事の代わりに research-survey へ委譲し、対話タスクが `blocked` に落ちた事故の再発防止）。
        if task_core::is_conversation(&parent) {
            return Err("対話では委譲できない。返事に『次にやりたいこと』として書け".to_string());
        }
        // ADR-0079 D4 (4)（Phase R1b）: 木の節点（計画の unit から作った子 task）は委譲を使わない。
        if parent.tree.is_some() {
            return Err(
                "木の節点（ADR-0079）では委譲できない。子 task は計画の kind task の unit から作る"
                    .to_string(),
            );
        }
        // ADR-0033 D4 / D5 / SPEC §3.1: 部をまたぐ連携は秘書が認める。別の部の課を `assignee` にした提案は
        // **子を作らずに**質問（`approvals` の 1 行になる固定の形）を残し、run の終わりに `Question` 終端へ
        // 回す。既に人が答えていれば（`once` / `standing`）その場で通す。判定は組織図と `approvals` /
        // `standing_rules` の前方一致だけを見る決定的なもので、LLM は使わない（DESIGN 原則 1）。
        // Phase 27（監査 H-2）: **バッチは分ける** — 同じ部宛ての提案はその場で子にする。
        let org = self.store.org_list().map_err(|e| format!("store: {e}"))?;
        // ADR-0069 D1（Phase 114）: 委譲（LLM）が書いた担当は使わない。担当は matching が決めるので、
        // 部をまたぐ認可（下の split）も担当を名指しした提案には起きなくなる。捨てた事実は進行に残す。
        let stripped: Vec<DelegateTask> = tasks
            .iter()
            .map(|t| {
                let mut t = t.clone();
                if let Some(a) = t.assignee.take().filter(|a| !a.trim().is_empty()) {
                    self.note(format!(
                        "delegate: 担当の指定 {a} は使わない（「{}」の担当は celeris が skills と harness から決定的に選ぶ。ADR-0069 D1）",
                        t.title
                    ));
                }
                t
            })
            .collect();
        let tasks: &[DelegateTask] = &stripped;
        let split =
            task_ops::conversation::split_delegation(self.store.as_ref(), &org, &parent, tasks)
                .map_err(|e| format!("authorization: {e}"))?;
        for denied in &split.denied {
            self.note(format!(
                "delegate denied: {} は人が認めなかった（子は作っていない）",
                denied.key()
            ));
        }
        for pending in &split.pending {
            self.store
                .append_event(
                    self.task_id,
                    &Event::QuestionRaised {
                        run_id: self.run_id.clone(),
                        text: pending.question(),
                    },
                )
                .map_err(|e| format!("store: {e}"))?;
        }
        if !split.pending.is_empty() {
            self.note(format!(
                "delegate deferred: {} 件は秘書の認可待ち（部をまたぐ委譲。子は作っていない）: {}",
                split.pending.len(),
                split
                    .pending
                    .iter()
                    .map(|c| c.key())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if split.allowed.is_empty() {
            return Ok(());
        }
        let tasks: &[DelegateTask] = &split.allowed;
        let already = self
            .delegated_this_run
            .load(std::sync::atomic::Ordering::SeqCst);
        let outcome = plan_delegation(
            self.store.as_ref(),
            &parent,
            tasks,
            already,
            &self.roles,
            &self.genres,
            &self.delegation,
            OffsetDateTime::now_utc(),
        )
        .map_err(|e| format!("validation: {e}"))?;
        for reason in &outcome.rejected {
            self.note(format!("delegate rejected: {reason}"));
        }
        // ADR-0062 B2（Phase 107）: 継承した Remote が担当の道具不足で Local に落ちたことをログに残す。
        for (child_id, reason) in &outcome.workspace_downgrades {
            tracing::info!(task_id = %self.task_id, child_id = %child_id, %reason, "workspace downgraded to local (ADR-0062 B2)");
        }
        if outcome.accepted.is_empty() {
            return Ok(());
        }
        let n = outcome.accepted.len();
        let ids = self
            .store
            .delegate_children(self.task_id, &self.run_id, outcome.accepted)
            .map_err(|e| format!("insert: {e}"))?;
        self.delegated_this_run
            .fetch_add(n, std::sync::atomic::Ordering::SeqCst);
        let listed: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        // Phase 27（監査 H-2）: 「N 件は作った、M 件は秘書の認可待ち」がワーカーの目にも入るようにする。
        let pending = if split.pending.is_empty() {
            String::new()
        } else {
            format!("（{} 件は秘書の認可待ち）", split.pending.len())
        };
        self.note(format!(
            "delegated {n} child task(s){pending}: {}",
            listed.join(", ")
        ));
        tracing::info!(task_id = %self.task_id, run_id = %self.run_id, children = n, "delegated child tasks inserted");
        Ok(())
    }
}

impl EventSink for StoreSink {
    fn browser_wait_open(
        &self,
        request: &task_core::browser_wait::NewBrowserWait,
    ) -> Result<(), String> {
        self.store
            .browser_wait_open(self.task_id, request, OffsetDateTime::now_utc())
            .map(|_| ())
            .map_err(|e| e.code().into())
    }
    fn browser_waits(&self) -> Result<Vec<task_core::browser_wait::BrowserWait>, String> {
        self.store
            .browser_waits_for_task(self.task_id)
            .map_err(|_| "browser wait store unavailable".into())
    }
    fn browser_approval_consume(
        &self,
        wait: &task_core::browser_wait::BrowserWait,
    ) -> Result<task_core::browser_wait::ConsumedBrowserApproval, String> {
        task_core::browser_wait::consume_credential_approval(
            self.store.as_ref(),
            self.task_id,
            wait,
            OffsetDateTime::now_utc(),
        )
        .map_err(String::from)
    }
    fn browser_updated(&self, browser: &task_core::BrowserRun) {
        if let Err(e) = self.store.append_event(
            self.task_id,
            &Event::BrowserUpdated {
                browser: browser.clone(),
            },
        ) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record browser lifecycle");
        }
    }
    fn progress(&self, msg: &str) {
        let ev = Event::worker_progress(self.run_id.clone(), msg);
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record progress");
        }
    }

    /// ADR-0048 D2（Phase 60a）: 構造化した進行をそのまま `Event::WorkerProgress` に残す
    /// （判断はしない。アダプタが決めた `kind` / `tool` / `summary` / `detail` を写すだけ）。
    fn progress_with(&self, msg: &str, fields: &task_core::ProgressFields) {
        let ev = Event::worker_progress_with(self.run_id.clone(), msg, fields.clone());
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record progress");
        }
    }

    fn artifact(&self, artifact: &ArtifactRef) {
        let ev = Event::ArtifactProduced {
            run_id: self.run_id.clone(),
            artifact: artifact.clone(),
        };
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record artifact");
        }
    }

    /// ADR-0044 D2（Phase 53）: ワーカーの `{"type":"comment"}` は `author_kind = node` で残す。
    /// 人は起こさない（通知は ADR-0037 の 5 種のまま）。状態は変えない。
    fn comment(&self, body: &str) {
        let author = self
            .store
            .get(self.task_id)
            .ok()
            .flatten()
            .and_then(|t| t.assignee.clone());
        match task_ops::comment::post_node_comment(
            self.store.as_ref(),
            self.task_id,
            author,
            Some(self.run_id.clone()),
            body.to_string(),
            OffsetDateTime::now_utc(),
        ) {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(task_id = %self.task_id, run_id = %self.run_id, error = %e, "failed to record the worker comment")
            }
        }
    }

    fn delegate(&self, tasks: &[DelegateTask]) {
        if let Err(reason) = self.delegate_impl(tasks) {
            tracing::warn!(task_id = %self.task_id, run_id = %self.run_id, %reason, "delegate proposal ignored");
            self.note(format!("delegate ignored: {reason}"));
        }
    }

    /// ADR-0070 D5（Phase 116）: `renew_lease` が DB busy/locked で失敗しても、すぐには諦めない。
    /// この呼び出しの中で最大 [`RENEW_LEASE_RETRIES`] 回（[`RENEW_LEASE_RETRY_DELAY`] 間隔）やり直す。
    /// それでも失敗したら WARN のみ（run はこの呼び出しの成否に関わらず続く。DB が一時的に混んでいた
    /// だけで run を止めない）。
    fn heartbeat(&self) {
        let Ok(mut last) = self.last_renew.lock() else {
            return;
        };
        if last.elapsed() < self.renew_every {
            return;
        }
        *last = Instant::now();
        let mut attempt = 0;
        loop {
            match self
                .store
                .renew_lease(self.task_id, &self.run_id, self.lease_ttl)
            {
                Ok(true) => return,
                Ok(false) => {
                    tracing::debug!(task_id = %self.task_id, run_id = %self.run_id, "lease not renewed (no longer running under this run)");
                    return;
                }
                Err(e) if task_core::is_busy_error(&e) && attempt < RENEW_LEASE_RETRIES => {
                    attempt += 1;
                    tracing::debug!(task_id = %self.task_id, run_id = %self.run_id, attempt, "lease renewal hit a busy database; retrying (ADR-0070 D5)");
                    std::thread::sleep(RENEW_LEASE_RETRY_DELAY);
                }
                Err(e) => {
                    tracing::warn!(task_id = %self.task_id, error = %e, "failed to renew lease");
                    return;
                }
            }
        }
    }

    /// ADR-0024 D4: run の途中でも観測値を `AccountBook` に記録する（`source = "run"`）。プールを使わない run では
    /// `account` が `None` なので no-op。
    fn rate_limit(&self, obs: RateLimitObservation) {
        let Some(account) = &self.account else { return };
        let Some(book) = &self.account_book else {
            return;
        };
        let Ok(mut book) = book.lock() else { return };
        book.record_observation(account, obs, ObservationSource::Run);
        if let Err(e) = book.save() {
            tracing::warn!(task_id = %self.task_id, %account, error = %e, "failed to save account book after rate_limit observation");
        }
    }

    /// ADR-0054 D1（Phase 67）: アダプタが run の途中で確定させた id（codex / acp）を `node_sessions` へ
    /// 書く。claude-code は celeris が前もって決めた id をそのまま報告するだけなので、通常は上書きでも
    /// 値は変わらない。`session_key` が無い run（継続セッションの対象でない）では no-op。
    fn session_established(&self, session_id: &str) {
        let Some((node_id, kind, project_id)) = &self.session_key else {
            return;
        };
        if let Err(e) = self
            .store
            .node_session_set_id(node_id, *kind, *project_id, session_id)
        {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record the established session id");
        }
    }

    /// ADR-0054 D1（Phase 67）: resume が拒否されたら、そのセッションを retire する（次の run は新規
    /// セッションになる。ADR-0054 D1「失敗も同じ経路で作り直す」）。`session_key` が無ければ no-op。
    fn session_resume_failed(&self, reason: &str) {
        let Some((node_id, kind, project_id)) = &self.session_key else {
            return;
        };
        match self
            .store
            .node_session_retire(node_id, *kind, *project_id, OffsetDateTime::now_utc())
        {
            Ok(true) => {
                tracing::warn!(task_id = %self.task_id, node_id, %reason, "resume rejected; session retired");
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(task_id = %self.task_id, error = %e, "failed to retire the session after a rejected resume");
            }
        }
    }
}

/// `Reviewer` run のシンク（ADR-0007 D5 6.）。進捗は対象 run の `WorkerProgress` に
/// `reviewer run <review_run_id>: ` を付けて記録し、レビュー run の成果物は記録しない
/// （`artifacts_for_run` が対象 run の成果物だけを返すようにするため）。
struct ReviewerSink {
    store: Arc<dyn TaskStore>,
    task_id: TaskId,
    subject_run_id: String,
    review_run_id: String,
    /// ADR-0024 D4: Reviewer run もプールのアカウントで走ることがあるので、同じ帳簿に観測値を記録する
    /// （呼び出し側があらかじめアダプタで解決して渡す。ADR-0025 D1）。
    account: Option<String>,
    account_book: Option<Arc<StdMutex<AccountBook>>>,
    /// ADR-0054 D1 / Phase 67b 追記: この Reviewer run が部門長の継続セッション（`kind = lead`）の
    /// 対象なら `(department_id, Lead, None)`。`session_established`/`session_resume_failed` がこれを
    /// 使って `node_sessions` を書く。部署の無い（従来の独立）Reviewer run では `None`（両方 no-op）。
    /// Phase 67 の実装では**この配線が抜けていて**、Lead セッションの resume 拒否が一切 retire
    /// されなかった（ADR-0054 D1「失敗も同じ経路で作り直す」が Lead セッションには効いていなかった）。
    session_key: Option<(String, task_core::SessionKind, Option<ProjectId>)>,
}

impl EventSink for ReviewerSink {
    fn progress(&self, msg: &str) {
        let ev = Event::worker_progress(
            self.subject_run_id.clone(),
            format!("reviewer run {}: {msg}", self.review_run_id),
        );
        if let Err(e) = self.store.append_event(self.task_id, &ev) {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record reviewer progress");
        }
    }

    fn artifact(&self, artifact: &ArtifactRef) {
        tracing::debug!(task_id = %self.task_id, review_run_id = %self.review_run_id, name = %artifact.name, "reviewer run artifact ignored");
    }

    fn rate_limit(&self, obs: RateLimitObservation) {
        let Some(account) = &self.account else { return };
        let Some(book) = &self.account_book else {
            return;
        };
        let Ok(mut book) = book.lock() else { return };
        book.record_observation(account, obs, ObservationSource::Run);
        if let Err(e) = book.save() {
            tracing::warn!(task_id = %self.task_id, %account, error = %e, "failed to save account book after reviewer rate_limit observation");
        }
    }

    /// ADR-0054 D1 / Phase 67b 追記: `StoreSink::session_established` と同じ（`node_sessions` の
    /// `session_id` を上書きする）。`session_key` が無ければ no-op。
    fn session_established(&self, session_id: &str) {
        let Some((node_id, kind, project_id)) = &self.session_key else {
            return;
        };
        if let Err(e) = self
            .store
            .node_session_set_id(node_id, *kind, *project_id, session_id)
        {
            tracing::warn!(task_id = %self.task_id, error = %e, "failed to record the established lead session id");
        }
    }

    /// ADR-0054 D1 / Phase 67b 追記: `StoreSink::session_resume_failed` と同じ（resume が拒否されたら
    /// この場で retire し、次の `resolve_node_session` が新しい Lead セッションを作る）。Phase 67 では
    /// この配線が抜けていて、部門長のレビュー run の resume 拒否が retire されずに残り続けた
    /// （本番で ULID の session_id が retire されないまま resume され続けた一因）。`session_key` が
    /// 無ければ no-op。
    fn session_resume_failed(&self, reason: &str) {
        let Some((node_id, kind, project_id)) = &self.session_key else {
            return;
        };
        match self
            .store
            .node_session_retire(node_id, *kind, *project_id, OffsetDateTime::now_utc())
        {
            Ok(true) => {
                tracing::warn!(task_id = %self.task_id, node_id, %reason, "lead session resume rejected; session retired");
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(task_id = %self.task_id, error = %e, "failed to retire the lead session after a rejected resume");
            }
        }
    }
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
                        check_results,
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
    fn workers_in_flight(&self) -> usize {
        self.running.len()
            + self
                .reviewing
                .values()
                .filter(|e| e.provider.is_some())
                .count()
    }

    fn provider_in_use(&self, provider: &ProviderId) -> usize {
        self.running
            .values()
            .filter(|e| &e.provider == provider)
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

    /// ADR-0074 D1.7（Phase F2）: 走らせている spawn の無い `integrate-<phase>`（running）を pending に
    /// 戻す（次の tick で冪等な手順でやり直す）。
    fn reconcile_integration(&self, task_id: TaskId, reason: &str) -> Result<(), DispatchError> {
        for wu in self.store.work_units_for(task_id)? {
            if wu.kind != task_core::WorkUnitKind::Integrate
                || wu.status != task_core::WorkUnitStatus::Running
            {
                continue;
            }
            let mut updated = wu.clone();
            updated.status = task_core::WorkUnitStatus::Pending;
            updated.clear_lease();
            updated.updated_at = rfc3339(OffsetDateTime::now_utc());
            self.store.work_unit_transition(
                task_id,
                updated,
                Event::WorkUnitTransitioned {
                    work_unit_id: wu.id.clone(),
                    key: wu.key.clone(),
                    from: task_core::WorkUnitStatus::Running,
                    to: task_core::WorkUnitStatus::Pending,
                    reason: reason.to_string(),
                    run_id: None,
                },
            )?;
        }
        Ok(())
    }

    fn reconcile_work_unit_run(
        &self,
        task_id: TaskId,
        run_id: &str,
        reason: &str,
    ) -> Result<(), DispatchError> {
        let units = self.store.work_units_for(task_id)?;
        let Some(wu) = units.iter().find(|u| {
            u.status == task_core::WorkUnitStatus::Running
                && u.last_run_id.as_deref() == Some(run_id)
        }) else {
            return Ok(());
        };
        let has_checkpoint = self
            .store
            .runs_for_work_unit(&wu.id)?
            .iter()
            .any(|r| r.checkpoint.is_some());
        let mut updated = wu.clone();
        updated.status = if has_checkpoint {
            task_core::WorkUnitStatus::NeedsContinuation
        } else {
            task_core::WorkUnitStatus::Ready
        };
        updated.clear_lease();
        self.store.work_unit_transition(
            task_id,
            updated.clone(),
            Event::WorkUnitTransitioned {
                work_unit_id: wu.id.clone(),
                key: wu.key.clone(),
                from: task_core::WorkUnitStatus::Running,
                to: updated.status,
                reason: reason.to_string(),
                run_id: Some(run_id.to_string()),
            },
        )?;
        Ok(())
    }

    /// ADR-0074 D1.4（Phase F2b）: 統合の間の Task の lease（工程の保持者）の期限。
    fn integration_ttl(&self) -> Duration {
        self.config.review_timeout.saturating_mul(4) + self.config.lease_grace
    }

    /// ADR-0074 D1.4/D1.7（Phase F2b）: Ready の Task で統合を始める（再起動の照合で pending に戻った
    /// 統合のやり直しなど）。Task の lease を工程の保持者で取ってから [`Self::start_integration`]。
    fn start_integration_from_ready(
        &mut self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
    ) -> Result<(), DispatchError> {
        if self.just_aborted.contains(&task.id) || !self.is_eligible(task) {
            return Ok(());
        }
        let holder = format!(
            "{PHASE_LEASE_PREFIX}{}:{}:{}",
            wu.plan_id,
            wu.phase.as_deref().unwrap_or_default(),
            ulid::Ulid::new()
        );
        if !self
            .store
            .acquire_lease(task.id, &holder, self.integration_ttl())?
        {
            return Ok(());
        }
        self.start_integration(task, &wu.id)
    }

    /// ADR-0079 D6（Phase R1c）: kind task の unit `unit` から作る子 task の worktree の基点（親の先頭の git
    /// リポジトリの sha）。葉の WU と同じ規則（`prepare_work_unit_workspace`）: 同じ段階の依存先があれば
    /// `integration::dependency_base`（依存先が子 task ならその子のブランチの HEAD）、無ければ親の task
    /// ブランチの HEAD（= 段階の基点。親のブランチは段階の途中では動かない）。親が WU の worktree を持たない
    /// （並列 1 に倒した: remote / shared / git でない）なら `None`（子もブランチを持たず、統合は子を merge しない。D5）。
    /// `Err` は子を作れない理由（unit を `failed` にする）。
    fn child_base_commit(
        &self,
        parent: &Task,
        unit: &task_core::WorkUnitRow,
        units: &[task_core::WorkUnitRow],
    ) -> Result<Option<String>, String> {
        let mode = self.parallel_mode(parent).map_err(|e| e.to_string())?;
        if !mode.worktrees || mode.fallback.is_some() {
            return Ok(None);
        }
        let Some(ws) = self.task_workspaces_for(parent) else {
            return Ok(None);
        };
        let Some((repo, task_wt)) = ws
            .repos
            .iter()
            .find_map(|r| r.worktree.as_ref().map(|wt| (r, wt)))
        else {
            return Ok(None);
        };
        // 親の task ブランチ（段階の基点）を先に用意する（段階の unit が子だけのときはまだ無い）。
        task_wt
            .ensure_blocking()
            .map_err(|e| format!("cannot prepare the parent's worktree: {e}"))?;
        let intra_dep = unit.depends_on.iter().find_map(|d| {
            units
                .iter()
                .find(|u| &u.key == d && u.phase.is_some() && u.phase == unit.phase)
        });
        let base = match intra_dep {
            Some(dep) => crate::integration::dependency_base(
                &repo.source,
                &parent.id.to_string(),
                dep,
                &task_wt.branch,
                &self.config.worktree_branch_prefix,
            )?,
            None => crate::integration::rev_parse(
                &repo.source,
                &format!("refs/heads/{}", task_wt.branch),
            )
            .ok_or_else(|| format!("the parent's task branch {} does not exist", task_wt.branch))?,
        };
        Ok(Some(base))
    }

    /// ADR-0079 D6（Phase R1c）: done になった子 task の worktree で、残った変更を決定的に commit する
    /// （WU の完了時と同じ `integration::commit_all`、メッセージ `task/<child_id>: <title>`。変更が無ければ
    /// commit しない）。戻り値は子のブランチと、先頭の git リポジトリのその HEAD。子の worktree が既に無ければ
    /// ブランチの HEAD を元のリポジトリで引く。子がブランチを持たない（shared / remote）なら `None`。
    fn commit_child_branch(&self, child: &Task) -> Option<(String, String)> {
        let ws = self.task_workspaces_for(child)?;
        let mut first: Option<(String, String)> = None;
        for repo in &ws.repos {
            let Some(wt) = &repo.worktree else {
                continue;
            };
            let head = if wt.dir.is_dir() {
                match crate::integration::commit_all(
                    &wt.dir,
                    &format!("task/{}: {}", child.id, child.title),
                ) {
                    Ok((head, _)) => Some(head),
                    Err(e) => {
                        tracing::warn!(child_id = %child.id, repo = %repo.name, error = %e, "could not commit the child task's remaining changes (ADR-0079 D6)");
                        crate::integration::rev_parse(&wt.dir, "HEAD")
                    }
                }
            } else {
                crate::integration::rev_parse(&wt.repo, &format!("refs/heads/{}", wt.branch))
            };
            if let Some(head) = head {
                first.get_or_insert((wt.branch.clone(), head));
            }
        }
        first
    }

    /// ADR-0079 D6（Phase R1c）: 段階 `phase` の統合が済んだ子 task の worktree を消す（ブランチは残す）。
    /// 失敗は警告だけ（取り込みは済んでいる。後片付けで段階を止めない）。
    fn remove_integrated_child_worktrees(
        &self,
        task_id: TaskId,
        units: &[task_core::WorkUnitRow],
        phase: &str,
    ) {
        // ADR-0079 D15（Phase R5b-prep）: 採用（`adopt`）した task の worktree は木が作ったものではないので消さない
        // （採用しても対象の履歴・作業場所は変えない）。
        let adopted: std::collections::BTreeSet<String> = self
            .store
            .execution_plan_active(task_id)
            .ok()
            .flatten()
            .map(|p| {
                p.spec
                    .units
                    .iter()
                    .filter(|u| u.adopt.is_some())
                    .map(|u| u.key.clone())
                    .collect()
            })
            .unwrap_or_default();
        for u in units.iter().filter(|u| {
            u.kind == task_core::WorkUnitKind::Task
                && u.status == task_core::WorkUnitStatus::Done
                && u.phase.as_deref() == Some(phase)
                && !adopted.contains(&u.key)
        }) {
            let Some(child) = u
                .child_task_id
                .as_deref()
                .and_then(|id| id.parse::<TaskId>().ok())
                .and_then(|id| self.store.get(id).ok().flatten())
            else {
                continue;
            };
            let Some(ws) = self.task_workspaces_for(&child) else {
                continue;
            };
            for wt in ws.repos.iter().filter_map(|r| r.worktree.as_ref()) {
                if let Err(e) = crate::integration::remove_wu_worktree(wt) {
                    tracing::warn!(%task_id, work_unit = %u.key, child_id = %child.id, error = %e, "could not remove the child task's worktree after integration (ADR-0079 D6)");
                }
            }
        }
    }

    /// ADR-0074 D1.4 / ADR-0079 D5・D6（Phase R1c）: 段階 `phase` の統合で merge するブランチ（`seq` 順）と、
    /// ブランチが必ずあるはずの子（`(unit key, branch)`。子の worktree を切った = `tree.base_commit` を持つ子）。
    /// - 葉の WU: `celeris-wu/<task>/<key>`（`phase_leaves`。従来どおり）。
    /// - done の kind task の unit: 子 task のブランチ `<prefix><child_id>`（`MergeItem::child_task`。
    ///   葉かどうかに関わらず入れる。子に依存する葉があっても子の commit は 1 度だけ入る）。子が
    ///   `workspace_mode = shared` か remote でブランチを持たなければ（`base_commit` が無い）、どのリポジトリにも
    ///   無くて構わない（D5 の「merge しない」）。
    fn integration_items(
        &self,
        units: &[task_core::WorkUnitRow],
        phase: &str,
    ) -> (Vec<crate::integration::MergeItem>, Vec<(String, String)>) {
        let mut ordered: Vec<(u32, crate::integration::MergeItem)> =
            task_core::phase_leaves(units, phase)
                .into_iter()
                .filter_map(|u| {
                    u.branch.clone().map(|branch| {
                        (
                            u.seq,
                            crate::integration::MergeItem::work_unit(&u.key, branch),
                        )
                    })
                })
                .collect();
        let mut expected = Vec::new();
        for u in units.iter().filter(|u| {
            u.kind == task_core::WorkUnitKind::Task
                && u.status == task_core::WorkUnitStatus::Done
                && u.phase.as_deref() == Some(phase)
        }) {
            let Some(child_id) = u.child_task_id.as_deref() else {
                continue;
            };
            let branch = format!("{}{child_id}", self.config.worktree_branch_prefix);
            let has_branch = child_id
                .parse::<TaskId>()
                .ok()
                .and_then(|id| self.store.get(id).ok().flatten())
                .is_some_and(|child| task_core::tree::child_base_commit(&child).is_some());
            if has_branch {
                expected.push((u.key.clone(), branch.clone()));
            }
            ordered.push((
                u.seq,
                crate::integration::MergeItem::child_task(&u.key, branch),
            ));
        }
        ordered.sort_by_key(|(seq, _)| *seq);
        (
            ordered.into_iter().map(|(_, item)| item).collect(),
            expected,
        )
    }

    /// ADR-0074 D1.4（Phase F2b）: 工程の統合を始める（統合 WU を running にし、Task の lease を延ばし、
    /// 葉の WU のブランチの merge と検査の再実行を spawn する。git の I/O は tick を止めない）。
    /// 並列 1 に倒した Task（WU の worktree が無い）では no-op（すぐに done）。
    fn start_integration(&mut self, task: &Task, integ_id: &str) -> Result<(), DispatchError> {
        let units = self.store.work_units_for(task.id)?;
        let Some(integ) = units.iter().find(|u| u.id == integ_id).cloned() else {
            return Ok(());
        };
        if !matches!(
            integ.status,
            task_core::WorkUnitStatus::Pending | task_core::WorkUnitStatus::Ready
        ) {
            return Ok(());
        }
        let phase = integ.phase.clone().unwrap_or_default();
        let mut running = integ.clone();
        running.status = task_core::WorkUnitStatus::Running;
        running.blocked_reason = None;
        running.updated_at = rfc3339(OffsetDateTime::now_utc());
        self.store.work_unit_transition(
            task.id,
            running,
            Event::WorkUnitTransitioned {
                work_unit_id: integ.id.clone(),
                key: integ.key.clone(),
                from: integ.status,
                to: task_core::WorkUnitStatus::Running,
                reason: "integrate".to_string(),
                run_id: None,
            },
        )?;
        if let Err(e) = self
            .store
            .extend_task_lease(task.id, self.integration_ttl())
        {
            tracing::warn!(task_id = %task.id, error = %e, "could not extend the task lease for the integration");
        }
        let mode = self.parallel_mode(task)?;
        let workspaces = self.task_workspaces_for(task);
        let (true, Some(ws)) = (mode.worktrees, workspaces) else {
            // 並列 1（WU は Task の worktree を共有した）: merge するブランチは無い（D1.2）。
            return self.on_integration_finished(task.id, &integ.id, Ok(IntegrationRun::default()));
        };
        // ADR-0079 D15（Phase R5b-prep）: 段階の unit が採用（adopt）した task だけのとき、この段階では leaf も子も
        // 走っていないので Task の worktree（段階の基点のブランチ）がまだ無い。統合の前に用意する（子の生成の前に
        // 親の worktree を用意する `child_base_commit` と同じ。既にあれば何もしない）。
        for wt in ws.repos.iter().filter_map(|r| r.worktree.as_ref()) {
            if let Err(e) = wt.ensure_blocking() {
                return self.on_integration_finished(
                    task.id,
                    &integ.id,
                    Err(format!(
                        "cannot prepare the task's worktree for the integration: {e}"
                    )),
                );
            }
        }
        // ADR-0079 D5 / D6（Phase R1c）: 葉の WU のブランチに、この段階の done の kind task の unit の
        // 子 task のブランチ `celeris/<child_id>` を足し、`seq` 順に merge する（子に依存する同じ段階の葉は
        // 子の HEAD から切られているので、どちらが先でも子の commit は 1 度だけ入る。既に入っていれば飛ばす）。
        let (items, expected_children) = self.integration_items(&units, &phase);
        let repos: Vec<PathBuf> = ws
            .repos
            .iter()
            .filter(|r| r.is_git())
            .map(|r| r.dir.clone())
            .collect();
        // D1.4 の 4: その工程の WU の checks（重複を除く）と workspace.toml の check。
        let mut checks: Vec<task_core::WorkUnitCheck> = Vec::new();
        for u in units.iter().filter(|u| {
            u.status.is_active()
                && u.kind != task_core::WorkUnitKind::Integrate
                && u.phase.as_deref() == Some(phase.as_str())
        }) {
            for c in &u.spec.checks {
                if !checks.iter().any(|x| x.cmd == c.cmd) {
                    checks.push(c.clone());
                }
            }
        }
        for cmd in self.default_checks(task) {
            if !checks.iter().any(|x| x.cmd == cmd) {
                checks.push(task_core::WorkUnitCheck {
                    cmd,
                    expect_exit: 0,
                });
            }
        }
        let task_dir = ws.task_dir.clone();
        // ADR-0074 F5-fix: 統合 WU の検査は Task の worktree で走るので `<repo-key>`。
        let check_env = self.check_cargo_target_env(task, None);
        let timeout = self.config.review_timeout;
        let tx = self.tx.clone();
        let task_id = task.id;
        let work_unit_id = integ.id.clone();
        let wu_id_for_entry = integ.id.clone();
        let handle = tokio::spawn(async move {
            let phase_for_merge = phase.clone();
            let repos_for_merge = repos.clone();
            let merged = tokio::task::spawn_blocking(move || -> Result<IntegrationRun, String> {
                let mut run = IntegrationRun::default();
                for (i, dir) in repos_for_merge.iter().enumerate() {
                    let out = crate::integration::integrate(dir, &items, &phase_for_merge)?;
                    if i == 0 {
                        run.merged = out.merged.clone();
                        run.head = out.head.clone();
                    } else {
                        // ADR-0079 D6: 子のブランチは子の repos（親の部分集合）にだけある。先頭の
                        // リポジトリに無かった子の merge も `merged` に残す（1 件目の commit）。
                        for m in &out.merged {
                            if !run.merged.iter().any(|x| x.key == m.key) {
                                run.merged.push(m.clone());
                            }
                        }
                    }
                    if out.conflict.is_some() {
                        run.conflict = out.conflict;
                        break;
                    }
                }
                if run.conflict.is_none()
                    && let Some((key, branch)) = expected_children
                        .iter()
                        .find(|(key, _)| !run.merged.iter().any(|m| &m.key == key))
                {
                    return Err(format!(
                        "child task unit {key}: its branch {branch} exists in none of the task's repositories \
                         (ADR-0079 D6)"
                    ));
                }
                Ok(run)
            })
            .await
            .map_err(|e| format!("integration task: {e}"))
            .and_then(|r| r);
            let result = match merged {
                Ok(mut run) if run.conflict.is_none() && !checks.is_empty() => {
                    let ws = match repos.first() {
                        Some(w) if w.is_dir() => {
                            task_worker::LocalWorkspace::new(&task_dir).with_work_dir(w)
                        }
                        _ => task_worker::LocalWorkspace::new(&task_dir),
                    }
                    .with_cargo_env(check_env);
                    let results = crate::review::run_work_unit_checks(&ws, &checks, timeout).await;
                    run.checks = checks
                        .iter()
                        .zip(results)
                        .map(|(c, (pass, reason))| (c.cmd.clone(), pass, reason))
                        .collect();
                    Ok(run)
                }
                other => other,
            };
            let _ = tx.send(Completion::Integration {
                task_id,
                work_unit_id,
                result: Box::new(result),
            });
        });
        self.integrating.insert(
            task.id,
            IntegrationEntry {
                work_unit_id: wu_id_for_entry,
                handle,
            },
        );
        Ok(())
    }

    /// ADR-0074 D1.4（Phase F2b）: 工程の統合の結果。衝突 → merge の repair WU、検査の失敗 → repair
    /// （分類に当たる）か replan（当たらない）、成功 → `PhaseIntegrated` と次の工程。
    fn on_integration_finished(
        &mut self,
        task_id: TaskId,
        work_unit_id: &str,
        result: Result<IntegrationRun, String>,
    ) -> Result<(), DispatchError> {
        if self
            .integrating
            .get(&task_id)
            .is_some_and(|e| e.work_unit_id == work_unit_id)
        {
            self.integrating.remove(&task_id);
        }
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        let units = self.store.work_units_for(task_id)?;
        let Some(integ) = units.iter().find(|u| u.id == work_unit_id).cloned() else {
            return Ok(());
        };
        if task.status != Status::Running || integ.status != task_core::WorkUnitStatus::Running {
            tracing::warn!(%task_id, work_unit = %integ.key, status = ?task.status, "stale integration result discarded");
            return Ok(());
        }
        let run = match result {
            Ok(run) => run,
            Err(msg) => {
                return self.integration_gives_up(
                    &task,
                    &integ,
                    &format!(
                        "phase {} の統合を実行できませんでした: {msg}",
                        integ.phase.as_deref().unwrap_or_default()
                    ),
                );
            }
        };
        if let Some(conflict) = run.conflict.clone() {
            return self.schedule_merge_repair(&task, &integ, &units, &conflict);
        }
        if run.checks.iter().any(|(_, pass, _)| !pass) {
            return self.schedule_integration_check_repair(&task, &integ, &units, &run);
        }
        self.finish_phase_integration(&task, &integ, run)
    }

    /// ADR-0074 D1.4 3.（Phase F2b）: 衝突したら repair WU `merge-<phase>-<key>`（Task の worktree で
    /// 走る、最小の context）を作り、統合 WU をそれに依存させて pending に戻す。repair が done になったら
    /// 統合は続きから再開する（済んだ merge は飛ばす）。工程ごとに 2 回まで、超えたら replan。
    fn schedule_merge_repair(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        units: &[task_core::WorkUnitRow],
        conflict: &crate::integration::Conflict,
    ) -> Result<(), DispatchError> {
        let phase = integ.phase.clone().unwrap_or_default();
        let prefix = format!("merge-{phase}-");
        let merge_repairs = units
            .iter()
            .filter(|u| u.kind == task_core::WorkUnitKind::Repair && u.key.starts_with(&prefix))
            .count();
        if merge_repairs >= MAX_MERGE_REPAIRS_PER_PHASE {
            return self.integration_gives_up(
                task,
                integ,
                &format!(
                    "phase {phase} の統合で WorkUnit {} の merge が衝突し、merge の repair の上限（{MAX_MERGE_REPAIRS_PER_PHASE} 回）に達しました",
                    conflict.key
                ),
            );
        }
        let mut key = format!("{prefix}{}", conflict.key);
        let mut n = 2;
        while units.iter().any(|u| u.key == key) {
            key = format!("{prefix}{}-{n}", conflict.key);
            n += 1;
        }
        let conflicted = units.iter().find(|u| u.key == conflict.key);
        let merged_already: Vec<&task_core::WorkUnitRow> = task_core::phase_leaves(units, &phase)
            .into_iter()
            .filter(|u| u.key != conflict.key)
            .collect();
        let decisions_of = |u: &task_core::WorkUnitRow| -> Vec<String> {
            u.last_run_id
                .as_deref()
                .and_then(|rid| self.store.run_index_get(rid).ok().flatten())
                .and_then(|r| r.checkpoint)
                .map(|cp| {
                    cp.decisions
                        .iter()
                        .map(|d| format!("{}（{}）", d.what, d.why))
                        .collect()
                })
                .unwrap_or_default()
        };
        let diff_stat = self
            .task_workspaces_for(task)
            .and_then(|ws| ws.repos.into_iter().find(|r| r.is_git()))
            .map(|r| crate::integration::diff_stat(&r.dir, "HEAD", &conflict.branch))
            .unwrap_or_default();
        let mut objective = format!(
            "工程 {phase} の統合で、WorkUnit {} のブランチ `{}` を Task のブランチへ merge したところ衝突しました。\n\
             Task の作業ツリーで `git merge --no-ff {}` を実行し、**衝突だけを解消して** commit してください。\
             設計は変えないこと。他の WorkUnit の成果を消さないこと。push はしないこと。\n\n衝突したファイル:\n",
            conflict.key, conflict.branch, conflict.branch
        );
        for file in &conflict.files {
            objective.push_str(&format!("- {file}\n"));
        }
        if let Some(c) = conflicted {
            objective.push_str(&format!("\n{} の目的: {}\n", c.key, c.spec.objective));
            for d in decisions_of(c) {
                objective.push_str(&format!("- 決定: {d}\n"));
            }
        }
        for u in &merged_already {
            objective.push_str(&format!(
                "\n既に入っている {} の目的: {}\n",
                u.key, u.spec.objective
            ));
            for d in decisions_of(u) {
                objective.push_str(&format!("- 決定: {d}\n"));
            }
        }
        if !diff_stat.is_empty() {
            objective.push_str(&format!(
                "\n`git diff --stat HEAD...{}`:\n{diff_stat}\n",
                conflict.branch
            ));
        }
        let (max_turns, max_wall_secs) = task_core::execution::RepairClass::MergeConflict.budget();
        let spec = task_core::WorkUnitSpec {
            key: key.clone(),
            kind: task_core::WorkUnitKind::Repair,
            title: format!(
                "repair (merge_conflict): {} の merge の衝突を解消",
                conflict.key
            ),
            objective,
            depends_on: vec![],
            done_when: vec![format!(
                "`git merge-base --is-ancestor {} HEAD` が成り立つ（衝突を解消して merge 済み）",
                conflict.branch
            )],
            checks: vec![task_core::WorkUnitCheck {
                cmd: format!("git merge-base --is-ancestor {} HEAD", conflict.branch),
                expect_exit: 0,
            }],
            context: task_core::WorkUnitContext {
                paths: conflict.files.clone(),
                ..Default::default()
            },
            harness: None,
            features: None,
            budget: Some(task_core::WorkUnitBudget {
                max_turns: Some(max_turns),
                max_wall_secs: Some(max_wall_secs),
            }),
            outputs: vec![],
            phase: Some(phase.clone()),
        };
        self.add_integration_repair(
            task,
            integ,
            spec,
            task_core::execution::RepairClass::MergeConflict.bucket(),
            "merge_conflict",
        )
    }

    /// ADR-0074 D1.4 4.（Phase F2b）: 統合後の検査の失敗。D16 の分類に当たれば repair WU を Task の
    /// worktree に作り（`max_repairs`・同じ class の上限の内なら）、当たらなければ replan。
    fn schedule_integration_check_repair(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        units: &[task_core::WorkUnitRow],
        run: &IntegrationRun,
    ) -> Result<(), DispatchError> {
        let phase = integ.phase.clone().unwrap_or_default();
        let failing: Vec<task_core::FailedCheck> = run
            .checks
            .iter()
            .filter(|(_, pass, _)| !pass)
            .map(|(cmd, _, reason)| task_core::FailedCheck {
                check: Check::Command {
                    cmd: cmd.clone(),
                    expect_exit: 0,
                },
                reason: reason.clone(),
                repair_hint: None,
            })
            .collect();
        let summary: Vec<String> = failing.iter().map(|f| f.reason.clone()).collect();
        let why = format!(
            "phase {phase} の統合後の検査が失敗しました: {}",
            summary.join("; ")
        );
        let class = match task_core::classify_review_failure(&failing) {
            task_core::RepairDecision::Repairable(c) => c,
            task_core::RepairDecision::Substantive => {
                return self.integration_gives_up(task, integ, &why);
            }
        };
        let repairs: Vec<&task_core::WorkUnitRow> = units
            .iter()
            .filter(|u| u.kind == task_core::WorkUnitKind::Repair)
            .collect();
        let same_class = repairs
            .iter()
            .filter(|u| Self::repair_bucket_of_title(&u.spec.title) == Some(class.bucket()))
            .count();
        if repairs.len() as u32 >= self.config.execution.max_repairs
            || same_class as u32 >= self.config.execution.max_repairs_per_class
        {
            return self.integration_gives_up(task, integ, &why);
        }
        let prefix = format!("repair-{phase}-");
        let n = units.iter().filter(|u| u.key.starts_with(&prefix)).count() + 1;
        let diff_stat = self
            .task_workspaces_for(task)
            .and_then(|ws| ws.repos.into_iter().find(|r| r.is_git()))
            .and_then(|r| {
                let branch = r.branch().map(str::to_string).unwrap_or_default();
                crate::checkpoint::gather_repo_facts(Some(&r.dir), &branch)
                    .0
                    .map(|s| s.diff_stat)
            });
        let objective = task_core::build_repair_objective(
            class,
            &summary,
            &task.title,
            &task.objective,
            diff_stat.as_deref(),
        );
        let (max_turns, max_wall_secs) = class.budget();
        let spec = task_core::WorkUnitSpec {
            key: format!("{prefix}{n}"),
            kind: task_core::WorkUnitKind::Repair,
            title: format!("repair ({}): 工程 {phase} の統合後の検査", class.bucket()),
            objective,
            depends_on: vec![],
            done_when: vec![],
            checks: vec![],
            context: Default::default(),
            harness: None,
            features: None,
            budget: Some(task_core::WorkUnitBudget {
                max_turns: Some(max_turns),
                max_wall_secs: Some(max_wall_secs),
            }),
            outputs: vec![],
            phase: Some(phase.clone()),
        };
        self.add_integration_repair(
            task,
            integ,
            spec,
            class.bucket(),
            "integration_check_failed",
        )
    }

    /// 統合の repair WU を足し（ready）、統合 WU をそれに依存させて pending に戻し、Task を
    /// `Continue{advance}`（Running → Ready）にする（repair の run は通常の dispatch に乗る）。
    fn add_integration_repair(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        spec: task_core::WorkUnitSpec,
        class: &str,
        reason: &str,
    ) -> Result<(), DispatchError> {
        let now = rfc3339(OffsetDateTime::now_utc());
        let key = spec.key.clone();
        let row = task_core::WorkUnitRow::new(
            task_core::new_id(),
            task.id.to_string(),
            integ.plan_id.clone(),
            integ.seq,
            spec,
            task_core::WorkUnitStatus::Ready,
            now.clone(),
        );
        let mut pending = integ.clone();
        pending.status = task_core::WorkUnitStatus::Pending;
        pending.clear_lease();
        pending.updated_at = now;
        if !pending.depends_on.contains(&key) {
            pending.depends_on.push(key.clone());
            pending.spec.depends_on.push(key.clone());
        }
        let events = vec![
            Event::WorkUnitTransitioned {
                work_unit_id: integ.id.clone(),
                key: integ.key.clone(),
                from: task_core::WorkUnitStatus::Running,
                to: task_core::WorkUnitStatus::Pending,
                reason: reason.to_string(),
                run_id: None,
            },
            Event::WorkUnitTransitioned {
                work_unit_id: row.id.clone(),
                key: row.key.clone(),
                from: task_core::WorkUnitStatus::Pending,
                to: task_core::WorkUnitStatus::Ready,
                reason: "integration_repair".to_string(),
                run_id: None,
            },
            Event::RepairScheduled {
                work_unit_id: row.id.clone(),
                key: row.key.clone(),
                class: class.to_string(),
                origin: task_core::execution::RepairOrigin::Integration,
            },
        ];
        self.store
            .work_units_apply(task.id, vec![row], vec![pending], events)?;
        match self.store.apply_transition_with_events(
            task.id,
            Trigger::Continue {
                why: task_core::ContinueWhy::Advance,
            },
            vec![],
        ) {
            Ok(_) | Err(StoreError::InvalidTransition(_)) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// 統合を諦める（merge の repair の上限・分類に当たらない検査の失敗・git の失敗）。統合 WU を
    /// failed にし、replan の余地があれば `Continue{replan}`（replan は統合 WU を pending に戻す）、
    /// 無ければ人に聞く（blocked。統合 WU は blocked(question) にして、回答で再開できるようにする）。
    fn integration_gives_up(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        why: &str,
    ) -> Result<(), DispatchError> {
        let replans_so_far = self
            .store
            .execution_plan_list(task.id)?
            .len()
            .saturating_sub(1) as u32;
        let can_replan = replans_so_far < self.effective_max_replans(task.id)?;
        let mut row = integ.clone();
        row.clear_lease();
        row.updated_at = rfc3339(OffsetDateTime::now_utc());
        if can_replan {
            row.status = task_core::WorkUnitStatus::Failed;
            row.blocked_reason = None;
        } else {
            row.status = task_core::WorkUnitStatus::Blocked;
            row.blocked_reason = Some(task_core::WorkUnitBlockedReason::Question);
        }
        self.store.work_unit_transition(
            task.id,
            row.clone(),
            Event::WorkUnitTransitioned {
                work_unit_id: integ.id.clone(),
                key: integ.key.clone(),
                from: integ.status,
                to: row.status,
                reason: "integration_failed".to_string(),
                run_id: None,
            },
        )?;
        let progress = Event::worker_progress(
            last_run_id(&self.store.events_for(task.id)?).unwrap_or_default(),
            why.to_string(),
        );
        let trigger = if can_replan {
            Trigger::Continue {
                why: task_core::ContinueWhy::Replan,
            }
        } else {
            if let Err(e) = crate::approvals::record_question_approval(
                self.store.as_ref(),
                task,
                why,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(task_id = %task.id, error = %e, "failed to record the approval for the integration failure");
            }
            Trigger::WorkerQuestion
        };
        match self
            .store
            .apply_transition_with_events(task.id, trigger, vec![progress])
        {
            Ok(_) | Err(StoreError::InvalidTransition(_)) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// ADR-0074 D1.4 5.（Phase F2b）: 統合の成功。`PhaseIntegrated` を残し、統合 WU を done にし、
    /// その工程の WU の worktree を消し（ブランチは残す）、次の工程の WU を ready にする。次の工程が
    /// あれば `Continue{advance}`、最後の工程なら `WorkerDone`（→ 最終レビュー）。
    fn finish_phase_integration(
        &mut self,
        task: &Task,
        integ: &task_core::WorkUnitRow,
        run: IntegrationRun,
    ) -> Result<(), DispatchError> {
        let task_id = task.id;
        let phase = integ.phase.clone().unwrap_or_default();
        let mut done = integ.clone();
        done.status = task_core::WorkUnitStatus::Done;
        done.clear_lease();
        done.updated_at = rfc3339(OffsetDateTime::now_utc());
        if !run.head.is_empty() {
            done.integrated_commit = Some(run.head.clone());
        }
        self.store.work_unit_transition(
            task_id,
            done,
            Event::WorkUnitTransitioned {
                work_unit_id: integ.id.clone(),
                key: integ.key.clone(),
                from: task_core::WorkUnitStatus::Running,
                to: task_core::WorkUnitStatus::Done,
                reason: "integrated".to_string(),
                run_id: None,
            },
        )?;
        let units = self.store.work_units_for(task_id)?;
        // D1.2: 統合が済んだ WU の worktree を消す（ブランチは Task の終端まで残す）。
        if let Some(ws) = self.task_workspaces_for(task) {
            for u in units
                .iter()
                .filter(|u| u.phase.as_deref() == Some(phase.as_str()) && u.branch.is_some())
            {
                for repo in ws.repos.iter().filter(|r| r.is_git()) {
                    let lwt = crate::integration::wu_worktree(
                        &ws.task_dir,
                        &task_id.to_string(),
                        &u.key,
                        &repo.name,
                        &repo.source,
                        u.base_commit.as_deref().unwrap_or_default(),
                    );
                    if let Err(e) = crate::integration::remove_wu_worktree(&lwt) {
                        tracing::warn!(%task_id, work_unit = %u.key, error = %e, "could not remove the work unit worktree after integration");
                    }
                }
            }
        }
        // ADR-0079 D6（Phase R1c）: 取り込んだ子 task の worktree を消す（ブランチ `celeris/<child_id>` は
        // root の終端まで残す。監査のため）。
        self.remove_integrated_child_worktrees(task_id, &units, &phase);
        // 次の工程の WU（工程の障壁が外れた）を ready にする。
        for id in task_core::newly_ready(&units) {
            if let Some(u) = units.iter().find(|u| u.id == id) {
                let mut row = u.clone();
                row.status = task_core::WorkUnitStatus::Ready;
                self.store.work_unit_transition(
                    task_id,
                    row,
                    Event::WorkUnitTransitioned {
                        work_unit_id: u.id.clone(),
                        key: u.key.clone(),
                        from: task_core::WorkUnitStatus::Pending,
                        to: task_core::WorkUnitStatus::Ready,
                        reason: "dependency_ready".to_string(),
                        run_id: None,
                    },
                )?;
            }
        }
        let units = self.store.work_units_for(task_id)?;
        let phase_event = Event::PhaseIntegrated {
            phase: phase.clone(),
            work_unit_id: integ.id.clone(),
            merged: run
                .merged
                .iter()
                .map(|m| task_core::PhaseMerged {
                    key: m.key.clone(),
                    commit: m.commit.clone(),
                    skipped: m.skipped,
                })
                .collect(),
            head: run.head.clone(),
            checks: run
                .checks
                .iter()
                .map(|(cmd, pass, summary)| task_core::PhaseCheckResult {
                    cmd: cmd.clone(),
                    pass: *pass,
                    summary: summary.clone(),
                })
                .collect(),
        };
        let all_done = matches!(
            crate::execution_scheduler::settle_phase(&units),
            crate::execution_scheduler::PhaseSettle::AllDone
        );
        // ADR-0074 D2.2/D2.3（Phase F3 途中確認）: 次の工程があり、かつこの工程が停止点として
        // 解決されているなら、`Continue{advance}` の代わりに `PhaseGate` で Blocked にする
        // （F2b からの申し送り: `finish_phase_integration` が `Continue{advance}`/`WorkerDone` を
        // 選ぶ場所に判定を差し込む）。最後の工程（`all_done`）では常に `WorkerDone`（D1.6 の表）。
        let mut extra_events = vec![phase_event];
        let mut trigger = if all_done {
            Trigger::WorkerDone
        } else {
            Trigger::Continue {
                why: task_core::ContinueWhy::Advance,
            }
        };
        if !all_done
            && let Ok(Some(active)) = self.store.execution_plan_active(task_id)
            && let Ok(events) = self.store.events_for(task_id)
        {
            let resolved = events
                .iter()
                .rev()
                .find_map(|(_, e)| match e {
                    Event::PausePointsResolved {
                        plan_id, phases, ..
                    } if *plan_id == active.id => Some(phases.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            if resolved.iter().any(|p| p == &phase) {
                let report = self.build_phase_report(task, &phase, &units, &active, &run, &events);
                if let Some(artifact_event) = self.write_phase_report_artifact(
                    task,
                    &phase,
                    resolved
                        .iter()
                        .position(|p| p == &phase)
                        .map(|i| i + 1)
                        .unwrap_or(1),
                    &report,
                ) {
                    extra_events.push(artifact_event);
                }
                extra_events.push(Event::PhaseReported {
                    phase: phase.clone(),
                    report: Box::new(report),
                });
                trigger = Trigger::PhaseGate {
                    phase: phase.clone(),
                };
            }
        }
        match self
            .store
            .apply_transition_with_events(task_id, trigger, extra_events)
        {
            Ok(outcome) => {
                tracing::info!(%task_id, %phase, next = ?outcome.next, "phase integrated");
                if outcome.next == Status::Reviewing {
                    let subject = ReviewSubject {
                        summary: self.plan_summary(&units),
                        evidence: Vec::new(),
                    };
                    let run_id = units
                        .iter()
                        .filter(|u| u.kind != task_core::WorkUnitKind::Integrate)
                        .filter_map(|u| u.last_run_id.clone())
                        .max()
                        .unwrap_or_default();
                    if !self.spawn_review(task_id, run_id, &subject)? {
                        self.pending_subjects.insert(task_id, subject);
                    }
                }
                Ok(())
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(%task_id, error = %e, "phase integration result could not be applied");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// ADR-0074 D2.3（Phase F3 途中確認）: 停止点の工程の統合の後に、決定的に途中報告を組み立てる
    /// （LLM は使わない。checkpoint・`PhaseIntegrated` の材料・quota の記録済み値を機械的に束ねる）。
    fn build_phase_report(
        &self,
        task: &Task,
        phase: &str,
        units: &[task_core::WorkUnitRow],
        active: &task_core::ExecutionPlanRow,
        run: &IntegrationRun,
        events: &[(u64, Event)],
    ) -> task_core::PhaseReport {
        // ADR-0079（Phase R1b）: /3 の段階は `internal_view` で /2 の工程に写して読む。
        let view = task_core::internal_view(&active.spec);
        let phase_title_of = |key: &str| -> String {
            view.phases
                .iter()
                .find(|p| p.key == key)
                .map(|p| p.title.clone())
                .unwrap_or_else(|| key.to_string())
        };
        let phase_order: Vec<&str> = view.phases.iter().map(|p| p.key.as_str()).collect();
        let current_idx = phase_order.iter().position(|k| *k == phase).unwrap_or(0);

        // 済んだ工程の一覧（現在の工程より前の工程だけ。工程の障壁により、それらは既に統合済み）。
        let mut phases_done = Vec::new();
        for &key in &phase_order[..current_idx] {
            let phase_units: Vec<&task_core::WorkUnitRow> = units
                .iter()
                .filter(|u| {
                    u.phase.as_deref() == Some(key) && u.kind != task_core::WorkUnitKind::Integrate
                })
                .collect();
            let n_wu = phase_units.len();
            let n_run: u32 = phase_units.iter().map(|u| u.runs).sum();
            let start = phase_units
                .iter()
                .filter_map(|u| parse_rfc3339(&u.created_at))
                .min();
            let end = phase_units
                .iter()
                .filter_map(|u| parse_rfc3339(&u.updated_at))
                .max();
            let wall = match (start, end) {
                (Some(s), Some(e)) => {
                    task_core::format_wall_ms((e - s).whole_milliseconds() as i64)
                }
                _ => "0m".to_string(),
            };
            phases_done.push(format!(
                "{}: {n_wu} WU / {n_run} run / {wall}",
                phase_title_of(key)
            ));
        }

        // この工程の WU ごとの要約（最終 checkpoint の completed 上位 5 件・decisions・known_failures）。
        let mut work_units = Vec::new();
        for u in units.iter().filter(|u| {
            u.phase.as_deref() == Some(phase) && u.kind != task_core::WorkUnitKind::Integrate
        }) {
            let mut parts = vec![u.spec.title.clone()];
            if let Some(cp) = task_ops::derive::latest_checkpoint(events, Some(&u.key)) {
                if !cp.completed.is_empty() {
                    let top: Vec<String> = cp.completed.iter().take(5).cloned().collect();
                    parts.push(format!("completed: {}", top.join("; ")));
                }
                if !cp.decisions.is_empty() {
                    let d: Vec<String> = cp
                        .decisions
                        .iter()
                        .map(|d| format!("{} ({})", d.what, d.why))
                        .collect();
                    parts.push(format!("decisions: {}", d.join("; ")));
                }
                if !cp.known_failures.is_empty() {
                    let f: Vec<String> = cp
                        .known_failures
                        .iter()
                        .map(|f| match &f.detail {
                            Some(detail) => format!("{}: {detail}", f.what),
                            None => f.what.clone(),
                        })
                        .collect();
                    parts.push(format!("known_failures: {}", f.join("; ")));
                }
            }
            work_units.push(format!("{}: {}", u.key, parts.join(" / ")));
        }

        // 統合の結果（merge・検査。`run` は今まさに終えた統合そのもの）。
        let mut integration: Vec<String> = run
            .merged
            .iter()
            .map(|m| {
                if m.skipped {
                    format!("{}: already integrated (skipped)", m.key)
                } else {
                    format!("merged {} @ {}", m.key, m.commit)
                }
            })
            .collect();
        for (cmd, pass, summary) in &run.checks {
            integration.push(format!(
                "{cmd}: {} ({summary})",
                if *pass { "pass" } else { "fail" }
            ));
        }

        let diff_stat = self.phase_diff_stat(task, units, &run.head);

        let next_key = phase_order.get(current_idx + 1).copied();
        let next_phase_work_units: Vec<String> = next_key
            .map(|k| {
                active
                    .spec
                    .work_units
                    .iter()
                    .filter(|w| w.phase.as_deref() == Some(k))
                    .map(|w| w.title.clone())
                    .collect()
            })
            .unwrap_or_default();

        let plain_events: Vec<Event> = events.iter().map(|(_, e)| e.clone()).collect();
        let metrics = task_core::summarize_execution_metrics(task, &plain_events);
        let quota_summary = task_core::quota_summary_line(&metrics);

        // 成果物へのリンク（この工程の WU の run が出した `ArtifactProduced`）。
        let phase_wu_ids: std::collections::HashSet<&str> = units
            .iter()
            .filter(|u| {
                u.phase.as_deref() == Some(phase) && u.kind != task_core::WorkUnitKind::Integrate
            })
            .map(|u| u.id.as_str())
            .collect();
        let phase_run_ids: std::collections::HashSet<&str> = events
            .iter()
            .filter_map(|(_, e)| match e {
                Event::WorkUnitTransitioned {
                    work_unit_id,
                    to: task_core::WorkUnitStatus::Running,
                    run_id: Some(rid),
                    ..
                } if phase_wu_ids.contains(work_unit_id.as_str()) => Some(rid.as_str()),
                _ => None,
            })
            .collect();
        let mut artifact_paths: Vec<String> = events
            .iter()
            .filter_map(|(_, e)| match e {
                Event::ArtifactProduced { run_id, artifact }
                    if phase_run_ids.contains(run_id.as_str()) =>
                {
                    Some(artifact.path.clone())
                }
                _ => None,
            })
            .collect();
        artifact_paths.sort();
        artifact_paths.dedup();

        // ADR-0079 D5 / D11（Phase R4a）: この段階の子 task ごとの要約の行（状態・subtree の run と定価・子の報告の見出し）。
        let child_units = task_ops::tree_view::stage_child_summaries(self.store.as_ref(), units, phase)
            .unwrap_or_else(|e| {
                tracing::warn!(task_id = %task.id, %phase, error = %e, "could not summarise the stage's child tasks");
                Vec::new()
            });
        let mut report = task_core::PhaseReport {
            phase: phase.to_string(),
            phase_title: phase_title_of(phase),
            phases_done,
            work_units,
            child_units,
            integration,
            diff_stat,
            next_phase: next_key.map(|k| k.to_string()),
            next_phase_work_units,
            quota_summary,
            artifact_paths,
        };
        task_core::truncate_phase_report(&mut report);
        report
    }

    /// D2.3: `git diff --stat <base>..<head>` の要約（最大 30 行）。base はこの Task の v2 計画で
    /// 最初に走った WU の `base_commit`（工程をまたいだ全体の差分）。git が使えない・base が無い・
    /// コマンドが失敗した場合は空（決定的な組み立ての一部として、失敗を報告に混ぜない）。
    fn phase_diff_stat(
        &self,
        task: &Task,
        units: &[task_core::WorkUnitRow],
        head: &str,
    ) -> Vec<String> {
        if head.is_empty() {
            return Vec::new();
        }
        let Some(ws) = self.task_workspaces_for(task) else {
            return Vec::new();
        };
        let Some(repo) = ws.repos.iter().find(|r| r.is_git()) else {
            return Vec::new();
        };
        let Some(base) = units
            .iter()
            .filter(|u| u.kind != task_core::WorkUnitKind::Integrate)
            .min_by_key(|u| u.seq)
            .and_then(|u| u.base_commit.clone())
        else {
            return Vec::new();
        };
        let output = std::process::Command::new("git")
            .current_dir(&repo.dir)
            .args(["diff", "--stat", &format!("{base}..{head}")])
            .output();
        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
                .lines()
                .take(30)
                .map(|s| s.to_string())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// D2.3: 途中報告を `artifacts/phase-reports/<n>-<phase>.md`（人が読める形。ADR-0067）として書き、
    /// `Event::ArtifactProduced` を返す（書けなければ `None`。工程を止めること自体は諦めない）。
    /// `run_id` は特定のワーカー run に属さない daemon 発の成果物なので、`PhaseIntegrated`/
    /// `WorkUnitCommitted` と同じ「daemon が決定的に作る」ことが分かる合成の値にする。
    fn write_phase_report_artifact(
        &self,
        task: &Task,
        phase: &str,
        n: usize,
        report: &task_core::PhaseReport,
    ) -> Option<Event> {
        let workspace_dir = self.task_dir(task)?;
        let artifacts_dir = self.artifacts_dir(task, &workspace_dir);
        let rel_prefix = task_core::artifacts::artifacts_rel_for(task, &workspace_dir);
        let filename = format!("{n}-{phase}.md");
        let abs_path = artifacts_dir.join("phase-reports").join(&filename);
        if let Some(parent) = abs_path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        std::fs::write(&abs_path, render_phase_report_markdown(report)).ok()?;
        let sha256 = task_worker::artifact::sha256_file(&abs_path).ok()?;
        Some(Event::ArtifactProduced {
            run_id: format!("daemon:phase-gate:{phase}"),
            artifact: ArtifactRef {
                name: filename.clone(),
                path: format!("{rel_prefix}/phase-reports/{filename}"),
                sha256,
                kind: "md".to_string(),
                declared: true,
            },
        })
    }

    fn dispatch_ready(&mut self) -> Result<usize, DispatchError> {
        self.unroutable.clear();
        self.cluster_waiting.clear();
        if self.workers_in_flight() >= self.config.max_concurrency {
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
            if self.workers_in_flight() >= self.config.max_concurrency {
                break;
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

    /// `dispatch_ready` の 1 件分（ADR-0074 D1.3（Phase F2b）で切り出した。並列 WU の 2 本目以降も
    /// ここを通る）。run を起こしたら `Ok(true)`。
    fn dispatch_one(
        &mut self,
        task: Task,
        forced_wu: Option<task_core::WorkUnitRow>,
        full: &mut std::collections::HashSet<ProviderId>,
        now: Instant,
    ) -> Result<bool, DispatchError> {
        // ADR-0074 D1.3（Phase F2b）: 並列 WU の 2 本目以降（Task は既に Running。担当・gate・
        // バックオフは 1 本目で済んでいる）。
        let second_pass = forced_wu.is_some();
        if !second_pass && self.running_for_task(task.id) > 0 {
            return Ok(false);
        }
        // ADR-0044 D2: この tick で打ち切ったばかりの run と同じ worktree に、すぐ次の run を
        // 入れない（孫プロセスが片付く猶予を 1 tick 置く）。
        if self.just_aborted.contains(&task.id) {
            return Ok(false);
        }
        // ADR-0041 D5: verify モードは `genre = "smoke"` の煙試験だけを起こす（他は ready のまま）。
        if !self.is_eligible(&task) {
            return Ok(false);
        }
        // ADR-0046 D5（Phase 59）: 担当が決まっていないタスクは dispatch の前に matching で決める
        // （計画 run の子、人が作ったタスク、Console から作られたタスクが全部ここを通る）。
        let mut task = if second_pass {
            task
        } else {
            match self.assign_if_needed(task)? {
                Some(task) => task,
                // 候補が無くて `blocked` にした（人に聞いた）。この tick では dispatch しない。
                None => return Ok(false),
            }
        };
        // ADR-0072 D13（Phase E3）: Complexity Gate（assign_if_needed の後、decide_lane の前。
        // 最初の dispatch で 1 回だけ判定する）。
        if !second_pass {
            task = self.execution_gate_if_needed(task)?;
            // ADR-0079 D9 / D7（Phase R3a）: `plan_invalid` に atomic と答えた節点は 1 run で走らせる。
            task = self.apply_plan_invalid_atomic(task)?;
        }
        // ADR-0072 D6/D15（Phase E2）: 計画のある Task は、次に走らせる WorkUnit を
        // 決定的な scheduler（`task_core::next_work_unit`）で選ぶ。計画が無ければ従来どおり
        // （`current_wu = None`。プロンプト・遷移は E1 までと 1 バイトも変わらない。(i)）。
        let mut replan_dispatch = false;
        let gate = match forced_wu {
            Some(wu) => WuDispatchGate::RunWorkUnit(Box::new(wu)),
            None => self.wu_dispatch_gate(task.id)?,
        };
        // ADR-0079 D9（Phase R2b）/ D7（Phase R3a）: `needed_before: [self]` の未回答の決定（plan_invalid・run 時の
        // 木の上限・worker の self）を待つ task は run を起こさない。
        if !second_pass && self.decision_self_hold(&task)? {
            return Ok(false);
        }
        // ADR-0079 D3（Phase R2a）: 木の上限（run・トークン・replan）。超えるなら新しい run を起こさず、
        // `kind: limit` の決定の要求を出して（木に 1 件）この節点だけを止める（兄弟の走っている run は続く）。
        if self.tree_run_limit_hold(&task, &gate)? {
            return Ok(false);
        }
        let current_wu = match gate {
            WuDispatchGate::Atomic => None,
            WuDispatchGate::RunWorkUnit(wu) => Some(*wu),
            WuDispatchGate::StartIntegration(wu) => {
                self.start_integration_from_ready(&task, &wu)?;
                return Ok(false);
            }
            // ADR-0074「F5-fix8 実装時の明確化」: 仕事の残っていない計画の最終レビュー（run は起こさない）。
            WuDispatchGate::FinalReview => {
                self.start_final_review_from_ready(&task)?;
                return Ok(false);
            }
            // ADR-0072 D17（Phase E4）: replan の planner run。既存の `is_planner_dispatch` の
            // 配線（budget/lane/role の上書き）をそのまま使うが、`ExecutionPlannerContext.replan`
            // を `true` にする（下）。
            WuDispatchGate::RunPlanner { replan } => {
                replan_dispatch = replan;
                None
            }
            WuDispatchGate::Skip => return Ok(false),
        };
        // ADR-0072 D14（Phase E3）/ D17（Phase E4）: gate が compound と判定し、`gate = "on"` で、
        // まだ計画が無い（`current_wu` が None = atomic 経路）なら、この run は task-local な
        // planner run にする。`replan_dispatch` は既に計画がある Task の replan（`wu_dispatch_gate`
        // が上限まで確認済み）。
        // ADR-0074「Phase F3（途中確認）実装時の逸脱・明確化」: `gate = "shadow"` でも、人が
        // `execution: compound` を明示した Task（`ExecutionGateDecision.source = Human`、
        // `rule_id = human/explicit`）は採用して planner run に進む（F5-1 dogfood で見つかった
        // 不具合の修正）。CoS のヒント（source = Hint）と規則表の判定（source = Policy）は
        // shadow では従来どおり記録だけ（採用しない）。
        let gate_decision = task.routing.as_ref().and_then(|r| r.execution.as_ref());
        let decision_is_compound =
            gate_decision.map(|d| d.mode) == Some(task_core::ExecutionMode::Compound);
        let shadow_human_explicit_compound = self.config.execution.gate
            == task_core::GateMode::Shadow
            && decision_is_compound
            && gate_decision.map(|d| d.source) == Some(task_core::GateSource::Human);
        // ADR-0079 D4 (1)（Phase R2a）: 木の子 task の compound は `[execution] gate` に関わらず採用する
        // （root が compound で分けると決めた木を途中で 1 run に潰さない）。root は従来どおり（U-R5）。
        let tree_child_compound = decision_is_compound && task_core::tree::is_tree_child(&task);
        let is_planner_dispatch = replan_dispatch
            || (current_wu.is_none()
                && decision_is_compound
                && (self.config.execution.gate == task_core::GateMode::On
                    || shadow_human_explicit_compound
                    || tree_child_compound));
        // D18/D14: 上書きする前の Task の予算（WU/planner の既定の計算に使う。ADR-0072 D14）。
        let original_task_budget = task.budget;
        // ADR-0074 D5.3（Phase F1）: planner run は `[execution.planner] tier`（既定 standard）で
        // 走る。人が Task に `tier:frontier` を明示していれば、それが優先される
        // （`TierSource::Human` かつ `worker_hint.tier == Frontier`）。
        let planner_human_frontier = task
            .routing
            .as_ref()
            .is_some_and(|r| r.tier_source == task_core::TierSource::Human)
            && task.worker_hint.tier == task_core::Tier::Frontier;
        if is_planner_dispatch {
            // D14: harness/adapter は `[execution.planner]`、lane は固定（下の `lane_decision` で
            // `TierSource::System`／人の明示なら `TierSource::Human` にする）。
            task.worker_hint.tier = if planner_human_frontier {
                task_core::Tier::Frontier
            } else {
                self.config.execution.planner.tier
            };
            task.worker_hint.adapter = Some(self.config.execution.planner.adapter.clone());
            task.budget.max_turns = self.config.execution.planner.max_turns;
            task.budget.max_wall_secs = self.config.execution.planner.max_wall_secs;
        } else if let Some(wu) = &current_wu {
            // ADR-0072 D18/E2 申し送り（Phase E3）: WU の予算（`WorkUnitSpec.budget`）を実際の
            // run の wall-clock/turn 上限に反映する。書かなければ D18 の既定
            // （`max(task.budget.*, 既定)`）。
            let default_max_turns = task.budget.max_turns.max(30);
            let default_max_wall = task.budget.max_wall_secs.max(1800);
            task.budget.max_turns = wu
                .spec
                .budget
                .and_then(|b| b.max_turns)
                .unwrap_or(default_max_turns);
            task.budget.max_wall_secs = wu
                .spec
                .budget
                .and_then(|b| b.max_wall_secs)
                .unwrap_or(default_max_wall);
        }
        // ADR-0010 D6（P-3）: ready に入った時刻（DB の updated_at）からのバックオフ。
        // 並列 WU の 2 本目以降（Task は Running）は 1 本目で済んでいるので見ない。
        if !second_pass && task.attempts > 0 {
            let delay = retry_backoff(
                self.config.retry_backoff_base,
                self.config.retry_backoff_max,
                task.attempts,
            );
            if self.now_utc() < task.updated_at + delay {
                tracing::debug!(task_id = %task.id, attempts = task.attempts, delay_ms = delay.as_millis() as u64, "retry backoff; not dispatching yet");
                return Ok(false);
            }
        }
        // ADR-0070 D3（Phase 116）: インフラ都合の再試行のバックオフ（`self.infra_backoff`。
        // `task.attempts` に依らない別軸。上のバックオフとは独立にゲートする）。期限を過ぎたら
        // このタスクへのゲートは外す（次に infra 失敗すればまた立て直す）。
        if !second_pass && let Some(until) = self.infra_backoff.get(&task.id).copied() {
            if self.now_utc() < until {
                tracing::debug!(task_id = %task.id, %until, "infra backoff; not dispatching yet");
                return Ok(false);
            }
            self.infra_backoff.remove(&task.id);
        }
        // ADR-0018: リモート実行のタスクは、クラスタの設定・cooldown・並列度・多重接続を先に確かめる。
        // ADR-0062 B1（Phase 107）: `cluster_of` が `None` の理由を分ける。(a) 設定に無いクラスタ
        // → 従来どおり `unroutable`（人が設定を直すまで進まない）。(b) 担当が `cluster:<id>` を
        // 持たない → `blocked` にして人に質問を 1 件作る（設定の問題ではなく担当の問題なので、
        // 「no such cluster in the config」という誤解を招く文言は出さない）。
        let cluster = match self.resolve_cluster(&task) {
            ClusterResolution::Local => None,
            ClusterResolution::Resolved(spec, path, mode) => Some((spec, path, mode)),
            ClusterResolution::NotConfigured => {
                if self.warned_unroutable.insert(task.id) {
                    let cluster_id = match &task.workspace {
                        WorkspaceSpec::Remote { cluster, .. } => cluster.clone(),
                        WorkspaceSpec::Local { .. } => String::new(),
                    };
                    tracing::warn!(task_id = %task.id, cluster = %cluster_id, "no such cluster in the config; task left ready");
                }
                self.unroutable.insert(task.id);
                return Ok(false);
            }
            ClusterResolution::AssigneeLacksTool { cluster } => {
                self.block_task_missing_cluster_tool(&task, &cluster)?;
                return Ok(false);
            }
        };
        if let Some((spec, _, _)) = &cluster {
            if self
                .cluster_cooldown
                .get(&spec.id)
                .is_some_and(|until| *until > now)
            {
                // ADR-0018 D2: 人がログインするまで進まないので、待ち対象には数えない（`--until-idle` を止めない）。
                self.cluster_waiting.insert(task.id);
                return Ok(false);
            }
            if self.cluster_in_use(&spec.id) >= spec.concurrency {
                return Ok(false);
            }
            // この tick の `refresh_cluster_liveness` の結果を使う（1 tick に 1 回だけ `ssh -O check` を呼ぶ）。
            let alive = self
                .cluster_connected
                .get(&spec.id)
                .copied()
                .unwrap_or(false);
            if !alive {
                let spec = spec.clone();
                // ADR-0032 D3: `auth = "publickey"` かつ接続フックがあれば、cooldown にする前に
                // 1 回だけ接続を試みる（cooldown 中はここに来ないので、tick ごとに ssh は湧かない。
                // 同じ tick の別タスクが同じクラスタを指していても、成功時は `cluster_connected` の
                // キャッシュが true になり、失敗時は下で cooldown が立つので、2 本目は走らない）。
                let attempt = self.try_auto_connect_cluster(&spec);
                if let Some(result) = &attempt {
                    self.note_key_auth_attempt(&spec, result.is_ok());
                }
                match attempt {
                    Some(Ok(())) => {
                        self.set_cluster_connected(&spec.id, true, ClusterConnChange::KeyAuth);
                    }
                    Some(Err(detail)) => {
                        self.mark_cluster_unavailable(
                            task.id,
                            &spec,
                            format!("auto-connect failed: {detail}"),
                        )?;
                        self.cluster_waiting.insert(task.id);
                        return Ok(false);
                    }
                    None => {
                        self.mark_cluster_unavailable(
                            task.id,
                            &spec,
                            format!(
                                "no ssh ControlMaster connection to {} (host {})",
                                spec.id, spec.host
                            ),
                        )?;
                        self.cluster_waiting.insert(task.id);
                        return Ok(false);
                    }
                }
            }
        }
        let dir_started = Instant::now();
        // ADR-0041 D1 / ADR-0043 D2: ローカルの作業場所（1 つ以上のリポジトリ）を用意する
        // （`dir` はその親 = `runs/` `artifacts/` の置き場）。
        let worktree = self.task_workspaces_for(&task);
        let dir = match &worktree {
            Some(ws) => ws.task_dir.clone(),
            None => match self.task_dir(&task) {
                Some(d) => d,
                None => {
                    tracing::warn!(task_id = %task.id, "cannot resolve the workspace directory; task left ready");
                    return Ok(false);
                }
            },
        };
        log_slow_step("task_dir", dir_started);
        // ADR-0074 D1.2（Phase F2b）: v2 の WU の run は、WU ごとの worktree
        // （`<task_dir>/wu/<key>/repos/<name>`、ブランチ `celeris-wu/<task_id>/<key>`）で走る。並列 1 に
        // 倒した Task・統合の repair WU は Task の worktree を共有する（`None`）。
        let v2_wu = current_wu.as_ref().filter(|w| w.phase.is_some()).cloned();
        let task_worktree = worktree.clone();
        let mut wu_workspace: Option<WorkUnitWorkspace> = None;
        if let Some(wu) = &v2_wu {
            // ADR-0074「Phase F5-fix7 実装時の明確化」: 一時的な失敗の後のバックオフ中は試さない。
            if let Some(f) = self.wu_prepare_failures.get(&wu.id)
                && self.now_utc() < f.retry_at
            {
                tracing::debug!(task_id = %task.id, work_unit = %wu.key, failures = f.count, retry_at = %f.retry_at, "work unit worktree backoff; not dispatching yet");
                return Ok(false);
            }
            match self.prepare_work_unit_workspace(&task, wu, task_worktree.as_ref()) {
                Ok(prepared) => {
                    self.wu_prepare_failures.remove(&wu.id);
                    wu_workspace = prepared;
                }
                Err(e) => {
                    self.on_work_unit_prepare_failed(&task, wu, &e, second_pass)?;
                    return Ok(false);
                }
            }
        }
        let worktree = match &wu_workspace {
            Some(w) => Some(w.workspaces.clone()),
            None => worktree,
        };
        // ADR-0052 D1 / D2（Phase 64）: 知識整理 run は dispatch の直前に `langmem` の接続先へ
        // `GET /models` を当て、届かなければ tier `cheap` の**汎用**ハーネスへ倒す
        // （`worker_hint.adapter` を外すだけ ＝ ADR-0049 の選び方にそのまま乗る）。LLM は呼ばない。
        let fallback_reason = self.knowledge_fallback_reason(&task, now);
        if let Some(reason) = &fallback_reason
            && let Some(tier) = self.config.knowledge.fallback_tier
        {
            task.worker_hint.adapter = None;
            task.worker_hint.tier = tier;
            task.budget.max_turns = KNOWLEDGE_FALLBACK_MAX_TURNS;
            task.budget.max_wall_secs = KNOWLEDGE_FALLBACK_MAX_WALL_SECS;
            tracing::info!(task_id = %task.id, %reason, ?tier, "knowledge: falling back to a generic harness");
        }
        // ADR-0069 D3 / D6（Phase 114）: `routing` を持つ execute タスクは、lane を決定的な policy
        // （TaskFeatures → 規則表 → 組織の天井）とリトライのエスカレーションで決める。人の明示・
        // System の tier はそのまま（記録だけ）。残量による調整はこの後の `select_tier`（別の層）。
        // ADR-0074 D5.3（Phase F1）: planner run は lane を丸めない固定の `[execution.planner]
        // tier`（既定 standard。E3〜E6 は frontier 固定だった）。人が Task に `tier:frontier` を
        // 明示していれば `TierSource::Human` として記録する。D21: WU の run は WU の view
        // （objective/acceptance/budget/genre/features を差し替えたもの）で lane を決める。
        let lane_decision = if is_planner_dispatch {
            let tier = task.worker_hint.tier;
            let (source, rule_id, reason) = if planner_human_frontier {
                (
                    task_core::TierSource::Human,
                    "planner/human-frontier".to_string(),
                    "human explicitly set tier:frontier on this task; the planner run \
                     inherits it (ADR-0074 D5.3)"
                        .to_string(),
                )
            } else {
                (
                    task_core::TierSource::System,
                    format!(
                        "planner/system-{}",
                        match tier {
                            task_core::Tier::Frontier => "frontier",
                            task_core::Tier::Standard => "standard",
                            task_core::Tier::Cheap => "cheap",
                        }
                    ),
                    "ADR-0074 D5.3: planner run runs at [execution.planner] tier, fixed by \
                     celeris code"
                        .to_string(),
                )
            };
            Some(task_core::LaneDecision {
                lane: tier,
                proposed: tier,
                source,
                rule_id,
                policy_version: task_core::LANE_POLICY_VERSION.to_string(),
                features: task_core::TaskFeatures::infer(&task),
                reasons: vec![reason],
                clamped_by: None,
                hint: None,
                escalation: None,
                shadow: None,
            })
        } else if let Some(wu) = &current_wu {
            self.decide_lane_for_work_unit(&task, wu)?
        } else {
            self.decide_lane(&task)?
        };
        if let Some(decision) = &lane_decision {
            task.worker_hint.tier = decision.lane;
        }
        // ADR-0054 Phase 67c: CoS の対話 run だけ、継続セッションの (adapter, account) に留まれるかを
        // 先に試す（`run_extras` の `is_cos_conversation` と同じ判定を select_provider より前に
        // 軽く行う。継続セッションを見つけてから選ぶのでないと、ADR-0049 ランキングが先に別の
        // アダプタ・アカウントへ倒れてしまう）。
        let sticky_session = self.cos_conversation_session(&task)?;
        let Some((adapter_id, provider_id, selected_account)) = self.select_provider(
            &task.worker_hint,
            now,
            task.id,
            full,
            sticky_session.as_ref(),
        ) else {
            return Ok(false);
        };
        let Some(base_adapter) = self.adapters.get(&provider_id).cloned() else {
            tracing::warn!(task_id = %task.id, provider = %provider_id, adapter = %adapter_id, "no adapter instance for provider");
            return Ok(false);
        };
        // ADR-0024 D2 / ADR-0025 D2: プールで選んだアカウントの env を重ねる。`with_env` が `None` を返すのは
        // アダプタの実装漏れ（設定検証で account_pool は claude-code/codex 限定にしているため通常は起きない）
        // なので、このタスクは今回見送る。
        let adapter = match &selected_account {
            Some((account_adapter, account_id)) => {
                match self.adapter_for_account(&base_adapter, *account_adapter, account_id) {
                    Some(a) => a,
                    None => {
                        tracing::warn!(task_id = %task.id, provider = %provider_id, account_id, "adapter does not support account pools (with_env returned None); skipping this tick");
                        return Ok(false);
                    }
                }
            }
            None => base_adapter,
        };
        let remaining = selected_account.as_ref().and_then(|(kind, id)| {
            let book = self.account_book(*kind)?;
            let book = book.lock().ok()?;
            let observation = book.state(id)?.usage.as_ref()?;
            crate::accounts::measured_remaining(observation, (self.now_unix_fn)())
        });
        let (tier, routing_reason) =
            match task_core::model_routing::select_tier(task.worker_hint.tier, remaining) {
                Ok(decision) => decision,
                Err(_) => return Ok(false), // quota refresh will make this task eligible again
            };
        // A legacy provider has no tier mapping: keep its historical behavior.
        if adapter
            .model_for_tier(task.worker_hint.tier)
            .ok()
            .flatten()
            .is_some()
        {
            task.worker_hint.tier = tier;
        }
        let resolved_model = match adapter.model_for_tier(task.worker_hint.tier) {
            Ok(model) => model,
            Err(reason) => {
                self.store.apply_transition_with_events(
                    task.id,
                    Trigger::Unroutable,
                    vec![Event::worker_progress(
                        "routing",
                        format!("model routing blocked: {reason}"),
                    )],
                )?;
                return Ok(false);
            }
        };
        let account = selected_account.as_ref().map(|(_, id)| id.clone());
        let account_adapter = selected_account.as_ref().map(|(a, _)| *a);

        let run_id = ulid::Ulid::new().to_string();
        let wall = Duration::from_secs(task.budget.max_wall_secs);
        let ttl = wall + self.config.lease_grace;
        let lease_started = Instant::now();
        // ADR-0074 D1.5（Phase F2b）: v2 の WU は、Task の lease を工程の保持者で取り（1 本目だけ）、
        // 続けて WU の lease を取る（Task の lease の期限は WU の lease の最大値まで延びる）。
        let acquired = match &v2_wu {
            Some(wu) => {
                let task_lease = second_pass || {
                    let holder = format!(
                        "{PHASE_LEASE_PREFIX}{}:{}:{}",
                        wu.plan_id,
                        wu.phase.as_deref().unwrap_or_default(),
                        ulid::Ulid::new()
                    );
                    self.store.acquire_lease(task.id, &holder, ttl)?
                };
                task_lease
                    && self.store.acquire_work_unit_lease(
                        task.id,
                        &wu.id,
                        &run_id,
                        ttl,
                        wu_workspace.as_ref().map(|w| w.branch.clone()),
                        wu_workspace.as_ref().map(|w| w.base.clone()),
                    )?
            }
            None => self.store.acquire_lease(task.id, &run_id, ttl)?,
        };
        log_slow_step("acquire_lease", lease_started);
        if !acquired {
            return Ok(false);
        }
        let model = resolved_model
            .or_else(|| self.models.get(&provider_id).cloned())
            .unwrap_or_default();
        let event_started = Instant::now();
        self.store.append_event(
            task.id,
            &Event::WorkerStarted {
                run_id: run_id.clone(),
                adapter: adapter_id.clone(),
                model: model.clone(),
                provider: Some(provider_id.clone()),
                // ADR-0024 D4: `account_pool` のプロバイダで選んだアカウント（プールを使わなければ `None`）。
                account: account.clone(),
                // ADR-0072 D14（Phase E3）: planner run だけ `Some(Planner)`（ワーカー run は
                // 従来どおり `None`）。
                role: if is_planner_dispatch {
                    Some(RunRole::Planner)
                } else {
                    None
                },
                task_role: task.role.clone(),
            },
        )?;
        // ADR-0077 D1 の dispatch での途中目標の `in_progress` は ADR-0079 D13（Phase R5a）で廃止（途中目標は凍結）。
        // ADR-0072 D5（E2b の指摘）: 計画の無い Task（暗黙の WorkUnit）の worker run も `runs`
        // 索引に書く（(g)「全タスクの run について書く」。WU の run は `start_work_unit_run`、
        // planner run はこの少し上で、それぞれ自分で `run_index_start` を呼ぶ）。
        if current_wu.is_none() && !is_planner_dispatch {
            let seq = current_run_seq(&self.store.events_for(task.id)?) + 1;
            if let Err(e) = self.store.run_index_start(task_core::RunRow {
                run_id: run_id.clone(),
                task_id: task.id.to_string(),
                work_unit_id: None,
                role: task_core::RunIndexRole::Worker,
                seq,
                status: task_core::RunIndexStatus::Running,
                adapter: Some(adapter_id.clone()),
                model: Some(model.clone()),
                account: account.clone(),
                session_id: None,
                checkpoint: None,
                usage: None,
                metrics: None,
                started_at: rfc3339(OffsetDateTime::now_utc()),
                finished_at: None,
            }) {
                tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the (implicit work unit) worker run start in the runs index");
            }
        }
        // ADR-0069 D5: この run の routing の監査記録（担当・harness・lane・model・features・規則）。
        if let Some(mut decision) = lane_decision {
            if task_core::model_policy::lane_rank(task.worker_hint.tier)
                < task_core::model_policy::lane_rank(decision.lane)
            {
                decision.reasons.push(format!(
                    "quota layer lowered lane {:?} -> {:?} (budget guard)",
                    decision.lane, task.worker_hint.tier
                ));
            }
            let record = task_core::RoutingRecord {
                org_node: task.assignee.clone(),
                harness: task.genre.clone(),
                resolution: task_core::model_routing::LaneResolution {
                    lane: Some(task.worker_hint.tier),
                    adapter: adapter_id.clone(),
                    provider: Some(provider_id.clone()),
                    account: account.clone(),
                    model_id: model.clone(),
                    // ADR-0069 Phase 118 D1: 監査記録は「設定した」値ではなく「実際に CLI へ
                    // 渡った」値を残す（対応しないアダプタでは `None` になる）。
                    reasoning_effort: adapter
                        .reasoning_effort_for_tier(task.worker_hint.tier)
                        .filter(|_| adapter.supports_reasoning_effort()),
                },
                quota_reason: Some(routing_reason.clone()),
                decision,
                // ADR-0072 D21（Phase E3）: WU の run だけ `work_unit_id` を持つ。
                work_unit_id: current_wu.as_ref().map(|wu| wu.id.clone()),
            };
            self.store.append_event(
                task.id,
                &Event::RoutingDecided {
                    run_id: run_id.clone(),
                    record: Box::new(record),
                },
            )?;
        }
        if matches!(adapter_id.as_str(), "claude-code" | "codex") {
            self.store.append_event(task.id, &Event::worker_progress(&run_id,
                format!("model routing: {routing_reason}; execution tier={:?}; provider={provider_id}", task.worker_hint.tier)))?;
        }
        // ADR-0052 D1: 検査の結果を進行（`status`）として残す（run が始まってから 1 行だけ）。
        let knowledge_fallback = match &fallback_reason {
            Some(reason) => {
                self.store.append_event(
                    task.id,
                    &Event::worker_progress_with(
                        &run_id,
                        format!(
                            "langmem の接続先に届かない（{reason}）。cheap のハーネスに倒す（{adapter_id}）"
                        ),
                        task_core::ProgressFields::of(task_core::ProgressKind::Status),
                    ),
                )?;
                Some(KnowledgeFallbackRun {
                    adapter: adapter_id.clone(),
                    instructions: task_worker::knowledge_fallback_instructions(
                        KNOWLEDGE_CANDIDATES_REL,
                    ),
                    budget: task.budget,
                })
            }
            None => None,
        };
        log_slow_step("append_worker_started", event_started);
        let limits = RunLimits {
            wall_clock: wall,
            idle_timeout: self.config.idle_timeout,
            kill_grace: self.config.kill_grace,
        };
        tracing::info!(task_id = %task.id, %run_id, adapter = %adapter_id, provider = %provider_id, account = account.as_deref(), "dispatching");
        let remote = cluster
            .as_ref()
            .map(|(spec, path, mode)| spec.ssh_settings(path, task.id, *mode));
        // ADR-0043 D3（Phase 56）: ホストか、コンテナか、runtime が無くて `blocked` か。
        let container =
            self.container_decision(&task, worktree.as_ref(), &adapter_id, remote.is_some());
        // Phase 55/56 の合流: コンテナで走らせるなら、止めるための口（runtime の実行ファイルと
        // `--label celeris.task=<task_id>`）を覚えておく（ADR-0044 P55-4 / ADR-0043 P56-7）。
        let container_stop: Option<Arc<dyn task_worker::ContainerStopper>> = match &container {
            ContainerDecision::Container(run) => {
                Some(Arc::new(task_worker::ContainerStop::of(&run.plan)))
            }
            ContainerDecision::Host | ContainerDecision::Unavailable { .. } => None,
        };
        let mut extras =
            self.run_extras(&task, worktree.as_ref(), account.as_deref(), &adapter_id)?;
        // ADR-0056 D3（Phase 79）: mount 名にあったが KB に見つからなかった skill を `status` の
        // 進行イベントで 1 行ずつ報告する（run は落とさない）。
        for name in &extras.missing_skills {
            self.store.append_event(
                task.id,
                &Event::worker_progress_with(
                    &run_id,
                    format!("skill {name} not found"),
                    task_core::ProgressFields::of(task_core::ProgressKind::Status),
                ),
            )?;
        }
        // ADR-0072 D6/D9/D15（Phase E2）: 計画のある Task の WU の run。WU の行を `running` にし
        // （`runs`/`last_run_id` を更新）、`runs` 索引に 1 行作り、prompt に載せる文脈を組み立てる。
        if let Some(wu) = &current_wu
            && let Err(e) = self.start_work_unit_run(
                task.id,
                wu,
                &run_id,
                &adapter_id,
                &model,
                account.as_deref(),
                &mut extras,
                v2_wu.is_some(),
            )
        {
            tracing::warn!(task_id = %task.id, work_unit = %wu.key, error = %e, "failed to record the work unit run start; continuing without work-unit context");
        }
        if let Some(w) = &wu_workspace {
            extras.artifacts_dir_override = Some(w.artifacts_dir.clone());
            // ADR-0074 F5-fix（不具合 1）: WU ごとの `CARGO_TARGET_DIR`。
            if let Some(wu) = &v2_wu {
                extras.cargo_target_work_unit = Some((wu.id.clone(), wu.key.clone()));
            }
        }
        if current_wu.is_some() {
            // ADR-0072 D22（Phase E3）: 計画のある Task の WU の run からは delegate.json を
            // 使えない（部をまたぐ委譲は Task 単位。D21）。
            extras.available_genres = Vec::new();
        }
        // ADR-0079 D7（Phase R3a）: 木の節点の worker の run は `result.json` の `decisions` で決定の要求を出せる。
        extras.decision_requests = !is_planner_dispatch
            && self.config.execution.limits.tree.enabled
            && self.is_tree_node(&task).unwrap_or(false);
        // ADR-0072 D14/D9（Phase E3）: planner run は、Task の担当が属する部署の**lead ノード**
        // （`department_of` が返す department ノードそのもの。ADR-0033 D1 の組織の木では
        // department ノード自身が「その部署の実効 profile」を持つ）の実効 profile で走る。
        // `node_sessions` は resume しない（対話タスクではないので、そもそも継続セッションの
        // 判定に掛からない。D9/D14）。
        if is_planner_dispatch {
            extras.execution_planner = Some(self.execution_planner_context(
                &task,
                original_task_budget,
                replan_dispatch,
            )?);
            // ADR-0072 D14（Phase E4b 項目3）: `[execution.planner].permission_mode`
            // （既定 `"plan"`）を、この run の実際の CLI 引数として `run_worker` に反映させる
            // （`RunContext` には乗せない。プロンプトではなく実行そのものの配線）。
            extras.planner_permission_mode =
                Some(self.config.execution.planner.permission_mode.clone());
            // ADR-0072 D5（Phase E3）: `runs` 索引に planner run の行を作る（WU の
            // `start_work_unit_run` と同じ役目。`role = planner`、`work_unit_id = None`）。
            let planner_seq = self
                .store
                .runs_for_task(task.id)
                .map(|rs| {
                    rs.iter()
                        .filter(|r| r.role == task_core::RunIndexRole::Planner)
                        .count() as u32
                        + 1
                })
                .unwrap_or(1);
            if let Err(e) = self.store.run_index_start(task_core::RunRow {
                run_id: run_id.clone(),
                task_id: task.id.to_string(),
                work_unit_id: None,
                role: task_core::RunIndexRole::Planner,
                seq: planner_seq,
                status: task_core::RunIndexStatus::Running,
                adapter: Some(adapter_id.clone()),
                model: Some(model.clone()),
                account: account.clone(),
                session_id: None,
                checkpoint: None,
                usage: None,
                metrics: None,
                started_at: rfc3339(OffsetDateTime::now_utc()),
                finished_at: None,
            }) {
                tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the planner run start in the runs index");
            }
            if let Ok(org) = self.store.org_list()
                && let Some(dept_id) = task
                    .assignee
                    .as_deref()
                    .and_then(|a| task_core::department_of(&org, a))
                && let Some(dept_node) = org.iter().find(|n| n.id == dept_id)
            {
                let effective = task_core::resolve_profile(&org, &dept_node.id);
                extras.profile = if effective.is_trivial() {
                    None
                } else {
                    Some(effective.with_task(&task))
                };
                extras.node = Some(NodeContext {
                    id: dept_node.id.clone(),
                    name: dept_node.name.clone(),
                    brief: dept_node.brief.clone(),
                });
            }
        }
        // ADR-0052 D2: フォールバックの前置き（LangMem に渡しているのと同じ抽出の指示 + 出力契約）を
        // 役割の指示文として載せる。依頼文（`maintenance_objective`）は `task.objective` のまま。
        if let Some(fallback) = &knowledge_fallback {
            extras.role = Some(RoleContext {
                id: task
                    .role
                    .clone()
                    .unwrap_or_else(|| task_core::BUILTIN_KNOWLEDGE.to_string()),
                instructions: fallback.instructions.clone(),
            });
            extras.knowledge_fallback = knowledge_fallback.clone();
        }
        // ADR-0043 D2: 中止されたときに片付けられるよう、この run で使う作業場所を覚えておく。
        if let Some(ws) = &task_worktree {
            self.task_workspaces.insert(task.id, ws.clone());
        }
        // ADR-0074 D4（Phase F3 quota）: 観測の `before` を記録する。ADR-0076: planner run も同じく
        // 登録する（`on_planner_finished` が `resolve_quota_estimate` で閉じる）。
        self.quota_begin(account.as_deref(), account_adapter, &run_id);
        let handle = self.spawn_worker(
            task.id,
            task.worker_hint.tier,
            run_id.clone(),
            provider_id.clone(),
            account.clone(),
            account_adapter,
            adapter,
            dir,
            limits,
            remote,
            worktree,
            extras,
            container,
        );
        self.running.insert(
            RunKey {
                task: task.id,
                work_unit: v2_wu.as_ref().map(|w| w.id.clone()),
            },
            RunEntry {
                run_id,
                provider: provider_id,
                handle,
                since: OffsetDateTime::now_utc(),
                cluster: cluster.map(|(spec, ..)| spec.id),
                account,
                account_adapter,
                container: container_stop,
            },
        );
        Ok(true)
    }

    /// Phase 38（ADR-0028 追記。実機のレビュー不合格から）: 計画が**ハーネスで動く分野**の担当に
    /// 「自分で決めた名前のファイルを書け」と要求していたら、その `artifact_exists` の条件を落として
    /// `objective` に本当の成果物の名前を注記する（`task_core::plan::fix_harness_artifacts`）。
    /// 壊さず直す（Plan run は失敗させず、`Question` にもしない）。判定は決定的で LLM は呼ばない
    /// （DESIGN 原則 1）。直した事実は `warn` に残す。
    fn fix_plan_for_harness(&self, task: &Task, plan: &mut PlanOutput, org: &[task_core::OrgNode]) {
        for note in task_core::fix_harness_artifacts(
            plan,
            task,
            org,
            &self.config.roles,
            &self.config.genres,
        ) {
            tracing::warn!(task_id = %task.id, "{note}");
        }
        // ADR-0063 D3（Phase 109）: 調査系（literature/web-research）の受け入れ条件に部分達成の
        // 逃げ道が無ければ**警告**（拒否はしない。決定的、LLM は使わない）。
        for note in task_core::warn_missing_partial_ok(
            plan,
            task,
            org,
            &self.config.roles,
            &self.config.genres,
        ) {
            tracing::warn!(task_id = %task.id, "{note}");
        }
        // ADR-0069 D1（Phase 114）: 計画（LLM）が書いた担当は使わない（子の `routing.dropped_assignee`
        // に残り、担当は matching が決める）。捨てた事実をここで 1 行ずつ残す。
        for (index, t) in plan.tasks.iter().enumerate() {
            if let Some(a) = t.assignee.as_deref().filter(|a| !a.trim().is_empty()) {
                tracing::info!(task_id = %task.id, index, dropped_assignee = %a, "plan-supplied assignee ignored; matching decides (ADR-0069 D1)");
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_worker(
        &self,
        task_id: TaskId,
        execution_tier: task_core::Tier,
        run_id: String,
        provider: ProviderId,
        account: Option<String>,
        account_adapter: Option<AccountAdapter>,
        adapter: Arc<dyn WorkerAdapter>,
        dir: PathBuf,
        limits: RunLimits,
        remote: Option<SshSettings>,
        worktree: Option<task_worker::TaskWorkspaces>,
        extras: RunExtras,
        container: ContainerDecision,
    ) -> JoinHandle<()> {
        let store = self.store.clone();
        let tx = self.tx.clone();
        let lease = LeaseRenewal {
            ttl: self.config.idle_timeout + self.config.lease_grace,
            every: self.config.lease_grace / 2,
        };
        let roles = self.config.roles.clone();
        let genres = self.config.genres.clone();
        let delegation = self.config.delegation;
        let account_book = account_adapter.and_then(|a| self.account_book(a));
        // ADR-0066 D1（Phase 110b）: `[workspace] shared_build_cache`（既定 true）。
        // ADR-0075 D3（Phase G1）: `[scratch]` が有効なら scratch pool、無効なら `build_cache_dir`。
        let cargo_target = if !self.config.shared_build_cache {
            CargoTargetPlan::None
        } else if self.scratch_active() {
            CargoTargetPlan::Scratch {
                settings: Box::new(self.config.scratch.clone()),
                candidates: self.scratch.candidates.clone(),
            }
        } else {
            CargoTargetPlan::Legacy(self.config.build_cache_dir.clone())
        };
        tokio::spawn(async move {
            let result = run_worker(
                store,
                adapter,
                task_id,
                execution_tier,
                dir,
                &run_id,
                limits,
                lease,
                remote,
                worktree,
                extras,
                roles,
                genres,
                delegation,
                account,
                account_book,
                container,
                cargo_target,
            )
            .await;
            let _ = tx.send(Completion::Worker {
                task_id,
                run_id,
                provider,
                result,
            });
        })
    }

    /// `ready_tasks` の取得件数。経路なしと分かっているタスク（`warned_unroutable`）の分だけ広げ、それらが窓を埋めて
    /// 後ろの実行可能なタスクが dispatch されない・`is_idle` が誤って真になることを防ぐ（ADR-0012 監査）。
    fn ready_window(&self) -> usize {
        self.config.max_concurrency * 4 + 16 + self.warned_unroutable.len()
    }

    /// ADR-0043 D4: レビュー担当の `Check::Command` の既定になる検査コマンド
    /// （タスクに `acceptance` が明示されていればそれが勝つ。決めるのはここではなく `review.rs` の
    /// 呼び出し側）。**先頭のリポジトリの** `[commands] check` だけを使う。
    /// ADR-0069 D3 / D6（Phase 114）: このタスクの lane を決める（`routing` を持つ execute タスクだけ。
    /// それ以外は `None` で従来どおり `worker_hint.tier`）。担当の実効 profile の天井（`allowed_tiers` /
    /// `budget.max_lane`）で丸め、やり直し（`attempts > 0`）ならイベントの履歴から
    /// `EscalationPolicy` で 1 段まで上げる。LLM は使わない（DESIGN 原則 1）。
    fn decide_lane(&self, task: &Task) -> Result<Option<task_core::LaneDecision>, DispatchError> {
        if task.routing.is_none() || task.kind != TaskKind::Execute {
            return Ok(None);
        }
        let org = self.store.org_list()?;
        let profile = task
            .assignee
            .as_deref()
            .filter(|_| !org.is_empty())
            .map(|a| task_core::profile::resolve(&org, a));
        let ceiling = profile
            .as_ref()
            .map(|p| p.lane_ceiling())
            .unwrap_or_default();
        let Some(mut decision) = task_core::model_policy::decide_for_task(task, &ceiling) else {
            return Ok(None);
        };
        if task.attempts > 0 && decision.source.policy_decides() {
            let events: Vec<Event> = self
                .store
                .events_for(task.id)?
                .into_iter()
                .map(|(_, e)| e)
                .collect();
            let history = task_core::retry_policy::attempt_history(task, &events);
            let policy = task_core::EscalationPolicy::for_task(task, profile.as_ref());
            let next = policy.decide(&history, decision.lane, task_core::BudgetState::Ok);
            if next.lane() != decision.lane {
                decision.reasons.push(format!(
                    "retry lane {:?} -> {:?}",
                    decision.lane,
                    next.lane()
                ));
            }
            decision.lane = next.lane();
            decision.escalation = Some(next.describe());
            tracing::info!(task_id = %task.id, attempts = task.attempts, decision = %next.describe(), "retry lane decided (ADR-0069 D6)");
        }
        Ok(Some(decision))
    }

    /// ADR-0046 D5（Phase 59）: `assignee` が無い `ready` のタスクの担当を**決定的に**決める。
    ///
    /// - 決まったら `Event::Assigned { node, score, reason }` を残して担当を書き戻し、そのタスクを返す。
    /// - 候補が 1 つも無ければ `blocked` にして人に聞き（ADR-0021 の質問経路）、`None` を返す。
    /// - matching の対象でない（担当が居る・ハーネスが無い）タスクはそのまま返す。
    ///
    /// LLM は使わない（DESIGN 原則 1）。
    fn assign_if_needed(&mut self, task: Task) -> Result<Option<Task>, DispatchError> {
        use task_ops::matching::Assignment;
        let org = self.store.org_list()?;
        match task_ops::matching::decide(&org, &task) {
            Assignment::NotApplicable => Ok(Some(task)),
            Assignment::Assigned {
                node,
                score,
                reason,
            } => {
                let mut updated = task.clone();
                updated.assignee = Some(node.clone());
                updated.updated_at = OffsetDateTime::now_utc();
                let event = Event::Assigned {
                    node: node.clone(),
                    score,
                    reason: reason.clone(),
                };
                match self.store.update_task(&updated, event) {
                    Ok(stored) => {
                        tracing::info!(
                            task_id = %task.id, assignee = %node, score, reason = %reason,
                            "matching decided the assignee (ADR-0046 D5)"
                        );
                        Ok(Some(stored))
                    }
                    Err(e) => {
                        tracing::warn!(task_id = %task.id, error = %e, "could not write the matched assignee");
                        Ok(Some(task))
                    }
                }
            }
            Assignment::Unroutable { question } => {
                // Phase 44 と同じ規律: ディスパッチャ由来の質問も `approvals` に残す（そうしないと
                // 認可画面に出ず、Discord にも飛ばない）。
                let now = OffsetDateTime::now_utc();
                if let Err(e) = crate::approvals::record_question_approval(
                    self.store.as_ref(),
                    &task,
                    &question,
                    now,
                ) {
                    tracing::warn!(task_id = %task.id, error = %e, "failed to record the approval for the unroutable question");
                }
                let run_id = format!("matching-{}", task.id);
                let events = vec![Event::QuestionRaised {
                    run_id,
                    text: question,
                }];
                match self
                    .store
                    .apply_transition_with_events(task.id, Trigger::Unroutable, events)
                {
                    Ok(_) => {
                        tracing::info!(task_id = %task.id, "no org node can take this task; asking a human (ADR-0046 D5)")
                    }
                    Err(StoreError::InvalidTransition(e)) => {
                        tracing::warn!(task_id = %task.id, error = %e, "unroutable transition could not be applied");
                    }
                    Err(e) => return Err(e.into()),
                }
                Ok(None)
            }
        }
    }

    /// ADR-0062 B1（Phase 107）: 担当が `cluster:<id>` を持たない remote タスクを `blocked` にし、
    /// `assign_if_needed` の `Assignment::Unroutable` と同じ流儀（`approvals` に 1 件、
    /// `Trigger::Unroutable` で `ready → blocked`）で人に質問する。人が答える（または担当・tools を
    /// 変える）と次の tick で `ready` に戻り、そのとき改めて `cluster_of` が評価し直す。
    fn block_task_missing_cluster_tool(
        &mut self,
        task: &Task,
        cluster: &str,
    ) -> Result<(), DispatchError> {
        let assignee = task.assignee.as_deref().unwrap_or("(unknown)");
        let wanted = format!("{}{cluster}", task_core::CLUSTER_TOOL_PREFIX);
        let org = self.store.org_list().unwrap_or_default();
        let holders: Vec<&str> = org
            .iter()
            .filter(|n| task_core::resolve_profile(&org, &n.id).has_tool(&wanted))
            .map(|n| n.id.as_str())
            .collect();
        let question = format!(
            "担当 `{assignee}` には道具 `{wanted}` が無いため、このタスクは {cluster} で実行できません。\
             組織画面で担当の tools に `{wanted}` を足す{}、または作業場所をローカルに変えてください。",
            if holders.is_empty() {
                "か、担当を変える".to_string()
            } else {
                format!("、担当を `{}` などに変える", holders.join("` / `"))
            }
        );
        let now = OffsetDateTime::now_utc();
        if let Err(e) =
            crate::approvals::record_question_approval(self.store.as_ref(), task, &question, now)
        {
            tracing::warn!(task_id = %task.id, error = %e, "failed to record the approval for the missing-cluster-tool question");
        }
        let run_id = format!("cluster-routing-{}", task.id);
        let events = vec![Event::QuestionRaised {
            run_id,
            text: question,
        }];
        match self
            .store
            .apply_transition_with_events(task.id, Trigger::Unroutable, events)
        {
            Ok(_) => {
                tracing::info!(task_id = %task.id, %assignee, %cluster, "assignee lacks the cluster tool; blocked and asked a human (ADR-0062 B1)");
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(task_id = %task.id, error = %e, "missing-cluster-tool transition could not be applied");
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    fn default_checks(&self, task: &Task) -> Vec<String> {
        let Some(ws) = self.task_workspaces_for(task) else {
            return Vec::new();
        };
        let Some(repo) = ws.repos.first() else {
            return Vec::new();
        };
        let from = if repo.dir.is_dir() {
            &repo.dir
        } else {
            &repo.source
        };
        task_core::workspace_config::load_or_default(from)
            .0
            .commands
            .check
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
