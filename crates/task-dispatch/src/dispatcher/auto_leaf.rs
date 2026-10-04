//! ADR-0079: depth-limited automatic leaves; durable counters from existing events.
use super::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug)]
pub(super) struct AutoLeafWatch {
    compactions: AtomicU32,
    max: u32,
    pub wake: tokio::sync::Notify,
}

impl AutoLeafWatch {
    pub fn new(compactions: u32, max: u32) -> Self {
        Self {
            compactions: AtomicU32::new(compactions),
            max,
            wake: tokio::sync::Notify::new(),
        }
    }
    pub fn compacted(&self) {
        let _ = self
            .compactions
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                Some(n.saturating_add(1))
            });
        if self.exceeded() {
            self.wake.notify_one();
        }
    }
    pub fn exceeded(&self) -> bool {
        self.compactions.load(Ordering::SeqCst) > self.max
    }
}

#[derive(Default)]
pub(super) struct AutoLeafCounts {
    pub compactions: u32,
    pub continuations: u32,
    pub progress: String,
    compacted_runs: HashSet<String>,
}

impl AutoLeafCounts {
    fn observe_end(&mut self, run_id: &str, end: task_core::RunEnd) {
        if end.is_continuable() {
            self.continuations = self.continuations.saturating_add(1);
        }
        if matches!(
            end,
            task_core::RunEnd::BudgetExhausted {
                kind: task_core::BudgetKind::Context
            }
        ) && !self.compacted_runs.contains(run_id)
        {
            self.compactions = self.compactions.saturating_add(1);
        }
    }
    pub fn exceeded(&self, limits: task_core::TreeLimits) -> bool {
        self.compactions > limits.auto_leaf_max_compactions
            || self.continuations > limits.auto_leaf_max_continuations
    }
}

/// Scope by WU identity (not only the key); answers reset the observation window.
/// Counts survive dispatcher restart and session resume. A context terminal does not
/// double count a run whose adapter already reported compaction boundaries.
pub(super) fn counts(
    events: &[(u64, Event)],
    wu: &task_core::WorkUnitRow,
    current_run: &str,
    end: Option<task_core::RunEnd>,
) -> Option<AutoLeafCounts> {
    let automatic = events
        .iter()
        .rev()
        .find_map(|(_, e)| match e {
            Event::UnitGateOverridden {
                plan_id,
                unit_key,
                action,
                ..
            } if plan_id == &wu.plan_id && unit_key == &wu.key => {
                Some(*action == task_core::UnitGateAction::AutoLeaf)
            }
            _ => None,
        })
        .unwrap_or(false);
    if !automatic {
        return None;
    }
    let prefix = format!("auto_leaf_budget:{}:", wu.id);
    let requests: HashSet<&str> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::DecisionRequested { decision } if decision.key.starts_with(&prefix) => {
                Some(decision.id.as_str())
            }
            _ => None,
        })
        .collect();
    let since = events
        .iter()
        .rev()
        .find_map(|(seq, e)| match e {
            Event::DecisionAnswered { id, option, .. }
                if requests.contains(id.as_str()) && option == "run-as-leaf" =>
            {
                Some(*seq)
            }
            _ => None,
        })
        .unwrap_or(0);
    let mut runs: HashSet<&str> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkUnitTransitioned {
                work_unit_id,
                run_id: Some(run_id),
                ..
            } if work_unit_id == &wu.id => Some(run_id.as_str()),
            _ => None,
        })
        .collect();
    runs.insert(current_run);
    let mut counts = AutoLeafCounts::default();
    for (_, event) in events.iter().filter(|(seq, _)| *seq > since) {
        match event {
            Event::WorkerProgress {
                run_id,
                kind,
                tool,
                msg,
                summary,
                ..
            } if runs.contains(run_id.as_str()) => {
                if *kind == Some(task_core::ProgressKind::Status)
                    && tool.as_deref() == Some(task_core::tree::CONTEXT_COMPACTION_TOOL)
                {
                    counts.compactions = counts.compactions.saturating_add(1);
                    counts.compacted_runs.insert(run_id.clone());
                } else if *kind == Some(task_core::ProgressKind::Status)
                    && tool.as_deref() == Some(task_core::tree::CONTEXT_ROLLOVER_TOOL)
                {
                    // This closed the preceding run's context. It must not suppress
                    // a new compaction / context exhaustion in this run.
                    counts.compactions = counts.compactions.saturating_add(1);
                } else {
                    counts.progress = summary
                        .as_deref()
                        .unwrap_or(msg)
                        .chars()
                        .take(500)
                        .collect();
                }
            }
            Event::WorkerFinished {
                run_id,
                end: Some(end),
                ..
            } if runs.contains(run_id.as_str()) => counts.observe_end(run_id, *end),
            _ => {}
        }
    }
    if let Some(end) = end {
        counts.observe_end(current_run, end);
    }
    Some(counts)
}

impl Dispatcher {
    pub(super) fn auto_leaf_decision(
        &self,
        task: &Task,
        wu: &task_core::WorkUnitRow,
        run_id: &str,
        counts: &AutoLeafCounts,
        checkpoint: Option<&task_core::Checkpoint>,
    ) -> Result<task_core::DecisionRequest, DispatchError> {
        use task_core::{
            CostOfReversal, DecisionKind, DecisionOption, DecisionRequest, DecisionStatus,
        };
        let limits = self.config.execution.limits.tree;
        let mut path =
            task_ops::tree::decision_path(self.store.as_ref(), task).map_err(ops_to_store)?;
        if let Some(last) = path.last_mut() {
            last.stage = wu.phase.clone();
        }
        path.push(task_core::DecisionPathEntry {
            task_id: task.id,
            title: wu.spec.title.clone(),
            stage: None,
            unit: Some(wu.key.clone()),
        });
        let progress = checkpoint
            .map(|cp| cp.completed.join("; "))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| counts.progress.clone())
            .chars()
            .take(500)
            .collect::<String>();
        Ok(DecisionRequest {
            id: ulid::Ulid::new().to_string(),
            key: format!("auto_leaf_budget:{}:{run_id}", wu.id),
            kind: DecisionKind::LeafTooLarge,
            question: format!("自動で実行した leaf {}「{}」の継続を確認してください。前回の続行回答以降（未回答なら開始以降）: compaction/context rollover {} 回（上限 {}）、continuation {} 回（上限 {}）。直近の進捗: {}", wu.key, wu.spec.title, counts.compactions, limits.auto_leaf_max_compactions, counts.continuations, limits.auto_leaf_max_continuations, if progress.is_empty() { "記録なし" } else { &progress }),
            options: vec![
                DecisionOption { key: "run-as-leaf".into(), label: "続ける".into(), consequence: Some("checkpoint/session を引き継ぎ、警戒の計数を再開する".into()) },
                DecisionOption { key: "replan".into(), label: "replan で小さな leaf に分ける".into(), consequence: None },
                DecisionOption { key: "withdraw".into(), label: "この unit を取り下げる".into(), consequence: None },
            ],
            recommended: "replan".into(), cost_of_reversal: CostOfReversal::Low,
            cost_note: Some("gate は compound だが深さ上限のため leaf のまま自動実行した。実測の回数が閾値を超えたので停止した。".into()),
            needed_before: vec![wu.key.clone()], path,
            raised_by: task_core::DecisionRaisedBy { task_id: task.id, run_id: Some(run_id.into()), origin: task_core::DecisionOrigin::Daemon },
            status: DecisionStatus::Open, answer: None, withdrawn_reason: None,
        })
    }
}
