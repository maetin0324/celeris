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
    LaneLimits, ResolvedRouting, RoutingMode, WorkClass, assist_lane, resolve_routing,
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

/// board に置かれた 1 回の Jev 呼び出し。`call_id` は呼び出しごとに一意（再起動をまたいでも重ならない）で、
/// 決定ログの消費行の鍵になる。`call_no` は同じ subject の何回目か（2 以上は再分類・再試行）。
#[derive(Debug, Clone, PartialEq)]
pub struct BoardCall {
    pub call_id: String,
    pub call_no: u32,
    pub item: BoardItem,
    /// この呼び出しに結び付いた版メタデータ。従来の `put` は全欄 `None`（欠測）。
    pub provenance: CallProvenance,
}

/// 1 回の Jev 呼び出しの版メタデータ。`call_id` と同じ `BoardCall` に載り、subject だけで引く別 call の値と混ざらない。
/// 欠測は `None`（設定の model 名で埋めない・片方から他方を複製しない）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallProvenance {
    /// 応答が名乗った model 名。
    pub response_model: Option<String>,
    /// 応答と独立に観測した版。
    pub observed_snapshot: Option<String>,
}

#[derive(Debug, Default)]
struct BoardState {
    current: HashMap<String, BoardCall>,
    /// subject ごとの呼び出し回数（`remove` しても減らさない）。
    calls: HashMap<String, u32>,
    /// まだ決定ログへ書いていない呼び出し（dispatcher が読む前に置き換わった分を落とさない）。
    unrecorded: HashMap<String, Vec<BoardCall>>,
}

/// 分類段 → dispatcher の受け渡し（subject = work unit id、無ければ task id）。dispatcher は読むだけ。
#[derive(Debug, Clone, Default)]
pub struct ClassificationBoard {
    inner: Arc<Mutex<BoardState>>,
}

fn next_call_id() -> String {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    static START: OnceLock<u128> = OnceLock::new();
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let start = *START.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    });
    format!("{start:x}-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

impl ClassificationBoard {
    /// 呼び出し 1 回分を置く。置くたびに新しい `call_id` が付く（前の呼び出しは決定ログに残る）。
    pub fn put(&self, subject: &str, item: BoardItem) {
        self.put_with_provenance(subject, item, CallProvenance::default());
    }

    /// `put` と同じ採番（`call_id`・`call_no`）で、版メタデータを同じ呼び出しに原子的に結び付けて置く。
    pub fn put_with_provenance(&self, subject: &str, item: BoardItem, provenance: CallProvenance) {
        if let Ok(mut s) = self.inner.lock() {
            let n = s.calls.entry(subject.to_owned()).or_insert(0);
            *n += 1;
            let call = BoardCall {
                call_id: next_call_id(),
                call_no: *n,
                item,
                provenance,
            };
            s.unrecorded
                .entry(subject.to_owned())
                .or_default()
                .push(call.clone());
            s.current.insert(subject.to_owned(), call);
        }
    }

    pub fn get(&self, subject: &str) -> Option<BoardItem> {
        self.get_call(subject).map(|c| c.item)
    }

    pub fn get_call(&self, subject: &str) -> Option<BoardCall> {
        self.inner.lock().ok()?.current.get(subject).cloned()
    }

    /// 決定ログへ未記録の呼び出しを古い順に取り出す（取り出したら空になる）。
    pub fn take_unrecorded(&self, subject: &str) -> Vec<BoardCall> {
        self.inner
            .lock()
            .ok()
            .and_then(|mut s| s.unrecorded.remove(subject))
            .unwrap_or_default()
    }

    pub fn remove(&self, subject: &str) {
        if let Ok(mut s) = self.inner.lock() {
            s.current.remove(subject);
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

/// task 欄の routing 固定（無ければ `None` = 従来どおり config に従う）。読めなければ warn して `None`
/// （dispatch は止めない。解決結果の record に `source: config` が残るので後から分かる）。
pub fn task_routing_override(
    store: &dyn TaskStore,
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

impl Dispatcher {
    /// daemon の配線（bootstrap・reload）。既定は `Fixed`。
    pub fn set_routing_mode(&mut self, mode: RoutingMode) {
        self.routing_mode = mode;
    }

    pub fn routing_mode(&self) -> RoutingMode {
        self.routing_mode
    }

    /// task 単位の routing の解決（ADR 2026-10-16-mlsys-measure-task-routing）:
    /// task 欄 `routing_override` があれば config の `routing.mode` より優先、無ければ従来。
    pub fn routing_for(&self, task: &Task) -> ResolvedRouting {
        let routing_override = task_routing_override(self.store.as_ref(), task.id);
        resolve_routing(self.routing_mode, routing_override.as_ref())
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

    /// Jev の呼び出し 1 回の消費を決定ログへ書く。呼び出しごとに一意の鍵（`call_id`）なので、成功・失敗・
    /// 再試行・再分類が上書きされず残り、同じ呼び出しを何度読んでも二重計上しない。返った費用は失敗時も
    /// 保持し、返らなければ欠測（NULL）のまま 0 にしない。分類を試みていない（`NotAttempted`）ときは書かない。
    /// 記録の失敗で dispatch は止めない。
    fn record_jev_usage(&self, task: &Task, subject: &str, call: &BoardCall) {
        let entry = &call.item.entry;
        let outcome = match &entry.status {
            ClassifierStatus::NotAttempted => return,
            ClassifierStatus::JevOk => "ok".to_string(),
            ClassifierStatus::FellBackToFixed { reason } => {
                format!(
                    "fallback:{}",
                    reason.split([' ', ':']).next().unwrap_or("unknown")
                )
            }
        };
        let task_id = task.id.to_string();
        let result = (|| -> Result<(), task_core::StoreError> {
            let campaign = self
                .store
                .task_campaign_resolve(&task_id)?
                .map(|c| c.campaign_id);
            let mut usage = task_core::decision_log::DecisionLogUsage::jev_call(
                &format!("{subject}:{outcome}:{}", call.call_id),
                call.call_no,
                &task_id,
                campaign.as_deref(),
                entry.cost_usd,
            );
            usage.is_retry = call.call_no > 1;
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
        for call in self.classification_board.take_unrecorded(&subject) {
            self.record_jev_usage(task, &subject, &call);
        }
        let item = self.classification_board.get(&subject);
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
