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

use std::collections::BTreeMap;

use task_core::chat::{
    ChatPrelaunchReason, ChatRun, ChatRunState, ChatStatusPhase, chat_run_state_is_terminal,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::launch::CosChatLaunch;
use crate::dispatcher::Dispatcher;

/// 人に見せる待ちの文面（枠待ちの印。試験もこの文面で数える）。
pub(super) const SLOT_WAIT_TEXT: &str = "CoS の枠の空き待ち";

/// 待っている run 1 本（run id で引く）。
#[derive(Debug, Clone)]
pub(crate) struct SlotWait {
    pub(super) thread_id: String,
    /// 選び直すときの route 順の起点（初回と同じ）。
    pub(super) first_route: usize,
}

pub(crate) type SlotWaits = BTreeMap<String, SlotWait>;

impl CosChatLaunch {
    /// 枠だけで選べなかった run を待ちに入れる。すでに待っている run には何も書かない（tick ごとに event を増やさない）。
    pub(super) fn hold_slot_wait(
        &mut self,
        run: &ChatRun,
        thread: &str,
        first_route: usize,
        now: OffsetDateTime,
    ) {
        if self.slot_waits.contains_key(&run.id) {
            return;
        }
        self.slot_waits.insert(
            run.id.clone(),
            SlotWait {
                thread_id: thread.to_owned(),
                first_route,
            },
        );
        let _ = self.store.chat_run_status(
            &run.id,
            ChatStatusPhase::Waiting,
            &format!(
                "{SLOT_WAIT_TEXT}: 同時実行数の枠が空くまで待ち、空いたら同じ入力で起動します"
            ),
            now,
        );
    }

    /// 待っている run を 1 tick に 1 回、期限・停止・枠を見て処理する。
    pub(super) fn retry_slot_waits(&mut self, dispatcher: &mut Dispatcher) {
        let ids: Vec<String> = self.slot_waits.keys().cloned().collect();
        for id in ids {
            let Some(wait) = self.slot_waits.get(&id).cloned() else {
                continue;
            };
            let now = dispatcher.now_utc();
            let Ok(run) = self.store.chat_run_get(&wait.thread_id, &id) else {
                self.slot_waits.remove(&id);
                continue;
            };
            // 他の経路（API・回収）が閉じた run、すでに起動した run は待ちから外す。
            if chat_run_state_is_terminal(run.state)
                || run.launched_at.is_some()
                || self.running.contains_key(&wait.thread_id)
                || !matches!(run.state, ChatRunState::Running | ChatRunState::Stopping)
            {
                self.slot_waits.remove(&id);
                continue;
            }
            if run.state == ChatRunState::Stopping {
                let state =
                    if super::control::stop_intent(&run) == super::sink::ChatStopIntent::Stop {
                        ChatRunState::Stopped
                    } else {
                        ChatRunState::Interrupted
                    };
                self.close_unlaunched(
                    &run,
                    state,
                    ChatPrelaunchReason::StoppedBeforeLaunch,
                    run.reason.as_deref().unwrap_or("stopped before launch"),
                    now,
                );
                continue;
            }
            let config = self
                .run_configs
                .get(&wait.thread_id)
                .unwrap_or(&self.config)
                .clone();
            let elapsed = run
                .started_at
                .as_deref()
                .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                .map_or(0, |start| (now - start).whole_seconds().max(0) as u64);
            if elapsed >= config.max_wall_secs {
                self.close_unlaunched(
                    &run,
                    ChatRunState::Failed,
                    ChatPrelaunchReason::SlotWaitExpired,
                    "CoS の枠が空かないまま実行期限に達しました",
                    now,
                );
                continue;
            }
            // 停止中・ディスク不足・設定で無効の間、CoS 全体の上限（max_cos_runs）に達している間は選び直さない。
            if !dispatcher.accepting_new_work
                || !dispatcher.disk_ready
                || !config.enabled
                || !self.has_capacity(dispatcher)
            {
                continue;
            }
            self.launch_routes(
                dispatcher,
                &wait.thread_id,
                &run,
                wait.first_route,
                None,
                now,
            );
            if self.running.contains_key(&wait.thread_id) {
                self.slot_waits.remove(&id);
                let _ = self.store.chat_run_status(
                    &id,
                    ChatStatusPhase::Working,
                    "CoS の枠が空いたので起動しました",
                    now,
                );
            } else if self
                .store
                .chat_run_get(&wait.thread_id, &id)
                .map(|r| chat_run_state_is_terminal(r.state))
                .unwrap_or(true)
            {
                // 枠以外の理由で選べなくなって閉じた（従来の route_unavailable）。
                self.slot_waits.remove(&id);
            }
        }
    }

    /// 起動していない run を閉じる。列挙の理由が書けない旧い行（追跡なし）は印なしの終了にする。
    fn close_unlaunched(
        &mut self,
        run: &ChatRun,
        state: ChatRunState,
        prelaunch: ChatPrelaunchReason,
        text: &str,
        now: OffsetDateTime,
    ) {
        self.slot_waits.remove(&run.id);
        let result = self
            .store
            .chat_run_finish_prelaunch(&run.id, state, prelaunch, Some(text), now)
            .map(|_| ())
            .or_else(|_| {
                self.store
                    .chat_run_finish(&run.id, state, None, Some(text), now)
                    .map(|_| ())
            });
        if let Err(error) = result {
            tracing::warn!(%error, run_id = %run.id, "CoS slot wait close failed");
        }
        if state == ChatRunState::Failed
            && let Err(error) = self.store.chat_system_message_add_once(
                &run.thread_id,
                &format!("cos-route:{}:slot-wait-expired", run.id),
                &format!("CoS が返事できませんでした。理由: {text}"),
                &[],
                now,
            )
        {
            tracing::warn!(%error, run_id = %run.id, "CoS slot wait notice failed");
        }
        let _ = self.store.cos_run_credential_revoke(&run.id, now);
        self.triage.digest_due = true;
    }
}
