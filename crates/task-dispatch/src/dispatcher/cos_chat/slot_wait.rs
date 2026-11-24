//! CoS の枠待ち（ADR 2026-11-06-cos-run-launch-accounting-and-drain D2）。
//!
//! route の選択が失敗し、`RouteTrace` の候補別の構造化理由が全部「枠」（[`slot_only_wait`]）のとき、run は失敗にせず
//! claim のまま（`launched_at` なし）Waiting にして、次の tick で同じ入力・同じ run・同じ route 順に選び直す。
//! 新しい run は作らない。待っている run は worker も route の予約も持たないので、使用数（`cos_in_flight`・
//! account の in_use）には数えない。
//!
//! 期限は run の `started_at + max_wall_secs`（run 開始時の設定。後から設定を延ばしても延びない）。driver の共有期限は
//! driver が `[cos] max_wall_secs` を残り以下にして守るので、daemon の期限はこれ以上に延びない。到達したら
//! `slot_wait_expired` で閉じる。待ちの状態は DB の run 行（Running・`launched_at` なし）が正本で、このモジュールの
//! 表は tick の走査対象を覚えるだけ。再起動したら表は空になり、起動時の回収が `daemon_restart_before_launch` で閉じる。
//!
//! ここには待ちの型と、値から次の手を決める [`step`] だけを置く（ADR 2026-11-18-dispatcher-store-access-pilot）。
//! 待ちへの記録は `routes.rs` の `hold_slot_wait`、巡回と起動は `launch.rs` の `retry_slot_waits`、起動しない run の
//! 閉じ方は `control.rs` の `close_unlaunched` が store と設定を読み書きする。

use std::collections::BTreeMap;

use task_core::chat::{ChatPrelaunchReason, ChatRun, ChatRunState, chat_run_state_is_terminal};
use time::OffsetDateTime;

use super::control::stop_intent;
use super::routes::run_elapsed;
use super::sink::ChatStopIntent;

/// 人に見せる待ちの文面（枠待ちの印。試験もこの文面で数える）。
pub(super) const SLOT_WAIT_TEXT: &str = "CoS の枠の空き待ち";

/// 待っている run 1 本（run id で引く）。
#[derive(Debug, Clone)]
pub(crate) struct SlotWait {
    pub(super) thread_id: String,
}

pub(crate) type SlotWaits = BTreeMap<String, SlotWait>;

/// 待っている run 1 本を、この tick でどうするか。
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SlotWaitStep<'a> {
    /// 他の経路（API・回収）が閉じた run、すでに起動した run、thread で別の run が走っている run は待ちから外す。
    Drop,
    /// 起動しないまま閉じる（人の停止・割り込み、または期限）。
    Close {
        state: ChatRunState,
        prelaunch: ChatPrelaunchReason,
        text: &'a str,
    },
    /// 停止中・ディスク不足・設定で無効・CoS 全体の上限（max_cos_runs）の間は選び直さない。
    Hold,
    /// 同じ run・同じ route 順で選び直す。
    Launch,
}

/// 判定の順序は終端・起動済み・走っている thread → Stopping → 期限（`started_at + max_wall_secs`。[`run_elapsed`]）→ gate。
/// `run` の状態・`launched_at`・`reason`（[`stop_intent`]）・`started_at` と、thread で run が走っているか、
/// run の設定の `max_wall_secs`、gate（受付中・ディスク・設定で有効・枠）の値だけで決める。
pub(super) fn step(
    run: &ChatRun,
    thread_running: bool,
    max_wall_secs: u64,
    gate: bool,
    now: OffsetDateTime,
) -> SlotWaitStep<'_> {
    if chat_run_state_is_terminal(run.state)
        || run.launched_at.is_some()
        || thread_running
        || !matches!(run.state, ChatRunState::Running | ChatRunState::Stopping)
    {
        return SlotWaitStep::Drop;
    }
    if run.state == ChatRunState::Stopping {
        return SlotWaitStep::Close {
            state: if stop_intent(run) == ChatStopIntent::Stop {
                ChatRunState::Stopped
            } else {
                ChatRunState::Interrupted
            },
            prelaunch: ChatPrelaunchReason::StoppedBeforeLaunch,
            text: run.reason.as_deref().unwrap_or("stopped before launch"),
        };
    }
    if run_elapsed(run, now) >= max_wall_secs {
        return SlotWaitStep::Close {
            state: ChatRunState::Failed,
            prelaunch: ChatPrelaunchReason::SlotWaitExpired,
            text: "CoS の枠が空かないまま実行期限に達しました",
        };
    }
    if gate {
        SlotWaitStep::Launch
    } else {
        SlotWaitStep::Hold
    }
}
