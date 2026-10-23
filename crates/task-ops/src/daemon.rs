//! デーモンのメモリ上のスナップショット（ADR-0013 D4, `docs/api/v1/api.md` §3.20 / §6.2）。
//!
//! ディスパッチャ（task-dispatch）が tick の最後に作って `tokio::sync::watch` に送り、API（task-api）が読む。両者が依存する
//! この crate に型を置く（task-api は task-dispatch に依存しない）。真実ではなく観測値で、DB には書かず `replay` の対象外。
//! 時刻は RFC 3339 の文字列（`Instant` は作る側で壁時計に直す）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::TaskId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DaemonSnapshot {
    /// 起動ごとの ULID（`GET /health` の `instance_id` と同じ）。
    pub instance_id: String,
    pub pid: u32,
    pub hostname: String,
    pub started_at: String,
    pub last_tick_at: String,
    pub ticks: u64,
    pub tick_ms: u64,
    pub in_flight: Vec<InFlight>,
    pub cooldowns: Vec<CooldownView>,
    /// ADR 2026-10-11-task-wait-reasons D4: この tick に dispatcher のメモリだけで決まった見送りの理由（M1〜M9。
    /// 枠満杯・経路なし・クラスタ・write-set・バックオフ・disk critical・委譲した子待ち・人の承認待ち）。task_id 順、
    /// 1 task に 1 件。DB で決まる理由（`task_core::db_waits` / `plan_waits`）は入れない。
    #[serde(default)]
    pub task_waits: Vec<SnapshotTaskWait>,
    /// ADR 2026-10-23-stage-order-and-plan-stall D5: 生存確認（ADR-0079 D10）が理由なく止まっていると知らせ
    /// （`Event::StallDetected`）、その後まだ何も起きていない `ready` の task（task_id 順）。受信箱の attention
    /// （`AttentionItem::Stalled`）の元。何か起きて最後の event が変われば次の確認で消える。
    #[serde(default)]
    pub stalled_tasks: Vec<SnapshotStalledTask>,
    /// ADR-0033 D3（Phase 25）: 秘書レベルの未読の報告と通知の判定。**API が応答を組むときに埋める**
    /// 唯一のフィールド（`last_notified_at` は `POST /reports/notified` が進める API プロセスの観測値で、
    /// ディスパッチャは知らない）。ディスパッチャが送るスナップショットでは常に `None`。
    #[serde(default)]
    pub reports: Option<task_core::ReportsLive>,
    /// ADR-0033 D5（Phase 26）: 未決定の認可（`approvals.decision IS NULL`）の件数。`reports` と同じ理由で
    /// **API が応答を組むときに埋める**（ディスパッチャが送るスナップショットでは常に 0）。
    #[serde(default)]
    pub approvals_pending: u32,
    /// ADR-0079 D7（Phase R3a）: 未回答の決定の要求の件数（決定を出した節点が終端でないもの）。
    /// `approvals_pending` と同じく **API が応答を組むときに埋める**（ディスパッチャが送るスナップショットでは常に 0）。
    #[serde(default)]
    pub decisions_open: u32,
    /// ADR-0018: `[[clusters]]` の稼働状況（`id` 昇順）。第 2 段階で追加したので、古いスナップショットには無い。
    #[serde(default)]
    pub clusters: Vec<ClusterLive>,
    /// ADR-0024 D1/D5: `[accounts] claude_dir` の絶対パス（`[accounts]` が無ければ `None`）。
    /// ADR-0025 D6: claude-code の根の別名として残す（`accounts_roots["claude-code"]` と同じ値）。
    #[serde(default)]
    pub accounts_root: Option<String>,
    /// ADR-0024 D1: `[accounts] max_runs_per_account`（`[accounts]` が無ければ `None`）。
    #[serde(default)]
    pub max_runs_per_account: Option<usize>,
    /// ADR-0025 D1/D6: アダプタごとの根ディレクトリ（`"claude-code"` / `"codex"` → 絶対パス。設定されていない
    /// アダプタはキーごと無い）。古いスナップショットには無いので既定は空。
    #[serde(default)]
    pub accounts_roots: std::collections::HashMap<String, String>,
    /// ADR-0024/0025: プールのアカウント（`adapter` → `id` の順、id 昇順）。`[accounts]` が無ければ空。
    #[serde(default)]
    pub accounts: Vec<AccountLive>,
    /// ADR 2026-10-08 account-concurrency-limits §(e)（人の決定 implicit-account）: アカウントを持たない行
    /// （`account_pool` が off）の**LLM source ごとの暗黙アカウント**（`implicit/<llm source id>`）。
    /// 最初の行の設定順。`[accounts]` が無くても出る。古いスナップショットには無いので既定は空。
    #[serde(default)]
    pub implicit_accounts: Vec<ImplicitAccountLive>,
    /// ADR-0043 D3（Phase 56）: `[containers]` の設定と、起動時に調べたコンテナ runtime。
    /// 古いスナップショットには無いので既定は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub containers: Option<ContainersLive>,
    /// ADR-0075 D6（Phase G1）: scratch pool の観測値（`GET /api/v1/metrics/scratch` と `celerisctl scratch status --json`
    /// と同じ `celeris.scratch-status/1`）。古いスナップショットと scratch を持たない構成では `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratch: Option<ScratchStatus>,
}

/// `ScratchStatus.schema` の値。
pub const SCRATCH_STATUS_SCHEMA: &str = "celeris.scratch-status/1";

/// ADR-0075 D6（Phase G1）: scratch pool の状態（`celeris.scratch-status/1`）。**観測値**で DB には書かない。
/// 容量は byte。サイズは測定スレッドの値（古くてもよい）、未測定は同じ repo の最大値で推定する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScratchStatus {
    /// 常に `celeris.scratch-status/1`。
    pub schema: String,
    /// scratch が有効か（`[scratch] enabled = false`、または NFS 上で無効化したら `false`）。
    pub enabled: bool,
    /// 無効化した理由（NFS 上など）。
    pub disabled_reason: Option<String>,
    /// `[scratch] dir`。
    pub dir: String,
    /// この状態を組んだ時刻（RFC 3339）。
    pub observed_at: String,
    /// `dir` の filesystem の容量と空き（statvfs。読めなければ `None`）。
    pub fs_total_bytes: Option<u64>,
    pub fs_free_bytes: Option<u64>,
    /// `targets/` の推定使用量と、そのうち P0（絶対に消さない）の量。
    pub targets_bytes: u64,
    pub pinned_bytes: u64,
    pub targets_max_bytes: u64,
    pub total_max_bytes: u64,
    /// 実効上限 = min(total_max, filesystem から pool の外の使用量と `min_free_disk_mb` を除いた分)（D1）。
    pub effective_max_bytes: u64,
    pub high_watermark: f64,
    pub low_watermark: f64,
    /// `none` | `high_watermark` | `low_disk` | `emergency`。
    pub pressure: String,
    /// owner ごとの行（owner の文字列順）。
    pub owners: Vec<ScratchOwnerView>,
    /// pool の外の旧い target（`build_cache_dir/cargo/*`、`release-build/.cargo-target`）の残り。
    pub legacy: Vec<ScratchLegacyView>,
    /// 直近の GC（rename したものがあった回）。
    pub last_gc: Option<ScratchGcView>,
    /// 廃止。常に `None`（ADR-0129）: sccache は Celeris の外（host の cargo 設定）になった。型は旧 client との
    /// 互換のため残す。
    #[serde(default)]
    pub sccache: Option<ScratchSccacheView>,
    /// 廃止。常に `None`（ADR-0129）: cache server（`celeris cache-server`）を撤去した。型は旧 client との互換の
    /// ため残す。
    #[serde(default)]
    pub cache: Option<ScratchCacheView>,
}

/// 廃止（ADR-0129）: Celeris の階層 cache server（sccache の webdav backend 向け）は撤去した。
/// `ScratchStatus.cache` が旧 client 向けの schema 互換のため型だけ残す（値は常に `None`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScratchCacheView {
    /// `ready`（`/healthz` が応答）| `disabled`（`[scratch.cache_server] enabled = false`）| `unavailable`（応答なし）。
    pub state: String,
    /// `ready` でない理由。
    pub reason: Option<String>,
    /// `http://127.0.0.1:<port>`（`SCCACHE_WEBDAV_ENDPOINT`）。
    pub endpoint: String,
    /// sccache の server が起動時に選んだ backend（`celerisctl scratch env --server` が `<scratch>/bin/sccache-server.mode`
    /// に書く）: `webdav`（この cache server）| `disk`（G2 の local disk）。記録が無ければ `None`。
    pub sccache_mode: Option<String>,
    /// cache server の `/stats`（応答が無ければ `None`）。
    pub stats: Option<ScratchCacheStats>,
}

/// `ScratchCacheStats.schema` の値。
pub const SCRATCH_CACHE_STATS_SCHEMA: &str = "celeris.scratch-cache-stats/1";

/// 廃止（ADR-0129）: cache server の `/stats`（`celeris.scratch-cache-stats/1`）。型だけ残す（値は常に `None`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScratchCacheStats {
    /// 常に `celeris.scratch-cache-stats/1`。
    pub schema: String,
    pub started_at: String,
    pub observed_at: String,
    /// GET（HEAD を含み、`.sccache_check` を除く）と PUT の数。
    pub gets: u64,
    pub puts: u64,
    /// GET の結果: L1 hit / L2 hit / miss（`gets = l1_hits + l2_hits + misses`）。
    pub l1_hits: u64,
    pub l2_hits: u64,
    pub misses: u64,
    /// L2 hit を L1 へ書き戻した数。
    pub promotes: u64,
    /// L1（ローカル）。
    pub l1_dir: String,
    pub l1_bytes: u64,
    pub l1_entries: u64,
    pub l1_max_bytes: u64,
    /// L1 の上限で LRU に落とした数（未 flush の entry は落とさない）。
    pub l1_evicted: u64,
    /// L2（NFS。content-addressed な immutable `<k0k1>/<key>.zst`）。
    pub l2_enabled: bool,
    pub l2_dir: Option<String>,
    /// `ok` | `degraded`（連続失敗で切り離し中。GET は L1 だけで応答し、flusher は待つ）| `disabled`。
    pub l2_state: String,
    /// L2 の使用量と entry 数（直近の走査〈GC〉と以後の flush から。走査前は `None`）。
    pub l2_bytes: Option<u64>,
    pub l2_entries: Option<u64>,
    pub l2_scanned_at: Option<String>,
    pub l2_max_bytes: u64,
    /// L2 の I/O の失敗・GET のタイムアウト・checksum 不一致で捨てた entry の数。
    pub l2_errors: u64,
    pub l2_timeouts: u64,
    pub l2_corrupt: u64,
    pub l2_degraded_since: Option<String>,
    /// 切り離し中なら次に L2 を試す時刻。
    pub l2_retry_at: Option<String>,
    pub l2_last_error: Option<String>,
    /// 直近の L2 の GC（`l2_max_bytes` を超えた分を mtime の古い順に消す）。
    pub l2_gc_last_at: Option<String>,
    pub l2_gc_removed: u64,
    pub l2_gc_removed_bytes: u64,
    /// flusher（L1 → L2 の非同期 write-back）の待ち行列と遅延。
    pub flush_queue_len: u64,
    pub flush_queue_bytes: u64,
    /// 待ち行列の先頭（最古）の待ち時間（秒）。空なら `None`。
    pub flush_oldest_age_secs: Option<u64>,
    pub flush_last_at: Option<String>,
    pub flush_written: u64,
    pub flush_written_bytes: u64,
    /// L2 に既にあったので書かなかった数。
    pub flush_skipped_existing: u64,
    /// 待ち行列の上限で「L2 に書かない」で落とした数。
    pub flush_dropped: u64,
    /// flusher の帯域の上限（MB/s、0 = 無制限）。
    pub flush_mbps: u64,
}

/// 廃止（ADR-0129）: sccache L1（`<scratch>/sccache-l1`）の配線は撤去した。`ScratchStatus.sccache` が旧 client
/// 向けの schema 互換のため型だけ残す（値は常に `None`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScratchSccacheView {
    /// `ready`（run に `RUSTC_WRAPPER` を与える）| `disabled`（設定で無効）| `unavailable`（バイナリか server が無い）。
    pub state: String,
    /// `ready` でない理由。
    pub reason: Option<String>,
    /// 本物の sccache（`[scratch.sccache] binary`）。
    pub binary: String,
    /// `SCCACHE_SERVER_PORT`。
    pub port: u16,
    /// `SCCACHE_DIR`。
    pub dir: String,
    /// `SCCACHE_CACHE_SIZE`（byte）。
    pub max_bytes: u64,
    /// `sccache --show-stats` の要約（`celerisctl scratch status` が server に問い合わせたときだけ。daemon の
    /// スナップショットでは `None`〈tick で client を起こさない〉）。
    pub stats: Option<ScratchSccacheStats>,
}

/// 廃止（ADR-0129）: cache server の `/stats`。型だけ残す（値は常に `None`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScratchSccacheStats {
    pub compile_requests: u64,
    pub hits: u64,
    pub misses: u64,
    /// Rust だけの hit / miss（owner をまたいだ依存の hit を見る。U1）。
    pub rust_hits: u64,
    pub rust_misses: u64,
    /// L1 の使用量（byte。読めなければ `None`）。
    pub cache_size_bytes: Option<u64>,
}

/// scratch pool の owner 1 つ（`targets/<owner>/`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScratchOwnerView {
    /// `task-<id>` / `task-<id>/wu-<id>` / `release-<sha12>` / `agent-<name>`（owner として読めない野良はパス）。
    pub owner: String,
    /// `task` | `work_unit` | `release` | `agent` | `stray`。
    pub kind: String,
    /// `p0` | `p1` | `p2` | `p3` | `seed` | `stray`。
    pub class: String,
    /// 分類の理由（`task running`、`lease expired` など）。
    pub reason: String,
    /// `target/` があるか（GC が刈った後は lease だけが残る）。
    pub has_target: bool,
    /// 測定したサイズ（未測定は `None`）と、GC が使う推定値。
    pub size_bytes: Option<u64>,
    pub estimated_bytes: u64,
    pub measured_at: Option<String>,
    /// `lease.json` の mtime（生存の合図）。
    pub lease_mtime: Option<String>,
    pub repo_key: Option<String>,
    /// base commit の先頭 12 桁。
    pub base_commit: Option<String>,
    /// adopt で引き継いだ元の owner。
    pub adopted_from: Option<String>,
    pub work_unit_key: Option<String>,
}

/// pool の外の旧い target 1 つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScratchLegacyView {
    pub path: String,
    /// `legacy`（1 時間以上更新が無く回収できる）| `p0`（未測定・1 時間以内に更新あり）。
    pub class: String,
    pub size_bytes: Option<u64>,
    pub last_write: Option<String>,
}

/// 直近の GC 1 回。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScratchGcView {
    pub at: String,
    /// その回の `pressure`。
    pub pressure: String,
    /// 空き < `min_free_disk_mb` の緊急 GC か。
    pub emergency: bool,
    pub removed: Vec<ScratchGcRemovedView>,
    pub reclaimed_bytes: u64,
}

/// GC が rename した 1 つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScratchGcRemovedView {
    pub id: String,
    /// `legacy` | `stray` | `p1` | `p2` | `p3` | `seed`。
    pub class: String,
    pub estimated_bytes: u64,
    /// `immediate`（watermark に関係なく回収）| `pressure`（目標に届くまで）。
    pub why: String,
}

/// ADR-0043 D3（Phase 56）: コンテナ実行の設定と起動時の検出（**観測値**。DB には書かない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContainersLive {
    /// `[containers] runtime`（`"auto"` | `"podman"` | `"docker"`）。
    pub preference: String,
    /// 実際に使う runtime（`"podman"` | `"docker"`）。どれも使えなければ `None`
    /// （`run = container` のタスクは dispatch されず `blocked` になる）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// 試した runtime ごとの `<runtime> info` の結果（試した順）。
    #[serde(default)]
    pub probes: Vec<ContainerProbeView>,
    /// `[containers] image_default`（`[container] image` も `dockerfile` も無いときのイメージ）。
    pub image_default: String,
    /// `[containers] build_dir`（Dockerfile からビルドしたイメージの作業場所。絶対パス）。
    pub build_dir: String,
}

/// `<runtime> info` の結果 1 件（`detail` は成功なら `"ok"`、失敗なら理由の 1 行）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContainerProbeView {
    pub runtime: String,
    pub detail: String,
}

/// ADR 2026-10-08 account-concurrency-limits §(e): 暗黙アカウント 1 件の観測値。単位は LLM source で、
/// 同じ source を使う非プール行は 1 件を共有する。上限の保存値は DB（`account_limits`）が正本で、
/// config の値は上限に使わない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ImplicitAccountLive {
    /// 解決済み LLM source の id（= 暗黙アカウントの id。`celeris` / `openai_compatible:<id>` 等）。
    pub source: String,
    /// この source を選択する route の model id（設定順）。
    #[serde(default)]
    pub models: Vec<String>,
    /// これらの行で走っている全 run（非 CoS・CoS・プロバイダを使うレビュー run）の数。
    pub in_use: u32,
}

/// ADR-0024/0025: プールの 1 アカウントの稼働状況（観測値。DB には書かない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AccountLive {
    /// ADR-0025 D1: `"claude-code"` | `"codex"`。古いスナップショットには無いので既定は `"claude-code"`。
    #[serde(default = "default_account_adapter")]
    pub adapter: String,
    pub id: String,
    /// ログイン済みを示すファイル（claude-code は `.credentials.json`、codex は `auth.json`）の有無。
    pub logged_in: bool,
    /// 実行中の run（ワーカー run + このアカウントを使う Reviewer run）の数。
    pub in_use: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<AccountUsageLive>,
    /// ADR-0024 D3 のスコア。除外されていれば `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// `"not_logged_in" | "at_capacity" | "cooldown" | "five_hour_exhausted" | "seven_day_exhausted" | "one_month_exhausted" | "rejected"`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown: Option<AccountCooldownLive>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<AccountCheckView>,
    /// 進行中のログイン中継（ADR-0024 D7）があるか。
    #[serde(default)]
    pub login_pending: bool,
}

/// Result of a credential check retained in the in-memory account view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AccountCheckView {
    pub at: String,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `RateLimitObservation` の観測値部分（Unix 秒のまま。壁時計の文字列化は task-api が行う）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AccountUsageLive {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<task_core::RateWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<task_core::RateWindow>,
    /// 1 か月窓（opencode go。ADR 2026-10-06 D1）。観測できなければ `None`（『不明』）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_month: Option<task_core::RateWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub observed_at: i64,
    /// `"run" | "check"`。
    pub source: String,
}

fn default_account_adapter() -> String {
    "claude-code".to_string()
}

/// アカウントの cooldown（Unix 秒）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AccountCooldownLive {
    pub until: i64,
    /// `"auth_failed" | "throttled" | "exhausted"`。
    pub reason: String,
}

/// ADR 2026-10-11-task-wait-reasons D4: dispatcher が見送った task と、その理由（メモリの理由）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotTaskWait {
    pub task_id: TaskId,
    pub wait: task_core::TaskWait,
}

/// ADR 2026-10-23-stage-order-and-plan-stall D5: `DaemonSnapshot.stalled_tasks` の 1 件（`StallDetected` の写し）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotStalledTask {
    pub task_id: TaskId,
    /// `StallDetected.reason`（`plan_stalled`・`nothing_runnable` など）。
    pub reason: String,
    pub detail: String,
    /// 止まっていると最初に見た時刻（RFC 3339）。
    pub since: String,
    /// `StallDetected` を記録した時刻（RFC 3339）。
    pub at: String,
}

/// 実行中の run（ワーカー run、またはプロバイダを使う Reviewer run）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InFlight {
    pub task_id: TaskId,
    pub run_id: String,
    pub provider: String,
    pub kind: InFlightKind,
    pub since: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InFlightKind {
    Worker,
    Reviewer,
}

/// `task_dispatch::policy::Cooldown`（`Instant`）を壁時計に直したもの。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CooldownView {
    pub source: String,
    pub account_id: String,
    pub until: String,
    /// `"throttled" | "auth_failed" | "exhausted"`。
    pub reason: String,
}

/// クラスタ（`[[clusters]]` の行）の稼働状況（ADR-0018 D2 / D5）。`env` の値・`setup` の中身は含めない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClusterLive {
    pub id: String,
    /// `~/.ssh/config` の `Host` 名。
    pub host: String,
    pub concurrency: usize,
    /// このクラスタで走っている run（ワーカー run + 判定）の数。
    pub in_use: u32,
    /// この tick で `ssh -O check` が成功した（人が張った多重接続がある）。
    pub connected: bool,
    /// 多重接続が無くて cooldown 中なら、その終わり（RFC 3339）。
    pub cooldown_until: Option<String>,
    /// ADR-0032 D1: `"manual"` / `"publickey"` / `"totp"`。古いスナップショットには無いので既定は `"manual"`
    /// （`celeris::config::ClusterConfig.auth` と同じ既定）。
    #[serde(default = "default_cluster_live_auth")]
    pub auth: String,
    /// ADR-0032 D4: GUI 発の接続（`POST /clusters/{id}/connect`）が進行中か。古いスナップショットには無いので既定は `false`。
    #[serde(default)]
    pub connect_pending: bool,
    /// ADR-0053 D3（Phase 66）: 鍵認証を試しても ssh master が繋がらず、人の TOTP 入力が要る状態か。
    /// 古いスナップショットには無いので既定は `false`。
    #[serde(default)]
    pub tunnel_login_needed: bool,
    /// ADR-0053 D3: このクラスタの port forward（`[[clusters]].forwards`）の生存。無ければ空
    /// （forward を持たないクラスタ、または古いスナップショット）。
    #[serde(default)]
    pub tunnel_forwards: Vec<TunnelForwardLive>,
    /// ADR-0078 D5: この daemon の起動以降の接続・切断の回数。古いスナップショットには無いので既定は 0。
    #[serde(default)]
    pub connection_stats: task_core::ClusterConnectionStats,
}

fn default_cluster_live_auth() -> String {
    "manual".to_string()
}

/// ADR-0053 D3（Phase 66）: 1 本の port forward の生存（`GET /clusters` にそのまま出す）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TunnelForwardLive {
    pub listen: String,
    pub target: String,
    /// forward 越しに `GET <listen>/v1/models` が届くか（直近の観測。`listener && target_healthy`）。
    pub up: bool,
    /// ADR-0053 Phase 85: 手元のリスナー（`-O forward`/`ssh -N -L`）が有るか。古いスナップショットには
    /// 無いので既定は `false`。
    #[serde(default)]
    pub listener: bool,
    /// ADR-0053 Phase 85: listener 越しに target（`/v1/models`）が健全か。古いスナップショットには
    /// 無いので既定は `false`。
    #[serde(default)]
    pub target_healthy: bool,
    /// ADR-0053 Phase 85: 直近の失敗理由（無ければ `null`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}
