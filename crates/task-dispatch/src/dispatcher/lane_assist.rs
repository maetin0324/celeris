//! `routing.mode = decision_assisted` の lane 層の配線（ADR 2026-10-15-mlsys-min-routing-policy D1/D2/D4）。
//!
//! 優先順位: ①人・System の明示 lane ②許可・天井（承認・安全制約）③利用可否 ④分類 ⑤fixed。
//! dispatcher は分類を**呼ばない**。別段（daemon の分類段）が `ClassificationBoard` に置いた結果を読むだけで、
//! 結果は判断 record（`celeris.route-decision/1` の `lane_assist`）に載る。`fixed`（既定）では何もしない。
//! 鍵未登録・認証失敗は既存の credential 待ち（`credential_gate`）へ渡す。新しい待ち機構は作らない。

use std::collections::HashMap;

use task_core::model::Tier;
use task_core::model_policy::{LaneCeiling, Level, TaskFeatures};
use task_core::route_select::{
    AssistInput, ClassifierStatus, Fact, LaneAssistRecord, LaneFacts, LaneLimits, ResolvedRouting,
    RoutingMode, WorkClass, assist_lane, resolve_routing,
};
use task_core::{LaneDecision, WorkUnitRow};

use super::dispatch_run::tier_label;
use super::*;
use crate::routing_runtime::classification_board::{BoardCall, ClassificationBoard};

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

/// task 欄の routing 固定（無ければ `None` = 従来どおり config に従う）。読めなければ warn して `None`
/// （dispatch は止めない。解決結果の record に `source: config` が残るので後から分かる）。
pub fn task_routing_override(
    store: &SqliteStore,
    task_id: task_core::TaskId,
) -> Option<task_core::route_select::RoutingOverride> {
    store
        .task_routing_override(task_id)
        .unwrap_or_else(|error| {
            tracing::warn!(%task_id, %error, "failed to read the task routing override");
            None
        })
}

fn is_credential_failure(status: &ClassifierStatus) -> bool {
    matches!(status, ClassifierStatus::FellBackToFixed { reason } if reason == "key_missing" || reason == "auth")
}

/// 分類 1 call の消費行。分類を試みていない（`NotAttempted`）call は None。RD・DL の `call_id` は
/// この行の鍵（`UsageKey::dedup_key`）と同一で、`jev_call_id(subject, call_no)` とは別（outcome と board の call_id を含む）。
fn jev_usage_row(
    task_id: &str,
    subject: &str,
    call: &BoardCall,
    campaign_id: Option<&str>,
) -> Option<task_core::decision_log::DecisionLogUsage> {
    let entry = &call.item.entry;
    let outcome = match &entry.status {
        ClassifierStatus::NotAttempted => return None,
        ClassifierStatus::JevOk => "ok".to_string(),
        ClassifierStatus::FellBackToFixed { reason } => {
            format!(
                "fallback:{}",
                reason.split([' ', ':']).next().unwrap_or("unknown")
            )
        }
    };
    let mut usage = task_core::decision_log::DecisionLogUsage::jev_call(
        &format!("{subject}:{outcome}:{}", call.call_id),
        call.call_no,
        task_id,
        campaign_id,
        entry.cost_usd,
    );
    usage.is_retry = call.call_no > 1;
    Some(usage)
}

impl Dispatcher {
    /// daemon の配線（bootstrap・reload）。既定は `Fixed`。
    pub fn set_routing_mode(&mut self, mode: RoutingMode) {
        self.routing.set_mode(mode);
    }

    pub fn routing_mode(&self) -> RoutingMode {
        self.routing.mode()
    }

    /// task 単位の routing の解決（ADR 2026-10-16-mlsys-measure-task-routing）:
    /// task 欄 `routing_override` があれば config の `routing.mode` より優先、無ければ従来。
    pub fn routing_for(&self, task: &Task) -> ResolvedRouting {
        let routing_override = task_routing_override(self.store.as_ref(), task.id);
        resolve_routing(self.routing.mode(), routing_override.as_ref())
    }

    /// 分類段が書き込む board（dispatcher 側は読むだけ）。
    pub fn classification_board(&self) -> ClassificationBoard {
        self.routing.board()
    }

    /// 分類段が動いている daemon が設定する。`Some(max)` の間、board に項目が無い subject の起動を最大 `max`
    /// （単調時計）だけ見送り、次の loop で分類段が先に走れるようにする。`None` は待たせない。
    pub fn set_classification_wait(&mut self, wait: Option<Duration>) {
        self.routing.set_classification_wait(wait);
    }

    /// decision-assisted で、分類がまだ board に載っていない subject の起動を上限付きで見送るか。
    /// 見送るのは上限内だけ。分類が失敗・timeout すれば `fell_back_to_fixed` の項目が載るので待ちは終わり、
    /// 上限を過ぎても項目が無ければ従来どおり（`not_attempted` の fixed）で起動する。
    pub(super) fn defer_for_classification(
        &mut self,
        task: &Task,
        wu: Option<&WorkUnitRow>,
    ) -> bool {
        let Some(max) = self.routing.classification_wait() else {
            return false;
        };
        if self.routing_for(task).mode == RoutingMode::Fixed {
            return false;
        }
        let subject = wu.map_or_else(|| task.id.to_string(), |w| w.id.clone());
        let now = self.monotonic_now();
        self.routing.defer_until_classified(&subject, now, max)
    }

    /// 担当の実効 profile と、その lane の天井（`allowed_tiers` / `budget.max_lane`。担当・組織が無ければ既定）。
    pub(super) fn assignee_ceiling(
        &self,
        task: &Task,
    ) -> Result<(Option<task_core::profile::EffectiveProfile>, LaneCeiling), DispatchError> {
        let org = self.store.org_list()?;
        let profile = task.assignee.as_deref().filter(|_| !org.is_empty());
        let profile = profile.map(|a| task_core::profile::resolve(&org, a));
        let ceiling = profile
            .as_ref()
            .map(|p| p.lane_ceiling())
            .unwrap_or_default();
        Ok((profile, ceiling))
    }

    fn lane_limits_for(&self, task: &Task) -> Result<LaneLimits, DispatchError> {
        let (_, ceiling) = self.assignee_ceiling(task)?;
        Ok(LaneLimits {
            allowed: ceiling.allowed,
            max_lane: ceiling.max_lane,
        })
    }

    /// Jev の呼び出し 1 回の消費を決定ログへ書く。呼び出しごとに一意の鍵（`call_id`）なので、成功・失敗・
    /// 再試行・再分類が上書きされず残り、同じ呼び出しを何度読んでも二重計上しない。返った費用は失敗時も
    /// 保持し、返らなければ欠測（NULL）のまま 0 にしない。分類を試みていない（`NotAttempted`）ときは書かない。
    /// 記録の失敗で dispatch は止めない。
    fn record_jev_usage(&self, task: &Task, subject: &str, call: &BoardCall) {
        let task_id = task.id.to_string();
        let result = (|| -> Result<(), task_core::StoreError> {
            let campaign = self
                .store
                .task_campaign_resolve(&task_id)?
                .map(|c| c.campaign_id);
            let Some(usage) = jev_usage_row(&task_id, subject, call, campaign.as_deref()) else {
                return Ok(());
            };
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
        let mode = self.routing_for(task).mode;
        if mode == RoutingMode::Fixed {
            return Ok((None, false));
        }
        let subject = wu.map_or_else(|| task.id.to_string(), |w| w.id.clone());
        for call in self.routing.board().take_unrecorded(&subject) {
            self.record_jev_usage(task, &subject, &call);
        }
        let call = self.routing.board().get_call(&subject);
        let item = call.as_ref().map(|c| c.item.clone());
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
                mode,
                explicit: !decision.source.policy_decides(),
                base: decision.lane,
                facts: lane_facts(&decision.features),
                entry: item.as_ref().map(|i| &i.entry),
                limits: &limits,
            },
            |t| open.get(&t).copied().unwrap_or(false),
        );
        let mut record = outcome.record;
        // 版・call ID は今回使った call（board の current）のもの。欠測は None のまま（設定値・response_model から複製しない）。
        if let (Some(record), Some(call)) = (record.as_mut(), call.as_ref()) {
            record.response_model = record
                .response_model
                .take()
                .or_else(|| call.provenance.response_model.clone());
            record.observed_snapshot = call.provenance.observed_snapshot.clone();
            record.call_id = jev_usage_row(&task.id.to_string(), &subject, call, None)
                .map(|u| u.key.dedup_key());
        }
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
            return Ok((record, true));
        }
        Ok((record, false))
    }
}
