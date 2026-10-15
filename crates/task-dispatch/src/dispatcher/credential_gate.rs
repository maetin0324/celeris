//! credential 待ちの入口（ADR 2026-10-15-mlsys-min-decision-assisted D4）。
//!
//! 経路が選ばれたあと、worker を起動する前に、その source が「登録が要る credential」を宣言していて
//! 未登録なら、**その work unit だけ**を `blocked(waiting_for_auth)` に移して `credential_waits` に記録する
//! （task 全体・兄弟・他の unit は止めない）。受信箱の要対応 item は `credential_waits` から派生する
//! （`task_ops::human_inbox`。秘密 id ごとに 1 件、通知は既存の受信箱 → CoS → webhook の経路に乗る）。
//! worker・prompt・event・ログには secret id だけを出し、値は読まない（存在確認だけ）。LLM は呼ばない。

use std::collections::HashMap;
use std::path::PathBuf;

use task_core::llm_route::SourceId;
use task_core::{
    CredentialWait, CredentialWaitEnter, Event, Status, WorkUnitBlockedReason, WorkUnitRow,
    WorkUnitStatus,
};

use super::route_select::RoutePick;
use super::*;

/// 1 つの source が必要とする credential の宣言（設定の `wait_for_credential = true` の source）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialGate {
    /// `[secrets]` の下の id（`credential_ref`）。値ではない。
    pub secret_id: String,
    /// 用途の表示名（source id）。
    pub provider: String,
    /// 受信箱に出す用途の 1 行。
    pub purpose: String,
}

/// 宣言の一式と、秘密の置き場（存在確認だけに使う）。
#[derive(Debug, Clone, Default)]
pub struct CredentialGates {
    pub secrets_dir: Option<PathBuf>,
    pub by_source: HashMap<SourceId, CredentialGate>,
}

impl CredentialGates {
    /// 秘密が登録済み（空でない file がある）か。値は読み捨てる。
    pub fn is_registered(&self, secret_id: &str) -> bool {
        let Some(dir) = &self.secrets_dir else {
            return false;
        };
        if secret_id.is_empty() || secret_id.contains(['/', '\\']) || secret_id.starts_with('.') {
            return false;
        }
        std::fs::read_to_string(dir.join(secret_id))
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    }
}

impl Dispatcher {
    /// daemon の配線（bootstrap・reload）。
    pub fn set_credential_gates(&mut self, gates: CredentialGates) {
        self.credential_gates = gates;
    }

    /// `pick` の source が未登録の credential を要するなら、その work unit を待機へ移して `true`
    /// （呼び出し側は run を起こさない）。登録済み・宣言なしなら `false`。冪等。
    pub(super) fn credential_gate_hold(
        &mut self,
        task: &Task,
        wu: Option<&WorkUnitRow>,
        pick: &RoutePick,
    ) -> Result<bool, DispatchError> {
        let Some(gate) = self.credential_gates.by_source.get(&pick.candidate.source) else {
            return Ok(false);
        };
        if self.credential_gates.is_registered(&gate.secret_id) {
            self.credential_held.remove(&task.id);
            return Ok(false);
        }
        let gate = gate.clone();
        self.enter_credential_wait(task, wu, &gate)?;
        Ok(true)
    }

    fn enter_credential_wait(
        &mut self,
        task: &Task,
        wu: Option<&WorkUnitRow>,
        gate: &CredentialGate,
    ) -> Result<(), DispatchError> {
        // 計画の無い（atomic）task は task 自身を 1 つの unit として数える。
        let wu_id = wu
            .map(|w| w.id.clone())
            .unwrap_or_else(|| format!("task:{}", task.id));
        if wu.is_none() && !self.credential_held.insert(task.id) {
            return Ok(());
        }
        let now = self.now_utc();
        let outcome = self.store.credential_wait_enter(
            &CredentialWaitEnter {
                work_unit_id: &wu_id,
                task_id: task.id,
                secret_id: &gate.secret_id,
                provider: &gate.provider,
                purpose: &gate.purpose,
            },
            now,
        )?;
        if outcome.created {
            tracing::info!(task_id = %task.id, work_unit = %wu_id, secret_id = %gate.secret_id, "credential missing; work unit waits for registration (waiting_for_auth)");
            self.store.credential_wait_set_notice_item(
                &outcome.wait.aggregation_key,
                &format!("credential_wait-{}", gate.secret_id),
                now,
            )?;
        }
        if let Some(wu) = wu {
            self.block_unit_waiting_for_auth(task, wu, now)?;
        }
        Ok(())
    }

    /// `ready` / `needs_continuation` の unit を `blocked(waiting_for_auth)` へ（他の状態には触れない）。
    fn block_unit_waiting_for_auth(
        &self,
        task: &Task,
        wu: &WorkUnitRow,
        now: OffsetDateTime,
    ) -> Result<(), DispatchError> {
        // 呼び出し側の写しが古くても二重に遷移させないよう、現在の行を読み直す。
        let Some(wu) = self
            .store
            .work_units_for(task.id)?
            .into_iter()
            .find(|u| u.id == wu.id)
        else {
            return Ok(());
        };
        if !matches!(
            wu.status,
            WorkUnitStatus::Ready | WorkUnitStatus::NeedsContinuation
        ) {
            return Ok(());
        }
        let mut row = wu.clone();
        row.status = WorkUnitStatus::Blocked;
        row.blocked_reason = Some(WorkUnitBlockedReason::WaitingForAuth);
        row.clear_lease();
        row.updated_at = rfc3339(now);
        let ev = Event::WorkUnitTransitioned {
            work_unit_id: wu.id.clone(),
            key: wu.key.clone(),
            from: wu.status,
            to: WorkUnitStatus::Blocked,
            reason: "waiting_for_auth".to_string(),
            run_id: None,
        };
        self.store
            .work_units_apply(task.id, Vec::new(), vec![row], vec![ev])?;
        Ok(())
    }

    /// daemon 起動時の復元。開いている wait を読み直し、(1) cancel 済みの task の wait は閉じ、
    /// (2) wait はあるのに unit が待機に移っていない（入口の途中で落ちた）ものは待機へ移す。
    /// 返り値は復元後も待っている wait の数。
    pub fn restore_credential_waits(&mut self) -> Result<Vec<CredentialWait>, DispatchError> {
        let now = self.now_utc();
        let mut still = Vec::new();
        for wait in self.store.credential_waits_list_waiting()? {
            let Some(task) = self.store.get(wait.task_id)? else {
                self.store.credential_wait_cancel(wait.task_id, now)?;
                continue;
            };
            if task.status == Status::Cancelled {
                self.store.credential_wait_cancel(wait.task_id, now)?;
                continue;
            }
            if let Some(unit) = self
                .store
                .work_units_for(wait.task_id)?
                .into_iter()
                .find(|u| u.id == wait.work_unit_id)
            {
                self.block_unit_waiting_for_auth(&task, &unit, now)?;
            } else {
                // atomic task は dispatch の gate が次の tick で同じ wait を見つけて保持する。
                self.credential_held.insert(wait.task_id);
            }
            still.push(wait);
        }
        Ok(still)
    }
}
