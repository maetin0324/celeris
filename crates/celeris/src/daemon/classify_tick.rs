//! tick 側の連携（ADR 2026-10-15-jev-wire D3）: ready の work unit（計画の無い task は task 自身）を
//! `ClassificationStage` に通して `ClassificationBoard` に置く。
//!
//! dispatcher・store は board を読むだけで Jev を呼ばない。ここは daemon 層で、`Dispatcher::tick` の**直前**に
//! 呼ばれる。`routing.mode = fixed` / `[jev] enabled = false` なら何もしない（board は空のまま）。
//! 同じ subject は board に分類済みの entry があれば再分類しない（冪等）。ただし鍵未登録・認証失敗の entry は
//! 鍵が登録し直されたかもしれないので、次に ready になったとき（credential 待ちの解除後）に分類し直す。

use std::sync::Arc;

use jev_classifier::{ClassifyInput, JevClassifier, JevError};
use task_core::execution_plan::WorkUnitKind;
use task_core::route_select::{ClassifierStatus, RoutingMode};
use task_core::{Task, TaskStore, WorkUnitRow, runnable_work_units};
use task_dispatch::{ClassificationBoard, CredentialGate};

use super::classify_stage::ClassificationStage;
use super::jev_key::SecretsDirKeyProvider;
use crate::config::JevSectionConfig;

/// 1 tick で見る task の上限（dispatcher の窓と同程度。分類は ready の先頭から）。
const WINDOW: usize = 32;

/// 1 回の `run` で新しく分類する件数の上限（分類は tick の前に await するので、dispatch を止める長さの上限）。
/// ready の subject を先に、残りは pending の先読み。超えた分は次の loop で分類する。
const MAX_CLASSIFICATIONS_PER_RUN: usize = 8;

/// dispatcher が、board に項目の無い subject の起動を見送る上限（ADR 2026-10-22-jev-classification-before-launch D3）。
/// 見送った subject は次の loop の分類段で先に分類される。超えたら分類なしで従来どおり起動する。
pub(crate) const DISPATCH_WAIT_MAX: std::time::Duration = std::time::Duration::from_secs(20);

pub(crate) struct ClassifyTick {
    stage: ClassificationStage,
    board: ClassificationBoard,
}

impl ClassifyTick {
    /// `[jev] enabled = true` のときだけ作る。`board` は dispatcher の `classification_board()`。
    pub(crate) fn build(
        jev: &JevSectionConfig,
        secrets_dir: Option<std::path::PathBuf>,
        board: ClassificationBoard,
    ) -> Result<Option<Self>, JevError> {
        if !jev.enabled {
            return Ok(None);
        }
        let classifier = JevClassifier::new(jev.to_jev_config())?;
        let keys = SecretsDirKeyProvider::new(secrets_dir, jev.secret_id.clone());
        let credential = CredentialGate {
            secret_id: jev.secret_id.clone(),
            provider: "jev".into(),
            purpose: "Jev で依頼を分類する".into(),
        };
        Ok(Some(Self::with_stage(
            ClassificationStage::new(
                Arc::new(classifier),
                Arc::new(keys),
                credential,
                board.clone(),
            ),
            board,
        )))
    }

    pub(crate) fn with_stage(stage: ClassificationStage, board: ClassificationBoard) -> Self {
        Self { stage, board }
    }

    /// 分類した件数を返す。store の読み取り失敗は warn して続ける（dispatch は止めない）。
    /// mode は task ごとに解決する（task 欄 `routing_override` が config の `mode` より優先。ADR 2026-10-16-mlsys-measure-task-routing）:
    /// config が fixed でも decision_assisted に固定した task は分類し、config が decision_assisted でも fixed に
    /// 固定した task は分類しない。
    pub(crate) async fn run(&self, store: &dyn TaskStore, mode: RoutingMode) -> usize {
        let mut tasks = match store.ready_tasks(WINDOW) {
            Ok(t) => t,
            Err(error) => {
                tracing::warn!(%error, "jev: could not read ready tasks");
                return 0;
            }
        };
        match store.running_tasks_with_runnable_work_units(WINDOW) {
            Ok(t) => tasks.extend(t),
            Err(error) => tracing::warn!(%error, "jev: could not read runnable work units"),
        }
        // 走っている WU があるだけの task も先読みの対象にする（pending の WU が次の tick で ready になって起動する）。
        match store.list(Some(task_core::Status::Running)) {
            Ok(running) => {
                for t in running.into_iter().take(WINDOW) {
                    if !tasks.iter().any(|known| known.id == t.id) {
                        tasks.push(t);
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "jev: could not read running tasks"),
        }
        let mut done = 0;
        for task in tasks {
            let routing_override = task_dispatch::task_routing_override(store, task.id);
            let mode =
                task_core::route_select::resolve_routing(mode, routing_override.as_ref()).mode;
            if mode == RoutingMode::Fixed {
                continue;
            }
            let units = store.work_units_for(task.id).unwrap_or_default();
            let subjects = if units.is_empty() {
                // 計画の無い task 自身は、ready の間だけ分類する（走り出した後は意味がない）。
                if task.status != task_core::Status::Ready {
                    continue;
                }
                vec![(task.id.to_string(), task_input(&task))]
            } else {
                // ready（runnable）を先に、続けて pending など終わっていない WU を先読みする（WU の spec は
                // 変わらないので、ready になる前に分類しておけば同じ tick で起動しても分類済み）。
                let ids = runnable_work_units(&units, 0, WINDOW);
                let ready = units.iter().filter(|u| ids.contains(&u.id));
                let ahead = units.iter().filter(|u| {
                    !ids.contains(&u.id)
                        && !u.status.is_terminal()
                        && u.kind != WorkUnitKind::Integrate
                        && u.kind != WorkUnitKind::Task
                });
                ready
                    .chain(ahead)
                    .map(|u| (u.id.clone(), unit_input(u, &units)))
                    .collect()
            };
            for (subject, input) in subjects {
                if done >= MAX_CLASSIFICATIONS_PER_RUN {
                    return done;
                }
                if self.needs_classification(&subject) {
                    self.stage.classify_for_mode(mode, &subject, &input).await;
                    done += 1;
                }
            }
        }
        done
    }

    fn needs_classification(&self, subject: &str) -> bool {
        match self.board.get(subject) {
            None => true,
            Some(item) => {
                item.credential.is_some()
                    && matches!(
                        &item.entry.status,
                        ClassifierStatus::FellBackToFixed { reason }
                            if reason == "key_missing" || reason == "auth"
                    )
            }
        }
    }
}

fn task_input(task: &Task) -> ClassifyInput {
    ClassifyInput {
        instruction: task.objective.clone(),
        acceptance: task.acceptance.iter().map(|c| c.text.clone()).collect(),
        required_artifacts: Vec::new(),
        dependency_summaries: Vec::new(),
        recent_failure: None,
    }
}

fn unit_input(unit: &WorkUnitRow, all: &[WorkUnitRow]) -> ClassifyInput {
    ClassifyInput {
        instruction: unit.spec.objective.clone(),
        acceptance: unit.spec.done_when.clone(),
        required_artifacts: unit.spec.outputs.clone(),
        dependency_summaries: all
            .iter()
            .filter(|u| {
                unit.depends_on.contains(&u.id) || unit.spec.depends_on.contains(&u.spec.key)
            })
            .map(|u| u.spec.title.clone())
            .collect(),
        recent_failure: None,
    }
}

#[cfg(test)]
#[path = "classify_tick/tests.rs"]
mod tests;

#[cfg(test)]
mod e2e;
