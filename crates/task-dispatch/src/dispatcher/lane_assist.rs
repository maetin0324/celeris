//! `routing.mode = decision_assisted` の lane 層の配線（ADR 2026-10-15-mlsys-min-routing-policy D1/D2/D4）。
//!
//! 優先順位: ①人・System の明示 lane ②許可・天井（承認・安全制約）③利用可否 ④分類 ⑤fixed。
//! dispatcher は分類を**呼ばない**。別段（daemon の分類段）が `ClassificationBoard` に置いた結果を読むだけで、
//! 結果は判断 record（`celeris.route-decision/1` の `lane_assist`）に載る。`fixed`（既定）では何もしない。
//! 鍵未登録・認証失敗は既存の credential 待ち（`credential_gate`）へ渡す。新しい待ち機構は作らない。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use task_core::model::Tier;
use task_core::model_policy::{Level, TaskFeatures};
use task_core::route_select::{
    AssistInput, ClassificationEntry, ClassifierStatus, Fact, LaneAssistRecord, LaneFacts,
    LaneLimits, RoutingMode, WorkClass, assist_lane,
};
use task_core::{LaneDecision, WorkUnitRow};

use super::credential_gate::CredentialGate;
use super::dispatch_run::tier_label;
use super::*;

/// 分類段が置く 1 件。`credential` は鍵未登録・認証失敗のときだけ（secret id と用途。値ではない）。
#[derive(Debug, Clone, PartialEq)]
pub struct BoardItem {
    pub entry: ClassificationEntry,
    pub credential: Option<CredentialGate>,
}

/// 分類段 → dispatcher の受け渡し（subject = work unit id、無ければ task id）。dispatcher は読むだけ。
#[derive(Debug, Clone, Default)]
pub struct ClassificationBoard {
    inner: Arc<Mutex<HashMap<String, BoardItem>>>,
}

impl ClassificationBoard {
    pub fn put(&self, subject: &str, item: BoardItem) {
        if let Ok(mut m) = self.inner.lock() {
            m.insert(subject.to_owned(), item);
        }
    }

    pub fn get(&self, subject: &str) -> Option<BoardItem> {
        self.inner.lock().ok()?.get(subject).cloned()
    }

    pub fn remove(&self, subject: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.remove(subject);
        }
    }
}

/// 構造化事実はコード側の `TaskFeatures` から決める。不明（Medium）は `Unknown`（cheap の根拠にしない）。
pub fn lane_facts(f: &TaskFeatures) -> LaneFacts {
    let fact = |level: Level, good: Level| {
        if level == good {
            Fact::Yes
        } else if level == Level::Medium {
            Fact::Unknown
        } else {
            Fact::No
        }
    };
    LaneFacts {
        class: if f.judgment == Level::High {
            WorkClass::HardInvestigation
        } else {
            WorkClass::Routine
        },
        procedure_clear: fact(f.ambiguity, Level::Low),
        machine_checkable: fact(f.verifiability, Level::High),
        reversible: fact(f.reversibility, Level::High),
        failure_impact_small: fact(f.consequence, Level::Low),
    }
}

fn is_credential_failure(status: &ClassifierStatus) -> bool {
    matches!(status, ClassifierStatus::FellBackToFixed { reason } if reason == "key_missing" || reason == "auth")
}

impl Dispatcher {
    /// daemon の配線（bootstrap・reload）。既定は `Fixed`。
    pub fn set_routing_mode(&mut self, mode: RoutingMode) {
        self.routing_mode = mode;
    }

    pub fn routing_mode(&self) -> RoutingMode {
        self.routing_mode
    }

    /// 分類段が書き込む board（dispatcher 側は読むだけ）。
    pub fn classification_board(&self) -> ClassificationBoard {
        self.classification_board.clone()
    }

    fn lane_limits_for(&self, task: &Task) -> Result<LaneLimits, DispatchError> {
        let org = self.store.org_list()?;
        let ceiling = task
            .assignee
            .as_deref()
            .filter(|_| !org.is_empty())
            .map(|a| task_core::profile::resolve(&org, a).lane_ceiling())
            .unwrap_or_default();
        Ok(LaneLimits {
            allowed: ceiling.allowed,
            max_lane: ceiling.max_lane,
        })
    }

    /// Jev の分類 1 回の消費を決定ログへ書く（`decision:jev:<subject>:1` の置換なので、同じ分類を何度
    /// 読んでも二重計上しない）。分類が返った（`JevOk`）ときだけ。費用が返らなければ欠測のまま。
    /// 記録の失敗で dispatch は止めない。
    fn record_jev_usage(&self, task: &Task, subject: &str, entry: &ClassificationEntry) {
        if entry.status != ClassifierStatus::JevOk {
            return;
        }
        let task_id = task.id.to_string();
        let result = (|| -> Result<(), task_core::StoreError> {
            let campaign = self
                .store
                .task_campaign_resolve(&task_id)?
                .map(|c| c.campaign_id);
            let usage = task_core::decision_log::DecisionLogUsage::jev_call(
                subject,
                1,
                &task_id,
                campaign.as_deref(),
                entry.cost_usd,
            );
            self.store.decision_log_record_usage(&usage, self.now_utc())
        })();
        if let Err(e) = result {
            tracing::warn!(task_id = %task.id, error = %e, "failed to record the jev usage");
        }
    }

    /// decision-assisted のとき、`decision.lane`（既存方策の結果）を分類で**引き上げ**得る。
    /// 返り値は (判断 record に載せる記録, credential 待ちに入れたか)。`Fixed` は何も変えず `(None, false)`。
    pub(super) fn apply_lane_assist(
        &mut self,
        task: &Task,
        wu: Option<&WorkUnitRow>,
        decision: &mut LaneDecision,
    ) -> Result<(Option<LaneAssistRecord>, bool), DispatchError> {
        if self.routing_mode == RoutingMode::Fixed {
            return Ok((None, false));
        }
        let subject = wu.map_or_else(|| task.id.to_string(), |w| w.id.clone());
        let item = self.classification_board.get(&subject);
        if let Some(item) = &item {
            self.record_jev_usage(task, &subject, &item.entry);
        }
        let limits = self.lane_limits_for(task)?;
        // 利用可否はコード側（route_select の admission）。引き上げ先の lane だけ事前に調べる。
        let mut open = HashMap::new();
        for lane in [Tier::Standard, Tier::Frontier] {
            let input = route_select::RouteInput {
                task_id: task.id.to_string(),
                harness: route_select::harness_request_for(task),
                model_hint: None,
                lane: Some(tier_label(lane).to_string()),
                cos: false,
            };
            open.insert(lane, self.select_route(&input).0.is_ok());
        }
        let outcome = assist_lane(
            &AssistInput {
                mode: self.routing_mode,
                explicit: !decision.source.policy_decides(),
                base: decision.lane,
                facts: lane_facts(&decision.features),
                entry: item.as_ref().map(|i| &i.entry),
                limits: &limits,
            },
            |t| open.get(&t).copied().unwrap_or(false),
        );
        if outcome.lane != decision.lane {
            decision.reasons.push(format!(
                "lane assist (decision_assisted): {} -> {}",
                tier_label(decision.lane),
                tier_label(outcome.lane)
            ));
            decision.lane = outcome.lane;
        }
        // 鍵未登録・認証失敗: 既存の credential 待ち（その unit だけ waiting_for_auth）へ。登録済みなら待たない。
        if let Some(item) = &item
            && is_credential_failure(&item.entry.status)
            && let Some(gate) = &item.credential
            && !self.credential_gates.is_registered(&gate.secret_id)
        {
            self.enter_credential_wait(task, wu, gate)?;
            return Ok((outcome.record, true));
        }
        Ok((outcome.record, false))
    }
}
