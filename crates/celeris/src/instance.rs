//! ADR-0040 D4（Phase 47）: celeris の「インスタンスの役割」とライブ引き継ぎ。
//!
//! 昇格（新しいリリースへの切り替え）は、新しいプロセスを**同じ DB・同じポート**に対して起こし、
//! 古いプロセスに `handoff_requested_at` を書いて `draining` にし、新しいプロセスが `active` を
//! 引き継ぐことで行う。判断はすべて tick の中で決定的に行い、LLM もワーカーも関与しない
//! （DESIGN 原則 1〜4。ここから `task-worker` は見えない）。
//!
//! 規則（ADR-0040 D4 そのまま）:
//!
//! - 起動時: `--mode verify` なら役割 `verify`（`daemon_instances` に**行を書かない**）。
//!   そうでなければ、**生きている** `active`（`drained_at` が無く、プロセスが生きている行。heartbeat
//!   の新旧は問わない）が居て **`release` が同じ**なら二重起動なので exit 3。
//!   生きている `active` が居れば `standby` になり、**生きている全ての `active`** の行に
//!   `handoff_requested_at` を書く。居なければ `active` になる（同じ transaction で確かめて書く）。
//! - 毎 tick: 自分の行に heartbeat を打つ。`active` は `handoff_requested_at` を見たら**同じ tick で**
//!   `draining` へ（listener を閉じ、dispatch と裏方を止める。手元の run は面倒を見続ける）。
//!   `standby` は、生きている他の `active` が 1 つも無くなったら `active` へ（判断と書き込みを 1 つの
//!   transaction で行う。ADR-0040 D4 付記 2026-10-05）。
//! - `active` が 2 つ以上生きていたら（規則に反する痕）、新しい方を残し古い方へ引き継ぎを要求する。
//! - 手元の run が 0 になったら `drained_at` を書いて exit 0。`[handoff] drain_timeout_secs` を
//!   超えたら残りを abort して exit 0。
//! - 他のインスタンスの行は、`drained_at` が付くか、heartbeat が古く**かつ**（active でない か プロセスが死んでいる）
//!   なら消す。古くても生きている `active` の行は消さない（まだ手放していないので、消すと二重起動になる）。

use std::sync::Arc;
use std::time::Duration;

use task_core::{DaemonInstance, InstanceRole, SharedRole, StoreError, TaskStore};
use time::OffsetDateTime;

/// `release` を書いていないときの既定（作業チェックアウトから直接起こした場合）。
pub const DEV_RELEASE: &str = "dev";
/// `--release` も無いときに見る環境変数（systemd の unit が渡す）。
pub const RELEASE_ENV: &str = "CELERIS_RELEASE";

/// このプロセスの身元（ADR-0040 D4）。`instance_id` は API の `GET /health` に出るものと同じ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceIdentity {
    pub instance_id: String,
    pub release: String,
    pub pid: u32,
}

impl InstanceIdentity {
    /// `release` は `--release <sha12>` > 環境変数 `CELERIS_RELEASE` > `"dev"` の順。
    pub fn new(cli_release: Option<&str>) -> Self {
        Self {
            instance_id: ulid::Ulid::new().to_string(),
            release: resolve_release(cli_release),
            pid: std::process::id(),
        }
    }
}

/// `--release` > `CELERIS_RELEASE` > `"dev"`（どちらも空文字は「無い」扱い）。
pub fn resolve_release(cli_release: Option<&str>) -> String {
    cli_release
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var(RELEASE_ENV)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| DEV_RELEASE.to_string())
}

/// heartbeat が「新しい」とみなす窓（ADR-0040 D4: `3 × tick + lease_grace`）。
pub fn freshness_window(tick: Duration, lease_grace_secs: u64) -> Duration {
    tick.saturating_mul(3) + Duration::from_secs(lease_grace_secs)
}

/// そのプロセスがまだ生きているか（同一ホスト前提。ADR-0040 D4「同一ホスト・同一 SQLite」）。
/// 判定できない環境では `true`（＝生きている）を返す。**保守的な側**（二重起動を疑う側）に倒す。
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return true;
    }
    match std::fs::metadata(format!("/proc/{pid}")) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        // `/proc` が無い（Linux 以外）等。判定できないので生きている扱い。
        Err(_) => true,
    }
}

/// 生きている `active` か（ADR-0040 D4 付記 2026-10-05）: 役割が `active` で `drained_at` が無く、
/// プロセスがまだ生きている行。heartbeat が古くても、プロセスが生きていれば手放すまで active とみなす
/// （古いと見て引き継ぐと、生きたままの旧 instance と二重に dispatch する）。
pub fn is_live_active(row: &DaemonInstance, alive: &dyn Fn(u32) -> bool) -> bool {
    row.role == InstanceRole::Active && row.drained_at.is_none() && alive(row.pid)
}

/// `self_id` 以外に生きている `active` が 1 つも無いか（active になってよい条件）。store の
/// `instance_register_if` / `instance_set_role_if` の `admit` に渡す。
pub fn no_other_live_active(
    rows: &[DaemonInstance],
    self_id: &str,
    alive: &dyn Fn(u32) -> bool,
) -> bool {
    !rows
        .iter()
        .any(|r| r.instance_id != self_id && is_live_active(r, alive))
}

/// 起動時の判断（純粋な関数。DB には触れない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupDecision {
    /// 生きている `active` がいない。自分が `active` になる（書き込みの直前にもう一度確かめる）。
    Active,
    /// 生きている `active` がいる。`standby` になり、それら全部に引き継ぎを要求する。
    Standby { active_instance_ids: Vec<String> },
    /// 同じ `release` の生きている `active` が既にいる。何もせず exit 3（同じ版を二重に起こさない）。
    DuplicateRelease { instance_id: String, pid: u32 },
}

/// ADR-0040 D4 の起動時の規則。`rows` は `daemon_instances` の全行。
///
/// `alive` は「その pid のプロセスがまだ生きているか」（本番は `pid_alive`）。heartbeat が新しくても
/// プロセスが消えていれば二重起動ではない — `SIGKILL` で落ちた直後に同じ版を起こし直すのは普通の
/// 復旧手順なので、そこで exit 3 を返すと復旧できなくなる（本 Phase での ADR-0040 D4 の細則）。
pub fn decide_startup(
    rows: &[DaemonInstance],
    release: &str,
    alive: &dyn Fn(u32) -> bool,
) -> StartupDecision {
    let live: Vec<&DaemonInstance> = rows.iter().filter(|r| is_live_active(r, alive)).collect();
    // 同じ版が動いていれば、それが誰であっても二重起動（`started_at` が古い方を代表に選ぶ）。
    if let Some(same) = live.iter().find(|r| r.release == release) {
        return StartupDecision::DuplicateRelease {
            instance_id: same.instance_id.clone(),
            pid: same.pid,
        };
    }
    if live.is_empty() {
        StartupDecision::Active
    } else {
        StartupDecision::Standby {
            active_instance_ids: live.iter().map(|r| r.instance_id.clone()).collect(),
        }
    }
}

/// 1 行を掃除してよいか（ADR-0040 D4）。`drained_at` が付いた行は常に消す（Phase 119 D4 の監視は
/// `Supervisor::step` の中で別に行う）。heartbeat が古い行は、active でないか、プロセスが死んでいれば消す。
/// **古くても生きている `active` の行は消さない**（まだ手放していないので、消すと見えなくなり二重起動になる）。
pub fn removable_row(
    row: &DaemonInstance,
    now: OffsetDateTime,
    freshness: Duration,
    alive: &dyn Fn(u32) -> bool,
) -> bool {
    row.drained_at.is_some()
        || (!row.is_fresh(now, freshness) && (row.role != InstanceRole::Active || !alive(row.pid)))
}

/// ADR-0040 付記（2026-10-02）: `promoting.json` の印が昇格を認可する期限（秒。固定。設定にしない）。
pub const PROMOTING_MAX_AGE_SECS: i64 = 900;
/// ADR-0040 付記: selfdeploy のスクリプトが昇格の間だけ `<releases_dir>/<sha12>/` に置く印。
pub const PROMOTING_FILE: &str = "promoting.json";
/// ADR-0040 付記の規則 3: 旧版の `start_promote_with_launcher` が `<releases_dir>/<sha12>/` に
/// 昇格プロセスの pid を書く lock（`releases.rs` の `lock_pid` と同じ書式: pid の 10 進数・前後空白可）。
pub const PROMOTE_LOCK_FILE: &str = "promote.lock";

/// `promote.lock` から読んだ事実（規則 3）。生死と更新時刻は読んだ時点のもの。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromoteLockFact {
    pub pid: u32,
    /// `pid_alive(pid)` の結果（pid 0 は判定せず `false`）。
    pub alive: bool,
    /// lock の最終更新時刻（mtime）。
    pub modified: OffsetDateTime,
}

/// `promoting.json` の中身のうち判定に使う欄（他の欄 `script` / `mode` / `pid` は読まない）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PromotingMarker {
    pub sha12: String,
    pub started_at: String,
}

/// 昇格の認可の判定に使う、ファイルシステムから読んだ事実（`read_promotion_evidence` が作る）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionEvidence {
    /// `<releases_dir>/<release>` が（ディレクトリでも symlink でも）あるか。
    pub release_managed: bool,
    /// `<releases_dir の親>/current` の `readlink`（無い・symlink でなければ `None`）。
    pub current_target: Option<std::path::PathBuf>,
    /// `<releases_dir>/<release>/promoting.json`: 無ければ `None`、読めない・壊れていれば `Some(Err)`。
    pub promoting: Option<Result<PromotingMarker, String>>,
    /// `<releases_dir>/<release>/promote.lock`: 無ければ `None`、読めない・pid でなければ `Some(Err)`。
    pub promote_lock: Option<Result<PromoteLockFact, String>>,
}

/// 昇格の認可の判定の結果（ADR-0040 付記）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionGate {
    /// 判定の対象外（`dev`、または `releases_dir` に無い名前）。従来どおり起動する。
    Skipped(String),
    /// 昇格済み（`current` 一致）または昇格中（新しい印）。DB を開いて起動してよい。
    Authorized(String),
    /// 認可が無い。DB を開かず exit 4 で終わる。
    Rejected(String),
}

/// ADR-0040 付記の判定（純粋な関数。ファイルシステムも DB も見ない）。
pub fn decide_promotion(
    release: &str,
    evidence: &PromotionEvidence,
    now: OffsetDateTime,
) -> PromotionGate {
    if release == DEV_RELEASE {
        return PromotionGate::Skipped("release is `dev`".into());
    }
    if !evidence.release_managed {
        return PromotionGate::Skipped(format!(
            "release `{release}` is not under releases_dir (unmanaged)"
        ));
    }
    // 規則 1: `current` のリンク先の最後の要素を名前で比べる（canonicalize しない）。
    let current_name = evidence
        .current_target
        .as_deref()
        .and_then(|t| t.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    if current_name.as_deref() == Some(release) {
        return PromotionGate::Authorized(format!("`current` points to `{release}`"));
    }
    let current_desc = match &evidence.current_target {
        Some(t) => format!("`current` -> `{}`", t.display()),
        None => "`current` does not exist".to_string(),
    };
    // 規則 2: 新しい `promoting.json` で `sha12` が一致する。
    let marker_desc = match &evidence.promoting {
        None => format!("no {PROMOTING_FILE}"),
        Some(Err(e)) => format!("{PROMOTING_FILE} is unreadable: {e}"),
        Some(Ok(marker)) if marker.sha12 != release => format!(
            "{PROMOTING_FILE} is for `{}`, not `{release}`",
            marker.sha12
        ),
        Some(Ok(marker)) => {
            match OffsetDateTime::parse(
                &marker.started_at,
                &time::format_description::well_known::Rfc3339,
            ) {
                Err(e) => format!(
                    "{PROMOTING_FILE} has an invalid started_at `{}`: {e}",
                    marker.started_at
                ),
                Ok(started) => {
                    let age = (now - started).whole_seconds();
                    // 時計のずれで未来の時刻になった印も、同じ幅の外なら認めない（期限が効かなくなるため）。
                    if age.abs() <= PROMOTING_MAX_AGE_SECS {
                        return PromotionGate::Authorized(format!(
                            "{PROMOTING_FILE} for `{release}` started {age}s ago"
                        ));
                    }
                    format!(
                        "{PROMOTING_FILE} started_at `{}` is {age}s old (limit {PROMOTING_MAX_AGE_SECS}s)",
                        marker.started_at
                    )
                }
            }
        }
    };
    // 規則 3: 旧版の GUI/API が書いた `promote.lock` の pid が生きていて、lock が新しい。
    let lock_desc = match &evidence.promote_lock {
        None => format!("no {PROMOTE_LOCK_FILE}"),
        Some(Err(e)) => format!("{PROMOTE_LOCK_FILE} is unreadable: {e}"),
        // `pid_alive(0)` は true を返すので、生死より先に弾く。
        Some(Ok(lock)) if lock.pid == 0 => format!("{PROMOTE_LOCK_FILE} has an invalid pid 0"),
        Some(Ok(lock)) if !lock.alive => {
            format!("{PROMOTE_LOCK_FILE} pid {} is not running", lock.pid)
        }
        Some(Ok(lock)) => {
            let age = (now - lock.modified).whole_seconds();
            // 規則 2 と同じく、未来の時刻も同じ幅の外なら認めない。
            if age.abs() <= PROMOTING_MAX_AGE_SECS {
                return PromotionGate::Authorized(format!(
                    "{PROMOTE_LOCK_FILE} pid {} is running and the lock is {age}s old",
                    lock.pid
                ));
            }
            format!(
                "{PROMOTE_LOCK_FILE} pid {} is running but the lock is {age}s old (limit {PROMOTING_MAX_AGE_SECS}s)",
                lock.pid
            )
        }
    };
    PromotionGate::Rejected(format!(
        "release `{release}` is not promoted: {current_desc}; {marker_desc}; {lock_desc}"
    ))
}

/// `promote.lock` を読む（規則 3）。無ければ `None`。
fn read_promote_lock(path: &std::path::Path) -> Option<Result<PromoteLockFact, String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some(Err(e.to_string())),
    };
    let pid = match text.trim().parse::<u32>() {
        Ok(pid) => pid,
        Err(e) => return Some(Err(format!("not a pid `{}`: {e}", text.trim()))),
    };
    let modified = match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => OffsetDateTime::from(t),
        Err(e) => return Some(Err(format!("no mtime: {e}"))),
    };
    Some(Ok(PromoteLockFact {
        pid,
        alive: pid != 0 && pid_alive(pid),
        modified,
    }))
}

/// `decide_promotion` に渡す事実をファイルシステムから読む（`current` は `releases_dir` の親にある）。
pub fn read_promotion_evidence(releases_dir: &std::path::Path, release: &str) -> PromotionEvidence {
    let release_dir = releases_dir.join(release);
    let release_managed = release != DEV_RELEASE
        && !release.contains('/')
        && std::fs::symlink_metadata(&release_dir).is_ok();
    let current_target = releases_dir
        .parent()
        .and_then(|parent| std::fs::read_link(parent.join("current")).ok());
    let promoting = if release_managed {
        match std::fs::read_to_string(release_dir.join(PROMOTING_FILE)) {
            Ok(text) => {
                Some(serde_json::from_str::<PromotingMarker>(&text).map_err(|e| e.to_string()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => Some(Err(e.to_string())),
        }
    } else {
        None
    };
    let promote_lock = if release_managed {
        read_promote_lock(&release_dir.join(PROMOTE_LOCK_FILE))
    } else {
        None
    };
    PromotionEvidence {
        release_managed,
        current_target,
        promoting,
        promote_lock,
    }
}

/// 毎 tick の判断（純粋な関数。DB には触れない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickDecision {
    /// 役割は変わらない。
    Stay,
    /// `standby` → `active`（dispatch と裏方を始める）。書き込みは `instance_set_role_if` で確かめる。
    Promote,
    /// `active` → `draining`（listener を閉じ、dispatch と裏方を止める）。
    Drain,
}

/// ADR-0040 D4 の毎 tick の規則。`self_id` は自分の `instance_id`。
///
/// - `active`: 自分の行に引き継ぎの要求があれば drain。生きている別の `active` が自分より新しければ、
///   新しい方を残すために自分が drain する（規則に反する 2 つ目の active の解消）。
/// - `standby`: 生きている他の `active` が 1 つも無ければ昇格を試みる（最終的な判断は store の書き込みで）。
pub fn decide_tick(
    role: InstanceRole,
    self_id: &str,
    rows: &[DaemonInstance],
    alive: &dyn Fn(u32) -> bool,
) -> TickDecision {
    match role {
        InstanceRole::Active => {
            let me = rows.iter().find(|r| r.instance_id == self_id);
            let asked = me.is_some_and(|r| r.handoff_requested_at.is_some());
            let superseded = me.is_some_and(|me| {
                rows.iter().any(|r| {
                    r.instance_id != self_id && is_live_active(r, alive) && newer_than(r, me)
                })
            });
            if asked || superseded {
                TickDecision::Drain
            } else {
                TickDecision::Stay
            }
        }
        InstanceRole::Standby => {
            if no_other_live_active(rows, self_id, alive) {
                TickDecision::Promote
            } else {
                TickDecision::Stay
            }
        }
        // `draining` はもう役割を変えない（run が 0 になるか drain timeout で終わる）。
        // `verify` はこの表に触れない。
        InstanceRole::Draining | InstanceRole::Verify => TickDecision::Stay,
    }
}

/// `a` が `b` より新しいか（`started_at`、同時刻は `instance_id` で決める全順序）。
fn newer_than(a: &DaemonInstance, b: &DaemonInstance) -> bool {
    (a.started_at, a.instance_id.as_str()) > (b.started_at, b.instance_id.as_str())
}

/// Phase 119 D4（監視）: `rows` のうち、自分（`self_id`）以外で `drained_at` が付いているのに
/// `alive(pid)` が真の行（＝「drain 後にプロセスが終了しない」障害。D1/D2 で直したが、念のための
/// 監視）。`instance_delete_stale` がこの行を消す直前に `Supervisor::step` が呼び、見つかった行だけ
/// WARN ログに残す。純粋な判定だけを持つ（DB にもログにも触れない）ので単体テストできる。
pub fn stale_but_alive_rows<'a>(
    rows: &'a [DaemonInstance],
    self_id: &str,
    alive: &dyn Fn(u32) -> bool,
) -> Vec<&'a DaemonInstance> {
    rows.iter()
        .filter(|r| r.instance_id != self_id && r.drained_at.is_some() && alive(r.pid))
        .collect()
}

/// 自分以外の、生きている `active` の行（起動時の引き継ぎ要求の対象）。
fn store_live_actives(
    store: &Arc<dyn TaskStore>,
    self_id: &str,
) -> Result<Vec<DaemonInstance>, StoreError> {
    Ok(store
        .instance_list()?
        .into_iter()
        .filter(|r| r.instance_id != self_id && is_live_active(r, &pid_alive))
        .collect())
}

/// `Supervisor::start` の結果。
pub enum Started {
    Running(Supervisor),
    /// 同じ `release` の `active` が既にいる（exit 3）。
    Duplicate {
        instance_id: String,
        pid: u32,
    },
}

/// 1 tick 進めた結果。呼び出し側（tick ループ）がこれを見て listener とディスパッチャを動かす。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// 何も変わらない。
    Stay,
    /// `active` になった。dispatch と裏方を始める。
    Promoted,
    /// `draining` になった。**この tick で** listener を閉じ、dispatch と裏方を止める。
    Draining,
    /// 手元の run が 0 になった（`drained_at` を書いた）。exit 0。
    Drained,
    /// `[handoff] drain_timeout_secs` を超えた（`drained_at` を書いた）。残りを abort して exit 0。
    DrainTimedOut,
}

/// `daemon_instances` の自分の行を持ち、毎 tick 役割を決めるもの（ADR-0040 D4）。
/// `--mode verify` では**作らない**（verify はこの表に触れない）。
pub struct Supervisor {
    store: Arc<dyn TaskStore>,
    identity: InstanceIdentity,
    /// API と共有する役割（API はこれを読んで `standby` の 503 を返す）。
    role: SharedRole,
    started_at: OffsetDateTime,
    freshness: Duration,
    drain_timeout: Duration,
    /// ADR-0070 D4（Phase 116）: `false`（既定）なら drain timeout で abort しない。
    drain_force_abort: bool,
    /// `draining` になった時刻（drain timeout の基準）。
    drain_started_at: Option<OffsetDateTime>,
    /// ADR-0070 D4: drain timeout の WARN ログをログスパムにしないための一度きりの印。
    drain_timeout_warned: bool,
}

impl Supervisor {
    /// 起動時の判断を行い、`daemon_instances` に自分の行を書く。生きている `active` が居なければ
    /// `active` として（判断と書き込みを 1 transaction で行う）、居れば `standby` として書き、生きている
    /// **全部**の `active` の行に `handoff_requested_at` を書く。同じ `release` が生きていれば
    /// 行を書かずに `Started::Duplicate` を返す。
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        store: Arc<dyn TaskStore>,
        identity: InstanceIdentity,
        role: SharedRole,
        freshness: Duration,
        drain_timeout: Duration,
        drain_force_abort: bool,
        now: OffsetDateTime,
    ) -> Result<Started, StoreError> {
        let rows = store.instance_list()?;
        if let StartupDecision::DuplicateRelease { instance_id, pid } =
            decide_startup(&rows, &identity.release, &pid_alive)
        {
            return Ok(Started::Duplicate { instance_id, pid });
        }
        let supervisor = Self {
            store,
            identity,
            role,
            started_at: now,
            freshness,
            drain_timeout,
            drain_force_abort,
            drain_started_at: None,
            drain_timeout_warned: false,
        };
        let self_id = supervisor.identity.instance_id.clone();
        let admitted = supervisor
            .store
            .instance_register_if(&supervisor.row(InstanceRole::Active, now), &|rows| {
                no_other_live_active(rows, &self_id, &pid_alive)
            })?;
        let initial = if admitted {
            InstanceRole::Active
        } else {
            // 生きている active が居る（または居たのが書く直前に現れた）。先に全部へ引き継ぎを要求してから、
            // standby として登録する。
            for active in store_live_actives(&supervisor.store, &self_id)? {
                match supervisor
                    .store
                    .instance_request_handoff(&active.instance_id, now)?
                {
                    true => tracing::info!(
                        active = %active.instance_id, release = %supervisor.identity.release,
                        "handoff requested (ADR-0040 D4)"
                    ),
                    false => tracing::info!(
                        active = %active.instance_id,
                        "handoff was already requested for the active instance"
                    ),
                }
            }
            supervisor
                .store
                .instance_register(&supervisor.row(InstanceRole::Standby, now))?;
            InstanceRole::Standby
        };
        supervisor.role.set(initial);
        tracing::info!(
            instance_id = %supervisor.identity.instance_id,
            release = %supervisor.identity.release,
            pid = supervisor.identity.pid,
            role = %initial,
            "instance registered (ADR-0040 D4)"
        );
        Ok(Started::Running(supervisor))
    }

    pub fn identity(&self) -> &InstanceIdentity {
        &self.identity
    }

    pub fn role(&self) -> InstanceRole {
        self.role.get()
    }

    fn row(&self, role: InstanceRole, now: OffsetDateTime) -> DaemonInstance {
        DaemonInstance {
            instance_id: self.identity.instance_id.clone(),
            release: self.identity.release.clone(),
            pid: self.identity.pid,
            role,
            started_at: self.started_at,
            heartbeat_at: now,
            handoff_requested_at: None,
            drained_at: None,
        }
    }

    /// heartbeat で行が見つからなかったときの登録し直し。`active` だった者は、行を失っている間に
    /// 別の `active` が生きて昇格していれば**書かずに** `draining` へ手を離す（二重 active を作らない）。
    fn register_again(&mut self, now: OffsetDateTime) -> Result<(), StoreError> {
        let self_id = self.identity.instance_id.clone();
        if self.role.get() == InstanceRole::Active {
            let admitted = self
                .store
                .instance_register_if(&self.row(InstanceRole::Active, now), &|rows| {
                    no_other_live_active(rows, &self_id, &pid_alive)
                })?;
            if !admitted {
                tracing::warn!(
                    instance_id = %self_id,
                    "lost the active row and another active is alive; draining instead of \
                     registering as active (ADR-0040 D4 付記 2026-10-05)"
                );
                self.role.set(InstanceRole::Draining);
                self.drain_started_at = Some(now);
                self.store
                    .instance_register(&self.row(InstanceRole::Draining, now))?;
            }
            return Ok(());
        }
        let current = self.role.get();
        self.store.instance_register(&self.row(current, now))
    }

    /// 1 tick 進める。`in_flight` は**このインスタンスが抱えている** run とレビューの数
    /// （`Dispatcher::in_flight`）。heartbeat → 役割の判断 → 古い行の掃除、の順に行う。
    pub fn step(&mut self, now: OffsetDateTime, in_flight: usize) -> Result<Step, StoreError> {
        let self_id = self.identity.instance_id.clone();
        // 1. heartbeat（何かの拍子に行が消えていたら登録し直す）。
        if !self.store.instance_heartbeat(&self_id, now)? {
            tracing::warn!(
                instance_id = %self_id,
                "the daemon_instances row disappeared; registering it again"
            );
            self.register_again(now)?;
        }
        // 2. 役割の判断。書き込みが要るもの（昇格）は store で同じ transaction の中で確かめる。
        let current = self.role.get();
        let rows = self.store.instance_list()?;
        let mut step = match decide_tick(current, &self_id, &rows, &pid_alive) {
            TickDecision::Stay => Step::Stay,
            TickDecision::Promote => {
                let admitted = self.store.instance_set_role_if(
                    &self_id,
                    InstanceRole::Active,
                    now,
                    &|rows| no_other_live_active(rows, &self_id, &pid_alive),
                )?;
                if admitted {
                    self.role.set(InstanceRole::Active);
                    tracing::info!(
                        instance_id = %self_id, release = %self.identity.release,
                        "standby -> active (no other active is alive; ADR-0040 D4)"
                    );
                    Step::Promoted
                } else {
                    // まだ生きている active がいる。その drain を待つ。
                    Step::Stay
                }
            }
            TickDecision::Drain => {
                self.store
                    .instance_set_role(&self_id, InstanceRole::Draining, now)?;
                self.role.set(InstanceRole::Draining);
                self.drain_started_at = Some(now);
                tracing::info!(
                    instance_id = %self_id, release = %self.identity.release, in_flight,
                    "active -> draining (a newer release or a newer active asked for the handoff; ADR-0040 D4)"
                );
                Step::Draining
            }
        };
        // 2b. 監視（ADR-0040 D4 付記 2026-10-05）: 生きている active が自分より古い行を残していれば、
        //     その全部へ引き継ぎを要求する（新しい方を残す。古い方は次の tick で drain する）。
        if self.role.get() == InstanceRole::Active {
            let me = self.row_of(&rows, &self_id);
            for older in rows.iter().filter(|r| {
                r.instance_id != self_id && is_live_active(r, &pid_alive) && newer_than(&me, r)
            }) {
                tracing::warn!(
                    instance_id = %self_id, older = %older.instance_id,
                    "two live active instances; requesting the handoff of the older one (ADR-0040 D4 付記 2026-10-05)"
                );
                self.store
                    .instance_request_handoff(&older.instance_id, now)?;
            }
        }
        // 3. 終わった・死んだ他のインスタンスの行を消す。
        //
        // Phase 119 D4（監視）: `drained_at` が付いた行は ADR-0040 D4 の設計どおりこの直後に消える。
        // 消える前に、その pid がまだ生きていれば WARN を出す — D1/D2 が直した「drain 後にプロセスが
        // 終了しない」障害（本番 2026-09-24）の再発を journal で気付けるようにするための、念のための監視。
        // `GET /health`/`GET /releases` の `instances`（`daemon_instances` をそのまま返す）は、この
        // 行が消える直前の tick に限って同じ `drained_at`/`pid` を見せる（`status.sh` の
        // `stale_instances`〈Phase 119 D3〉は systemd を直接見るのでこの削除タイミングに左右されない）。
        for r in stale_but_alive_rows(&rows, &self_id, &pid_alive) {
            tracing::warn!(
                instance_id = %r.instance_id, release = %r.release, pid = r.pid,
                drained_at = ?r.drained_at,
                "stale: a drained daemon instance's process is still alive (it did not exit \
                 after drain; Phase 119 D1/D4). check with `ps --pid <pid>` or `systemctl \
                 --user status celeris@<release>`"
            );
        }
        let freshness = self.freshness;
        match self
            .store
            .instance_delete_where(&self_id, &|r| removable_row(r, now, freshness, &pid_alive))
        {
            Ok(removed) if !removed.is_empty() => {
                tracing::info!(removed = ?removed, "removed drained or dead daemon_instances rows");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "could not remove the stale daemon_instances rows")
            }
        }
        // 4. drain の進み具合（`Step::Draining` を返した tick では listener を閉じるのが先なので、
        //    drained の判定は次の tick から）。
        if step == Step::Stay && self.role.get() == InstanceRole::Draining {
            if in_flight == 0 {
                self.store.instance_mark_drained(&self_id, now)?;
                tracing::info!(instance_id = %self_id, "drained; exiting 0 (ADR-0040 D4)");
                step = Step::Drained;
            } else if self.drain_timed_out(now) {
                if self.drain_force_abort {
                    self.store.instance_mark_drained(&self_id, now)?;
                    tracing::warn!(
                        instance_id = %self_id, in_flight,
                        drain_timeout_secs = self.drain_timeout.as_secs(),
                        "drain timeout; aborting the remaining runs and exiting 0 (ADR-0040 D4, \
                         drain_force_abort = true)"
                    );
                    step = Step::DrainTimedOut;
                } else if !self.drain_timeout_warned {
                    // ADR-0070 D4（Phase 116）: run のプロセスが生きている限り待つ。abort しない。
                    self.drain_timeout_warned = true;
                    tracing::warn!(
                        instance_id = %self_id, in_flight,
                        drain_timeout_secs = self.drain_timeout.as_secs(),
                        "drain timeout reached but runs are still alive; waiting instead of \
                         aborting (ADR-0070 D4). set [handoff] drain_force_abort = true to force \
                         an abort"
                    );
                }
            }
        }
        Ok(step)
    }

    /// `rows` から自分の行を探す（無ければ自分の `started_at` で作る）。
    fn row_of(&self, rows: &[DaemonInstance], self_id: &str) -> DaemonInstance {
        rows.iter()
            .find(|r| r.instance_id == self_id)
            .cloned()
            .unwrap_or_else(|| self.row(InstanceRole::Active, self.started_at))
    }

    fn drain_timed_out(&self, now: OffsetDateTime) -> bool {
        let Some(started) = self.drain_started_at else {
            return false;
        };
        let limit = time::Duration::try_from(self.drain_timeout).unwrap_or(time::Duration::MAX);
        now - started >= limit
    }

    /// 終了時に自分の行を消す（SIGTERM / `--until-idle` / `--max-ticks` の停止。drain のときは
    /// `drained_at` を残したまま消える）。失敗しても停止は止めない（次に起きた誰かが古い行として消す）。
    pub fn deregister(&self) {
        match self.store.instance_delete(&self.identity.instance_id) {
            Ok(_) => {
                tracing::info!(instance_id = %self.identity.instance_id, "instance row removed")
            }
            Err(e) => tracing::warn!(error = %e, "could not remove the instance row"),
        }
    }
}

#[cfg(test)]
#[path = "instance/tests.rs"]
mod tests;
