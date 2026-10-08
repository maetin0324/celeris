//! ADR 2026-10-08-browser-prod-enablement D2: browser task の前提（適合台帳）の gate。dispatch の直前
//! （lease・worker・LLM の前）に台帳の状態を見て、無い・古いなら `BrowserPrereqBlock` で止め、揃ったら
//! tick の再評価で `BrowserPrereqResume` に戻す。判定は `task_worker::browser_ledger` の決定的な規則だけ。

use super::*;
use task_core::browser_prerequisite::{BrowserPrerequisiteCode, REASON_BLOCKED};

/// 台帳が変わらなくても止まっている task を見直す間隔（tick 数）。task policy の変更などを拾う。
const RESCAN_EVERY_TICKS: u64 = 30;

/// 止めた task の最後の code（`BrowserPrerequisiteBlocked`）。
fn last_blocked_code(events: &[(u64, Event)]) -> Option<BrowserPrerequisiteCode> {
    events.iter().rev().find_map(|(_, e)| match e {
        Event::BrowserPrerequisiteBlocked { code, .. } => Some(*code),
        _ => None,
    })
}

impl Dispatcher {
    /// 試験・点検用: 台帳の見張りを差し替える（既定は daemon の設定と PATH の agent-browser）。
    pub fn set_browser_ledger_watch(&mut self, watch: task_worker::browser_ledger::LedgerWatch) {
        self.browser_ledger = watch;
        self.browser_prereq_rescan = true;
    }

    /// worker が browser 経路に入るのと同じ条件（`requests_browser`・Execute・planner でない）。
    pub(super) fn browser_prereq_applies(task: &Task, is_planner_dispatch: bool) -> bool {
        !is_planner_dispatch
            && task.kind == TaskKind::Execute
            && task_core::browser::requests_browser(&task.skills)
    }

    fn refresh_browser_ledger(&mut self) {
        if self.browser_ledger.refresh() {
            self.browser_prereq_rescan = true;
        }
    }

    fn browser_task_code(&self, task_id: TaskId) -> BrowserPrerequisiteCode {
        let credential_use = self
            .store
            .browser_task_policy_get(task_id)
            .ok()
            .flatten()
            .is_some_and(|p| {
                p.allowed_actions
                    .contains(&task_core::BrowserAction::CredentialUse)
            });
        self.browser_ledger.status().task_code(credential_use)
    }

    /// dispatch の直前の gate。前提が無ければ task を `blocked` にして `Ok(true)`（この tick は起こさない）。
    pub(super) fn browser_prereq_hold(&mut self, task: &Task) -> Result<bool, DispatchError> {
        self.refresh_browser_ledger();
        let code = self.browser_task_code(task.id);
        if code.is_ok() {
            return Ok(false);
        }
        let event = Event::BrowserPrerequisiteBlocked {
            code,
            message: code.message().to_string(),
        };
        match self.store.apply_transition_with_events(
            task.id,
            Trigger::BrowserPrereqBlock,
            vec![event],
        ) {
            Ok(_) => {
                tracing::warn!(task_id = %task.id, code = code.as_str(), "browser task held before dispatch: prerequisite missing (ADR 2026-10-08 D2)");
            }
            Err(e) => {
                tracing::warn!(task_id = %task.id, error = %e, "could not hold the browser task on its prerequisite");
            }
        }
        Ok(true)
    }

    /// tick ごと: 台帳が変わった（または一定 tick ごと）なら、`browser_prerequisite` で止めた task を
    /// 見直し、前提が揃ったものを `ready` に戻す。code が変わっただけなら event を 1 回足す。
    pub(super) fn resume_browser_prereq_blocked(&mut self) -> Result<usize, DispatchError> {
        self.refresh_browser_ledger();
        let ledger_changed = std::mem::take(&mut self.browser_prereq_rescan);
        if !ledger_changed && !self.ticks.is_multiple_of(RESCAN_EVERY_TICKS) {
            return Ok(0);
        }
        let mut resumed = 0;
        for task in self.store.list(Some(Status::Blocked))? {
            if !task_core::browser::requests_browser(&task.skills) {
                continue;
            }
            let events = self.store.events_for(task.id)?;
            if task_ops::phase_gate::last_transition_reason(&events) != Some(REASON_BLOCKED) {
                continue;
            }
            let previous = last_blocked_code(&events).unwrap_or(BrowserPrerequisiteCode::Missing);
            // 台帳全体の code（未配置・古い等）は台帳が変わったときだけ見直す。worker が台帳を読めずに止めた
            // task を、変わっていない台帳で起こし直して往復させない。task ごとの code は定期的にも見る。
            let task_level = matches!(
                previous,
                BrowserPrerequisiteCode::LedgerLacksCredential
                    | BrowserPrerequisiteCode::BrowserPolicyMissing
            );
            if !ledger_changed && !task_level {
                continue;
            }
            let code = self.browser_task_code(task.id);
            if code.is_ok() {
                match self.store.apply_transition_with_events(
                    task.id,
                    Trigger::BrowserPrereqResume,
                    vec![Event::BrowserPrerequisiteResumed { code: previous }],
                ) {
                    Ok(_) => {
                        resumed += 1;
                        tracing::info!(task_id = %task.id, "browser prerequisite resolved; task back to ready (ADR 2026-10-08 D2)");
                    }
                    Err(e) => {
                        tracing::warn!(task_id = %task.id, error = %e, "could not resume the browser task");
                    }
                }
            } else if code != previous {
                self.store.append_event(
                    task.id,
                    &Event::BrowserPrerequisiteBlocked {
                        code,
                        message: code.message().to_string(),
                    },
                )?;
            }
        }
        Ok(resumed)
    }
}
