//! Phase F5-fix6: 再起動直後に、居なくなったデーモンが抱えていた run（孤児）を lease の失効を待たずに
//! 回収するための判定（純粋関数と設定だけ。DB にも LLM にも触れない）。
//!
//! ## 「lease の持ち主が居ない」の定義（ADR-0070 付記「F5-fix6 実装時の明確化」）
//!
//! lease（`tasks.lease_worker_run_id` / `work_units.lease_run_id`）には持ち主のインスタンスが
//! 書かれていない（ADR-0070 D4 で `instance_id` を lease に埋め込む案は採らなかった）。run の
//! プロセスの pid も DB・run ディレクトリのどこにも残らない（`task_worker::process_group` の表は
//! プロセス内のメモリだけ）。そこで次の 2 つが**両方**成り立つときに限り、その run の持ち主は
//! 居ないと判断する:
//!
//! 1. **このインスタンスが抱えていない**: `Dispatcher` の `running` / `checking` / `integrating` /
//!    `reviewing` / `awaiting_children` のどれにもその Task（v2 の WU ならその run）が無い
//!    （呼び出し側が見る）。
//! 2. **run を抱えうる他のインスタンスが 1 つも生きていない**（[`holder_gone`]）: `daemon_instances`
//!    の自分以外の行のうち、`role` が `active` / `draining` で、`drained_at` が無く、`heartbeat_at` が
//!    `freshness`（ADR-0040 D4 の `3 × tick + lease_grace`）以内で、かつ `pid` のプロセスが生きている
//!    ものが 1 つも無い。`standby` は dispatch しないので run を持たない（数えない）。`verify` は
//!    表に行を書かない。
//!
//! run のワーカープロセスは、それを起こしたデーモンの子（stdout/stderr はデーモンへの pipe、
//! `result.json` はデーモン内のアダプタが書く）なので、持ち主のデーモンが居なければ、その run の
//! 結果を記録できる者はもう居ない（systemd の `KillMode=control-group` では子も同時に止まる）。
//!
//! **ライブ切替（ADR-0040 D4）の例外**: draining の旧デーモンは自分の run を面倒を見続け、その間
//! heartbeat を打ち続ける。その行は 2. の「生きている他のインスタンス」に当たるので、新しい active は
//! 旧が抱える run を**横取りしない**（従来どおり lease の失効か、旧の完了の記録を待つ）。逆に、その間は
//! 別の（クラッシュした）デーモンの孤児も lease 失効まで待つ（保守的な側に倒す）。

use std::sync::Arc;
use std::time::Duration;

use task_core::{DaemonInstance, InstanceRole};
use time::OffsetDateTime;

/// 孤児の回収に使う event / WU 遷移の reason。
pub const ORPHAN_TAKEOVER_REASON: &str = "orphan_takeover";

/// 孤児の回収を有効にする設定（celeris が `daemon_instances` に行を持つときだけ渡す。`--mode verify`
/// と、この設定を渡さないテスト・`celerisctl` の組み立てでは無効 = 従来どおり lease の失効を待つ）。
#[derive(Clone)]
pub struct OrphanTakeover {
    /// このインスタンスの `instance_id`（`daemon_instances` の自分の行）。
    pub instance_id: String,
    /// heartbeat が新しいとみなす窓（ADR-0040 D4 の `freshness_window`）。
    pub freshness: Duration,
    /// その pid のプロセスが生きているか（本番は `/proc/<pid>` を見る。判定できなければ `true`）。
    pub pid_alive: Arc<dyn Fn(u32) -> bool + Send + Sync>,
}

impl std::fmt::Debug for OrphanTakeover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrphanTakeover")
            .field("instance_id", &self.instance_id)
            .field("freshness", &self.freshness)
            .finish_non_exhaustive()
    }
}

/// run を抱えうる他のインスタンスが生きているか（上の定義の 2.）。`true` = 持ち主は居ない。
pub fn holder_gone(
    rows: &[DaemonInstance],
    self_id: &str,
    now: OffsetDateTime,
    freshness: Duration,
    alive: &dyn Fn(u32) -> bool,
) -> bool {
    !rows.iter().any(|r| {
        r.instance_id != self_id
            && matches!(r.role, InstanceRole::Active | InstanceRole::Draining)
            && r.drained_at.is_none()
            && r.is_fresh(now, freshness)
            && alive(r.pid)
    })
}

/// lease を持たない `running` の `runs` 行（reviewer run 等）を閉じる理由（`WorkerFinished` の
/// `interrupted: <why>`）。持ち主のデーモンが 1 つも生きていないとき。
pub const OWNERLESS_GONE_WHY: &str = "orphan_takeover: no live daemon instance holds this run";
/// 同上。他の生きているインスタンスはあるが、run の期限（`started_at + ttl`）を過ぎたとき。
pub const OWNERLESS_DEADLINE_WHY: &str =
    "orphan_takeover: the run outlived its deadline without a lease";

/// 持ち主の居ない `running` の `runs` 行の扱い（[`ownerless_run_decision`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerlessRun {
    /// まだ閉じない（他の生きているインスタンスが面倒を見ているかもしれない）。
    Keep,
    /// `WorkerFinished{end: cancelled, outcome: "interrupted: <why>"}` で閉じる。
    Close { why: &'static str },
}

/// このインスタンスが抱えておらず、task / WU の lease も持たない `running` の `runs` 行を閉じるか。
/// 呼び出し側が「手元に無い」「lease を持たない」「task が終端でない」を確かめた後に使う。
///
/// - run を抱えうる他のインスタンスが 1 つも生きていない（`holders_gone`、[`holder_gone`]）→ すぐ閉じる
///   （結果を記録できる者はもう居ない）。
/// - 他に生きているインスタンスがある（例: ライブ切替で draining の旧デーモンがまだ自分のレビューを
///   面倒見ている）→ `started_at + ttl` を過ぎるまでは閉じない。過ぎたら閉じる（その run の時間の上限を
///   越えて生きている run は無い）。
pub fn ownerless_run_decision(
    holders_gone: bool,
    started_at: OffsetDateTime,
    ttl: Duration,
    now: OffsetDateTime,
) -> OwnerlessRun {
    if holders_gone {
        return OwnerlessRun::Close {
            why: OWNERLESS_GONE_WHY,
        };
    }
    if now >= started_at + ttl {
        return OwnerlessRun::Close {
            why: OWNERLESS_DEADLINE_WHY,
        };
    }
    OwnerlessRun::Keep
}

/// 同一ホストの pid の生死（`/proc/<pid>`）。判定できない環境・pid 0 は `true`（生きている＝横取り
/// しない、保守的な側）。`celeris::instance::pid_alive` と同じ規則。
pub fn proc_pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return true;
    }
    match std::fs::metadata(format!("/proc/{pid}")) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests;
