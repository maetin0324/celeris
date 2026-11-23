//! ADR 2026-11-22-parent-checkpoint-evaluator D2.3・D3.4: 親の途中評価の reviewer run の起動の条件・試行の記録・終わりの
//! 決着（最終の記録）。run の起動・実行・終わりの記録は `health_review/design_gate.rs` の共通の起動口
//! （`start_side_review`・`on_side_review_finished`）だけが行い、ここは写さない。
//!
//! 状態は保存しない。実行中の判断・試行の数・評価済みは、判断の行と root の `ArtifactProduced` の name から
//! `task_core::parent_checkpoint::checkpoint_states` で毎回導く（D1.3）。LLM は呼ばない。
//!
//! `Dispatcher` に method を足さない（repo health の `size.responsibility-growth`。`dispatcher` module の本体の行と
//! method の数が閾値を超えている）ため、この file の関数は `&mut Dispatcher` を受ける自由関数にする。呼び出しは
//! `dispatcher/project_wake.rs` の `tick_project_controller` の 1 行と、起動口の終わりの腕・`holds_side_review` の 1 行。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use task_core::parent_checkpoint as pc;
use task_core::project_proposal::{ProjectAction, sha256_hex};
use task_core::project_wake::{ControllerMode, ProjectControllerDecision};
use task_core::store::SqliteStore;
use task_core::strategy::Reference;
use task_core::{ArtifactRef, Event, Status, Task, TaskId};
use task_worker::protocol::{
    ParentCheckpointChild, ParentCheckpointGoal, ParentCheckpointMetrics, ParentCheckpointPrevious,
    ParentCheckpointRequest,
};
use task_worker::{AdapterError, RunOutcome, Terminal};

use crate::dispatcher::design_gate::SideReviewKind;
use crate::dispatcher::{DispatchError, Dispatcher};
use crate::project_controller::TreeTokens;
use crate::project_decider_ts::decision_proposal;

/// tick の段（`tick_project_controller` の判断の後）: `enabled` の案件の実行中の `evaluate` の判断ごとに、root の run を
/// 手元に持っていない・木の上限が止めない・試行 < 上限のときだけ評価の run を起こし、試行の記録を書く。上限に達して
/// run を持っていなければ `invalid` の最終の記録を書く（D2.3）。失敗は log に残して次の tick に回す。
pub(crate) fn launch(d: &mut Dispatcher, settings: &pc::CheckpointSettings, tokens: TreeTokens) {
    if let Err(error) = launch_all(d, settings, tokens) {
        tracing::warn!(%error, "parent evaluation: launch failed; retrying on the next tick");
    }
}

/// 評価の run の完了の種類 `SideReviewKind::ParentEvaluation` の中身: どの判断の評価か（`decision_id`）と、起動の
/// 時点の試行の上限（終わりの決着が `invalid` の最終の記録を書くかの判定に使う。D3.4）。
pub(crate) struct Subject {
    pub(crate) decision_id: String,
    pub(crate) max_attempts: u32,
}

/// 1 つの root の実行中の判断（判断の順）と、その `triggers`・判断の行の `request_sha256`。
struct Running {
    root: Task,
    decisions: Vec<(String, Vec<String>, String)>,
    attempts: BTreeMap<String, u32>,
}

fn launch_all(
    d: &mut Dispatcher,
    settings: &pc::CheckpointSettings,
    tokens: TreeTokens,
) -> Result<(), DispatchError> {
    let store = d.store();
    let max = settings.max_evaluation_attempts();
    for state in store.project_controller_active()? {
        // shadow・dry_run の判断は `suppressed` で実行中にならないが、mode でも起こさない（D9.1）。
        if state.mode != ControllerMode::Enabled {
            continue;
        }
        let rows = store.project_controller_decisions(&state.project_id, None, usize::MAX)?;
        for running in running_by_root(&store, &rows)? {
            let root = &running.root;
            for (id, triggers, request_sha256) in &running.decisions {
                // 条件 1: root の run を手元に持っていれば起こさない（`register_run` が entry を上書きするため）。
                if d.supervisor.has_task_run(root.id) {
                    break;
                }
                let attempts = running.attempts.get(id).copied().unwrap_or(0);
                if attempts >= max {
                    finish(&store, d, root, id, "", None, max)?;
                    continue;
                }
                // 条件 2: 木の run・費用の上限（設計 review と同じ扱い。`kind: limit` の決定の要求に任せる）。
                if d.tree_limit_hold_for(root, task_core::NextRun::Worker)? {
                    break;
                }
                let Some(request) = request(&store, root, id, triggers, request_sha256, tokens)?
                else {
                    continue;
                };
                let review = task_worker::ReviewRequest {
                    parent_checkpoint: Some(Box::new(request)),
                    ..Default::default()
                };
                let kind = SideReviewKind::ParentEvaluation(Subject {
                    decision_id: id.clone(),
                    max_attempts: max,
                });
                let Some(run_id) = d.start_side_review(root, review, kind)? else {
                    continue;
                };
                let n = attempts + 1;
                let body = serde_json::json!({"schema": pc::ATTEMPT_SCHEMA, "decision_id": id, "attempt": n, "run_id": run_id});
                let name = pc::attempt_artifact_name(id, n);
                write_record(
                    &store,
                    d,
                    root,
                    &name,
                    pc::ATTEMPT_ARTIFACT_KIND,
                    &run_id,
                    &body,
                )?;
            }
        }
    }
    Ok(())
}

/// 案件の判断の行から、root ごとの実行中の `evaluate` の判断と試行の数を導く（D1.3。行は新しい順で来る）。
fn running_by_root(
    store: &SqliteStore,
    rows: &[ProjectControllerDecision],
) -> Result<Vec<Running>, DispatchError> {
    let mut by_root: BTreeMap<String, Vec<(pc::EvaluateDecision, String)>> = BTreeMap::new();
    for row in rows.iter().rev() {
        if let Some(ProjectAction::Evaluate { root_id, triggers }) =
            decision_proposal(row).map(|p| p.action)
        {
            let effect = match row.effect_state.as_str() {
                "pending" => pc::DecisionEffect::Pending,
                "applied" => pc::DecisionEffect::Applied,
                "suppressed" => pc::DecisionEffect::Suppressed,
                _ => pc::DecisionEffect::Other,
            };
            let decision = pc::EvaluateDecision {
                id: row.id.clone(),
                triggers,
                effect,
            };
            by_root
                .entry(root_id)
                .or_default()
                .push((decision, row.request_sha256.clone()));
        }
    }
    let mut out = Vec::new();
    for (root_id, decisions) in by_root {
        let Some(root) = root_id
            .parse::<TaskId>()
            .ok()
            .map(|id| store.get(id))
            .transpose()?
            .flatten()
        else {
            continue;
        };
        let events = store.events_for(root.id)?;
        let names = artifact_names(&events);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let plain: Vec<pc::EvaluateDecision> = decisions.iter().map(|(d, _)| d.clone()).collect();
        let states = pc::checkpoint_states(&plain, &names);
        let attempts = states
            .running
            .iter()
            .map(|id| (id.clone(), states.attempts(id)))
            .collect();
        let decisions = states
            .running
            .iter()
            .filter_map(|id| {
                let (d, sha) = decisions.iter().find(|(d, _)| &d.id == id)?;
                Some((id.clone(), d.triggers.clone(), sha.clone()))
            })
            .collect();
        out.push(Running {
            root,
            decisions,
            attempts,
        });
    }
    Ok(out)
}

fn artifact_names(events: &[(u64, Event)]) -> Vec<String> {
    events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ArtifactProduced { artifact, .. } => Some(artifact.name.clone()),
            _ => None,
        })
        .collect()
}

/// D3.1 の入力。root が strategy root でなければ `None`（run を起こさない。D9.1）。
fn request(
    store: &SqliteStore,
    root: &Task,
    decision_id: &str,
    triggers: &[String],
    request_sha256: &str,
    tokens: TreeTokens,
) -> Result<Option<ParentCheckpointRequest>, DispatchError> {
    let Some((_, strategy)) =
        task_core::strategy_reads::strategy_task_creates(store, Some(root.id))?
            .into_iter()
            .next()
    else {
        return Ok(None);
    };
    let mut children = Vec::new();
    for child in store.children(root.id)? {
        let passed = store
            .events_for(child.id)?
            .iter()
            .any(|(_, e)| matches!(e, Event::ReviewerPassed { .. }));
        children.push(ParentCheckpointChild {
            task_id: child.id,
            title: child.title.clone(),
            status: child.status,
            reviewer_passed: passed,
            criteria: Vec::new(),
        });
    }
    let previous = store
        .events_for(root.id)?
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::ArtifactProduced { artifact, .. }
                if pc::evaluation_decision_id(&artifact.name).is_some() =>
            {
                Some(ParentCheckpointPrevious {
                    name: artifact.name,
                    sha256: artifact.sha256,
                })
            }
            _ => None,
        })
        .collect();
    let metrics = ParentCheckpointMetrics {
        children_terminal: children.iter().filter(|c| c.status.is_terminal()).count() as u32,
        children_total: children.len() as u32,
        tokens_used: tokens(store, root.id, None)?,
        max_tokens: root.budget.max_tokens,
    };
    Ok(Some(ParentCheckpointRequest {
        decision_id: decision_id.to_string(),
        root_id: root.id,
        triggers: triggers.to_vec(),
        goal: ParentCheckpointGoal {
            request_sha256: request_sha256.to_string(),
            outcomes: Vec::new(),
        },
        strategy,
        children,
        metrics,
        previous,
        rubric: ParentCheckpointRequest::fixed_rubric(),
    }))
}

/// 起動口の終わりの腕（`SideReviewKind::ParentEvaluation`）: `review.json` の `parent_evaluation` を D4 の
/// `check_evaluation` に通し、`checked`、または `invalid` で試行が上限に達したときだけ最終の記録を書く（D3.4）。
/// `done` で終わらなかった run は節が無いものとして扱う（試行は起動の時点で数えている）。
pub(crate) fn settle(
    d: &Dispatcher,
    root_id: TaskId,
    run_id: &str,
    subject: Subject,
    result: &Result<RunOutcome, AdapterError>,
) -> Result<(), DispatchError> {
    let Subject {
        decision_id,
        max_attempts,
    } = subject;
    let store = d.store();
    let Some(root) = store.get(root_id)? else {
        return Ok(());
    };
    let done = matches!(
        result,
        Ok(RunOutcome {
            terminal: Terminal::Done { .. },
            ..
        })
    );
    let raw = d
        .task_dir(&root)
        .filter(|_| done)
        .map(|dir| task_core::artifacts::artifacts_dir_for(&root, &dir))
        .and_then(|dir| std::fs::read(dir.join(super::REVIEW_FILE_NAME)).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|v| v.get("parent_evaluation").cloned());
    finish(&store, d, &root, &decision_id, run_id, raw, max_attempts)
}

/// 最終の記録を書く（既にあれば何もしない）。`raw` が `None`（節が無い・run が終わらない・上限）なら `invalid`。
fn finish(
    store: &SqliteStore,
    d: &Dispatcher,
    root: &Task,
    decision_id: &str,
    run_id: &str,
    raw: Option<serde_json::Value>,
    max_attempts: u32,
) -> Result<(), DispatchError> {
    let events = store.events_for(root.id)?;
    let names = artifact_names(&events);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    if names
        .iter()
        .any(|n| pc::evaluation_decision_id(n) == Some(decision_id))
    {
        return Ok(());
    }
    let checked = pc::check_evaluation(raw.as_ref(), &facts(store, root, raw.as_ref())?);
    let attempts = pc::checkpoint_states(&[], &names).attempts(decision_id);
    if checked.status == pc::RecordStatus::Invalid && attempts < max_attempts {
        tracing::info!(task_id = %root.id, decision_id, %run_id, attempts, "parent evaluation invalid below the attempt limit; the next tick retries");
        return Ok(());
    }
    let record = pc::ParentEvaluationRecord::new(decision_id, &root.id.to_string(), raw, &checked);
    let name = pc::evaluation_artifact_name(decision_id);
    write_record(
        store,
        d,
        root,
        &name,
        pc::EVALUATION_ARTIFACT_KIND,
        run_id,
        &record,
    )
}

/// D4 の事実。参照は同じ案件の task と、その task の `ArtifactProduced`（name と sha256）だけを実在とする。commit の
/// 実在は確かめない（解決しない参照は支持の根拠に数えない）。`goal_reached` は false（D6 の値を入れるのは未実装）。
fn facts(
    store: &SqliteStore,
    root: &Task,
    raw: Option<&serde_json::Value>,
) -> Result<pc::EvaluationFacts, DispatchError> {
    let mut resolved = Vec::new();
    for r in raw.map(pc::references_to_resolve).unwrap_or_default() {
        let task_id = match &r {
            Reference::Task { task_id } | Reference::Artifact { task_id, .. } => task_id,
            _ => continue,
        };
        let Some(task) = task_id
            .parse::<TaskId>()
            .ok()
            .map(|id| store.get(id))
            .transpose()?
            .flatten()
            .filter(|t| t.project_id == root.project_id)
        else {
            continue;
        };
        let found = match &r {
            Reference::Artifact { name, sha256, .. } => {
                store.events_for(task.id)?.iter().any(|(_, e)| {
                    matches!(e, Event::ArtifactProduced { artifact, .. } if &artifact.name == name && &artifact.sha256 == sha256)
                })
            }
            _ => true,
        };
        if found {
            resolved.push(r);
        }
    }
    let mut children = pc::ChildOutcomes::default();
    for child in store.children(root.id)? {
        match child.status {
            Status::Done => {
                let passed = store
                    .events_for(child.id)?
                    .iter()
                    .any(|(_, e)| matches!(e, Event::ReviewerPassed { .. }));
                if passed {
                    children.done_passed += 1;
                } else {
                    children.done_unpassed += 1;
                }
            }
            Status::Failed => children.failed += 1,
            Status::Cancelled => children.cancelled += 1,
            _ => {}
        }
    }
    Ok(pc::EvaluationFacts {
        resolved,
        goal_reached: false,
        children,
    })
}

/// root の成果物の dir に `name` の JSON を書き、既存の `ArtifactProduced`（`declared: false`）を root に追記する。
/// root の dir が分からなければ書かない（次の tick・次の試行に回す）。
fn write_record(
    store: &SqliteStore,
    d: &Dispatcher,
    root: &Task,
    name: &str,
    kind: &str,
    run_id: &str,
    body: &impl serde::Serialize,
) -> Result<(), DispatchError> {
    let Some(dir) = d.task_dir(root) else {
        tracing::warn!(task_id = %root.id, name, "parent evaluation record deferred: the root directory is unknown");
        return Ok(());
    };
    let text = serde_json::to_string_pretty(body)
        .map_err(|e| task_core::store::StoreError::Invalid(e.to_string()))?;
    let path = task_core::artifacts::artifacts_dir_for(root, &dir).join(name);
    write_file(&path, &text)?;
    let artifact = ArtifactRef {
        name: name.to_string(),
        path: path.display().to_string(),
        sha256: sha256_hex(&text),
        kind: kind.to_string(),
        declared: false,
    };
    store.append_event(
        root.id,
        &Event::ArtifactProduced {
            run_id: run_id.to_string(),
            artifact,
        },
    )?;
    Ok(())
}

fn write_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

/// `holds_side_review` の評価の半分（D2.3 (b)）: `run_id` が、最終の記録の無い判断の最後の試行の記録の run で、
/// その run の `WorkerFinished` がまだ無ければ `true`。root の events だけで決まる。
pub(crate) fn in_flight(events: &[(u64, Event)], run_id: &str) -> bool {
    let mut finals = BTreeSet::new();
    let mut last: BTreeMap<&str, (u32, &str)> = BTreeMap::new();
    let mut finished = false;
    for (_, e) in events {
        match e {
            Event::ArtifactProduced {
                artifact,
                run_id: r,
            } => {
                if let Some(id) = pc::evaluation_decision_id(&artifact.name) {
                    finals.insert(id);
                } else if let Some((id, n)) = pc::attempt_of(&artifact.name)
                    && last.get(id).is_none_or(|(m, _)| n > *m)
                {
                    last.insert(id, (n, r.as_str()));
                }
            }
            Event::WorkerFinished { run_id: r, .. } if r == run_id => finished = true,
            _ => {}
        }
    }
    !finished
        && last
            .iter()
            .any(|(id, (_, r))| *r == run_id && !finals.contains(id))
}
