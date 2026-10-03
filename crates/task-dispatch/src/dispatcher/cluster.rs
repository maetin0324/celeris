//! cluster / tunnel の接続・生存確認・probe（ADR-0018、ADR-0023、ADR-0053 D3、ADR-0066）。ADR-0082 の L1。
//!
//! cluster の hook は `run_cluster_hooks_off_async` で包んで呼ぶ（tick の中で直接ブロックしない）。

use super::*;

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
    /// ADR-0090 D7: クラスタ job の durable wait の poll 間隔と上限（`[[clusters]] job_wait`）。
    pub job_wait: task_core::cluster_job::ClusterJobWaitLimits,
    /// ADR-0062 A: master 越しの実通信（`ssh -o BatchMode=yes <host> -- true`）による生存確認を
    /// この秒数ごとに行う（`0` で無効）。既定 300 秒。NAT / ファイアウォールの idle timeout で
    /// TCP が黙って死んでも、`-O check`（unix socket を見るだけ）は気づかないので、実通信で確定させる。
    pub liveness_probe_secs: u64,
}

/// ADR-0062 B1（Phase 107）: `resolve_cluster` の結果。`cluster_of`（従来の `Option` 契約）はこれを
/// 畳んだだけだが、`dispatch_ready` は `NotConfigured`（設定にクラスタが無い）と
/// `AssigneeLacksTool`（担当が `cluster:<id>` を持たない）を区別して扱う（前者は従来どおり
/// `unroutable` の警告、後者は `blocked` にして人に聞く）。
pub(super) enum ClusterResolution {
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
/// 本番でこれが毎 tick 起きて tick が 6 秒に伸びた。`agent-docs/adr/0053-llm-source-proxy.md`「Phase 85 追記」）。
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
pub(super) fn next_key_auth_backoff(current: Duration) -> Duration {
    if current.is_zero() {
        return KEY_AUTH_BACKOFF_MIN;
    }
    current.saturating_mul(2).min(KEY_AUTH_BACKOFF_MAX)
}

/// ADR-0078 D4/D5: `cluster_connected` を書き換えたきっかけ（遷移の `method` / `cause` を決める）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClusterConnChange {
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
pub(super) struct ClusterConnState {
    /// 最後に繋がった時刻（切れたら `None`）。`uptime_secs` に使う。
    pub(super) connected_since: Option<Instant>,
    /// D3-3: 実通信 probe の連続失敗の回数。
    pub(super) probe_failures: u32,
    /// D3-3: probe の連続失敗で「死んだ」と確定した。`-O check` が通っても probe が 1 回成功するまで
    /// 接続中に戻さない。
    pub(super) probe_dead: bool,
    /// D3-4: 別スレッドで走っている実通信 probe の結果の受け口（tick は待たない）。
    pub(super) probe_inflight: Option<std::sync::mpsc::Receiver<bool>>,
    /// D3-1: totp / manual のクラスタで、この切断について鍵認証の再接続を試したか。
    pub(super) key_auth_tried: bool,
    /// D3-2: publickey の現在のバックオフと、次に試してよい時刻。
    pub(super) key_auth_backoff: Duration,
    pub(super) next_key_auth_at: Option<Instant>,
    /// D4: publickey の鍵認証の再接続が続けて失敗した回数と、この切断で報告したか。
    pub(super) key_auth_failures: u32,
    pub(super) unavailable_reported: bool,
    /// GUI 発の接続（`POST /clusters/{id}/connect`）が終わった直後か（次の false→true の `method` を
    /// `borrowed` ではなく `auth` の値にする）。
    pub(super) gui_connect_finished: bool,
    /// D4: 最後の切断の説明（通知の本文に使う）。
    pub(super) last_lost_detail: Option<String>,
    /// D5: この daemon の起動以降の回数。
    pub(super) stats: task_core::ClusterConnectionStats,
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
pub(super) struct ClusterDisconnectInfo {
    pub(super) exit_code: Option<i32>,
    pub(super) detail: String,
    pub(super) reported: bool,
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
pub(super) struct ForwardObservation {
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
pub(super) fn next_probe_interval_secs(current_secs: u64, min_secs: u64, healthy: bool) -> u64 {
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
pub(super) struct TargetProbeState {
    healthy: bool,
    /// `healthy == false` のときの理由（[`TunnelProbe`] の `Err`）。
    error: Option<String>,
    checked_at: Instant,
    /// 次に probe するまでの間隔（バックオフ済み）。
    interval_secs: u64,
}

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
pub(super) const CLUSTER_LIVENESS_INTERVAL: Duration = Duration::from_secs(5);

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
pub(super) fn run_cluster_hooks_off_async<F, T>(f: F) -> T
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

impl Dispatcher {
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

    /// 試験の継ぎ目: remote workspace の worker run・判定が使う ssh / rsync のコマンドを差し替える
    /// （偽の `ssh_command` で、外部ネットワークに出ずに remote 経路を通す）。本番の配線は呼ばない。
    pub fn set_cluster_ssh_command_override(&mut self, command: Vec<String>) {
        self.cluster_ssh_command_override = Some(command);
    }

    /// `ClusterSpec::ssh_settings` に `set_cluster_ssh_command_override` の差し替えを当てたもの。
    pub(super) fn remote_ssh_settings(
        &self,
        spec: &ClusterSpec,
        remote_path: &std::path::Path,
        task_id: task_core::TaskId,
        mode: task_core::WorkspaceMode,
    ) -> SshSettings {
        let mut settings = spec.ssh_settings(remote_path, task_id, mode);
        if let Some(command) = &self.cluster_ssh_command_override {
            settings.ssh_command = command.clone();
            settings.rsync_command = command.clone();
        }
        settings
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

    /// ADR-0018 D2 / ADR-0032 D3: 多重接続が無い（または自動接続を試みて失敗した）クラスタを cooldown にし、
    /// 理由をタスクのイベントに残す（人にログインを促すため）。`reason` は呼び出し側が組み立てる
    /// （自動接続を試みて失敗した場合は `"auto-connect failed: ..."` を含め、従来の「接続が無い」だけの文言と区別する）。
    pub(super) fn mark_cluster_unavailable(
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
    pub(super) fn try_auto_connect_cluster(
        &self,
        spec: &ClusterSpec,
    ) -> Option<Result<(), String>> {
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
    pub(super) fn refresh_cluster_liveness(&mut self) {
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
    pub(super) fn take_cluster_probe_result(&mut self, id: &str) -> Option<bool> {
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
    pub(super) fn set_cluster_connected(
        &mut self,
        id: &str,
        alive: bool,
        change: ClusterConnChange,
    ) {
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
    pub(super) fn key_auth_allowed(&self, spec: &ClusterSpec) -> bool {
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
    pub(super) fn note_key_auth_attempt(&mut self, spec: &ClusterSpec, ok: bool) {
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
    pub(super) fn maybe_report_publickey_outage(&mut self, spec: &ClusterSpec) {
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
    pub(super) fn refresh_cluster_master_exits(&mut self) {
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
    pub(super) fn refresh_cluster_tunnels(&mut self) {
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
    pub(super) fn ensure_cluster_master_for_tunnel(&mut self, spec: &ClusterSpec) -> bool {
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

    pub(super) fn clear_login_needed(&mut self, cluster_id: &str) {
        if self.cluster_login_needed.remove(cluster_id) {
            tracing::info!(cluster = %cluster_id, "tunnel: login is no longer needed");
        }
    }

    /// 初めて「ログインが要る」状態に入ったときだけ、イベントを積み報告を 1 件記録する（重複排除）。
    pub(super) fn mark_login_needed(&mut self, spec: &ClusterSpec) {
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
    pub(super) fn refresh_one_forward(&mut self, spec: &ClusterSpec, fwd: &ClusterForwardSpec) {
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
    pub(super) fn ensure_tunnel_prober_started(&mut self) {
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
    pub(super) fn probe_listener(&self, listen: &str) -> bool {
        self.tunnel_listener_probe
            .as_ref()
            .map(|p| p(listen))
            .unwrap_or(false)
    }

    /// forward の target（先方）が健全か（`/v1/models` が応答するか）。`refresh_one_forward` の
    /// 初回の種蒔きと、専用スレッドの両方から呼ぶ。
    pub(super) fn probe_forward(&self, listen: &str) -> Result<(), String> {
        self.tunnel_probe
            .as_ref()
            .map(|p| p(listen))
            .unwrap_or_else(|| Err("no tunnel probe is configured".to_string()))
    }

    /// 状態遷移を記録する（`tunnel_state` を更新し、フェーズ（Up/Down/TargetUnreachable）が変わったときだけ
    /// `tunnel_events` に積む。Phase 85: 同じフェーズが続く間は 1 行も出さない＝スパムしない）。
    pub(super) fn observe_tunnel(
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

    pub(super) fn push_tunnel_event(
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
    pub(super) fn cluster_of(
        &mut self,
        task: &Task,
    ) -> Option<(ClusterSpec, PathBuf, WorkspaceMode)> {
        match self.resolve_cluster(task) {
            ClusterResolution::Resolved(spec, path, mode) => Some((*spec, path, mode)),
            ClusterResolution::Local
            | ClusterResolution::NotConfigured
            | ClusterResolution::AssigneeLacksTool { .. } => None,
        }
    }

    /// `cluster_of` の中身。`None` の理由を dispatch_ready が区別できるようにする（ADR-0062 B1）。
    pub(super) fn resolve_cluster(&mut self, task: &Task) -> ClusterResolution {
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
    pub(super) fn task_may_use_cluster(&mut self, task: &Task, cluster: &str) -> bool {
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
    pub(super) fn cluster_in_use(&self, cluster: &str) -> usize {
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
}
