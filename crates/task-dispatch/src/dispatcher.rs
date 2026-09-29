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
mod housekeeping;
mod quota_book;
mod snapshot;

pub use snapshot::SnapshotPublisher;

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

/// ADR-0018: コマンドを実行するクラスタ 1 つ分の設定（`celeris::config::ClusterConfig` の写し。task-dispatch は celeris に依存しない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterSpec {
    pub id: String,
    /// `~/.ssh/config` の `Host` 名。
    pub host: String,
    /// このクラスタで同時に走らせる run の上限。
    pub concurrency: usize,
    pub sync: SyncMode,
    pub delete_on_push: bool,
    pub setup: Vec<String>,
    /// 決定的な順に並べた環境変数。
    pub env: Vec<(String, String)>,
    pub rsync_excludes: Vec<String>,
    /// ADR-0019 D1: `sync = "worktree"` のときの設定。
    pub worktree: task_worker::WorktreeSettings,
    /// ADR-0032 D1: `"manual"`（既定） / `"publickey"` / `"totp"`。`"publickey"` のときだけディスパッチャが
    /// 自動で接続を試みる（D3）。
    pub auth: String,
    /// ADR-0053 D3（Phase 66）: このクラスタの ssh master に張る port forward（Qwen トンネル等）。
    /// 空なら `refresh_cluster_tunnels` は何もしない（従来どおり）。
    pub forwards: Vec<ClusterForwardSpec>,
    /// ADR-0059 D6: 設定ファイルの `work_dir`（`[[clusters]] work_dir`）。DB の上書き
    /// （`cluster_settings`）があればそちらが勝つ（`Dispatcher::effective_work_dir` が決める）。
    pub work_dir: Option<PathBuf>,
    /// ADR-0062 A（Phase 107）: master の argv に足す `-o ServerAliveInterval=<n> -o
    /// ServerAliveCountMax=3 -o TCPKeepAlive=yes`（`0` なら keepalive を付けない）。既定 30 秒。
    /// コマンドラインの `-o` は `~/.ssh/config` より優先されるので、人の設定を変えずに効く。
    pub keepalive_secs: u64,
    /// ADR-0062 A: master 越しの実通信（`ssh -o BatchMode=yes <host> -- true`）による生存確認を
    /// この秒数ごとに行う（`0` で無効）。既定 300 秒。NAT / ファイアウォールの idle timeout で
    /// TCP が黙って死んでも、`-O check`（unix socket を見るだけ）は気づかないので、実通信で確定させる。
    pub liveness_probe_secs: u64,
}

/// ADR-0062 B1（Phase 107）: `resolve_cluster` の結果。`cluster_of`（従来の `Option` 契約）はこれを
/// 畳んだだけだが、`dispatch_ready` は `NotConfigured`（設定にクラスタが無い）と
/// `AssigneeLacksTool`（担当が `cluster:<id>` を持たない）を区別して扱う（前者は従来どおり
/// `unroutable` の警告、後者は `blocked` にして人に聞く）。
enum ClusterResolution {
    /// `WorkspaceSpec::Local` のタスク。
    Local,
    /// `clippy::large_enum_variant`: `ClusterSpec` は大きいので `Box` に入れる。
    Resolved(Box<ClusterSpec>, PathBuf, WorkspaceMode),
    /// `[[clusters]]` にそのクラスタ id が無い。
    NotConfigured,
    /// クラスタは設定にあるが、担当が `cluster:<id>` を持たない（ADR-0062 B1）。
    AssigneeLacksTool { cluster: String },
}

/// ADR-0053 D3: 1 本の port forward（`ssh -O forward -L <listen>:<target> <host>` 相当）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterForwardSpec {
    /// ローカル（celeris が bind する側）。`"127.0.0.1:18000"` の形。
    pub listen: String,
    /// master のホスト側から見た転送先。`"bnode150:18000"` の形。
    pub target: String,
    /// ADR-0053 Phase 85: target（`/v1/models`）の健康 probe をこの秒数より短い間隔では行わない
    /// （`[[clusters.forwards]] probe_interval_secs`。既定 30 秒）。リスナーの有無の確認（軽い TCP
    /// connect）はこの間隔に縛られない — 毎回の `refresh_cluster_tunnels` で見る。
    pub probe_interval_secs: u64,
}

/// ADR-0032 D3: `auth = "publickey"` のクラスタに未接続なら、cooldown にする前にディスパッチャが 1 回だけ
/// 接続を試みるためのフック。引数は `(cluster_id, host)`。同期でブロックしてよい（`control_master_alive_blocking`
/// と同じ扱い）。本番では celeris が `task_worker::cluster_login::start_connect` 相当の実装を挿す。テストでは
/// 偽物を挿す。未設定（`None`）なら自動接続はせず、従来どおり cooldown に落ちる。
///
/// ADR-0053 D3（Phase 66）: `refresh_cluster_tunnels` もこの同じフックを再利用する。「TOTP を要求する前に
/// 鍵認証を試す」ため、`auth` の値に関わらず（`"totp"` のクラスタでも）まず呼ぶ。`"totp"` クラスタでは
/// 通常失敗する（鍵だけでは入れない）が、既にセッションが有効ならそのまま繋がることがある。
pub type ClusterConnector = Arc<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync>;

/// ADR-0053 D3: master 上に forward を用意する（無ければ張る、あれば何もしない。冪等）フック。
/// 引数は `(host, listen, target)`。本番では celeris が `ssh -O forward -L <listen>:<target> <host>`
/// （届かなければ master 上で `ssh -N -L` を張るフォールバック）を挿す。テストは偽物を挿す。
pub type TunnelForwardEnsurer = Arc<dyn Fn(&str, &str, &str) -> Result<(), String> + Send + Sync>;

/// ADR-0053 D3 / Phase 85: forward の**先方（target）**の健康を見るフック。引数は `listen`
/// （`"127.0.0.1:18000"`）。本番では `GET http://<listen>/v1/models` の probe
/// （`task_worker::probe_models`）を挿す。テストは偽物を挿す。**listener（`-O forward` が届いているか）
/// とは別物**: これは `refresh_one_forward` がリスナーの存在を確認した後、かつ `probe_interval_secs` の
/// 間隔でしか呼ばない（Phase 85 のバックオフ。本番で `-O forward` は張れているのに先方の vLLM が
/// 落ちている観測から、tick を毎回 3 秒級の HTTP で遅くしないため）。
///
/// 届かないときは人が読む 1 行の理由（時間切れ・接続拒否・HTTP ステータス）を `Err` で返す。
/// `last_error` にそのまま載る（2026-09-24: 転送先が master のホストから時間切れなのか、先方の
/// vLLM が落ちているのかが `target_unreachable` だけでは見分けられなかった）。
pub type TunnelProbe = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// ADR-0053 Phase 85: forward の**リスナー**（`-O forward`/`ssh -N -L` が実際に手元の `listen` で待ち受けて
/// いるか）を見るフック。軽い確認（`listen` への TCP connect 等。ssh を起こさない）を想定。`None`
/// （フックを挿さない）のときは常に「存在しない」として扱う（＝安全側のデフォルト。`tunnel_forward_ensurer`
/// に(再)確立を試みさせる。Phase 66 までの「probe が失敗したら毎回張り直す」と同じ保守的な挙動）。
///
/// **listener の有無と target の健康は別の観測**（Phase 85 の本旨）: listener が有るのに target が
/// 不健全（先方が落ちている）なら `-O forward` は再発行しない（listener は既に有るので無意味な上、
/// 本番でこれが毎 tick 起きて tick が 6 秒に伸びた。`docs/adr/0053-llm-source-proxy.md`「Phase 85 追記」）。
pub type TunnelListenerProbe = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// ADR-0053 Phase 84b: クラスタの ssh master の多重接続の有無を調べるフック。引数は
/// `(ssh_command, host)`（`control_master_alive_blocking` と同じ形）。既定（`Dispatcher::new`）は本物の
/// `ssh -O check`（`control_master_alive_blocking`）。
///
/// **観測（2026-09-21）**: このフックが無かった頃は `refresh_cluster_liveness` が常に本物の
/// `control_master_alive_blocking` を直接呼んでいたため、`crates/celeris/src/lib.rs` の
/// `a_totp_cluster_with_a_forward_does_not_panic_the_first_tick_phase_66b` が `host = "pegasus"` の
/// クラスタでテストしていたところ、テストを動かすマシン自身が実際に `pegasus` へ ssh ControlMaster を
/// 張っていた（人が別作業で張った）ため、テストの意図（master 未接続 → `cluster_connector` が呼ばれる）
/// に反して「master 生存」と判定され、`cluster_connector` が一度も呼ばれずに落ちた。テストは実機の ssh
/// 状態に依存してはならないので、このフックで差し替え可能にした（テストは常に偽物を挿す。本物の
/// `ssh -O check` を経由するのは本番だけ）。
pub type ClusterLivenessProbe = Arc<dyn Fn(&[String], &str) -> bool + Send + Sync>;

/// ADR-0062 A（Phase 107）: master 越しの実通信で生存を確定させるフック。引数は
/// `(ssh_command, host, timeout)`。本物の実装は `ssh -o BatchMode=yes <host> -- true` を
/// `timeout` で打ち切って呼ぶ（本番では `task_worker::ssh::control_master_command_probe_blocking`）。
/// `-O check` は unix ソケットを見るだけなので、NAT / ファイアウォールの idle timeout で TCP が
/// 黙って死んでいても気づかない。こちらは実際にリモートへコマンドを 1 つ流すので確定できる。
pub type ClusterCommandProbe = Arc<dyn Fn(&[String], &str, Duration) -> bool + Send + Sync>;

/// ADR-0062 A: 実通信 probe の打ち切りに使うタイムアウト（既定 10 秒）。
const LIVENESS_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// ADR-0078 D3-3: 実通信 probe がこの回数だけ続けて失敗したら、接続を「死んだ」と確定する
/// （1〜2 回の失敗〈遅いだけの timeout を含む〉では master に触らず、接続中のまま扱う）。
const LIVENESS_PROBE_FAILURES_TO_LOSE: u32 = 3;

/// ADR-0078 D3-2: `auth = "publickey"` の鍵認証による自動再接続のバックオフの初期値（6 秒）。
const KEY_AUTH_BACKOFF_MIN: Duration = Duration::from_secs(6);
/// ADR-0078 D3-2: 同じく上限（5 分）。
const KEY_AUTH_BACKOFF_MAX: Duration = Duration::from_secs(300);
/// ADR-0078 D4: publickey のクラスタで、鍵認証の再接続がこの回数だけ続けて失敗したら報告する。
const KEY_AUTH_FAILURES_TO_REPORT: u32 = 3;

/// ADR-0078 D3-2: 次の鍵認証の再接続までの間隔（純関数）。初回は `KEY_AUTH_BACKOFF_MIN`、以後は倍々で
/// `KEY_AUTH_BACKOFF_MAX` で頭打ち（6 → 12 → 24 → … → 300 秒）。
fn next_key_auth_backoff(current: Duration) -> Duration {
    if current.is_zero() {
        return KEY_AUTH_BACKOFF_MIN;
    }
    current.saturating_mul(2).min(KEY_AUTH_BACKOFF_MAX)
}

/// ADR-0078 D4/D5: `cluster_connected` を書き換えたきっかけ（遷移の `method` / `cause` を決める）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClusterConnChange {
    /// `ssh -O check`（`refresh_cluster_liveness`）の結果。
    Checked,
    /// 実通信 probe が `LIVENESS_PROBE_FAILURES_TO_LOSE` 回続けて失敗した。
    ProbeFailed,
    /// celeris が保持している master（`Child`）が自分で終了した。
    MasterExited,
    /// 鍵認証の再接続（`cluster_connector`）が成功した。
    KeyAuth,
}

impl ClusterConnChange {
    fn lost_cause(self) -> &'static str {
        match self {
            ClusterConnChange::Checked | ClusterConnChange::KeyAuth => "check_failed",
            ClusterConnChange::ProbeFailed => "probe_failed",
            ClusterConnChange::MasterExited => "master_exited",
        }
    }
}

/// ADR-0078: クラスタ 1 つの接続の帳簿（生存の遷移・probe・鍵認証の再接続・回数）。
#[derive(Debug, Default)]
struct ClusterConnState {
    /// 最後に繋がった時刻（切れたら `None`）。`uptime_secs` に使う。
    connected_since: Option<Instant>,
    /// D3-3: 実通信 probe の連続失敗の回数。
    probe_failures: u32,
    /// D3-3: probe の連続失敗で「死んだ」と確定した。`-O check` が通っても probe が 1 回成功するまで
    /// 接続中に戻さない。
    probe_dead: bool,
    /// D3-4: 別スレッドで走っている実通信 probe の結果の受け口（tick は待たない）。
    probe_inflight: Option<std::sync::mpsc::Receiver<bool>>,
    /// D3-1: totp / manual のクラスタで、この切断について鍵認証の再接続を試したか。
    key_auth_tried: bool,
    /// D3-2: publickey の現在のバックオフと、次に試してよい時刻。
    key_auth_backoff: Duration,
    next_key_auth_at: Option<Instant>,
    /// D4: publickey の鍵認証の再接続が続けて失敗した回数と、この切断で報告したか。
    key_auth_failures: u32,
    unavailable_reported: bool,
    /// GUI 発の接続（`POST /clusters/{id}/connect`）が終わった直後か（次の false→true の `method` を
    /// `borrowed` ではなく `auth` の値にする）。
    gui_connect_finished: bool,
    /// D4: 最後の切断の説明（通知の本文に使う）。
    last_lost_detail: Option<String>,
    /// D5: この daemon の起動以降の回数。
    stats: task_core::ClusterConnectionStats,
}

/// ADR-0062 A: celeris が保持している ssh master の終了を検出するフック。呼ぶたびに、その時点で
/// 新たに終了が確認できた master の一覧を返す（同じ終了を二度返さない契約。本物の実装は
/// `ClusterMasters` から exited のものを取り除きながら集める）。
pub type ClusterMasterWatcher = Arc<dyn Fn() -> Vec<ClusterMasterExit> + Send + Sync>;

/// ADR-0062 A: `ClusterMasterWatcher` が返す 1 件（自分で終了した master）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterMasterExit {
    pub cluster: String,
    pub exit_code: Option<i32>,
    pub stderr_tail: String,
}

/// ADR-0062 A: 明示的な切断を経ずに失われた接続の詳細。`mark_cluster_unavailable` が読み、
/// 報告に足してから `Event::ClusterMasterExited` を 1 回だけ残す（`reported`）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClusterDisconnectInfo {
    exit_code: Option<i32>,
    detail: String,
    reported: bool,
}

/// ADR-0053 D3: トンネル 1 本の状態遷移（Console / cluster API に出す。`take_tunnel_events` で取り出す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelEvent {
    pub cluster: String,
    /// `"login_needed"` のときは空。
    pub listen: String,
    pub kind: TunnelEventKind,
    pub at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelEventKind {
    /// forward が初めて（または前回の観測が無い状態から）繋がった（listener 有り・target 健全）。
    Up,
    /// forward の**リスナーが消えた**（master は生きているが `-O forward` が届かない）。
    Down,
    /// 一度 `Down`/`TargetUnreachable` を観測した forward が繋がり直した（listener 有り・target 健全）。
    Restored,
    /// master が落ち、鍵認証も失敗した（人の TOTP が要る）。
    LoginNeeded,
    /// ADR-0053 Phase 85: listener は有る（`-O forward` は張れている）が、target（先方の vLLM 等）が
    /// `/v1/models` に応答しない。**listener が無いわけではないので re-add はしない**（本番観測: forward は
    /// 張れているのに bnode150 が応答せず、`Down` 扱いで毎 tick 張り直していた不具合の修正）。
    TargetUnreachable,
}

impl TunnelEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TunnelEventKind::Up => "up",
            TunnelEventKind::Down => "down",
            TunnelEventKind::Restored => "restored",
            TunnelEventKind::LoginNeeded => "login_needed",
            TunnelEventKind::TargetUnreachable => "target_unreachable",
        }
    }
}

/// ADR-0053 D3: `tunnel_events` に積む上限（古いものから捨てる。無限に溜め込まない）。
const TUNNEL_EVENTS_CAP: usize = 100;

/// `Dispatcher::tunnel_state` のキー。
fn tunnel_key(cluster: &str, listen: &str) -> String {
    format!("{cluster}\u{0}{listen}")
}

/// ADR-0053 Phase 85: `[[clusters.forwards]] probe_interval_secs` の既定（30 秒）。
pub const DEFAULT_TUNNEL_PROBE_INTERVAL_SECS: u64 = 30;

/// ADR-0053 Phase 85: 1 forward の直近の観測（`Dispatcher::tunnel_state` に積む）。listener（`-O forward`
/// の有無）と target の健康（`/v1/models`）を別々に持つ。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ForwardObservation {
    /// `-O forward`/`ssh -N -L` の手元のリスナーが有るか。
    listener: bool,
    /// listener 越しに target（`/v1/models`）が健全か。listener が無ければ意味を持たない（常に `false`）。
    target_healthy: bool,
    /// 直近の失敗理由（無ければ `None`）。
    last_error: Option<String>,
}

impl ForwardObservation {
    /// 従来の「forward が届く」（`tunnel_reachable`）と同じ意味: listener も target も健全。
    fn up(&self) -> bool {
        self.listener && self.target_healthy
    }

    fn phase(&self) -> ForwardPhase {
        match (self.listener, self.target_healthy) {
            (true, true) => ForwardPhase::Up,
            (true, false) => ForwardPhase::TargetUnreachable,
            (false, _) => ForwardPhase::Down,
        }
    }
}

/// `ForwardObservation` から導いた 3 値（イベント種別の判定用。`Unknown` は「まだ観測が無い」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForwardPhase {
    Up,
    Down,
    TargetUnreachable,
}

/// ADR-0066 D3（Phase 110b）: target probe の連続失敗に対する間隔の上限（10 分）。
/// `probe_interval_secs`（既定 30 秒。forward ごとに設定可）を最小間隔として、失敗するたびに倍にして
/// ここで頭打ちにする。1 回でも成功すれば `probe_interval_secs` に戻る。
pub const TUNNEL_PROBE_MAX_INTERVAL_SECS: u64 = 600;

/// 次に probe するまでの間隔（純粋関数）: 成功したら `min_secs`（forward の `probe_interval_secs`）に
/// 戻す。失敗したら現在の間隔を倍にし、`TUNNEL_PROBE_MAX_INTERVAL_SECS` で頭打ちにする
/// （30 → 60 → 120 → 240 → 480 → 600…）。`current_secs` が `min_secs` を下回っていても `min_secs` を
/// 下限にする（設定変更で `probe_interval_secs` が上がった場合の安全側）。
fn next_probe_interval_secs(current_secs: u64, min_secs: u64, healthy: bool) -> u64 {
    if healthy {
        return min_secs;
    }
    current_secs
        .max(min_secs)
        .saturating_mul(2)
        .min(TUNNEL_PROBE_MAX_INTERVAL_SECS)
}

/// ADR-0066 D3: forward 1 本の target probe の直近の結果（`Dispatcher::tunnel_probe_state` に積む。
/// 専用スレッドが書き、tick は読むだけ）。
#[derive(Debug, Clone)]
struct TargetProbeState {
    healthy: bool,
    /// `healthy == false` のときの理由（[`TunnelProbe`] の `Err`）。
    error: Option<String>,
    checked_at: Instant,
    /// 次に probe するまでの間隔（バックオフ済み）。
    interval_secs: u64,
}

/// ADR-0041 D5: この celeris が**面倒を見てよいタスク**の述語。`None`（既定）は「全部」＝従来どおり。
///
/// 検証（`--mode verify`）の celeris は、ここに「`genre = "smoke"` で、かつアダプタが `fake`」を渡す。
/// dispatch だけでなく、**ストア上の他のタスクの状態を変えうる経路すべて**（期限切れリースの回収、
/// `reviewing` の拾い上げ）で同じ述語を使う。手元で起こした run の後始末はこの述語に関係なく続ける
/// （自分が起こしたものは必ず自分が畳む）。
pub type TaskFilter = Arc<dyn Fn(&Task) -> bool + Send + Sync>;

impl ClusterSpec {
    /// このタスクの写し（ローカル）とリモートのパス（呼び出し側が D6 の `work_dir` で解決済み）から、
    /// ワーカー用の設定を作る。`task_id` は worktree のディレクトリ名とブランチ名に使う（ADR-0019 D2）。
    /// `mode`（ADR-0059 D1、`WorkspaceSpec::Remote.mode`）が `Shared` なら、クラスタの `sync` 設定に
    /// 関わらず**同期も worktree も行わない**（`SyncMode::None`）。`Worktree`（省略時の既定を含む）は
    /// 従来どおりクラスタの `sync` に従う。
    pub fn ssh_settings(
        &self,
        remote_path: &std::path::Path,
        task_id: task_core::TaskId,
        mode: task_core::WorkspaceMode,
    ) -> SshSettings {
        let mut settings = SshSettings::new(self.id.clone(), self.host.clone(), remote_path);
        settings.sync = match mode {
            task_core::WorkspaceMode::Shared => SyncMode::None,
            task_core::WorkspaceMode::Worktree => self.sync,
        };
        settings.delete_on_push = self.delete_on_push;
        settings.setup = self.setup.clone();
        settings.env = self.env.clone();
        settings.rsync_excludes = self.rsync_excludes.clone();
        settings.worktree = self.worktree.clone();
        settings.task_id = task_id.to_string();
        settings
    }
}

/// ADR-0023 D1: クラスタの多重接続を確認する間隔（`ssh -O check`）。tick がこれより長ければ毎 tick になる。
const CLUSTER_LIVENESS_INTERVAL: Duration = Duration::from_secs(5);

/// ADR-0053 D3 / Phase 66b: `refresh_cluster_liveness` / `refresh_cluster_tunnels` は `ssh` を同期に
/// 呼び、`cluster_connector`（ADR-0032 D3）経由でネストした tokio ランタイムを `block_on` することがある。
/// `tick()` は celeris の tick ループの中から**同期のまま**呼ばれ、そのループ自身が（マルチスレッドの）
/// tokio ランタイムの上で動く async タスクなので、ここで直接ブロックすると (a) 他の非同期処理を
/// 数秒単位で止め、(b) ネストしたランタイムの `block_on` が「Cannot start a runtime from within a
/// runtime」で panic する（本番 2026-09-21 の観測、`crates/celeris/src/lib.rs` の `cluster_connector`）。
///
/// `f` を、tokio の文脈を一切持たない本物の OS スレッドへ逃がして実行する。マルチスレッド・ランタイムの
/// 中から呼ばれたときだけ `block_in_place` で包み、スケジューラが他のタスクを別ワーカーへ逃がせるように
/// する（`block_in_place` は `current_thread` ランタイムの中で呼ぶと panic するため、既存の
/// `#[tokio::test]`〈既定は current_thread〉の `tunnel_*` テストや、ランタイムが無い素の同期呼び出しでは
/// 使わない。どちらの場合も `f` は素の OS スレッドで動くので、ネストしたランタイムを作っても安全）。
///
/// Phase 81: `f` の戻り値をそのまま返す（`try_auto_connect_cluster` は `Result<(), String>` を
/// 呼び出し元に返す必要があるため、`refresh_cluster_liveness`/`refresh_cluster_tunnels` の頃の
/// `FnOnce() + Send`〈戻り値 `()`〉から一般化した。既存の 2 呼び出し（`()` を返す）はそのまま通る）。
fn run_cluster_hooks_off_async<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send,
    T: Send,
{
    let on_multi_thread_runtime = tokio::runtime::Handle::try_current()
        .map(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
        .unwrap_or(false);
    let spawn_and_join = move || match std::thread::scope(|scope| scope.spawn(f).join()) {
        Ok(v) => v,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    if on_multi_thread_runtime {
        tokio::task::block_in_place(spawn_and_join)
    } else {
        spawn_and_join()
    }
}

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

/// ADR-0033 D4（Phase 24 / Phase 27）: この run の途中で「部をまたぐ委譲」を止めたときに `StoreSink` が
/// 残した質問（1 件の部またぎにつき 1 件。同じ文面は 1 回だけ）。
fn cross_department_questions_of(events: &[(u64, Event)], run_id: &str) -> Vec<String> {
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

/// ADR-0072 D7（Phase E1）: 「二重の安全網」。構造化した `Terminal::BudgetExhausted` を返さない
/// 古い経路・アダプタのために、`Terminal::Error{retryable:true}` のメッセージを字句判定する
/// （`task_core::is_budget_outcome` と同じ語彙。分類できなければ `None`）。
fn classify_budget_kind_from_text(message: &str) -> Option<task_core::BudgetKind> {
    let m = message.to_lowercase();
    if task_core::looks_like_context_exceeded(&m) {
        Some(task_core::BudgetKind::Context)
    } else if m.contains("max_turns") || m.contains("max turns") || m.contains("turn limit") {
        Some(task_core::BudgetKind::Turns)
    } else if m.contains("wall-clock") || m.contains("wall clock") || m.contains("budget") {
        Some(task_core::BudgetKind::WallClock)
    } else {
        None
    }
}

/// ADR-0072 D9（Phase E1）: `WorkerFinished.outcome` に載せる、人が読む 1 行の終わり方の説明。
fn describe_run_end(end: task_core::RunEnd) -> String {
    match end {
        task_core::RunEnd::Completed => "completed".to_string(),
        task_core::RunEnd::Yielded => "yielded".to_string(),
        task_core::RunEnd::BudgetExhausted { kind } => {
            let k = match kind {
                task_core::BudgetKind::Turns => "turns",
                task_core::BudgetKind::WallClock => "wall_clock",
                task_core::BudgetKind::Context => "context",
            };
            format!("budget_exhausted({k})")
        }
        task_core::RunEnd::Question => "question".to_string(),
        task_core::RunEnd::Failed { retryable } => format!("failed(retryable={retryable})"),
        task_core::RunEnd::HarnessError { class } => format!("harness_error({class:?})"),
        task_core::RunEnd::Cancelled => "cancelled".to_string(),
    }
}

/// Phase F5-fix3: 拒否した planner の計画の移し先（`artifacts/` の中）。
const REJECTED_PLAN_FILE: &str = "execution-plan.rejected.json";
/// Phase F5-fix3: `give_up_or_retry_planner` の進捗・質問の文面（`planner_rejections_since_last_plan` が
/// 同じ定数で理由を取り出す。文面は F5-fix2 までと 1 バイトも変えない）。
// ADR-0079 D10（Phase R3b）: 生存確認（`task_ops::tree::planner_retry_pending`）が同じ文言を読むので task_ops に置く。
const PLANNER_RETRY_PREFIX: &str = task_ops::tree::PLANNER_RETRY_PREFIX;
const PLANNER_RETRY_SUFFIX: &str = task_ops::tree::PLANNER_RETRY_SUFFIX;
const PLANNER_BLOCKED_PREFIX: &str = "計画を直せませんでした（";
const PLANNER_BLOCKED_SUFFIX: &str = "）。予算を増やして続ける／人が計画を書き直す\n（PUT /tasks/{id}/execution-plan）／中止のいずれかを選んでください。";

fn planner_retry_message(reason: &str) -> String {
    format!("{PLANNER_RETRY_PREFIX}{reason}{PLANNER_RETRY_SUFFIX}")
}

fn planner_blocked_question(reason: &str) -> String {
    format!("{PLANNER_BLOCKED_PREFIX}{reason}{PLANNER_BLOCKED_SUFFIX}")
}

/// Phase F5-fix3: 直近の `ExecutionPlanned`（無ければ最初）より後で、daemon が planner の計画を拒否した
/// 理由（`planner_retry_message` / `planner_blocked_question` の進捗から、古い順・重複なし）。次の
/// planner run のプロンプトに「前の計画はこの理由で拒否された」として渡す（純粋関数）。
fn planner_rejections_since_last_plan(events: &[(u64, Event)]) -> Vec<String> {
    let since = events
        .iter()
        .rposition(|(_, e)| matches!(e, Event::ExecutionPlanned { .. }))
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut out: Vec<String> = Vec::new();
    for (_, e) in &events[since..] {
        let Event::WorkerProgress {
            msg, kind: None, ..
        } = e
        else {
            continue;
        };
        let reason = msg
            .strip_prefix(PLANNER_RETRY_PREFIX)
            .and_then(|r| r.strip_suffix(PLANNER_RETRY_SUFFIX))
            .or_else(|| {
                msg.strip_prefix(PLANNER_BLOCKED_PREFIX)
                    .and_then(|r| r.strip_suffix(PLANNER_BLOCKED_SUFFIX))
            });
        if let Some(reason) = reason
            && !out.iter().any(|r| r == reason)
        {
            out.push(reason.to_string());
        }
    }
    out
}

/// ADR-0079 D7（Phase R3a）: 節点の events で、`plan_invalid` の決定への最後の回答（`DecisionAnswered`）の位置
/// （planner の試行の窓を開け直す境目）。
fn plan_invalid_answer_position(events: &[(u64, Event)]) -> Option<usize> {
    let ids: std::collections::BTreeSet<&str> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::DecisionRequested { decision }
                if decision.kind == task_core::DecisionKind::PlanInvalid =>
            {
                Some(decision.id.as_str())
            }
            _ => None,
        })
        .collect();
    events.iter().rposition(
        |(_, e)| matches!(e, Event::DecisionAnswered { id, .. } if ids.contains(id.as_str())),
    )
}

/// ADR-0079 D7（Phase R3a）: 最後の計画の採用の後に `plan_invalid` へ「replan」で答えた人の note（planner に
/// 渡す。無ければ `None`）。
fn plan_invalid_replan_note(events: &[(u64, Event)]) -> Option<String> {
    let idx = plan_invalid_answer_position(events)?;
    let last_plan = events
        .iter()
        .rposition(|(_, e)| matches!(e, Event::ExecutionPlanned { .. }));
    if last_plan.is_some_and(|p| p > idx) {
        return None;
    }
    match &events[idx].1 {
        Event::DecisionAnswered {
            option, note, by, ..
        } if task_core::answer_effect(task_core::DecisionKind::PlanInvalid, option)
            == task_core::DecisionEffect::Replan =>
        {
            Some(
                match note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                    Some(n) => format!(
                        "人の指示（{by}、plan_invalid の決定への回答 replan）: {n} — 前の試行の拒否の理由を踏まえ、この指示に従って計画を書き直してください"
                    ),
                    None => format!(
                        "人（{by}）が plan_invalid の決定に replan と答えました。前の試行の拒否の理由を直して計画を書き直してください"
                    ),
                },
            )
        }
        _ => None,
    }
}

/// ADR-0072 D5（E2b の指摘、Phase E3 で配線）: reviewer run の `runs` 索引の finish。`completed_review_run`
/// が `None`（Reviewer run を起動しなかった判定）なら何もしない。失敗しても run は壊さない（警告のみ）。
///
/// ADR-0074 §6 F1 (k)（Phase F1 で直した逸脱）: 以前はここで `usage` を常に `None` に固定しており、
/// reviewer run の `runs` 索引の行が（`WorkerFinished` に実際の usage があっても）`usage_json` を
/// 持たないまま確定していた（E6 report 問題 3、`celerisctl replay --check` で
/// `rebuild_work_units_and_runs` の usage と食い違う）。`usage` を引数で受け取り、そのまま渡す。
fn finish_reviewer_run_index(
    store: &dyn TaskStore,
    completed_review_run: &Option<String>,
    status: task_core::RunIndexStatus,
    usage: Option<task_core::Usage>,
    metrics: Option<task_core::RunMetrics>,
) {
    let Some(run_id) = completed_review_run else {
        return;
    };
    if let Err(e) = store.run_index_finish(
        run_id,
        status,
        None,
        usage,
        metrics,
        OffsetDateTime::now_utc(),
    ) {
        tracing::warn!(%run_id, error = %e, "failed to finish the reviewer run in the runs index");
    }
}

/// (k): `reviewer_finished`（`Some(Event::WorkerFinished{..})` なら）が運ぶ `usage` を取り出す
/// （`finish_reviewer_run_index` に渡すため。move で消費する前に呼ぶ）。
fn worker_finished_usage(event: &Option<Event>) -> Option<task_core::Usage> {
    match event {
        Some(Event::WorkerFinished { usage, .. }) => *usage,
        _ => None,
    }
}

/// ADR-0074 §6 F1 (k)（Phase F1 で直した逸脱）: reviewer run の `WorkerFinished.end` は今まで常に
/// `None` だった（`task_ops::replay::rebuild_work_units_and_runs` はこれを `HarnessError` として
/// 復元するので、`celerisctl replay --check` は reviewer run のたびに `status` の食い違いを報告して
/// いた）。この関数が最終的に決めた `RunIndexStatus` と同じ `RunEnd` を event 自身にも書き戻す。
fn set_worker_finished_end(event: &mut Option<Event>, end: task_core::RunEnd) {
    if let Some(Event::WorkerFinished { end: slot, .. }) = event.as_mut() {
        *slot = Some(end);
    }
}

/// ADR-0074 D5.3（Phase F1）: planner の出力を読む。`schema` が `celeris.execution-plan-delta/1`
/// なら差分として `active` な計画に当て、全体に展開する（`base_version` は `active` の版と一致
/// しなければならない。それ以外は「移行期間」として `celeris.execution-plan/1` の全体形式で読む
/// （D5.3「旧形式（全体）の replan 出力も受け付ける」）。
fn parse_planner_output(
    text: &str,
    active: Option<&task_core::ExecutionPlanRow>,
    done_keys: &std::collections::BTreeSet<String>,
) -> Result<task_core::ExecutionPlanSpec, String> {
    let raw: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("execution-plan.json の形式が不正: {e}"))?;
    let schema = raw.get("schema").and_then(|v| v.as_str()).unwrap_or("");
    if schema == task_core::execution_plan::EXECUTION_PLAN_DELTA_SCHEMA {
        let Some(active) = active else {
            return Err(
                "execution-plan-delta/1 は replan（既に計画がある Task）でだけ受け付ける"
                    .to_string(),
            );
        };
        // ADR-0079 D9（Phase R2b）: 差分は /2 の形（`work_units`）しか持たない。/3 の replan は計画の全体を書く。
        if active.spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3 {
            return Err(format!(
                "execution-plan-delta/1 は {} の計画の replan には使えない（計画の全体を書くこと。done の unit は省略してよい）",
                task_core::EXECUTION_PLAN_SCHEMA_V3
            ));
        }
        let delta: task_core::execution_plan::ExecutionPlanDelta = serde_json::from_value(raw)
            .map_err(|e| format!("execution-plan-delta.json の形式が不正: {e}"))?;
        if delta.base_version != active.version {
            return Err(format!(
                "execution-plan-delta.json の base_version {} が現在の版 {} と一致しない",
                delta.base_version, active.version
            ));
        }
        task_core::execution_plan::apply_delta(&active.spec, &delta)
    } else {
        let mut spec: task_core::ExecutionPlanSpec = serde_json::from_value(raw)
            .map_err(|e| format!("execution-plan.json の形式が不正: {e}"))?;
        // ADR-0079 D9（Phase R2b）: /3 の replan では done の unit を今の版の spec のまま持ち越す（planner が
        // 省いた・書き写し損ねた unit も。unit の gate で上げ下げした spec を planner は知らない）。
        if let Some(active) = active {
            task_core::execution_plan::carry_done_units_v3(&active.spec, &mut spec, done_keys);
        }
        Ok(spec)
    }
}

/// ADR-0072 D14（Phase E3）: 計画の各 WorkUnit の `harness` が `[[genres]]` にある id だけかを確かめる
/// （担当の profile が許す harness に限る、の簡易版。`genres` が空の設定では検証しない。既存の
/// `[[genres]] roles` の検証と同じ考え方）。
fn validate_plan_harnesses(
    spec: &task_core::ExecutionPlanSpec,
    genres: &[GenreSpec],
) -> Result<(), String> {
    if genres.is_empty() {
        return Ok(());
    }
    for wu in &spec.work_units {
        if let Some(harness) = &wu.harness
            && !genres.iter().any(|g| &g.id == harness)
        {
            return Err(format!(
                "work unit {} has unknown harness {harness:?} (not in [[genres]])",
                wu.key
            ));
        }
    }
    Ok(())
}

/// ADR-0072 D9（Phase E1）: この run が continuation（予算切れ・yield の続き）なら、次の run の
/// `RunContext.continuation` に渡す最小限の文脈を events から純粋に組み立てる。`events` には、
/// これから始まる run 自身の `WorkerStarted`（run_seq の計算に使う）が既に入っている前提
/// （`run_worker` が `dispatch` の遷移と `WorkerStarted` の追記のあとに呼ばれるため）。
/// continuation でなければ `None`（前の run の会話・出力の全文は載せない。D9）。
fn build_continuation_context(events: &[(u64, Event)]) -> Option<task_worker::ContinuationContext> {
    if consecutive_continuations(events) == 0 {
        return None;
    }
    let checkpoint = latest_checkpoint(events, None)?;
    let run_seq = current_run_seq(events);
    let previous_end = events
        .iter()
        .rev()
        .find_map(|(_, ev)| match ev {
            Event::WorkerFinished {
                role: None,
                end: Some(e),
                ..
            } => Some(describe_run_end(*e)),
            _ => None,
        })
        .unwrap_or_else(|| "budget_exhausted".to_string());
    // これまでの run の 1 行要約（古い順、最大 10 件）。
    let mut seq_by_run: HashMap<&str, u32> = HashMap::new();
    let mut n = 0u32;
    for (_, ev) in events {
        if let Event::WorkerStarted {
            run_id, role: None, ..
        } = ev
        {
            n += 1;
            seq_by_run.insert(run_id.as_str(), n);
        }
    }
    let mut prior_runs: Vec<String> = Vec::new();
    for (_, ev) in events {
        if let Event::WorkerFinished {
            run_id,
            role: None,
            end: Some(e),
            ..
        } = ev
            && let Some(seq) = seq_by_run.get(run_id.as_str())
        {
            prior_runs.push(format!("Run #{seq} {}", describe_run_end(*e)));
        }
    }
    if prior_runs.len() > 10 {
        let start = prior_runs.len() - 10;
        prior_runs = prior_runs.split_off(start);
    }
    let checkpoint_json = serde_json::to_value(&checkpoint).ok()?;
    Some(task_worker::ContinuationContext {
        run_seq,
        previous_end,
        checkpoint: checkpoint_json,
        prior_runs,
    })
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

/// ADR-0066 D3: 専用スレッドが期限をチェックする周期。`probe_interval_secs`（最短でも 1 秒）よりずっと
/// 短くして、期限が来たらすぐ probe する。
const TUNNEL_PROBER_STEP: Duration = Duration::from_millis(200);

/// ADR-0066 D3: forward ごとの target probe を、tick とは無縁に裏で行い続けるループ
/// （`ensure_tunnel_prober_started` が専用スレッドで起こす）。`stop` が立ったら抜ける。
fn tunnel_prober_loop(
    probe: TunnelProbe,
    forwards: Vec<(String, String, String, u64)>,
    state: Arc<std::sync::Mutex<HashMap<String, TargetProbeState>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let now = Instant::now();
        for (cluster, listen, _target, min_secs) in &forwards {
            let key = tunnel_key(cluster, listen);
            let due = {
                let Ok(guard) = state.lock() else { continue };
                guard
                    .get(&key)
                    .map(|s| {
                        now.duration_since(s.checked_at) >= Duration::from_secs(s.interval_secs)
                    })
                    .unwrap_or(true)
            };
            if !due {
                continue;
            }
            let result = probe(listen);
            let healthy = result.is_ok();
            let Ok(mut guard) = state.lock() else {
                continue;
            };
            let prev_interval = guard
                .get(&key)
                .map(|s| s.interval_secs)
                .unwrap_or(*min_secs);
            guard.insert(
                key,
                TargetProbeState {
                    healthy,
                    error: result.err(),
                    checked_at: Instant::now(),
                    interval_secs: next_probe_interval_secs(prev_interval, *min_secs, healthy),
                },
            );
        }
        std::thread::sleep(TUNNEL_PROBER_STEP);
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

    /// ADR-0040 D4: `[handoff] drain_timeout_secs` を超えたときに、残っている run とレビューを
    /// 打ち切る。DB の状態は変えない（リースが切れて新しい active が従来の「リース切れ」の経路で拾う）。
    /// 打ち切った数を返す。
    pub fn abort_all_runs(&mut self) -> usize {
        // ADR-0044 §5 Phase 53 追記（Phase 55）: drain も他の 4 つと同じ止め方
        // （プロセスグループへ SIGTERM → `kill_grace_secs` → SIGKILL）。
        let kill_grace = self.config.kill_grace;
        let mut aborted = 0;
        for (key, entry) in self.running.drain() {
            let task_id = key.task;
            tracing::warn!(task_id = %task_id, run_id = %entry.run_id, "drain timeout; aborting the run (the lease will expire and the new active will reclaim it)");
            // Phase 55/56 の合流: コンテナで走っている run はラベル越しにも止める（P55-4 / P56-7）。
            task_worker::kill_tree_with(&entry.run_id, kill_grace, entry.container);
            entry.handle.abort();
            aborted += 1;
        }
        let reviewing: Vec<(TaskId, ReviewEntry)> = self.reviewing.drain().collect();
        for (task_id, entry) in reviewing {
            tracing::warn!(task_id = %task_id, "drain timeout; aborting the review");
            task_worker::kill_tree(&entry.run_id, kill_grace);
            if let Some(review_run_id) = &entry.review_run_id {
                task_worker::kill_tree(review_run_id, kill_grace);
                // Phase F5-fix3: Reviewer run は lease を持たない（新しい active は review をやり直すだけで
                // この run を閉じない）ので、ここで `runs` 行ごと閉じる。Task の状態は変えない。
                self.close_aborted_run(
                    task_id,
                    review_run_id,
                    Some(RunRole::Reviewer),
                    "review aborted (drain timeout)",
                );
            }
            entry.handle.abort();
            aborted += 1;
        }
        // Phase F5-fix2: 検査中の WU と工程の統合も止める（DB は変えない。lease 切れの経路で
        // 新しい active が拾う。検査前の run の result.json があれば、そこから確定させる）。
        for (run_id, entry) in self.checking.drain() {
            tracing::warn!(task_id = %entry.task_id, %run_id, "drain timeout; aborting the work unit checks");
            entry.handle.abort();
            aborted += 1;
        }
        for (task_id, entry) in self.integrating.drain() {
            tracing::warn!(%task_id, work_unit = %entry.work_unit_id, "drain timeout; aborting the phase integration");
            entry.handle.abort();
            aborted += 1;
        }
        self.pending_subjects.clear();
        aborted
    }

    /// Phase F5-fix6: SIGTERM / SIGINT（`systemctl restart`、`promote.sh` の停止→起動）で止まる直前に、
    /// 手元の worker run の終わりを DB に記録する。本番 2026-09-28 17:05:50Z: 止まるデーモンは SIGTERM を
    /// 受けた tick でループを抜け、0.6 秒後にアダプタが `result.json`（exit=143）を書いたが、完了を
    /// 受け取る者が居ないまま exit し、`worker_finished` も Task の遷移も残らなかった（新しいデーモンは
    /// lease の失効まで 15 分待った）。
    ///
    /// 1. 既に届いた完了を記録する（`drain_completions`）。
    /// 2. 残りの run はプロセスグループごと止め（`stop_run`）、`WorkerFinished{outcome: "interrupted:
    ///    daemon shutdown …", end: cancelled}` を残す。Task の lease を持つ run は `InfraRequeue`
    ///    （attempts を消費しない。上限超過は `infra failure ×N`）、WU の run は WU を ready /
    ///    needs_continuation に戻す（reason `shutdown`）。
    /// 3. ただし run が既に error 以外の終端の `result.json`（done 等）を書き終えていたら DB は触らない
    ///    （次のデーモンが孤児の回収でその内容から確定させる。ここで検査・レビューを spawn しても exit で
    ///    失われるため）。
    ///
    /// レビュー・WU の検査・工程の統合は触らない（次のデーモンの `recover_reviews` と孤児の回収が拾う）。
    /// DB に記録した run の数を返す。
    pub fn interrupt_runs_on_shutdown(&mut self) -> usize {
        if let Err(e) = self.drain_completions() {
            tracing::warn!(error = %e, "failed to record the completions received before the shutdown");
        }
        let entries: Vec<(RunKey, RunEntry)> = self.running.drain().collect();
        let mut recorded = 0;
        for (key, entry) in entries {
            let run_id = entry.run_id.clone();
            let since = entry.since;
            self.stop_run(&run_id, entry.handle, entry.container);
            let task = match self.store.get(key.task) {
                Ok(Some(t)) => t,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(task_id = %key.task, %run_id, error = %e, "could not read the task while recording the shutdown");
                    continue;
                }
            };
            if let Some(dir) = self.task_dir(&task)
                && let Some(terminal) = terminal_from_run_dir(&dir, &run_id)
                && !matches!(terminal, Terminal::Error { .. })
            {
                tracing::info!(task_id = %task.id, %run_id, "the run already wrote a terminal result.json; leaving it to the next daemon's orphan takeover (Phase F5-fix6)");
                continue;
            }
            match self.record_shutdown_interrupt(&task, &run_id, since) {
                Ok(()) => {
                    tracing::warn!(task_id = %task.id, %run_id, "daemon shutdown: the run was stopped and recorded as interrupted (Phase F5-fix6)");
                    recorded += 1;
                }
                Err(e) => {
                    tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the shutdown interrupt; the next daemon's orphan takeover will pick it up");
                }
            }
        }
        recorded
    }

    fn record_shutdown_interrupt(
        &mut self,
        task: &Task,
        run_id: &str,
        since: OffsetDateTime,
    ) -> Result<(), DispatchError> {
        let events = self.store.events_for(task.id)?;
        if events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerFinished { run_id: r, .. } if r == run_id))
        {
            return Ok(());
        }
        let finished = |outcome: String| Event::WorkerFinished {
            run_id: run_id.to_string(),
            outcome,
            usage: None,
            role: None,
            metrics: Some(task_core::RunMetrics {
                wall_ms: wall_ms_since(since),
                retries: task.attempts,
                peak_context_tokens: None,
                turns: None,
            }),
            end: Some(task_core::RunEnd::Cancelled),
        };
        let holds_task_lease = task.status == Status::Running
            && task
                .lease
                .as_ref()
                .is_some_and(|l| l.worker_run_id == run_id);
        if holds_task_lease {
            let infra_n = consecutive_infra_requeues(&events) + 1;
            let (trigger, outcome) = if infra_n <= self.config.max_infra_retries {
                (
                    Trigger::InfraRequeue,
                    format!("interrupted: {SHUTDOWN_WHY} (run_id={run_id})"),
                )
            } else {
                (
                    Trigger::WorkerError { retryable: false },
                    format!("{INFRA_FAILURE_MARKER}{infra_n}: {SHUTDOWN_WHY} (run_id={run_id})"),
                )
            };
            match self
                .store
                .apply_transition_with_events(task.id, trigger, vec![finished(outcome)])
            {
                Ok(_) | Err(StoreError::InvalidTransition(_)) => {}
                Err(e) => return Err(e.into()),
            }
        } else {
            self.store.append_event(
                task.id,
                &finished(format!("interrupted: {SHUTDOWN_WHY} (run_id={run_id})")),
            )?;
        }
        self.reconcile_work_unit_run(task.id, run_id, "shutdown")
    }

    /// ADR-0032 D3: `auth = "publickey"` のクラスタへの自動接続を有効にする（celeris 側が本番の実装を挿す）。
    /// 呼ばなければ従来どおり自動接続しない。
    pub fn set_cluster_connector(&mut self, connector: ClusterConnector) {
        self.cluster_connector = Some(connector);
    }

    /// ADR-0032 D4/D5: GUI 発の接続が進行中かを記録する（celeris の管理 API が呼ぶ。呼び出しは celeris 側の配線）。
    pub fn set_cluster_connect_pending(&mut self, id: &str, pending: bool) {
        if pending {
            self.connect_pending_clusters.insert(id.to_string());
        } else if self.connect_pending_clusters.remove(id) {
            // ADR-0078 D5: 次に繋がったら、それは GUI 発の接続（`method = auth`）として数える。
            self.cluster_conn
                .entry(id.to_string())
                .or_default()
                .gui_connect_finished = true;
        }
    }

    /// ADR-0053 D3（Phase 66）: `[[clusters]].forwards` を(再)確立するフックを挿す。呼ばなければ
    /// `refresh_cluster_tunnels` は forward を張り直さない（届かないまま観測するだけ）。
    pub fn set_tunnel_forward_ensurer(&mut self, ensurer: TunnelForwardEnsurer) {
        self.tunnel_forward_ensurer = Some(ensurer);
    }

    /// ADR-0053 D3 / Phase 85: forward の target（先方）の健康を見るフックを挿す。呼ばなければ常に
    /// 「不健全」扱い。
    pub fn set_tunnel_probe(&mut self, probe: TunnelProbe) {
        self.tunnel_probe = Some(probe);
    }

    /// ADR-0053 Phase 85: forward のリスナー（`-O forward` が実際に手元で待ち受けているか）を見るフックを
    /// 挿す。呼ばなければ常に「無い」扱い（安全側のデフォルト）。
    pub fn set_tunnel_listener_probe(&mut self, probe: TunnelListenerProbe) {
        self.tunnel_listener_probe = Some(probe);
    }

    /// ADR-0053 Phase 84b: クラスタの ssh master の多重接続の有無を調べるフックを差し替える。
    /// 呼ばなければ本物の `ssh -O check`（`control_master_alive_blocking`）のまま。テストは実機の ssh
    /// 状態に依存しないよう、必ずこれで偽物に差し替える。
    pub fn set_cluster_liveness_probe(&mut self, probe: ClusterLivenessProbe) {
        self.cluster_liveness_probe = probe;
    }

    /// ADR-0062 A（Phase 107）: master 越しの実通信 probe を挿す（celeris 側の配線）。
    pub fn set_cluster_command_probe(&mut self, probe: ClusterCommandProbe) {
        self.cluster_command_probe = Some(probe);
    }

    /// ADR-0062 A: celeris が保持している master の終了検出フックを挿す（celeris 側の配線）。
    pub fn set_cluster_master_watcher(&mut self, watcher: ClusterMasterWatcher) {
        self.cluster_master_watcher = Some(watcher);
    }

    /// ADR-0053 D3: 直近のトンネル状態遷移を取り出す（呼ぶと空になる。celeris はこれを Discord/Console に流す）。
    pub fn take_tunnel_events(&mut self) -> Vec<TunnelEvent> {
        self.tunnel_events.drain(..).collect()
    }

    /// ADR-0053 D3: 現在「TOTP ログインが要る」状態のクラスタ id（昇順）。
    pub fn clusters_needing_login(&self) -> Vec<String> {
        let mut v: Vec<String> = self.cluster_login_needed.iter().cloned().collect();
        v.sort();
        v
    }

    /// ADR-0053 D3: forward が今届いているか（listener も target も健全。無ければ観測が無い＝`false`）。
    pub fn tunnel_reachable(&self, cluster: &str, listen: &str) -> bool {
        self.tunnel_state
            .get(&tunnel_key(cluster, listen))
            .map(ForwardObservation::up)
            .unwrap_or(false)
    }

    /// ADR-0053 Phase 85: forward の**リスナー**が今有るか（`-O forward` が届いているか。target の健康とは
    /// 別。無ければ観測が無い＝`false`）。
    pub fn tunnel_listener_present(&self, cluster: &str, listen: &str) -> bool {
        self.tunnel_state
            .get(&tunnel_key(cluster, listen))
            .map(|o| o.listener)
            .unwrap_or(false)
    }

    /// ADR-0053 Phase 85: forward の**target**が今健全か（`/v1/models` が応答するか。listener の有無とは
    /// 別。無ければ観測が無い＝`false`）。
    pub fn tunnel_target_healthy(&self, cluster: &str, listen: &str) -> bool {
        self.tunnel_state
            .get(&tunnel_key(cluster, listen))
            .map(|o| o.target_healthy)
            .unwrap_or(false)
    }

    /// ADR-0053 Phase 85: forward の直近の失敗理由（無ければ `None`）。
    pub fn tunnel_last_error(&self, cluster: &str, listen: &str) -> Option<String> {
        self.tunnel_state
            .get(&tunnel_key(cluster, listen))
            .and_then(|o| o.last_error.clone())
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

    /// ADR-0018 D2 / ADR-0032 D3: 多重接続が無い（または自動接続を試みて失敗した）クラスタを cooldown にし、
    /// 理由をタスクのイベントに残す（人にログインを促すため）。`reason` は呼び出し側が組み立てる
    /// （自動接続を試みて失敗した場合は `"auto-connect failed: ..."` を含め、従来の「接続が無い」だけの文言と区別する）。
    fn mark_cluster_unavailable(
        &mut self,
        task_id: TaskId,
        spec: &ClusterSpec,
        reason: String,
    ) -> Result<(), DispatchError> {
        let mut reason = reason;
        // ADR-0062 A（Phase 107）: 明示的な切断を経ずに master が終了していた／実通信 probe が
        // 失敗していたなら、その詳細（exit code・stderr の末尾）を報告に足し、`Event::ClusterMasterExited`
        // を 1 回だけ残す（同じ切断について 2 回目以降は reason に足すだけ。cooldown が明けて接続が
        // 戻れば `refresh_cluster_liveness` がこのエントリを消す）。
        if let Some(info) = self.cluster_disconnect_info.get(&spec.id).cloned() {
            let exit_str = info
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown (signal?)".to_string());
            reason = format!(
                "{reason}\nssh master exited (exit_code={exit_str}): {}",
                info.detail
            );
            if !info.reported {
                self.store.append_event(
                    task_id,
                    &Event::ClusterMasterExited {
                        cluster: spec.id.clone(),
                        exit_code: info.exit_code,
                        stderr_tail: info.detail.clone(),
                    },
                )?;
                if let Some(entry) = self.cluster_disconnect_info.get_mut(&spec.id) {
                    entry.reported = true;
                }
            }
        }
        // ADR-0062 A: TOTP のクラスタは人の入力が要るので、その場での自動復旧を待たせない。
        if spec.auth == "totp" {
            reason =
                format!("{reason}\nGUI の「クラスタ」画面から TOTP を入力して再接続してください。");
        }
        let first = self
            .cluster_cooldown
            .insert(
                spec.id.clone(),
                Instant::now() + self.config.cluster_cooldown,
            )
            .is_none();
        if first {
            tracing::warn!(
                cluster = %spec.id, host = %spec.host, %reason,
                "no ssh ControlMaster connection; run `scripts/cluster-login.sh {}` to log in again", spec.host
            );
        }
        self.store.append_event(
            task_id,
            &Event::ClusterUnavailable {
                cluster: spec.id.clone(),
                host: spec.host.clone(),
                reason: reason.clone(),
            },
        )?;
        // ADR-0033 D3: クラスタが落ちたことは `infra` 相当のノードの悪い知らせとして人まで上げる
        // （案件に紐づかない）。同じホストの障害を毎 tick 繰り返さないよう、cooldown の間は 1 件だけにする。
        let now = OffsetDateTime::now_utc();
        let cooldown_secs =
            i64::try_from(self.config.cluster_cooldown.as_secs()).unwrap_or(i64::MAX);
        match crate::reports::cluster_report_recently_recorded(
            self.store.as_ref(),
            &spec.host,
            now,
            cooldown_secs,
        ) {
            Ok(true) => {}
            Ok(false) => {
                if let Err(e) = crate::reports::record_cluster_unavailable_report(
                    self.store.as_ref(),
                    &spec.id,
                    &spec.host,
                    &reason,
                    Some(task_id),
                    now,
                ) {
                    tracing::warn!(cluster = %spec.id, error = %e, "failed to record the cluster report");
                }
            }
            Err(e) => {
                tracing::warn!(cluster = %spec.id, error = %e, "failed to read the recent cluster reports")
            }
        }
        Ok(())
    }

    /// ADR-0032 D3: `auth = "publickey"` のクラスタに接続フックが刺さっていれば 1 回だけ接続を試みる。
    /// フックが無い、または `auth` が `"publickey"` でなければ `None`（＝試みなかった。呼び出し側は従来どおり
    /// cooldown に落とす）。試みた場合は結果（`Ok(())` = 成功、`Err(detail)` = 失敗の理由）を返す。
    ///
    /// ADR-0053 Phase 66b の「未解決事項」/ Phase 81: `dispatch_ready`（この関数の呼び出し元）は
    /// celeris の tick ループの中から**同期のまま**呼ばれ、そのループ自身がマルチスレッドの tokio
    /// ランタイムの上で動く async タスクなので、`cluster_connector`（ネストしたランタイムを
    /// `block_on` しうる）をここでインラインに呼ぶと `refresh_cluster_tunnels` と同じ形で panic
    /// しうる（本番はまだ踏んでいないが、理論上の危険性は Phase 66b で指摘済み）。
    /// `refresh_cluster_liveness`/`refresh_cluster_tunnels` と同じ `run_cluster_hooks_off_async`
    /// （本物の OS スレッドへ逃がし、マルチスレッド・ランタイムの上でだけ `block_in_place` で包む）に
    /// 通すことで、`cluster_connector` 側の防御的ガード（`Handle::try_current().is_ok()` なら
    /// エラーを返す）に頼らずに済むようにした。
    fn try_auto_connect_cluster(&self, spec: &ClusterSpec) -> Option<Result<(), String>> {
        if spec.auth != "publickey" {
            return None;
        }
        let connector = self.cluster_connector.as_ref()?.clone();
        let id = spec.id.clone();
        let host = spec.host.clone();
        Some(run_cluster_hooks_off_async(move || connector(&id, &host)))
    }

    /// ADR-0018 D2: 設定の全クラスタについて、多重接続の有無を 1 tick に 1 回調べる（`ssh -O check` は unix ソケットを
    /// 見るだけで即座に返る。ネットワークにも認証にも触れない）。結果は dispatch の判断とスナップショットの `connected` に使う。
    /// 接続が戻っていれば cooldown を解く（人がログインし直したら、次の tick から再開できるように）。
    fn refresh_cluster_liveness(&mut self) {
        // ADR-0041 D5 / Phase 66c: `--mode verify` は「migrations、API と `smoke` の煙試験だけ。他の
        // dispatch も裏方の仕事も無い」（`self.eligible.is_some()` が `--mode verify` の煙試験だけを
        // 目印。ADR-0041 D5、`set_eligible_tasks` の doc を参照）。`refresh_cluster_liveness` は DB へは
        // 書かないが、実クラスタへ `ssh -O check` を打つ副作用（生の ssh 呼び出し）は「裏方の仕事」その
        // ものなので、verify では走らせない。
        if self.eligible.is_some() {
            return;
        }
        if self.config.clusters.is_empty() {
            return;
        }
        // ADR-0023 D1: tick ごとではなく 5 秒に 1 回。外れた判定で dispatch しても、ssh が 255 を返して
        // 供給側失敗（attempts を消費しない cooldown）になるだけなので、多少古くても困らない。
        let now = self.monotonic_now();
        if let Some(last) = self.last_cluster_liveness
            && now.duration_since(last) < CLUSTER_LIVENESS_INTERVAL
        {
            return;
        }
        self.last_cluster_liveness = Some(now);
        let ssh_command = SshSettings::new("", "", "/").ssh_command;
        let mut specs: Vec<(String, String, u64)> = self
            .config
            .clusters
            .values()
            .map(|c| (c.id.clone(), c.host.clone(), c.liveness_probe_secs))
            .collect();
        specs.sort();
        for (id, host, liveness_probe_secs) in specs {
            let mut alive = (self.cluster_liveness_probe)(&ssh_command, &host);
            let mut change = ClusterConnChange::Checked;
            if !alive {
                // `-O check` が落ちた: probe の途中経過は捨てる（次に繋がったら数え直す）。
                if let Some(state) = self.cluster_conn.get_mut(&id) {
                    state.probe_failures = 0;
                    state.probe_dead = false;
                    state.probe_inflight = None;
                }
            }
            // ADR-0062 A（Phase 107）: `-O check` は unix ソケットを見るだけで、NAT / ファイアウォールの
            // idle timeout で TCP が黙って死んでいても「Master running」を返し続ける。master 越しの
            // 実通信（`ssh -o BatchMode=yes <host> -- true`）で定期的に確定させる。
            if alive
                && liveness_probe_secs > 0
                && let Some(probe) = self.cluster_command_probe.clone()
            {
                // ADR-0078 D3-4: probe は別スレッドで走らせ、ここでは終わった結果だけを拾う（tick を
                // `LIVENESS_PROBE_TIMEOUT` まで塞がない）。
                if let Some(ok) = self.take_cluster_probe_result(&id) {
                    let state = self.cluster_conn.entry(id.clone()).or_default();
                    if ok {
                        state.probe_failures = 0;
                        state.probe_dead = false;
                    } else {
                        state.probe_failures += 1;
                        let failures = state.probe_failures;
                        tracing::warn!(
                            cluster = %id, %host, failures,
                            "liveness probe (ssh -- true) failed; the master is kept (ADR-0078 D3-3)"
                        );
                        // ADR-0078 D3-3: `-O exit` はしない（遅いだけの timeout で TOTP の要る master を
                        // 捨てない）。本当に TCP が死んでいれば master は keepalive で自分で終わる。
                        if failures >= LIVENESS_PROBE_FAILURES_TO_LOSE && !state.probe_dead {
                            state.probe_dead = true;
                            self.cluster_disconnect_info.insert(
                                id.clone(),
                                ClusterDisconnectInfo {
                                    exit_code: None,
                                    detail: format!(
                                        "liveness probe: `ssh -o BatchMode=yes {host} -- true` failed or timed out {failures} times in a row"
                                    ),
                                    reported: false,
                                },
                            );
                        }
                    }
                }
                let state = self.cluster_conn.entry(id.clone()).or_default();
                if state.probe_dead {
                    alive = false;
                    change = ClusterConnChange::ProbeFailed;
                }
                let due = state.probe_inflight.is_none()
                    && self.last_cluster_command_probe.get(&id).is_none_or(|last| {
                        now.duration_since(*last) >= Duration::from_secs(liveness_probe_secs)
                    });
                if due {
                    self.last_cluster_command_probe.insert(id.clone(), now);
                    let (tx, rx) = std::sync::mpsc::channel();
                    let ssh_command = ssh_command.clone();
                    let host_for_probe = host.clone();
                    match std::thread::Builder::new()
                        .name(format!("celeris-cluster-probe-{id}"))
                        .spawn(move || {
                            let ok = probe(&ssh_command, &host_for_probe, LIVENESS_PROBE_TIMEOUT);
                            // 受け手（dispatcher）が先に捨てていれば送れないだけ。
                            let _ = tx.send(ok);
                        }) {
                        Ok(_) => {
                            self.cluster_conn
                                .entry(id.clone())
                                .or_default()
                                .probe_inflight = Some(rx);
                        }
                        Err(e) => {
                            tracing::warn!(cluster = %id, error = %e, "could not start the liveness probe thread");
                        }
                    }
                }
            }
            self.set_cluster_connected(&id, alive, change);
            if alive {
                // ADR-0062 A: 接続が戻ったら、前回の切断の詳細は捨てる（次に切れたときは新しい詳細で
                // 1 回だけ報告する）。
                self.cluster_disconnect_info.remove(&id);
                if self.cluster_cooldown.remove(&id).is_some() {
                    tracing::info!(cluster = %id, %host, "ssh ControlMaster connection is back; cluster cooldown cleared");
                }
            }
        }
    }

    /// ADR-0078 D3-4: 別スレッドの実通信 probe が終わっていればその結果を取り出す（まだなら `None`。
    /// スレッドが結果を送らずに終わった〈panic〉なら失敗として数える）。
    fn take_cluster_probe_result(&mut self, id: &str) -> Option<bool> {
        let state = self.cluster_conn.get_mut(id)?;
        let rx = state.probe_inflight.as_ref()?;
        let result = match rx.try_recv() {
            Ok(ok) => ok,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        };
        state.probe_inflight = None;
        Some(result)
    }

    /// ADR-0078 D4/D5: `cluster_connected` の書き換えはすべてここを通す。true→false（切断）と
    /// false→true（接続）の遷移を 1 か所で拾い、log（固定の文言と構造化フィールド。journal で数えられる）・
    /// `cluster_connection_log`（daemon の再起動をまたいで数える）・起動以降の回数に残す。切断では
    /// 人への通知も出す（totp で鍵認証の出番が無いときはここで、ある場合は鍵認証が失敗したときに
    /// `ensure_cluster_master_for_tunnel` が出す）。同じ値の書き込み（false のままの tick 等）では何もしない。
    fn set_cluster_connected(&mut self, id: &str, alive: bool, change: ClusterConnChange) {
        let was = self
            .cluster_connected
            .insert(id.to_string(), alive)
            .unwrap_or(false);
        if was == alive {
            return;
        }
        let spec = self.config.clusters.get(id).cloned();
        let host = spec.as_ref().map(|s| s.host.clone()).unwrap_or_default();
        let auth = spec.as_ref().map(|s| s.auth.clone()).unwrap_or_default();
        let now = OffsetDateTime::now_utc();
        let last_tick_gap_ms = self.last_tick_gap_ms;
        let state = self.cluster_conn.entry(id.to_string()).or_default();
        let record = if alive {
            let method = if change == ClusterConnChange::KeyAuth {
                "publickey"
            } else if std::mem::take(&mut state.gui_connect_finished) {
                if auth == "totp" { "totp" } else { "publickey" }
            } else {
                "borrowed"
            };
            state.connected_since = Some(Instant::now());
            state.probe_failures = 0;
            state.probe_dead = false;
            state.key_auth_tried = false;
            state.key_auth_backoff = Duration::ZERO;
            state.next_key_auth_at = None;
            state.key_auth_failures = 0;
            state.unavailable_reported = false;
            let record = task_core::ClusterConnectionRecord {
                cluster_id: id.to_string(),
                kind: "connected".into(),
                method: Some(method.into()),
                cause: None,
                uptime_secs: None,
                at: now,
            };
            state.stats.add(&record);
            let attempt = state.stats.connects_totp
                + state.stats.connects_publickey
                + state.stats.connects_borrowed;
            tracing::info!(cluster = %id, %host, method, attempt, "cluster ssh master connected");
            record
        } else {
            let cause = change.lost_cause();
            let uptime_secs = state.connected_since.take().map(|t| t.elapsed().as_secs());
            tracing::warn!(
                cluster = %id, %host, cause, uptime_secs = ?uptime_secs, last_tick_gap_ms,
                "cluster ssh master lost"
            );
            let at = now.format(&Rfc3339).unwrap_or_default();
            let uptime = uptime_secs
                .map(|s| format!("{s} 秒"))
                .unwrap_or_else(|| "不明".to_string());
            state.last_lost_detail = Some(format!(
                "切れた時刻: {at}（接続していた時間: {uptime}、推定の理由: {cause}）"
            ));
            let record = task_core::ClusterConnectionRecord {
                cluster_id: id.to_string(),
                kind: "lost".into(),
                method: None,
                cause: Some(cause.into()),
                uptime_secs,
                at: now,
            };
            state.stats.add(&record);
            record
        };
        if let Err(e) = self.store.cluster_connection_record(&record) {
            tracing::warn!(cluster = %id, error = %e, "failed to record the cluster connection change");
        }
        if alive {
            // 繋がったら「ログインが要る」は解く（forward の無いクラスタは `ensure_cluster_master_for_tunnel`
            // を通らないので、ここで解かないと次の切断を知らせられない）。
            self.clear_login_needed(id);
        }
        // ADR-0078 D4: totp / manual のクラスタで、鍵認証の再接続の出番が無い（forward が無いので
        // `ensure_cluster_master_for_tunnel` を通らない、または接続フックが無い）なら、ここで人に知らせる。
        if !alive
            && let Some(spec) = spec
            && spec.auth != "publickey"
            && (spec.forwards.is_empty() || self.cluster_connector.is_none())
        {
            self.mark_login_needed(&spec);
        }
    }

    /// ADR-0078 D3: 今、このクラスタで鍵認証の再接続を試してよいか。totp / manual は切断 1 回につき
    /// 1 回だけ（ADR-0032 §3）、publickey は指数バックオフ。
    fn key_auth_allowed(&self, spec: &ClusterSpec) -> bool {
        let Some(state) = self.cluster_conn.get(&spec.id) else {
            return true;
        };
        if spec.auth == "publickey" {
            state
                .next_key_auth_at
                .is_none_or(|at| self.monotonic_now() >= at)
        } else {
            !state.key_auth_tried
        }
    }

    /// ADR-0078 D3/D5: 鍵認証の再接続を 1 回試した結果を帳簿・log・`cluster_connection_log` に残す。
    fn note_key_auth_attempt(&mut self, spec: &ClusterSpec, ok: bool) {
        let now_instant = self.monotonic_now();
        let state = self.cluster_conn.entry(spec.id.clone()).or_default();
        state.key_auth_tried = true;
        if ok {
            state.key_auth_backoff = Duration::ZERO;
            state.next_key_auth_at = None;
            state.key_auth_failures = 0;
        } else if spec.auth == "publickey" {
            state.key_auth_backoff = next_key_auth_backoff(state.key_auth_backoff);
            state.next_key_auth_at = Some(now_instant + state.key_auth_backoff);
            state.key_auth_failures += 1;
        }
        let backoff_secs = state.key_auth_backoff.as_secs();
        let record = task_core::ClusterConnectionRecord {
            cluster_id: spec.id.clone(),
            kind: "key_auth_attempt".into(),
            method: None,
            cause: Some(if ok { "ok" } else { "failed" }.into()),
            uptime_secs: None,
            at: OffsetDateTime::now_utc(),
        };
        state.stats.add(&record);
        tracing::info!(cluster = %spec.id, ok, backoff_secs, "cluster key-auth reconnect attempt");
        if let Err(e) = self.store.cluster_connection_record(&record) {
            tracing::warn!(cluster = %spec.id, error = %e, "failed to record the cluster key-auth attempt");
        }
    }

    /// ADR-0078 D4: publickey のクラスタで鍵認証の再接続が `KEY_AUTH_FAILURES_TO_REPORT` 回続けて
    /// 失敗したら、`ClusterUnavailable` の報告を切断 1 回につき 1 件だけ出す。
    fn maybe_report_publickey_outage(&mut self, spec: &ClusterSpec) {
        let Some(state) = self.cluster_conn.get_mut(&spec.id) else {
            return;
        };
        if state.unavailable_reported || state.key_auth_failures < KEY_AUTH_FAILURES_TO_REPORT {
            return;
        }
        state.unavailable_reported = true;
        let reason = format!(
            "鍵認証による再接続が {} 回続けて失敗しました。{}",
            state.key_auth_failures,
            state.last_lost_detail.clone().unwrap_or_default()
        );
        if let Err(e) = crate::reports::record_cluster_unavailable_report(
            self.store.as_ref(),
            &spec.id,
            &spec.host,
            &reason,
            None,
            OffsetDateTime::now_utc(),
        ) {
            tracing::warn!(cluster = %spec.id, error = %e, "failed to record the cluster report");
        }
    }

    /// ADR-0062 A（Phase 107）: celeris が保持している master（`Child`）の終了を検出する
    /// フック（[`ClusterMasterWatcher`]）を、tick ごとに 1 回呼ぶ。見つかった終了はクラスタごとに
    /// 記録し（[`ClusterDisconnectInfo`]）、次に `mark_cluster_unavailable` がそのクラスタを
    /// 拾ったときに stderr の末尾・exit code を報告へ足し、`Event::ClusterMasterExited` を 1 回残す。
    fn refresh_cluster_master_exits(&mut self) {
        if self.eligible.is_some() {
            return;
        }
        let Some(watcher) = self.cluster_master_watcher.clone() else {
            return;
        };
        for exit in watcher() {
            self.set_cluster_connected(&exit.cluster, false, ClusterConnChange::MasterExited);
            tracing::warn!(
                cluster = %exit.cluster, exit_code = ?exit.exit_code,
                "cluster ssh master exited on its own (ADR-0062 A)"
            );
            self.cluster_disconnect_info.insert(
                exit.cluster.clone(),
                ClusterDisconnectInfo {
                    exit_code: exit.exit_code,
                    detail: exit.stderr_tail,
                    reported: false,
                },
            );
        }
    }

    /// ADR-0053 D3（Phase 66）: `[[clusters]].forwards` を持つクラスタのトンネルを見て、必要なら張り直す。
    /// `refresh_cluster_liveness` の直後に呼ぶ（`cluster_connected` を読むため、同じ間隔で間引く）。
    ///
    /// 1. master が死んでいたら、まず鍵認証だけで繋ぎ直す（`cluster_connector`。`auth` の値に関わらず試す
    ///    — 「TOTP を要求する前に鍵認証を試す」。ADR-0053 D3）。それでも駄目なら「TOTP ログインが要る」を
    ///    立てる（クラスタごとに 1 回だけ報告する。`cluster_login_needed` が同じ outage の間の重複を防ぐ）。
    /// 2. master が生きていれば、forward ごとに probe → 届かなければ `tunnel_forward_ensurer` で
    ///    (再)確立 → 再 probe。最終的な到達性の変化を `tunnel_events` に積む（Up/Down/Restored）。
    fn refresh_cluster_tunnels(&mut self) {
        // ADR-0041 D5 / Phase 66c（実機 2026-09-21、release 2b3aaf1d633a）: `--mode verify` は
        // dispatch も裏方の仕事もしない。しかしこれは無条件に呼ばれていたため、staging の verify
        // インスタンスでも forward の(再)確立・`cluster_login_needed` の報告書き込みが起こり、
        // `verify.sh` check 2（本番のコピーと件数が一致すること）が `reports(snapshot=135
        // staging=136)` で落ちた。`refresh_cluster_liveness` と同じ目印
        // （`self.eligible.is_some()` = `--mode verify` の煙試験）で丸ごと止める。
        if self.eligible.is_some() {
            return;
        }
        if !self
            .config
            .clusters
            .values()
            .any(|c| !c.forwards.is_empty())
        {
            return;
        }
        let now_instant = Instant::now();
        if let Some(last) = self.last_cluster_tunnel_refresh
            && now_instant.duration_since(last) < CLUSTER_LIVENESS_INTERVAL
        {
            return;
        }
        self.last_cluster_tunnel_refresh = Some(now_instant);

        let mut specs: Vec<ClusterSpec> = self
            .config
            .clusters
            .values()
            .filter(|c| !c.forwards.is_empty())
            .cloned()
            .collect();
        specs.sort_by(|a, b| a.id.cmp(&b.id));

        for spec in specs {
            let alive = self.ensure_cluster_master_for_tunnel(&spec);
            if !alive {
                for fwd in &spec.forwards {
                    self.observe_tunnel(
                        &spec.id,
                        &fwd.listen,
                        false,
                        false,
                        Some("the cluster ssh master is not connected".to_string()),
                    );
                }
                continue;
            }
            for fwd in spec.forwards.clone() {
                self.refresh_one_forward(&spec, &fwd);
            }
        }
    }

    /// master が生きているか確認し、死んでいれば鍵認証を 1 回試す（D3: TOTP の前に鍵認証）。
    /// 成功したら `cluster_connected` / cooldown を更新し、「ログインが要る」を解除する。
    /// 失敗したら「TOTP ログインが要る」を立てる（初めてのときだけ報告する）。
    fn ensure_cluster_master_for_tunnel(&mut self, spec: &ClusterSpec) -> bool {
        if self
            .cluster_connected
            .get(&spec.id)
            .copied()
            .unwrap_or(false)
        {
            self.clear_login_needed(&spec.id);
            return true;
        }
        // ADR-0078 D3-1/D3-2: 鍵認証の再接続は、totp / manual なら切断 1 回につき 1 回だけ、publickey は
        // 指数バックオフで試す（以前は約 6 秒ごとに試し、TOTP が要る pegasus では 2 日で 7,461 回失敗した）。
        if let Some(connector) = self.cluster_connector.clone()
            && self.key_auth_allowed(spec)
        {
            let result = connector(&spec.id, &spec.host);
            self.note_key_auth_attempt(spec, result.is_ok());
            match result {
                Ok(()) => {
                    self.set_cluster_connected(&spec.id, true, ClusterConnChange::KeyAuth);
                    self.cluster_cooldown.remove(&spec.id);
                    self.clear_login_needed(&spec.id);
                    tracing::info!(cluster = %spec.id, host = %spec.host, "tunnel: key auth reconnected the ssh master");
                    return true;
                }
                Err(detail) => {
                    tracing::debug!(cluster = %spec.id, host = %spec.host, %detail, "tunnel: key auth did not reconnect");
                }
            }
        }
        if spec.auth == "publickey" {
            // ADR-0078 D4: publickey は TOTP を求めない。続けて失敗したときだけ報告する。
            self.maybe_report_publickey_outage(spec);
            return false;
        }
        self.mark_login_needed(spec);
        false
    }

    fn clear_login_needed(&mut self, cluster_id: &str) {
        if self.cluster_login_needed.remove(cluster_id) {
            tracing::info!(cluster = %cluster_id, "tunnel: login is no longer needed");
        }
    }

    /// 初めて「ログインが要る」状態に入ったときだけ、イベントを積み報告を 1 件記録する（重複排除）。
    fn mark_login_needed(&mut self, spec: &ClusterSpec) {
        if !self.cluster_login_needed.insert(spec.id.clone()) {
            return;
        }
        let now = OffsetDateTime::now_utc();
        self.push_tunnel_event(&spec.id, "", TunnelEventKind::LoginNeeded, now);
        let detail = self
            .cluster_conn
            .get(&spec.id)
            .and_then(|s| s.last_lost_detail.clone());
        if let Err(e) = crate::reports::record_cluster_login_needed_report(
            self.store.as_ref(),
            &spec.id,
            &spec.host,
            detail.as_deref(),
            now,
        ) {
            tracing::warn!(cluster = %spec.id, error = %e, "failed to record the cluster login-needed report");
        }
    }

    /// ADR-0053 D3 / Phase 85 / ADR-0066 D3: 1 本の forward。まずリスナー（`-O forward` の手元の待ち受け）
    /// の有無を見る（軽い。ssh は起こさない）。無ければ `tunnel_forward_ensurer` で(再)確立を試みる。
    ///
    /// **target の健康 probe は tick の同期経路から外れている**（ADR-0066 D3。Phase 85 の
    /// `probe_interval_secs` によるバックオフだけでは、tick の間隔と `probe_interval_secs` の既定が
    /// 同じ 30 秒だったため実質毎 tick 期限が来てしまい、`slow tick phases`〈`tunnel_ms`≈2000〉が
    /// 1 時間に 118 件出ていた〈本番観測〉）。listener が有るとき、この forward を初めて観測する
    /// （`tunnel_probe_state` にまだ記録が無い）場合だけ、この tick の中で 1 回だけ同期に probe して
    /// 種を蒔く。2 回目以降は専用スレッド（`ensure_tunnel_prober_started`）が裏で probe し続け、
    /// ここは `Mutex` を読むだけ（ssh も HTTP も呼ばない）。
    fn refresh_one_forward(&mut self, spec: &ClusterSpec, fwd: &ClusterForwardSpec) {
        let mut listener = self.probe_listener(&fwd.listen);
        let mut ensure_error: Option<String> = None;
        if !listener && let Some(ensure) = self.tunnel_forward_ensurer.clone() {
            match ensure(&spec.host, &fwd.listen, &fwd.target) {
                Ok(()) => {
                    listener = self.probe_listener(&fwd.listen);
                }
                Err(detail) => {
                    tracing::warn!(cluster = %spec.id, listen = %fwd.listen, target = %fwd.target, %detail, "tunnel: could not (re-)establish the forward");
                    ensure_error = Some(detail);
                }
            }
        }
        if !listener {
            let error = ensure_error
                .unwrap_or_else(|| "no listener on the local forward address".to_string());
            self.observe_tunnel(&spec.id, &fwd.listen, false, false, Some(error));
            return;
        }

        // **専用スレッドを起こすのは、この forward を種蒔きした後**（`ensure_tunnel_prober_started` は
        // 呼ぶたびに全 forward の一覧を見るので、先に起こすと「まだ種が無い」状態のこの forward を
        // 専用スレッドと取り合い、probe が二重に呼ばれることがある）。
        let key = tunnel_key(&spec.id, &fwd.listen);
        let existing = self
            .tunnel_probe_state
            .lock()
            .ok()
            .and_then(|guard| guard.get(&key).cloned());
        let observed = match existing {
            Some(state) => state,
            // まだ一度も probe していない（この forward の listener が今このプロセスで初めて有りに
            // なった）。専用スレッドの次の周回を待つと最初の観測が遅れるので、ここだけ 1 回同期に
            // probe して種を蒔く（Phase 85 までの「listener が有ればまず 1 回は確かめる」を保つ）。
            None => {
                let now = Instant::now();
                let result = self.probe_forward(&fwd.listen);
                let healthy = result.is_ok();
                let state = TargetProbeState {
                    healthy,
                    error: result.err(),
                    checked_at: now,
                    interval_secs: next_probe_interval_secs(
                        fwd.probe_interval_secs.max(1),
                        fwd.probe_interval_secs.max(1),
                        healthy,
                    ),
                };
                if let Ok(mut guard) = self.tunnel_probe_state.lock() {
                    guard.insert(key, state.clone());
                }
                state
            }
        };
        self.ensure_tunnel_prober_started();
        let error = if observed.healthy {
            None
        } else {
            Some(match &observed.error {
                Some(reason) => format!(
                    "target {} (as seen from {}) did not answer /v1/models through the forward: {reason}",
                    fwd.target, spec.host
                ),
                None => format!(
                    "target {} did not answer /v1/models through the forward",
                    fwd.target
                ),
            })
        };
        self.observe_tunnel(&spec.id, &fwd.listen, true, observed.healthy, error);
    }

    /// ADR-0066 D3: forward 一覧から、target probe を裏で行い続ける専用スレッドを起こす（二重に起こさ
    /// ない。`tunnel_probe` が挿してなければ何もしない）。`Drop` で止める。
    fn ensure_tunnel_prober_started(&mut self) {
        if self.tunnel_prober_stop.is_some() {
            return;
        }
        let Some(probe) = self.tunnel_probe.clone() else {
            return;
        };
        let forwards: Vec<(String, String, String, u64)> = self
            .config
            .clusters
            .values()
            .flat_map(|c| {
                let id = c.id.clone();
                c.forwards.iter().map(move |f| {
                    (
                        id.clone(),
                        f.listen.clone(),
                        f.target.clone(),
                        f.probe_interval_secs.max(1),
                    )
                })
            })
            .collect();
        if forwards.is_empty() {
            return;
        }
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let state = self.tunnel_probe_state.clone();
        let stop_for_thread = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("celeris-tunnel-probe".to_string())
            .spawn(move || tunnel_prober_loop(probe, forwards, state, stop_for_thread));
        match spawned {
            Ok(_handle) => self.tunnel_prober_stop = Some(stop),
            Err(e) => tracing::warn!(error = %e, "tunnel: could not start the target-probe thread"),
        }
    }

    /// forward のリスナーが手元に有るか（`-O forward`/`ssh -N -L` が届いているか）。
    fn probe_listener(&self, listen: &str) -> bool {
        self.tunnel_listener_probe
            .as_ref()
            .map(|p| p(listen))
            .unwrap_or(false)
    }

    /// forward の target（先方）が健全か（`/v1/models` が応答するか）。`refresh_one_forward` の
    /// 初回の種蒔きと、専用スレッドの両方から呼ぶ。
    fn probe_forward(&self, listen: &str) -> Result<(), String> {
        self.tunnel_probe
            .as_ref()
            .map(|p| p(listen))
            .unwrap_or_else(|| Err("no tunnel probe is configured".to_string()))
    }

    /// 状態遷移を記録する（`tunnel_state` を更新し、フェーズ（Up/Down/TargetUnreachable）が変わったときだけ
    /// `tunnel_events` に積む。Phase 85: 同じフェーズが続く間は 1 行も出さない＝スパムしない）。
    fn observe_tunnel(
        &mut self,
        cluster: &str,
        listen: &str,
        listener: bool,
        target_healthy: bool,
        last_error: Option<String>,
    ) {
        let key = tunnel_key(cluster, listen);
        let prev_phase = self.tunnel_state.get(&key).map(ForwardObservation::phase);
        let now = OffsetDateTime::now_utc();
        let observation = ForwardObservation {
            listener,
            target_healthy,
            last_error,
        };
        let new_phase = observation.phase();
        if prev_phase != Some(new_phase) {
            let kind = match new_phase {
                ForwardPhase::Up => {
                    if matches!(
                        prev_phase,
                        Some(ForwardPhase::Down) | Some(ForwardPhase::TargetUnreachable)
                    ) {
                        TunnelEventKind::Restored
                    } else {
                        TunnelEventKind::Up
                    }
                }
                ForwardPhase::Down => TunnelEventKind::Down,
                ForwardPhase::TargetUnreachable => TunnelEventKind::TargetUnreachable,
            };
            self.push_tunnel_event(cluster, listen, kind, now);
        }
        self.tunnel_state.insert(key, observation);
    }

    fn push_tunnel_event(
        &mut self,
        cluster: &str,
        listen: &str,
        kind: TunnelEventKind,
        at: OffsetDateTime,
    ) {
        tracing::info!(
            cluster,
            listen,
            kind = kind.as_str(),
            "tunnel: state transition"
        );
        self.tunnel_events.push_back(TunnelEvent {
            cluster: cluster.to_string(),
            listen: listen.to_string(),
            kind,
            at,
        });
        while self.tunnel_events.len() > TUNNEL_EVENTS_CAP {
            self.tunnel_events.pop_front();
        }
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

    fn on_worker_finished(
        &mut self,
        task_id: TaskId,
        run_id: String,
        provider: ProviderId,
        result: Result<RunOutcome, AdapterError>,
    ) -> Result<(), DispatchError> {
        // ADR-0024 D2/D4: プールで選んだアカウント（無ければ `None`）。失敗の cooldown をプロバイダかアカウントか
        // どちらに向けるかを後で決める。
        // ADR-0061（Phase 104）: `since`（dispatch した時刻）も一緒に取り出し、run の wall time を計算する。
        let (account, account_adapter, run_since) = self
            .take_running_by_run_id(&run_id)
            .map(|e| (e.account, e.account_adapter, Some(e.since)))
            .unwrap_or((None, None, None));
        let Some(task) = self.store.get(task_id)? else {
            tracing::warn!(%task_id, %run_id, "worker finished for unknown task");
            self.release_quota_if_tracked(
                &run_id,
                account.as_deref(),
                account_adapter,
                &provider,
                task_id,
            );
            return Ok(());
        };
        let lease_matches = self.run_holds_lease(&task, &run_id)?;
        if !lease_matches {
            // ADR-0002 D9 / ADR-0005 D4: リース回収済み・cancel 済みの古い結果は捨てる。
            tracing::warn!(%task_id, %run_id, status = ?task.status, "stale worker result discarded");
            self.release_quota_if_tracked(
                &run_id,
                account.as_deref(),
                account_adapter,
                &provider,
                task_id,
            );
            return Ok(());
        }
        // ADR-0072 D6（Phase E2）: 計画のある Task で、この run が `running` の WorkUnit のものなら
        // `Some`（`last_run_id` が一致するもの。無ければ暗黙の WorkUnit = 従来どおり `None`）。
        let current_wu = self.store.work_units_for(task_id)?.into_iter().find(|u| {
            u.status == task_core::WorkUnitStatus::Running
                && u.last_run_id.as_deref() == Some(run_id.as_str())
        });
        // ADR-0072 D14（Phase E3）: task-local な planner run はここで分岐する（Reviewing への遷移・
        // checkpoint の合成など、通常のワーカー/WU の判定は経由しない）。
        if current_wu.is_none() {
            let started_as_planner = self.store.events_for(task_id)?.iter().any(|(_, e)| {
                matches!(
                    e,
                    Event::WorkerStarted { run_id: r, role: Some(RunRole::Planner), .. }
                        if r == &run_id
                )
            });
            if started_as_planner {
                return self.on_planner_finished(
                    task_id,
                    &task,
                    run_id,
                    run_since,
                    provider,
                    (account.as_deref(), account_adapter),
                    result,
                );
            }
        }
        // ADR-0072 D14/D6・E4 (g): この WU に決定的な `checks`（`Command`）があり、run が
        // `Terminal::Done` で終わったのなら、WU を `done` にする前にそれらを実行する（`review.rs` の
        // Command 実行を再利用）。checks が無い WU・atomic な run はこれまでどおり即座に
        // `finish_worker_result` へ進む。
        if let Some(wu) = &current_wu
            && !wu.spec.checks.is_empty()
            && matches!(
                &result,
                Ok(RunOutcome {
                    terminal: Terminal::Done { .. },
                    ..
                })
            )
        {
            return self.spawn_work_unit_checks(
                task_id,
                run_id,
                wu.clone(),
                account,
                account_adapter,
                run_since,
                provider,
                result,
            );
        }
        self.finish_worker_result(
            task,
            current_wu,
            run_id,
            account,
            account_adapter,
            run_since,
            provider,
            result,
        )
    }

    /// ADR-0072 D14/D6・E4 (g): `wu.spec.checks` を `review::run_work_unit_checks`（review.rs の
    /// `Check::Command` 実行を再利用）で実行し、終わったら `Completion::WorkUnitChecks` を送る。
    /// `self.running` からは既に取り除かれている（`on_worker_finished` の冒頭）ので、ここでは
    /// lease の維持や snapshot への影響は無い（review run の Command 実行と同じ扱い）。
    #[allow(clippy::too_many_arguments)]
    fn spawn_work_unit_checks(
        &mut self,
        task_id: TaskId,
        run_id: String,
        wu: task_core::WorkUnitRow,
        account: Option<String>,
        account_adapter: Option<AccountAdapter>,
        run_since: Option<OffsetDateTime>,
        provider: ProviderId,
        result: Result<RunOutcome, AdapterError>,
    ) -> Result<(), DispatchError> {
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        let Some(dir) = self.task_dir(&task) else {
            // リモートの workspace は E4 の範囲外（review.rs の Command 実行も同様、remote_review
            // 経由の別経路を持つ。ここでは checks を飛ばして通常どおり `Done` として扱う）。
            return self.finish_worker_result(
                task,
                Some(wu),
                run_id,
                account,
                account_adapter,
                run_since,
                provider,
                result,
            );
        };
        // ADR-0074 D1.2（Phase F2b）: v2 の WU の checks は WU の作業ツリーで走らせ、検査の間も WU の
        // lease（と Task の lease）を延ばしておく（検査中に照合で WU を戻さないため）。
        let work_dir = match self.work_unit_trees(&task, &wu)?.into_iter().next() {
            Some((tree, _)) => Some(tree),
            None => self.work_dir_for(&task),
        };
        if wu.phase.is_some() {
            let ttl = self
                .config
                .review_timeout
                .saturating_mul(wu.spec.checks.len() as u32 * 2 + 1)
                + self.config.lease_grace;
            if let Err(e) = self.store.renew_lease(task_id, &run_id, ttl) {
                tracing::warn!(%task_id, %run_id, error = %e, "could not extend the work unit lease for its checks");
            }
        }
        // ADR-0074 F5-fix（不具合 1）: 検査も run と同じ `CARGO_TARGET_DIR` で走らせる（自分の
        // worktree の WU は `<repo-key>/wu-<id>`、それ以外は `<repo-key>`）。
        let own_tree = !self.work_unit_trees(&task, &wu)?.is_empty();
        let check_env = self.check_cargo_target_env(&task, own_tree.then_some(wu.id.as_str()));
        let ws: task_worker::LocalWorkspace = match work_dir {
            Some(w) if w.is_dir() => task_worker::LocalWorkspace::new(&dir).with_work_dir(w),
            _ => task_worker::LocalWorkspace::new(&dir),
        }
        .with_cargo_env(check_env);
        let checks = wu.spec.checks.clone();
        let timeout = self.config.review_timeout;
        let tx = self.tx.clone();
        let checking_run_id = run_id.clone();
        let handle = tokio::spawn(async move {
            let check_results = crate::review::run_work_unit_checks(&ws, &checks, timeout).await;
            let _ = tx.send(Completion::WorkUnitChecks {
                task_id,
                run_id,
                account,
                account_adapter,
                run_since,
                provider,
                result: Box::new(result),
                check_results,
            });
        });
        // Phase F5-fix2: 検査の間も「手元の仕事」として数える（`in_flight`・lease の照合）。
        self.checking
            .insert(checking_run_id, CheckingEntry { task_id, handle });
        Ok(())
    }

    /// ADR-0072 D14/D6・E4 (g): `spawn_work_unit_checks` の結果を受けて、`finish_worker_result` に
    /// 引き継ぐ。1 つでも `pass = false` があれば、この run を `Terminal::Error{retryable: true}`
    /// （WU の retry。`execution_scheduler::decide` が `RunEnd::Failed{retryable:true}` として扱う）に
    /// すり替える。全部 pass なら元の `result`（`Terminal::Done`）をそのまま使う。
    #[allow(clippy::too_many_arguments)]
    fn on_work_unit_checks_finished(
        &mut self,
        task_id: TaskId,
        run_id: String,
        account: Option<String>,
        account_adapter: Option<AccountAdapter>,
        run_since: Option<OffsetDateTime>,
        provider: ProviderId,
        result: Result<RunOutcome, AdapterError>,
        check_results: Vec<(bool, String)>,
    ) -> Result<(), DispatchError> {
        let Some(task) = self.store.get(task_id)? else {
            tracing::warn!(%task_id, %run_id, "work unit checks finished for unknown task");
            return Ok(());
        };
        // ADR-0002 D9 / ADR-0005 D4: checks の実行中にリースが失効・タスクが cancel されていたら、
        // stale worker result と同じ扱いで捨てる。
        let lease_matches = self.run_holds_lease(&task, &run_id)?;
        if !lease_matches {
            tracing::warn!(%task_id, %run_id, status = ?task.status, "stale work unit check result discarded");
            return Ok(());
        }
        let current_wu = self.store.work_units_for(task_id)?.into_iter().find(|u| {
            u.status == task_core::WorkUnitStatus::Running
                && u.last_run_id.as_deref() == Some(run_id.as_str())
        });
        let failed: Vec<&str> = check_results
            .iter()
            .filter(|(pass, _)| !pass)
            .map(|(_, reason)| reason.as_str())
            .collect();
        let result = if failed.is_empty() {
            result
        } else {
            Ok(RunOutcome {
                terminal: Terminal::Error {
                    message: format!("work unit checks failed: {}", failed.join("; ")),
                    retryable: true,
                },
                exit_code: None,
            })
        };
        self.finish_worker_result(
            task,
            current_wu,
            run_id,
            account,
            account_adapter,
            run_since,
            provider,
            result,
        )
    }

    /// `on_worker_finished`（checks が無い、または atomic な run）と `on_work_unit_checks_finished`
    /// （WU の checks が終わった後）の共通の後段。`task`/`current_wu` は呼び出し側が確定させたもの。
    #[allow(clippy::too_many_arguments)]
    fn finish_worker_result(
        &mut self,
        task: Task,
        current_wu: Option<task_core::WorkUnitRow>,
        run_id: String,
        account: Option<String>,
        account_adapter: Option<AccountAdapter>,
        run_since: Option<OffsetDateTime>,
        provider: ProviderId,
        result: Result<RunOutcome, AdapterError>,
    ) -> Result<(), DispatchError> {
        let task_id = task.id;
        let mut subject = ReviewSubject::default();
        // ADR-0013 D9: 供給側失敗なら種別（ProviderThrottled.reason）を、result を消費する前に取っておく。
        let failure_reason = result.as_ref().err().and_then(provider_failure_reason);
        // ADR-0033 D3 / ADR-0034 D2（監査 M-1〜M-3）: 報告の材料も、result を消費する前に取る。
        // `terminal_report` は `Ok(RunOutcome)` の内容（question / worker 自身が返した error）。
        // `adapter_error_text` は `Err(AdapterError)`（アダプタ／供給側の失敗）の表示文字列。
        // どちらも「報告するかどうか」は後で `outcome.next` を見て決める（run の終端ではなくタスクの終端状態）。
        let terminal_report = crate::reports::terminal_report(&result);
        let adapter_error_text = result.as_ref().err().map(|e| e.to_string());
        // ADR-0033 D6（Phase 24）: 結果ファイルの `memory` を、この run の担当の記憶に追記する
        // （run の終わり方に依らず。ファイル I/O だけで、覚える中身を決めるのはワーカー側）。
        self.absorb_memory(&task);
        // ADR-0033 D4 / SPEC §3.1: 部をまたぐ委譲を試みた run は、子を作らずに秘書へ聞く終わり方にする
        // （`StoreSink::delegate_impl` が `QuestionRaised` を残している）。
        let cross_department =
            cross_department_questions_of(&self.store.events_for(task_id)?, &run_id);
        // ADR-0070 D3（Phase 116）: `Trigger::InfraRequeue` を選んだときだけ `Some(n)`（n 回目の
        // インフラ再試行）。`Ok(outcome) =>` の中で `self.infra_backoff` のバックオフ期限を立てるのに使う。
        let mut infra_requeue_n: Option<u32> = None;
        // ADR-0072 D7（Phase E1）: この run の構造化した終わり方（`WorkerFinished.end` に写す）。
        let mut run_end: Option<task_core::RunEnd> = None;
        // ADR-0072 D9: `Terminal::Yielded` の生の checkpoint JSON（`result.json` の `yield`）。
        let mut yield_checkpoint_json: Option<serde_json::Value> = None;
        let (mut trigger, mut outcome_str, usage, provider_outcome) = match result {
            Ok(RunOutcome {
                terminal:
                    Terminal::Done {
                        summary,
                        usage,
                        evidence,
                    },
                ..
            }) => {
                subject = ReviewSubject {
                    summary: summary.clone(),
                    evidence,
                };
                run_end = Some(task_core::RunEnd::Completed);
                (
                    Trigger::WorkerDone,
                    format!("done: {summary}"),
                    usage,
                    ProviderOutcome::Ok,
                )
            }
            // ADR-0033 D4 / Phase 28: 対話 run は `Question` を出さない。人に聞きたいことは返事に書けば
            // よいので、そのまま `Done` 扱いにする（`approvals` の行は作らない。実機で秘書が「最終試行なので
            // 自分の一般知識で答えた」まま `Question` の代わりに走った事故の反省）。
            Ok(RunOutcome {
                terminal: Terminal::Question { text },
                ..
            }) if task_core::is_conversation(&task) => {
                subject = ReviewSubject {
                    summary: text.clone(),
                    evidence: Vec::new(),
                };
                run_end = Some(task_core::RunEnd::Completed);
                (
                    Trigger::WorkerDone,
                    format!("done: {text}"),
                    None,
                    ProviderOutcome::Ok,
                )
            }
            Ok(RunOutcome {
                terminal: Terminal::Question { text },
                ..
            }) => {
                run_end = Some(task_core::RunEnd::Question);
                (
                    Trigger::WorkerQuestion,
                    format!("question: {text}"),
                    None,
                    ProviderOutcome::Ok,
                )
            }
            Ok(RunOutcome {
                terminal: Terminal::Error { message, retryable },
                ..
            }) => {
                // ADR-0072 D7: 二重の安全網。構造化されていない `Error` でも、予算切れの語彙なら
                // `BudgetExhausted` として分類する（usage はこの経路では運べない）。
                run_end = if retryable {
                    classify_budget_kind_from_text(&message)
                        .map(|kind| task_core::RunEnd::BudgetExhausted { kind })
                } else {
                    None
                };
                if run_end.is_none() {
                    run_end = Some(task_core::RunEnd::Failed { retryable });
                }
                (
                    Trigger::WorkerError { retryable },
                    format!("error(retryable={retryable}): {message}"),
                    None,
                    ProviderOutcome::Ok,
                )
            }
            // ADR-0072 D9/D10（Phase E1）: graceful yield（result.json の `{"yield": {...}}`）。
            // `trigger`/`outcome_str` はここでは仮の値で、continuation の判定（下）で確定させる。
            Ok(RunOutcome {
                terminal: Terminal::Yielded { checkpoint, usage },
                ..
            }) => {
                run_end = Some(task_core::RunEnd::Yielded);
                yield_checkpoint_json = Some(checkpoint);
                (
                    Trigger::WorkerError { retryable: true },
                    "error(retryable=true): yielded".to_string(),
                    usage,
                    ProviderOutcome::Ok,
                )
            }
            // ADR-0072 D7（Phase E1）: turn / wall-clock / context の上限に当たった。usage を運ぶ。
            Ok(RunOutcome {
                terminal:
                    Terminal::BudgetExhausted {
                        kind,
                        message,
                        usage,
                    },
                ..
            }) => {
                run_end = Some(task_core::RunEnd::BudgetExhausted { kind });
                (
                    Trigger::WorkerError { retryable: true },
                    format!("error(retryable=true): budget exhausted ({kind:?}): {message}"),
                    usage,
                    ProviderOutcome::Ok,
                )
            }
            Err(e) => match provider_failure_outcome(&e) {
                // ADR-0010 D5（P-21）: 供給側失敗は attempts を消費せず requeue し、プロバイダを cooldown にする。
                Some(po)
                    if consecutive_requeues(&self.store.events_for(task_id)?)
                        < self.config.max_requeues =>
                {
                    (Trigger::Requeue, format!("requeue: adapter: {e}"), None, po)
                }
                // ADR-0011（P-38）: 同じ試行での連続 requeue が上限に達したら、通常の失敗として attempts を消費する。
                Some(po) => (
                    Trigger::WorkerError { retryable: true },
                    format!(
                        "error(retryable=true): requeue limit ({}) reached: adapter: {e}",
                        self.config.max_requeues
                    ),
                    None,
                    po,
                ),
                // ADR-0070 D3（Phase 116）: プロバイダが分類できない失敗（resume 拒否・プロセス
                // I/O・result.json 不在など）は「インフラ都合」として attempts を消費せず、
                // `max_infra_retries` までバックオフして再試行する。上限に達したときだけ
                // `WorkerError{retryable:false}`（無条件に `Failed`）で打ち切り、`"infra failure ×N"`
                // を付ける（D1 の失敗分類がこの接頭辞を見る）。
                None => {
                    let infra_n = consecutive_infra_requeues(&self.store.events_for(task_id)?) + 1;
                    if infra_n <= self.config.max_infra_retries {
                        infra_requeue_n = Some(infra_n);
                        (
                            Trigger::InfraRequeue,
                            format!("infra_requeue: adapter: {e}"),
                            None,
                            ProviderOutcome::Ok,
                        )
                    } else {
                        (
                            Trigger::WorkerError { retryable: false },
                            format!("{INFRA_FAILURE_MARKER}{infra_n}: adapter: {e}"),
                            None,
                            ProviderOutcome::Ok,
                        )
                    }
                }
            },
        };
        // ADR-0072 D7/D8/D9/D11/D18（Phase E1/E2）: 予算切れ・yield の続き（continuation）。
        // checkpoint は常に合成して残す（(b)）。continuation そのものの可否・上限到達の扱いは
        // `[execution]` で決める。無効化・上限到達のときは trigger/outcome_str を従来の形に戻す。
        let mut checkpoint_event: Option<Event> = None;
        // ADR-0072 D5（Phase E2）: `runs` 索引の `finish` に使う（`(g)`。atomic/WU どちらの run にも
        // 書く）。
        let mut checkpoint_for_index: Option<task_core::Checkpoint> = None;
        // ADR-0072 D6（Phase E2）: この run が WU の run なら、その WU の新しい行と、伝播で
        // 一緒に書く他の WU の新しい行（`newly_blocked`/`newly_ready`）、`plan_complete` かどうか。
        let mut wu_update: Option<(
            task_core::WorkUnitRow,
            &'static str,
            Vec<task_core::WorkUnitRow>,
        )> = None;
        // ADR-0079 D7（Phase R3a）: worker が `result.json` の `decisions` で出した決定の要求（記録と止める unit）。
        let mut worker_decisions: Option<WorkerDecisions> = None;
        if let Some(wu) = &current_wu {
            // ADR-0072 D6（Phase E2）: WU の run。`end` が無ければ（分類できない供給側・インフラの
            // 失敗）、harness_error 相当として WU を ready に戻すだけで、Task レベルの trigger は
            // 触らない（既存の Requeue/InfraRequeue/WorkerError の経路のまま。D6 の表どおり）。
            let effective_end = run_end.unwrap_or(task_core::RunEnd::HarnessError {
                class: task_core::HarnessErrorClass::Infra,
            });
            let reset_only = matches!(
                effective_end,
                task_core::RunEnd::HarnessError { .. } | task_core::RunEnd::Cancelled
            );
            let mut checkpoint_opt: Option<task_core::Checkpoint> = None;
            let mut prev_checkpoint_opt: Option<task_core::Checkpoint> = None;
            let mut no_progress_before = 0u32;
            if effective_end.is_continuable() && self.config.execution.continuation {
                let events_so_far = self.store.events_for(task_id)?;
                // ADR-0074 D1.6（Phase F2b）: v2 の WU は WU の worktree・ブランチ・base で取る。
                let (artifacts_dir, cwd_buf, branch, base) =
                    self.work_unit_checkpoint_site(&task, wu);
                let cwd = cwd_buf.as_deref();
                let activity: Vec<crate::checkpoint::ToolActivity> = events_so_far
                    .iter()
                    .filter_map(|(_, ev)| match ev {
                        Event::WorkerProgress {
                            run_id: r,
                            kind: Some(task_core::ProgressKind::ToolUse),
                            tool,
                            summary,
                            ..
                        } if r == &run_id => Some(crate::checkpoint::ToolActivity::Use {
                            tool: tool.clone(),
                            summary: summary.clone(),
                        }),
                        Event::WorkerProgress {
                            run_id: r,
                            kind: Some(task_core::ProgressKind::ToolResult),
                            error,
                            ..
                        } if r == &run_id => {
                            Some(crate::checkpoint::ToolActivity::Result { error: *error })
                        }
                        _ => None,
                    })
                    .collect();
                let mechanical =
                    crate::checkpoint::gather_from(cwd, &branch, base.as_deref(), &activity);
                let worker_checkpoint = yield_checkpoint_json
                    .as_ref()
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .or_else(|| {
                        artifacts_dir
                            .as_deref()
                            .and_then(crate::checkpoint::read_worker_checkpoint)
                    });
                let checkpoint_end = effective_end
                    .as_checkpoint_end()
                    .unwrap_or(task_core::CheckpointEnd::BudgetExhausted);
                let ctx = task_core::CheckpointContext {
                    task_id: task_id.to_string(),
                    work_unit: Some(wu.key.clone()),
                    run_id: run_id.clone(),
                    run_seq: wu.runs,
                    end: checkpoint_end,
                    created_at: rfc3339(OffsetDateTime::now_utc()),
                };
                let checkpoint = task_core::merge_checkpoint(worker_checkpoint, mechanical, ctx);
                prev_checkpoint_opt = latest_checkpoint(&events_so_far, Some(&wu.id));
                no_progress_before = no_progress_streak(&events_so_far, Some(&wu.id));
                checkpoint_event = Some(Event::CheckpointSaved {
                    run_id: run_id.clone(),
                    work_unit_id: Some(wu.id.clone()),
                    checkpoint: Box::new(checkpoint.clone()),
                });
                checkpoint_opt = Some(checkpoint);
            }
            checkpoint_for_index = checkpoint_opt.clone();

            if effective_end.is_continuable() && !self.config.execution.continuation {
                // ADR-0072 §6 (f): `[execution] continuation = false` なら従来どおり
                // `WorkerError{retryable:true}` に戻す（WU の状態は変えない）。
                trigger = Trigger::WorkerError { retryable: true };
                outcome_str = format!(
                    "error(retryable=true): {} (continuation disabled)",
                    describe_run_end(effective_end)
                );
            } else {
                let units = self.store.work_units_for(task_id)?;
                let limits = crate::execution_scheduler::WuLimits {
                    max_continuations: self.config.execution.max_continuations_per_work_unit,
                    no_progress_limit: self.config.execution.no_progress_limit,
                    max_retries: task.budget.max_retries,
                };
                let decision = crate::execution_scheduler::decide(
                    effective_end,
                    &run_id,
                    wu,
                    &units,
                    crate::execution_scheduler::ContinuationInputs {
                        checkpoint: checkpoint_opt.as_ref(),
                        prev_checkpoint: prev_checkpoint_opt.as_ref(),
                        no_progress_before,
                    },
                    limits,
                );
                if !reset_only {
                    trigger = decision.trigger;
                    match decision.reason {
                        "failed" => {
                            let msg = outcome_str
                                .strip_prefix("error(retryable=false): ")
                                .or_else(|| outcome_str.strip_prefix("error(retryable=true): "))
                                .unwrap_or(outcome_str.as_str());
                            outcome_str = format!(
                                "error(retryable=false): work unit {} failed: {msg}",
                                wu.key
                            );
                        }
                        "limit" => {
                            if let Some(question) = &decision.outcome_override {
                                outcome_str = question.clone();
                            }
                        }
                        "continue" => {
                            outcome_str = format!(
                                "continue: {} の続き（WorkUnit {}, Run #{}）",
                                describe_run_end(effective_end),
                                wu.key,
                                wu.runs + 1
                            );
                        }
                        "retry" => {
                            outcome_str = format!(
                                "work_unit_retry: WorkUnit {} を最初からやり直します（{}/{}）",
                                wu.key, decision.updated.retries, limits.max_retries
                            );
                        }
                        // ADR-0072 D17 3.（Phase E4b 項目2）: worker の checkpoint/result.json が
                        // `plan_issue` を書いた。
                        "plan_issue" => {
                            let text = checkpoint_opt
                                .as_ref()
                                .and_then(|cp| cp.plan_issue.clone())
                                .unwrap_or_default();
                            outcome_str = format!(
                                "question: WorkUnit {} が計画の問題を申告しました: {text}",
                                wu.key
                            );
                        }
                        _ => {}
                    }
                    // ADR-0072 D17（Phase E4）/ D17 3.（Phase E4b 項目2）: WU が failed、または
                    // 進捗なし/continuation の上限（"limit"）に達した、または `plan_issue` を
                    // 申告したら、replan の余地（`max_replans`）があれば Task を failed/blocked に
                    // する代わりに replan の planner run を起こす（`ContinueWhy::Replan`）。
                    // D12「失敗にしないもの」: 進捗なし・継続の上限到達は元々失敗にしない。
                    // D12 3.: WU failed は「replan できない」ときだけ failed にする。
                    if matches!(decision.reason, "failed" | "limit" | "plan_issue") {
                        let replans_so_far = self
                            .store
                            .execution_plan_list(task_id)?
                            .len()
                            .saturating_sub(1) as u32;
                        if replans_so_far < self.effective_max_replans(task_id)? {
                            let why = match decision.reason {
                                "failed" => format!("work unit {} failed", wu.key),
                                "limit" => format!("work unit {} made no progress", wu.key),
                                _ => {
                                    let text = checkpoint_opt
                                        .as_ref()
                                        .and_then(|cp| cp.plan_issue.clone())
                                        .unwrap_or_default();
                                    format!("work unit {} reported a plan issue: {text}", wu.key)
                                }
                            };
                            trigger = Trigger::Continue {
                                why: task_core::ContinueWhy::Replan,
                            };
                            outcome_str = format!("replan: {why}");
                        }
                    }
                    if decision.plan_complete {
                        // D15: Task の完了。`ReviewSubject.summary` は WU ごとの最終 checkpoint の
                        // `completed` を key ごとに 1 段落ずつ並べた決定的な要約にする（evidence は
                        // この最後の run のものを残す）。checkpoint が無い WU は「完了」とだけ書く。
                        let mut paragraphs = Vec::new();
                        for u in &units {
                            if u.id == wu.id {
                                continue;
                            }
                            if !u.status.is_active() || u.status != task_core::WorkUnitStatus::Done
                            {
                                continue;
                            }
                            let completed = u
                                .last_run_id
                                .as_deref()
                                .and_then(|rid| self.store.run_index_get(rid).ok().flatten())
                                .and_then(|r| r.checkpoint)
                                .map(|cp| cp.completed.join("; "))
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| "完了".to_string());
                            paragraphs.push(format!("{}: {}", u.spec.title, completed));
                        }
                        let this_completed = checkpoint_for_index
                            .as_ref()
                            .map(|cp| cp.completed.join("; "))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| subject.summary.clone());
                        paragraphs.push(format!("{}: {}", wu.spec.title, this_completed));
                        subject.summary = paragraphs.join("\n");
                    }
                }
                let mut all_new_rows = decision.newly_blocked.clone();
                all_new_rows.extend(decision.newly_ready.clone());
                let mut updated_row = decision.updated;
                let mut wu_reason = decision.reason;
                // ADR-0079 D7（Phase R3a）: 木の節点の leaf の run が `result.json` の `decisions` で人への決定の
                // 要求を出した。記録し（path 付き）、`needed_before: self` ならこの leaf を done にせず
                // `blocked(decision)` にする（答えは次の run の前置きの「人の決定」節に入る）。他の unit を指した決定は
                // その unit だけを止め、この run の完了は妨げない。
                if wu_reason == "completed" && !reset_only {
                    let (site_artifacts, _, _, _) = self.work_unit_checkpoint_site(&task, wu);
                    if let Some(found) =
                        self.worker_decisions(&task, Some(wu), site_artifacts.as_deref(), &run_id)?
                    {
                        if found.self_hold {
                            updated_row.status = task_core::WorkUnitStatus::Blocked;
                            updated_row.blocked_reason =
                                Some(task_core::WorkUnitBlockedReason::Decision);
                            wu_reason = "decision";
                            // この leaf はまだ done ではないので、それを待つ unit は ready にしない。
                            all_new_rows.clear();
                            if matches!(trigger, Trigger::WorkerDone) {
                                trigger = Trigger::Continue {
                                    why: task_core::ContinueWhy::Advance,
                                };
                            }
                            outcome_str = format!(
                                "decision: WorkUnit {} は人の決定を待ちます（決定の要求 {} 件。ADR-0079 D7）",
                                wu.key, found.count
                            );
                        }
                        worker_decisions = Some(found);
                    }
                }
                wu_update = Some((updated_row, wu_reason, all_new_rows));
            }
        } else if let Some(end) = run_end
            && end.is_continuable()
        {
            let events_so_far = self.store.events_for(task_id)?;
            let run_seq = current_run_seq(&events_so_far);
            let workspace_dir = self.task_dir(&task);
            let artifacts_dir = workspace_dir.as_ref().map(|d| self.artifacts_dir(&task, d));
            let workspaces = self.task_workspaces_for(&task);
            let cwd = workspaces.as_ref().and_then(|w| w.cwd());
            let branch = workspaces
                .as_ref()
                .and_then(|w| w.repos.first())
                .and_then(|r| r.branch())
                .unwrap_or_default();
            // ADR-0072 D8: `tests_run`（最大 10 件）と `recent_activity`（最大 20 行）は、この run の
            // `WorkerProgress{kind: tool_use}` と、それに続く `tool_result` の組から作る。
            let activity: Vec<crate::checkpoint::ToolActivity> = events_so_far
                .iter()
                .filter_map(|(_, ev)| match ev {
                    Event::WorkerProgress {
                        run_id: r,
                        kind: Some(task_core::ProgressKind::ToolUse),
                        tool,
                        summary,
                        ..
                    } if r == &run_id => Some(crate::checkpoint::ToolActivity::Use {
                        tool: tool.clone(),
                        summary: summary.clone(),
                    }),
                    Event::WorkerProgress {
                        run_id: r,
                        kind: Some(task_core::ProgressKind::ToolResult),
                        error,
                        ..
                    } if r == &run_id => {
                        Some(crate::checkpoint::ToolActivity::Result { error: *error })
                    }
                    _ => None,
                })
                .collect();
            let mechanical = crate::checkpoint::gather(cwd, branch, &activity);
            // D9: 優先順位は `result.json.yield` > `checkpoint.json` > mechanical。
            let worker_checkpoint = yield_checkpoint_json
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .or_else(|| {
                    artifacts_dir
                        .as_deref()
                        .and_then(crate::checkpoint::read_worker_checkpoint)
                });
            let checkpoint_end = end
                .as_checkpoint_end()
                .unwrap_or(task_core::CheckpointEnd::BudgetExhausted);
            let ctx = task_core::CheckpointContext {
                task_id: task_id.to_string(),
                work_unit: None,
                run_id: run_id.clone(),
                run_seq,
                end: checkpoint_end,
                created_at: rfc3339(OffsetDateTime::now_utc()),
            };
            let checkpoint = task_core::merge_checkpoint(worker_checkpoint, mechanical, ctx);

            if self.config.execution.continuation {
                let continuations_so_far = consecutive_continuations(&events_so_far);
                let prev_checkpoint = latest_checkpoint(&events_so_far, None);
                let progressed =
                    task_core::checkpoint_shows_progress(prev_checkpoint.as_ref(), &checkpoint);
                let no_progress = if progressed {
                    0
                } else {
                    no_progress_streak(&events_so_far, None) + 1
                };
                checkpoint_for_index = Some(checkpoint.clone());
                checkpoint_event = Some(Event::CheckpointSaved {
                    run_id: run_id.clone(),
                    work_unit_id: None,
                    checkpoint: Box::new(checkpoint),
                });
                if continuations_so_far >= self.config.execution.max_continuations_per_work_unit
                    || no_progress >= self.config.execution.no_progress_limit
                {
                    // ADR-0072 D18: 上限到達・進捗なしは失敗にせず、人に聞く（blocked）。
                    trigger = Trigger::WorkerQuestion;
                    outcome_str = format!(
                        "question: 実行が進みません（continuation {continuations_so_far} 回 / 進捗なし {no_progress} 回）。予算を増やして続ける／分割し直す（replan）／中止のいずれかを選んでください。"
                    );
                } else {
                    trigger = Trigger::Continue {
                        why: task_core::ContinueWhy::Continue,
                    };
                    outcome_str = format!(
                        "continue: {} の続き（Run #{}）",
                        describe_run_end(end),
                        run_seq + 1
                    );
                }
            } else {
                // ADR-0072 §6 (f): `[execution] continuation = false` なら従来どおり
                // `WorkerError{retryable:true}` に戻す（checkpoint も保存しない）。
                trigger = Trigger::WorkerError { retryable: true };
                outcome_str = format!(
                    "error(retryable=true): {} (continuation disabled)",
                    describe_run_end(end)
                );
            }
        }
        // ADR-0079 D7（Phase R3a）: 木の節点の atomic の run（WU を持たない）が `result.json` の `decisions` を
        // 書いた。記録し、`needed_before: self` なら最終レビューに進めず `ready` に戻して（`advance`）、答えが
        // 出るまで run を起こさない（`decision_self_hold`。答えは次の run の前置きの `answers` に入る）。
        if current_wu.is_none()
            && matches!(trigger, Trigger::WorkerDone)
            && matches!(run_end, Some(task_core::RunEnd::Completed))
        {
            let artifacts_dir = self.task_dir(&task).map(|d| self.artifacts_dir(&task, &d));
            if let Some(found) =
                self.worker_decisions(&task, None, artifacts_dir.as_deref(), &run_id)?
            {
                if found.self_hold {
                    trigger = Trigger::Continue {
                        why: task_core::ContinueWhy::Advance,
                    };
                    outcome_str = format!(
                        "decision: 人の決定を待ちます（決定の要求 {} 件。ADR-0079 D7）",
                        found.count
                    );
                }
                worker_decisions = Some(found);
            }
        }
        // ADR-0054 D1（Phase 67）: CoS の対話 run（継続セッション）は、この run の usage を
        // `node_sessions.approx_tokens` に積む（rollover 判定の材料。turns も 1 進む）。継続セッションの
        // 対象でない run（`node_sessions` の行が無い）では `node_session_touch` が no-op で返るだけ。
        if task_core::is_conversation(&task) && task.assignee.as_deref() == Some(task_core::COS_ID)
        {
            let tokens = usage
                .as_ref()
                .map(|u| u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0))
                .unwrap_or(0);
            if let Err(e) = self.store.node_session_touch(
                task_core::COS_ID,
                task_core::SessionKind::Conversation,
                None,
                tokens as i64,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(%task_id, error = %e, "failed to record session usage");
            }
        }
        // ADR-0033 D4: 部をまたぐ委譲の質問は、run の自己申告の終わり方より優先する（子は作られていない）。
        // Phase 27: 人に見せる質問は 1 件の部またぎにつき 1 つ（`approvals` の行の単位）。
        let mut questions: Vec<String> = Vec::new();
        if matches!(trigger, Trigger::WorkerQuestion) {
            questions.push(
                outcome_str
                    .strip_prefix("question: ")
                    .unwrap_or(outcome_str.as_str())
                    .to_string(),
            );
        }
        if !cross_department.is_empty() {
            if !matches!(trigger, Trigger::WorkerQuestion) {
                trigger = Trigger::WorkerQuestion;
                outcome_str = format!("question: {}", cross_department.join("\n"));
                subject = ReviewSubject::default();
            }
            questions.extend(cross_department.iter().cloned());
        }
        // ADR-0024 D4 / S10: プール経由の run の失敗は、原因がアカウント側（throttled/auth_failed/exhausted）なら
        // アカウントを cooldown にしプロバイダは cooldown にしない。`Spawn` 失敗（起動できない）はアカウントの
        // 責任ではないので、通常どおりプロバイダを cooldown にする（`failure_reason == Some("spawn")`）。
        let account_at_fault = account.is_some() && failure_reason != Some("spawn");
        let policy_outcome = if account_at_fault {
            ProviderOutcome::Ok
        } else {
            provider_outcome.clone()
        };
        self.policy.report(provider.clone(), &policy_outcome);

        // ADR-0061（Phase 104）: `retries` はこの run が始まった時点でタスクが既に消費していた試行回数
        // （= 遷移前の `task.attempts`）。
        let metrics = run_since.map(|since| task_core::RunMetrics {
            wall_ms: wall_ms_since(since),
            retries: task.attempts,
            peak_context_tokens: None,
            turns: None,
        });
        // ADR-0072 D5/D6/D15（Phase E2）: WU の行の更新（このWU自身 + 伝播で一緒に決まった他のWU）を、
        // Task の trigger の適用とは別に先に書く（既存の `RoutingDecided` 等の慣習と同じ:
        // 別のトランザクションでも監査上の実害は無い。再起動時の照合は `work_units.status` を正とする）。
        // ADR-0074 D1.2（Phase F2b）: v2 の WU が done になったら、WU の作業ツリーで commit する。
        let mut committed_event: Option<Event> = None;
        if let Some((updated_wu, reason, side_effect_rows)) = wu_update.take() {
            let mut updated_wu = updated_wu;
            if updated_wu.phase.is_some() && reason == "completed" {
                committed_event = self.commit_work_unit(&task, &mut updated_wu)?;
            }
            let from = current_wu
                .as_ref()
                .map(|w| w.status)
                .unwrap_or(updated_wu.status);
            if let Err(e) = self.store.work_unit_transition(
                task_id,
                updated_wu.clone(),
                Event::WorkUnitTransitioned {
                    work_unit_id: updated_wu.id.clone(),
                    key: updated_wu.key.clone(),
                    from,
                    to: updated_wu.status,
                    reason: reason.to_string(),
                    run_id: Some(run_id.clone()),
                },
            ) {
                tracing::warn!(%task_id, %run_id, error = %e, "failed to record the work unit transition");
            }
            for row in side_effect_rows {
                // D15: `newly_ready`/`dependents_to_block` の対象は、必ず未着手（`pending`）の
                // WU だけ（依存が未解決な限り `ready` には上がれないため）。
                let dep_reason = if row.status == task_core::WorkUnitStatus::Blocked {
                    "dependency_failed"
                } else {
                    "dependency_ready"
                };
                if let Err(e) = self.store.work_unit_transition(
                    task_id,
                    row.clone(),
                    Event::WorkUnitTransitioned {
                        work_unit_id: row.id.clone(),
                        key: row.key.clone(),
                        from: task_core::WorkUnitStatus::Pending,
                        to: row.status,
                        reason: dep_reason.to_string(),
                        run_id: None,
                    },
                ) {
                    tracing::warn!(%task_id, %run_id, work_unit = %row.key, error = %e, "failed to record a dependent work unit transition");
                }
            }
        }
        // ADR-0079 D7（Phase R3a）: worker の決定の要求（`DecisionRequested`、path 付き）と、それが指した他の unit の
        // `blocked(decision)` を 1 トランザクションで残す（工程の判定〈下の `settle_phase`〉より前）。
        if let Some(found) = worker_decisions.take() {
            tracing::info!(%task_id, %run_id, decisions = found.count, self_hold = found.self_hold, held = found.held_rows.len(), "the worker asked humans for decisions (ADR-0079 D7)");
            if let Err(e) =
                self.store
                    .work_units_apply(task_id, Vec::new(), found.held_rows, found.events)
            {
                tracing::warn!(%task_id, %run_id, error = %e, "failed to record the worker's decision requests");
            }
        }
        // ADR-0072 D5（Phase E2）: `runs` 索引の finish（(g): 全タスクの run について書く）。
        {
            let index_status = run_end
                .map(task_core::RunIndexStatus::from_run_end)
                .unwrap_or(task_core::RunIndexStatus::HarnessError);
            if let Err(e) = self.store.run_index_finish(
                &run_id,
                index_status,
                checkpoint_for_index.clone(),
                usage,
                metrics,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(%task_id, %run_id, error = %e, "failed to finish the runs index row");
            }
        }
        // ADR-0074 D1.6（Phase F2b）: v2 の WU の run。兄弟の WU（または統合）が走っていれば Task は
        // 遷移させない。in-flight が 0 になったら工程の状態（question → 失敗 → 起こせる WU → 統合）で決める。
        let mut v2_settle: Option<crate::execution_scheduler::PhaseSettle> = None;
        if let Some(wu) = &current_wu
            && wu.phase.is_some()
        {
            use crate::execution_scheduler::PhaseSettle;
            let units_now = self.store.work_units_for(task_id)?;
            let settle = crate::execution_scheduler::settle_phase(&units_now);
            match &settle {
                PhaseSettle::Wait | PhaseSettle::Integrate(_) => {
                    // 質問は in-flight が 0 になってから（承認の行もそのときに作る）。
                    questions.clear();
                }
                PhaseSettle::Advance => {
                    if matches!(trigger, Trigger::WorkerDone) {
                        trigger = Trigger::Continue {
                            why: task_core::ContinueWhy::Advance,
                        };
                    }
                }
                PhaseSettle::Question(id) | PhaseSettle::Failure(id) if id != &wu.id => {
                    let (t, _outcome, q) =
                        self.deferred_work_unit_trigger(task_id, &units_now, id)?;
                    trigger = t;
                    questions = q;
                }
                PhaseSettle::AllDone => {
                    trigger = Trigger::WorkerDone;
                }
                _ => {}
            }
            v2_settle = Some(settle);
        }
        let finished = Event::WorkerFinished {
            run_id: run_id.clone(),
            outcome: outcome_str.clone(),
            usage,
            role: None,
            metrics,
            end: run_end,
        };
        let mut events = vec![finished];
        // ADR-0072 D5/D8（Phase E1）: `CheckpointSaved` は `WorkerFinished` と同じトランザクションで残す。
        if let Some(checkpoint_event) = checkpoint_event {
            events.push(checkpoint_event);
        }
        if let Some(ev) = committed_event {
            events.push(ev);
        }
        // ADR-0074 D4（Phase F3 quota）: この run の quota 消費を見積もる（`WorkerFinished` と同じ
        // トランザクションで残す。重なった run のグループがこれで閉じれば、他のメンバー分は
        // `resolve_quota_estimate` の中で別タスクへ直接書く）。
        {
            let model_for_quota = self.started_model_of(task_id, &run_id);
            let quota_event = self.resolve_quota_estimate(
                task_id,
                &run_id,
                current_wu.as_ref().map(|wu| wu.id.clone()),
                account.as_deref(),
                account_adapter,
                &provider,
                &model_for_quota,
                usage.as_ref(),
            );
            events.push(quota_event);
        }
        if let Some(reason) = failure_reason {
            match (&account, account_adapter) {
                // ADR-0024 D4: アカウントの cooldown として記録する（`ProviderThrottled` イベントは出さない）。
                (Some(acct), Some(adapter)) if reason != "spawn" => {
                    self.record_account_failure(adapter, acct, reason, &provider_outcome)
                }
                // ADR-0013 D9 / S10: プールを使わない、または Spawn 失敗（アカウント非依存）はプロバイダの
                // cooldown として、遷移と同じトランザクションで記録する。
                _ => {
                    if let Some(ev) =
                        self.provider_throttled_event(&provider, &provider_outcome, reason)
                    {
                        events.push(ev);
                    }
                }
            }
        }
        // ADR-0033 D5（Phase 26 / Phase 27）: `Question` で終わった run（部をまたぐ委譲の質問への置き換えも
        // 含む）は、既存の `answers[]` の経路（`Status::Blocked`）に加えて `approvals` にも 1 件ずつ残す
        // （部またぎは 1 件の委譲につき 1 行。同じ質問がまだ未決なら増やさない）。
        for text in &questions {
            if let Err(e) = crate::approvals::record_question_approval(
                self.store.as_ref(),
                &task,
                text,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(%task_id, %run_id, error = %e, "failed to record the approval for this question");
            }
        }
        // ADR-0074 D1.6（Phase F2b）: 兄弟が走っている／工程の統合を始めるなら、Task は遷移させない
        // （events だけを残す）。
        if let Some(settle) = &v2_settle
            && matches!(
                settle,
                crate::execution_scheduler::PhaseSettle::Wait
                    | crate::execution_scheduler::PhaseSettle::Integrate(_)
            )
        {
            for ev in &events {
                self.store.append_event(task_id, ev)?;
            }
            tracing::info!(%task_id, %run_id, settle = ?settle, outcome = %outcome_str, "work unit finished (task stays running)");
            if let crate::execution_scheduler::PhaseSettle::Integrate(id) = settle {
                self.start_integration(&task, id)?;
            }
            return Ok(());
        }
        match self
            .store
            .apply_transition_with_events(task_id, trigger, events)
        {
            Ok(outcome) => {
                tracing::info!(%task_id, %run_id, next = ?outcome.next, attempts = outcome.attempts, outcome = %outcome_str, "worker finished");
                // ADR-0070 D3（Phase 116）: `InfraRequeue` は `dispatch_ready` がすぐ拾わないよう、
                // バックオフの期限を立てる（30秒/2分/5分。`infra_backoff_delay`）。
                if let Some(n) = infra_requeue_n {
                    let until = OffsetDateTime::now_utc() + infra_backoff_delay(n);
                    tracing::warn!(%task_id, %run_id, attempt = n, until = %until, "infra failure; requeued with backoff (attempts not consumed; ADR-0070 D3)");
                    self.infra_backoff.insert(task_id, until);
                }
                // ADR-0033 D4（Phase 24 / 監査 M-5）: 対話用タスクの run なら、`summary`（質問なら本文、
                // 失敗なら理由）をそのノードの返事として `messages` に残す。**「返事できませんでした」は
                // タスクが `Failed` に落ちたときだけ**（requeue / まだ試行が残る失敗では書かない）。
                self.record_conversation_reply(&task, &run_id, &outcome_str, outcome.next);
                // ADR-0038 D1 の `milestone_proposal` の取り込みは ADR-0079 D13（Phase R5a）で廃止（途中目標は凍結）。
                // ADR-0034 D2（監査 M-1〜M-3）: `question` は run の終端でそのまま届ける。`done` はレビューを
                // 通って `Status::Done` になってから（`on_review_finished` 側）作るので、ここでは作らない。
                // Phase 28: 対話 run の `Question` は `Done` 扱い（上の match）なので、ここでは報告しない
                // （レビューが通れば `on_review_finished` 側が通常の `Done` 報告を作る）。
                if !task_core::is_conversation(&task)
                    && matches!(
                        terminal_report,
                        Some(crate::reports::TerminalReport::Question { .. })
                    )
                    && let Some(question) = terminal_report.as_ref()
                    && let Err(e) = crate::reports::record_run_report(
                        self.store.as_ref(),
                        &task,
                        &run_id,
                        question,
                        None,
                        OffsetDateTime::now_utc(),
                    )
                {
                    tracing::warn!(%task_id, %run_id, error = %e, "failed to record the report for this run");
                }
                // `bad_news` は `Status::Failed` に遷移したときだけ、原因を問わず作る（ワーカー自身の `error`、
                // 供給側失敗が requeue 上限に達した場合のどちらも含む。監査 M-1）。
                if outcome.next == Status::Failed {
                    let bad_news = match &terminal_report {
                        Some(t @ crate::reports::TerminalReport::Error { .. }) => Some(t.clone()),
                        _ => adapter_error_text.as_ref().map(|message| {
                            crate::reports::TerminalReport::Error {
                                message: message.clone(),
                                retryable: true,
                            }
                        }),
                    };
                    if let Some(terminal) = bad_news.as_ref()
                        && let Err(e) = crate::reports::record_run_report(
                            self.store.as_ref(),
                            &task,
                            &run_id,
                            terminal,
                            None,
                            OffsetDateTime::now_utc(),
                        )
                    {
                        tracing::warn!(%task_id, %run_id, error = %e, "failed to record the report for this run");
                    }
                }
                if outcome.next == Status::Reviewing
                    && !self.spawn_review(task_id, run_id, &subject)?
                {
                    // Reviewer run の枠が無い: 次 tick の recover_reviews で再試行する。
                    self.pending_subjects.insert(task_id, subject);
                }
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(%task_id, %run_id, error = %e, "worker result could not be applied");
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// ADR-0072 D14（Phase E3）: task-local な planner run の終わり方を判定する。`current_wu` が無い
    /// （計画がまだ無い）Task の `RunRole::Planner` run はすべてここを通る。
    ///
    /// `artifacts/execution-plan.json` を D14 で検証し、
    /// - 妥当（かつ run が `Terminal::Done` で終わった）なら採用して `Trigger::Continue{Planned}`。
    /// - 不正・run 自体が異常終了（error/question/budget_exhausted/yielded）なら、1 回だけ再試行する
    ///   （もう一度 planner run を起こす。2 回目もだめなら計画を作らず atomic に倒す。D14「1 回だけ
    ///   再試行し、それでも不正なら atomic に倒す」）。Task は失敗させない（D12）。
    #[allow(clippy::too_many_arguments)]
    fn on_planner_finished(
        &mut self,
        task_id: TaskId,
        task: &Task,
        run_id: String,
        run_since: Option<OffsetDateTime>,
        provider: ProviderId,
        (account, account_adapter): (Option<&str>, Option<AccountAdapter>),
        result: Result<RunOutcome, AdapterError>,
    ) -> Result<(), DispatchError> {
        let now = OffsetDateTime::now_utc();
        // ADR-0013 D9 / S10 と同じ扱い（planner run は account pool のプロバイダを使わない前提。
        // `[execution.planner].adapter` は既定 claude-code で、アカウントの当たり外れは通常の
        // policy 報告に任せる）。
        let policy_outcome = match &result {
            Ok(_) => ProviderOutcome::Ok,
            Err(e) => provider_failure_outcome(e).unwrap_or(ProviderOutcome::Ok),
        };
        self.policy.report(provider.clone(), &policy_outcome);

        let (run_end, describe, usage): (
            Option<task_core::RunEnd>,
            String,
            Option<task_core::Usage>,
        ) = match &result {
            Ok(RunOutcome {
                terminal: Terminal::Done { summary, usage, .. },
                ..
            }) => (
                Some(task_core::RunEnd::Completed),
                format!("done: {summary}"),
                *usage,
            ),
            Ok(RunOutcome {
                terminal: Terminal::Question { text },
                ..
            }) => (
                Some(task_core::RunEnd::Question),
                format!("question: {text}"),
                None,
            ),
            Ok(RunOutcome {
                terminal: Terminal::Error { message, retryable },
                ..
            }) => (
                Some(task_core::RunEnd::Failed {
                    retryable: *retryable,
                }),
                format!("error(retryable={retryable}): {message}"),
                None,
            ),
            Ok(RunOutcome {
                terminal: Terminal::Yielded { usage, .. },
                ..
            }) => (
                Some(task_core::RunEnd::Yielded),
                "yielded (planner runs are not continued; treated as an invalid attempt)"
                    .to_string(),
                *usage,
            ),
            Ok(RunOutcome {
                terminal:
                    Terminal::BudgetExhausted {
                        kind,
                        message,
                        usage,
                    },
                ..
            }) => (
                Some(task_core::RunEnd::BudgetExhausted { kind: *kind }),
                format!("budget_exhausted({kind:?}): {message}"),
                *usage,
            ),
            Err(e) => (None, format!("infra error: {e}"), None),
        };

        // ADR-0076: planner run の quota 消費も worker と同じ `resolve_quota_estimate` で見積もる。
        // 以降の `?` で抜けても `QuotaActivity` が閉じているよう、分岐より前に一度だけ求め、
        // `WorkerFinished` を保存するすべての分岐で同じトランザクションに添える。
        let quota_event = {
            let model = self.started_model_of(task_id, &run_id);
            self.resolve_quota_estimate(
                task_id,
                &run_id,
                None,
                account,
                account_adapter,
                &provider,
                &model,
                usage.as_ref(),
            )
        };

        // ADR-0072 D17（Phase E4）: この run が replan（既に `active` な計画がある）なら、done の
        // WU の key/spec が変わっていないことも検証する（D14「replan のときは done の WU の key と
        // spec が変わっていないこと」）。
        let active_plan = self.store.execution_plan_active(task_id)?;
        // ADR-0074 F5-fix（不具合 2）: daemon が足した WU（`kind = integrate`・統合の repair WU）は
        // planner の視野に無いので対象から外す（`task_ops::execution::replan` と同じ規則）。
        let done_work_units: Vec<(String, task_core::WorkUnitSpec)> = if let Some(active) =
            active_plan.as_ref()
        {
            task_core::replan_done_work_units(&active.spec, &self.store.work_units_for(task_id)?)
        } else {
            Vec::new()
        };
        // D14: 計画の検証は run が `Terminal::Done` で終わったときだけ試みる（それ以外は無条件に
        // 「不正な試行」として扱う）。
        // ADR-0079 D3 / D4 (3)（Phase R2a）: /3 の計画が計画の上限（段階の数・段階あたりの unit・子 task・
        // `max_depth`）だけで不正なら、1 回目は従来どおり不正な試行（planner に理由を渡して再試行）、最後の
        // 試行では上限を緩めて採用し、超えた分の unit を `kind: limit` の決定の要求で止める（黙って切らない・
        // atomic に倒さない）。採用に使う上限を `adopt_limits` に返す。
        let mut adopt_limits = self.config.execution.limits;
        let validation: Result<task_core::execution_plan::ValidatedPlan, String> = if matches!(
            result,
            Ok(RunOutcome {
                terminal: Terminal::Done { .. },
                ..
            })
        ) {
            let workspace_dir = self.task_dir(task);
            let artifacts_dir = workspace_dir.as_ref().map(|d| self.artifacts_dir(task, d));
            match artifacts_dir
                .as_deref()
                .map(|d| d.join("execution-plan.json"))
                .and_then(|p| std::fs::read_to_string(p).ok())
            {
                None => Err("artifacts/execution-plan.json が見つからない".to_string()),
                Some(text) => match parse_planner_output(
                    &text,
                    active_plan.as_ref(),
                    &done_work_units.iter().map(|(k, _)| k.clone()).collect(),
                ) {
                    Err(e) => Err(e),
                    Ok(spec) => match validate_plan_harnesses(&spec, &self.config.genres) {
                        Err(e) => Err(e),
                        Ok(()) => {
                            let ctx = task_core::PlanContext {
                                origin: task_core::PlanOrigin::Planner,
                                depth: task_core::tree::depth_of(task),
                            };
                            match task_core::execution_plan::validate_with(
                                &spec,
                                self.config.execution.limits,
                                &done_work_units,
                                ctx,
                            ) {
                                Ok(v) => Ok(v),
                                Err(errors)
                                    if spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3
                                        && errors.iter().all(is_tree_plan_limit_error)
                                        && self.planner_attempts_in_window(task_id)?
                                            >= MAX_PLANNER_ATTEMPTS =>
                                {
                                    let relaxed =
                                        relaxed_tree_plan_limits(self.config.execution.limits);
                                    match task_core::execution_plan::validate_with(
                                        &spec,
                                        relaxed,
                                        &done_work_units,
                                        ctx,
                                    ) {
                                        Ok(v) => {
                                            tracing::warn!(%task_id, %run_id, errors = %task_ops::execution::describe_validation_errors(&errors), "the /3 plan breaches only the plan limits on the last attempt; adopting it and holding the excess units for a limit decision (ADR-0079 D3)");
                                            adopt_limits = relaxed;
                                            Ok(v)
                                        }
                                        Err(e) => {
                                            Err(task_ops::execution::describe_validation_errors(&e))
                                        }
                                    }
                                }
                                Err(errors) => {
                                    Err(task_ops::execution::describe_validation_errors(&errors))
                                }
                            }
                        }
                    },
                },
            }
        } else {
            Err(format!("planner run did not finish cleanly: {describe}"))
        };

        let metrics = run_since.map(|since| task_core::RunMetrics {
            wall_ms: wall_ms_since(since),
            retries: task.attempts,
            peak_context_tokens: None,
            turns: None,
        });
        let index_status = run_end
            .map(task_core::RunIndexStatus::from_run_end)
            .unwrap_or(task_core::RunIndexStatus::HarnessError);
        if let Err(e) =
            self.store
                .run_index_finish(&run_id, index_status, None, usage, metrics, now)
        {
            tracing::warn!(%task_id, %run_id, error = %e, "failed to finish the runs index row for the planner run");
        }

        match validation {
            Ok(validated) => {
                // ADR-0079 D4 (3)（Phase R2a）: /3 の各 unit に unit の gate をかけ、leaf を task に上げる・
                // task を leaf に下げる（採用する計画の spec に当てる）。子 task にできない深さの compound な
                // leaf と、上限を超える unit は採用の直後に決定の要求で止める（`apply_tree_plan_holds`）。
                let (validated, tree_plan) =
                    self.tree_plan_gate(task, validated, &done_work_units, &mut adopt_limits);
                let outcome_str = format!(
                    "done: {} work unit(s) planned",
                    validated.spec.work_units.len()
                );
                let finished = Event::WorkerFinished {
                    run_id: run_id.clone(),
                    outcome: outcome_str,
                    usage,
                    role: Some(RunRole::Planner),
                    metrics,
                    end: run_end,
                };
                // ADR-0074 D3.7（Phase F4b (f)）: planner の `children` を既存の委譲の検証に通す
                // （初回の採用だけ。replan で新しい子を足すことはしない — 既存の子の key だけを許す）。
                let children = if active_plan.is_some() {
                    let existing: std::collections::BTreeSet<String> = self
                        .store
                        .children(task_id)?
                        .into_iter()
                        .flat_map(|c| c.labels.into_iter())
                        .collect();
                    match validated
                        .spec
                        .children
                        .iter()
                        .find(|c| !existing.contains(&task_core::child_label(&c.key)))
                    {
                        Some(c) => Err(format!(
                            "child {} is new; children can only be added when the plan is first adopted",
                            c.key
                        )),
                        None => Ok(task_ops::delegate::ChildrenPlan::Ready(Vec::new())),
                    }
                } else {
                    task_ops::delegate::plan_children(
                        self.store.as_ref(),
                        task,
                        &validated.spec.children,
                        &self.config.roles,
                        &self.config.genres,
                        &self.config.delegation,
                        now,
                    )
                };
                // ADR-0079 D2 / D4 (4)（Phase R1b）: /3 の kind task の unit は、repos が親の部分集合で
                // あること、部をまたぐ子の認可（ADR-0074 F4b と同じ規則）を採用の前に確かめる。
                let children = match children {
                    Ok(task_ops::delegate::ChildrenPlan::Ready(c))
                        if validated.spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3 =>
                    {
                        self.tree_plan_checks(task, &validated.spec, now)
                            .map(|plan| match plan {
                                task_ops::delegate::ChildrenPlan::Ready(_) => {
                                    task_ops::delegate::ChildrenPlan::Ready(c)
                                }
                                other => other,
                            })
                    }
                    other => other,
                };
                let children = match children {
                    Ok(task_ops::delegate::ChildrenPlan::Ready(children)) => children,
                    Ok(task_ops::delegate::ChildrenPlan::NeedsAuthorization(questions)) => {
                        // SPEC §3.1 / ADR-0033 D4・D5: 部をまたぐ子は秘書（人）への質問。認可されたら
                        // planner をもう一度走らせ、同じ子が認可済みとして通る。
                        let question = questions.join("\n");
                        let progress = Event::worker_progress(run_id.clone(), question.clone());
                        self.store.apply_transition_with_events(
                            task_id,
                            Trigger::WorkerQuestion,
                            vec![finished, quota_event, progress],
                        )?;
                        for q in &questions {
                            if let Err(e) = crate::approvals::record_question_approval(
                                self.store.as_ref(),
                                task,
                                q,
                                now,
                            ) {
                                tracing::warn!(%task_id, error = %e, "failed to record the cross-department approval for planner children");
                            }
                        }
                        return Ok(());
                    }
                    Err(reason) => {
                        self.give_up_or_retry_planner(
                            task_id,
                            task,
                            &run_id,
                            vec![finished, quota_event],
                            format!("子 Task の提案が委譲の検証に通りませんでした: {reason}"),
                            now,
                        )?;
                        return Ok(());
                    }
                };
                let adopted = if active_plan.is_some() {
                    // ADR-0072 D17（Phase E4）: replan。done の WU は保持し、旧版を supersede する。
                    task_ops::execution::replan(
                        self.store.as_ref(),
                        task_id,
                        validated.spec,
                        "replan (planner run)".to_string(),
                        task_core::PlanOrigin::Planner,
                        Some(run_id.clone()),
                        adopt_limits,
                        now,
                    )
                    .map(|(plan, _diff)| plan)
                } else {
                    task_ops::execution::adopt_plan_with_children(
                        self.store.as_ref(),
                        task_id,
                        validated.spec,
                        task_core::PlanOrigin::Planner,
                        Some(run_id.clone()),
                        adopt_limits,
                        now,
                        children,
                    )
                };
                match adopted {
                    Ok(plan) => {
                        if let Some(tree_plan) = tree_plan
                            && let Err(e) =
                                self.apply_tree_plan_holds(task, &plan, &run_id, tree_plan, now)
                        {
                            tracing::warn!(%task_id, %run_id, error = %e, "failed to record the unit gate / tree limit holds of the adopted plan (ADR-0079 R2a)");
                        }
                        // ADR-0079 D8（Phase R3b）: root の /3 の計画は、決定を含む・`review: human` の段階・上限に
                        // 近い、のどれかなら人の承認を待つ（`PlanGate`、unit を 1 つも起こさない）。そうでなければ
                        // 進め、報告の流れに 1 件だけ残す（Discord は鳴らさない。U-R3）。
                        let approval = match self.root_plan_approval(task, &plan) {
                            Ok(a) => a,
                            Err(e) => {
                                tracing::warn!(%task_id, %run_id, error = %e, "failed to evaluate the root plan approval; asking a human to be safe (ADR-0079 D8)");
                                Some(task_core::PlanApproval {
                                    required: true,
                                    reasons: vec![format!("evaluation_failed:{e}")],
                                })
                            }
                        };
                        match approval {
                            Some(a) if a.required => {
                                tracing::info!(%task_id, plan_id = %plan.id, reasons = ?a.reasons, "the root plan needs a human approval (ADR-0079 D8)");
                                self.store.apply_transition_with_events(
                                    task_id,
                                    Trigger::PlanGate {
                                        plan_id: plan.id.clone(),
                                    },
                                    vec![
                                        finished,
                                        quota_event,
                                        Event::PlanApprovalRequested {
                                            plan_id: plan.id.clone(),
                                            reasons: a.reasons,
                                        },
                                    ],
                                )?;
                            }
                            approval => {
                                self.store.apply_transition_with_events(
                                    task_id,
                                    Trigger::Continue {
                                        why: task_core::ContinueWhy::Planned,
                                    },
                                    vec![finished, quota_event],
                                )?;
                                if approval.is_some() {
                                    let (headline, body) =
                                        task_ops::plan_gate::plan_notice(&plan.spec);
                                    if let Err(e) = task_ops::plan_gate::record_plan_notice(
                                        self.store.as_ref(),
                                        task,
                                        &headline,
                                        &body,
                                        now,
                                    ) {
                                        tracing::warn!(%task_id, error = %e, "failed to record the plan notice report (ADR-0079 D8)");
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // 採用そのものが失敗した（既に有効な計画がある等、通常起きない）: 不正な
                        // 試行として retry/give-up の判断に合流させる。
                        tracing::warn!(%task_id, %run_id, error = %e, "failed to adopt the validated plan; treating as an invalid attempt");
                        self.give_up_or_retry_planner(
                            task_id,
                            task,
                            &run_id,
                            vec![finished, quota_event],
                            format!("計画の採用に失敗しました: {e}"),
                            now,
                        )?;
                    }
                }
            }
            Err(reason) => {
                let finished = Event::WorkerFinished {
                    run_id: run_id.clone(),
                    outcome: format!("error(retryable=true): invalid execution plan: {reason}"),
                    usage,
                    role: Some(RunRole::Planner),
                    metrics,
                    end: run_end,
                };
                self.give_up_or_retry_planner(
                    task_id,
                    task,
                    &run_id,
                    vec![finished, quota_event],
                    reason,
                    now,
                )?;
            }
        }
        Ok(())
    }

    /// ADR-0072 D14（Phase E3）/ D17（Phase E4）: planner の出力が不正だった（または run が異常終了
    /// した）ときの、「1 回だけ再試行、それでも駄目なら諦める」の判断。**この「試行」の窓は直近の
    /// `Event::ExecutionPlanned`（無ければ Task の最初）から数える**（E4 の注記: 最初の gate 判定の
    /// planner 試行と、後の replan の planner 試行を混同しない。`Event::WorkerStarted{role: Planner}`
    /// の件数〈この run 自身を含む〉を events から純粋に導出する。D5 と同じ考え方）。
    /// 諦めたときの振る舞いは呼び出し時点の状態で決める: 既に `active` な計画が無ければ fresh
    /// planning の give up（atomic に倒す。D14）、既に `active` な計画があれば replan の give up
    /// （`blocked`。D12「失敗にしないもの」、D17/D18）。Task を `failed` にはしない。
    // `finished` は `WorkerFinished` とそれに添える Event（ADR-0076 の `QuotaEstimated`）。
    /// Phase F5-fix3: 拒否した planner の計画（`artifacts/execution-plan.json`）を
    /// `artifacts/execution-plan.rejected.json` に移す（上書き）。無ければ何もしない。失敗しても警告だけ。
    fn set_aside_rejected_plan(&self, task: &Task) {
        let Some(dir) = self
            .task_dir(task)
            .map(|d| self.artifacts_dir(task, d.as_path()))
        else {
            return;
        };
        let from = dir.join("execution-plan.json");
        if !from.exists() {
            return;
        }
        let to = dir.join(REJECTED_PLAN_FILE);
        if let Err(e) = std::fs::rename(&from, &to) {
            tracing::warn!(task_id = %task.id, error = %e, "failed to set the rejected execution plan aside");
        }
    }

    fn give_up_or_retry_planner(
        &self,
        task_id: TaskId,
        task: &Task,
        run_id: &str,
        finished: Vec<Event>,
        reason: String,
        now: OffsetDateTime,
    ) -> Result<(), DispatchError> {
        // Phase F5-fix3: 拒否した計画のファイルを残すと、次の planner run はそれを見つけて「検証済み」と
        // 思い込みそのまま再提出する（dogfood 4 回目の 2 回目の試行）。`execution-plan.rejected.json` に移す。
        self.set_aside_rejected_plan(task);
        let attempts_so_far = self.planner_attempts_in_window(task_id)?;
        let tree_planner = self.is_tree_planner(task)?;
        if attempts_so_far < MAX_PLANNER_ATTEMPTS {
            let progress = Event::worker_progress(run_id, planner_retry_message(&reason));
            self.store.apply_transition_with_events(
                task_id,
                Trigger::Continue {
                    why: task_core::ContinueWhy::Planned,
                },
                finished.into_iter().chain([progress]).collect(),
            )?;
        } else if tree_planner {
            // ADR-0079 D9（Phase R2b）: /3 の planner（木の節点）の計画が 2 回とも不正だった。atomic に倒さず
            // （分けると決めた仕事を黙って 1 run に潰さない）、replan でも自由文の質問にせず、`kind: plan_invalid`
            // の決定の要求を出す。Task は `ready` に戻し、決定が開いている間は run を起こさない
            // （`plan_invalid_hold`。R2a の木の上限の止め方と同じ）。回答の入口は R3a。
            let mut errors = planner_rejections_since_last_plan(&self.store.events_for(task_id)?);
            if !errors.iter().any(|e| e == &reason) {
                errors.push(reason.clone());
            }
            let replan = self.store.execution_plan_active(task_id)?.is_some();
            let path =
                task_ops::tree::decision_path(self.store.as_ref(), task).map_err(ops_to_store)?;
            let request = task_core::tree::plan_invalid_decision(
                task_id,
                replan,
                &errors,
                path,
                task_core::DecisionRaisedBy {
                    task_id,
                    run_id: Some(run_id.to_string()),
                    origin: task_core::DecisionOrigin::Daemon,
                },
            );
            tracing::warn!(%task_id, %run_id, decision = %request.id, replan, "the /3 plan was invalid twice; asking a human (plan_invalid) instead of falling back to atomic (ADR-0079 D9)");
            let progress = Event::worker_progress(
                run_id,
                format!(
                    "計画（/3）を 2 回とも採用できませんでした（{reason}）。atomic には倒さず、人の決定（plan_invalid）を待ちます（ADR-0079 D9）。"
                ),
            );
            let mut events: Vec<Event> = finished.into_iter().chain([progress]).collect();
            if self.open_plan_invalid(task)?.is_none() {
                events.push(Event::DecisionRequested {
                    decision: Box::new(request),
                });
            }
            self.store.apply_transition_with_events(
                task_id,
                Trigger::Continue {
                    why: task_core::ContinueWhy::Planned,
                },
                events,
            )?;
        } else if self.store.execution_plan_active(task_id)?.is_some() {
            // ADR-0072 D17（Phase E4）: これは replan の planner run（既に `active` な計画がある）。
            // 2 回とも不正だったので、直せないまま突き進まず人に聞く（`blocked`。D12「失敗にしない
            // もの」の一つ。atomic への書き換えはしない — 既に WU の履歴がある計画を捨てるのは
            // 安全ではない）。
            let question = planner_blocked_question(&reason);
            let progress = Event::worker_progress(run_id, question.clone());
            self.store.apply_transition_with_events(
                task_id,
                Trigger::WorkerQuestion,
                finished.into_iter().chain([progress]).collect(),
            )?;
            if let Err(e) = crate::approvals::record_question_approval(
                self.store.as_ref(),
                task,
                &question,
                now,
            ) {
                tracing::warn!(%task_id, error = %e, "failed to record the approval for the replan question");
            }
        } else {
            // D14: それでも不正なら atomic に倒す（暗黙の WU で実行する）。gate の判定を Atomic に
            // 書き換えて監査に残す（`Task.routing.execution` を書き換えないと、次の dispatch でまた
            // planner run を起こそうとしてしまう）。
            if let Some(mut routing) = task.routing.clone()
                && let Some(mut decision) = routing.execution.clone()
            {
                decision.mode = task_core::ExecutionMode::Atomic;
                decision.rule_id = "atomic/planner-invalid".to_string();
                decision.signals.push(task_core::GateSignal {
                    name: "planner_retry_exhausted".to_string(),
                    weight: 0,
                    detail: format!(
                        "{attempts_so_far} planner attempts failed; falling back to atomic: {reason}"
                    ),
                });
                routing.execution = Some(decision.clone());
                let mut fresh = task.clone();
                fresh.routing = Some(routing);
                fresh.updated_at = now;
                if let Err(e) = self.store.update_task(
                    &fresh,
                    Event::ExecutionGated {
                        decision: Box::new(decision),
                    },
                ) {
                    tracing::warn!(%task_id, error = %e, "failed to record the atomic fallback after planner retries were exhausted");
                }
            }
            let progress = Event::worker_progress(
                run_id,
                format!("計画を採用できず直接実行に切り替えました（{reason}）。"),
            );
            self.store.apply_transition_with_events(
                task_id,
                Trigger::Continue {
                    why: task_core::ContinueWhy::Planned,
                },
                finished.into_iter().chain([progress]).collect(),
            )?;
        }
        Ok(())
    }

    /// ADR-0033 D6（Phase 24）: 結果ファイル（`<artifacts_dir>/result.json`）の `memory` を担当の記憶に追記する。
    /// `[memory]` を設定していない・担当がいない・`memory` が無いときは何もしない。失敗しても run は壊さない。
    fn absorb_memory(&self, task: &Task) {
        let (Some(dir), Some(node_id)) = (&self.config.memory_dir, task.assignee.as_deref()) else {
            return;
        };
        let Some(workspace) = self.task_dir(task) else {
            return;
        };
        // ADR-0036 D2: 結果ファイルはそのタスクの成果物ディレクトリの中。
        let Some(update) = task_worker::read_result_memory(&self.artifacts_dir(task, &workspace))
        else {
            return;
        };
        let today = OffsetDateTime::now_utc().date().to_string();
        let project = task.project_id.map(|p| p.to_string());
        if let Err(e) = MemoryDir::new(dir).append(node_id, project.as_deref(), &update, &today) {
            tracing::warn!(task_id = %task.id, error = %e, "failed to append to the node's memory");
        }
    }

    /// ADR-0033 D4（Phase 24 / 監査 M-5）: 対話用タスクの run の終わりを、そのノードの返事として
    /// `messages` に残す。`outcome_str` は `done: <summary>` / `question: <text>` /
    /// `error(...): <message>` のいずれか。`next` は遷移後の状態で、**失敗の返事は `Failed` のときだけ**
    /// 書く（retryable な途中失敗や requeue では、同じ問いに何度も「返事できませんでした」が並ばない）。
    fn record_conversation_reply(
        &self,
        task: &Task,
        run_id: &str,
        outcome_str: &str,
        next: Status,
    ) {
        if !task_core::is_conversation(task) {
            return;
        }
        let mut metadata = None;
        let text = if let Some(summary) = outcome_str.strip_prefix("done: ") {
            let mut text = summary.to_string();
            // ADR-0048 D3（Phase 60b）: CoS が `done` で返ってきたときだけ、結果ファイルの `actions` を
            // 決定的に実行する。実行できなかった action があれば返事に節を足し、実行結果は metadata に残す。
            if let Some(outcome) = self.absorb_console_actions(task, run_id) {
                if let Some(note) = outcome.failure_note() {
                    text.push_str(&note);
                }
                metadata = outcome.to_metadata();
            }
            text
        } else if let Some(question) = outcome_str.strip_prefix("question: ") {
            // 質問は人への問いかけそのものなので、返事としてもそのまま見せる（`approvals` にも 1 行入る）。
            question.to_string()
        } else if next == Status::Failed {
            task_core::failure_reply(outcome_str)
        } else {
            return;
        };
        if let Err(e) = task_ops::conversation::record_reply_with_metadata(
            self.store.as_ref(),
            task,
            run_id,
            &text,
            metadata,
            OffsetDateTime::now_utc(),
        ) {
            tracing::warn!(task_id = %task.id, error = %e, "failed to record the conversation reply");
        }
    }

    /// ADR-0048 D3（Phase 60b）: CoS（根ノード。`OrgKind::Secretary`）の対話 run の結果ファイルの
    /// `actions` を決定的に実行する。CoS 以外の対話・対話でない run・宣言が無い run では何もしない
    /// （`None`）。冪等（`task_ops::actions::execute` が `run_id` を記録し、2 回目は `None`）。
    /// 失敗しても run は壊さない。
    fn absorb_console_actions(
        &self,
        task: &Task,
        run_id: &str,
    ) -> Option<task_ops::actions::ActionsOutcome> {
        if !task_core::is_conversation(task) {
            return None;
        }
        let assignee = task.assignee.as_deref()?;
        let org = self.store.org_list().ok()?;
        let node = org.iter().find(|n| n.id == assignee)?;
        if node.kind != OrgKind::Secretary {
            return None;
        }
        let workspace = self.task_dir(task)?;
        let artifacts_dir = self.artifacts_dir(task, &workspace);
        let parsed = task_worker::read_result_actions(&artifacts_dir);
        if parsed.is_empty() {
            return None;
        }
        // Phase 98（ADR-0018）: `create_task.workspace` がクラスタを指すときに `[[clusters]]` へ照らして
        // 検証するため、既知のクラスタ id を渡す。
        let known_clusters: Vec<String> = self.config.clusters.keys().cloned().collect();
        match task_ops::actions::execute(
            self.store.as_ref(),
            &org,
            &self.config.roles,
            &self.config.genres,
            &known_clusters,
            task,
            run_id,
            &parsed.valid,
            &parsed.malformed,
            OffsetDateTime::now_utc(),
        ) {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to execute console actions");
                None
            }
        }
    }

    fn on_review_finished(
        &mut self,
        task_id: TaskId,
        run_id: String,
        mut outcome: ReviewOutcome,
    ) -> Result<(), DispatchError> {
        let entry = self.reviewing.remove(&task_id);
        // ADR-0014 D1: Reviewer run の終わりを WorkerFinished{role: reviewer} として残す（判定の適用・延期・破棄のどれでも）。
        let completed_review_run = outcome.reviewer_run.as_ref().map(|r| r.run_id.clone());
        // ADR-0061（Phase 104）: `retries` は Reviewer run には無い概念（対象タスクの `attempts` とは別軸）
        // なので 0 固定。wall time は `ReviewEntry.since` から計算する。
        let review_metrics = entry.as_ref().map(|e| task_core::RunMetrics {
            wall_ms: wall_ms_since(e.since),
            retries: 0,
            peak_context_tokens: None,
            turns: None,
        });
        let mut reviewer_finished = outcome.reviewer_run.take().map(|r| Event::WorkerFinished {
            run_id: r.run_id,
            outcome: r.outcome,
            usage: r.usage,
            role: Some(RunRole::Reviewer),
            metrics: review_metrics,
            end: None,
        });
        // ADR-0076: Reviewer run の quota 消費も worker と同じ `resolve_quota_estimate` で一度だけ
        // 見積もる（対象タスクが消えた・stale でも `QuotaActivity` を閉じるため、分岐より前）。
        // Reviewer run を起こさなかったレビュー（command 等だけ）には作らない。
        let reviewer_quota = match (&completed_review_run, entry.as_ref()) {
            (Some(review_run_id), Some(e)) => e.provider.clone().map(|provider| {
                let model = self.started_model_of(task_id, review_run_id);
                let usage = worker_finished_usage(&reviewer_finished);
                self.resolve_quota_estimate(
                    task_id,
                    review_run_id,
                    None,
                    e.account.as_deref(),
                    e.account_adapter,
                    &provider,
                    &model,
                    usage.as_ref(),
                )
            }),
            _ => None,
        };
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        // ADR-0054 D1（Phase 67）: この run が Reviewer run を伴っていれば、対象タスクの部署の根ノード
        // （engineering/research/operations）の継続セッション（`kind = lead`）に usage を積む（rollover
        // 判定の材料。turns も 1 進む）。部署が無い・対応しないアダプタでは `node_session_touch` が no-op。
        if let Some(Event::WorkerFinished { usage, .. }) = &reviewer_finished
            && let Some(node_id) = task.assignee.as_deref()
        {
            match self.store.org_list() {
                Ok(org) => {
                    if let Some(department) = task_core::department_of(&org, node_id) {
                        let tokens = usage
                            .as_ref()
                            .map(|u| u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0))
                            .unwrap_or(0);
                        if let Err(e) = self.store.node_session_touch(
                            &department,
                            task_core::SessionKind::Lead,
                            None,
                            tokens as i64,
                            OffsetDateTime::now_utc(),
                        ) {
                            tracing::warn!(%task_id, error = %e, "failed to record department lead session usage");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(%task_id, error = %e, "failed to list org for department lead session touch");
                }
            }
        }
        if task.status != Status::Reviewing {
            tracing::warn!(%task_id, status = ?task.status, "review result discarded (task no longer reviewing)");
            set_worker_finished_end(&mut reviewer_finished, task_core::RunEnd::Cancelled);
            for ev in reviewer_finished.iter().chain(reviewer_quota.iter()) {
                self.store.append_event(task_id, ev)?;
            }
            finish_reviewer_run_index(
                self.store.as_ref(),
                &completed_review_run,
                task_core::RunIndexStatus::Cancelled,
                worker_finished_usage(&reviewer_finished),
                review_metrics,
            );
            return Ok(());
        }
        let mut throttled_events = Vec::new();
        if let Some(pf) = outcome.provider_failure.take() {
            // ADR-0054 D2（Phase 113）: `pf.outcome = Some(..)` はプロバイダが分類できた供給側失敗
            // （Throttled/Exhausted/AuthFailed）で、従来どおりプロバイダ/アカウントの cooldown に
            // 報告する。`None` は「reviewer run 自身のインフラ都合の失敗」（is_error の結果・
            // プロセス失敗・resume 拒否など）で、こちらは cooldown の対象にしない
            // （プロバイダ・アカウントの問題ではなく、たまたまこの run が失敗しただけのため）。
            if let Some(classified) = pf.outcome.clone() {
                let review_account = entry.as_ref().and_then(|e| e.account.clone());
                let review_account_adapter = entry.as_ref().and_then(|e| e.account_adapter);
                if let Some(provider) = entry.as_ref().and_then(|e| e.provider.clone()) {
                    // ADR-0024 D4: プール経由の Reviewer run の失敗もプロバイダを cooldown にせず、アカウントに向ける。
                    let policy_outcome = if review_account.is_some() {
                        ProviderOutcome::Ok
                    } else {
                        classified.clone()
                    };
                    self.policy.report(provider.clone(), &policy_outcome);
                    match (&review_account, review_account_adapter) {
                        (Some(acct), Some(adapter)) => self.record_account_failure(
                            adapter,
                            acct,
                            cooldown_reason_name(&classified),
                            &classified,
                        ),
                        _ => {
                            if let Some(ev) = self.provider_throttled_event(
                                &provider,
                                &classified,
                                cooldown_reason_name(&classified),
                            ) {
                                throttled_events.push(ev);
                            }
                        }
                    }
                }
            }
            // ADR-0054 D2（Phase 113）: 分類できた供給側失敗は従来どおり `max_requeues` /
            // `REVIEWER_REQUEUED_PREFIX` で数える。reviewer run 自身のインフラ都合の失敗は、
            // 別のカウンタ・別の上限（`[review] max_reviewer_retries`、既定 3。criterion ごとではなく
            // この reviewing 試行での連続失敗回数だが、Reviewer 条件は同じ run でまとめて判定される
            // ため実質的に criterion ごとの回数と一致する）で数え、プロバイダの cooldown 回数とは
            // 混ぜない。
            let is_infra_failure = pf.outcome.is_none();
            let (deferrals, limit, prefix) = if is_infra_failure {
                (
                    consecutive_reviewer_infra_failures(&self.store.events_for(task_id)?),
                    self.config.max_reviewer_retries,
                    REVIEWER_INFRA_FAILURE_PREFIX,
                )
            } else {
                (
                    consecutive_reviewer_requeues(&self.store.events_for(task_id)?),
                    self.config.max_requeues,
                    REVIEWER_REQUEUED_PREFIX,
                )
            };
            if deferrals < limit {
                // ADR-0010 D5（P-29）/ ADR-0054 D2: Reviewer run の供給側・インフラ失敗は判定しない。
                // reviewing のまま次 tick に回す（attempts を消費しない）。
                self.store.append_event(
                    task_id,
                    &Event::worker_progress(run_id.clone(), format!("{prefix}{}", pf.message)),
                )?;
                // (k): event 自身の `end` も `HarnessError` に揃える（供給側 = Supply、reviewer run
                // 自身のインフラ都合 = Infra）。
                set_worker_finished_end(
                    &mut reviewer_finished,
                    task_core::RunEnd::HarnessError {
                        class: if is_infra_failure {
                            task_core::HarnessErrorClass::Infra
                        } else {
                            task_core::HarnessErrorClass::Supply
                        },
                    },
                );
                for ev in reviewer_finished.iter().chain(reviewer_quota.iter()) {
                    self.store.append_event(task_id, ev)?;
                }
                for ev in &throttled_events {
                    self.store.append_event(task_id, ev)?;
                }
                finish_reviewer_run_index(
                    self.store.as_ref(),
                    &completed_review_run,
                    task_core::RunIndexStatus::HarnessError,
                    worker_finished_usage(&reviewer_finished),
                    review_metrics,
                );
                if let Some(entry) = entry {
                    self.pending_subjects.insert(task_id, entry.subject);
                }
                tracing::warn!(%task_id, %run_id, reason = %pf.message, infra = is_infra_failure, "reviewer run hit a provider/infra failure; review deferred");
                return Ok(());
            }
            // ADR-0011（P-38）/ ADR-0054 D2: 連続延期・連続インフラ失敗が上限に達したら、未判定の
            // Reviewer 条件を fail として通常どおり判定を適用する（人の承認・command・artifact_exists
            // の結果は `outcome.verdicts` に既に入っているのでそのまま残る）。
            let limit_message = if is_infra_failure {
                format!("reviewer infra failure ×{limit}: {}", pf.message)
            } else {
                format!("requeue limit ({limit}) reached: {}", pf.message)
            };
            tracing::warn!(%task_id, %run_id, reason = %pf.message, limit, infra = is_infra_failure, "reviewer run retry limit reached; failing reviewer criteria");
            if let Some(Event::WorkerFinished {
                outcome: finished_outcome,
                ..
            }) = reviewer_finished.as_mut()
            {
                *finished_outcome = format!("error(retryable=false): {limit_message}");
            }
            for (idx, criterion) in task.acceptance.iter().enumerate() {
                if matches!(criterion.check, Check::Reviewer)
                    && !outcome.verdicts.iter().any(|v| v.criterion_idx == idx)
                {
                    outcome.verdicts.push(Verdict {
                        criterion_idx: idx,
                        pass: false,
                        reason: limit_message.clone(),
                        repair_hint: None,
                    });
                }
            }
            outcome.verdicts.sort_by_key(|v| v.criterion_idx);
        }
        let all_pass = outcome.all_pass();
        if let Some(old) = self.store.delivery_get(task_id)?
            && old.worker_run == run_id
            && completed_review_run.as_deref() == Some(old.review_run.as_str())
            && old.state == task_core::DeliveryState::Reviewing
            && let Some(verdict) = outcome
                .verdicts
                .iter()
                .find(|v| v.criterion_idx == old.criterion_idx)
        {
            let mut next = old.clone();
            next.decision = Some(all_pass && verdict.pass);
            next.state = if all_pass && verdict.pass {
                task_core::DeliveryState::MergeQueued
            } else {
                task_core::DeliveryState::Blocked
            };
            next.detail = outcome
                .verdicts
                .iter()
                .filter(|v| !v.pass || v.criterion_idx == old.criterion_idx)
                .map(|v| v.reason.clone())
                .collect::<Vec<_>>()
                .join("\n");
            let review_reason = verdict
                .reason
                .strip_prefix(&format!("reviewer({}): ", old.review_run))
                .unwrap_or(&verdict.reason);
            if review_reason.trim_start().starts_with("[needs-human]") {
                next.detail = format!("{}\n{}", review_reason, next.detail);
            }
            self.store.delivery_save(Some(&old), &next)?;
        }

        // ADR-0034 D2（監査 M-1〜M-3）: レビュー不合格の理由（この run が `Status::Failed` に直結した場合の
        // bad_news の材料。`Status::Ready` に戻るだけの途中の失敗では使わない）。
        let review_fail_message = if all_pass {
            None
        } else {
            let reasons: Vec<String> = outcome
                .verdicts
                .iter()
                .filter(|v| !v.pass)
                .map(|v| v.reason.clone())
                .collect();
            Some(reasons.join("; "))
        };
        let reviewer_index_status = match &reviewer_finished {
            Some(Event::WorkerFinished { outcome, .. })
                if outcome.starts_with("error(retryable=false)") =>
            {
                task_core::RunIndexStatus::Failed
            }
            _ => task_core::RunIndexStatus::Completed,
        };
        // ADR-0074 §6 F1 (k): event 自身の `end` も同じ判定に揃える（`replay --check` が
        // `rebuild_work_units_and_runs` で正しく `status` を復元できるように）。
        set_worker_finished_end(
            &mut reviewer_finished,
            match reviewer_index_status {
                task_core::RunIndexStatus::Failed => task_core::RunEnd::Failed { retryable: false },
                _ => task_core::RunEnd::Completed,
            },
        );
        finish_reviewer_run_index(
            self.store.as_ref(),
            &completed_review_run,
            reviewer_index_status,
            worker_finished_usage(&reviewer_finished),
            review_metrics,
        );
        let mut events: Vec<Event> = reviewer_finished
            .into_iter()
            .chain(reviewer_quota)
            .chain(outcome.verdicts.iter().map(|v| Event::ReviewVerdict {
                run_id: run_id.clone(),
                criterion_idx: v.criterion_idx,
                pass: v.pass,
                reason: v.reason.clone(),
            }))
            .chain(throttled_events)
            .collect();
        // ADR-0016 D2 / M5: 全 pass でも委譲した子が終端でなければ、判定だけ記録して reviewing のまま待つ。
        if all_pass {
            let pending = pending_children(self.store.as_ref(), task_id).map_err(ops_to_store)?;
            if pending > 0 {
                for ev in &events {
                    self.store.append_event(task_id, ev)?;
                }
                self.store.append_event(
                    task_id,
                    &Event::worker_progress(
                        run_id.clone(),
                        format!("waiting for {pending} delegated child task(s) before completing"),
                    ),
                )?;
                self.awaiting_children.insert(
                    task_id,
                    AwaitingChildren {
                        run_id: run_id.clone(),
                        plan: outcome.plan,
                    },
                );
                tracing::info!(%task_id, %run_id, pending, "review passed; waiting for delegated children");
                return Ok(());
            }
            // ADR-0021 D1: 子が失敗していたら、集約・完了より先に「やり直す or 人に聞く」。
            if self.escalate_failed_children(&task, &run_id, &mut events)? {
                return Ok(());
            }
            // ADR-0016 D3 / M4: 子が全て終端で、まだ集約 run をしていなければ集約 run を予約する。
            if self.needs_aggregate_run(&task)? {
                return self.schedule_aggregate_run(task_id, &run_id, events);
            }
        }
        // ADR-0072 D16（Phase E4）: 修復できる不合格（class が揃っていて、repair の上限内）なら、
        // `ReviewFail` の代わりに `ReviewRepair`（attempts 据え置き）+ 最小の context の repair WU で
        // 直す。上限を超えている・修復できない種類が混ざっていれば `None`（従来どおり `ReviewFail` へ）。
        if !all_pass
            && task.kind == TaskKind::Execute
            && let Some(repair_outcome) =
                self.try_review_repair(&task, &outcome.verdicts, &mut events)?
        {
            tracing::info!(%task_id, %run_id, next = ?repair_outcome.next, "review failed but repaired locally (ADR-0072 D16)");
            return Ok(());
        }
        // ADR-0007 D3/D4: Plan が全 pass なら子タスクの挿入と ReviewPass を同一トランザクションで行う。
        let result = match (all_pass, task.kind, outcome.plan) {
            (true, TaskKind::Plan, Some(mut plan)) => {
                let org = self.store.org_list()?;
                self.fix_plan_for_harness(&task, &mut plan, &org);
                // ADR-0039 D2: 子の作業場所は 明示 > 案件 > 親。
                let project_workspace =
                    task_ops::delegate::project_workspace(self.store.as_ref(), &task)
                        .map_err(ops_to_store)?;
                // ADR-0043 D2: 子のリポジトリは 明示（計画の `repos`）> 親 > 案件の primary。
                let project_repos = task_ops::delegate::project_repos(self.store.as_ref(), &task)
                    .map_err(ops_to_store)?;
                let home = task_core::home_dir();
                let workspace = task_core::WorkspaceContext {
                    project: project_workspace.as_ref(),
                    home: home.as_deref(),
                    repos: &project_repos,
                };
                let children = materialize_logging(
                    &task,
                    &plan,
                    &org,
                    &self.config.roles,
                    &self.config.genres,
                    workspace,
                    OffsetDateTime::now_utc(),
                    &mut |child_id, reason| {
                        tracing::info!(task_id = %task_id, child_id = %child_id, %reason, "workspace downgraded to local (ADR-0062 B2)");
                    },
                );
                let n = children.len();
                let r = self.store.complete_plan(
                    task_id,
                    events,
                    children,
                    self.config.plan_auto_accept,
                );
                if r.is_ok() {
                    tracing::info!(%task_id, %run_id, children = n, auto_accept = self.config.plan_auto_accept, "plan completed; children inserted");
                }
                r
            }
            // ADR-0079 D13（Phase R5a）: 案件計画（マイルストーン DAG）の run の提案の取り込み
            // （`finish_project_plan_run`）は廃止。新しく作る入口が無く（`POST /projects/{id}/plan` は 410）、
            // 残っていても下の Plan kind の失敗と同じに扱う（提案を作らない）。
            (true, TaskKind::Plan, None) => {
                // review_task は Plan kind に必ず暗黙の判定を付けるので、ここには来ないはず。
                tracing::error!(%task_id, "plan review passed without a parsed plan; treating as failure");
                self.store
                    .apply_transition_with_events(task_id, Trigger::ReviewFail, events)
            }
            (true, _, _) => {
                self.store
                    .apply_transition_with_events(task_id, Trigger::ReviewPass, events)
            }
            (false, _, _) => {
                self.store
                    .apply_transition_with_events(task_id, Trigger::ReviewFail, events)
            }
        };
        match result {
            Ok(outcome) => {
                tracing::info!(%task_id, %run_id, all_pass, next = ?outcome.next, attempts = outcome.attempts, "review finished");
                // ADR-0034 D2（監査 M-1〜M-3）: `result` はレビューを通って `Status::Done` になったときだけ作る
                // (ワーカーの「できました」がここで差し戻された分は報告にしない。DESIGN 原則 4)。
                if outcome.next == Status::Done
                    && let Some(review_entry) = entry.as_ref()
                {
                    let terminal = crate::reports::TerminalReport::Done {
                        summary: review_entry.subject.summary.clone(),
                        evidence: crate::reports::format_evidence(&review_entry.subject.evidence),
                    };
                    // ADR-0034 D7: ワーカーが結果ファイルで宣言した `report.kind`（無ければ既定の `result`）。
                    let declared = self.task_dir(&task).and_then(|ws| {
                        task_worker::read_result_report_kind(&self.artifacts_dir(&task, &ws))
                    });
                    if let Err(e) = crate::reports::record_run_report(
                        self.store.as_ref(),
                        &task,
                        &run_id,
                        &terminal,
                        declared.as_deref(),
                        OffsetDateTime::now_utc(),
                    ) {
                        tracing::warn!(%task_id, %run_id, error = %e, "failed to record the report for this run");
                    }
                } else if outcome.next == Status::Failed
                    && let Some(message) = review_fail_message.as_ref()
                {
                    // レビュー不合格が retry を使い切って `Status::Failed` になった場合の bad_news（原因を問わない。監査 M-1）。
                    let terminal = crate::reports::TerminalReport::Error {
                        message: message.clone(),
                        retryable: false,
                    };
                    if let Err(e) = crate::reports::record_run_report(
                        self.store.as_ref(),
                        &task,
                        &run_id,
                        &terminal,
                        None,
                        OffsetDateTime::now_utc(),
                    ) {
                        tracing::warn!(%task_id, %run_id, error = %e, "failed to record the report for this run");
                    }
                }
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(%task_id, error = %e, "review result could not be applied");
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// `work_units.spec.title` の `"repair (<bucket>): …"` から bucket 名を読む（repair の per-class
    /// カウンタ用。D16 は `WorkUnitSpec` に専用の欄を足さない設計なので、`try_review_repair` が書いた
    /// title を決定的に読み戻す）。
    fn repair_bucket_of_title(title: &str) -> Option<&str> {
        title.strip_prefix("repair (")?.split(')').next()
    }

    /// ADR-0072 D16（Phase E4）: 最終レビューの不合格を分類し、修復できて上限内なら repair WU を
    /// 実体化して `Trigger::ReviewRepair` を適用する（`Some` を返す）。修復できない・上限を超えて
    /// いれば `events` に触れずに `None` を返す（呼び出し側が従来どおり `ReviewFail` へ進む）。
    fn try_review_repair(
        &self,
        task: &Task,
        verdicts: &[Verdict],
        events: &mut Vec<Event>,
    ) -> Result<Option<task_core::Outcome>, DispatchError> {
        let task_id = task.id;
        let failing: Vec<task_core::FailedCheck> = verdicts
            .iter()
            .filter(|v| !v.pass)
            .map(|v| {
                let check = task
                    .acceptance
                    .get(v.criterion_idx)
                    .map(|c| c.check.clone())
                    // 暗黙の条件（Plan/aggregate/repo_checks/research）は task.acceptance に無く、
                    // D16 の分類表にも無いので修復できない（`Check::Human` と同じ扱いに倒す）。
                    .unwrap_or(task_core::Check::Human);
                task_core::FailedCheck {
                    check,
                    reason: v.reason.clone(),
                    repair_hint: v.repair_hint.clone(),
                }
            })
            .collect();
        if failing.is_empty() {
            return Ok(None);
        }
        let class = match task_core::classify_review_failure(&failing) {
            task_core::RepairDecision::Repairable(c) => c,
            task_core::RepairDecision::Substantive => return Ok(None),
        };

        let units = self.store.work_units_for(task_id)?;
        let repairs: Vec<&task_core::WorkUnitRow> = units
            .iter()
            .filter(|u| u.kind == task_core::WorkUnitKind::Repair)
            .collect();
        if repairs.len() as u32 >= self.config.execution.max_repairs {
            return Ok(None);
        }
        let same_class = repairs
            .iter()
            .filter(|u| Self::repair_bucket_of_title(&u.spec.title) == Some(class.bucket()))
            .count();
        if same_class as u32 >= self.config.execution.max_repairs_per_class {
            return Ok(None);
        }

        let (max_turns, max_wall_secs) = class.budget();
        let failing_details: Vec<String> = failing.iter().map(|f| f.reason.clone()).collect();
        let workspaces = self.task_workspaces_for(task);
        let cwd = workspaces.as_ref().and_then(|w| w.cwd());
        let branch = workspaces
            .as_ref()
            .and_then(|w| w.repos.first())
            .and_then(|r| r.branch())
            .unwrap_or_default();
        let diff_stat = crate::checkpoint::gather_repo_facts(cwd, branch)
            .0
            .map(|r| r.diff_stat);
        let objective = task_core::build_repair_objective(
            class,
            &failing_details,
            &task.title,
            &task.objective,
            diff_stat.as_deref(),
        );
        let n = repairs.len() + 1;
        let spec = task_core::WorkUnitSpec {
            key: format!("repair-{n}"),
            kind: task_core::WorkUnitKind::Repair,
            title: format!("repair ({}): 修復", class.bucket()),
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
            phase: None,
        };

        let now = rfc3339(OffsetDateTime::now_utc());
        let active_plan = self.store.execution_plan_active(task_id)?;
        let (new_plan, work_units, extra) = match active_plan {
            Some(plan) => {
                // D6: 計画のある Task は最終レビューまでに全 WU が done なので、既存の seq の続き。
                let seq = units.iter().map(|u| u.seq).max().unwrap_or(0) + 1;
                let row = task_core::WorkUnitRow::new(
                    task_core::new_id(),
                    task_id.to_string(),
                    plan.id.clone(),
                    seq,
                    spec,
                    task_core::WorkUnitStatus::Ready,
                    now.clone(),
                );
                let ev = Event::WorkUnitTransitioned {
                    work_unit_id: row.id.clone(),
                    key: row.key.clone(),
                    from: task_core::WorkUnitStatus::Pending,
                    to: task_core::WorkUnitStatus::Ready,
                    reason: "review_repair".to_string(),
                    run_id: None,
                };
                // ADR-0074 D6.2（Phase F1）: この repair WU の class を events に残す
                // （`execution_metrics::summarize` が `repairs_by_class` を組み立てる材料。
                // `unknown` を無くす）。
                let scheduled = Event::RepairScheduled {
                    work_unit_id: row.id.clone(),
                    key: row.key.clone(),
                    class: class.bucket().to_string(),
                    origin: task_core::execution::RepairOrigin::Review,
                };
                (None, vec![row], vec![ev, scheduled])
            }
            None => {
                // D5: atomic な Task は、初めての WorkUnit で暗黙の WU を `main`（done）として実体化する。
                let plan_id = task_core::new_id();
                let main_spec = task_core::WorkUnitSpec {
                    key: "main".to_string(),
                    kind: task_core::WorkUnitKind::Implement,
                    title: task.title.clone(),
                    objective: task.objective.clone(),
                    depends_on: vec![],
                    done_when: vec![],
                    checks: vec![],
                    context: Default::default(),
                    harness: None,
                    features: None,
                    budget: None,
                    outputs: vec![],
                    phase: None,
                };
                let main_row = task_core::WorkUnitRow::new(
                    task_core::new_id(),
                    task_id.to_string(),
                    plan_id.clone(),
                    0,
                    main_spec.clone(),
                    task_core::WorkUnitStatus::Done,
                    now.clone(),
                );
                let repair_row = task_core::WorkUnitRow::new(
                    task_core::new_id(),
                    task_id.to_string(),
                    plan_id.clone(),
                    1,
                    spec.clone(),
                    task_core::WorkUnitStatus::Ready,
                    now.clone(),
                );
                let plan_spec = task_core::ExecutionPlanSpec {
                    stages: Vec::new(),
                    units: Vec::new(),
                    decisions: Vec::new(),
                    schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
                    rationale: "reviewer repair: 暗黙の WorkUnit を実体化".to_string(),
                    work_units: vec![main_spec, spec],
                    phases: Vec::new(),
                    children: Vec::new(),
                };
                let plan_row = task_core::ExecutionPlanRow {
                    id: plan_id.clone(),
                    task_id: task_id.to_string(),
                    version: 1,
                    origin: task_core::PlanOrigin::Repair,
                    planner_run_id: None,
                    status: task_core::PlanStatus::Active,
                    spec: plan_spec.clone(),
                    created_at: now.clone(),
                    superseded_at: None,
                };
                let ev = Event::ExecutionPlanned {
                    plan_id,
                    version: 1,
                    origin: task_core::PlanOrigin::Repair,
                    supersedes: None,
                    reason: Some("review_repair".to_string()),
                    plan: Box::new(plan_spec),
                };
                let scheduled = Event::RepairScheduled {
                    work_unit_id: repair_row.id.clone(),
                    key: repair_row.key.clone(),
                    class: class.bucket().to_string(),
                    origin: task_core::execution::RepairOrigin::Review,
                };
                (
                    Some(plan_row),
                    vec![main_row, repair_row],
                    vec![ev, scheduled],
                )
            }
        };

        let mut all_events = std::mem::take(events);
        all_events.extend(extra);
        match self
            .store
            .review_repair_apply(task_id, all_events, new_plan, work_units)
        {
            Ok(outcome) => Ok(Some(outcome)),
            Err(e) => Err(e.into()),
        }
    }

    /// ADR-0016 M4: `aggregate = true` で、Approval 以外の子が 1 件以上あり、まだ集約遷移をしていないか。
    fn needs_aggregate_run(&self, task: &Task) -> Result<bool, DispatchError> {
        if !task.aggregate {
            return Ok(false);
        }
        let has_children = self
            .store
            .children(task.id)?
            .iter()
            .any(|c| c.kind != TaskKind::Approval);
        if !has_children {
            return Ok(false);
        }
        let events = self.store.events_for(task.id)?;
        Ok(!has_aggregate_transition(&events))
    }

    /// ADR-0021 D1/D3: 委譲した子（`Event::Delegated`）のうち、**前回この親が子の失敗を扱ってから後に** `failed` に
    /// なったもの。一度扱った失敗は数え直さない（親が別の子に割り当て直して成功したのに、古い失敗で止まらないため）。
    /// 返り値は id 順の `(子, 直近のワーカー run の outcome)`。
    ///
    /// Phase 45（実機バグ）: ここは親と子で別々のタスクのイベント id を比較するので、`events_for`（タスクごとの
    /// ローカルな `seq`）ではなく `events_for_with_global_ids`（`events` テーブルの全タスク共通の `id`）を
    /// 使わなければならない。`seq` で比較すると、子の方がイベント数が多い（＝ `seq` が大きい）場合に
    /// 「一度扱った失敗」でも毎回「新規」と判定され、同じ質問が繰り返し出る。
    fn newly_failed_delegated_children(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<(Task, Option<String>)>, DispatchError> {
        let events = self.store.events_for_with_global_ids(task_id)?;
        // 直近に子の失敗を扱った時点（グローバル id。イベント id は単調増加）。
        let handled_at = events
            .iter()
            .rev()
            .find_map(|(id, e)| match e {
                Event::Transitioned { reason, .. } if reason == Trigger::ChildFailed.name() => {
                    Some(*id)
                }
                _ => None,
            })
            .unwrap_or(0);
        let mut delegated: Vec<TaskId> = events
            .iter()
            .filter_map(|(_, e)| match e {
                Event::Delegated { task_ids, .. } => Some(task_ids.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        delegated.sort();
        delegated.dedup();

        let mut out = Vec::new();
        for id in delegated {
            let Some(child) = self.store.get(id)? else {
                continue;
            };
            if child.status != Status::Failed {
                continue;
            }
            let child_events = self.store.events_for_with_global_ids(id)?;
            let failed_at = child_events.iter().rev().find_map(|(eid, e)| match e {
                Event::Transitioned {
                    to: Status::Failed, ..
                } => Some(*eid),
                _ => None,
            });
            // 既に扱った失敗（id が前回の child_failed より前）は数えない。
            if failed_at.is_some_and(|at| at <= handled_at) {
                continue;
            }
            let outcome = child_events.iter().rev().find_map(|(_, e)| match e {
                Event::WorkerFinished {
                    outcome,
                    role: None,
                    ..
                } => Some(outcome.clone()),
                _ => None,
            });
            out.push((child, outcome));
        }
        Ok(out)
    }

    /// ADR-0021 D1/D2: 委譲した子が失敗していたら、親をやり直す（attempts に余裕があるとき）か、
    /// 人間に質問して待つ（`blocked`）。**親を `failed` にはしない。** 扱ったら `true`（`events` も記録済み）。
    /// `events`（判定結果など、まだ記録していないもの）は、**扱ったときだけ**同じトランザクションで一緒に記録する。
    /// 扱わなかったときは触らない（呼び出し側がそのまま使う）。
    fn escalate_failed_children(
        &mut self,
        task: &Task,
        run_id: &str,
        events: &mut Vec<Event>,
    ) -> Result<bool, DispatchError> {
        if self.config.delegation.on_child_failure == OnChildFailure::Ignore {
            return Ok(false);
        }
        // ADR-0033 D4 / Phase 28: 対話タスクは委譲できない（子を持たない）ので、そもそも子の失敗は
        // 起きないはずだが、念のため `retry_then_ask` の対象から外す（対話タスクが `blocked` に落ちる
        // 経路を完全に断つ）。
        if task_core::is_conversation(task) {
            return Ok(false);
        }
        let failed = self.newly_failed_delegated_children(task.id)?;
        if failed.is_empty() {
            return Ok(false);
        }
        let mut events = std::mem::take(events);
        let listed = failed
            .iter()
            .map(|(child, outcome)| {
                format!(
                    "{} ({}): {}",
                    child.title,
                    child.id,
                    outcome.as_deref().unwrap_or("(no outcome recorded)")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        // 状態機械と同じ判定（ADR-0021 D1）。ここで分かるのは「やり直せるか」だけ。
        let will_retry = task.attempts < task.budget.max_retries;
        if will_retry {
            events.push(Event::worker_progress(
                run_id,
                format!(
                    "{} delegated child task(s) failed; retrying this task (attempt {}/{}): {listed}",
                    failed.len(),
                    task.attempts + 1,
                    task.budget.max_retries,
                ),
            ));
        } else {
            let text = format!(
                "委譲した子タスクが失敗し、やり直し（max_retries = {}）でも解決しませんでした。どうしますか。\n\
                 失敗した子: {listed}\n\
                 回答するとこのタスクは指示を持って再開します: celerisctl answer {} \"…\"",
                task.budget.max_retries, task.id,
            );
            // Phase 44（実機 2026-09-18）: この質問はディスパッチャ由来（ワーカーの `Question` ではない）だが、
            // Phase 26 と同じく `approvals` にも残す。そうしないと認可画面に出ず、`approval_pending` の
            // Discord 通知も飛ばない（受信箱にだけ出て気づかれない）。
            if let Err(e) = crate::approvals::record_question_approval(
                self.store.as_ref(),
                task,
                &text,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the approval for the child-failure question");
            }
            events.push(Event::QuestionRaised {
                run_id: run_id.to_string(),
                text,
            });
        }
        match self
            .store
            .apply_transition_with_events(task.id, Trigger::ChildFailed, events)
        {
            Ok(outcome) => {
                tracing::info!(
                    task_id = %task.id, %run_id, next = ?outcome.next, failed = failed.len(),
                    "delegated child task(s) failed; parent retries or asks a human (ADR-0021)"
                );
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(task_id = %task.id, error = %e, "child_failed transition could not be applied");
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        }
        Ok(true)
    }

    /// ADR-0016 M1 / M4: `Aggregate`（reviewing → ready、attempts 据え置き）を適用し、次の dispatch を集約 run にする。
    fn schedule_aggregate_run(
        &mut self,
        task_id: TaskId,
        run_id: &str,
        mut events: Vec<Event>,
    ) -> Result<(), DispatchError> {
        let children = self.store.children(task_id)?.len();
        events.push(Event::worker_progress(
            run_id,
            format!(
                "all {children} delegated child task(s) finished; scheduling the aggregate run"
            ),
        ));
        match self
            .store
            .apply_transition_with_events(task_id, Trigger::Aggregate, events)
        {
            Ok(outcome) => {
                tracing::info!(%task_id, %run_id, next = ?outcome.next, "aggregate run scheduled");
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(%task_id, error = %e, "aggregate transition could not be applied");
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// ADR-0016 M5: 子待ちの親を毎 tick 数え直し、全て終端になったら集約 run（M4）か `ReviewPass`（Plan なら `complete_plan`）。
    fn settle_awaiting_children(&mut self) -> Result<(), DispatchError> {
        let ids: Vec<TaskId> = self.awaiting_children.keys().copied().collect();
        for task_id in ids {
            let Some(task) = self.store.get(task_id)? else {
                self.awaiting_children.remove(&task_id);
                continue;
            };
            if task.status != Status::Reviewing {
                self.awaiting_children.remove(&task_id);
                continue;
            }
            let pending = pending_children(self.store.as_ref(), task_id).map_err(ops_to_store)?;
            if pending > 0 {
                continue;
            }
            let Some(waiting) = self.awaiting_children.remove(&task_id) else {
                continue;
            };
            // ADR-0021 D1: 子が失敗していたら、集約・完了より先に「やり直す or 人に聞く」。
            if self.escalate_failed_children(&task, &waiting.run_id, &mut Vec::new())? {
                continue;
            }
            if self.needs_aggregate_run(&task)? {
                self.schedule_aggregate_run(task_id, &waiting.run_id, Vec::new())?;
                continue;
            }
            let result = match (task.kind, waiting.plan) {
                (TaskKind::Plan, Some(mut plan)) => {
                    let org = self.store.org_list()?;
                    self.fix_plan_for_harness(&task, &mut plan, &org);
                    // ADR-0039 D2: 子の作業場所は 明示 > 案件 > 親。
                    let project_workspace =
                        task_ops::delegate::project_workspace(self.store.as_ref(), &task)
                            .map_err(ops_to_store)?;
                    // ADR-0043 D2: 子のリポジトリは 明示（計画の `repos`）> 親 > 案件の primary。
                    let project_repos =
                        task_ops::delegate::project_repos(self.store.as_ref(), &task)
                            .map_err(ops_to_store)?;
                    let home = task_core::home_dir();
                    let workspace = task_core::WorkspaceContext {
                        project: project_workspace.as_ref(),
                        home: home.as_deref(),
                        repos: &project_repos,
                    };
                    let children = materialize_logging(
                        &task,
                        &plan,
                        &org,
                        &self.config.roles,
                        &self.config.genres,
                        workspace,
                        OffsetDateTime::now_utc(),
                        &mut |child_id, reason| {
                            tracing::info!(task_id = %task_id, child_id = %child_id, %reason, "workspace downgraded to local (ADR-0062 B2)");
                        },
                    );
                    self.store.complete_plan(
                        task_id,
                        Vec::new(),
                        children,
                        self.config.plan_auto_accept,
                    )
                }
                _ => self.store.apply_transition_with_events(
                    task_id,
                    Trigger::ReviewPass,
                    Vec::new(),
                ),
            };
            match result {
                Ok(outcome) => {
                    tracing::info!(%task_id, run_id = %waiting.run_id, next = ?outcome.next, "delegated children finished; parent completed");
                }
                Err(StoreError::InvalidTransition(e)) => {
                    tracing::warn!(%task_id, error = %e, "parent completion could not be applied");
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// ADR-0070 D5（Phase 116）: lease が期限切れでも、**このインスタンスが持っている run**
    /// （`self.running` に entry がある）のプロセスがまだ生きていれば（`process_group::group_alive`）、
    /// reclaim せず lease を延長して続行する（DB busy で数 tick 更新できなかっただけ、という実際に
    /// 起きた事故〈PROGRESS Phase 116〉をここで救う）。延長にも失敗したら次の tick に持ち越す。
    /// 死んでいる（またはこのインスタンスの管理外）ときだけ、ADR-0070 D3 の分岐
    /// （`InfraRequeue` でバックオフ再試行、`max_infra_retries` 到達で打ち切り）に乗せる。
    /// `Trigger::LeaseExpired`（無条件に attempts を消費する）はもう使わない。
    ///
    /// Phase F5-fix6: lease がまだ切れていなくても、持ち主のデーモンが居ない run（孤児。定義は
    /// `crate::orphan`）は同じ経路で**すぐに**回収する（result.json があればその内容で確定、無ければ
    /// `interrupted: orphan_takeover` で requeue）。v2 の工程の lease は `reconcile_parallel_tasks` が
    /// WU ごとに扱う。
    fn reclaim_expired_leases(&mut self) -> Result<usize, DispatchError> {
        let now = OffsetDateTime::now_utc();
        let mut count = 0;
        let mut holders_gone: Option<bool> = None;
        for task in self.store.list(Some(Status::Running))? {
            // ADR-0041 D5: 面倒を見ないタスクのリースは奪わない（verify は本番のコピーの行を書き換えない）。
            if !self.is_eligible(&task) {
                continue;
            }
            let Some(lease) = &task.lease else { continue };
            let orphaned = if lease.expires_at > now {
                if is_phase_lease_holder(&lease.worker_run_id)
                    || self.holds_task_in_hand(task.id)
                    || !self.lease_holders_gone(&mut holders_gone, now)
                {
                    continue;
                }
                true
            } else {
                false
            };
            if orphaned {
                self.note_orphan_takeover(&task, &lease.worker_run_id, lease.expires_at);
            }
            // ADR-0074 D1.5（Phase F2）: v2 の並列 WU では同じ Task の run が複数ありうる。
            // どれか 1 本でも生きていれば、その run の lease（WU の lease）を延ばす
            // （`renew_lease` が Task の lease も延ばす）。
            let alive_entry = self
                .running
                .iter()
                .filter(|(k, _)| k.task == task.id)
                .map(|(_, e)| e)
                .find(|e| task_worker::process_group::group_alive(&e.run_id));
            if let Some(entry) = alive_entry {
                let ttl = Duration::from_secs(task.budget.max_wall_secs) + self.config.lease_grace;
                match self.store.renew_lease(task.id, &entry.run_id, ttl) {
                    Ok(true) => {
                        tracing::warn!(task_id = %task.id, run_id = %entry.run_id, "lease expired but the run's process is still alive; extended instead of reclaiming (ADR-0070 D5)");
                        continue;
                    }
                    Ok(false) => {
                        // 一致しない（レース。他の何かがリースを動かした）。下の通常の reclaim へ。
                    }
                    Err(e) => {
                        tracing::warn!(task_id = %task.id, run_id = %entry.run_id, error = %e, "failed to extend the lease for a still-alive run; will retry reclaiming next tick");
                        continue;
                    }
                }
            }
            // Phase F5-fix2: run は終わったが WU の `checks` がまだ走っている（`checking`）。検査が
            // 延ばした lease（`review_timeout × (2n+1)`）より長引いても、検査の完了を受け取るまで回収しない。
            let checking_run = self
                .checking
                .iter()
                .find(|(_, e)| e.task_id == task.id)
                .map(|(run_id, _)| run_id.clone());
            if let Some(run_id) = checking_run {
                let ttl = self.config.review_timeout + self.config.lease_grace;
                match self.store.renew_lease(task.id, &run_id, ttl) {
                    Ok(true) => {
                        tracing::warn!(task_id = %task.id, %run_id, "lease expired while the work unit checks are still running; extended instead of reclaiming (Phase F5-fix2)");
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to extend the lease for running work unit checks; will retry reclaiming next tick");
                        continue;
                    }
                }
            }
            // ADR-0061（Phase 104）: `entry` を消費する前に `since`（wall time 計算用）を取っておく。
            let mut run_since: Option<OffsetDateTime> = None;
            let keys: Vec<RunKey> = self
                .running
                .keys()
                .filter(|k| k.task == task.id)
                .cloned()
                .collect();
            for key in keys {
                if let Some(entry) = self.running.remove(&key) {
                    run_since = run_since.or(Some(entry.since));
                    // ADR-0044 Phase 53 追記: リース喪失も同じ止め方（プロセスグループごと。
                    // コンテナで走っていればラベル越しにも同じ 2 段を送る）。
                    self.stop_run(&entry.run_id, entry.handle, entry.container);
                }
            }
            // ADR-0074 D1.5/D1.7（Phase F2）: 工程の lease（v2）なら、lease を持っていた run は
            // `running` の WU の run（`lease_run_id`）。それぞれに `WorkerFinished` を残し、WU を
            // 照合で戻す（統合の途中なら `integrate-<phase>` も pending に戻す）。
            let phase_lease = is_phase_lease_holder(&lease.worker_run_id);
            let wu_runs: Vec<String> = if phase_lease {
                self.store
                    .work_units_for(task.id)?
                    .into_iter()
                    .filter(|u| {
                        u.status == task_core::WorkUnitStatus::Running
                            && u.kind != task_core::WorkUnitKind::Integrate
                    })
                    .filter_map(|u| u.lease_run_id.or(u.last_run_id))
                    .collect()
            } else {
                vec![lease.worker_run_id.clone()]
            };
            // Phase F5-fix2（P-F5-3）: 終端の result.json を残して消えた run は、requeue せずに
            // その内容で確定させる。1 本でも確定させたらこの tick の回収はやめる（Task の遷移・lease は
            // 確定の経路が決めた。残りの死んだ WU の run は次の tick の照合が拾う）。
            let mut finalised_any = false;
            for run_id in &wu_runs {
                if self.finalise_from_result_json(&task, run_id) {
                    finalised_any = true;
                }
            }
            if finalised_any {
                count += 1;
                continue;
            }
            let metrics = run_since.map(|since| task_core::RunMetrics {
                wall_ms: wall_ms_since(since),
                retries: task.attempts,
                peak_context_tokens: None,
                turns: None,
            });
            let infra_n = consecutive_infra_requeues(&self.store.events_for(task.id)?) + 1;
            // Phase F5-fix6: 孤児は lease 切れではなく「持ち主のデーモンが居なくなって中断された run」。
            let why = if orphaned {
                ORPHAN_WHY
            } else {
                "lease expired"
            };
            let (trigger, outcome_text) = if infra_n <= self.config.max_infra_retries {
                (
                    Trigger::InfraRequeue,
                    if orphaned {
                        format!("interrupted: {why} (run_id={})", lease.worker_run_id)
                    } else {
                        format!("infra_requeue: {why} (run_id={})", lease.worker_run_id)
                    },
                )
            } else {
                (
                    Trigger::WorkerError { retryable: false },
                    format!(
                        "{INFRA_FAILURE_MARKER}{infra_n}: {why} (run_id={})",
                        lease.worker_run_id
                    ),
                )
            };
            let class = if orphaned {
                task_core::HarnessErrorClass::Infra
            } else {
                task_core::HarnessErrorClass::LeaseExpired
            };
            let wu_reason = if orphaned {
                crate::orphan::ORPHAN_TAKEOVER_REASON
            } else {
                "restart_reconcile"
            };
            let finished: Vec<Event> = wu_runs
                .iter()
                .map(|run_id| Event::WorkerFinished {
                    run_id: run_id.clone(),
                    outcome: outcome_text.clone(),
                    usage: None,
                    role: None,
                    metrics,
                    end: Some(task_core::RunEnd::HarnessError { class }),
                })
                .collect();
            match self
                .store
                .apply_transition_with_events(task.id, trigger.clone(), finished)
            {
                Ok(outcome) => {
                    tracing::warn!(task_id = %task.id, run_id = %lease.worker_run_id, next = ?outcome.next, attempts = outcome.attempts, orphaned, "{why}; reclaimed");
                    // 孤児（再起動の中断）はバックオフしない（インフラの不調ではなく人の再起動）。
                    if matches!(trigger, Trigger::InfraRequeue) && !orphaned {
                        let until = now + infra_backoff_delay(infra_n);
                        self.infra_backoff.insert(task.id, until);
                    }
                    // ADR-0072 D15（Phase E2）: この run が計画のある Task の WU のものだったなら、
                    // その WU の行も `running` のまま残さず、checkpoint があれば `needs_continuation`、
                    // 無ければ `ready` に戻す（`WorkUnitTransitioned{reason: "restart_reconcile"}`）。
                    for run_id in &wu_runs {
                        if let Err(e) = self.reconcile_work_unit_run(task.id, run_id, wu_reason) {
                            tracing::warn!(task_id = %task.id, run_id = %run_id, error = %e, "failed to reconcile the work unit for a reclaimed lease");
                        }
                    }
                    if phase_lease
                        && let Err(e) = self.reconcile_integration(task.id, "restart_reconcile")
                    {
                        tracing::warn!(task_id = %task.id, error = %e, "failed to reconcile the phase integration for a reclaimed lease");
                    }
                    count += 1;
                }
                Err(StoreError::InvalidTransition(e)) => {
                    tracing::warn!(task_id = %task.id, error = %e, "lease reclaim skipped");
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(count)
    }

    /// ADR-0044 §5 Phase 53 追記（Phase 55）: **run の止め方はこれ 1 つ**。
    ///
    /// `cancel` / 人のコメントによる割り込み（`Interrupt`）/ 実時間・無入力のタイムアウト /
    /// リース喪失 / drain タイムアウトのどれも、ここを通って
    /// **ワーカーのプロセスグループに SIGTERM → `kill_grace_secs` → SIGKILL** を送る
    /// （`task_worker::kill_tree`）。ハーネスが起こした孫（`cargo test`、`node`、シェル）まで届く。
    /// タイムアウトだけは `task_worker::subprocess` の中でも同じ手順を踏むが、そちらが先に終わって
    /// いれば登録が無いので、ここは何もしない（二重には送らない）。
    ///
    /// tokio の `JoinHandle::abort()` は**従来どおり即座に**行う（run の記録を止めるための帳簿）。
    ///
    /// Phase 55/56 の合流（ADR-0044 P55-4 / ADR-0043 P56-7）: `container` が `Some`（= ADR-0043 D3 で
    /// コンテナ実行に倒した run）なら、`killpg` と**同じ 2 段**を
    /// `--label celeris.task=<task_id>` 越しにも送る（`<runtime> kill --signal TERM` → `grace` →
    /// `<runtime> rm -f`）。`killpg` は `<runtime> run` のクライアントにしか届かず、
    /// コンテナの中は別の PID 名前空間なので、これが無いと中のハーネスが生き残る。
    fn stop_run(
        &self,
        run_id: &str,
        handle: JoinHandle<()>,
        container: Option<Arc<dyn task_worker::ContainerStopper>>,
    ) {
        task_worker::kill_tree_with(run_id, self.config.kill_grace, container);
        handle.abort();
    }

    /// レビュー側（判定コマンドの run と Reviewer run）の停止。run は 2 本ありうるので両方に送る。
    fn stop_review(&self, entry: ReviewEntry) {
        task_worker::kill_tree(&entry.run_id, self.config.kill_grace);
        if let Some(review_run_id) = &entry.review_run_id {
            task_worker::kill_tree(review_run_id, self.config.kill_grace);
        }
        entry.handle.abort();
    }

    /// Phase F5-fix3: 止めた run に `WorkerFinished` がまだ無ければ、`interrupted: <why>`（`end =
    /// cancelled`。GUI では割り込みと同じ「失敗ではない」扱い、コメントの割り込みも消費しない）を追記する。
    /// ストアが同じトランザクションで `runs` 行を `cancelled` にする。失敗しても警告だけ。
    fn close_aborted_run(&self, task_id: TaskId, run_id: &str, role: Option<RunRole>, why: &str) {
        let already = match self.store.events_for(task_id) {
            Ok(events) => events
                .iter()
                .any(|(_, e)| matches!(e, Event::WorkerFinished { run_id: r, .. } if r == run_id)),
            Err(e) => {
                tracing::warn!(%task_id, %run_id, error = %e, "failed to read events before closing an aborted run");
                return;
            }
        };
        if already {
            return;
        }
        let finished = Event::WorkerFinished {
            run_id: run_id.to_string(),
            outcome: format!("interrupted: {why}"),
            usage: None,
            role,
            metrics: None,
            end: Some(task_core::RunEnd::Cancelled),
        };
        if let Err(e) = self.store.append_event(task_id, &finished) {
            tracing::warn!(%task_id, %run_id, error = %e, "failed to close an aborted run");
        }
    }

    /// ADR-0002 D9: ストア上で `running` でなくなった（cancel / ADR-0044 D2 の割り込み等）run を
    /// 強制終了する。打ち切ったタスクは `just_aborted` に入れ、**この tick では dispatch し直さない**。
    fn abort_stale_runs(&mut self) -> Result<(), DispatchError> {
        self.just_aborted.clear();
        let keys: Vec<RunKey> = self.running.keys().cloned().collect();
        for key in keys {
            let id = key.task;
            let current = self.store.get(id)?;
            let still_ours = match (&current, self.running.get(&key)) {
                (Some(t), Some(entry)) => self.run_holds_lease(t, &entry.run_id)?,
                _ => false,
            };
            if !still_ours && let Some(entry) = self.running.remove(&key) {
                tracing::warn!(task_id = %id, run_id = %entry.run_id, "aborting run (task no longer running under this lease)");
                // ADR-0044 Phase 53 追記: プロセスグループごと止める（孫まで。コンテナならその中も）。
                let run_id = entry.run_id.clone();
                self.stop_run(&entry.run_id, entry.handle, entry.container);
                // Phase F5-fix3: cancel 等で止めた run は誰も `WorkerFinished` を書かない（割り込み・lease の
                // 回収なら書いてある）。書かれていなければ閉じる（`runs` 行も終端になる）。
                self.close_aborted_run(
                    id,
                    &run_id,
                    None,
                    "aborted (task no longer running under this lease)",
                );
                self.just_aborted.insert(id);
                // ADR-0074 D1.6/D1.7（Phase F2）: v2 の WU の run を止めたなら、WU を `running` の
                // まま残さない（割り込み・lease 喪失なら checkpoint の有無で needs_continuation /
                // ready に戻す。Cancel は `cancel_open_work_units` が cancelled にする）。
                if key.work_unit.is_some()
                    && let Some(t) = &current
                    && !t.status.is_terminal()
                    && let Err(e) = self.reconcile_work_unit_run(id, &entry.run_id, "aborted")
                {
                    tracing::warn!(task_id = %id, run_id = %entry.run_id, error = %e, "failed to reconcile the work unit of an aborted run");
                }
            }
        }
        // ADR-0074 D1.4/D1.6（Phase F2b）: Running でなくなった Task の統合も止める。Cancel なら
        // 未完了の WU を cancelled にし、WU の worktree とブランチを消す。
        let integrating: Vec<TaskId> = self.integrating.keys().copied().collect();
        for id in integrating {
            let still_running =
                matches!(self.store.get(id)?, Some(t) if t.status == Status::Running);
            if !still_running && let Some(entry) = self.integrating.remove(&id) {
                tracing::warn!(task_id = %id, "aborting the phase integration (task no longer running)");
                entry.handle.abort();
                self.just_aborted.insert(id);
            }
        }
        // Phase F5-fix2: Running でなくなった Task の WU の検査も止める（結果は捨てられるだけなので、
        // draining のインスタンスを待たせない）。
        let checking: Vec<(String, TaskId)> = self
            .checking
            .iter()
            .map(|(run_id, e)| (run_id.clone(), e.task_id))
            .collect();
        for (run_id, id) in checking {
            let still_running =
                matches!(self.store.get(id)?, Some(t) if t.status == Status::Running);
            if !still_running && let Some(entry) = self.checking.remove(&run_id) {
                tracing::warn!(task_id = %id, %run_id, "aborting the work unit checks (task no longer running)");
                entry.handle.abort();
            }
        }
        let aborted: Vec<TaskId> = self.just_aborted.iter().copied().collect();
        for id in aborted {
            if let Some(t) = self.store.get(id)?
                && t.status == Status::Cancelled
                && self.store.execution_plan_active(id)?.is_some()
            {
                self.cancel_open_work_units(&t)?;
            }
        }
        // レビュー中に cancel されたタスクの判定（Reviewer run を含む）も中断する。
        let ids: Vec<TaskId> = self.reviewing.keys().copied().collect();
        for id in ids {
            let still_reviewing =
                matches!(self.store.get(id)?, Some(t) if t.status == Status::Reviewing);
            if !still_reviewing && let Some(entry) = self.reviewing.remove(&id) {
                tracing::warn!(task_id = %id, "aborting review (task no longer reviewing)");
                // ADR-0076: 止めた Reviewer run の `QuotaActivity` も閉じる（Event は残さない）。
                if let (Some(review_run_id), Some(provider)) =
                    (entry.review_run_id.clone(), entry.provider.clone())
                {
                    self.release_quota_if_tracked(
                        &review_run_id,
                        entry.account.as_deref(),
                        entry.account_adapter,
                        &provider,
                        id,
                    );
                }
                if let Some(review_run_id) = entry.review_run_id.clone() {
                    self.close_aborted_run(
                        id,
                        &review_run_id,
                        Some(RunRole::Reviewer),
                        "review aborted (task no longer reviewing)",
                    );
                }
                self.stop_review(entry);
                self.pending_subjects.remove(&id);
            }
        }
        Ok(())
    }

    fn recover_reviews(&mut self) -> Result<(), DispatchError> {
        let reviewing_tasks = self.store.list(Some(Status::Reviewing))?;
        // 承認待ちの記録は、まだ reviewing のタスクだけに保つ（cancel 等で抜けたものをスナップショットに残さない。ADR-0013 D4）。
        self.awaiting_human
            .retain(|id| reviewing_tasks.iter().any(|t| t.id == *id));
        for task in reviewing_tasks {
            if self.reviewing.contains_key(&task.id)
                || self.awaiting_children.contains_key(&task.id)
            {
                continue;
            }
            // ADR-0041 D5: 面倒を見ないタスクのレビューは拾わない（verify は他人のタスクを判定しない）。
            if !self.is_eligible(&task) {
                continue;
            }
            let events = self.store.events_for(task.id)?;
            let run_id = last_run_id(&events).unwrap_or_default();
            // 前 tick で見送った場合はメモリ上の done 内容、再起動後は runs/<run_id>/result.json から復元。
            let subject = match self.pending_subjects.remove(&task.id) {
                Some(s) => s,
                None => self
                    .task_dir(&task)
                    .map(|dir| subject_from_run_dir(&dir, &run_id))
                    .unwrap_or_default(),
            };
            if !self.spawn_review(task.id, run_id, &subject)? {
                self.pending_subjects.insert(task.id, subject);
            }
        }
        Ok(())
    }

    /// ADR-0074 D1.5（Phase F2）: その Task の鍵を持つ run の数（v1・atomic なら 0 か 1）。
    fn running_for_task(&self, task_id: TaskId) -> usize {
        self.running.keys().filter(|k| k.task == task_id).count()
    }

    /// ADR-0074 D1.5: `run_id` の run を `running` から取り除いて返す（`Completion` は `run_id` を
    /// 運ぶので、終わった run の鍵はここで引く。並列度の上限は小さいので線形探索でよい）。
    fn take_running_by_run_id(&mut self, run_id: &str) -> Option<RunEntry> {
        let key = self
            .running
            .iter()
            .find(|(_, e)| e.run_id == run_id)
            .map(|(k, _)| k.clone())?;
        self.running.remove(&key)
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

    /// ADR-0007 D2: その Plan 自身を含む祖先 Plan の数。
    fn plan_depth(&self, task: &Task) -> Result<u32, DispatchError> {
        let mut depth = 0;
        let mut current = Some(task.clone());
        let mut hops = 0;
        while let Some(t) = current {
            if t.kind == TaskKind::Plan {
                depth += 1;
            }
            hops += 1;
            if hops > 64 {
                break;
            }
            current = match t.parent_id {
                Some(p) => self.store.get(p)?,
                None => None,
            };
        }
        Ok(depth)
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

    /// ADR-0072 D15（Phase E2）: `run_id` の run が「まだ `running` の WU」に属していたら、
    /// checkpoint があれば `needs_continuation`、無ければ `ready` に戻す
    /// （`WorkUnitTransitioned{reason}`）。属していなければ何もしない（`Ok(())`）。
    /// ADR-0074 D1.5（Phase F2）: `run_id` の run がまだこの Task の lease を持っているか。
    /// v1・atomic は従来どおり Task の lease の保持者と比べる。v2（工程の lease）では WU の
    /// `lease_run_id` と比べる（lease を失った WU の run の結果を捨てる判定）。
    /// Phase F5-fix2: run の完了（`on_worker_finished` / `on_work_unit_checks_finished`）の確定が
    /// エラーで終わった。`drain_completions` が `?` で tick ごと抜けると受信済みの完了が消え、run は
    /// `running` のまま lease 切れまで残る。ここでエラーを event に残し、run を「インフラ都合の失敗」
    /// （ADR-0070 D3 の `InfraRequeue`、上限を超えたら `infra failure ×N`）として閉じる。記録にも
    /// 失敗したら ERROR だけ残す（lease 切れの経路が拾い、`runs/<run_id>/result.json` があれば
    /// そこから確定させる）。
    fn record_finalisation_failure(
        &mut self,
        task_id: TaskId,
        run_id: &str,
        error: &DispatchError,
    ) {
        tracing::error!(%task_id, %run_id, %error, "failed to finalise a finished run; recording it as an infra failure (Phase F5-fix2)");
        if let Err(e) = self.close_run_after_finalisation_failure(task_id, run_id, error) {
            tracing::error!(%task_id, %run_id, error = %e, "could not record the finalisation failure either; the lease expiry will reclaim the run");
        }
    }

    fn close_run_after_finalisation_failure(
        &mut self,
        task_id: TaskId,
        run_id: &str,
        error: &DispatchError,
    ) -> Result<(), DispatchError> {
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        // 途中まで書けていて、既に run が lease を手放している（Task の遷移まで済んだ）なら何もしない。
        if !self.run_holds_lease(&task, run_id)? {
            return Ok(());
        }
        let Some(lease) = task.lease.clone() else {
            return Ok(());
        };
        let now = OffsetDateTime::now_utc();
        let events = self.store.events_for(task_id)?;
        let finished = |outcome: String| Event::WorkerFinished {
            run_id: run_id.to_string(),
            outcome,
            usage: None,
            role: None,
            metrics: None,
            end: Some(task_core::RunEnd::HarnessError {
                class: task_core::HarnessErrorClass::Infra,
            }),
        };
        if let Err(e) = self.store.run_index_finish(
            run_id,
            task_core::RunIndexStatus::HarnessError,
            None,
            None,
            None,
            now,
        ) {
            tracing::warn!(%task_id, %run_id, error = %e, "failed to finish the runs index row");
        }
        if is_phase_lease_holder(&lease.worker_run_id) {
            // ADR-0074 D1.5/D1.7: 工程の lease（v2）。兄弟の WU の run を巻き込まないよう Task は
            // 遷移させず、この WU だけを戻す（何も走っていなければ `reconcile_parallel_tasks` が
            // Task を ready に戻す）。同じ WU で `max_infra_retries` を超えたら WU を failed にする
            // （ADR-0072 D12/D17: replan の余地があれば replan、無ければ Task の失敗）。
            let wu = self.store.work_units_for(task_id)?.into_iter().find(|u| {
                u.status == task_core::WorkUnitStatus::Running
                    && u.lease_run_id.as_deref() == Some(run_id)
            });
            let Some(wu) = wu else {
                return Ok(());
            };
            let failures_so_far = events
                .iter()
                .filter(|(_, e)| {
                    matches!(
                        e,
                        Event::WorkUnitTransitioned { work_unit_id, reason, .. }
                            if work_unit_id == &wu.id && reason == FINALISE_FAILED_REASON
                    )
                })
                .count() as u32;
            let infra_n = failures_so_far + 1;
            let exhausted = infra_n > self.config.max_infra_retries;
            let outcome = if exhausted {
                format!("{INFRA_FAILURE_MARKER}{infra_n}: finalisation failed: {error}")
            } else {
                format!("infra_requeue: finalisation failed: {error}")
            };
            self.store.append_event(task_id, &finished(outcome))?;
            if exhausted {
                let mut updated = wu.clone();
                updated.status = task_core::WorkUnitStatus::Failed;
                updated.clear_lease();
                updated.updated_at = rfc3339(now);
                self.store.work_unit_transition(
                    task_id,
                    updated,
                    Event::WorkUnitTransitioned {
                        work_unit_id: wu.id.clone(),
                        key: wu.key.clone(),
                        from: task_core::WorkUnitStatus::Running,
                        to: task_core::WorkUnitStatus::Failed,
                        reason: FINALISE_FAILED_REASON.to_string(),
                        run_id: Some(run_id.to_string()),
                    },
                )?;
            } else {
                self.reconcile_work_unit_run(task_id, run_id, FINALISE_FAILED_REASON)?;
            }
            return Ok(());
        }
        // Task の lease をこの run が持つ（atomic・v1 の WU の run）: lease 切れの回収と同じ遷移。
        let infra_n = consecutive_infra_requeues(&events) + 1;
        let (trigger, outcome) = if infra_n <= self.config.max_infra_retries {
            (
                Trigger::InfraRequeue,
                format!("infra_requeue: finalisation failed: {error}"),
            )
        } else {
            (
                Trigger::WorkerError { retryable: false },
                format!("{INFRA_FAILURE_MARKER}{infra_n}: finalisation failed: {error}"),
            )
        };
        self.store.apply_transition_with_events(
            task_id,
            trigger.clone(),
            vec![finished(outcome)],
        )?;
        if matches!(trigger, Trigger::InfraRequeue) {
            self.infra_backoff
                .insert(task_id, now + infra_backoff_delay(infra_n));
        }
        self.reconcile_work_unit_run(task_id, run_id, FINALISE_FAILED_REASON)?;
        Ok(())
    }

    /// Phase F5-fix2（P-F5-3 の result.json の部分）: lease（または WU の lease）が切れた run で、
    /// このインスタンスが抱えていない（`running`/`checking` に無い）もののうち、
    /// `runs/<run_id>/result.json` に終端が残っているものは、requeue せずにその内容で確定させる
    /// （`on_worker_finished` と同じ経路。WU の `checks` があればここから走り直す）。
    /// 本番では draining の旧デーモンが WU の検査の途中で exit し、完了した run が `lease expired`
    /// で捨てられてやり直しになった。確定させた（または確定の失敗を記録した）ら `true`。
    fn finalise_from_result_json(&mut self, task: &Task, run_id: &str) -> bool {
        if self.running.values().any(|e| e.run_id == run_id) || self.checking.contains_key(run_id) {
            return false;
        }
        let Some(dir) = self.task_dir(task) else {
            return false;
        };
        let Some(terminal) = terminal_from_run_dir(&dir, run_id) else {
            return false;
        };
        let provider = self
            .store
            .events_for(task.id)
            .ok()
            .and_then(|events| {
                events.iter().rev().find_map(|(_, e)| match e {
                    Event::WorkerStarted {
                        run_id: r,
                        provider,
                        ..
                    } if r == run_id => provider.clone(),
                    _ => None,
                })
            })
            .unwrap_or_default();
        tracing::warn!(task_id = %task.id, %run_id, "the run's lease expired (or its daemon is gone) but it left a terminal result.json; finalising from it instead of requeueing (Phase F5-fix2 / F5-fix6)");
        if let Err(e) = self.on_worker_finished(
            task.id,
            run_id.to_string(),
            provider,
            Ok(RunOutcome {
                terminal,
                exit_code: None,
            }),
        ) {
            self.record_finalisation_failure(task.id, run_id, &e);
        }
        true
    }

    /// Phase F5-fix6: このインスタンスがその Task の run・検査・統合・レビューを手元に持っているか
    /// （`crate::orphan` の定義の 1.）。
    fn holds_task_in_hand(&self, task_id: TaskId) -> bool {
        self.running.keys().any(|k| k.task == task_id)
            || self.checking.values().any(|e| e.task_id == task_id)
            || self.integrating.contains_key(&task_id)
            || self.reviewing.contains_key(&task_id)
            || self.awaiting_children.contains_key(&task_id)
    }

    /// Phase F5-fix6: run を抱えうる他のデーモンが 1 つも生きていないか（`crate::orphan::holder_gone`）。
    /// 孤児の回収が無効（設定なし）・このインスタンスが新しい仕事を受けていない（draining / standby）・
    /// `daemon_instances` が読めないときは `false`（従来どおり lease の失効を待つ）。1 回の照合の中では
    /// `cache` に覚えて DB を 1 回だけ読む。
    fn lease_holders_gone(&self, cache: &mut Option<bool>, now: OffsetDateTime) -> bool {
        if let Some(v) = *cache {
            return v;
        }
        let gone = match &self.orphan_takeover {
            Some(t) if self.accepting_new_work => match self.store.instance_list() {
                Ok(rows) => crate::orphan::holder_gone(
                    &rows,
                    &t.instance_id,
                    now,
                    t.freshness,
                    t.pid_alive.as_ref(),
                ),
                Err(e) => {
                    tracing::warn!(error = %e, "could not read daemon_instances; not taking over orphaned runs this tick");
                    false
                }
            },
            _ => false,
        };
        *cache = Some(gone);
        gone
    }

    /// Phase F5-fix6: 孤児を回収することを log と event（`worker_progress`、`orphan_takeover: …`）に残す。
    fn note_orphan_takeover(&self, task: &Task, run_id: &str, lease_until: OffsetDateTime) {
        tracing::warn!(
            task_id = %task.id, %run_id, lease_expires_at = %rfc3339(lease_until),
            reason = crate::orphan::ORPHAN_TAKEOVER_REASON,
            "the daemon that held this run is gone (no live active/draining instance besides this one, and the run is not in hand); taking it over without waiting for the lease (Phase F5-fix6)"
        );
        let ev = Event::worker_progress(
            run_id.to_string(),
            format!(
                "{}: このランを持っていたデーモンが居ないため、lease の期限（{}）を待たずに回収します。",
                crate::orphan::ORPHAN_TAKEOVER_REASON,
                rfc3339(lease_until)
            ),
        );
        if let Err(e) = self.store.append_event(task.id, &ev) {
            tracing::warn!(task_id = %task.id, %run_id, error = %e, "failed to record the orphan takeover");
        }
    }

    /// Phase F5-fix6: 工程の lease（v2）の WU の孤児 run で result.json が無いもの。`WorkerFinished`
    /// （`interrupted: …`、`harness_error(infra)`）で `runs` 行を閉じ、WU を ready / needs_continuation
    /// に戻す（reason `orphan_takeover`。Task は遷移させない）。
    fn requeue_orphaned_work_unit_run(
        &mut self,
        task: &Task,
        run_id: &str,
    ) -> Result<(), DispatchError> {
        let already = self
            .store
            .events_for(task.id)?
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerFinished { run_id: r, .. } if r == run_id));
        if !already {
            self.store.append_event(
                task.id,
                &Event::WorkerFinished {
                    run_id: run_id.to_string(),
                    outcome: format!("interrupted: {ORPHAN_WHY} (run_id={run_id})"),
                    usage: None,
                    role: None,
                    metrics: None,
                    end: Some(task_core::RunEnd::HarnessError {
                        class: task_core::HarnessErrorClass::Infra,
                    }),
                },
            )?;
        }
        self.reconcile_work_unit_run(task.id, run_id, crate::orphan::ORPHAN_TAKEOVER_REASON)
    }

    fn run_holds_lease(&self, task: &Task, run_id: &str) -> Result<bool, DispatchError> {
        if task.status != Status::Running {
            return Ok(false);
        }
        let Some(lease) = task.lease.as_ref() else {
            return Ok(false);
        };
        if lease.worker_run_id == run_id {
            return Ok(true);
        }
        if !is_phase_lease_holder(&lease.worker_run_id) {
            return Ok(false);
        }
        Ok(self.store.work_units_for(task.id)?.iter().any(|u| {
            u.status == task_core::WorkUnitStatus::Running
                && u.lease_run_id.as_deref() == Some(run_id)
        }))
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

    /// ADR-0072 D15（Phase E2）: `dispatch_ready` がこの Task について何をすべきか。
    fn wu_dispatch_gate(&self, task_id: TaskId) -> Result<WuDispatchGate, DispatchError> {
        let Some(active_plan) = self.store.execution_plan_active(task_id)? else {
            return Ok(WuDispatchGate::Atomic);
        };
        // ADR-0072「Phase F6 実装時の決定」: 計画を持つ Task に人が後から compound を依頼した
        // （`POST /tasks/{id}/execution/decompose`、`ExecutionHintSet{replan: true}`）なら、次の run は
        // replan の planner run（D17 5.。`max_replans` に数える）。依頼はこの dispatch の
        // `Transitioned{to: running}` で消費される（`pending_replan_request`）。
        let events = self.store.events_for(task_id)?;
        // ADR-0079 D10 / R3a 付記 15.（Phase R3b）: planner の試行が拒否されて「もう一度だけ試します」が約束された
        // （人の replan の依頼・途中確認 / 承認の replan は 1 回目の `Transitioned{to: running}` で消費済み）。起点を
        // 問わず 2 回目の試行を起こす（この replan は 1 回目で `replan_gate` を通っている）。
        if task_ops::tree::planner_retry_pending(&events) {
            return Ok(WuDispatchGate::RunPlanner { replan: true });
        }
        if task_ops::regate::pending_replan_request(&events).is_some() {
            let gate = self.replan_gate(task_id)?;
            if matches!(gate, WuDispatchGate::RunPlanner { .. }) {
                return Ok(gate);
            }
            tracing::warn!(%task_id, "a human replan was requested but max_replans is exhausted; continuing with the current plan");
        }
        // ADR-0074 D2.4（Phase F3 途中確認）: 人が途中確認で「replan」を選んだ（直前の遷移が
        // `phase_replan`）なら、次の run は replan の planner run（`max_replans` に数える）。
        // 上限を使い切っていれば人の指示は `answers` に残したまま次の工程へ進める（警告を残す）。
        if task_ops::phase_gate::last_transition_reason(&events)
            == Some(task_core::PhaseResumeMode::Replan.name())
        {
            let gate = self.replan_gate(task_id)?;
            if matches!(gate, WuDispatchGate::RunPlanner { .. }) {
                return Ok(gate);
            }
            tracing::warn!(%task_id, "phase replan requested but max_replans is exhausted; continuing with the next phase");
        }
        // ADR-0074 D1.3（Phase F2b）: v2 の計画は工程ごとの scheduler（`settle_phase` /
        // `runnable_work_units`）で決める。v1 は従来どおり（`next_work_unit`）。
        // ADR-0079（Phase R1b）: /3 も段階ごとの scheduler（`internal_view` で /2 の工程と同じ行）。
        let v2 = task_core::is_phased_schema(&active_plan.spec.schema);
        let mut units = self.store.work_units_for(task_id)?;
        // ADR-0074 D3.7（Phase F4b (f)）: `child:<key>` の依存を子 Task の状態で決定的に解く。
        let (changed, waiting_on_children) = self.resolve_child_dependencies(task_id, &units)?;
        if changed {
            units = self.store.work_units_for(task_id)?;
        }
        if waiting_on_children
            && !units.iter().any(|u| {
                matches!(
                    u.status,
                    task_core::WorkUnitStatus::Ready
                        | task_core::WorkUnitStatus::NeedsContinuation
                        | task_core::WorkUnitStatus::Running
                )
            })
        {
            // 進められる WU は子の完了待ちのものだけ（Task は ready のまま待つ）。
            return Ok(WuDispatchGate::Skip);
        }
        let stuck = if v2 {
            matches!(
                crate::execution_scheduler::settle_phase(&units),
                crate::execution_scheduler::PhaseSettle::Question(_)
                    | crate::execution_scheduler::PhaseSettle::Failure(_)
            )
        } else {
            matches!(
                task_core::next_work_unit(&units),
                task_core::NextStep::Stuck(_)
            )
        };
        // ADR-0072 D18（Phase E2）/ D17（Phase E4）: 人の回答（`Trigger::Answer` で Task が
        // `Blocked` から `Ready` に戻った）で、`blocked(question|limit)` の WU を再開する（窓は 0 に
        // 戻る。`Ready`/`NeedsContinuation` が無い = `next_work_unit` が `Stuck` を返すときだけ試す）。
        // E4: `Continue{why: Replan}`（WU の failed/limit から replan する。下）でも Task は
        // `running → ready` に戻るので、**直前の `Transitioned.reason` が実際に `"answer"` のとき
        // だけ**再開する（`replan` を誤って人の回答扱いにしない）。
        if stuck {
            let just_answered = self
                .store
                .events_for(task_id)?
                .iter()
                .rev()
                .find_map(|(_, e)| match e {
                    Event::Transitioned { reason, .. } => Some(reason.clone()),
                    _ => None,
                })
                == Some("answer".to_string());
            if just_answered {
                let resumable: Vec<task_core::WorkUnitRow> = units
                    .iter()
                    .filter(|u| {
                        u.status == task_core::WorkUnitStatus::Blocked
                            && matches!(
                                u.blocked_reason,
                                Some(task_core::WorkUnitBlockedReason::Question)
                                    | Some(task_core::WorkUnitBlockedReason::Limit)
                                    // ADR-0072 D17 3.（Phase E4b 項目2）: replan の上限を使い切った
                                    // plan_issue も、人の回答で（Question と同じく `Ready` から
                                    // やり直す形で）再開できる。
                                    | Some(task_core::WorkUnitBlockedReason::PlanIssue)
                            )
                    })
                    .cloned()
                    .collect();
                for wu in resumable {
                    let resumed = crate::execution_scheduler::resume_after_answer(&wu);
                    self.store.work_unit_transition(
                        task_id,
                        resumed.clone(),
                        Event::WorkUnitTransitioned {
                            work_unit_id: wu.id.clone(),
                            key: wu.key.clone(),
                            from: task_core::WorkUnitStatus::Blocked,
                            to: resumed.status,
                            reason: "answer".to_string(),
                            run_id: None,
                        },
                    )?;
                }
                units = self.store.work_units_for(task_id)?;
            }
        }
        // ADR-0074「F5-fix8 実装時の明確化」: 仕事の残っていない計画（WU がすべて done、または有効な WU が
        // 1 つも無い）。v1 / v2 / v3 共通。採用の後にまだ最終レビューを受けていない版なら最終レビューへ、
        // 不合格の後なら従来どおり replan（D17 4.）。以前は採用の直後でも replan に回り、`max_replans` を
        // 使い切っていると `Skip` のまま黙って止まっていた（F5-fix8 の事故）。
        if task_core::plan_work_finished(&units) {
            return self.finished_plan_gate(task_id, &active_plan.id, &events);
        }
        if v2 {
            return self.wu_dispatch_gate_v2(task_id, units);
        }
        match task_core::next_work_unit(&units) {
            task_core::NextStep::RunWorkUnit(id) => {
                match units.into_iter().find(|u| u.id == id) {
                    Some(wu) => Ok(WuDispatchGate::RunWorkUnit(Box::new(wu))),
                    // 理論上到達しない（`next_work_unit` は `units` の中の id しか返さない）。
                    None => Ok(WuDispatchGate::Skip),
                }
            }
            // ADR-0072 D17（Phase E4）: 正常完了（`plan_complete`）は `on_worker_finished` が即座に
            // `WorkerDone` へ遷移させるので、`Ready` の Task をこの状態（全 WU done）で見るのは、
            // repair の上限を使い切った後の `ReviewFail`、または実質的な review 不合格の後の再
            // dispatch（D17 4.）だけ（理論上の一瞬の不整合を除く）。replan の余地があれば試す。
            task_core::NextStep::AllDone => self.replan_gate(task_id),
            task_core::NextStep::RunPlanner { replan } => Ok(WuDispatchGate::RunPlanner { replan }),
            task_core::NextStep::Stuck(reason) => {
                // ADR-0072 D17（Phase E4）/ D17 3.（Phase E4b 項目2）: WU が failed、または
                // blocked(dependency_failed/limit/plan_issue) のままで進められる WU が無いなら
                // replan の対象（D17 1./2./3.）。plan_issue は通常この分岐に来る前に即
                // `Continue{why: Replan}` で Ready に戻るので、ここに残るのは replan の上限を
                // 使い切った直後の一瞬（`replan_gate` が `Skip` を返す）だけ。
                let has_unresolved_failure = units.iter().any(|u| {
                    u.status == task_core::WorkUnitStatus::Failed
                        || (u.status == task_core::WorkUnitStatus::Blocked
                            && matches!(
                                u.blocked_reason,
                                Some(task_core::WorkUnitBlockedReason::DependencyFailed)
                                    | Some(task_core::WorkUnitBlockedReason::Limit)
                                    | Some(task_core::WorkUnitBlockedReason::PlanIssue)
                            ))
                });
                if has_unresolved_failure {
                    self.replan_gate(task_id)
                } else {
                    tracing::warn!(task_id = %task_id, %reason, "execution plan stuck; not dispatching this tick");
                    Ok(WuDispatchGate::Skip)
                }
            }
        }
    }

    /// ADR-0074 D3.7（Phase F4b (f)）: `depends_on: ["child:<key>"]` の WU を、子 Task（`child-<key>` の
    /// 印を持つ `parent_id = task_id` の Task）の状態で進める。子が `done` なら（他の依存も満たされて
    /// いれば）`pending → ready`、子が `failed` / `cancelled` なら `pending → blocked(dependency_failed)`
    /// （D17 の replan の対象）。戻り値は（書き換えたか、まだ終わっていない子を待っている WU があるか）。
    fn resolve_child_dependencies(
        &self,
        task_id: TaskId,
        units: &[task_core::WorkUnitRow],
    ) -> Result<(bool, bool), DispatchError> {
        let prefix = task_core::CHILD_DEP_PREFIX;
        let has_child_deps =
            |u: &task_core::WorkUnitRow| u.depends_on.iter().any(|d| d.starts_with(prefix));
        if !units
            .iter()
            .any(|u| u.status == task_core::WorkUnitStatus::Pending && has_child_deps(u))
        {
            return Ok((false, false));
        }
        let mut done: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut failed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for child in self.store.children(task_id)? {
            for label in &child.labels {
                let Some(key) = label.strip_prefix("child-") else {
                    continue;
                };
                let dep = format!("{prefix}{key}");
                match child.status {
                    Status::Done => {
                        done.insert(dep);
                    }
                    Status::Failed | Status::Cancelled => {
                        failed.insert(dep);
                    }
                    _ => {}
                }
            }
        }
        let mut changed = false;
        for u in units.iter().filter(|u| {
            u.status == task_core::WorkUnitStatus::Pending
                && u.depends_on.iter().any(|d| failed.contains(d))
        }) {
            let mut row = u.clone();
            row.status = task_core::WorkUnitStatus::Blocked;
            row.blocked_reason = Some(task_core::WorkUnitBlockedReason::DependencyFailed);
            self.store.work_unit_transition(
                task_id,
                row,
                Event::WorkUnitTransitioned {
                    work_unit_id: u.id.clone(),
                    key: u.key.clone(),
                    from: task_core::WorkUnitStatus::Pending,
                    to: task_core::WorkUnitStatus::Blocked,
                    reason: "dependency_failed".to_string(),
                    run_id: None,
                },
            )?;
            changed = true;
        }
        if !changed {
            for id in task_core::newly_ready_with(units, &done) {
                let Some(u) = units.iter().find(|u| u.id == id) else {
                    continue;
                };
                if !has_child_deps(u) {
                    continue;
                }
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
                        reason: "child_done".to_string(),
                        run_id: None,
                    },
                )?;
                changed = true;
            }
        }
        let waiting = units.iter().any(|u| {
            u.status == task_core::WorkUnitStatus::Pending
                && u.depends_on
                    .iter()
                    .any(|d| d.starts_with(prefix) && !done.contains(d) && !failed.contains(d))
        });
        Ok((changed, waiting))
    }

    /// ADR-0074 D1.3/D1.6（Phase F2b）: v2 の計画の Ready な Task が次に何をするか。
    fn wu_dispatch_gate_v2(
        &self,
        task_id: TaskId,
        units: Vec<task_core::WorkUnitRow>,
    ) -> Result<WuDispatchGate, DispatchError> {
        use crate::execution_scheduler::PhaseSettle;
        match crate::execution_scheduler::settle_phase(&units) {
            PhaseSettle::Integrate(id) => Ok(units
                .into_iter()
                .find(|u| u.id == id)
                .map(|u| WuDispatchGate::StartIntegration(Box::new(u)))
                .unwrap_or(WuDispatchGate::Skip)),
            // 全部 done（最終レビューの不合格の後など。v1 の `AllDone` と同じ）・工程の失敗は replan。
            PhaseSettle::AllDone | PhaseSettle::Failure(_) => self.replan_gate(task_id),
            PhaseSettle::Question(_) | PhaseSettle::Wait => Ok(WuDispatchGate::Skip),
            PhaseSettle::Advance => {
                let Some(task) = self.store.get(task_id)? else {
                    return Ok(WuDispatchGate::Skip);
                };
                let mode = self.parallel_mode(&task)?;
                let ids = task_core::runnable_work_units(&units, 0, mode.limit);
                Ok(ids
                    .first()
                    .and_then(|id| units.into_iter().find(|u| &u.id == id))
                    .map(|u| WuDispatchGate::RunWorkUnit(Box::new(u)))
                    .unwrap_or(WuDispatchGate::Skip))
            }
        }
    }

    /// ADR-0074 D1.2（Phase F2b）: v2 の Task を並列でどう走らせるか（決定的）。v1・atomic・計画の
    /// 無い Task は並列 1（WU の worktree なし）。remote / `Shared` / 書き込み可能な `dir` の repo /
    /// git の worktree が無い Task は並列 1 に倒し、理由を返す。
    fn parallel_mode(&self, task: &Task) -> Result<ParallelMode, DispatchError> {
        let serial = |reason: Option<String>| ParallelMode {
            limit: 1,
            worktrees: false,
            fallback: reason,
        };
        let Some(plan) = self.store.execution_plan_active(task.id)? else {
            return Ok(serial(None));
        };
        if !task_core::is_phased_schema(&plan.spec.schema) {
            return Ok(serial(None));
        }
        match &task.workspace {
            WorkspaceSpec::Remote { .. } => {
                return Ok(serial(Some(
                    "remote workspace: no work unit worktrees on the cluster (ADR-0074 D1.2)"
                        .to_string(),
                )));
            }
            WorkspaceSpec::Local {
                mode: Some(WorkspaceMode::Shared),
                ..
            } => {
                return Ok(serial(Some(
                    "workspace_mode = shared: work units share the task's directory (ADR-0074 D1.2)"
                        .to_string(),
                )));
            }
            WorkspaceSpec::Local { .. } => {}
        }
        let Some(ws) = self.task_workspaces_for(task) else {
            return Ok(serial(Some(
                "no git worktree for this task (the local path is not a git repository)"
                    .to_string(),
            )));
        };
        if ws.repos.is_empty() {
            return Ok(serial(Some(
                "no repository in this task's workspace".to_string(),
            )));
        }
        if let Some(repo) = ws.repos.iter().find(|r| !r.is_git()) {
            return Ok(serial(Some(format!(
                "repository {} is a writable dir (not git): work units would share it (ADR-0074 D1.2)",
                repo.name
            ))));
        }
        Ok(ParallelMode {
            limit: self
                .config
                .execution
                .max_parallel_work_units
                .clamp(1, MAX_PARALLEL_WORK_UNITS_CAP),
            worktrees: true,
            fallback: None,
        })
    }

    /// ADR-0074 D1.2（Phase F2b）: 並列 1 に倒した理由を計画ごとに 1 回だけ残す。
    fn record_serialized(&self, task_id: TaskId, plan_id: &str, reason: &str) {
        let already = self.store.events_for(task_id).is_ok_and(|events| {
            events.iter().any(
                |(_, e)| matches!(e, Event::WorkUnitsSerialized { plan_id: p, .. } if p == plan_id),
            )
        });
        if already {
            return;
        }
        if let Err(e) = self.store.append_event(
            task_id,
            &Event::WorkUnitsSerialized {
                plan_id: plan_id.to_string(),
                reason: reason.to_string(),
            },
        ) {
            tracing::warn!(%task_id, error = %e, "failed to record why the work units run serially");
        }
    }

    /// ADR-0074 D1.2（Phase F2b）: WU の run の worktree を用意する（冪等）。
    /// - 並列 1 に倒した Task（理由を記録）・統合の repair WU（Task の worktree で走る）は `Ok(None)`。
    /// - 基点は、既にブランチがあればそれを使い回し、無ければ同じ工程の依存先の WU ブランチの HEAD
    ///   （積み上げ）か Task ブランチの HEAD。依存先の WU ブランチが無ければ
    ///   `integration::dependency_base`（F5-fix7: 依存先の記録した commit か Task ブランチの HEAD）。
    fn prepare_work_unit_workspace(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
        task_ws: Option<&task_worker::TaskWorkspaces>,
    ) -> Result<Option<WorkUnitWorkspace>, WuPrepareError> {
        let mode = self
            .parallel_mode(task)
            .map_err(|e| WuPrepareError::transient(e.to_string()))?;
        if let Some(reason) = &mode.fallback {
            self.record_serialized(task.id, &wu.plan_id, reason);
            return Ok(None);
        }
        if !mode.worktrees || wu.kind == task_core::WorkUnitKind::Repair {
            return Ok(None);
        }
        let Some(ws) = task_ws else {
            return Ok(None);
        };
        // Task の worktree（基点のブランチ）を先に用意する。
        for repo in &ws.repos {
            if let Some(wt) = &repo.worktree {
                wt.ensure_blocking().map_err(|e| {
                    WuPrepareError::transient(format!("cannot prepare the task worktree: {e}"))
                })?;
            }
        }
        let units = self
            .store
            .work_units_for(task.id)
            .map_err(|e| WuPrepareError::transient(e.to_string()))?;
        let intra_dep = wu.depends_on.iter().find_map(|d| {
            units
                .iter()
                .find(|u| &u.key == d && u.phase.is_some() && u.phase == wu.phase)
        });
        let task_id = task.id.to_string();
        let branch = crate::integration::wu_branch(&task_id, &wu.key);
        let wu_dir = crate::integration::wu_dir(&ws.task_dir, &wu.key);
        let mut repos = Vec::new();
        let mut first_base: Option<String> = None;
        for repo in &ws.repos {
            let Some(task_wt) = &repo.worktree else {
                continue;
            };
            let existing =
                crate::integration::rev_parse(&repo.source, &format!("refs/heads/{branch}"));
            let base = match (&existing, intra_dep) {
                (Some(_), _) => wu
                    .base_commit
                    .clone()
                    .or_else(|| existing.clone())
                    .unwrap_or_default(),
                // ADR-0074「Phase F5-fix7 実装時の明確化」: 依存先の WU ブランチが無い（Task の worktree で
                // 走った repair / 統合 WU、ref が消えた）ときは、依存先の記録した commit か Task ブランチの
                // HEAD に倒す（`integration::dependency_base`）。解決できなければ時間では直らない。
                (None, Some(dep_row)) => crate::integration::dependency_base(
                    &repo.source,
                    &task_id,
                    dep_row,
                    &task_wt.branch,
                    &self.config.worktree_branch_prefix,
                )
                .map_err(WuPrepareError::permanent)?,
                (None, None) => crate::integration::rev_parse(
                    &repo.source,
                    &format!("refs/heads/{}", task_wt.branch),
                )
                .ok_or_else(|| {
                    WuPrepareError::permanent(format!(
                        "task branch {} does not exist",
                        task_wt.branch
                    ))
                })?,
            };
            let lwt = crate::integration::wu_worktree(
                &ws.task_dir,
                &task_id,
                &wu.key,
                &repo.name,
                &repo.source,
                &base,
            );
            crate::integration::ensure_wu_worktree(&lwt).map_err(WuPrepareError::transient)?;
            first_base.get_or_insert(base);
            repos.push(task_worker::TaskRepo::git(repo.name.clone(), lwt));
        }
        if repos.is_empty() {
            return Ok(None);
        }
        Ok(Some(WorkUnitWorkspace {
            workspaces: task_worker::TaskWorkspaces {
                task_dir: wu_dir.clone(),
                repos,
            },
            branch,
            base: wu.base_commit.clone().or(first_base).unwrap_or_default(),
            artifacts_dir: wu_dir.join(task_core::artifacts::ARTIFACTS_DIR_NAME),
        }))
    }

    /// ADR-0074「Phase F5-fix7 実装時の明確化」: WU の worktree を用意できなかった。黙って tick ごとに
    /// やり直し続けない（本番 2026-09-28 の 20 分の停止）:
    /// - 一時的な失敗は [`MAX_WU_PREPARE_ATTEMPTS`] 回まで [`wu_prepare_backoff`] で待ってやり直す。
    /// - 時間で直らない失敗（依存先の成果が解決できない等）と、上限を使い切った一時的な失敗は、WU を
    ///   `blocked(question)`（`WorkUnitTransitioned{reason: "prepare_failed"}`）にし、理由を
    ///   `worker_progress` に残す。Task が Ready（1 本目）なら `approvals` に 1 件作って `Trigger::Unroutable`
    ///   で `ready → blocked`（`QuestionRaised`。ADR-0062 B1 の `block_task_missing_cluster_tool` と同じ出口）。
    ///   人が直して回答すると、D18 の `answer` の経路でこの WU が ready に戻り、もう一度用意を試す。
    ///   並列の 2 本目以降（Task は Running）は WU だけ blocked にし、兄弟の run が終わったときの
    ///   `settle_phase` → `Question` が Task を blocked にする。
    fn on_work_unit_prepare_failed(
        &mut self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
        error: &WuPrepareError,
        second_pass: bool,
    ) -> Result<(), DispatchError> {
        let now = self.now_utc();
        if !error.permanent {
            let count = self
                .wu_prepare_failures
                .get(&wu.id)
                .map_or(0, |f| f.count)
                .saturating_add(1);
            if count < MAX_WU_PREPARE_ATTEMPTS {
                let retry_at = now + wu_prepare_backoff(count);
                self.wu_prepare_failures
                    .insert(wu.id.clone(), WuPrepareFailures { count, retry_at });
                tracing::warn!(task_id = %task.id, work_unit = %wu.key, error = %error, attempt = count, max_attempts = MAX_WU_PREPARE_ATTEMPTS, %retry_at, "cannot prepare the work unit worktree (transient); retrying after a backoff");
                return Ok(());
            }
        }
        self.wu_prepare_failures.remove(&wu.id);
        tracing::warn!(task_id = %task.id, work_unit = %wu.key, error = %error, permanent = error.permanent, "cannot prepare the work unit worktree; blocking the work unit and asking a human (reason=prepare_failed)");
        let mut row = wu.clone();
        row.status = task_core::WorkUnitStatus::Blocked;
        row.blocked_reason = Some(task_core::WorkUnitBlockedReason::Question);
        row.clear_lease();
        row.updated_at = rfc3339(now);
        self.store.work_unit_transition(
            task.id,
            row,
            Event::WorkUnitTransitioned {
                work_unit_id: wu.id.clone(),
                key: wu.key.clone(),
                from: wu.status,
                to: task_core::WorkUnitStatus::Blocked,
                reason: "prepare_failed".to_string(),
                run_id: None,
            },
        )?;
        let question = format!(
            "WorkUnit `{}` の作業場所（git worktree）を用意できないため、この WU を止めました: {}。\
             依存先のブランチ・Task ブランチ・git の状態を直してから回答すると、この WU の用意をやり直します。",
            wu.key, error.message
        );
        let progress = Event::worker_progress(
            "prepare",
            format!("prepare_failed: work unit {}: {}", wu.key, error.message),
        );
        if second_pass {
            self.store.append_event(task.id, &progress)?;
            return Ok(());
        }
        if let Err(e) = crate::approvals::record_question_approval(
            self.store.as_ref(),
            task,
            &question,
            OffsetDateTime::now_utc(),
        ) {
            tracing::warn!(task_id = %task.id, error = %e, "failed to record the approval for the work unit prepare failure");
        }
        let events = vec![
            progress,
            Event::QuestionRaised {
                run_id: format!("wu-prepare-{}", wu.id),
                text: question,
            },
        ];
        match self
            .store
            .apply_transition_with_events(task.id, Trigger::Unroutable, events)
        {
            Ok(_) => Ok(()),
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(task_id = %task.id, error = %e, "prepare-failed transition could not be applied");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// ADR-0074 D1.2（Phase F2b）: v2 の WU が作業したツリー（`(dir, branch)`。repo ごと）。WU の
    /// worktree を持つ WU はそのツリー、統合の repair WU のように Task の worktree で走った WU は Task の
    /// worktree。並列 1 に倒した Task・v1 は空（daemon は commit しない）。
    fn work_unit_trees(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
    ) -> Result<Vec<(PathBuf, String)>, DispatchError> {
        if wu.phase.is_none() {
            return Ok(Vec::new());
        }
        let Some(ws) = self.task_workspaces_for(task) else {
            return Ok(Vec::new());
        };
        if let Some(branch) = &wu.branch {
            let wu_dir = crate::integration::wu_dir(&ws.task_dir, &wu.key);
            return Ok(ws
                .repos
                .iter()
                .filter(|r| r.is_git())
                .map(|r| {
                    (
                        wu_dir
                            .join(task_worker::task_repos::REPOS_DIR_NAME)
                            .join(&r.name),
                        branch.clone(),
                    )
                })
                .collect());
        }
        if !self.parallel_mode(task)?.worktrees {
            return Ok(Vec::new());
        }
        Ok(ws
            .repos
            .iter()
            .filter_map(|r| r.branch().map(|b| (r.dir.clone(), b.to_string())))
            .collect())
    }

    /// ADR-0074 D1.2（Phase F2b）: WU の run が done になったら、その作業ツリーで決定的に commit する
    /// （作者は celeris の固定値。変更が無ければ commit しない）。`WorkUnitCommitted` を返し、
    /// `wu.head_commit` を書き換える。ツリーが無ければ（並列 1・v1）`None`。
    fn commit_work_unit(
        &self,
        task: &Task,
        wu: &mut task_core::WorkUnitRow,
    ) -> Result<Option<Event>, DispatchError> {
        let trees = self.work_unit_trees(task, wu)?;
        let mut first: Option<(String, String)> = None;
        for (dir, branch) in trees {
            if !dir.is_dir() {
                continue;
            }
            match crate::integration::commit_all(
                &dir,
                &crate::integration::commit_message(&wu.key, &wu.spec.title),
            ) {
                Ok((head, _)) => {
                    first.get_or_insert((head, branch));
                }
                Err(e) => {
                    tracing::warn!(task_id = %task.id, work_unit = %wu.key, error = %e, "could not commit the work unit's changes");
                }
            }
        }
        let Some((head, branch)) = first else {
            return Ok(None);
        };
        wu.head_commit = Some(head.clone());
        Ok(Some(Event::WorkUnitCommitted {
            work_unit_id: wu.id.clone(),
            key: wu.key.clone(),
            branch,
            base: wu.base_commit.clone(),
            commit: head,
        }))
    }

    /// ADR-0074 D1.6（Phase F2b）: checkpoint の mechanical な欄を取る場所（`(artifacts_dir, cwd,
    /// branch, base)`）。WU の worktree を持つ v2 の WU は WU の worktree・WU のブランチ・`base_commit`、
    /// それ以外は従来どおり Task の作業場所。
    fn work_unit_checkpoint_site(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
    ) -> (Option<PathBuf>, Option<PathBuf>, String, Option<String>) {
        let workspaces = self.task_workspaces_for(task);
        if let Some(branch) = &wu.branch
            && let Some(ws) = &workspaces
        {
            let wu_dir = crate::integration::wu_dir(&ws.task_dir, &wu.key);
            let cwd = ws.repos.iter().find(|r| r.is_git()).map(|r| {
                wu_dir
                    .join(task_worker::task_repos::REPOS_DIR_NAME)
                    .join(&r.name)
            });
            return (
                Some(wu_dir.join(task_core::artifacts::ARTIFACTS_DIR_NAME)),
                cwd,
                branch.clone(),
                wu.base_commit.clone(),
            );
        }
        let workspace_dir = self.task_dir(task);
        let artifacts_dir = workspace_dir.as_ref().map(|d| self.artifacts_dir(task, d));
        let cwd = workspaces
            .as_ref()
            .and_then(|w| w.cwd())
            .map(Path::to_path_buf);
        let branch = workspaces
            .as_ref()
            .and_then(|w| w.repos.first())
            .and_then(|r| r.branch())
            .unwrap_or_default()
            .to_string();
        // ADR-0079 D6（Phase R1c）: 木の子の差分の基点は `tree.base_commit`（main との merge-base ではない）。
        let base = task_core::tree::child_base_commit(task).map(str::to_string);
        (artifacts_dir, cwd, branch, base)
    }

    /// ADR-0074 D1.6（Phase F2b）: 兄弟が走っている間に question / 失敗で止まった WU（`id`）について、
    /// in-flight が 0 になった今 Task をどう遷移させるか（trigger と人への質問）。
    fn deferred_work_unit_trigger(
        &self,
        task_id: TaskId,
        units: &[task_core::WorkUnitRow],
        id: &str,
    ) -> Result<(Trigger, String, Vec<String>), DispatchError> {
        let Some(wu) = units.iter().find(|u| u.id == id) else {
            return Ok((
                Trigger::Continue {
                    why: task_core::ContinueWhy::Advance,
                },
                String::new(),
                Vec::new(),
            ));
        };
        let events = self.store.events_for(task_id)?;
        let last_outcome = wu
            .last_run_id
            .as_deref()
            .and_then(|rid| {
                events.iter().rev().find_map(|(_, e)| match e {
                    Event::WorkerFinished {
                        run_id, outcome, ..
                    } if run_id == rid => Some(outcome.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();
        let question_text = last_outcome
            .strip_prefix("question: ")
            .unwrap_or(last_outcome.as_str())
            .to_string();
        if wu.status == task_core::WorkUnitStatus::Blocked
            && wu.blocked_reason == Some(task_core::WorkUnitBlockedReason::Question)
        {
            let text = if question_text.is_empty() {
                format!("WorkUnit {} が質問しています", wu.key)
            } else {
                question_text
            };
            return Ok((
                Trigger::WorkerQuestion,
                format!("question: {text}"),
                vec![text],
            ));
        }
        let replans_so_far = self
            .store
            .execution_plan_list(task_id)?
            .len()
            .saturating_sub(1) as u32;
        if replans_so_far < self.effective_max_replans(task_id)? {
            let why = match (wu.status, wu.blocked_reason) {
                (task_core::WorkUnitStatus::Failed, _) => format!("work unit {} failed", wu.key),
                (_, Some(task_core::WorkUnitBlockedReason::Limit)) => {
                    format!("work unit {} made no progress", wu.key)
                }
                (_, Some(task_core::WorkUnitBlockedReason::PlanIssue)) => {
                    format!("work unit {} reported a plan issue", wu.key)
                }
                _ => format!("work unit {} is blocked by a failed dependency", wu.key),
            };
            return Ok((
                Trigger::Continue {
                    why: task_core::ContinueWhy::Replan,
                },
                format!("replan: {why}"),
                Vec::new(),
            ));
        }
        if matches!(wu.status, task_core::WorkUnitStatus::Failed)
            || wu.blocked_reason == Some(task_core::WorkUnitBlockedReason::DependencyFailed)
        {
            return Ok((
                Trigger::WorkerError { retryable: false },
                format!("error(retryable=false): work unit {} failed", wu.key),
                Vec::new(),
            ));
        }
        let text = if question_text.is_empty() {
            format!(
                "WorkUnit {} が進みません。続け方を指示してください。",
                wu.key
            )
        } else {
            question_text
        };
        Ok((
            Trigger::WorkerQuestion,
            format!("question: {text}"),
            vec![text],
        ))
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

    /// D15/ADR-0074 D1.4: 最終レビューに渡す決定的な要約（WU ごとの最終 checkpoint の `completed` を
    /// 1 段落ずつ。統合 WU は除く）。
    fn plan_summary(&self, units: &[task_core::WorkUnitRow]) -> String {
        let mut paragraphs = Vec::new();
        for u in units {
            if u.kind == task_core::WorkUnitKind::Integrate
                || u.status != task_core::WorkUnitStatus::Done
            {
                continue;
            }
            let completed = u
                .last_run_id
                .as_deref()
                .and_then(|rid| self.store.run_index_get(rid).ok().flatten())
                .and_then(|r| r.checkpoint)
                .map(|cp| cp.completed.join("; "))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "完了".to_string());
            paragraphs.push(format!("{}: {}", u.spec.title, completed));
        }
        paragraphs.join("\n")
    }

    /// ADR-0074 D1.7（Phase F2b）: tick の最初に、v2（工程の lease）の Task を照合する。
    /// - WU が running で、WU の lease が切れていて、このインスタンスの run でもない → 戻す。
    /// - Task が Running で、WU も統合も走っていない → `Continue{advance}`（Ready に戻して通常の経路へ）。
    ///
    /// Task の lease ごと切れた（全部が死んだ）Task は `reclaim_expired_leases` → `InfraRequeue` が扱う。
    ///
    /// Phase F5-fix6: WU の lease がまだ切れていなくても、その run の持ち主のデーモンが居なければ
    /// （孤児。`crate::orphan`）同じく戻す（result.json があればその内容で確定、無ければ reason
    /// `orphan_takeover` で ready / needs_continuation）。統合 WU も同じ（spawn が手元に無ければ pending）。
    fn reconcile_parallel_tasks(&mut self) -> Result<(), DispatchError> {
        let now = OffsetDateTime::now_utc();
        let mut holders_gone: Option<bool> = None;
        for task in self.store.list(Some(Status::Running))? {
            if !self.is_eligible(&task) {
                continue;
            }
            let Some(lease) = &task.lease else { continue };
            if !is_phase_lease_holder(&lease.worker_run_id) || lease.expires_at <= now {
                continue;
            }
            let units = self.store.work_units_for(task.id)?;
            for u in units.iter().filter(|u| {
                u.status == task_core::WorkUnitStatus::Running
                    && u.kind != task_core::WorkUnitKind::Integrate
            }) {
                let Some(run_id) = u.lease_run_id.clone().or_else(|| u.last_run_id.clone()) else {
                    continue;
                };
                // Phase F5-fix2: 検査中の run（`checking`）も手元の run。
                let ours = self.running.values().any(|e| e.run_id == run_id)
                    || self.checking.contains_key(&run_id);
                let expired = u
                    .lease_expires_at
                    .as_deref()
                    .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                    .is_none_or(|t| t <= now);
                if ours {
                    continue;
                }
                if expired {
                    if self.finalise_from_result_json(&task, &run_id) {
                        continue;
                    }
                    tracing::warn!(task_id = %task.id, work_unit = %u.key, %run_id, "work unit lease expired without a live run; reconciling (ADR-0074 D1.7)");
                    self.reconcile_work_unit_run(task.id, &run_id, "restart_reconcile")?;
                } else if self.lease_holders_gone(&mut holders_gone, now) {
                    // Phase F5-fix6: 持ち主のデーモンが居ない WU の run。lease の失効を待たない。
                    let until = u
                        .lease_expires_at
                        .as_deref()
                        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                        .unwrap_or(now);
                    self.note_orphan_takeover(&task, &run_id, until);
                    if self.finalise_from_result_json(&task, &run_id) {
                        continue;
                    }
                    self.requeue_orphaned_work_unit_run(&task, &run_id)?;
                }
            }
            // Phase F5-fix6: 持ち主の居ない工程の統合（統合 WU が running で、spawn が手元に無い）。
            if !self.integrating.contains_key(&task.id)
                && self.running_for_task(task.id) == 0
                && units.iter().any(|u| {
                    u.kind == task_core::WorkUnitKind::Integrate
                        && u.status == task_core::WorkUnitStatus::Running
                })
                && self.lease_holders_gone(&mut holders_gone, now)
            {
                tracing::warn!(task_id = %task.id, "the phase integration's daemon is gone; returning the integration to pending without waiting for the lease (Phase F5-fix6 orphan_takeover)");
                self.reconcile_integration(task.id, crate::orphan::ORPHAN_TAKEOVER_REASON)?;
            }
            if self.running_for_task(task.id) > 0 || self.integrating.contains_key(&task.id) {
                continue;
            }
            let units = self.store.work_units_for(task.id)?;
            // ADR-0079 D5（Phase R1b）: kind task の unit の `running` は子 task の写し（この Task の run ではない）。
            if units.iter().any(|u| {
                u.status == task_core::WorkUnitStatus::Running
                    && u.kind != task_core::WorkUnitKind::Task
            }) {
                continue;
            }
            tracing::warn!(task_id = %task.id, "running v2 task has no work unit or integration in flight; returning it to ready (ADR-0074 D1.7)");
            match self.store.apply_transition_with_events(
                task.id,
                Trigger::Continue {
                    why: task_core::ContinueWhy::Advance,
                },
                vec![],
            ) {
                Ok(_) | Err(StoreError::InvalidTransition(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// ADR-0074 D1.6（Phase F2b）/ ADR-0072 D6: Cancel（取り下げ）で、未完了の WU を cancelled にし、
    /// WU の worktree とブランチを消す（ADR-0043 D2 の中止の規則）。
    fn cancel_open_work_units(&self, task: &Task) -> Result<(), DispatchError> {
        let units = self.store.work_units_for(task.id)?;
        let mut rows = Vec::new();
        let mut events = Vec::new();
        for u in units.iter().filter(|u| !u.status.is_terminal()) {
            let mut row = u.clone();
            row.status = task_core::WorkUnitStatus::Cancelled;
            row.blocked_reason = None;
            row.clear_lease();
            row.updated_at = rfc3339(OffsetDateTime::now_utc());
            events.push(Event::WorkUnitTransitioned {
                work_unit_id: u.id.clone(),
                key: u.key.clone(),
                from: u.status,
                to: task_core::WorkUnitStatus::Cancelled,
                reason: "cancel".to_string(),
                run_id: None,
            });
            rows.push(row);
        }
        if !rows.is_empty() {
            self.store
                .work_units_apply(task.id, Vec::new(), rows, events)?;
        }
        if let Some(ws) = self.task_workspaces_for(task) {
            for u in units.iter().filter(|u| u.branch.is_some()) {
                for repo in ws.repos.iter().filter(|r| r.is_git()) {
                    let lwt = crate::integration::wu_worktree(
                        &ws.task_dir,
                        &task.id.to_string(),
                        &u.key,
                        &repo.name,
                        &repo.source,
                        u.base_commit.as_deref().unwrap_or_default(),
                    );
                    let _ = lwt.remove_with_branch();
                    if let Some(parent) = lwt.dir.parent() {
                        let _ = std::fs::remove_dir(parent);
                    }
                }
            }
        }
        Ok(())
    }

    /// ADR-0072 D14: 直近の `ExecutionPlanned`（無ければ Task の最初）から数えた planner run の試行
    /// （`WorkerStarted{role: Planner}` の件数。この run 自身を含む）。
    fn planner_attempts_in_window(&self, task_id: TaskId) -> Result<usize, DispatchError> {
        let events = self.store.events_for(task_id)?;
        // ADR-0079 D7（Phase R3a）: `plan_invalid` への回答（replan）も窓を開け直す（人の指示つきで planner を
        // もう一度 2 回まで試す）。
        let since_idx = events
            .iter()
            .rposition(|(_, e)| matches!(e, Event::ExecutionPlanned { .. }))
            .max(plan_invalid_answer_position(&events));
        Ok(events
            .iter()
            .enumerate()
            .filter(|(i, (_, e))| {
                since_idx.is_none_or(|s| *i > s)
                    && matches!(
                        e,
                        Event::WorkerStarted {
                            role: Some(RunRole::Planner),
                            ..
                        }
                    )
            })
            .count())
    }

    /// ADR-0079 D4 (3) / D3（Phase R2a）: 採用する /3 の計画に unit の gate をかけ（上げる・下げるを spec に
    /// 当てる）、採用の直後に止める unit（子 task にできない compound な leaf、上限を超える unit）を決める。
    /// /3 でない・木が無効なら何もしない（`None`）。上げる・下げるで計画の上限を超えうるので、spec を変えた
    /// ときは `adopt_limits` を計画の上限を外したものにする（超えた分は `holds` が止める）。変えた spec が
    /// 他の理由で検証に落ちれば（通常起きない）、元の spec のまま採用し、上げる・下げるは当てない（警告）。
    fn tree_plan_gate(
        &self,
        task: &Task,
        validated: task_core::execution_plan::ValidatedPlan,
        done_work_units: &[(String, task_core::WorkUnitSpec)],
        adopt_limits: &mut task_core::ExecutionLimits,
    ) -> (
        task_core::execution_plan::ValidatedPlan,
        Option<TreePlanOutcome>,
    ) {
        // ADR-0079 R5b-prep: 人の計画（`PUT`）と同じ関数（`task_ops::tree_plan::unit_gate_plan`）。
        task_ops::tree_plan::unit_gate_plan(
            self.store.as_ref(),
            task,
            validated,
            done_work_units,
            self.config.execution.limits,
            adopt_limits,
            task_core::PlanOrigin::Planner,
        )
    }

    /// ADR-0079 D4 (3) / D3（Phase R2a）: 採用した /3 の計画について、unit の gate の不一致
    /// （`UnitGateOverridden`）と、決定の要求（`leaf_too_large` / `limit`。D7 の形、path 付き、`decisions` の行）
    /// と、止める unit の `blocked(decision)` を 1 トランザクションで残す。止めるのは `pending` / `ready` の行
    /// だけ（replan で持ち越した走っている・終わった行は止めない）。同じ段階の他の unit・兄弟は止めない。
    fn apply_tree_plan_holds(
        &self,
        task: &Task,
        plan: &task_core::ExecutionPlanRow,
        run_id: &str,
        outcome: TreePlanOutcome,
        now: OffsetDateTime,
    ) -> Result<(), DispatchError> {
        // ADR-0079 R5b-prep: 人の計画（`PUT`）と同じ関数（`task_ops::tree_plan::plan_hold_writes`）。planner の経路は
        // 採用の後の行を store から読み、止めを続きの 1 トランザクションで書く（R2a / R3a のまま）。
        let units = self.store.work_units_for(task.id)?;
        let writes = task_ops::tree_plan::plan_hold_writes(
            self.store.as_ref(),
            task,
            plan,
            &units,
            Some(run_id),
            task_core::DecisionOrigin::Planner,
            outcome,
            now,
        )
        .map_err(ops_to_store)?;
        if !writes.events.is_empty() {
            self.store
                .work_units_apply(task.id, Vec::new(), writes.rows, writes.events)?;
        }
        Ok(())
    }

    /// ADR-0079 D10（Phase R3b）: 木の生存確認。`ready` の木の節点（木の子・/3 の計画を持つ root）を
    /// `task_core::tree::liveness` で分け、「走っている / 走れる / 名指しの待ち」のどれでもない（`Unexplained`）まま
    /// `liveness_timeout_secs` 続いたら、`Event::StallDetected` と障害通知（`TaskFailed`、key
    /// `tree-stall:<task_id>:<StallDetected の seq>`）を 1 回だけ出す。同じ止まり方の間は繰り返さない（節点の最後の
    /// event が `StallDetected` の間は見送る）。`running` / `reviewing`（lease・レビュー）と `blocked`（人の質問・
    /// 途中確認・計画の承認）は常に名指しの状態なので `ready` だけを見る。判定は store の読み取りだけで、LLM なし。
    /// ADR-0074「F5-fix8 実装時の明確化」: 木が無効でも、また木の節点でなくても、有効な計画を持つ `ready` の
    /// Task（/1・/2 の計画）を同じ規則で見る（通知の key は `stall:<task_id>:<seq>`。木の節点は従来どおり
    /// `tree-stall:`）。計画を持たない atomic の Task は対象外。
    fn check_tree_liveness(&mut self) -> Result<(), DispatchError> {
        let tree = self.config.execution.limits.tree;
        let now = self.now_utc();
        if let Some(last) = self.liveness_checked_at
            && (now - last).whole_seconds() < LIVENESS_CHECK_INTERVAL_SECS
        {
            return Ok(());
        }
        self.liveness_checked_at = Some(now);
        let mut nodes: Vec<Task> = Vec::new();
        let mut tree_nodes: std::collections::HashSet<TaskId> = std::collections::HashSet::new();
        for t in self.store.list(Some(Status::Ready))? {
            if tree.enabled && self.is_tree_node(&t)? {
                tree_nodes.insert(t.id);
                nodes.push(t);
            } else if self.store.execution_plan_active(t.id)?.is_some() {
                nodes.push(t);
            }
        }
        if nodes.is_empty() {
            self.stall_watch.clear();
            return Ok(());
        }
        let eligible: std::collections::HashSet<TaskId> = self
            .store
            .ready_tasks(100_000)?
            .into_iter()
            .map(|t| t.id)
            .collect();
        let mut facts = Vec::with_capacity(nodes.len());
        for t in &nodes {
            let replans_so_far = self
                .store
                .execution_plan_list(t.id)?
                .len()
                .saturating_sub(1) as u32;
            let replans_left = replans_so_far < self.effective_max_replans(t.id)?;
            facts.push(
                task_ops::tree::node_liveness_facts(
                    self.store.as_ref(),
                    t,
                    eligible.contains(&t.id),
                    replans_left,
                )
                .map_err(ops_to_store)?,
            );
        }
        let verdicts = task_core::tree::liveness(&task_core::TreeSnapshot { nodes: facts });
        let mut watching: std::collections::HashSet<TaskId> = std::collections::HashSet::new();
        for v in verdicts {
            if v.class != task_core::LivenessClass::Unexplained {
                continue;
            }
            let events = self.store.events_for(v.task_id)?;
            let Some((last_seq, last)) = events.last() else {
                continue;
            };
            if matches!(last, Event::StallDetected { .. }) {
                // 既に知らせた止まり方（何か起きるまで繰り返さない）。
                continue;
            }
            watching.insert(v.task_id);
            let since = match self.stall_watch.get(&v.task_id) {
                Some((seq, since)) if seq == last_seq => *since,
                _ => {
                    self.stall_watch.insert(v.task_id, (*last_seq, now));
                    now
                }
            };
            let elapsed = (now - since).whole_seconds();
            if elapsed < i64::try_from(tree.liveness_timeout_secs).unwrap_or(i64::MAX) {
                continue;
            }
            let Some(task) = nodes.iter().find(|t| t.id == v.task_id) else {
                continue;
            };
            let path =
                task_ops::tree::decision_path(self.store.as_ref(), task).map_err(ops_to_store)?;
            let breadcrumb = path
                .iter()
                .map(|p| p.title.as_str())
                .collect::<Vec<_>>()
                .join(" › ");
            let seq = self.store.append_event(
                v.task_id,
                &Event::StallDetected {
                    task_id: v.task_id,
                    detail: v.detail.clone(),
                    reason: v.reason.clone(),
                    since: rfc3339(since),
                    path,
                },
            )?;
            let is_tree_node = tree_nodes.contains(&v.task_id);
            let body = format!(
                "障害（stall）: 『{}』が理由なく止まっています（{} 秒以上。{}）: {}。位置: {}（{}）",
                task.title,
                elapsed,
                v.reason,
                v.detail,
                breadcrumb,
                if is_tree_node {
                    "ADR-0079 D10"
                } else {
                    "ADR-0074 F5-fix8"
                }
            );
            tracing::error!(task_id = %v.task_id, reason = %v.reason, detail = %v.detail, elapsed, tree = is_tree_node, "a task with an execution plan is stalled without a named wait (ADR-0079 D10 / ADR-0074 F5-fix8)");
            let key_prefix = if is_tree_node { "tree-stall" } else { "stall" };
            if let Err(e) = self.store.notification_upsert_pending(
                NotificationKind::TaskFailed,
                &format!("{key_prefix}:{}:{seq}", v.task_id),
                &body,
                task.project_id,
                now,
            ) {
                tracing::error!(task_id = %v.task_id, error = %e, "failed to record the stall notification");
            }
            self.stall_watch.remove(&v.task_id);
            watching.remove(&v.task_id);
        }
        self.stall_watch.retain(|id, _| watching.contains(id));
        Ok(())
    }

    /// ADR-0079 D8（Phase R3b）: 採用した計画が root の /3 の計画なら、承認の要否（`task_core::tree::plan_approval`）。
    /// 木が無効・/3 でない・木の子（子の計画は承認を求めない）なら `None`（従来どおり進める。報告も残さない）。
    fn root_plan_approval(
        &self,
        task: &Task,
        plan: &task_core::ExecutionPlanRow,
    ) -> Result<Option<task_core::PlanApproval>, DispatchError> {
        if !self.config.execution.limits.tree.enabled
            || plan.spec.schema != task_core::EXECUTION_PLAN_SCHEMA_V3
            || task_core::tree::is_tree_child(task)
        {
            return Ok(None);
        }
        let facts = task_ops::plan_gate::approval_facts(self.store.as_ref(), task, plan)
            .map_err(ops_to_store)?;
        let limits = self.effective_tree_limits(task_core::tree::root_id_of(task))?;
        Ok(Some(task_core::tree::plan_approval(&facts, &limits)))
    }

    /// ADR-0079 D9（Phase R2b）: この task が出した未回答の `kind: plan_invalid` の決定。
    fn open_plan_invalid(
        &self,
        task: &Task,
    ) -> Result<Option<task_core::DecisionRow>, DispatchError> {
        let root_id = task_core::tree::root_id_of(task);
        Ok(self
            .store
            .decisions_list(Some(root_id))?
            .into_iter()
            .find(|d| {
                d.task_id == task.id
                    && d.kind == task_core::DecisionKind::PlanInvalid
                    && d.status == task_core::DecisionStatus::Open
            }))
    }

    /// ADR-0079 D7（Phase R3a）: 木の節点の worker の run の `result.json` の `decisions` を読み、検証して
    /// （`task_core::decision::prepare_worker_decisions`: 形・指す先・key の重複・`max_open_decisions` の残りを超えた分の
    /// 束ね）、記録する event と止める unit を組み立てる。書き込みはしない。木が無効・木の節点でない・ファイルが
    /// 無い・`decisions` が無ければ `None`（従来と 1 バイトも変わらない）。
    fn worker_decisions(
        &self,
        task: &Task,
        wu: Option<&task_core::WorkUnitRow>,
        artifacts_dir: Option<&Path>,
        run_id: &str,
    ) -> Result<Option<WorkerDecisions>, DispatchError> {
        let limits = self.config.execution.limits.tree;
        if !limits.enabled || !self.is_tree_node(task)? {
            return Ok(None);
        }
        let Some(dir) = artifacts_dir else {
            return Ok(None);
        };
        let Ok(text) = std::fs::read_to_string(dir.join("result.json")) else {
            return Ok(None);
        };
        let raw = task_core::decision::worker_decisions_from_json(&text);
        if raw.is_empty() {
            return Ok(None);
        }
        let now = rfc3339(OffsetDateTime::now_utc());
        let units = if wu.is_some() {
            self.store.work_units_for(task.id)?
        } else {
            Vec::new()
        };
        let unit_keys: std::collections::BTreeSet<String> = units
            .iter()
            .filter(|u| u.kind != task_core::WorkUnitKind::Integrate)
            .map(|u| u.key.clone())
            .collect();
        let stages: std::collections::BTreeSet<String> =
            units.iter().filter_map(|u| u.phase.clone()).collect();
        let root_id = task_core::tree::root_id_of(task);
        let rows = self.store.decisions_list(Some(root_id))?;
        let taken: std::collections::BTreeSet<String> = rows
            .iter()
            .filter(|r| r.task_id == task.id)
            .map(|r| r.key.clone())
            .collect();
        let open_tree = rows
            .iter()
            .filter(|r| r.status == task_core::DecisionStatus::Open)
            .count();
        let open_node = rows
            .iter()
            .filter(|r| r.status == task_core::DecisionStatus::Open && r.task_id == task.id)
            .count();
        let cap = limits
            .max_open_decisions_per_plan
            .saturating_sub(open_node)
            .min(limits.max_open_decisions_per_tree.saturating_sub(open_tree));
        let self_key = wu.map(|w| w.key.as_str());
        let batch = task_core::decision::prepare_worker_decisions(
            &raw, self_key, &unit_keys, &stages, &taken, cap,
        );
        let mut events: Vec<Event> = Vec::new();
        for reason in &batch.rejected {
            events.push(Event::worker_progress(
                run_id,
                format!("worker の決定の要求を記録できませんでした（ADR-0079 D7）: {reason}"),
            ));
        }
        if !batch.bundled.is_empty() {
            events.push(Event::worker_progress(
                run_id,
                format!(
                    "未回答の決定の上限（残り {cap} 件）を超えたので、worker の決定 {} を 1 件にまとめました（ADR-0079 D7）",
                    batch.bundled.join(", ")
                ),
            ));
        }
        if batch.accepted.is_empty() {
            return Ok(Some(WorkerDecisions {
                events,
                held_rows: Vec::new(),
                self_hold: false,
                count: 0,
            }));
        }
        let mut path =
            task_ops::tree::decision_path(self.store.as_ref(), task).map_err(ops_to_store)?;
        if let Some(w) = wu {
            // 決定を出した leaf を path の最後の段に（パンくず「root › … › <段階> › <leaf>」。`stage` は D7 の
            // path と同じく「次の段が属する段階」なので、節点の段に leaf の段階を書く）。
            if let Some(last) = path.last_mut() {
                last.stage = w.phase.clone();
            }
            path.push(task_core::DecisionPathEntry {
                task_id: task.id,
                title: w.spec.title.clone(),
                stage: None,
                unit: Some(w.key.clone()),
            });
        }
        let raised_by = task_core::DecisionRaisedBy {
            task_id: task.id,
            run_id: Some(run_id.to_string()),
            origin: task_core::DecisionOrigin::Worker,
        };
        let self_target = self_key.unwrap_or(task_core::decision::NEEDED_BEFORE_SELF);
        let self_hold = batch
            .accepted
            .iter()
            .any(|d| d.needed_before.iter().any(|n| n == self_target));
        for spec in &batch.accepted {
            events.push(Event::DecisionRequested {
                decision: Box::new(task_core::decision::request_from_spec(
                    spec,
                    path.clone(),
                    raised_by.clone(),
                )),
            });
        }
        // 他の unit（まだ走っていないもの）を止める。この leaf 自身は呼び出し側が止める。
        let mut held_rows = Vec::new();
        for u in &units {
            if Some(u.key.as_str()) == self_key
                || u.kind == task_core::WorkUnitKind::Integrate
                || !matches!(
                    u.status,
                    task_core::WorkUnitStatus::Pending | task_core::WorkUnitStatus::Ready
                )
            {
                continue;
            }
            let stage_ref = u
                .phase
                .as_deref()
                .map(|p| format!("{}{p}", task_core::decision::NEEDED_BEFORE_STAGE_PREFIX));
            let named = batch.accepted.iter().any(|d| {
                d.needed_before
                    .iter()
                    .any(|n| n == &u.key || stage_ref.as_deref() == Some(n.as_str()))
            });
            if !named {
                continue;
            }
            let mut row = u.clone();
            row.status = task_core::WorkUnitStatus::Blocked;
            row.blocked_reason = Some(task_core::WorkUnitBlockedReason::Decision);
            row.updated_at = now.clone();
            events.push(Event::WorkUnitTransitioned {
                work_unit_id: u.id.clone(),
                key: u.key.clone(),
                from: u.status,
                to: task_core::WorkUnitStatus::Blocked,
                reason: "decision".to_string(),
                run_id: Some(run_id.to_string()),
            });
            held_rows.push(row);
        }
        Ok(Some(WorkerDecisions {
            events,
            held_rows,
            self_hold,
            count: batch.accepted.len(),
        }))
    }

    /// ADR-0079 D9（Phase R2b）/ D7（Phase R3a）: この節点が出した未回答の決定で `needed_before: [self]` のもの
    /// （`plan_invalid`・atomic の run の worker の `self`）がある task は run を起こさない（`ready` のまま。名指しの
    /// 待ち = その決定）。回答（`task_ops::decision::answer`）で決定が閉じれば次の tick から走る。`kind: limit` の
    /// `self`（run 時の木の上限・節点の replan の上限）はここでは止めない: run を起こすときに `tree_run_limit_hold` /
    /// `replan_gate` が回答の余裕込みで見直す（R2a 付記 9.: 走っている run・判定・統合は止めない）。木が無効なら
    /// 常に `false`（従来と 1 バイトも変わらない）。
    fn decision_self_hold(&self, task: &Task) -> Result<bool, DispatchError> {
        if !self.config.execution.limits.tree.enabled {
            return Ok(false);
        }
        let root_id = task_core::tree::root_id_of(task);
        Ok(self.store.decisions_list(Some(root_id))?.iter().any(|d| {
            d.task_id == task.id
                && d.status == task_core::DecisionStatus::Open
                && d.kind != task_core::DecisionKind::Limit
                && d.needed_before
                    .iter()
                    .any(|n| n == task_core::decision::NEEDED_BEFORE_SELF)
        }))
    }

    /// ADR-0079 D9 / D7（Phase R3a）: この節点の最後の `plan_invalid` の決定（回答済みでも）。
    fn last_plan_invalid(
        &self,
        task: &Task,
    ) -> Result<Option<task_core::DecisionRow>, DispatchError> {
        let root_id = task_core::tree::root_id_of(task);
        Ok(self
            .store
            .decisions_list(Some(root_id))?
            .into_iter()
            .rfind(|d| d.task_id == task.id && d.kind == task_core::DecisionKind::PlanInvalid))
    }

    /// ADR-0079 D9 / D7（Phase R3a）: `plan_invalid` に「atomic で試す」と答えた節点（計画がまだ無い）の gate の
    /// 判定を atomic に書き換える（ADR-0072 D14 の atomic への倒し方と同じ書き方。規則 id `atomic/decision`）。
    /// 決定的（回答済みの行を読むだけ）。当てなければ `task` をそのまま返す。
    fn apply_plan_invalid_atomic(&self, task: Task) -> Result<Task, DispatchError> {
        if !self.config.execution.limits.tree.enabled {
            return Ok(task);
        }
        let Some(row) = self.last_plan_invalid(&task)? else {
            return Ok(task);
        };
        let Some(answer) = row.request.answer.as_ref() else {
            return Ok(task);
        };
        if task_core::answer_effect(row.kind, &answer.option) != task_core::DecisionEffect::Atomic
            || self.store.execution_plan_active(task.id)?.is_some()
        {
            return Ok(task);
        }
        let Some(mut routing) = task.routing.clone() else {
            return Ok(task);
        };
        let Some(mut decision) = routing.execution.clone() else {
            return Ok(task);
        };
        if decision.mode == task_core::ExecutionMode::Atomic {
            return Ok(task);
        }
        decision.mode = task_core::ExecutionMode::Atomic;
        decision.rule_id = "atomic/decision".to_string();
        decision.signals.push(task_core::GateSignal {
            name: "plan_invalid_answered_atomic".to_string(),
            weight: 0,
            detail: format!(
                "a human answered the plan_invalid decision {} with atomic ({})",
                row.id, answer.by
            ),
        });
        routing.execution = Some(decision.clone());
        let mut fresh = task.clone();
        fresh.routing = Some(routing);
        fresh.updated_at = OffsetDateTime::now_utc();
        tracing::info!(task_id = %task.id, decision = %row.id, "plan_invalid answered with atomic; running the node as one run (ADR-0079 D9 / R3a)");
        Ok(self.store.update_task(
            &fresh,
            Event::ExecutionGated {
                decision: Box::new(decision),
            },
        )?)
    }

    /// ADR-0079 D7（Phase R3a）: 回答で足した余裕（`raise-once` / `replan`）を当てた木の上限（run 時の上限だけ）。
    fn effective_tree_limits(
        &self,
        root_id: TaskId,
    ) -> Result<task_core::TreeLimits, DispatchError> {
        let tree = self.config.execution.limits.tree;
        let allowances = task_ops::decision::limit_allowances(self.store.as_ref(), root_id, None)
            .map_err(ops_to_store)?;
        Ok(task_core::tree::limits_with_allowances(&tree, &allowances))
    }

    /// ADR-0079 D7（Phase R3a）: 節点の replan の上限（`[execution] max_replans`）に、`limit:max_replans` への
    /// `raise-once` / `replan` の回答の数を足したもの（木が無効・木の節点でなければ設定の値のまま）。
    fn effective_max_replans(&self, task_id: TaskId) -> Result<u32, DispatchError> {
        let base = self.config.execution.max_replans;
        if !self.config.execution.limits.tree.enabled {
            return Ok(base);
        }
        let Some(task) = self.store.get(task_id)? else {
            return Ok(base);
        };
        let root_id = task_core::tree::root_id_of(&task);
        let allowances =
            task_ops::decision::limit_allowances(self.store.as_ref(), root_id, Some(task_id))
                .map_err(ops_to_store)?;
        Ok(base.saturating_add(
            allowances
                .get(&task_core::TreeLimitKind::NodeReplans)
                .copied()
                .unwrap_or(0),
        ))
    }

    /// ADR-0079 D9（Phase R2b）: 木の節点の replan が節点の上限（`[execution] max_replans`）に達した。黙って
    /// 止まらず（`Skip` のまま何も起きない、を避ける）`kind: limit`（`limit:max_replans`）の決定の要求を 1 件だけ
    /// 出す（同じ節点で未回答のものがあれば出さない）。木が無効・木の節点でなければ何もしない。
    fn raise_node_replan_limit(&self, task_id: TaskId, used: u32) -> Result<(), DispatchError> {
        if !self.config.execution.limits.tree.enabled {
            return Ok(());
        }
        let Some(task) = self.store.get(task_id)? else {
            return Ok(());
        };
        if !self.is_tree_node(&task)? {
            return Ok(());
        }
        let key = task_core::TreeLimitKind::NodeReplans.decision_key(None);
        let root_id = task_core::tree::root_id_of(&task);
        let already = self
            .store
            .decisions_list(Some(root_id))?
            .into_iter()
            .any(|d| {
                d.task_id == task_id && d.key == key && d.status == task_core::DecisionStatus::Open
            });
        if already {
            return Ok(());
        }
        let path =
            task_ops::tree::decision_path(self.store.as_ref(), &task).map_err(ops_to_store)?;
        let request = task_core::tree::limit_decision(
            task_core::TreeLimitKind::NodeReplans,
            None,
            u64::from(used),
            u64::from(self.effective_max_replans(task_id)?),
            vec![task_core::decision::NEEDED_BEFORE_SELF.to_string()],
            path,
            task_core::DecisionRaisedBy {
                task_id,
                run_id: None,
                origin: task_core::DecisionOrigin::Daemon,
            },
        );
        tracing::warn!(%task_id, used, decision = %request.id, "the node's replans are exhausted; asking a human (ADR-0079 D9)");
        self.store.append_event(
            task_id,
            &Event::DecisionRequested {
                decision: Box::new(request),
            },
        )?;
        Ok(())
    }

    /// ADR-0079 D3（Phase R2a）: 木の節点か（木の子 task、または /3 の計画を持つ root）。
    fn is_tree_node(&self, task: &Task) -> Result<bool, DispatchError> {
        if task.tree.is_some() {
            return Ok(true);
        }
        Ok(self
            .store
            .execution_plan_active(task.id)?
            .is_some_and(|p| p.spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3))
    }

    /// ADR-0079 D3（Phase R2a）: この dispatch が起こす run（worker / planner / repair。統合と「何もしない」は
    /// 数えない）が木の上限（`max_tree_runs` / `max_tree_tokens` / replan なら `max_tree_replans`）を超えるなら
    /// `true`（呼び出し側は run を起こさない。Task は今の状態のまま待つ）。超えたときは `kind: limit` の決定の
    /// 要求を**木に 1 件だけ**出す（同じ key の未回答の決定が木にあれば出さない。tick ごとに増やさない）。
    /// 木が無効・木の節点でなければ常に `false`（従来と 1 バイトも変わらない）。
    fn tree_run_limit_hold(
        &self,
        task: &Task,
        gate: &WuDispatchGate,
    ) -> Result<bool, DispatchError> {
        let tree = self.config.execution.limits.tree;
        if !tree.enabled {
            return Ok(false);
        }
        let next = match gate {
            WuDispatchGate::Atomic | WuDispatchGate::RunWorkUnit(_) => task_core::NextRun::Worker,
            WuDispatchGate::RunPlanner { replan } => {
                task_core::NextRun::Planner { replan: *replan }
            }
            WuDispatchGate::StartIntegration(_)
            | WuDispatchGate::FinalReview
            | WuDispatchGate::Skip => return Ok(false),
        };
        if !self.is_tree_node(task)? {
            return Ok(false);
        }
        let root_id = task_core::tree::root_id_of(task);
        let counters =
            task_ops::tree::tree_counters(self.store.as_ref(), root_id).map_err(ops_to_store)?;
        // ADR-0079 D7（Phase R3a）: `raise-once` / `replan` の回答で足した余裕を当てる。
        let tree = self.effective_tree_limits(root_id)?;
        let Some(breach) = task_core::tree::run_limit_breach(&tree, &counters, next) else {
            return Ok(false);
        };
        let key = breach.limit.decision_key(None);
        if task_ops::tree::open_decision(self.store.as_ref(), root_id, &key)
            .map_err(ops_to_store)?
            .is_none()
        {
            let path =
                task_ops::tree::decision_path(self.store.as_ref(), task).map_err(ops_to_store)?;
            let request = task_core::tree::limit_decision(
                breach.limit,
                None,
                breach.count,
                breach.max,
                vec![task_core::decision::NEEDED_BEFORE_SELF.to_string()],
                path,
                task_core::DecisionRaisedBy {
                    task_id: task.id,
                    run_id: None,
                    origin: task_core::DecisionOrigin::Daemon,
                },
            );
            tracing::warn!(task_id = %task.id, %root_id, limit = breach.limit.as_str(), count = breach.count, max = breach.max, decision = %request.id, "a tree limit is exceeded; this node starts no new run until a human decides (ADR-0079 D3)");
            self.store.append_event(
                task.id,
                &Event::DecisionRequested {
                    decision: Box::new(request),
                },
            )?;
        }
        Ok(true)
    }

    /// ADR-0079 D2 / D4 (4)（Phase R1b）: /3 の計画の採用前の検査。kind task の unit の `repos` が親の
    /// repos の部分集合か（外れれば `Err` = 不正な試行）、部をまたぐ子が認可済みか（未認可なら
    /// `NeedsAuthorization`、人が認めなかったなら `Err`）。子はまだ作らない（unit が ready になったとき）。
    fn tree_plan_checks(
        &self,
        task: &Task,
        spec: &task_core::ExecutionPlanSpec,
        now: OffsetDateTime,
    ) -> Result<task_ops::delegate::ChildrenPlan, String> {
        task_ops::tree::check_task_unit_repos(self.store.as_ref(), task, spec)?;
        let mut tentative = Vec::new();
        for unit in spec.units.iter().filter(|u| u.is_task()) {
            let (child, _) = task_ops::tree::build_child_task(
                self.store.as_ref(),
                task,
                "",
                unit,
                &[],
                &self.config.roles,
                &self.config.genres,
                now,
            )?;
            tentative.push(child);
        }
        let pending =
            task_ops::delegate::cross_department_questions(self.store.as_ref(), task, &tentative)?;
        if pending.is_empty() {
            Ok(task_ops::delegate::ChildrenPlan::Ready(Vec::new()))
        } else {
            Ok(task_ops::delegate::ChildrenPlan::NeedsAuthorization(
                pending,
            ))
        }
    }

    /// ADR-0079 D4 (4)・(5) / D5（Phase R1b）: 木の照合（tick ごと。決定的、LLM なし）。終わっていない
    /// kind task の unit を持つ Task ごとに:
    /// 1. 子の状態を unit に写す（子 done → unit done、子 failed / cancelled → unit failed。非終端は running の
    ///    まま）。unit が done になったら、それを待っていた unit を ready にする。
    /// 2. ready の kind task の unit（依存は `newly_ready` が満たした、段階は現在の段階）で、`needs_decisions`
    ///    がすべて回答済みで、`max_parallel_child_tasks` に空きがあるものから子 task を 1 トランザクションで作る。
    ///
    /// 親の状態は変えない（子だけを待つ親は `Ready` のまま `WuDispatchGate::Skip`、段階が揃えば次の
    /// dispatch で統合）。親が終端なら何もしない（中止の連鎖は store が子へ伝える）。
    fn reconcile_tree_units(&mut self) -> Result<(), DispatchError> {
        let now = OffsetDateTime::now_utc();
        for task_id in self.store.tasks_with_open_task_units()? {
            let Some(parent) = self.store.get(task_id)? else {
                continue;
            };
            if parent.status.is_terminal() {
                // 子だけを待っていた（run の無い）親の中止: 未完了の unit を cancelled にする（子は store の
                // 連鎖で既に中止済み）。done / failed の親の残りの kind task の unit も閉じる（照合の対象から外す）。
                if parent.status == Status::Cancelled {
                    self.cancel_open_work_units(&parent)?;
                } else {
                    self.close_open_task_units(&parent)?;
                }
                continue;
            }
            let Some(plan) = self.store.execution_plan_active(task_id)? else {
                continue;
            };
            if plan.spec.schema != task_core::EXECUTION_PLAN_SCHEMA_V3 {
                continue;
            }
            // 1. 子の状態の写し。
            let units = self.store.work_units_for(task_id)?;
            let mut changed = false;
            for u in units.iter().filter(|u| {
                u.kind == task_core::WorkUnitKind::Task
                    && u.status == task_core::WorkUnitStatus::Running
            }) {
                let Some(child_id) = u
                    .child_task_id
                    .as_deref()
                    .and_then(|s| s.parse::<TaskId>().ok())
                else {
                    continue;
                };
                let Some(child) = self.store.get(child_id)? else {
                    continue;
                };
                let Some((to, reason)) = task_ops::tree::unit_mirror(child.status) else {
                    continue;
                };
                // ADR-0079 D9（Phase R2b）: 子が基盤の分類（infra）で failed なら、親の replan にはしない:
                // 同じ unit から 1 回だけ子を作り直し、それでも失敗したら障害通知と unit `blocked(infra)`。
                if child.status == Status::Failed {
                    let failure = task_ops::tree::child_failure(self.store.as_ref(), &child)
                        .map_err(ops_to_store)?;
                    if failure.class == task_ops::derive::FailureClass::Infra {
                        self.handle_child_infra_failure(&parent, &plan, u, &child, &failure, now)?;
                        changed = true;
                        continue;
                    }
                }
                let mut row = u.clone();
                row.status = to;
                row.blocked_reason = None;
                row.clear_lease();
                row.updated_at = rfc3339(now);
                let mut events = vec![Event::WorkUnitTransitioned {
                    work_unit_id: u.id.clone(),
                    key: u.key.clone(),
                    from: u.status,
                    to,
                    reason: reason.to_string(),
                    run_id: None,
                }];
                // ADR-0079 D6（Phase R1c）: 子の done は「親の段階で取り込まれる準備ができた」。子の worktree に
                // 残った変更を決定的に commit し（WU の完了時の commit と同じ規則）、子のブランチの HEAD を
                // unit の `head_commit`、子の基点を `base_commit` に残す（`WorkUnitCommitted`、同じトランザクション）。
                if to == task_core::WorkUnitStatus::Done
                    && let Some((branch, head)) = self.commit_child_branch(&child)
                {
                    row.head_commit = Some(head.clone());
                    row.base_commit =
                        task_core::tree::child_base_commit(&child).map(str::to_string);
                    events.push(Event::WorkUnitCommitted {
                        work_unit_id: u.id.clone(),
                        key: u.key.clone(),
                        branch,
                        base: row.base_commit.clone(),
                        commit: head,
                    });
                }
                self.store
                    .work_units_apply(task_id, Vec::new(), vec![row], events)?;
                tracing::info!(%task_id, work_unit = %u.key, %child_id, child_status = ?child.status, to = ?to, "task unit mirrors its child task (ADR-0079 D4 (5))");
                changed = true;
            }
            let units = if changed {
                let units = self.store.work_units_for(task_id)?;
                // 子の done で依存が満たされた unit を ready に（段階の障壁は `newly_ready` が見る）。
                for id in task_core::newly_ready(&units) {
                    let Some(u) = units.iter().find(|u| u.id == id) else {
                        continue;
                    };
                    let mut row = u.clone();
                    row.status = task_core::WorkUnitStatus::Ready;
                    row.updated_at = rfc3339(now);
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
                self.store.work_units_for(task_id)?
            } else {
                units
            };
            // 2. 子 task の生成。ADR-0079 D8（Phase R3b）: root の計画が承認を待つ間は子を作らない
            // （承認までは unit を 1 つも起こさない）。
            if parent.status == Status::Blocked
                && task_ops::plan_gate::is_awaiting_plan_approval(
                    &parent,
                    &self.store.events_for(task_id)?,
                )
            {
                continue;
            }
            let limit = self
                .config
                .execution
                .limits
                .tree
                .max_parallel_child_tasks
                .max(1);
            let mut open_children = units
                .iter()
                .filter(|u| {
                    u.kind == task_core::WorkUnitKind::Task
                        && u.status == task_core::WorkUnitStatus::Running
                })
                .count();
            // 現在の段階（終端でない行のうち seq 最小の行の段階。`runnable_work_units` と同じ）。
            let current_stage = units
                .iter()
                .filter(|u| !u.status.is_terminal())
                .min_by_key(|u| u.seq)
                .and_then(|u| u.phase.clone());
            let mut ready: Vec<&task_core::WorkUnitRow> = units
                .iter()
                .filter(|u| {
                    u.kind == task_core::WorkUnitKind::Task
                        && u.status == task_core::WorkUnitStatus::Ready
                        && u.child_task_id.is_none()
                        && u.phase == current_stage
                })
                .collect();
            ready.sort_by_key(|u| u.seq);
            for u in ready {
                if open_children >= limit {
                    break;
                }
                let Some(unit_spec) = plan.spec.units.iter().find(|s| s.key == u.key) else {
                    continue;
                };
                // ADR-0079 D15（Phase R5b-prep）: `adopt` の unit は新しい子を作らない（既存の task を採用の入口
                // 〈人の計画の採用・`POST /tasks/{id}/tree/adopt`〉が結ぶまで待つ）。
                if unit_spec.adopt.is_some() {
                    continue;
                }
                let Some(decisions) = task_ops::tree::answered_decisions(
                    self.store.as_ref(),
                    task_id,
                    &u.needs_decisions,
                )
                .map_err(ops_to_store)?
                else {
                    // 答えの無い決定を待つ（この unit だけ。兄弟は止めない。ADR-0079 D7）。
                    continue;
                };
                // ADR-0079 D6（Phase R1c）: 子の worktree の基点（段階の基点か、同じ段階の依存先の HEAD）を
                // 子の `tree.base_commit` に書く（`Created` の Task の JSON に入るので replay で同じ値になる）。
                let built = task_ops::tree::build_child_task(
                    self.store.as_ref(),
                    &parent,
                    &plan.id,
                    unit_spec,
                    &decisions,
                    &self.config.roles,
                    &self.config.genres,
                    now,
                )
                .and_then(|(mut child, downgrades)| {
                    let base = self.child_base_commit(&parent, u, &units)?;
                    // ADR-0079 D9（Phase R2b）: 同じ unit から作る何回目の子か（replan が失敗した子の unit を
                    // 同じ key で残したときは attempt + 1）。
                    let attempt = self
                        .store
                        .events_for(task_id)
                        .map(|events| task_ops::tree::child_attempts(&events, &u.key) + 1)
                        .unwrap_or(1);
                    if let Some(tree) = child.tree.as_mut() {
                        tree.base_commit = base;
                        if let Some(pu) = tree.parent_unit.as_mut() {
                            pu.attempt = attempt;
                        }
                    }
                    Ok((child, downgrades))
                });
                match built {
                    Ok((child, downgrades)) => {
                        for reason in &downgrades {
                            tracing::info!(%task_id, child_id = %child.id, %reason, "workspace downgraded to local (ADR-0062 B2)");
                        }
                        let depth = task_core::tree::depth_of(&child);
                        let mut row = u.clone();
                        row.status = task_core::WorkUnitStatus::Running;
                        row.blocked_reason = None;
                        row.child_task_id = Some(child.id.to_string());
                        row.updated_at = rfc3339(now);
                        let events = vec![
                            Event::WorkUnitTransitioned {
                                work_unit_id: u.id.clone(),
                                key: u.key.clone(),
                                from: task_core::WorkUnitStatus::Ready,
                                to: task_core::WorkUnitStatus::Running,
                                reason: "child_created".to_string(),
                                run_id: None,
                            },
                            Event::ChildTaskCreated {
                                plan_id: plan.id.clone(),
                                unit_key: u.key.clone(),
                                child_task_id: child.id,
                                depth,
                            },
                        ];
                        if self
                            .store
                            .tree_child_create(task_id, &child, row, events, None)?
                        {
                            tracing::info!(%task_id, work_unit = %u.key, child_id = %child.id, depth, "child task created from a task unit (ADR-0079 D4 (4))");
                            open_children += 1;
                        }
                    }
                    Err(reason) => {
                        tracing::warn!(%task_id, work_unit = %u.key, %reason, "could not create the child task; the unit fails (ADR-0079 D4 (4))");
                        let mut row = u.clone();
                        row.status = task_core::WorkUnitStatus::Failed;
                        row.updated_at = rfc3339(now);
                        self.store.work_unit_transition(
                            task_id,
                            row,
                            Event::WorkUnitTransitioned {
                                work_unit_id: u.id.clone(),
                                key: u.key.clone(),
                                from: task_core::WorkUnitStatus::Ready,
                                to: task_core::WorkUnitStatus::Failed,
                                reason: "child_create_failed".to_string(),
                                run_id: None,
                            },
                        )?;
                        self.store.append_event(
                            task_id,
                            &Event::worker_progress(
                                String::new(),
                                format!("子 task「{}」を作れませんでした: {reason}", u.spec.title),
                            ),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// ADR-0079 D9（Phase R2b）: kind task の unit の子が基盤の分類（`classify_task_failure` の infra: 基盤の再試行
    /// 〈`InfraRequeue`〉を使い切った harness_error・lease の失効・準備の失敗、供給側の requeue の上限）で `failed`
    /// になった。人への質問にはしない:
    /// - この版で作り直した回数が `MAX_CHILD_INFRA_RETRIES`（1）未満なら、同じ unit から新しい子（attempt + 1、同じ
    ///   基点）を作る。unit は `running` のまま（`WorkUnitTransitioned{child_infra_retry}` と `ChildTaskCreated`）。
    /// - 使い切っていれば unit を `blocked(infra)`（`child_infra_failed`）にし、障害通知（`TaskFailed`、基盤の分類）を
    ///   1 件出す。段階は完了しない（親は `ready` のまま待つ）。同じ段階の他の unit・兄弟の子は止めない。
    fn handle_child_infra_failure(
        &self,
        parent: &Task,
        plan: &task_core::ExecutionPlanRow,
        u: &task_core::WorkUnitRow,
        child: &Task,
        failure: &task_ops::tree::ChildFailure,
        now: OffsetDateTime,
    ) -> Result<(), DispatchError> {
        let task_id = parent.id;
        let events = self.store.events_for(task_id)?;
        let retries = task_ops::tree::child_infra_retries(&events, &u.id);
        let child_id = child.id.to_string();
        let mut block_reason = failure.reason.clone();
        if retries < task_core::tree::MAX_CHILD_INFRA_RETRIES {
            let decisions = task_ops::tree::answered_decisions(
                self.store.as_ref(),
                task_id,
                &u.needs_decisions,
            )
            .map_err(ops_to_store)?
            .unwrap_or_default();
            let built = match plan.spec.units.iter().find(|s| s.key == u.key) {
                None => Err(format!("unit {} is not in the active plan", u.key)),
                Some(unit_spec) => task_ops::tree::build_child_task(
                    self.store.as_ref(),
                    parent,
                    &plan.id,
                    unit_spec,
                    &decisions,
                    &self.config.roles,
                    &self.config.genres,
                    now,
                ),
            };
            match built {
                Ok((mut next, _downgrades)) => {
                    let attempt = task_ops::tree::child_attempts(&events, &u.key) + 1;
                    if let Some(tree) = next.tree.as_mut() {
                        // 基点は同じ（親のブランチは段階の途中では動かない。ADR-0079 D6）。
                        tree.base_commit =
                            task_core::tree::child_base_commit(child).map(str::to_string);
                        if let Some(pu) = tree.parent_unit.as_mut() {
                            pu.attempt = attempt;
                        }
                    }
                    let depth = task_core::tree::depth_of(&next);
                    let mut row = u.clone();
                    row.child_task_id = Some(next.id.to_string());
                    row.updated_at = rfc3339(now);
                    let parent_events = vec![
                        Event::WorkUnitTransitioned {
                            work_unit_id: u.id.clone(),
                            key: u.key.clone(),
                            from: task_core::WorkUnitStatus::Running,
                            to: task_core::WorkUnitStatus::Running,
                            reason: task_ops::tree::CHILD_INFRA_RETRY_REASON.to_string(),
                            run_id: None,
                        },
                        Event::ChildTaskCreated {
                            plan_id: plan.id.clone(),
                            unit_key: u.key.clone(),
                            child_task_id: next.id,
                            depth,
                        },
                        Event::worker_progress(
                            String::new(),
                            format!(
                                "子 task「{}」（{}）が基盤の失敗で終わりました（{}）。質問にはせず、同じ unit {} から子を作り直します（attempt {attempt}。ADR-0079 D9）。",
                                child.title, child.id, failure.reason, u.key
                            ),
                        ),
                    ];
                    if self.store.tree_child_create(
                        task_id,
                        &next,
                        row,
                        parent_events,
                        Some(&child_id),
                    )? {
                        tracing::warn!(%task_id, work_unit = %u.key, failed_child = %child.id, child_id = %next.id, attempt, "the child task failed for an infrastructure reason; recreated it from the same unit (ADR-0079 D9)");
                    }
                    return Ok(());
                }
                Err(reason) => {
                    block_reason = format!("{block_reason}（作り直せませんでした: {reason}）");
                }
            }
        }
        let mut row = u.clone();
        row.status = task_core::WorkUnitStatus::Blocked;
        row.blocked_reason = Some(task_core::WorkUnitBlockedReason::Infra);
        row.clear_lease();
        row.updated_at = rfc3339(now);
        let attempts = retries + 1;
        let body = format!(
            "障害（infra）: 子 task「{}」が基盤の失敗で終わりました（自動の作り直し {retries} 回の後。計 {attempts} 回）: {block_reason}。unit {} は blocked(infra)、親「{}」の段階 {} は止まり、兄弟は続きます。再試行は人の操作で（ADR-0079 D9）。",
            child.title,
            u.key,
            parent.title,
            u.phase.as_deref().unwrap_or("-")
        );
        self.store.work_units_apply(
            task_id,
            Vec::new(),
            vec![row],
            vec![
                Event::WorkUnitTransitioned {
                    work_unit_id: u.id.clone(),
                    key: u.key.clone(),
                    from: u.status,
                    to: task_core::WorkUnitStatus::Blocked,
                    reason: task_ops::tree::CHILD_INFRA_FAILED_REASON.to_string(),
                    run_id: None,
                },
                Event::worker_progress(String::new(), body.clone()),
            ],
        )?;
        tracing::error!(%task_id, work_unit = %u.key, %child_id, "the child task failed again for an infrastructure reason; the unit is blocked(infra) and a failure notification is raised (ADR-0079 D9)");
        if let Err(e) = self.store.notification_upsert_pending(
            NotificationKind::TaskFailed,
            &format!("tree-infra:{}:{child_id}", u.id),
            &body,
            parent.project_id,
            now,
        ) {
            tracing::error!(%task_id, error = %e, "failed to record the infra failure notification");
        }
        Ok(())
    }

    /// ADR-0079（Phase R1b）: 終端（done / failed）になった親に残った kind task の unit を cancelled にする
    /// （`parent_terminal`。子は store の連鎖で中止済みか終端）。
    fn close_open_task_units(&self, task: &Task) -> Result<(), DispatchError> {
        let units = self.store.work_units_for(task.id)?;
        let mut rows = Vec::new();
        let mut events = Vec::new();
        for u in units
            .iter()
            .filter(|u| u.kind == task_core::WorkUnitKind::Task && !u.status.is_terminal())
        {
            let mut row = u.clone();
            row.status = task_core::WorkUnitStatus::Cancelled;
            row.blocked_reason = None;
            row.clear_lease();
            row.updated_at = rfc3339(OffsetDateTime::now_utc());
            events.push(Event::WorkUnitTransitioned {
                work_unit_id: u.id.clone(),
                key: u.key.clone(),
                from: u.status,
                to: task_core::WorkUnitStatus::Cancelled,
                reason: "parent_terminal".to_string(),
                run_id: None,
            });
            rows.push(row);
        }
        if !rows.is_empty() {
            self.store
                .work_units_apply(task.id, Vec::new(), rows, events)?;
        }
        Ok(())
    }

    /// ADR-0074「F5-fix8 実装時の明確化」: 仕事の残っていない計画の次の一手。採用（`ExecutionPlanned`）の後に
    /// 最終レビューの判定がまだ無ければ `FinalReview`、判定の後（不合格で `ready` に戻った）なら `replan_gate`。
    fn finished_plan_gate(
        &self,
        task_id: TaskId,
        plan_id: &str,
        events: &[(u64, Event)],
    ) -> Result<WuDispatchGate, DispatchError> {
        if task_core::plan_awaits_final_review(events, plan_id) {
            return Ok(WuDispatchGate::FinalReview);
        }
        self.replan_gate(task_id)
    }

    /// ADR-0074「F5-fix8 実装時の明確化」: `ready` の Task の、仕事の残っていない計画を最終レビューに出す
    /// （`Trigger::PlanComplete`。run は起こさない）。レビューの主題は完了した WU の要約（`finish_phase_integration`
    /// と同じ）の前に、今の版の計画の `rationale`（replan で何も足さなかった理由など）を置く。
    fn start_final_review_from_ready(&mut self, task: &Task) -> Result<(), DispatchError> {
        let task_id = task.id;
        let units = self.store.work_units_for(task_id)?;
        let mut summary = self.plan_summary(&units);
        if let Some(active) = self.store.execution_plan_active(task_id)?
            && !active.spec.rationale.trim().is_empty()
        {
            let head = format!(
                "計画 v{}（仕事の残っていない版）: {}",
                active.version,
                active.spec.rationale.trim()
            );
            summary = if summary.is_empty() {
                head
            } else {
                format!("{head}\n{summary}")
            };
        }
        let subject = ReviewSubject {
            summary,
            evidence: Vec::new(),
        };
        let run_id = units
            .iter()
            .filter(|u| u.kind != task_core::WorkUnitKind::Integrate)
            .filter_map(|u| u.last_run_id.clone())
            .max()
            .unwrap_or_default();
        match self
            .store
            .apply_transition_with_events(task_id, Trigger::PlanComplete, Vec::new())
        {
            Ok(outcome) => {
                tracing::info!(%task_id, next = ?outcome.next, "the active plan has no work left and has not been reviewed yet; sending it to the final review (ADR-0074 F5-fix8)");
                if outcome.next == Status::Reviewing
                    && !self.spawn_review(task_id, run_id, &subject)?
                {
                    self.pending_subjects.insert(task_id, subject);
                }
                Ok(())
            }
            Err(StoreError::InvalidTransition(e)) => {
                tracing::warn!(%task_id, error = %e, "could not send a finished plan to the final review");
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// ADR-0072 D17/D18（Phase E4）: replan の余地（`max_replans`）があれば `RunPlanner{replan:
    /// true}`、無ければ `Skip`（進められる WU が無いまま何もしない。呼び出し元が既に上限を見て
    /// `Continue{why: Replan}` を避けていれば通常ここには来ない防御的フォールバック）。
    fn replan_gate(&self, task_id: TaskId) -> Result<WuDispatchGate, DispatchError> {
        let replans_so_far = self
            .store
            .execution_plan_list(task_id)?
            .len()
            .saturating_sub(1) as u32;
        if replans_so_far < self.effective_max_replans(task_id)? {
            Ok(WuDispatchGate::RunPlanner { replan: true })
        } else {
            // ADR-0079 D9（Phase R2b）: 木の節点では上限の超過を人への決定の要求にする（黙って止まらない）。
            self.raise_node_replan_limit(task_id, replans_so_far)?;
            Ok(WuDispatchGate::Skip)
        }
    }

    /// ADR-0072 D5/D6/D9（Phase E2）: WU の run を始める（行の遷移・`runs` 索引・prompt 文脈）。
    /// `extras.work_unit`/`extras.continuation_override` を書き換える。
    #[allow(clippy::too_many_arguments)]
    fn start_work_unit_run(
        &self,
        task_id: TaskId,
        wu: &task_core::WorkUnitRow,
        run_id: &str,
        adapter_id: &str,
        model: &str,
        account: Option<&str>,
        extras: &mut RunExtras,
        lease_taken: bool,
    ) -> Result<(), DispatchError> {
        let is_continuation = wu.status == task_core::WorkUnitStatus::NeedsContinuation;
        if is_continuation {
            extras.continuation_override = self.work_unit_continuation_context(wu);
        }
        // ADR-0074 D1.5（Phase F2b）: v2 の WU は `acquire_work_unit_lease` が既に running にしている。
        if !lease_taken {
            let mut updated = wu.clone();
            updated.status = task_core::WorkUnitStatus::Running;
            updated.blocked_reason = None;
            updated.runs += 1;
            updated.last_run_id = Some(run_id.to_string());
            updated.updated_at = rfc3339(OffsetDateTime::now_utc());
            self.store.work_unit_transition(
                task_id,
                updated,
                Event::WorkUnitTransitioned {
                    work_unit_id: wu.id.clone(),
                    key: wu.key.clone(),
                    from: wu.status,
                    to: task_core::WorkUnitStatus::Running,
                    reason: "dispatch".to_string(),
                    run_id: Some(run_id.to_string()),
                },
            )?;
        }
        self.store.run_index_start(task_core::RunRow {
            run_id: run_id.to_string(),
            task_id: task_id.to_string(),
            work_unit_id: Some(wu.id.clone()),
            role: task_core::RunIndexRole::Worker,
            seq: wu.runs + 1,
            status: task_core::RunIndexStatus::Running,
            adapter: Some(adapter_id.to_string()),
            model: Some(model.to_string()),
            account: account.map(str::to_string),
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: rfc3339(OffsetDateTime::now_utc()),
            finished_at: None,
        })?;
        let units = self.store.work_units_for(task_id)?;
        extras.work_unit = Some(self.work_unit_prompt_context(task_id, &units, wu)?);
        Ok(())
    }

    /// ADR-0072 D9（Phase E2）: WU の spec から prompt に渡す文脈を組み立てる（`## Objective` の
    /// 差し替え・計画の一覧・依存する WU の完了要約）。
    fn work_unit_prompt_context(
        &self,
        task_id: TaskId,
        units: &[task_core::WorkUnitRow],
        wu: &task_core::WorkUnitRow,
    ) -> Result<task_worker::protocol::WorkUnitPromptContext, DispatchError> {
        let task_objective_excerpt = self
            .store
            .get(task_id)?
            .map(|t| t.objective.chars().take(1500).collect::<String>())
            .unwrap_or_default();
        let plan_overview: Vec<String> = units
            .iter()
            .map(|u| format!("{}: {} [{}]", u.key, u.spec.title, u.status.as_str()))
            .collect();
        let mut dependency_summaries = Vec::new();
        for dep_key in &wu.spec.depends_on {
            let Some(dep) = units.iter().find(|u| &u.key == dep_key) else {
                continue;
            };
            let completed = dep
                .last_run_id
                .as_deref()
                .and_then(|run_id| self.store.run_index_get(run_id).ok().flatten())
                .and_then(|r| r.checkpoint)
                .and_then(|cp| cp.completed.last().cloned())
                .unwrap_or_else(|| "完了".to_string());
            dependency_summaries.push(format!("{}: {}", dep.spec.title, completed));
        }
        // ADR-0074 D1.2（Phase F2b）: WU ごとの worktree で走る run には、作業ブランチと並行しうる兄弟を渡す。
        let branch = self
            .store
            .work_unit_get(&wu.id)?
            .and_then(|row| row.branch)
            .or_else(|| wu.branch.clone());
        let parallel_siblings = if branch.is_some() {
            units
                .iter()
                .filter(|u| {
                    u.id != wu.id
                        && u.phase.is_some()
                        && u.phase == wu.phase
                        && u.kind != task_core::WorkUnitKind::Integrate
                        && u.status.is_active()
                })
                .map(|u| format!("{}: {}", u.key, u.spec.title))
                .collect()
        } else {
            Vec::new()
        };
        // ADR-0079 D7（Phase R3a）: この leaf が待っていた決定の人の回答（前置きの「人の決定」節。固定の書式）。
        let human_decisions = match self.store.get(task_id)? {
            Some(task) if self.config.execution.limits.tree.enabled => {
                task_ops::decision::leaf_decision_lines(
                    self.store.as_ref(),
                    task_id,
                    task_core::tree::root_id_of(&task),
                    wu,
                )
                .map_err(ops_to_store)?
            }
            _ => Vec::new(),
        };
        Ok(task_worker::protocol::WorkUnitPromptContext {
            key: wu.key.clone(),
            title: wu.spec.title.clone(),
            objective: wu.spec.objective.clone(),
            done_when: wu.spec.done_when.clone(),
            task_objective_excerpt,
            dependency_summaries,
            plan_overview,
            branch,
            parallel_siblings,
            human_decisions,
        })
    }

    /// ADR-0072 D9（Phase E2）: この WU の continuation の文脈を `runs` 索引から組み立てる
    /// （events は WU をまたぐ run の区別を持たないため、`events` ではなく `runs` を使う。
    /// E1 の `build_continuation_context` と同じ役割の WU 版）。
    fn work_unit_continuation_context(
        &self,
        wu: &task_core::WorkUnitRow,
    ) -> Option<task_worker::ContinuationContext> {
        if wu.continuations == 0 {
            return None;
        }
        let runs = self.store.runs_for_work_unit(&wu.id).ok()?;
        let last = runs.last()?;
        let checkpoint = last.checkpoint.clone()?;
        let checkpoint_json = serde_json::to_value(&checkpoint).ok()?;
        let previous_end = match last.status {
            task_core::RunIndexStatus::Yielded => "yielded".to_string(),
            other => other.as_str().to_string(),
        };
        let prior_runs: Vec<String> = runs
            .iter()
            .map(|r| format!("Run #{} {}", r.seq, r.status.as_str()))
            .collect();
        Some(task_worker::ContinuationContext {
            run_seq: wu.runs + 1,
            previous_end,
            checkpoint: checkpoint_json,
            prior_runs,
        })
    }

    /// ADR-0072 D13（Phase E3）: Complexity Gate。Task の最初の dispatch で 1 回だけ判定し、
    /// `Event::ExecutionGated` と `Task.routing.execution` を同じトランザクションで書く。
    /// `gate = "off"` なら何もしない。対象外の大半（対話・support-task・kind != Execute・`routing`
    /// 無し）は呼び出し側で既に除いてある（D13「いつ」節）ので、ここでは残りの対象外
    /// （固定パイプラインの harness・`workspace_mode = Shared`）を `execution_gate::decide` の中で
    /// 判定する。すでに判定済みの Task（`routing.execution` が `Some`）には触らない。
    ///
    /// ADR-0079 D4 (1)（Phase R2a）: 木の子 task（`tree.parent_unit` を持つ。深さ ≥ 2）は、閾値を深さで
    /// 上げ（`5 + gate_depth_step × (depth − 1)`）、`[execution] gate` が `shadow` / `off` でも判定し採用する
    /// （`shadow = false`、`depth` を記録）。root・木でない task は従来どおり（1 バイトも変えない。U-R5）。
    fn execution_gate_if_needed(&self, task: Task) -> Result<Task, DispatchError> {
        let tree_child = task_core::tree::is_tree_child(&task);
        if self.config.execution.gate == task_core::GateMode::Off && !tree_child {
            return Ok(task);
        }
        if task.kind != task_core::TaskKind::Execute || task.routing.is_none() {
            return Ok(task);
        }
        if task_core::support_kind(&task).is_some() {
            return Ok(task);
        }
        let routing = task.routing.clone().unwrap_or_default();
        if routing.execution.is_some() {
            return Ok(task);
        }
        let (features, _overridden) =
            task_core::TaskFeatures::infer_with_hints(&task, routing.features.as_ref());
        let human_execution = routing
            .execution_hint
            .filter(|h| h.explicit)
            .map(|h| h.mode);
        let cos_hint_compound = routing
            .execution_hint
            .is_some_and(|h| !h.explicit && h.mode == task_core::ExecutionMode::Compound);
        // ADR-0079 D4 (1)（Phase R2a）: 木の子は shadow でも採用する（記録は `shadow = false`）。
        let shadow = self.config.execution.gate == task_core::GateMode::Shadow && !tree_child;
        let at = if tree_child {
            task_core::GateThreshold::at_depth(
                task_core::tree::depth_of(&task),
                self.config.execution.limits.tree.gate_depth_step,
            )
        } else {
            task_core::GateThreshold::ROOT
        };
        // S4/S6: E3 では決定的な既定値（`false`/`None`）で運用する（U10 と同じく、閾値・重みは
        // shadow の記録を見て後で調整する。ADR-0072「Phase E3 実装時の逸脱・明確化」参照）。
        let decision = task_core::decide_execution_gate_at(
            &task,
            &features,
            human_execution,
            cos_hint_compound,
            task_core::ExecutionGateInputs::default(),
            shadow,
            at,
        );
        let mut fresh = task.clone();
        let mut new_routing = routing;
        new_routing.execution = Some(decision.clone());
        fresh.routing = Some(new_routing);
        fresh.updated_at = OffsetDateTime::now_utc();
        match self.store.update_task(
            &fresh,
            Event::ExecutionGated {
                decision: Box::new(decision),
            },
        ) {
            Ok(updated) => Ok(updated),
            Err(e) => {
                tracing::warn!(task_id = %task.id, error = %e, "failed to record the execution gate decision; continuing without it");
                Ok(task)
            }
        }
    }

    /// ADR-0072 D14（Phase E3）/ D17（Phase E4b 項目1）: planner run のプロンプトに渡す gate の
    /// 根拠と D18 の上限。`replan` のときは、今の計画（版・WU ごとの状態・完了/失敗の要約）・起こした
    /// 理由・保持すべき `done` の WU の key も添える。`original_budget` は planner 用に上書きする
    /// **前**の Task の budget（WU の既定の計算に使う）。
    fn execution_planner_context(
        &self,
        task: &Task,
        original_budget: task_core::Budget,
        replan: bool,
    ) -> Result<task_worker::protocol::ExecutionPlannerContext, DispatchError> {
        let decision = task.routing.as_ref().and_then(|r| r.execution.clone());
        let limits = self.config.execution.limits;
        // Phase F5-fix3: 同じ計画の回で前の planner run の計画が拒否されていれば、その理由を渡す。
        let planner_events = self.store.events_for(task.id)?;
        let mut previous_attempt_errors = planner_rejections_since_last_plan(&planner_events);
        // ADR-0079 D7（Phase R3a）: `plan_invalid` に replan と答えた人の note を planner に渡す（末尾に 1 行）。
        if let Some(note) = plan_invalid_replan_note(&planner_events) {
            previous_attempt_errors.push(note);
        }
        let (replan_reason, current_plan_version, work_unit_summaries, preserve_done_keys) =
            if replan {
                let version = self
                    .store
                    .execution_plan_active(task.id)?
                    .map(|p| p.version);
                let units = self.store.work_units_for(task.id)?;
                let summaries = units
                    .iter()
                    .map(|u| self.work_unit_plan_summary_line(u))
                    .collect();
                let preserve = units
                    .iter()
                    .filter(|u| u.status == task_core::WorkUnitStatus::Done)
                    .map(|u| u.key.clone())
                    .collect();
                (
                    self.replan_trigger_reason(task.id)?,
                    version,
                    summaries,
                    preserve,
                )
            } else {
                (String::new(), None, Vec::new(), Vec::new())
            };
        Ok(task_worker::protocol::ExecutionPlannerContext {
            gate_rule_id: decision
                .as_ref()
                .map(|d| d.rule_id.clone())
                .unwrap_or_default(),
            gate_score: decision.as_ref().map(|d| d.score).unwrap_or(0),
            gate_signals: decision
                .as_ref()
                .map(|d| {
                    d.signals
                        .iter()
                        .map(|s| format!("{}: {} (+{})", s.name, s.detail, s.weight))
                        .collect()
                })
                .unwrap_or_default(),
            max_work_units: if self.config.execution.parallel {
                limits.max_work_units_v2
            } else {
                limits.max_work_units
            },
            work_unit_max_turns: limits.work_unit_max_turns,
            work_unit_max_wall_secs: limits.work_unit_max_wall_secs,
            default_max_turns: original_budget.max_turns.max(30),
            default_max_wall_secs: original_budget.max_wall_secs.max(1800),
            replan,
            replan_reason,
            current_plan_version,
            work_unit_summaries,
            preserve_done_keys,
            parallel: self.config.execution.parallel,
            max_phases: if self.config.execution.parallel {
                limits.max_phases
            } else {
                0
            },
            max_title_chars: limits.max_title_chars,
            max_objective_chars: limits.max_objective_chars,
            max_done_when_items: limits.max_done_when_items,
            max_done_when_chars: limits.max_done_when_chars,
            max_checks: limits.max_checks,
            max_rationale_chars: limits.max_rationale_chars,
            max_plan_json_bytes: limits.max_plan_json_bytes,
            max_children: limits.max_children,
            previous_attempt_errors,
            tree: if self.is_tree_planner(task)? {
                Some(self.tree_planner_context(task)?)
            } else {
                None
            },
        })
    }

    /// ADR-0079 D2 / D4 (2)（Phase R2b）: この task の planner に /3 を書かせるか（`[execution.tree] enabled` で、
    /// まだ計画が無いか、今の計画が /3）。/1・/2 の計画の replan は従来どおり（/2 の形と差分）。
    fn is_tree_planner(&self, task: &Task) -> Result<bool, DispatchError> {
        if !self.config.execution.limits.tree.enabled {
            return Ok(false);
        }
        Ok(match self.store.execution_plan_active(task.id)? {
            None => true,
            Some(plan) => plan.spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3,
        })
    }

    /// ADR-0079 D4 (2) / D12（Phase R2b）: /3 の planner に渡す木の中の位置（深さ・残りの深さ・祖先）、計画と木の
    /// 上限の残り（`[execution.tree]` と `task_ops::tree::tree_counters`。検証と同じ値）、人の段階の名指し。
    fn tree_planner_context(
        &self,
        task: &Task,
    ) -> Result<task_worker::protocol::TreePlannerContext, DispatchError> {
        let depth = task_core::tree::depth_of(task);
        let root_id = task_core::tree::root_id_of(task);
        // ADR-0079 D7（Phase R3a）: 回答（`raise-once`）で足した余裕も残りに含める。
        let tree = self.effective_tree_limits(root_id)?;
        let counters =
            task_ops::tree::tree_counters(self.store.as_ref(), root_id).map_err(ops_to_store)?;
        let open_decisions = self
            .store
            .decisions_list(Some(root_id))?
            .into_iter()
            .filter(|d| d.status == task_core::DecisionStatus::Open)
            .count();
        let node_replans = self
            .store
            .execution_plan_list(task.id)?
            .len()
            .saturating_sub(1) as u64;
        let chain =
            task_ops::tree::ancestors_with_self(self.store.as_ref(), task).map_err(ops_to_store)?;
        let ancestors = chain
            .iter()
            .enumerate()
            .take(chain.len().saturating_sub(1))
            .map(|(i, t)| task_worker::protocol::TreeAncestorContext {
                title: t.title.clone(),
                stage: chain
                    .get(i + 1)
                    .and_then(|next| next.tree.as_ref())
                    .and_then(|tr| tr.parent_unit.as_ref())
                    .map(|u| u.stage.clone()),
                objective_excerpt: t.objective.chars().take(300).collect(),
            })
            .collect();
        Ok(task_worker::protocol::TreePlannerContext {
            depth,
            max_depth: tree.max_depth,
            remaining_depth: task_core::tree::remaining_depth(depth, tree.max_depth),
            max_stages: tree.max_stages,
            max_units_per_stage: tree.max_units_per_stage,
            max_child_tasks_per_plan: tree.max_child_tasks_per_plan,
            max_decisions_per_plan: tree.max_open_decisions_per_plan,
            max_parallel_child_tasks: tree.max_parallel_child_tasks,
            leaves_left: u64::from(tree.max_tree_leaves.saturating_sub(counters.leaves)),
            runs_left: u64::from(tree.max_tree_runs.saturating_sub(counters.runs)),
            replans_left: u64::from(tree.max_tree_replans.saturating_sub(counters.replans)),
            node_replans_left: u64::from(self.effective_max_replans(task.id)?)
                .saturating_sub(node_replans),
            tokens_left: tree
                .max_tree_tokens
                .map(|max| max.saturating_sub(counters.tokens)),
            open_decisions_left: tree
                .max_open_decisions_per_tree
                .saturating_sub(open_decisions) as u64,
            ancestors,
            stages_hint: task
                .routing
                .as_ref()
                .map(|r| r.stages_hint.clone())
                .unwrap_or_default(),
        })
    }

    /// ADR-0072 D17（Phase E4b 項目1）: 今の計画の 1 つの WorkUnit を、replan run のプロンプトに
    /// 載せる 1 行に要約する。`done`/`failed` は最新の checkpoint（`last_run_id` から `runs` 索引を
    /// 引く）の `completed`/`known_failures` を使う。checkpoint が無ければ状態だけを出す
    /// （`runs`/`checkpoint` の欠落は既存の run でも起こりうる。D5/D8 の合成規則と同じく「無ければ
    /// 状態だけ」に倒す）。
    fn work_unit_plan_summary_line(&self, wu: &task_core::WorkUnitRow) -> String {
        // ADR-0079 D9（Phase R2b）: kind task の unit は子 task の状態と、失敗なら理由と checkpoint の要約。
        if wu.kind == task_core::WorkUnitKind::Task {
            let status = match wu.blocked_reason {
                Some(reason) => format!("{} ({})", wu.status.as_str(), reason.as_str()),
                None => wu.status.as_str().to_string(),
            };
            let child = wu
                .child_task_id
                .as_deref()
                .and_then(|id| id.parse::<TaskId>().ok())
                .and_then(|id| self.store.get(id).ok().flatten());
            let Some(child) = child else {
                return format!("{} (task) status={status}: no child task yet", wu.key);
            };
            let mut line = format!(
                "{} (task) status={status}: child task {} \"{}\" is {}",
                wu.key,
                child.id,
                child.title,
                format!("{:?}", child.status).to_lowercase()
            );
            if matches!(child.status, Status::Failed | Status::Cancelled)
                && let Ok(f) = task_ops::tree::child_failure(self.store.as_ref(), &child)
            {
                line.push_str(&format!(" ({}): {}", f.class.as_str(), f.reason));
                if let Some(cp) = f.checkpoint {
                    line.push_str(&format!("; last checkpoint: {cp}"));
                }
            }
            return line;
        }
        let checkpoint = wu
            .last_run_id
            .as_deref()
            .and_then(|rid| self.store.run_index_get(rid).ok().flatten())
            .and_then(|r| r.checkpoint);
        let status = match wu.blocked_reason {
            Some(reason) => format!("{} ({})", wu.status.as_str(), reason.as_str()),
            None => wu.status.as_str().to_string(),
        };
        let detail = match wu.status {
            task_core::WorkUnitStatus::Done => checkpoint
                .as_ref()
                .map(|cp| cp.completed.join("; "))
                .filter(|s| !s.is_empty()),
            task_core::WorkUnitStatus::Failed => checkpoint.as_ref().and_then(|cp| {
                let joined = cp
                    .known_failures
                    .iter()
                    .map(|f| f.what.clone())
                    .collect::<Vec<_>>()
                    .join("; ");
                if joined.is_empty() {
                    None
                } else {
                    Some(joined)
                }
            }),
            _ => checkpoint.as_ref().and_then(|cp| {
                if cp.next_action.is_empty() {
                    None
                } else {
                    Some(cp.next_action.clone())
                }
            }),
        };
        match detail {
            Some(detail) => format!(
                "{} ({}) status={}: {}",
                wu.key,
                wu.kind.as_str(),
                status,
                detail
            ),
            None => format!("{} ({}) status={}", wu.key, wu.kind.as_str(), status),
        }
    }

    /// ADR-0072 D17（Phase E4b 項目1）: replan の planner run に渡す「起こした理由」。events を
    /// 新しい方から辿り、決定的に文字列化する（events が正本。D5）。
    /// - WU の failed/limit からの replan（`execution_scheduler::decide` が `outcome_str = "replan:
    ///   <why>"` を書く。`crate::dispatcher` の `finish_worker_result` 参照）は、その `<why>` をそのまま使う。
    /// - 実質的な review 不合格（D17 4.、`Trigger::ReviewFail` の後の再 dispatch）は、直近の
    ///   `Event::ReviewVerdict{pass:false}` の理由を添える。
    /// - どちらでもなければ（人の依頼 D17 5. など）決定的な既定文を返す。
    fn replan_trigger_reason(&self, task_id: TaskId) -> Result<String, DispatchError> {
        let events = self.store.events_for(task_id)?;
        for (_, ev) in events.iter().rev() {
            match ev {
                // ADR-0079 D9（Phase R2b）: 子 task が失敗（work）・中止で終わり、unit が failed になった。理由は
                // 子の分類と理由（最終レビューの不合格の理由を含む）と最後の checkpoint の要約。
                Event::WorkUnitTransitioned {
                    work_unit_id,
                    key,
                    reason,
                    ..
                } if reason == "child_failed" || reason == "child_cancelled" => {
                    let child = self
                        .store
                        .work_unit_get(work_unit_id)?
                        .and_then(|u| u.child_task_id)
                        .and_then(|id| id.parse::<TaskId>().ok())
                        .and_then(|id| self.store.get(id).ok().flatten());
                    let Some(child) = child else {
                        return Ok(format!("child task of unit {key} failed"));
                    };
                    let failure = task_ops::tree::child_failure(self.store.as_ref(), &child)
                        .map_err(ops_to_store)?;
                    let attempt = child
                        .tree
                        .as_ref()
                        .and_then(|t| t.parent_unit.as_ref())
                        .map(|u| u.attempt)
                        .unwrap_or(1);
                    let mut out = format!(
                        "child task \"{}\" (unit {key}, attempt {attempt}, {}) failed ({}): {}",
                        child.title,
                        child.id,
                        failure.class.as_str(),
                        failure.reason
                    );
                    if let Some(cp) = failure.checkpoint {
                        out.push_str(&format!("; the child's last checkpoint: {cp}"));
                    }
                    return Ok(out);
                }
                // ADR-0072「Phase F6 実装時の決定」: 人が後から依頼した replan（「人の指示: <note>」）。
                Event::ExecutionHintSet {
                    replan: true,
                    note,
                    source,
                    ..
                } => {
                    return Ok(match note.as_deref() {
                        Some(n) if !n.is_empty() => format!("人の指示（{source}）: {n}"),
                        _ => format!("a human ({source}) requested a replan of this task"),
                    });
                }
                Event::WorkerFinished { outcome, .. } if outcome.starts_with("replan: ") => {
                    return Ok(outcome
                        .strip_prefix("replan: ")
                        .unwrap_or(outcome)
                        .to_string());
                }
                // ADR-0074 D2.4（Phase F3 途中確認）: 途中確認で人が選んだ replan。「人の指示: <note>」。
                Event::Transitioned { reason, .. }
                    if reason == task_core::PhaseResumeMode::Replan.name() =>
                {
                    return Ok(task_ops::phase_gate::phase_replan_instruction(&events)
                        .unwrap_or_else(|| {
                            "a human requested a replan at a phase checkpoint".to_string()
                        }));
                }
                Event::Transitioned { reason, .. } if reason == "review_fail" => {
                    let reasons: Vec<String> = events
                        .iter()
                        .rev()
                        .filter_map(|(_, e)| match e {
                            Event::ReviewVerdict {
                                pass: false,
                                reason,
                                ..
                            } => Some(reason.clone()),
                            _ => None,
                        })
                        .take(3)
                        .collect();
                    return Ok(if reasons.is_empty() {
                        "the final review failed and could not be repaired locally".to_string()
                    } else {
                        format!(
                            "the final review failed and could not be repaired locally: {}",
                            reasons.join("; ")
                        )
                    });
                }
                _ => {}
            }
        }
        Ok("a human or the daemon requested a replan".to_string())
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

    /// ADR-0074 D1.3 3.（Phase F2b）: 並列 WU の 2 本目以降。このインスタンスが既に run を持っている
    /// （＝工程の lease の持ち主の）Task だけを対象にする（引き継ぎ中の別インスタンスの Task には
    /// 手を出さない。持ち主のいない Task は再起動の照合〈D1.7〉が Ready に戻す）。
    fn dispatch_parallel_work_units(
        &mut self,
        full: &mut std::collections::HashSet<ProviderId>,
        now: Instant,
    ) -> Result<usize, DispatchError> {
        if self.workers_in_flight() >= self.config.max_concurrency {
            return Ok(0);
        }
        let mut dispatched = 0;
        let window = self.ready_window();
        for task in self.store.running_tasks_with_runnable_work_units(window)? {
            if !self.is_eligible(&task)
                || self.running_for_task(task.id) == 0
                || self.integrating.contains_key(&task.id)
                || self.just_aborted.contains(&task.id)
            {
                continue;
            }
            // ADR-0079 D13（Phase R5a）: subtree の一時停止・案件の停止の後は、走っている run は終わるまで走らせるが、
            // 並列 WU の 2 本目以降は新しく起こさない（`ready_tasks` と同じ判定）。
            if self.store.halted_by_pause(&task)? {
                continue;
            }
            let mode = self.parallel_mode(&task)?;
            loop {
                if self.workers_in_flight() >= self.config.max_concurrency {
                    return Ok(dispatched);
                }
                let units = self.store.work_units_for(task.id)?;
                let in_flight = units
                    .iter()
                    .filter(|u| {
                        u.status == task_core::WorkUnitStatus::Running
                            && u.kind != task_core::WorkUnitKind::Integrate
                    })
                    .count();
                let ids = task_core::runnable_work_units(&units, in_flight, mode.limit);
                let Some(wu) = ids
                    .first()
                    .and_then(|id| units.into_iter().find(|u| &u.id == id))
                else {
                    break;
                };
                if !self.dispatch_one(task.clone(), Some(wu), full, now)? {
                    break;
                }
                dispatched += 1;
            }
        }
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

    /// ADR-0016 D1 / D3, ADR-0027 D1: run 開始時にワーカーへ渡す役割の指示文、委譲できる run なら使える
    /// 分野の一覧、集約 run なら子の要約。
    fn run_extras(
        &self,
        task: &Task,
        worktree: Option<&task_worker::TaskWorkspaces>,
        // ADR-0054 D1（Phase 67）: 選んだアカウントとアダプタ id。CoS の対話・部門長のレビュー run の
        // 継続セッション（`session` / `session_diff`）を決めるのに要る。他の全ての値の計算には使わない。
        account: Option<&str>,
        adapter_id: &str,
    ) -> Result<RunExtras, DispatchError> {
        let role = task.role.as_deref().map(|id| RoleContext {
            id: id.to_string(),
            instructions: RoleSpec::find(&self.config.roles, id)
                .and_then(|r| r.instructions.clone())
                .unwrap_or_default(),
        });
        // ADR-0027 D1 / ADR-0028 D3: 委譲の指示文を出す run（Execute/Approval）と、子の分野を選べる
        // Plan run（`build_plan_prompt` も同じ節を出す）にだけ使える分野の一覧を渡す
        // （プロンプト側の条件と同じ。`claude_code::build_prompt` 参照）。
        // ADR-0033 D4 / Phase 28: 対話 run は委譲できないので渡さない（`delegate.json` を書かせない）。
        let is_conv = task_core::is_conversation(task);
        // ADR-0079 D4 (4)（Phase R1b）: 木の節点（計画の unit から作った子 task）の run は委譲できない
        // （子を作る入口は計画の kind task の unit だけ）。
        let available_genres = if !is_conv
            && task.tree.is_none()
            && matches!(
                task.kind,
                TaskKind::Execute | TaskKind::Approval | TaskKind::Plan
            ) {
            self.config
                .genres
                .iter()
                .map(|g| GenreContext::from_spec(g, &self.config.roles))
                .collect()
        } else {
            Vec::new()
        };
        // ADR-0033 D4 / D6（Phase 24）: この run をする「人」（担当のノード）、その記憶、この案件での
        // 直近のやり取り、そして分解・委譲できる run には組織図。全てストアとファイルの読み取りだけで、
        // LLM は使わない（DESIGN 原則 1）。
        let org = if task.assignee.is_some() || !available_genres.is_empty() {
            self.store.org_list()?
        } else {
            Vec::new()
        };
        let assigned = task
            .assignee
            .as_deref()
            .and_then(|id| org.iter().find(|n| n.id == id));
        let node = assigned.map(|n| NodeContext {
            id: n.id.clone(),
            name: n.name.clone(),
            brief: n.brief.clone(),
        });
        // ADR-0033 D5（Phase 26）: 担当がいる run にだけ、その担当宛て + 全員向けの永続の認可を注入する
        // （担当がいない run に、たまたま同じ id を持つ他ノード宛ての規則を混ぜないため。memory/conversation
        // と同じ条件）。
        let standing_rules = match assigned {
            Some(n) => self
                .store
                .standing_rule_list(Some(&n.id))?
                .into_iter()
                .map(|r| r.rule)
                .collect(),
            None => Vec::new(),
        };
        let memory = match (&self.config.memory_dir, assigned) {
            (Some(dir), Some(n)) => Some(
                MemoryDir::new(dir).load(&n.id, task.project_id.map(|p| p.to_string()).as_deref()),
            ),
            _ => None,
        };
        let conversation = match assigned {
            Some(n) => {
                let mut turns: Vec<ConversationTurn> = self
                    .store
                    .message_list(
                        &n.id,
                        task.project_id,
                        task_ops::conversation::CONVERSATION_HISTORY,
                    )?
                    .iter()
                    .map(|m| ConversationTurn {
                        role: m.role,
                        text: m.text.clone(),
                    })
                    .collect();
                // 監査 L-6: 今回の本文（`objective`）と同じ最後の `user` の行は落とす（二重に載せない）。
                if let Some(last) = turns.last()
                    && last.role == task_core::MessageRole::User
                    && last.text == task.objective
                {
                    turns.pop();
                }
                turns
            }
            None => Vec::new(),
        };
        // ADR-0033 D4 / Phase 28: 対話 run にだけ、相手が秘書（ADR-0046 D6 の CoS）かそれ以外かを渡す
        // （`preamble` が「作業を始めるな、返事だけ書け」の指示文を出し分けるためだけの印。担当が組織に
        // 無ければ CoS 以外扱いにする。判定は決定的で LLM は使わない）。
        let conversation_addressee = if is_conv {
            Some(match assigned {
                Some(n) if n.kind == OrgKind::Secretary => ConversationAddressee::Secretary,
                _ => ConversationAddressee::Other,
            })
        } else {
            None
        };
        // 分解・委譲できる run（`available_genres` を渡す run と同じ条件）にだけ組織図を渡す。
        // ADR-0046 D6: **CoS の対話 run** にも渡す（誰が何をできるかを見せる。人選はしない）。
        let is_cos_conversation = conversation_addressee == Some(ConversationAddressee::Secretary);
        // ADR-0054 D1（Phase 67）: CoS の対話は継続セッション（全体で 1 本。`project_id` は常に
        // `None`。案件を開いているときも前置きに案件の文脈を足すだけでセッションは同じ）。対応しない
        // アダプタ（claude-code/codex/acp 以外）では継続しない（`None` のまま。前置きは Phase 66 までと
        // バイト単位で同じ）。organization / node / memory / conversation はこの直後で、継続中
        // （`resume = true`）なら差分だけにする（毎回流し直さない。D1「前置きは継続中は差分だけ」）。
        let (session, session_diff) = if is_cos_conversation {
            self.resolve_node_session(
                task_core::COS_ID,
                task_core::SessionKind::Conversation,
                None,
                adapter_id,
                account,
                OffsetDateTime::now_utc(),
            )?
        } else {
            (None, Vec::new())
        };
        // ADR-0054 D1: 継続中（`resume = true`）の CoS 対話 run だけ、この後の brief・記憶・組織の一覧・
        // 直近のやり取り・進行中の案件を差分に置き換える（省く）。新規セッション（`resume = false`。
        // rollover・アカウント変更・resume 失敗の後を含む）では、これまでどおり全量を渡す。
        let continuing = is_cos_conversation && session.as_ref().is_some_and(|s| s.resume);
        let node = if continuing { None } else { node };
        let memory = if continuing { None } else { memory };
        let conversation = if continuing { Vec::new() } else { conversation };
        let organization = if (available_genres.is_empty() && !is_cos_conversation) || continuing {
            Vec::new()
        } else {
            org.iter()
                .map(|n| OrgNodeContext::with_profile(n, &task_core::resolve_profile(&org, &n.id)))
                .collect()
        };
        // ADR-0046 D1（Phase 59）: 担当ノードの実効 profile（根→葉の merge ＋ タスクの上書き）。
        // profile を 1 つも書いていない組織では `None`（前置きは Phase 58 までとバイト単位で同じ）。
        let profile = assigned.and_then(|n| {
            let effective = task_core::resolve_profile(&org, &n.id);
            if effective.is_trivial() {
                None
            } else {
                Some(effective.with_task(task))
            }
        });
        // ADR-0046 D4（Phase 59）: 既定（`production`）の進め方は渡さない（前置きを変えない）。
        let mode = if task.mode == task_core::TaskMode::Production {
            None
        } else {
            Some(task.mode)
        };
        // Phase 30（ADR-0033 D4 追記）: 対話は常に対話用分野で走る（`task.genre`）。その人が自分の仕事で
        // 何を使うかを知って答えられるように、対話 run にだけ、担当ノード**自身**の分野
        // （`node.genre`。対話用分野とは別物）を「仕事で使う道具」として渡す。決定的（`[[genres]]` の
        // manifest を引くだけ）。
        let work_genre = if is_conv {
            assigned
                .and_then(|n| n.genre.as_deref())
                .and_then(|id| GenreSpec::find(&self.config.genres, id))
                .map(|g| GenreContext::from_spec(g, &self.config.roles))
        } else {
            None
        };
        // Phase 33（ADR-0033 D4 追記。実機の事故 — 担当が自分の直近の失敗を知らずに「対象タスク ID が
        // 必要です」と聞き返した — の再発防止）: 対話 run にだけ、担当の直近の仕事を渡す。
        let recent_work = if is_conv {
            match assigned {
                Some(n) => self.recent_work_of(&n.id, task.project_id)?,
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        // Phase 41（ADR-0038 D1）: 途中目標レビューの対話 run にだけ、その途中目標とそこまでの仕事の成果を
        // 渡す（集めるのは決定的: ストアのタスク・イベントと成果物ファイルを読むだけ）。
        let milestone_review = match task_core::milestone_review_of(task) {
            Some(milestone_id) => self.milestone_review_of(task.project_id, milestone_id)?,
            None => None,
        };
        // Phase 43（ADR-0039 D3）: 案件が作業場所を決めていれば、その場所を前置きに出す（決定的:
        // `projects.workspace` を引いて 1 行にするだけ）。対話 run（秘書との会話・途中目標のレビュー）には
        // 出さない（会話は編集をしないので、人のリポジトリの中で走らせる理由が無い。ADR-0039 D2）。
        let mut workspace_note = match conversation_addressee {
            Some(_) => None,
            None => task_ops::delegate::project_workspace(self.store.as_ref(), task)
                .map_err(ops_to_store)?
                .as_ref()
                .map(task_worker::preamble::workspace_note),
        };
        // ADR-0041 D1 / ADR-0043 D2 / D8: 作業ツリーを切る run には、リポジトリ一覧（ブランチ・base）、
        // 検査コマンド、成果物の置き場を足す。
        if conversation_addressee.is_none()
            && let Some(ws) = worktree
        {
            let mut line = task_worker::preamble::repos_note(&self.repo_notes(ws));
            // ADR-0066 D1（Phase 110b）: 共有ビルドキャッシュが有効で、かつ git のリポジトリが
            // 1 つでもあれば「`target/` は共有キャッシュにある」の 1 行を足す。
            if self.config.shared_build_cache && ws.repos.iter().any(|r| r.is_git()) {
                line.push_str(task_worker::preamble::shared_build_cache_note());
            }
            workspace_note = Some(match workspace_note {
                Some(note) => format!("{note}\n{line}"),
                None => line,
            });
        }
        // ADR-0043 D2: 計画 run には**案件のリポジトリの一覧**（名前 / 種類 / 説明）を渡す。
        // プランナーは子タスクごとに `repos: ["benchfs"]` と名前で指定する。
        if task.kind == TaskKind::Plan && conversation_addressee.is_none() {
            let listing =
                task_worker::preamble::project_repos_note(&self.project_repo_notes(task)?);
            if !listing.is_empty() {
                workspace_note = Some(match workspace_note {
                    Some(note) => format!("{note}\n{listing}"),
                    None => listing,
                });
            }
        }
        let events = self.store.events_for(task.id)?;
        // 集約 run（ADR-0016 D3）と、子の失敗によるやり直し run（ADR-0021 D1）は、子の結果を見て判断する。
        let children = if (task.aggregate && has_aggregate_transition(&events))
            || has_child_failed_transition(&events)
        {
            let mut out = Vec::new();
            for child in self.store.children(task.id)? {
                if child.kind == TaskKind::Approval {
                    continue;
                }
                let child_events = self.store.events_for(child.id)?;
                let outcome = child_events.iter().rev().find_map(|(_, e)| match e {
                    Event::WorkerFinished {
                        outcome,
                        role: None,
                        ..
                    } => Some(outcome.clone()),
                    _ => None,
                });
                let artifacts = child_events
                    .iter()
                    .filter_map(|(_, e)| match e {
                        Event::ArtifactProduced { artifact, .. } => Some(artifact.clone()),
                        _ => None,
                    })
                    .collect();
                // ADR-0041 D1: 子は親の作業場所を継ぐので、子ごとに別の worktree になる。親が成果を
                // 統合するときは子のブランチを merge する（統合は LLM の仕事）。
                let child_worktree = self.task_workspaces_for(&child);
                out.push(ChildSummary {
                    id: child.id,
                    title: child.title.clone(),
                    role: child.role.clone(),
                    status: child.status,
                    outcome,
                    artifacts,
                    workspace: child_worktree
                        .as_ref()
                        .map(|ws| ws.task_dir.clone())
                        .or_else(|| self.task_dir(&child)),
                    branch: child_worktree.and_then(|ws| {
                        ws.repos
                            .first()
                            .and_then(|r| r.branch().map(str::to_string))
                    }),
                });
            }
            out
        } else {
            Vec::new()
        };
        // ADR-0044 D2（Phase 53）: コメントの糸（最新 20 件、古い順）と、直前の run を止めた人のコメント。
        // どちらも決定的に引くだけ（LLM は関与しない）。
        let all_comments = self.store.comments_for(task.id)?;
        let interrupt =
            task_ops::comment::interrupting_comment(&events, &all_comments).map(|c| c.body.clone());
        let comments: Vec<CommentContext> = all_comments
            .iter()
            .skip(
                all_comments
                    .len()
                    .saturating_sub(task_core::PREAMBLE_COMMENTS),
            )
            .map(CommentContext::from)
            .collect();
        // ADR-0047 D2（Phase 61）/ ADR-0046 D1（Phase 59 追記）: 実効マウント（担当ノードの実効
        // profile が継いだ知識 ＋ 設定の既定 ＋ 案件の `projects/<slug>`）と、その索引。
        // 決定的（`index.json` とファイルを読むだけ。LLM も判断も無い）。
        let profile_knowledge: Vec<task_core::KnowledgeMount> = assigned
            .map(|n| task_core::resolve_profile(&org, &n.id).knowledge)
            .unwrap_or_default();
        let knowledge =
            self.knowledge_context(task, assigned.map(|n| n.id.as_str()), &profile_knowledge);
        // ADR-0056 D3（Phase 79）: 担当ノードの実効 profile が継いだ skills mount を KB から解決する。
        // 決定的（ファイルを読むだけ。LLM は関与しない）。見つからない名前は run を落とさず、呼び出し元
        // （`dispatch_ready`）が `status` の進行イベントを 1 行出す。
        let profile_skills_mounts: Vec<String> = assigned
            .map(|n| task_core::resolve_profile(&org, &n.id).skills_mounts)
            .unwrap_or_default();
        let (skills, missing_skills) = self.skills_context(&profile_skills_mounts);
        // ADR-0048 D3（Phase 60b）: CoS の対話 run にだけ、進行中の案件とその途中目標を渡す
        // （`actions` の `create_task.project` を選ぶ材料。決定的にストアを
        // 読むだけ。CoS 以外の run・継続中の run（ADR-0054 D1: 差分に「新しい案件」が乗る）では常に空）。
        let active_projects = if is_cos_conversation && !continuing {
            self.active_projects_context()?
        } else {
            Vec::new()
        };
        // ADR-0059 D6（Phase 99）/ Phase 99b 追記: CoS が `create_task.workspace` を組む材料として、
        // 既知のクラスタとその実効 work_dir を渡す（未登録なら `work_dir: null` なので、CoS は `path`
        // を省略すべきと分かる。D4 の指示文と対）。`recent_work` / `knowledge` / `profile` / `role` と
        // 同じく、継続中の run でも毎回渡す（1 回渡してもクラスタの登録・接続状態は変わりうる「いまの
        // 状態」であり、`active_projects` のような「前回からの差分」で足りるものとは性質が違う）。
        let clusters = if is_cos_conversation {
            self.cluster_context()
        } else {
            Vec::new()
        };
        Ok(RunExtras {
            role,
            children,
            available_genres,
            node,
            memory,
            conversation,
            standing_rules,
            organization,
            conversation_addressee,
            work_genre,
            recent_work,
            milestone_review,
            workspace_note,
            profile,
            mode,
            comments,
            interrupt,
            knowledge,
            active_projects,
            clusters,
            // ADR-0052 D2: フォールバックの判断は `dispatch_ready` がする（ここは run ごとの文脈だけ）。
            knowledge_fallback: None,
            session,
            session_diff,
            skills,
            missing_skills,
            // ADR-0072 D9/D21（Phase E2）: WU の run かどうかは `dispatch_ready` が判断し、
            // ここ（`run_extras`）の返り値を上書きする（ここでは常に `None`）。
            work_unit: None,
            continuation_override: None,
            // ADR-0072 D13/D14（Phase E3）: planner run かどうかも `dispatch_ready` が判断し、
            // ここの返り値を上書きする（ここでは常に `None`）。
            execution_planner: None,
            // ADR-0072 D14（Phase E4b 項目3）: 同上、`dispatch_ready` が planner run のときだけ
            // 上書きする（ここでは常に `None`）。
            planner_permission_mode: None,
            artifacts_dir_override: None,
            cargo_target_work_unit: None,
            // ADR-0079 D7（Phase R3a）: 木の節点の worker の run だけ `dispatch_ready` が上書きする。
            decision_requests: false,
        })
    }

    /// ADR-0059 D6（Phase 99）: `[[clusters]]` の id（決定的な順、昇順）と、接続状態・実効
    /// work_dir（DB の上書き `cluster_settings` > 設定ファイルの `work_dir`）。CoS の対話 run にだけ渡す。
    fn cluster_context(&self) -> Vec<task_worker::ClusterContext> {
        let mut ids: Vec<&String> = self.config.clusters.keys().collect();
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let spec = &self.config.clusters[id];
                let work_dir = self
                    .store
                    .cluster_settings_get(id)
                    .ok()
                    .flatten()
                    .and_then(|s| s.work_dir)
                    .or_else(|| {
                        spec.work_dir
                            .as_ref()
                            .map(|p| p.to_string_lossy().into_owned())
                    });
                task_worker::ClusterContext {
                    id: id.clone(),
                    connected: self.cluster_connected.get(id).copied().unwrap_or(false),
                    work_dir,
                }
            })
            .collect()
    }

    /// ADR-0054 Phase 67c: この run が CoS の対話 run になるなら、その現役セッション（あれば）を返す。
    /// `run_extras` の `is_cos_conversation` と同じ判定（対話 run で、担当が `OrgKind::Secretary`）を
    /// `select_provider` より前に行う（sticky 選択がランキングより先に効く必要があるため。ADR-0054 D1
    /// の CoS セッションは `node_id = "cos"` 固定）。対象でなければ `Ok(None)`。
    fn cos_conversation_session(&self, task: &Task) -> Result<Option<NodeSession>, DispatchError> {
        if !task_core::is_conversation(task) {
            return Ok(None);
        }
        let Some(assignee) = task.assignee.as_deref() else {
            return Ok(None);
        };
        let org = self.store.org_list()?;
        let is_secretary = org
            .iter()
            .find(|n| n.id == assignee)
            .is_some_and(|n| n.kind == OrgKind::Secretary);
        if !is_secretary {
            return Ok(None);
        }
        Ok(self
            .store
            .node_session_active(task_core::COS_ID, SessionKind::Conversation, None)?)
    }

    /// ADR-0054 D1（Phase 67）: `(node_id, kind, project_id)` の継続セッションを決める・作る・引退させる
    /// （store の読み書き。判断そのものは `crate::sessions::decide`、純粋・テスト容易）。対応しない
    /// アダプタでは `(None, vec![])` を返す（このノード・kind は継続セッションを持たない）。
    fn resolve_node_session(
        &self,
        node_id: &str,
        kind: SessionKind,
        project_id: Option<ProjectId>,
        adapter_id: &str,
        account: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<(Option<task_worker::protocol::SessionHandle>, Vec<String>), DispatchError> {
        if !crate::sessions::adapter_supports_sessions(adapter_id) {
            return Ok((None, Vec::new()));
        }
        let active = self.store.node_session_active(node_id, kind, project_id)?;
        let action = crate::sessions::decide(
            active.as_ref(),
            adapter_id,
            account,
            self.config.session_rollover_tokens,
            // ADR-0054 D1: resume の失敗（アダプタがセッション不明/拒否を報告した）の明示検出は
            // 今回のスコープには含めない（PROGRESS の未解決事項）。供給側失敗として requeue されるだけ。
            false,
        );
        match action {
            crate::sessions::SessionAction::Resume => {
                let active = active.expect("SessionAction::Resume implies an active session");
                let diff = self.session_diff_since(node_id, active.last_used_at)?;
                Ok((
                    Some(task_worker::protocol::SessionHandle {
                        adapter: adapter_id.to_string(),
                        session_id: active.session_id.clone(),
                        resume: true,
                    }),
                    diff,
                ))
            }
            crate::sessions::SessionAction::Fresh(reason) => {
                if let Some(stale) = &active {
                    // ADR-0054 Phase 67b 追記: 本番の自己修復（P-67b-1）。`claude-code` の
                    // `session_id` が UUID でない行（Phase 67 が ULID を渡していた事故）を retire する
                    // ときは、次の障害調査のためにも必ずログへ残す。
                    if matches!(reason, crate::sessions::FreshReason::InvalidSessionId) {
                        tracing::warn!(
                            node_id,
                            kind = kind.as_str(),
                            adapter = adapter_id,
                            old_session_id = %stale.session_id,
                            "node_sessions.session_id はこのアダプタでは使えない形式（claude-code は UUID が必要）。\
                             retire して新しいセッションを作る（ADR-0054 Phase 67b）"
                        );
                    }
                    self.store
                        .node_session_retire(node_id, kind, project_id, now)?;
                }
                // claude-code は celeris が id を前もって決める（`--session-id`）。codex / acp は
                // アダプタが run の途中で初めて確定させるので、確定するまでは空文字（ADR-0054 D1）。
                // Phase 67b: id の発行は `crate::sessions::new_session_id`（純粋関数、テスト対象）に
                // 切り出した。Phase 67 まで使っていた `ulid::Ulid::new().to_string()`（ULID）は Claude
                // Code CLI 2.1.278 が `--session-id`/`--resume` に要求する UUID 形式ではなく、本番の
                // すべての CoS 対話・部門長レビュー run を壊した（2026-09-21 観測）。
                let session_id = crate::sessions::new_session_id(adapter_id);
                let new_session = NodeSession::new(
                    node_id,
                    kind,
                    project_id,
                    adapter_id,
                    account.map(str::to_string),
                    session_id.clone(),
                    now,
                );
                self.store.node_session_create(&new_session)?;
                let summary = if reason.needs_summary() {
                    self.session_summary(node_id)?
                } else {
                    Vec::new()
                };
                Ok((
                    Some(task_worker::protocol::SessionHandle {
                        adapter: adapter_id.to_string(),
                        session_id,
                        resume: false,
                    }),
                    summary,
                ))
            }
        }
    }

    /// ADR-0054 D1: 継続中セッションの前置きに出す差分（`since` より後に起きたこと。新しい人の発言・
    /// 終端タスクの要約・新しい案件。認可の結果は今回のスコープには含めない — PROGRESS の未解決事項）。
    fn session_diff_since(
        &self,
        node_id: &str,
        since: OffsetDateTime,
    ) -> Result<Vec<String>, DispatchError> {
        // `project_id: None` = 絞らない（`message_page` の規約。ADR-0048 D1）。CoS のセッションは
        // どの案件についての発言も同じ 1 本のセッションに乗るため、案件を問わず読む。
        let messages = self.store.message_page(Some(node_id), None, None, 50)?;
        let new_messages: Vec<(OffsetDateTime, task_core::MessageRole, String)> = messages
            .into_iter()
            .map(|m| (m.created_at, m.role, m.text))
            .collect();
        let finished_tasks = self.finished_tasks_since(since)?;
        let new_projects = self.new_projects_since(since)?;
        Ok(crate::sessions::diff_lines(
            &new_messages,
            &finished_tasks,
            &[],
            &new_projects,
            since,
        ))
    }

    /// ADR-0054 D1: 新しいセッションを継ぐときの「これまでの要約」（ADR-0033 D4 の対話履歴の末尾
    /// `CONVERSATION_HISTORY` 件）。
    fn session_summary(&self, node_id: &str) -> Result<Vec<String>, DispatchError> {
        let messages = self.store.message_page(
            Some(node_id),
            None,
            None,
            task_ops::conversation::CONVERSATION_HISTORY,
        )?;
        let history: Vec<(task_core::MessageRole, String)> =
            messages.into_iter().map(|m| (m.role, m.text)).collect();
        Ok(crate::sessions::summary_lines(
            &history,
            task_ops::conversation::CONVERSATION_HISTORY,
        ))
    }

    /// ADR-0054 D1: 前回の run 以降に終端になったタスク（支援タスクは除く）の 1 行要約。
    fn finished_tasks_since(
        &self,
        since: OffsetDateTime,
    ) -> Result<Vec<(OffsetDateTime, String)>, DispatchError> {
        let filter = ListFilter {
            statuses: vec![Status::Done, Status::Failed, Status::Blocked],
            ..ListFilter::default()
        };
        let page = self
            .store
            .list_page(&filter, ListOrder::UpdatedDesc, None, RECENT_WORK_SCAN)?;
        let mut out = Vec::new();
        for task in page.items {
            if support_kind(&task).is_some() || task.updated_at <= since {
                continue;
            }
            let events = self.store.events_for(task.id)?;
            let outcome = recent_work_outcome(&task, &events).unwrap_or_default();
            out.push((task.updated_at, format!("{} — {outcome}", task.title)));
        }
        Ok(out)
    }

    /// ADR-0054 D1: `since` より後に作られた案件（`proposed`/`active`。人が新しく開いたもの）。
    fn new_projects_since(
        &self,
        since: OffsetDateTime,
    ) -> Result<Vec<(OffsetDateTime, String)>, DispatchError> {
        let projects = self.store.project_list()?;
        Ok(projects
            .into_iter()
            .filter(|p| p.created_at > since)
            .map(|p| (p.created_at, p.title))
            .collect())
    }

    /// ADR-0048 D3（Phase 60b）: CoS の対話 run に渡す「進行中の案件」（`proposed` / `active` の案件だけ。
    /// 決定的にストアを読むだけ。LLM も判断も無い）。ADR-0079 D12 / D13（Phase R5a）: 途中目標は凍結したので
    /// CoS には渡さない（`milestones` は常に空。CoS は案件〈方向〉だけを選び、段階は `stages_hint` に書く）。
    fn active_projects_context(&self) -> Result<Vec<ActiveProjectContext>, DispatchError> {
        let mut projects = self.store.project_list()?;
        projects.retain(|p| matches!(p.status, ProjectStatus::Proposed | ProjectStatus::Active));
        projects.sort_by_key(|p| p.id);
        let mut out = Vec::new();
        for project in projects.into_iter().take(ACTIVE_PROJECTS_SCAN) {
            out.push(ActiveProjectContext {
                repos: self
                    .store
                    .repo_list(project.id)?
                    .into_iter()
                    .map(|repo| repo.name)
                    .collect(),
                id: project.id.to_string(),
                title: project.title.clone(),
                status: project.status.as_str().to_string(),
                milestones: Vec::new(),
            });
        }
        Ok(out)
    }

    /// ADR-0047 D2（Phase 61）: この run が読める知識の**索引だけ**を組む（純粋に近い: 設定・DB・
    /// `index.json`・ファイル名を読むだけ。本文は入れない）。
    ///
    /// 実効マウント = `[knowledge] default_mounts` ＋ 案件の `projects/<slug>`（自動）
    /// ＋ タスクの明示（Phase 61 では無い。Phase 59 の実効 profile がここに合流する）。
    /// ADR-0046 D1（Phase 59 追記）: `profile_knowledge` は担当ノードの**実効 profile**が継いだ
    /// マウント（`task_core::resolve_profile(..).knowledge`）。組織の和 → 設定の既定 → 案件の順で
    /// `merge_mounts` に渡す（先に出てきたものが前置きの索引で先頭に来る）。
    fn knowledge_context(
        &self,
        task: &Task,
        node_id: Option<&str>,
        profile_knowledge: &[task_core::KnowledgeMount],
    ) -> Option<task_worker::protocol::KnowledgeContext> {
        let root = &self.config.knowledge.root;
        // 案件は自動で `projects/<slug>` をマウントする（ADR-0047 D2）。
        let project = task
            .project_id
            .and_then(|id| self.store.project_get(id).ok().flatten());
        let mut project_mounts = Vec::new();
        if let Some(project) = &project {
            // Phase K-1: 案件の slug（`projects.slug`。ADR-0044 D7 追記）。
            let slug = project.kb_slug();
            project_mounts.push(task_core::KnowledgeMount::kb(format!("projects/{slug}")));
        }
        // ADR-0046 D1（Phase 59 追記）: 実効 profile ＋ 設定の既定 ＋ 案件の順で和を取る。
        let mounts = task_core::knowledge::merge_mounts(&[
            profile_knowledge,
            &self.config.knowledge.default_mounts,
            &project_mounts,
        ]);
        if mounts.is_empty() {
            return None;
        }
        let index = if task_ops::knowledge::exists(root) {
            task_ops::knowledge::ensure_index(root).items
        } else {
            Vec::new()
        };
        let mut items: Vec<task_core::KnowledgeItem> = Vec::new();
        for mount in &mounts {
            match mount.kind {
                task_core::MountKind::Kb => {
                    items.extend(
                        index
                            .iter()
                            .filter(|i| task_core::knowledge::mount_matches(mount, i))
                            .cloned(),
                    );
                }
                // `repo` は案件のリポジトリの文書の根のページ（ADR-0043 / ADR-0044 D7）。
                task_core::MountKind::Repo => {
                    if let Some(name) = mount.name.as_deref() {
                        items.extend(self.repo_doc_items(task, name, mount));
                    }
                }
                // `dir` は任意のローカルディレクトリの `*.md`（読み取り）。
                task_core::MountKind::Dir => {
                    if let Some(dir) = mount.path.as_deref() {
                        let label = mount.label();
                        items.extend(
                            task_ops::knowledge::list_pages(dir)
                                .into_iter()
                                .take(200)
                                .map(|rel| task_core::KnowledgeItem {
                                    title: rel.rsplit('/').next().unwrap_or(&rel).to_string(),
                                    path: dir.join(&rel).display().to_string(),
                                    scope: Some(label.clone()),
                                    ..task_core::KnowledgeItem::default()
                                }),
                        );
                    }
                }
                // `memory` はそのノードの手帳（ADR-0033 D3。中身は「覚えていること」の節に既に出ている）。
                task_core::MountKind::Memory => {
                    let node = mount.name.as_deref().or(node_id);
                    if let (Some(dir), Some(node)) = (&self.config.memory_dir, node) {
                        let notes = dir.join(node).join("notes.md");
                        if notes.exists() {
                            items.push(task_core::KnowledgeItem {
                                path: notes.display().to_string(),
                                title: "あなたの手帳（案件をまたぐ記憶）".to_string(),
                                scope: Some(format!("memory:{node}")),
                                ..task_core::KnowledgeItem::default()
                            });
                        }
                    }
                }
            }
        }
        Some(task_worker::protocol::KnowledgeContext {
            mounts,
            index: items,
        })
    }

    /// ADR-0056 D3（Phase 79）: `skills_mounts`（skill 名の一覧）を KB の `skills/<name>/` から解決する
    /// （`SKILL.md` があるかどうかを読むだけ。決定的、LLM は関与しない）。見つかったものは
    /// `RunContext.skills` に乗せる `SkillMount`、見つからなかった名前は 2 つ目の戻り値（呼び出し元が
    /// `status` の進行イベントを 1 行出し、run は落とさない）。
    fn skills_context(
        &self,
        mounts: &[String],
    ) -> (Vec<task_worker::protocol::SkillMount>, Vec<String>) {
        let root = &self.config.knowledge.root;
        let mut skills = Vec::new();
        let mut missing = Vec::new();
        for name in mounts {
            match task_ops::knowledge::skills_get(root, name) {
                Some(detail) => {
                    let description = task_ops::knowledge::skill_description(&detail.skill_md);
                    let path = root
                        .join(task_core::knowledge::SKILLS_DIR)
                        .join(name)
                        .display()
                        .to_string();
                    skills.push(task_worker::protocol::SkillMount {
                        name: name.clone(),
                        path,
                        description,
                    });
                }
                None => missing.push(name.clone()),
            }
        }
        (skills, missing)
    }

    /// `repo` マウント 1 件分（案件のその名前のリポジトリの文書の根のページ）。
    fn repo_doc_items(
        &self,
        task: &Task,
        name: &str,
        mount: &task_core::KnowledgeMount,
    ) -> Vec<task_core::KnowledgeItem> {
        let Some(project_id) = task.project_id else {
            return Vec::new();
        };
        let Ok(repos) = self.store.repo_list(project_id) else {
            return Vec::new();
        };
        let Some(repo) = repos.iter().find(|r| r.name == name) else {
            return Vec::new();
        };
        let task_core::WorkspaceSpec::Local { path, .. } = &repo.location else {
            return Vec::new();
        };
        let (config, _) = task_core::workspace_config::load_or_default(path);
        let docs_root = mount.docs.clone().unwrap_or(config.outputs.docs);
        let branch = task_ops::changes::default_branch(path, repo.default_branch.as_deref());
        let label = mount.label();
        task_ops::docs::list(path, &branch, &docs_root)
            .into_iter()
            .take(200)
            .map(|rel| task_core::KnowledgeItem {
                title: rel.rsplit('/').next().unwrap_or(&rel).to_string(),
                path: rel,
                scope: Some(label.clone()),
                ..task_core::KnowledgeItem::default()
            })
            .collect()
    }

    /// Phase 41（ADR-0038 D1）: レビューの対話 run に渡す「その途中目標のここまで」。
    /// その途中目標に属する仕事（裏方は除く。作られた順に最大 20 件）の title / status / 終端の要約 /
    /// 主な成果物（`answer.md` / `report.md` の先頭 4,000 字）を集める。LLM は使わない（DESIGN 原則 1）。
    fn milestone_review_of(
        &self,
        project_id: Option<ProjectId>,
        milestone_id: task_core::MilestoneId,
    ) -> Result<Option<MilestoneReviewContext>, DispatchError> {
        let Some(project_id) = project_id else {
            return Ok(None);
        };
        let Some(milestone) = self
            .store
            .milestone_list(project_id)?
            .into_iter()
            .find(|m| m.id == milestone_id)
        else {
            return Ok(None);
        };
        let filter = ListFilter {
            project_id: Some(project_id),
            ..ListFilter::default()
        };
        let page = self.store.list_page(
            &filter,
            ListOrder::CreatedDesc,
            None,
            MILESTONE_REVIEW_TASK_SCAN,
        )?;
        let mut subjects: Vec<Task> = page
            .items
            .into_iter()
            .filter(|t| t.milestone_id == Some(milestone_id) && support_kind(t).is_none())
            .collect();
        subjects.sort_by_key(|t| (t.created_at, t.id));
        let mut tasks = Vec::new();
        for task in subjects.into_iter().take(MILESTONE_REVIEW_TASK_LIMIT) {
            let events = self.store.events_for(task.id)?;
            tasks.push(MilestoneTaskResult {
                title: task.title.clone(),
                status: task.status,
                outcome: recent_work_outcome(&task, &events),
                artifacts_excerpt: self.milestone_artifacts_excerpt(&task),
            });
        }
        Ok(Some(MilestoneReviewContext {
            milestone: MilestoneBrief {
                id: milestone.id.to_string(),
                title: milestone.title.clone(),
                description: milestone.description.clone(),
                status: milestone.status.as_str().to_string(),
            },
            tasks,
        }))
    }

    /// Phase 41（ADR-0038 D1）: その仕事が残した `answer.md` / `report.md` の先頭 4,000 字
    /// （両方あれば名前を見出しに付けて繋ぎ、全体を 4,000 字で切る。読めなければ空文字）。
    fn milestone_artifacts_excerpt(&self, task: &Task) -> String {
        let Some(workspace) = self.task_dir(task) else {
            return String::new();
        };
        let dir = self.artifacts_dir(task, &workspace);
        let mut out = String::new();
        for name in MILESTONE_REVIEW_ARTIFACTS {
            if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
                out.push_str(&format!("#### {name}\n"));
                out.push_str(text.trim_end());
                out.push('\n');
            }
        }
        truncate_chars(&out, MILESTONE_REVIEW_EXCERPT_CHARS)
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

    /// Phase 33（ADR-0033 D4 追記）: `node_id` の直近の仕事（対話・まとめ・承認・レビューは除く）を
    /// 更新の新しい順に最大 10 件集める。`current_project` があれば、その案件のものを先に、
    /// 残りは他の案件から（`current_project` の中でも他の案件のものでも、それぞれ更新の新しい順は保つ）。
    /// ストアの読み取りだけで、LLM は使わない（DESIGN 原則 1）。
    fn recent_work_of(
        &self,
        node_id: &str,
        current_project: Option<ProjectId>,
    ) -> Result<Vec<RecentWork>, DispatchError> {
        let filter = ListFilter {
            assignee: Some(node_id.to_string()),
            ..ListFilter::default()
        };
        let page = self
            .store
            .list_page(&filter, ListOrder::UpdatedDesc, None, RECENT_WORK_SCAN)?;
        let candidates: Vec<Task> = page
            .items
            .into_iter()
            .filter(|t| support_kind(t).is_none())
            .collect();
        let (same_project, other_project): (Vec<Task>, Vec<Task>) = if current_project.is_some() {
            candidates
                .into_iter()
                .partition(|t| t.project_id == current_project)
        } else {
            (Vec::new(), candidates)
        };
        let mut project_titles: HashMap<ProjectId, Option<String>> = HashMap::new();
        let mut out = Vec::with_capacity(RECENT_WORK_LIMIT);
        for task in same_project
            .into_iter()
            .chain(other_project)
            .take(RECENT_WORK_LIMIT)
        {
            let events = self.store.events_for(task.id)?;
            let artifacts = artifact_names_of(&events);
            let outcome = recent_work_outcome(&task, &events);
            let finished_at =
                if matches!(task.status, Status::Done | Status::Failed | Status::Blocked) {
                    Some(rfc3339(task.updated_at))
                } else {
                    None
                };
            let project_title = match task.project_id {
                Some(pid) => project_titles
                    .entry(pid)
                    .or_insert_with(|| self.store.project_get(pid).ok().flatten().map(|p| p.title))
                    .clone(),
                None => None,
            };
            out.push(RecentWork {
                task_id: task.id,
                title: task.title.clone(),
                project_title,
                status: task.status,
                finished_at,
                outcome,
                artifacts,
            });
        }
        Ok(out)
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

    /// レビューを開始する。`Reviewer` 条件があるのにプロバイダ／並列度の枠が無いときは `Ok(false)`
    /// （タスクは `reviewing` のまま。次 tick の `recover_reviews` が再試行する。ADR-0007 D5 1.）。
    fn spawn_review(
        &mut self,
        task_id: TaskId,
        run_id: String,
        subject: &ReviewSubject,
    ) -> Result<bool, DispatchError> {
        if !self.accepting_new_work || !self.disk_ready {
            return Ok(false);
        }
        let Some(task) = self.store.get(task_id)? else {
            return Ok(true);
        };
        if task.status != Status::Reviewing {
            return Ok(true);
        }
        let Some(dir) = self.task_dir(&task) else {
            tracing::warn!(%task_id, "cannot review task with remote workspace");
            return Ok(true);
        };
        // Old and new daemons can overlap during live handoff. Both command checks
        // and model review hold the same per-task lock through verdict persistence.
        let lock_dir = dir.join("runs");
        std::fs::create_dir_all(&lock_dir)
            .map_err(|e| StoreError::Invalid(format!("review lock directory: {e}")))?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_dir.join(format!(".review-{task_id}.lock")))
            .map_err(|e| StoreError::Invalid(format!("review lock: {e}")))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(false),
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(StoreError::Invalid(format!("review lock: {e}")).into());
            }
        }
        let review_lock = Arc::new(lock);
        // The previous owner may have committed a verdict after our first read.
        let Some(mut task) = self.store.get(task_id)? else {
            return Ok(true);
        };
        if task.status != Status::Reviewing {
            return Ok(true);
        }

        let human = match self.resolve_human_approvals(&task)? {
            Some(h) => {
                self.awaiting_human.remove(&task_id);
                h
            }
            None => {
                self.awaiting_human.insert(task_id);
                tracing::debug!(%task_id, "review deferred (waiting for human approval)");
                return Ok(false);
            }
        };

        let reviewer = if needs_reviewer_run(&task) {
            match self.pick_reviewer(&task, &run_id) {
                Some(r) => Some(r),
                None => {
                    tracing::debug!(%task_id, "reviewer run deferred (no provider capacity)");
                    return Ok(false);
                }
            }
        } else {
            None
        };
        let provider = reviewer.as_ref().map(|(p, _, _)| p.clone());
        // ADR-0024 D2: `account_pool` で選んだアカウント（プールを使わない、または Reviewer run を起動しない場合は `None`）。
        let selected_account = reviewer.as_ref().and_then(|(_, a, _)| a.clone());
        let account = selected_account.as_ref().map(|(_, id)| id.clone());
        let account_adapter = selected_account.as_ref().map(|(a, _)| *a);
        // ADR-0014 D1: (provider, Reviewer run の id, adapter) — WorkerStarted の記録と in_flight に使う。
        let review_run = reviewer
            .as_ref()
            .map(|(p, _, r)| (p.clone(), r.run_id.clone(), r.adapter.id().to_string()));
        let reviewer_run = reviewer.map(|(_, _, r)| r);
        if let Some(run) = &reviewer_run {
            task_ops::delivery::begin(
                self.store.as_ref(),
                &mut task,
                &self.config.workspace_root,
                &self.config.delivery,
                &run_id,
                &run.run_id,
            )
            .map_err(DispatchError::from)?;
        }

        // ADR-0074 D3.3（Phase F4a (b)）: 案件計画（マイルストーン DAG）の run は `plan.json` ではなく
        // `project-plan.json` を書くので、`plan.json` の解析・検証（`PlanCheck`）はしない
        // （`finish_project_plan_proposal` が別に読む）。
        let plan = if task.kind == TaskKind::Plan && !task_core::is_milestones_plan_task(&task) {
            Some(PlanCheck {
                depth: self.plan_depth(&task)?,
                limits: PlanLimits::default(),
                genres: self.config.genres.clone(),
                // ADR-0043 D2: 計画が子に書ける `repos` の名前（案件に登録されているものだけ）。
                repos: task_ops::delegate::project_repos(self.store.as_ref(), &task)
                    .map_err(ops_to_store)?
                    .into_iter()
                    .map(|r| r.name)
                    .collect(),
            })
        } else {
            None
        };
        // ADR-0016 M4: 集約 run のレビューには暗黙の条件「artifacts/summary.md がある」が加わる。
        let aggregate =
            task.aggregate && has_aggregate_transition(&self.store.events_for(task_id)?);

        // ADR-0018: 判定コマンドもクラスタで実行する。
        let cluster = self.cluster_of(&task);
        let remote_settings = cluster
            .as_ref()
            .map(|(spec, path, mode)| spec.ssh_settings(path, task.id, *mode));
        let cluster_id = cluster.as_ref().map(|(spec, ..)| spec.id.clone());
        let events = self.store.events_for(task_id)?;
        let produced = artifacts_for_run(&events, &run_id);
        // ADR-0014 D1: Reviewer run も対象タスクに WorkerStarted（role: reviewer）を残す（アカウント別の集計に含めるため）。
        if let Some((provider_id, review_run_id, adapter_id)) = &review_run {
            let model = self
                .adapters
                .get(provider_id)
                .and_then(|a| {
                    a.model_for_tier(self.config.reviewer_hint.tier)
                        .ok()
                        .flatten()
                })
                .or_else(|| self.models.get(provider_id).cloned())
                .unwrap_or_default();
            self.store.append_event(
                task_id,
                &Event::WorkerStarted {
                    run_id: review_run_id.clone(),
                    adapter: adapter_id.clone(),
                    model: model.clone(),
                    provider: Some(provider_id.clone()),
                    account: account.clone(),
                    role: Some(RunRole::Reviewer),
                    task_role: None,
                },
            )?;
            // ADR-0072 D5（E2b の指摘）: reviewer run も `runs` 索引に書く
            // （(g)「全タスクの run について書く」）。
            let seq = self
                .store
                .runs_for_task(task_id)
                .map(|rs| {
                    rs.iter()
                        .filter(|r| r.role == task_core::RunIndexRole::Reviewer)
                        .count() as u32
                        + 1
                })
                .unwrap_or(1);
            if let Err(e) = self.store.run_index_start(task_core::RunRow {
                run_id: review_run_id.clone(),
                task_id: task_id.to_string(),
                work_unit_id: None,
                role: task_core::RunIndexRole::Reviewer,
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
                tracing::warn!(%task_id, run_id = %review_run_id, error = %e, "failed to record the reviewer run start in the runs index");
            }
            // ADR-0076: reviewer run も worker と同じく quota の `before` を登録する
            // （`on_review_finished` が `resolve_quota_estimate` で閉じる）。
            self.quota_begin(account.as_deref(), account_adapter, review_run_id);
        }
        let timeout = self.config.review_timeout;
        // ADR-0036 D1/D2: 判定（`plan.json` / `review.json` / `summary.md` / `ArtifactExists` の既定パス）は
        // 対象タスクの成果物ディレクトリを基準にする。
        let artifacts_dir = self.artifacts_dir(&task, &dir);
        let entry_subject = subject.clone();
        let entry_run_id = run_id.clone();
        let subject = subject.clone();
        let tx = self.tx.clone();
        let remote_review = remote_settings.clone();
        // ADR-0019 D1 6. / ADR-0041 D1: 判定コマンドは worktree の中で実行する（元のリポジトリでは実行しない）。
        let review_work_dir = self.work_dir_for(&task);
        // ADR-0074 F5-fix: reviewer の checks は Task の worktree で走るので `<repo-key>`
        // （Task 単位の run と同じ target）。
        let review_check_env = self.check_cargo_target_env(&task, None);
        // ADR-0043 D4: リポジトリが宣言した検査コマンド（`workspace.toml` の `[commands] check`）。
        // ADR-0046 D4（Phase 59）: `mode = prototype` は「明示の受け入れ条件だけ」なので使わない。
        let repo_checks = if task.mode == task_core::TaskMode::Prototype {
            Vec::new()
        } else {
            self.default_checks(&task)
        };
        // ADR-0046 D4（Phase 59）: `mode = research` は「結果に出典か計測の記録」を暗黙の条件に足す。
        let research = task.mode == task_core::TaskMode::Research;
        let running_review_lock = review_lock.clone();
        // ADR-0079 D6（Phase R1c）: 木の子の最終レビューは親のブランチと比べる（検査の
        // `merge-base --is-ancestor main` を親のブランチに置き換え、reviewer の前置きに取り込み先を書く）。
        // ADR-0079 R5b-fix2: remote workspace の reviewer には worker と同じ `.celeris/remote-exec` の
        // 指示を足し、木の子なら「ブランチ統合なし」の注記にする（親のブランチの行は出さない）。
        let task = crate::review::review_view(
            task,
            &self.config.worktree_branch_prefix,
            remote_review.as_ref(),
        );
        let handle = tokio::spawn(async move {
            let _review_lock = running_review_lock;
            let ws: Box<dyn Workspace> = match remote_review {
                Some(settings) => {
                    let ssh = SshWorkspace::new(&dir, settings);
                    // R5b-fix2: reviewer が使うラッパを置く（worker の run が置いたものを最新の設定で
                    // 書き直すだけ。置けなくても判定そのものは続ける）。
                    if let Err(e) = ssh.write_remote_exec_helper().await {
                        tracing::warn!(%task_id, error = %e, "could not write the remote-exec helper for the reviewer (R5b-fix2)");
                    }
                    Box::new(ssh)
                }
                None => Box::new(
                    match review_work_dir {
                        Some(work) if work.is_dir() => {
                            LocalWorkspace::new(&dir).with_work_dir(work)
                        }
                        _ => LocalWorkspace::new(&dir),
                    }
                    .with_cargo_env(review_check_env),
                ),
            };
            let extras = ReviewExtras {
                subject,
                plan,
                reviewer: reviewer_run,
                human,
                aggregate,
                // ADR-0043 D4: リポジトリの `[commands] check`（タスクに検査コマンドが無いときだけ効く）。
                repo_checks,
                // ADR-0046 D4: `mode = research` の暗黙の条件。
                research,
            };
            let outcome = review_task(
                &task,
                ws.as_ref(),
                &dir,
                &artifacts_dir,
                &produced,
                timeout,
                extras,
            )
            .await;
            // The entry owns the lock through verdict persistence. Release this
            // task's copy before sending completion so it cannot outlive that entry.
            drop(_review_lock);
            let _ = tx.send(Completion::Review {
                task_id,
                run_id,
                outcome,
            });
        });
        self.reviewing.insert(
            task_id,
            ReviewEntry {
                _review_lock: review_lock,
                handle,
                provider,
                subject: entry_subject,
                run_id: entry_run_id,
                review_run_id: review_run.map(|(_, id, _)| id),
                since: OffsetDateTime::now_utc(),
                cluster: cluster_id,
                account,
                account_adapter,
            },
        );
        Ok(true)
    }

    /// `task.acceptance` の各 `Check::Human` について `Approval` 子タスクを解決する（ADR-0008 D2）。
    /// 子が無ければ作る。いずれかがまだ未決（`Ready`/`Draft`）なら `Ok(None)`（レビュー全体を延期）。
    /// 全て終端に達していれば `idx -> (pass, reason)` を返す（`Human` criterion が無ければ空の map）。
    fn resolve_human_approvals(&self, task: &Task) -> Result<Option<HumanVerdicts>, DispatchError> {
        let human_indices: Vec<usize> = task
            .acceptance
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c.check, Check::Human))
            .map(|(idx, _)| idx)
            .collect();
        if human_indices.is_empty() {
            return Ok(Some(HashMap::new()));
        }

        let existing_children = self.store.list(None)?;
        let mut resolved = HashMap::with_capacity(human_indices.len());
        for idx in human_indices {
            let title = human_approval_title(task, idx);
            let child = match existing_children.iter().find(|c| {
                c.parent_id == Some(task.id) && c.kind == TaskKind::Approval && c.title == title
            }) {
                Some(c) => c.clone(),
                None => self.create_human_approval_child(task, idx, &title)?,
            };
            match child.status {
                Status::Done => {
                    resolved.insert(
                        idx,
                        (true, format!("approved (approval task {})", child.id)),
                    );
                }
                Status::Failed => {
                    let note = approval_decision_note(&self.store.events_for(child.id)?);
                    resolved.insert(
                        idx,
                        (
                            false,
                            format!("rejected (approval task {}){note}", child.id),
                        ),
                    );
                }
                Status::Cancelled => {
                    resolved.insert(
                        idx,
                        (false, format!("approval task {} was cancelled", child.id)),
                    );
                }
                _ => return Ok(None),
            }
        }
        Ok(Some(resolved))
    }

    /// `Human` criterion のための `Approval` 子タスクを新規作成する（ADR-0008 D2）。
    fn create_human_approval_child(
        &self,
        task: &Task,
        idx: usize,
        title: &str,
    ) -> Result<Task, DispatchError> {
        let now = OffsetDateTime::now_utc();
        let approval = Task {
            tree: None,
            paused_at: None,
            routing: None,
            repos: Vec::new(),
            id: TaskId::new(),
            parent_id: Some(task.id),
            kind: TaskKind::Approval,
            title: title.to_string(),
            objective: task.acceptance[idx].text.clone(),
            acceptance: vec![],
            inputs: vec![],
            depends_on: vec![],
            status: Status::Ready,
            priority: task.priority,
            worker_hint: task.worker_hint.clone(),
            workspace: task.workspace.clone(),
            budget: task.budget,
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: None,
            aggregate: false,
            // ADR-0033 D2（監査 D-3）: 派生タスクは親の案件・途中目標・担当を継ぐ（仕事の木から子が消えないように）。
            project_id: task.project_id,
            milestone_id: task.milestone_id,
            assignee: task.assignee.clone(),
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
            // ADR-0046 D4（Phase 59）: 派生タスクは親の進め方を継ぐ。
            skills: Vec::new(),
            mode: task.mode,
        };
        // ADR-0010 D2: 挿入・Created・ApprovalRequested を 1 トランザクションで。
        self.store
            .create_task(&approval, vec![Event::ApprovalRequested])?;
        tracing::info!(task_id = %task.id, approval_id = %approval.id, criterion_idx = idx, "created approval child for human check");
        Ok(approval)
    }

    /// `Reviewer` run のアダプタ／プロバイダを選ぶ（ADR-0007 D5 1.）。並列度の枠は実行中 run と共有する。
    /// ADR-0024 D2: 選んだプロバイダが `account_pool` ならアカウントも選ぶ（戻り値の第 2 要素）。
    #[allow(clippy::type_complexity)]
    fn pick_reviewer(
        &mut self,
        task: &Task,
        subject_run_id: &str,
    ) -> Option<(ProviderId, Option<(AccountAdapter, String)>, ReviewerRun)> {
        if self.workers_in_flight() >= self.config.max_concurrency {
            return None;
        }
        // ADR-0012 D2: ワーカー run と同じ手順（上限のプロバイダを飛ばして次へ、候補なしは warn）で選ぶ。
        let org = self.store.org_list().ok()?;
        let department = task
            .assignee
            .as_deref()
            .and_then(|id| task_core::department_of(&org, id));
        let node = department
            .as_deref()
            .and_then(|id| org.iter().find(|n| n.id == id))
            .map(|n| NodeContext {
                id: n.id.clone(),
                name: n.name.clone(),
                brief: n.brief.clone(),
            });
        let profile = department
            .as_deref()
            .map(|id| task_core::profile::resolve(&org, id));
        // ADR-0069 Phase 118 D4: reviewer の lane は、上ほど強い優先順位で決める。
        //   1. 部署の `profile.review_tier`（ADR-0069 D2。最も具体的な指定）。
        //   2. `[reviewer] tier` の明示（`reviewer_tier_override`）。
        //   3. 既定: worker run の lane に一致させ、組織の天井（`lane_ceiling`）で丸める。
        // 1./2. は丸めない（人・運用の明示は組織の既定より強い。D2 の「人の明示 tier は天井で
        // 丸めない」と同じ考え方を運用の明示にも適用する）。
        let ceiling = profile
            .as_ref()
            .map(|p| p.lane_ceiling())
            .unwrap_or_default();
        let worker_lane = task.worker_hint.tier;
        let (default_tier, default_clamp) = ceiling.clamp(worker_lane);
        let (reviewer_lane, review_rule_id, review_reasons) = match (
            profile.as_ref().and_then(|p| p.review_tier),
            self.config.reviewer_tier_override,
        ) {
            (Some(dept_tier), _) => (
                dept_tier,
                "reviewer/department-review-tier",
                vec![format!(
                    "org profile review.tier = {dept_tier:?} (most specific; ADR-0069 D2)"
                )],
            ),
            (None, Some(explicit)) => (
                explicit,
                "reviewer/explicit-config",
                vec![format!(
                    "[reviewer] tier = {explicit:?} (explicit config; Phase 118 D4)"
                )],
            ),
            (None, None) => {
                let mut reasons = vec![format!("default: matches the worker lane {worker_lane:?}")];
                if let Some(clamp) = &default_clamp {
                    reasons.push(clamp.clone());
                }
                (default_tier, "reviewer/matches-worker-lane", reasons)
            }
        };
        let mut hint = self.config.reviewer_hint.clone();
        hint.tier = reviewer_lane;

        // ADR-0054 Phase 67c: 部署のレビュー・切り分け run（`kind = lead`）も、継続セッションの
        // (adapter, account) に留まれるかを先に試す（`resolve_node_session` と同じキー）。
        let sticky_session = match &department {
            Some(dept_id) => self
                .store
                .node_session_active(dept_id, task_core::SessionKind::Lead, None)
                .ok()
                .flatten(),
            None => None,
        };
        let mut full = std::collections::HashSet::new();
        let (adapter_id, provider_id, selected_account) = self.select_provider(
            &hint,
            Instant::now(),
            task.id,
            &mut full,
            sticky_session.as_ref(),
        )?;
        let base_adapter = match self.adapters.get(&provider_id) {
            Some(a) => a.clone(),
            None => {
                tracing::warn!(task_id = %task.id, provider = %provider_id, adapter = %adapter_id, "no adapter instance for reviewer provider");
                return None;
            }
        };
        if let Err(reason) = base_adapter.model_for_tier(hint.tier) {
            if self.warned_unroutable.insert(task.id) {
                let _ = self.store.append_event(
                    task.id,
                    &Event::worker_progress(
                        subject_run_id,
                        format!("review model routing blocked: {reason}"),
                    ),
                );
            }
            return None;
        }
        let adapter = match &selected_account {
            Some((account_adapter, account_id)) => {
                match self.adapter_for_account(&base_adapter, *account_adapter, account_id) {
                    Some(a) => a,
                    None => {
                        tracing::warn!(task_id = %task.id, provider = %provider_id, account_id, "adapter does not support account pools for reviewer run; deferring");
                        return None;
                    }
                }
            }
            None => base_adapter,
        };
        let account = selected_account.as_ref().map(|(_, id)| id.clone());
        let account_adapter = selected_account.as_ref().map(|(a, _)| *a);
        let review_run_id = ulid::Ulid::new().to_string();
        // ADR-0054 D1 / Phase 67b 追記: 部署があれば、この run の `session_established`/
        // `session_resume_failed` を Lead セッション（`kind = lead`）に配線する（Phase 67 で抜けていた
        // 配線。下の `resolve_node_session` と同じ `(department, Lead, None)` のキー）。
        let session_key = department
            .clone()
            .map(|dept_id| (dept_id, task_core::SessionKind::Lead, None));
        let sink = ReviewerSink {
            store: self.store.clone(),
            task_id: task.id,
            subject_run_id: subject_run_id.to_string(),
            review_run_id: review_run_id.clone(),
            account: account.clone(),
            account_book: account_adapter.and_then(|a| self.account_book(a)),
            session_key,
        };
        // ADR-0054 D1（Phase 67）: 部署の根ノード（engineering/research/operations）は
        // レビュー・切り分け run を継続セッション（`kind = lead`）で走らせる（ADR-0051）。部署が無い
        // 仕事（従来の独立レビュアー）では継続しない。store のエラーはレビューそのものを止めない
        // （継続無し＝Phase 66 までと同じ挙動にフォールバックする）。
        let (session, session_diff) = match &department {
            Some(dept_id) => match self.resolve_node_session(
                dept_id,
                task_core::SessionKind::Lead,
                None,
                &adapter_id,
                account.as_deref(),
                OffsetDateTime::now_utc(),
            ) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(task_id = %task.id, department = %dept_id, error = %e, "failed to resolve the department lead session; reviewing without one");
                    (None, Vec::new())
                }
            },
            None => (None, Vec::new()),
        };
        tracing::info!(task_id = %task.id, %review_run_id, adapter = %adapter_id, provider = %provider_id, account = account.as_deref(), "starting reviewer run");
        // ADR-0069 Phase 118 D4: reviewer run にも `Event::RoutingDecided` を残す（Phase 114 は worker
        // run にしか出していなかった）。reviewer は `TaskFeatures` 規則表を通らないので `rule_id` は
        // 上で決めた 3 種のいずれか、`source = System`（policy ではなく config/組織の指定で決まる）。
        // ストア書き込み失敗はレビューそのものを止めない（`warned_unroutable` と同じベストエフォート）。
        let review_model_id = adapter
            .model_for_tier(hint.tier)
            .ok()
            .flatten()
            .unwrap_or_default();
        let review_reasoning_effort = adapter
            .reasoning_effort_for_tier(hint.tier)
            .filter(|_| adapter.supports_reasoning_effort());
        let review_record = task_core::RoutingRecord {
            org_node: department.clone(),
            harness: Some("reviewer".to_string()),
            decision: task_core::LaneDecision {
                lane: reviewer_lane,
                proposed: worker_lane,
                source: task_core::TierSource::System,
                rule_id: review_rule_id.to_string(),
                policy_version: task_core::LANE_POLICY_VERSION.to_string(),
                features: task_core::TaskFeatures::infer(task),
                reasons: review_reasons,
                clamped_by: if review_rule_id == "reviewer/matches-worker-lane" {
                    default_clamp.clone()
                } else {
                    None
                },
                hint: None,
                escalation: None,
                shadow: None,
            },
            resolution: task_core::model_routing::LaneResolution {
                lane: Some(reviewer_lane),
                adapter: adapter_id.clone(),
                provider: Some(provider_id.clone()),
                account: account.clone(),
                model_id: review_model_id,
                reasoning_effort: review_reasoning_effort,
            },
            quota_reason: None,
            work_unit_id: None,
        };
        let _ = self.store.append_event(
            task.id,
            &Event::RoutingDecided {
                run_id: review_run_id.clone(),
                record: Box::new(review_record),
            },
        );
        Some((
            provider_id,
            selected_account,
            ReviewerRun {
                node,
                profile,
                adapter,
                run_id: review_run_id,
                limits: RunLimits {
                    wall_clock: Duration::from_secs(task.budget.max_wall_secs),
                    idle_timeout: self.config.idle_timeout,
                    kill_grace: self.config.kill_grace,
                },
                sink: Box::new(sink),
                hint,
                // Phase 38（ADR-0028 追記）: レビュー対象の分野の manifest（決定的。設定を引くだけ）。
                subject_genre: task
                    .genre
                    .as_deref()
                    .and_then(|id| GenreSpec::find(&self.config.genres, id))
                    .map(|g| GenreContext::from_spec(g, &self.config.roles)),
                session,
                session_diff,
            },
        ))
    }

    /// ADR-0013 D9: cooldown に入った供給側失敗の `ProviderThrottled`。期限はポリシーの `cooldowns()` から取り、
    /// ポリシーが公開しない場合は `Throttled.retry_after` から計算する（どちらも無ければ記録しない）。
    fn provider_throttled_event(
        &self,
        provider: &str,
        outcome: &ProviderOutcome,
        reason: &str,
    ) -> Option<Event> {
        let now = Instant::now();
        let until = self
            .policy
            .cooldowns(now)
            .into_iter()
            .find(|c| c.provider == provider)
            .map(|c| c.until)
            .or(match outcome {
                ProviderOutcome::Throttled { retry_after } => Some(now + *retry_after),
                _ => None,
            })?;
        Some(Event::ProviderThrottled {
            provider: provider.to_string(),
            until: OffsetDateTime::now_utc() + until.saturating_duration_since(now),
            reason: Some(reason.to_string()),
        })
    }

    /// `ready_tasks` の取得件数。経路なしと分かっているタスク（`warned_unroutable`）の分だけ広げ、それらが窓を埋めて
    /// 後ろの実行可能なタスクが dispatch されない・`is_idle` が誤って真になることを防ぐ（ADR-0012 監査）。
    fn ready_window(&self) -> usize {
        self.config.max_concurrency * 4 + 16 + self.warned_unroutable.len()
    }

    /// ADR-0012 D2（P-20 / P-33）: 並列度の上限に達したプロバイダを除外しながら選ぶ（設定表の次の行へフォールバック）。
    /// 条件に合うプロバイダが設定に無ければ、タスクごとに 1 回 warn し `unroutable` に入れる。
    /// ADR-0024 D2: 選んだプロバイダが `account_pool = true` なら、続けて D3 でアカウントを選ぶ。選べるアカウントが
    /// 無ければそのプロバイダを満杯として扱い（除外集合に入れて）次の候補へ進む。戻り値の第 3 要素が選んだアカウント
    /// （プールを使わないプロバイダなら `None`）。
    ///
    /// ADR-0054 Phase 67c: `sticky_session` にこのノード・kind の現役セッションが渡されたら、まず
    /// `crate::sessions::decide_sticky` でそのセッションの `(adapter, account_id)` に留まれるかを試す
    /// （`sticky_provider`）。留まれれば ADR-0049 のランキングを走らせない（毎 run アカウントを
    /// 付け替えてセッションを退役させ続ける事故の修正）。留まれなければ、これまでどおり下のランキングへ。
    #[allow(clippy::type_complexity)]
    fn select_provider(
        &mut self,
        hint: &task_core::WorkerHint,
        now: Instant,
        task_id: TaskId,
        full: &mut std::collections::HashSet<ProviderId>,
        sticky_session: Option<&NodeSession>,
    ) -> Option<(AdapterId, ProviderId, Option<(AccountAdapter, String)>)> {
        if let Some(sticky) = self.sticky_provider(sticky_session, hint, now, &*full) {
            return Some(sticky);
        }
        // 候補を列挙するための除外はこの選択だけ。満杯の集合へ候補自体を混ぜない。
        let mut visited = full.clone();
        let mut best_pool = None;
        let mut best_score = f64::NEG_INFINITY;
        let mut fallback = None;
        for _ in 0..64 {
            match self.policy.select(hint, now, &visited) {
                Selection::Picked { adapter, provider } => {
                    if !visited.insert(provider.clone()) {
                        break;
                    }
                    let limit = self.policy.concurrency_limit(provider.clone());
                    if self.provider_in_use(&provider) >= limit {
                        full.insert(provider);
                        continue;
                    }
                    if self.account_pool_providers.contains(&provider) {
                        let Some(account_adapter) = AccountAdapter::parse(&adapter) else {
                            full.insert(provider);
                            continue;
                        };
                        let requested_account = self
                            .adapters
                            .get(&provider)
                            .and_then(|a| a.account_id())
                            .map(str::to_owned);
                        let Some(account_id) =
                            self.pick_account(account_adapter, requested_account.as_deref())
                        else {
                            full.insert(provider);
                            continue;
                        };
                        let score = self.account_score(account_adapter, &account_id);
                        if score > best_score {
                            best_score = score;
                            best_pool =
                                Some((adapter, provider, Some((account_adapter, account_id))));
                        }
                    } else if fallback.is_none() {
                        fallback = Some((adapter, provider, None));
                    }
                }
                Selection::Busy => break,
                Selection::NoMatchingProvider => {
                    if best_pool.is_none() && fallback.is_none() {
                        self.unroutable.insert(task_id);
                        if self.warned_unroutable.insert(task_id) {
                            tracing::warn!(%task_id, ?hint, "no provider in the config matches this worker_hint");
                        }
                    }
                    break;
                }
            }
        }
        let selected = best_pool.or(fallback);
        if selected.is_some() {
            self.warned_unroutable.remove(&task_id);
        }
        selected
    }

    /// ADR-0054 Phase 67c: `sticky_session` があれば、`crate::sessions::decide_sticky` にかけて
    /// 「留まれるか」を判断する。使う材料（アカウントの状態・設定表に今もその tier を提供する行が
    /// あるか）はここで集める（I/O）。留まれるなら `select_provider` と同じ形の戻り値、留まれなければ
    /// `None`（呼び出し側が通常のランキングへフォールバックする）。
    #[allow(clippy::type_complexity)]
    fn sticky_provider(
        &mut self,
        sticky_session: Option<&NodeSession>,
        hint: &task_core::WorkerHint,
        now: Instant,
        full: &std::collections::HashSet<ProviderId>,
    ) -> Option<(AdapterId, ProviderId, Option<(AccountAdapter, String)>)> {
        let active = sticky_session?;
        let account_usable = match &active.account_id {
            None => true,
            Some(account_id) => AccountAdapter::parse(&active.adapter)
                .is_some_and(|adapter| self.account_usable(adapter, account_id)),
        };
        let provider = self.matching_provider_for_adapter(&active.adapter, hint.tier, now, full);
        // 設定行が見つかっても、プールの有無がセッション作成時と食い違っていたら（config を書き換えた
        // 等）留まらない。`decide` の `AccountChanged`（プールを使う ⇔ 使わないの切り替えも該当）と
        // 矛盾しないように。
        let provider_offers_tier = provider.as_ref().is_some_and(|p| {
            self.account_pool_providers.contains(p) == active.account_id.is_some()
        });
        let decision = crate::sessions::decide_sticky(
            Some(active),
            self.config.session_rollover_tokens,
            account_usable,
            provider_offers_tier,
        );
        if decision != crate::sessions::StickyDecision::Stick {
            return None;
        }
        let provider_id = provider?;
        let selected_account = active
            .account_id
            .clone()
            .and_then(|id| AccountAdapter::parse(&active.adapter).map(|a| (a, id)));
        Some((active.adapter.clone(), provider_id, selected_account))
    }

    /// ADR-0054 Phase 67c: `adapter_id` の設定行のうち、`requested_tier` と同じかそれ以上の tier を
    /// 提供し（`crate::sessions::tier_rank`。要求そのものから順に試す）、cooldown 中でも並列度上限でも
    /// ないものを 1 つ返す。`hint.adapter` をこの 1 アダプタに固定して `ProviderPolicy::select` に
    /// 任せるので、cooldown・除外集合の扱いは通常のランキングと同じ規則になる。
    fn matching_provider_for_adapter(
        &self,
        adapter_id: &str,
        requested_tier: Tier,
        now: Instant,
        excluded: &std::collections::HashSet<ProviderId>,
    ) -> Option<ProviderId> {
        let mut tiers: Vec<Tier> = [Tier::Cheap, Tier::Standard, Tier::Frontier]
            .into_iter()
            .filter(|t| {
                crate::sessions::tier_rank(*t) >= crate::sessions::tier_rank(requested_tier)
            })
            .collect();
        tiers.sort_by_key(|t| crate::sessions::tier_rank(*t));
        for tier in tiers {
            let pinned = task_core::WorkerHint {
                tier,
                adapter: Some(adapter_id.to_string()),
            };
            if let Selection::Picked { provider, .. } = self.policy.select(&pinned, now, excluded) {
                let limit = self.policy.concurrency_limit(provider.clone());
                if self.provider_in_use(&provider) < limit {
                    return Some(provider);
                }
            }
        }
        None
    }

    /// ADR-0054 Phase 67c: 指定した 1 アカウントが今すぐ使えるか（ログイン済み・cooldown 外・上限未満・
    /// 枯渇していない。`crate::accounts::evaluate` の除外判定をそのまま使う）。`pick_account` と同じ
    /// 読み取りだが、ベストスコアを探すのではなく特定の 1 件が使えるかだけを見る。
    fn account_usable(&mut self, adapter: AccountAdapter, account_id: &str) -> bool {
        let Some(cfg) = self.config.accounts.clone() else {
            return false;
        };
        let Some(root) = cfg.root_for(adapter) else {
            return false;
        };
        let dirs = self
            .accounts_scan_cache
            .entry(adapter)
            .or_insert_with(|| scan_accounts(root, adapter))
            .clone();
        let Some(dir) = dirs.iter().find(|d| d.id == account_id) else {
            return false;
        };
        let Some(book) = self.account_book(adapter) else {
            return false;
        };
        let now = (self.now_unix_fn)();
        let book = book.lock().unwrap_or_else(|e| e.into_inner());
        let candidate = AccountCandidate {
            id: account_id,
            logged_in: dir.logged_in,
            in_use: self.account_in_use(adapter, account_id),
        };
        evaluate(
            &candidate,
            book.state(account_id),
            cfg.max_runs_per_account,
            now,
        )
        .excluded
        .is_none()
    }

    fn account_score(&self, adapter: AccountAdapter, id: &str) -> f64 {
        let Some(book) = self.account_book(adapter) else {
            return f64::NEG_INFINITY;
        };
        let book = book.lock().unwrap_or_else(|e| e.into_inner());
        let candidate = AccountCandidate {
            id,
            logged_in: true,
            in_use: self.account_in_use(adapter, id),
        };
        crate::accounts::evaluate(
            &candidate,
            book.state(id),
            self.config
                .accounts
                .as_ref()
                .map_or(1, |c| c.max_runs_per_account),
            (self.now_unix_fn)(),
        )
        .score
        .unwrap_or(f64::NEG_INFINITY)
    }

    /// ADR-0024 D3 / ADR-0025 D2: `[accounts]` の指定アダプタのプールから 1 アカウントを選ぶ（残量に基づく決定的な
    /// 選択）。そのアダプタの根ディレクトリが無い、または選べるアカウントが無ければ `None`。
    /// ディレクトリのスキャンは tick につき高々 1 回（アダプタごと）。
    fn pick_account(&mut self, adapter: AccountAdapter, requested: Option<&str>) -> Option<String> {
        let cfg = self.config.accounts.clone()?;
        let root = cfg.root_for(adapter)?;
        let dirs = self
            .accounts_scan_cache
            .entry(adapter)
            .or_insert_with(|| scan_accounts(root, adapter))
            .clone();
        let now = (self.now_unix_fn)();
        let book = self.account_book(adapter)?;
        let book = book.lock().unwrap_or_else(|e| e.into_inner());
        let candidates: Vec<AccountCandidate<'_>> = dirs
            .iter()
            .filter(|d| requested.is_none_or(|id| d.id == id))
            .map(|d| AccountCandidate {
                id: d.id.as_str(),
                logged_in: d.logged_in,
                in_use: self.account_in_use(adapter, &d.id),
            })
            .collect();
        select_account(&candidates, &book, cfg.max_runs_per_account, now)
    }

    /// ADR-0024 D4: プール run の供給側失敗をアカウントの cooldown として記録する（プロバイダは cooldown にしない）。
    /// `reason` は `provider_failure_reason` と同じ語彙（`throttled` / `auth_failed` / `exhausted` / `spawn`）。
    fn record_account_failure(
        &self,
        adapter: AccountAdapter,
        account_id: &str,
        reason: &str,
        outcome: &ProviderOutcome,
    ) {
        let Some(cfg) = &self.config.accounts else {
            return;
        };
        let now = (self.now_unix_fn)();
        let fallback_secs = match outcome {
            ProviderOutcome::Throttled { retry_after } if retry_after.as_secs() > 0 => {
                retry_after.as_secs()
            }
            _ => cfg.fallback_cooldown_secs,
        };
        let cooldown_reason = account_cooldown_reason_from_failure(reason);
        let Some(book) = self.account_book(adapter) else {
            return;
        };
        let Ok(mut book) = book.lock() else { return };
        let cooldown = {
            let state = book.state(account_id);
            cooldown_for_failure(state, cooldown_reason, now, fallback_secs)
        };
        book.set_cooldown(account_id, cooldown, now);
        if let Err(e) = book.save() {
            tracing::warn!(%account_id, %adapter, error = %e, "failed to save account book after cooldown");
        }
    }

    /// ADR-0024 D2 / ADR-0025 D2: プールから選んだアカウントの環境変数（claude-code は
    /// `CLAUDE_SECURESTORAGE_CONFIG_DIR`、codex は `CODEX_HOME`）を末尾に重ねたアダプタを返す。`with_env` が
    /// `None`（アダプタがこの経路を実装していない）なら `None`（呼び出し側は満杯として扱う）。
    fn adapter_for_account(
        &self,
        base: &Arc<dyn WorkerAdapter>,
        account_adapter: AccountAdapter,
        account_id: &str,
    ) -> Option<Arc<dyn WorkerAdapter>> {
        let cfg = self.config.accounts.as_ref()?;
        let root = cfg.root_for(account_adapter)?;
        let dir = root.join(account_id);
        base.with_env(&[(
            account_adapter.env_var().to_string(),
            dir.display().to_string(),
        )])
    }

    /// ADR-0005 D3: `Local{path}` がそのタスクの作業ディレクトリ。相対なら `workspace_root` 基準。
    ///
    /// ADR-0041 D1: ただし worktree を切るタスク（`mode = worktree` かつ `path` が git リポジトリ）では、
    /// **`runs/` `inputs/` `artifacts/` を置く場所**は `workspace_root/<task_id>` になる
    /// （作業ツリーそのものは `<そこ>/tree`。`git status --porcelain` を汚さないため外に出す）。
    fn task_dir(&self, task: &Task) -> Option<PathBuf> {
        match &task.workspace {
            WorkspaceSpec::Local { path, .. } => Some(match self.task_workspaces_for(task) {
                Some(ws) => ws.task_dir,
                None if path.is_absolute() => path.clone(),
                None => self.config.workspace_root.join(path),
            }),
            // ADR-0018 D1: クラスタ側が正で、手元は写し（`workspace_root/<task_id>`）。
            WorkspaceSpec::Remote { .. } => {
                Some(self.config.workspace_root.join(task.id.to_string()))
            }
        }
    }

    /// ADR-0041 D1 / ADR-0043 D2: そのタスクに用意する（用意した）ローカルの作業場所。
    ///
    /// 決め方は決定的（LLM は使わない）:
    ///
    /// 1. **タスクがリポジトリを選んでいる**（`task.repos`、ADR-0043 D2）→ `<task_dir>/repos/<name>/` に
    ///    1 つずつ並べる。`kind = git` で実際に git リポジトリなら worktree、そうでなければ実体への
    ///    シンボリックリンク（`kind = dir`、`mode = shared`、git でなかった場合）。
    ///    **リモートのリポジトリを含むタスクは `None`**（従来の ADR-0018 / 0019 の経路に倒す。
    ///    ローカルと混ぜたタスクは作成時に 422 で弾いてある）。
    /// 2. **選んでいない**（案件にリポジトリが無い・案件に属さない）→ Phase 49 と同じ 1 つだけの worktree
    ///    （`<task_dir>/tree`）。条件は 3 つ: `Local`、`mode = worktree`（既定）、`path` が git リポジトリ。
    ///
    /// base の決め方は `task_worker::resolve_base`（`main` / 本番の `current` / `HEAD`。ADR-0041 D1）。
    fn task_workspaces_for(&self, task: &Task) -> Option<task_worker::TaskWorkspaces> {
        let task_dir = self.config.workspace_root.join(task.id.to_string());
        if task.repos.is_empty() {
            let worktree = self.legacy_worktree_for(task, &task_dir)?;
            let name = task_worker::task_repos::repo_display_name(&worktree.repo);
            return Some(task_worker::TaskWorkspaces {
                task_dir,
                repos: vec![task_worker::TaskRepo::git(name, worktree)],
            });
        }
        let mut repos = Vec::with_capacity(task.repos.len());
        for reference in &task.repos {
            let row = match self.store.repo_get(reference.repo_id) {
                Ok(Some(row)) => row,
                Ok(None) => {
                    tracing::warn!(task_id = %task.id, repo = %reference.name, "the project repo is gone; falling back to the plain workspace");
                    return None;
                }
                Err(e) => {
                    tracing::error!(task_id = %task.id, repo = %reference.name, error = %e, "cannot read the project repo");
                    return None;
                }
            };
            let task_core::WorkspaceSpec::Local { path, mode } = &row.location else {
                // ADR-0043 D2 / D7: リモートは従来の経路（手元の写し + rsync）。
                return None;
            };
            let source = if path.is_absolute() {
                path.clone()
            } else {
                self.config.workspace_root.join(path)
            };
            let dir = task_dir.join(task_worker::REPOS_DIR_NAME).join(&row.name);
            let wants_worktree = row.kind == task_core::RepoKind::Git
                && mode.unwrap_or_default() == task_core::WorkspaceMode::Worktree
                && task_worker::is_git_repo(&source);
            let base = if wants_worktree {
                self.worktree_base_for(task, &source)
            } else {
                None
            };
            // A human-approved documentation reconciliation arrives with a committed worktree.
            // Preserve that exact branch for the subsequent ordinary verification/review task.
            if task_dir
                .join("artifacts/reconciliation-plan.json")
                .is_file()
                && let Some(marker) = task_ops::workspace::read_marker(&task_dir)
                && let Some(existing) = marker.repos.iter().find(|r| r.name == row.name)
                && std::path::Path::new(&existing.source) == source
                && std::path::Path::new(&existing.dir) == dir
                && let (Some(branch), Some(sha)) = (&existing.branch, &existing.base)
                && branch.starts_with("docs-reconcile/")
                && task_ops::changes::current_branch(&dir).as_ref() == Some(branch)
            {
                repos.push(task_worker::TaskRepo::git(
                    row.name.clone(),
                    task_worker::LocalWorktree {
                        dir,
                        task_dir: task_dir.clone(),
                        repo: source,
                        branch: branch.clone(),
                        base: task_worker::BaseRef {
                            kind: task_worker::BaseKind::Main,
                            sha: sha.clone(),
                        },
                    },
                ));
                continue;
            }
            match base {
                Some(base) => repos.push(task_worker::TaskRepo::git(
                    row.name.clone(),
                    task_worker::LocalWorktree {
                        dir,
                        task_dir: task_dir.clone(),
                        repo: source,
                        branch: format!("{}{}", self.config.worktree_branch_prefix, task.id),
                        base,
                    },
                )),
                None => repos.push(task_worker::TaskRepo::link(row.name.clone(), source, dir)),
            }
        }
        if repos.is_empty() {
            return None;
        }
        Some(task_worker::TaskWorkspaces { task_dir, repos })
    }

    /// ADR-0043 D3（Phase 56）: **このタスクをコンテナで走らせるか**。ストアと `workspace.toml` を
    /// 読むだけの決定的な判断で、LLM は使わない（DESIGN 原則 1）。
    ///
    /// - リモート（`WorkspaceSpec::Remote`）と作業場所の無いタスクはホスト（ADR-0043 D7 は後続）
    /// - `paperqa` / `local-deep-research` はホスト（道具立てがホストの venv にある。`container::decide`）
    /// - 1 つでも `run = container`（か `auto` + `[run] mode = "container"`）なら**コンテナ**
    /// - runtime が使えなければ `Unavailable`（run を始めず `blocked` にして人に聞く）
    fn container_decision(
        &self,
        task: &Task,
        worktree: Option<&task_worker::TaskWorkspaces>,
        adapter_id: &str,
        remote: bool,
    ) -> ContainerDecision {
        let Some(ws) = worktree else {
            return ContainerDecision::Host;
        };
        if remote {
            return ContainerDecision::Host;
        }
        // リポジトリごとの `run` と `is_primary`。Phase 49 の 1 リポジトリのタスクは
        // `project_repos` の行を持たないので `auto` + primary として扱う。
        let mut inputs: Vec<task_worker::RepoRunInput> = Vec::with_capacity(ws.repos.len());
        for repo in &ws.repos {
            let row = task
                .repos
                .iter()
                .find(|r| r.name == repo.name)
                .and_then(|r| self.store.repo_get(r.repo_id).ok().flatten());
            // `workspace.toml` は作業ツリーがあればそこ、無ければ元のリポジトリ（`repo_notes` と同じ規則）。
            let from = if repo.dir.is_dir() {
                &repo.dir
            } else {
                &repo.source
            };
            let (config, warning) = task_core::workspace_config::load_or_default(from);
            if let Some(warning) = warning {
                tracing::warn!(repo = %repo.name, %warning, "cannot read workspace.toml; using the defaults");
            }
            inputs.push(task_worker::RepoRunInput {
                name: repo.name.clone(),
                run: row
                    .as_ref()
                    .map(|r| r.run)
                    .unwrap_or(task_core::RepoRun::Auto),
                is_primary: row.as_ref().map(|r| r.is_primary).unwrap_or(true),
                config,
                config_dir: from.clone(),
            });
        }
        let Some(choice) = task_worker::container::decide(&inputs, adapter_id) else {
            return ContainerDecision::Host;
        };
        let Some(runtime) = self.container_probe.runtime else {
            return ContainerDecision::Unavailable {
                question: task_worker::container::unavailable_question(
                    &self.container_probe,
                    &choice.repo,
                ),
            };
        };
        // `dir` のリポジトリ（シンボリックリンク）は**実体**を同じパスでマウントする。
        let dir_repos: Vec<PathBuf> = ws
            .repos
            .iter()
            .filter(|r| !r.is_git())
            .map(|r| r.source.clone())
            .collect();
        let (uid, gid) = task_worker::container::host_ids();
        let plan = task_worker::ContainerPlan {
            runtime,
            program: runtime.as_str().to_string(),
            // イメージは `run_worker` が run の直前に決める（ビルドが要ることがある）。
            image: self.config.containers.image_default.clone(),
            task_dir: ws.task_dir.clone(),
            dir_repos,
            creds: Vec::new(),
            extra_mounts: choice.mounts.clone(),
            // ADR-0047 D3（Phase 61）: 知識ベースがあれば同じパスで見せる（`_inbox` だけ書き込み可）。
            knowledge_root: Some(self.config.knowledge.root.clone()).filter(|r| r.is_dir()),
            env: choice.env.clone(),
            task_id: task.id.to_string(),
            uid,
            gid,
        };
        ContainerDecision::Container(Box::new(ContainerRun {
            plan,
            image: choice.image,
            image_default: self.config.containers.image_default.clone(),
            build_root: self.config.containers.build_dir.clone(),
            build_timeout: self.config.containers.build_timeout,
            repo: choice.repo,
        }))
    }

    /// Phase 49（ADR-0041 D1）の 1 リポジトリだけの worktree（`<task_dir>/tree`）。
    fn legacy_worktree_for(
        &self,
        task: &Task,
        task_dir: &Path,
    ) -> Option<task_worker::LocalWorktree> {
        let WorkspaceSpec::Local { path, .. } = &task.workspace else {
            return None;
        };
        if task.workspace.local_mode() != task_core::WorkspaceMode::Worktree {
            return None;
        }
        let repo = if path.is_absolute() {
            path.clone()
        } else {
            self.config.workspace_root.join(path)
        };
        if !task_worker::is_git_repo(&repo) {
            return None;
        }
        let base = self.worktree_base_for(task, &repo)?;
        Some(task_worker::LocalWorktree {
            dir: task_dir.join(task_worker::WORKTREE_DIR_NAME),
            task_dir: task_dir.to_path_buf(),
            repo,
            branch: format!("{}{}", self.config.worktree_branch_prefix, task.id),
            base,
        })
    }

    /// ADR-0079 D6（Phase R1c）: task の worktree の base。木の子 task は既定ブランチではなく
    /// `Task.tree.base_commit`（親の段階の基点、または同じ段階の依存先の HEAD。子を作るときに
    /// [`Self::child_base_commit`] が決めた値）から切る（`BaseKind::Parent`）。この値が `repo` で解決できない
    /// （子の repos のうち先頭以外のリポジトリ。基点は先頭のリポジトリの sha だけを持つ）ときは、そのリポジトリの
    /// 親のブランチ `celeris/<parent_id>` の HEAD（親のブランチは段階の途中では動かない。D6）。どちらも
    /// 無ければ従来の規則（`main`）に倒す。木の子でない task は従来どおり [`Self::worktree_base`]。
    fn worktree_base_for(&self, task: &Task, repo: &Path) -> Option<task_worker::BaseRef> {
        if let Some(parent_branch) =
            task_core::tree::parent_branch(task, &self.config.worktree_branch_prefix)
        {
            let sha = task_core::tree::child_base_commit(task)
                .and_then(|sha| crate::integration::commit_in(repo, sha))
                .or_else(|| {
                    crate::integration::rev_parse(repo, &format!("refs/heads/{parent_branch}"))
                });
            if let Some(sha) = sha {
                return Some(task_worker::BaseRef {
                    kind: task_worker::BaseKind::Parent,
                    sha,
                });
            }
            tracing::warn!(task_id = %task.id, repo = %repo.display(), %parent_branch, "neither the child's base_commit nor the parent's branch exists in this repository; the child's worktree falls back to the default base (ADR-0079 D6)");
        }
        self.worktree_base(repo)
    }

    /// ADR-0041 D1 の base の規則（`main` / 本番の `current` / `HEAD`）。
    fn worktree_base(&self, repo: &Path) -> Option<task_worker::BaseRef> {
        let current = self
            .config
            .releases_dir
            .as_deref()
            .and_then(task_worker::current_release_sha);
        task_worker::resolve_base(repo, current.as_deref())
    }

    /// ADR-0043 D2 / D4 / D8: 前置きの「作業場所」に出すリポジトリ一覧（決定的。`workspace.toml` を
    /// 読むだけで、判断も LLM も無い）。`description` / `check` / `outputs` は
    /// `.config/celeris/workspace.toml` に**書いてあることだけ**を使う（ADR-0043 §3）。
    ///
    /// 読む場所は、作業ツリーが既にあればそこ、無ければ元のリポジトリ（初回の dispatch では
    /// worktree をまだ切っていないため）。
    fn repo_notes(&self, ws: &task_worker::TaskWorkspaces) -> Vec<task_worker::preamble::RepoNote> {
        ws.repos
            .iter()
            .map(|repo| {
                let from = if repo.dir.is_dir() { &repo.dir } else { &repo.source };
                let (config, warning) = task_core::workspace_config::load_or_default(from);
                if let Some(warning) = warning {
                    tracing::warn!(repo = %repo.name, %warning, "cannot read workspace.toml; using the defaults");
                }
                task_worker::preamble::RepoNote {
                    name: repo.name.clone(),
                    dir: repo.dir.to_string_lossy().into_owned(),
                    git: repo.is_git(),
                    branch: repo.branch().map(str::to_string),
                    base: repo.worktree.as_ref().map(|w| w.base.sha12()),
                    base_kind: repo.worktree.as_ref().map(|w| w.base.kind.as_str().to_string()),
                    description: config.workspace.description.clone(),
                    check: config.commands.check.clone(),
                    docs: config.outputs.docs.clone(),
                    deliverables: config.outputs.deliverables.clone(),
                }
            })
            .collect()
    }

    /// ADR-0043 D2 / D4: 計画 run に渡す「この案件のリポジトリ」（名前 / 種類 / 置き場 / `workspace.toml` の
    /// `description`）。ストアと `workspace.toml` を読むだけで、判断も LLM も無い。
    fn project_repo_notes(
        &self,
        task: &Task,
    ) -> Result<Vec<task_worker::preamble::ProjectRepoNote>, DispatchError> {
        let repos =
            task_ops::delegate::project_repos(self.store.as_ref(), task).map_err(ops_to_store)?;
        Ok(repos
            .into_iter()
            .map(|repo| {
                let (location, local) = match &repo.location {
                    WorkspaceSpec::Local { path, .. } => {
                        (path.display().to_string(), Some(path.clone()))
                    }
                    WorkspaceSpec::Remote { cluster, path, .. } => {
                        (format!("{cluster}:{}", path.display()), None)
                    }
                };
                let description = local.as_deref().and_then(|dir| {
                    task_core::workspace_config::load_or_default(dir)
                        .0
                        .workspace
                        .description
                });
                task_worker::preamble::ProjectRepoNote {
                    name: repo.name,
                    kind: repo.kind.as_str().to_string(),
                    location,
                    description,
                    is_primary: repo.is_primary,
                }
            })
            .collect())
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

    /// ADR-0072 D21（Phase E3）: WU の run の lane。`decide_lane` と同じ天井（担当ノードの実効
    /// profile）を使うが、`TaskFeatures` は WU の view（`decide_for_work_unit`）で計算する。
    /// エスカレーション（リトライでの lane の引き上げ）は WU の retries を数えないので、E3 では
    /// 行わない（`task.attempts` は計画のある Task では WU の失敗で増えない。D11）。
    /// ADR-0074 D5.2（Phase F1）: `[execution] work_unit_lane_cap = "task"`（既定）のときは、Task
    /// 自身の（policy が決めた、エスカレーション前の）lane を上限の材料として渡す。`"none"` なら
    /// 上限を掛けない。
    fn decide_lane_for_work_unit(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
    ) -> Result<Option<task_core::LaneDecision>, DispatchError> {
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
        let task_lane = match self.config.execution.work_unit_lane_cap {
            task_core::WorkUnitLaneCap::Task => {
                task_core::model_policy::decide_for_task(task, &ceiling).map(|d| d.lane)
            }
            task_core::WorkUnitLaneCap::None => None,
        };
        Ok(task_core::model_policy::decide_for_work_unit(
            task, wu, &ceiling, task_lane,
        ))
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

    /// テスト用: Phase 49 のときの「1 つだけの worktree」の姿（`task_workspaces_for` の先頭）。
    #[cfg(test)]
    fn local_worktree_for(&self, task: &Task) -> Option<task_worker::LocalWorktree> {
        self.task_workspaces_for(task)
            .and_then(|ws| ws.repos.into_iter().next())
            .and_then(|r| r.worktree)
    }

    fn work_dir_for(&self, task: &Task) -> Option<PathBuf> {
        self.task_workspaces_for(task)
            .and_then(|ws| ws.cwd().map(Path::to_path_buf))
    }

    /// ADR-0036 D1: そのタスクの成果物ディレクトリ（`<dir>/artifacts` か `<dir>/.taskd/artifacts/<task_id>`）。
    /// 判定は `task_core::artifacts`（純粋関数）。アダプタ・レビュー・記憶の読み取りはすべてこれを使う。
    fn artifacts_dir(&self, task: &Task, workspace_dir: &Path) -> PathBuf {
        task_core::artifacts::artifacts_dir_for(task, workspace_dir)
    }

    /// ADR-0018: `WorkspaceSpec::Remote` のタスクのクラスタ設定とリモートのパス。ローカルのタスクは `None`。
    ///
    /// ADR-0062 B1（Phase 107）: **担当が `cluster:<id>` を持たないなら接続経路を渡さない**（remote を
    /// 組まない）。人間の指示により、ADR-0046 D8 の「道具を 1 つも宣言していないノードは従来どおり通す」
    /// という例外は**クラスタの利用可否についてだけ**廃止した（`task_may_use_cluster`）。
    ///
    /// ADR-0059 D6: `path` が絶対・`~`/`~/…` ならそのまま、それ以外（省略・相対）は実効 `work_dir`
    /// （DB 上書き `cluster_settings` > 設定の `[[clusters]] work_dir` > 無し）からの相対に解決する。
    /// 解決できなければ `path` をそのまま返す（`remote_dir_is_resolved` で検出できる形のまま。
    /// ここでは `None` にしない — 「クラスタが無い」とは別の失敗なので `unroutable` に混ぜない）。
    fn cluster_of(&mut self, task: &Task) -> Option<(ClusterSpec, PathBuf, WorkspaceMode)> {
        match self.resolve_cluster(task) {
            ClusterResolution::Resolved(spec, path, mode) => Some((*spec, path, mode)),
            ClusterResolution::Local
            | ClusterResolution::NotConfigured
            | ClusterResolution::AssigneeLacksTool { .. } => None,
        }
    }

    /// `cluster_of` の中身。`None` の理由を dispatch_ready が区別できるようにする（ADR-0062 B1）。
    fn resolve_cluster(&mut self, task: &Task) -> ClusterResolution {
        match &task.workspace {
            WorkspaceSpec::Local { .. } => ClusterResolution::Local,
            WorkspaceSpec::Remote { cluster, path, .. } => {
                if !self.task_may_use_cluster(task, cluster) {
                    return ClusterResolution::AssigneeLacksTool {
                        cluster: cluster.clone(),
                    };
                }
                let Some(spec) = self.config.clusters.get(cluster).cloned() else {
                    return ClusterResolution::NotConfigured;
                };
                let db_work_dir = self
                    .store
                    .cluster_settings_get(cluster)
                    .ok()
                    .flatten()
                    .and_then(|s| s.work_dir)
                    .map(PathBuf::from);
                let work_dir = db_work_dir.or_else(|| spec.work_dir.clone());
                let resolved =
                    resolve_remote_dir(path, work_dir.as_deref()).unwrap_or_else(|| path.clone());
                ClusterResolution::Resolved(Box::new(spec), resolved, task.workspace.remote_mode())
            }
        }
    }

    /// ADR-0062 B1: そのタスクの担当がそのクラスタを使えるか（決定的。組織の profile だけを見る）。
    /// 人間の指示（Phase 107 追加指示）により、ADR-0046 D8 の「道具を 1 つも宣言していないノードは
    /// 従来どおり通す」という例外はここでは**廃止した**（remote 作業場所でクラスタに繋いでよいのは、
    /// 実効 profile に `cluster:<id>` を持つノードだけ）。warn はタスクごとに 1 回
    /// （`warned_cluster_tool`。`warned_unroutable` と同じ流儀）。
    fn task_may_use_cluster(&mut self, task: &Task, cluster: &str) -> bool {
        let Some(assignee) = task.assignee.as_deref() else {
            return true;
        };
        let org = match self.store.org_list() {
            Ok(org) => org,
            Err(e) => {
                tracing::warn!(task_id = %task.id, error = %e, "could not read the org tree; allowing the cluster");
                return true;
            }
        };
        let effective = task_core::resolve_profile(&org, assignee);
        let wanted = format!("{}{cluster}", task_core::CLUSTER_TOOL_PREFIX);
        if effective.has_tool(&wanted) {
            self.warned_cluster_tool.remove(&task.id);
            return true;
        }
        if self.warned_cluster_tool.insert(task.id) {
            tracing::warn!(
                task_id = %task.id, %assignee, %cluster,
                "assignee does not have the cluster tool; not wiring the remote (ADR-0062 B1)"
            );
        }
        false
    }

    /// そのクラスタで走っている run の数（ADR-0018 D5: プロバイダとクラスタの二次元）。
    fn cluster_in_use(&self, cluster: &str) -> usize {
        self.running
            .values()
            .filter(|e| e.cluster.as_deref() == Some(cluster))
            .count()
            + self
                .reviewing
                .values()
                .filter(|e| e.cluster.as_deref() == Some(cluster))
                .count()
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

/// ADR-0043 D3 / D4: `[commands] setup` の 1 コマンドあたりの上限（設定キーにはしない。
/// `cargo fetch` / `pnpm install` が入る想定で、run の予算とは別に取る）。
const SETUP_TIMEOUT: Duration = Duration::from_secs(1800);

/// ADR-0043 D3（Phase 56）: 起動時の `<runtime> info` の上限（届かない docker デーモンで固まらない）。
const CONTAINER_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// ADR-0041 D1 / ADR-0043 D2: `<task_dir>/worktree.json` の中身。先頭の 5 つは Phase 49 からある
/// 「1 リポジトリのときの姿」で、複数リポジトリのタスクでは `repos[0]`（cwd になるもの）の写しが入る。
fn worktree_marker(ws: &task_worker::TaskWorkspaces) -> task_ops::workspace::WorktreeMarker {
    let first = ws.repos.first();
    task_ops::workspace::WorktreeMarker {
        repo: first
            .map(|r| r.source.to_string_lossy().into_owned())
            .unwrap_or_default(),
        dir: first
            .map(|r| r.dir.to_string_lossy().into_owned())
            .unwrap_or_default(),
        branch: first
            .and_then(|r| r.branch())
            .unwrap_or_default()
            .to_string(),
        base: first
            .and_then(|r| r.worktree.as_ref())
            .map(|w| w.base.sha.clone())
            .unwrap_or_default(),
        base_kind: first
            .and_then(|r| r.worktree.as_ref())
            .map(|w| w.base.kind.as_str().to_string())
            .unwrap_or_default(),
        repos: ws
            .repos
            .iter()
            .map(|r| task_ops::workspace::WorktreeMarkerRepo {
                name: r.name.clone(),
                kind: if r.is_git() {
                    "git".into()
                } else {
                    "dir".into()
                },
                source: r.source.to_string_lossy().into_owned(),
                dir: r.dir.to_string_lossy().into_owned(),
                branch: r.branch().map(str::to_string),
                base: r.worktree.as_ref().map(|w| w.base.sha.clone()),
                base_kind: r
                    .worktree
                    .as_ref()
                    .map(|w| w.base.kind.as_str().to_string()),
            })
            .collect(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_worker(
    store: Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    task_id: TaskId,
    execution_tier: task_core::Tier,
    dir: PathBuf,
    run_id: &str,
    limits: RunLimits,
    lease: LeaseRenewal,
    remote: Option<SshSettings>,
    worktree: Option<task_worker::TaskWorkspaces>,
    extras: RunExtras,
    roles: Vec<RoleSpec>,
    genres: Vec<GenreSpec>,
    delegation: DelegationLimits,
    account: Option<String>,
    account_book: Option<Arc<StdMutex<AccountBook>>>,
    container: ContainerDecision,
    // ADR-0066 D1（Phase 110b）/ ADR-0075 D3（Phase G1）: ローカルの git worktree のホスト実行に限って
    // `CARGO_TARGET_DIR` を与える（コンテナ・Remote は対象外。`container_plan` が決まってから判断する）。
    // scratch なら `<scratch>/targets/<owner>/target`、無効なら `<build_cache_dir>/cargo/<repo-key>[/wu-<id>]`。
    cargo_target: CargoTargetPlan,
) -> Result<RunOutcome, AdapterError> {
    // リース取得後の状態（running, lease あり）をワーカーに渡す。
    let mut task = store
        .get(task_id)
        .map_err(|e| AdapterError::Other(format!("store: {e}")))?
        .ok_or_else(|| AdapterError::Other("task vanished".into()))?;
    task.worker_hint.tier = execution_tier;
    // ADR-0072 D14（Phase E4b 項目3）: `extras` は後段で複数のフィールドが個別に消費されるので、
    // 使う値だけ先に取り出しておく。
    let planner_permission_mode = extras.planner_permission_mode.clone();
    // ADR-0052 D2（Phase 64）: フォールバックした知識整理 run は、DB のタスクではなく**ワーカーに渡す
    // 写し**だけを書き換える（専用アダプタの固定を外し、予算を `max_turns = 8` / `max_wall_secs = 600` に）。
    if let Some(fallback) = &extras.knowledge_fallback {
        task.worker_hint.adapter = None;
        task.budget = fallback.budget;
        tracing::debug!(task_id = %task_id, adapter = %fallback.adapter, "knowledge: running the fallback extraction");
    }
    // ADR-0018 D1/D3: リモート実行のタスクは、クラスタの内容を写しに取り込み、ラッパを置き、その使い方を指示文に足す
    // （DB のタスクは変えない。ワーカーに渡す写しだけ）。ADR-0059 D6: `path` が実効 `work_dir` から解決
    // できなかった（`cluster_of` が絶対・`~` 始まりに直せなかった）ら、worktree 準備を試す前にここで
    // 明確なエラーにする。
    // ADR-0079 R5b-fix2: remote なら run の後に push するため、用意した `SshWorkspace` を控える。
    let mut remote_ws: Option<SshWorkspace> = None;
    let workspace = match &remote {
        Some(settings) => {
            if !remote_dir_is_resolved(&settings.remote_dir) {
                return Err(AdapterError::Other(format!(
                    "cluster {}: no working directory registered for {:?}; register one via \
                     `PUT /clusters/{}/settings` or the clusters screen (ADR-0059 D6)",
                    settings.cluster, settings.remote_dir, settings.cluster
                )));
            }
            let mut effective_settings = settings.clone();
            let mut ws = SshWorkspace::new(&dir, effective_settings.clone());
            let prepared = match ws.prepare(&task).await {
                Ok(p) => p,
                // ADR-0059 D3: 自動の格下げ。`mode` 省略・`repos` 無し（コードを触らない仕事）なら、
                // worktree が切れなくても失敗にせず `shared`（`SyncMode::None`）として同じ run の中で
                // 続行する。明示的に `mode = "worktree"` を選んだタスクは格下げしない（利用者の意図を
                // 尊重し、設定ミスを隠さない）。
                Err(WorkspaceError::NotAGitRepository(reason))
                    if remote_mode_omitted(&task.workspace) && task.repos.is_empty() =>
                {
                    effective_settings.sync = SyncMode::None;
                    ws = SshWorkspace::new(&dir, effective_settings.clone());
                    let downgraded = ws.prepare(&task).await.map_err(|e| {
                        workspace_error_to_adapter(e, "workspace prepare (downgraded to shared)")
                    })?;
                    if let Some(new_workspace) = downgraded_remote_workspace(&task.workspace) {
                        if let Ok(Some(mut fresh)) = store.get(task_id) {
                            fresh.workspace = new_workspace.clone();
                            fresh.updated_at = OffsetDateTime::now_utc();
                            if let Err(e) = store.update_task(
                                &fresh,
                                Event::WorkspaceModeDowngraded {
                                    cluster: effective_settings.cluster.clone(),
                                    path: effective_settings
                                        .remote_dir
                                        .to_string_lossy()
                                        .into_owned(),
                                    reason,
                                },
                            ) {
                                tracing::warn!(task_id = %task_id, error = %e, "could not persist the workspace mode downgrade (ADR-0059 D3)");
                            }
                        }
                        task.workspace = new_workspace;
                    }
                    downgraded
                }
                Err(e) => return Err(workspace_error_to_adapter(e, "workspace prepare")),
            };
            ws.write_remote_exec_helper()
                .await
                .map_err(|e| workspace_error_to_adapter(e, "remote-exec helper"))?;
            task.objective
                .push_str(&remote_exec_instructions(&effective_settings));
            remote_ws = Some(ws);
            prepared
        }
        // ADR-0041 D1 / ADR-0043 D2: ローカルの作業場所（1 つ以上のリポジトリ）を用意し、その
        // 先頭をワーカーのカレントディレクトリにする（`runs/` `inputs/` `artifacts/` は作業ツリーの外の `dir`）。
        None => {
            let mut ws = LocalWorkspace::new(&dir);
            if let Some(wt) = &worktree {
                wt.ensure()
                    .await
                    .map_err(|e| workspace_error_to_adapter(e, "worktree prepare"))?;
                // 目印（`<task_dir>/worktree.json`）: API・CLI はこれを見て「run のログと成果物は
                // 作業ツリーの外にある」と判断する（git を起こさない。worktree を消した後も残す）。
                if let Err(e) =
                    task_ops::workspace::write_marker(&wt.task_dir, &worktree_marker(wt))
                {
                    tracing::warn!(task_id = %task_id, error = %e, "could not write the worktree marker");
                }
                if let Some(cwd) = wt.cwd() {
                    ws = ws.with_work_dir(cwd);
                }
            }
            ws.prepare(&task)
                .await
                .map_err(|e| AdapterError::Other(format!("workspace prepare: {e}")))?
        }
    };
    // ADR-0043 D3（Phase 56）: この run の実行環境。コンテナなら**イメージをここで用意する**
    // （`[container] image` はそのまま、`dockerfile` は内容の sha のタグでビルドしてキャッシュ）。
    // runtime が無い・ビルドが落ちたときは run を始めず、`setup` の失敗と同じ経路で人に聞く。
    let container_plan: Option<task_worker::SharedPlan> = match container {
        ContainerDecision::Host => None,
        ContainerDecision::Unavailable { question } => {
            tracing::warn!(task_id = %task_id, %question, "no container runtime; asking a human");
            return Ok(RunOutcome {
                terminal: Terminal::Question { text: question },
                exit_code: None,
            });
        }
        ContainerDecision::Container(run) => {
            let mut run = *run;
            let log_path = dir.join(task_worker::container::BUILD_LOG);
            let resolved = match task_worker::container::resolve_image(
                &run.image,
                &run.image_default,
                &run.build_root,
            ) {
                Ok(resolved) => resolved,
                Err(e) => {
                    tracing::warn!(task_id = %task_id, error = %e, "cannot resolve the container image");
                    return Ok(RunOutcome {
                        terminal: Terminal::Question {
                            text: task_worker::container::image_question(&run.repo, &e, &log_path),
                        },
                        exit_code: None,
                    });
                }
            };
            match resolved {
                task_worker::container::ResolvedImage::Ready(tag) => run.plan.image = tag,
                task_worker::container::ResolvedImage::Build(request) => {
                    let program = run.plan.program.clone();
                    let exists =
                        move |tag: &str| task_worker::container::image_exists(&program, tag);
                    if let Err(e) = task_worker::container::ensure_image(
                        &run.plan.program,
                        &request,
                        run.build_timeout,
                        &log_path,
                        &exists,
                    )
                    .await
                    {
                        tracing::warn!(task_id = %task_id, error = %e, "container image build failed");
                        return Ok(RunOutcome {
                            terminal: Terminal::Question {
                                text: task_worker::container::image_question(
                                    &run.repo, &e, &log_path,
                                ),
                            },
                            exit_code: None,
                        });
                    }
                    run.plan.image = request.tag;
                }
            }
            tracing::info!(task_id = %task_id, image = %run.plan.image, runtime = run.plan.runtime.as_str(), repo = %run.repo, "running in a container");
            Some(Arc::new(run.plan))
        }
    };
    // ADR-0043 D3 / D4: worktree を作った直後に `[commands] setup` を**一度だけ**流す
    // （記録は `runs/setup.log`。そのファイルがあれば済んでいる）。コンテナのタスクは
    // **コンテナの中で**流す（Phase 56）。落ちたら run を始めず、既存の質問の経路で
    // タスクを `blocked` にして人に聞く。
    if let Some(wt) = &worktree
        && remote.is_none()
        && !wt
            .task_dir
            .join(task_worker::task_repos::SETUP_LOG)
            .exists()
    {
        match task_worker::run_setup_in(
            &wt.repos,
            &wt.task_dir,
            SETUP_TIMEOUT,
            container_plan.as_ref(),
        )
        .await
        {
            Ok(outcome) if outcome.ok => {}
            Ok(outcome) => {
                tracing::warn!(task_id = %task_id, failures = ?outcome.failures, "setup failed; asking a human");
                return Ok(RunOutcome {
                    terminal: Terminal::Question {
                        text: format!(
                            "setup が失敗しました（`.config/celeris/workspace.toml` の `[commands] setup`）: {}。\
                             記録は `{}` にあります。直し方を教えてください（設定を直す／この手順を飛ばす）。",
                            outcome.failures.join(" / "),
                            wt.task_dir
                                .join(task_worker::task_repos::SETUP_LOG)
                                .display()
                        ),
                    },
                    exit_code: None,
                });
            }
            Err(e) => {
                tracing::warn!(task_id = %task_id, error = %e, "could not run setup");
                return Ok(RunOutcome {
                    terminal: Terminal::Question {
                        text: format!(
                            "setup が失敗しました（`.config/celeris/workspace.toml` の `[commands] setup` を流せませんでした）: {e}。\
                             直し方を教えてください（設定を直す／この手順を飛ばす）。"
                        ),
                    },
                    exit_code: None,
                });
            }
        }
    }
    // ADR-0041 D1 / ADR-0043 D2: ワーカーの cwd は先頭のリポジトリ（`workspace` は足回りの親のまま）。
    let work_dir = worktree
        .as_ref()
        .and_then(|wt| wt.cwd())
        .map(|cwd| cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()));
    // ADR-0036 D1: 成果物ディレクトリはタスクごと（共有 workspace では `.taskd/artifacts/<task_id>/`）。
    // 決めるのはディスパッチャで、アダプタは `req.artifacts_dir` に書くだけ。
    let artifacts_dir = match &extras.artifacts_dir_override {
        // ADR-0074 D1.2（Phase F2b）: v2 の WU の run は WU ごとの成果物の置き場を使う。
        Some(dir) => {
            let _ = tokio::fs::create_dir_all(dir).await;
            dir.clone()
        }
        None => task_core::artifacts::artifacts_dir_for(&task, &workspace),
    };
    // ADR-0067 D3: run 完了後に未申告の成果物を拾うための控え（`workspace`/`artifacts_dir` はこの後
    // `req` に移る）。git worktree ではない local の作業場所に限る（`remote.is_none() &&
    // worktree.is_none()` を後段で見る）。
    let workspace_for_undeclared_scan = workspace.clone();
    let artifacts_dir_for_undeclared_scan = artifacts_dir.clone();
    if task.kind == TaskKind::Plan {
        // ADR-0007 D1: 前回の run の plan.json を今回の出力と誤読しない。
        let _ = tokio::fs::remove_file(artifacts_dir.join(PLAN_FILE_NAME)).await;
    }
    let events = store
        .events_for(task_id)
        .map_err(|e| AdapterError::Other(format!("store: {e}")))?;
    let prior_review = to_prior_review(prior_review_from_events(&events));
    // ADR-0044 D2: 対話 run（人への返事だけをする run。Phase 28）にはコメントの書き方を出さない。
    let writes_comments = extras.conversation_addressee.is_none();
    let cargo_target_work_unit = extras.cargo_target_work_unit.clone();
    let mut req = RunRequest {
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: task.clone(),
        workspace,
        work_dir,
        artifacts_dir,
        context: RunContext {
            browser: None,
            browser_policy: store
                .browser_task_policy_get(task.id)
                .map_err(|e| AdapterError::Other(format!("browser policy: {e}")))?,
            prior_review,
            inputs: task.inputs.clone(),
            answers: to_answers(answers_from_events(&events)),
            review: None,
            role: extras.role,
            children: extras.children,
            available_genres: extras.available_genres,
            node: extras.node,
            memory: extras.memory,
            conversation: extras.conversation,
            // ADR-0033 D5（Phase 26）: 担当宛て + 全員向けの永続の認可。
            standing_rules: extras.standing_rules,
            organization: extras.organization,
            // ADR-0059 D6（Phase 99）: CoS の対話 run だけに入る。
            clusters: extras.clusters,
            conversation_addressee: extras.conversation_addressee,
            work_genre: extras.work_genre,
            recent_work: extras.recent_work,
            // Phase 41（ADR-0038 D1）: 途中目標レビューの対話 run だけに入る。
            milestone_review: extras.milestone_review,
            // Phase 43（ADR-0039 D3）: 案件が作業場所を決めている run だけに入る。
            workspace_note: extras.workspace_note,
            knowledge: extras.knowledge,
            // ADR-0044 D2（Phase 53）: コメントの糸と、直前の run を止めた人のコメント。
            comments: extras.comments,
            interrupt: extras.interrupt,
            // ADR-0046 D1 / D4（Phase 59）: 実効 profile と進め方（どちらも既定なら `None`）。
            profile: extras.profile,
            mode: extras.mode,
            // 仕事の run はコメントを書ける。**対話 run は書かせない**（Phase 28 の「返事だけをする」と
            // ぶつかる）。レビュー run は `review.rs` が `RunContext::default()` を使うので既定の false。
            comments_enabled: writes_comments,
            // Phase 38（ADR-0028 追記）: レビュー run（`review.rs` が組む）だけに入る。
            subject_genre: None,
            // ADR-0048 D3（Phase 60b）: CoS の対話 run だけに入る。
            active_projects: extras.active_projects,
            // ADR-0054 D1（Phase 67）: 継続セッション（CoS の対話・部門長のレビュー run だけ）。
            session: extras.session,
            session_diff: extras.session_diff,
            // ADR-0056 D3（Phase 79）: mount された skills（KB に実在したものだけ）。
            skills: extras.skills,
            // ADR-0072 D9（Phase E1/E2）: 予算切れ・yield の続きなら、前の run の checkpoint と
            // これまでの run の 1 行要約。continuation でない run では `None`
            // （プロンプトは D10 の追加分を除きバイト単位で従来どおり）。WU の run では
            // `dispatch_ready` が `runs` 索引から組み立てた文脈（`continuation_override`）を使う
            // （events は WU をまたぐ run_id の区別を持たないため）。
            continuation: extras
                .continuation_override
                .clone()
                .or_else(|| build_continuation_context(&events)),
            // ADR-0072 D9/D21（Phase E2）: 計画のある Task の WU の run にだけ `Some`。
            work_unit: extras.work_unit.clone(),
            // ADR-0072 D13/D14（Phase E3）: task-local な planner run にだけ `Some`。
            execution_planner: extras.execution_planner.clone(),
            decision_requests: extras.decision_requests,
        },
    };
    // ADR-0066 D1（Phase 110b）: ローカルの git worktree のホスト実行にだけ、共有ビルドキャッシュの
    // `CARGO_TARGET_DIR` を与える（コンテナ実行〈`container_plan.is_some()`〉と Remote は対象外）。
    let repo_for_target = worktree
        .as_ref()
        .and_then(|wt| wt.repos.first())
        .filter(|r| r.is_git() && container_plan.is_none() && remote.is_none())
        .cloned();
    // ADR-0075 D4（Phase G2）: scratch なら env は `task_worker::scratch::cargo_env`（checks・`celerisctl scratch env` と
    // 同じ関数。`CARGO_TARGET_DIR`・`[scratch.cargo]`・server が応答すれば sccache 系）。legacy は `CARGO_TARGET_DIR` だけ。
    let target: Option<(PathBuf, task_worker::scratch::CargoEnv)> = match (
        &cargo_target,
        repo_for_target,
    ) {
        (CargoTargetPlan::None, _) | (_, None) => None,
        // ADR-0075 D3: owner は Task 単位の run なら `task-<id>`、自分の worktree の WU なら `task-<id>/wu-<id>`。
        (
            CargoTargetPlan::Scratch {
                settings,
                candidates,
            },
            Some(repo),
        ) => {
            let owner = match &cargo_target_work_unit {
                Some((id, _)) => task_worker::scratch::Owner::work_unit(task_id.to_string(), id),
                None => task_worker::scratch::Owner::task(task_id.to_string()),
            };
            let (settings, candidates) = (settings.clone(), candidates.clone());
            let key = cargo_target_work_unit.as_ref().map(|(_, k)| k.clone());
            // 割り当てに失敗したら sccache は配線しない（継いだ族は外す。G3-fix1）。
            let fallback = (
                settings.pool().target_dir(&owner),
                task_worker::scratch::CargoEnv::from_set(task_worker::scratch::target_env(
                    &settings.pool(),
                    &owner,
                )),
            );
            match tokio::task::spawn_blocking(move || {
                let target = allocate_scratch_target(&settings, &candidates, &owner, &repo, key);
                let state =
                    task_worker::scratch::resolve_sccache(&settings, task_worker::scratch::server_listening);
                if let Some(reason) = state.reason() {
                    tracing::debug!(owner = %owner, state = state.label(), reason, "scratch: sccache is not wired for this run (ADR-0075 D4)");
                }
                // adopt は owner のパスへ rename するので、`target` は env の `CARGO_TARGET_DIR` と同じ。
                // G3-fix1: 与えない sccache の族は `remove`（継いだ値を外す）。
                let env = task_worker::scratch::cargo_child_env_with(&settings, &owner, &state);
                (target, env)
            })
            .await
            {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!(task_id = %task_id, error = %e, "scratch: allocation task failed; using the target path");
                    Some(fallback)
                }
            }
        }
        // ADR-0074 F5-fix（不具合 1）: 並列の WU は `<repo-key>/wu-<id>`（兄弟 WU の別ブランチの
        // 生成物を混ぜない）。Task 単位の run は従来どおり `<repo-key>`。
        (CargoTargetPlan::Legacy(build_cache_dir), Some(repo)) => {
            let dir = match &cargo_target_work_unit {
                Some((id, _)) => task_worker::build_cache::work_unit_cargo_target_dir(
                    build_cache_dir,
                    &repo.source,
                    id,
                ),
                None => task_worker::build_cache::cargo_target_dir(build_cache_dir, &repo.source),
            };
            let env = task_worker::scratch::CargoEnv::set_only(vec![(
                task_worker::build_cache::CARGO_TARGET_DIR_VAR.to_string(),
                dir.display().to_string(),
            )]);
            Some((dir, env))
        }
    };
    let adapter = if let Some((target, env)) = target {
        // ADR-0075 G3-fix1: 与えない sccache の族（daemon から継いだ `RUSTC_WRAPPER` / `SCCACHE_*`）を先に外す。
        // `env_remove` を持たないアダプタには `RUSTC_WRAPPER` などを空の値で上書きして代える（cargo は空を未設定と扱う）。
        let (adapter, set) = if env.remove.is_empty() {
            (adapter, env.set)
        } else {
            match adapter.with_env_removed(&env.remove) {
                Some(wrapped) => (wrapped, env.set),
                None => {
                    tracing::debug!(task_id = %task_id, adapter = %adapter.id(), "adapter does not support with_env_removed; overriding RUSTC_WRAPPER with an empty value (ADR-0075 G3-fix1)");
                    let set = env.set_with_empty_wrappers();
                    (adapter, set)
                }
            }
        };
        match adapter.with_env(&set) {
            Some(wrapped) => {
                // request.json（監査）に実際に与えた値を残す。
                req.cargo_target_dir = Some(target);
                wrapped
            }
            None => {
                tracing::debug!(task_id = %task_id, adapter = %adapter.id(), "adapter does not support with_env; CARGO_TARGET_DIR was not applied (ADR-0066 D1)");
                adapter
            }
        }
    } else {
        adapter
    };
    // ADR-0043 D3（Phase 56）: コンテナで走らせる run は、ここでアダプタを包んだ複製に差し替える
    // （差し込み点はアダプタ側の `container::wrap` 1 か所）。この経路を持たないアダプタ
    // （`with_container` が `None`）はホストのまま走る。
    let adapter = match &container_plan {
        Some(plan) => match adapter.with_container(Arc::clone(plan)) {
            Some(wrapped) => wrapped,
            None => {
                tracing::warn!(task_id = %task_id, adapter = %adapter.id(), "adapter does not support containers; running on the host");
                adapter
            }
        },
        None => adapter,
    };
    // ADR-0072 D14（Phase E4b 項目3）: planner run（`extras.planner_permission_mode` が `Some`）は
    // `[execution.planner].permission_mode` を実際の CLI 引数として反映する。対応しないアダプタ
    // （`with_permission_mode` が `None` を返す）はアダプタ既定の permission-mode のまま走る
    // （E3 実装時の既定の動作と同じ。実害は無い）。
    let adapter = match &planner_permission_mode {
        Some(mode) if !mode.is_empty() => match adapter.with_permission_mode(mode) {
            Some(wrapped) => wrapped,
            None => {
                tracing::debug!(task_id = %task_id, adapter = %adapter.id(), %mode, "adapter does not support with_permission_mode; planner permission_mode was not applied (ADR-0072 D14)");
                adapter
            }
        },
        _ => adapter,
    };
    // ADR-0054 D1（Phase 67）: `run_worker` を通る run で継続セッションを持てるのは CoS の対話 run
    // だけ（部門長のレビュー run は `review.rs` の別経路。`run_extras` の `is_cos_conversation` と同じ
    // 判定で `extras.session` が埋まるので、ここでは `req.context.session` の有無だけを見ればよい）。
    let session_key = req.context.session.is_some().then(|| {
        (
            task_core::COS_ID.to_string(),
            task_core::SessionKind::Conversation,
            None,
        )
    });
    // ADR-0067 D3: `store` は `sink` に move されるので、後段の未申告成果物の登録用に控えておく。
    let store_for_undeclared_scan = Arc::clone(&store);
    let sink = StoreSink {
        store,
        task_id,
        run_id: run_id.to_string(),
        lease_ttl: lease.ttl,
        renew_every: lease.every,
        last_renew: std::sync::Mutex::new(Instant::now()),
        roles,
        genres,
        delegation,
        delegated_this_run: std::sync::atomic::AtomicUsize::new(0),
        account,
        account_book,
        session_key,
    };
    let outcome = if task_core::browser::requests_browser(&req.task.skills)
        && (remote.is_some() || container_plan.is_some())
    {
        Err(AdapterError::Other(
            "browser capability currently requires a local host run".into(),
        ))
    } else {
        task_worker::browser::run(adapter, req, run_id, limits, &sink).await
    };
    // ADR-0079 R5b-fix2: remote workspace は run が終わるたびに（成否に関わらず）手元の写しをクラスタへ
    // push する（review と次の run の prepare〈`--delete` 付きの pull〉の前）。push が落ちたら印が残り、
    // 次の pull は先に push をやり直すので手元の編集は消えない。
    let outcome = match &remote_ws {
        Some(ws) => push_remote_after_run(ws, &sink, outcome).await,
        None => outcome,
    };
    // ADR-0067 D3 / ADR-0074 D6.3（Phase F1 (j)）: run が成功したら未申告の成果物を登録する。
    // - git worktree ではない local の作業場所（`remote`/`worktree` どちらも無い）はリポジトリ全体
    //   （`artifacts_dir` の外を含む）から `*.md` を拾う（従来どおり。取りこぼし防止）。
    // - git worktree の Task（`worktree` が `Some`）は、そのリポジトリの全体を走査すると無関係な
    //   コードまで拾ってしまうので、その run の `artifacts_dir` の**中だけ**を、人が読む拡張子
    //   （md/html/pdf/csv/png）で走査する。
    if outcome.is_ok() && remote.is_none() {
        let existing_paths: std::collections::HashSet<String> = events
            .iter()
            .filter_map(|(_, ev)| match ev {
                Event::ArtifactProduced { artifact, .. } => Some(artifact.path.clone()),
                _ => None,
            })
            .collect();
        let found = if worktree.is_none() {
            crate::undeclared_artifacts::scan_undeclared_markdown_artifacts(
                &workspace_for_undeclared_scan,
                &artifacts_dir_for_undeclared_scan,
                &existing_paths,
            )
        } else {
            crate::undeclared_artifacts::scan_undeclared_artifacts_in_dir(
                &workspace_for_undeclared_scan,
                &artifacts_dir_for_undeclared_scan,
                &existing_paths,
            )
        };
        for artifact in found {
            let ev = Event::ArtifactProduced {
                run_id: run_id.to_string(),
                artifact,
            };
            if let Err(e) = store_for_undeclared_scan.append_event(task_id, &ev) {
                tracing::warn!(task_id = %task_id, error = %e, "failed to record an undeclared artifact (ADR-0067 D3 / ADR-0074 D6.3)");
            }
        }
    }
    outcome
}

/// ADR-0079 R5b-fix2: ワーカー run の後の push（1 run につき 1 回）。結果は進行（`WorkerProgress`）に
/// 1 行残す。push が落ちたら error 付きの進行を残し、run が成功していても失敗として返す（黙って
/// review に進まない。`Unreachable` は供給側失敗 = attempts を消費しない requeue）。run 自体が既に
/// 失敗していればその失敗をそのまま返す。
async fn push_remote_after_run(
    ws: &SshWorkspace,
    sink: &dyn task_worker::EventSink,
    outcome: Result<RunOutcome, AdapterError>,
) -> Result<RunOutcome, AdapterError> {
    if ws.settings().sync == SyncMode::None {
        return outcome;
    }
    let target = format!(
        "{}:{}",
        ws.settings().cluster,
        ws.effective_remote_dir().to_string_lossy()
    );
    match ws.push_after_run().await {
        Ok(()) => {
            sink.progress(&format!(
                "pushed the workspace to cluster {target} after the run"
            ));
            outcome
        }
        Err(e) => {
            let msg = format!(
                "push to cluster {target} after the run failed: {e} (local edits are kept; the next sync pushes before pulling)"
            );
            sink.progress_with(
                &msg,
                &task_core::ProgressFields {
                    error: true,
                    ..Default::default()
                },
            );
            tracing::warn!(cluster = %ws.settings().cluster, error = %e, "push after the run failed (R5b-fix2)");
            match outcome {
                Ok(_) => Err(workspace_error_to_adapter(e, "workspace push after run")),
                Err(original) => Err(original),
            }
        }
    }
}

/// ワーカー run 中のリース延長パラメータ（ADR-0010 D7）。
#[derive(Debug, Clone, Copy)]
struct LeaseRenewal {
    /// 延長後の ttl（`idle_timeout + lease_grace`）。
    ttl: Duration,
    /// 延長の最小間隔（`lease_grace / 2`）。
    every: Duration,
}

/// ADR-0018 D5: ワークスペースの失敗をアダプタの失敗に写す。`Unreachable`（ssh / rsync 自体の失敗）は
/// 供給側失敗（`Spawn`）にして、attempts を消費せず requeue されるようにする。
fn workspace_error_to_adapter(e: task_worker::WorkspaceError, context: &str) -> AdapterError {
    match e {
        task_worker::WorkspaceError::Unreachable(msg) => {
            AdapterError::Spawn(std::io::Error::other(format!("{context}: {msg}")))
        }
        other => AdapterError::Other(format!("{context}: {other}")),
    }
}

/// ADR-0059 D3: `mode` が省略された（明示的に `"worktree"` を選んでいない）Remote workspace か。
/// `Local` は関係ないので `false`。
fn remote_mode_omitted(workspace: &WorkspaceSpec) -> bool {
    matches!(workspace, WorkspaceSpec::Remote { mode: None, .. })
}

/// ADR-0059 D3: `Remote` workspace を `mode: Some(Shared)` に書き換えた複製（自動の格下げ）。
/// `Local` は関係ないので `None`。
fn downgraded_remote_workspace(workspace: &WorkspaceSpec) -> Option<WorkspaceSpec> {
    match workspace {
        WorkspaceSpec::Remote { cluster, path, .. } => Some(WorkspaceSpec::Remote {
            cluster: cluster.clone(),
            path: path.clone(),
            mode: Some(WorkspaceMode::Shared),
        }),
        WorkspaceSpec::Local { .. } => None,
    }
}

/// ADR-0061（Phase 104）: dispatch した時刻（`RunEntry`/`ReviewEntry` の `since`）から今までの
/// 壁時計時間をミリ秒で計算する。`since` が未来（時計のずれ等）なら 0 に丸める。
fn wall_ms_since(since: OffsetDateTime) -> u64 {
    (OffsetDateTime::now_utc() - since)
        .whole_milliseconds()
        .max(0) as u64
}

/// `ProviderThrottled.reason` に書く供給側失敗の種別（ADR-0013 D9）。供給側失敗でなければ `None`。
fn provider_failure_reason(e: &AdapterError) -> Option<&'static str> {
    match e {
        AdapterError::Throttled { .. } => Some("throttled"),
        AdapterError::AuthFailed(_) => Some("auth_failed"),
        AdapterError::Exhausted(_) => Some("exhausted"),
        AdapterError::Spawn(_) => Some("spawn"),
        AdapterError::Io(_) | AdapterError::Serde(_) | AdapterError::Other(_) => None,
    }
}

/// Reviewer run の供給側失敗（`ProviderOutcome` しか残っていない）の種別名（ADR-0013 D9）。
fn cooldown_reason_name(outcome: &ProviderOutcome) -> &'static str {
    match outcome {
        ProviderOutcome::Throttled { .. } => "throttled",
        ProviderOutcome::AuthFailed => "auth_failed",
        ProviderOutcome::Exhausted => "exhausted",
        ProviderOutcome::Ok => "ok",
    }
}

/// ADR-0024 D3: `AccountView.excluded_reason` の語彙（`docs/gui/api.md` §3.29）。
fn excluded_reason_name(reason: ExcludedReason) -> &'static str {
    match reason {
        ExcludedReason::NotLoggedIn => "not_logged_in",
        ExcludedReason::AtCapacity => "at_capacity",
        ExcludedReason::Cooldown => "cooldown",
        ExcludedReason::FiveHourExhausted => "five_hour_exhausted",
        ExcludedReason::SevenDayExhausted => "seven_day_exhausted",
        ExcludedReason::Rejected => "rejected",
    }
}

/// ADR-0024 D4/D5: `AccountCooldownView.reason` の語彙。
fn account_cooldown_reason_name(reason: AccountCooldownReason) -> &'static str {
    match reason {
        AccountCooldownReason::AuthFailed => "auth_failed",
        AccountCooldownReason::Throttled => "throttled",
        AccountCooldownReason::Exhausted => "exhausted",
    }
}

/// ADR-0024 D4: 供給側失敗の種別名（`provider_failure_reason` と同じ語彙）を `AccountCooldownReason` に写す。
fn account_cooldown_reason_from_failure(reason: &str) -> AccountCooldownReason {
    match reason {
        "auth_failed" => AccountCooldownReason::AuthFailed,
        "throttled" => AccountCooldownReason::Throttled,
        // "exhausted" | "spawn"
        _ => AccountCooldownReason::Exhausted,
    }
}

/// 供給側失敗（ADR-0010 D5）なら `ProviderPolicy::report` に渡す結果を返す。起動失敗（`Spawn`）も供給側として扱う。
/// `AdapterError`/`ProviderOutcome` は `task-dispatch`/`task-worker` の型なので、`task-ops` には移さない。
pub fn provider_failure_outcome(e: &AdapterError) -> Option<ProviderOutcome> {
    match e {
        AdapterError::Throttled { retry_after } => Some(ProviderOutcome::Throttled {
            retry_after: *retry_after,
        }),
        AdapterError::AuthFailed(_) => Some(ProviderOutcome::AuthFailed),
        AdapterError::Exhausted(_) | AdapterError::Spawn(_) => Some(ProviderOutcome::Exhausted),
        AdapterError::Io(_) | AdapterError::Serde(_) | AdapterError::Other(_) => None,
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
