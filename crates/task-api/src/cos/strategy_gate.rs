//! ADR 2026-11-21-strategy-evidence-contract D3・D4: CoS の `task.create` が任意の欄 `strategy`（戦略の文書の
//! 成果物の参照 `{task_id, name, sha256}`）を持つときだけ、文書を読み、照合の文脈を store から集めて
//! [`task_core::strategy::validate_document`] を実行の意図（[`Intent::Execute`]）で呼ぶ。違反があれば task を
//! 作らずに 422 を返す（code は最初の違反の規則の code、`rules`・`codes`・`violations` に全件）。
//!
//! 文脈の正本は既存のものだけを読む（書かない）: 文書は作った task の `ArtifactProduced` と workspace の file、
//! 今の `Project.request` の sha256、承認済み Roadmap の版、作る task の案件（`request.body.project_id`）、
//! `chat` の参照が引いた発言の今の revision・本文の sha256・撤回（`SqliteStore::chat_cited_message`）、
//! 承認は `decisions` の行と最後の `DecisionAnswered{option, by}`（人は `human:<名前>` だけ。D2.2）。
//! 欄の無い `task.create` はここを通らない（呼び出し側の `operations::create_operation` が欄のあるときだけ呼ぶ）。
//!
//! ADR 2026-11-22-strategy-review-critic D2・D3.7・D4・D5: 文書の検査に続けて、実行の入口の事実（文書を作った task の
//! status、`task_core::strategy_critic::critic_subjects` による Critic の対象、`cos_operations` から導く鎖の版ごとの
//! 実験 task の集合 X(v) とその終端、起票の `execution`・`max_wall_secs`・`strategy.candidate`）を集めて
//! [`task_core::strategy::check_execution`] に渡す。規則はそこにあり、ここには書かない。違反は文書の違反と同じ 422 に並ぶ。

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use task_core::decision::{DecisionRow, DecisionStatus};
use task_core::model::{ArtifactRef, Event, Status, Task, TaskId};
use task_core::project_proposal::sha256_hex;
use task_core::store::{SqliteStore, StoreError};
use task_core::strategy::{
    self, APPROVE_OPTION, ApprovalRecord, ArtifactFacts, ArtifactPointer, CheckContext,
    ExecutionFacts, ExperimentTask, Intent, PreviousVersion, RefStatus, Reference, Rule,
    StrategyDocument, Violation,
};
use task_core::strategy_critic::critic_subjects;
use task_core::{ExecutionMode, Project, ProjectId};

use super::operations::{DispatchEnv, Matched, unprocessable};
use crate::problem::{ApiProblem, store_problem};

/// `strategy` を `task.create` 以外の操作に付けた（D4）。
pub(crate) const STRATEGY_NOT_APPLICABLE: &str = "strategy_not_applicable";

/// `/cos/operations` の欄 `strategy`: 実行する戦略の文書（作った task・成果物の name・記録の sha256）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StrategyRef {
    pub task_id: String,
    pub name: String,
    pub sha256: String,
    /// ADR 2026-11-22-strategy-review-critic D2.2: どの候補の task か（`[a-z0-9-]{1,40}`）。実験段階の版では
    /// open の候補、実行段階の版では省略か `selected` の候補。`cos_operations.payload_json.strategy.candidate` が記録になる。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
}

/// 欄 `strategy` のある要求の検査。通れば `Ok(())`、違反があれば 422。
pub(crate) fn check(
    store: &SqliteStore,
    env: &DispatchEnv,
    matched: &Matched,
    body: &Value,
    strategy: &StrategyRef,
) -> Result<(), ApiProblem> {
    if matched.action != "task.create" {
        return Err(unprocessable(
            STRATEGY_NOT_APPLICABLE,
            format!(
                "strategy is accepted only with task.create, not {}",
                matched.action
            ),
        ));
    }
    let violations = violations(store, env, body, strategy)?;
    refusal(&violations).map_or(Ok(()), Err)
}

fn refusal(violations: &[Violation]) -> Option<ApiProblem> {
    let first = violations.first()?;
    let mut rules: Vec<Rule> = violations.iter().map(|v| v.rule).collect();
    rules.sort();
    rules.dedup();
    let mut codes: Vec<&str> = rules.iter().map(|r| r.code()).collect();
    codes.sort_unstable();
    codes.dedup();
    let detail = violations
        .iter()
        .take(8)
        .map(|v| format!("{}: {}", v.path, v.detail))
        .collect::<Vec<_>>()
        .join("; ");
    Some(
        unprocessable(
            first.rule.code(),
            format!("the strategy cannot be executed: {detail}"),
        )
        .with_extra("rules", &rules)
        .with_extra("codes", &codes)
        .with_extra("violations", violations),
    )
}

fn violation(rule: Rule, path: &str, detail: &str) -> Violation {
    Violation {
        rule,
        path: path.into(),
        detail: detail.into(),
    }
}

fn bare(sha: &str) -> &str {
    sha.strip_prefix("sha256:").unwrap_or(sha)
}

/// 照合の文脈を集めて検査する。参照の文書が読めない段階の違反はそこで返す（検査する文書が無い）。
fn violations(
    store: &SqliteStore,
    env: &DispatchEnv,
    body: &Value,
    strategy: &StrategyRef,
) -> Result<Vec<Violation>, ApiProblem> {
    let project = match body
        .get("project_id")
        .and_then(Value::as_str)
        .and_then(|raw| raw.parse::<ProjectId>().ok())
    {
        Some(id) => store.project_get(id).map_err(store_problem)?,
        None => None,
    };
    let Some(project) = project else {
        return Ok(vec![violation(
            Rule::OtherProject,
            "request.body.project_id",
            "作る task に案件が無い（戦略の文書の案件と一致しない）",
        )]);
    };
    let mut reader = Reader {
        store,
        env,
        target: project.id.to_string(),
        projects: BTreeMap::new(),
    };
    let mut out = Vec::new();
    let Some(source) = reader.task(&strategy.task_id)? else {
        return Ok(vec![violation(
            Rule::SourceMissing,
            "strategy.task_id",
            "文書を作った task が無い",
        )]);
    };
    if source.project_id.map(|p| p.to_string()) != Some(reader.target.clone()) {
        out.push(violation(
            Rule::OtherProject,
            "strategy.task_id",
            "文書を作った task が作る task と別の案件",
        ));
    }
    let produced = reader.produced(source.id)?;
    let Some((run_id, artifact)) = produced
        .iter()
        .find(|(_, a)| a.name == strategy.name && bare(&a.sha256) == bare(&strategy.sha256))
    else {
        out.push(violation(
            Rule::SourceMissing,
            "strategy",
            "文書を作った task にこの name・sha256 の成果物が無い",
        ));
        return Ok(out);
    };
    let Some(bytes) = reader.file(&source, &artifact.path) else {
        out.push(violation(
            Rule::SourceMissing,
            "strategy",
            "成果物の file が workspace に無い",
        ));
        return Ok(out);
    };
    if format!("{:x}", Sha256::digest(&bytes)) != bare(&artifact.sha256) {
        out.push(violation(
            Rule::ContentHash,
            "strategy",
            "成果物の file が記録の後に書き換えられた",
        ));
        return Ok(out);
    }
    let doc: Option<StrategyDocument> = serde_json::from_slice(&bytes).ok();
    let rows = store.decisions_list(None).map_err(store_problem)?;
    let mut ctx = CheckContext {
        intent: Intent::Execute,
        project_id: reader.target.clone(),
        request_sha256: sha256_hex(&project.request),
        roadmap_version: store
            .roadmap_approved(&reader.target)
            .map_err(store_problem)?
            .map(|v| v.version),
        artifact: ArtifactFacts {
            name: artifact.name.clone(),
            sha256: artifact.sha256.clone(),
            run_id: run_id.clone(),
        },
        key_sha256s: produced
            .iter()
            .filter(|(_, a)| a.name == strategy.name)
            .map(|(_, a)| a.sha256.clone())
            .collect(),
        chain: Vec::new(),
        approval: None,
        latest_approved_version: None,
        references: Vec::new(),
    };
    if let Some(doc) = &doc {
        ctx.chain = reader.chain(doc)?;
        ctx.approval = approval(store, &rows, source.id, doc, &strategy.sha256)?;
        for subject in reader.human_approvals(&rows, &doc.strategy_id)? {
            ctx.latest_approved_version = ctx.latest_approved_version.max(Some(subject.version));
            if subject.version == doc.version {
                ctx.key_sha256s.push(subject.sha256);
            }
        }
        ctx.references = reader.references(&project, doc)?;
    }
    out.extend(strategy::validate_document(&bytes, &ctx));
    if let Some(doc) = &doc {
        let facts = execution_facts(store, &source, doc, &ctx.chain, body, strategy)?;
        out.extend(strategy::check_execution(doc, &facts));
    }
    Ok(out)
}

/// ADR 2026-11-22-strategy-review-critic D5: 実行の入口の事実（読むだけ。規則は `check_execution`）。
fn execution_facts(
    store: &SqliteStore,
    source: &Task,
    doc: &StrategyDocument,
    chain: &[PreviousVersion],
    body: &Value,
    strategy: &StrategyRef,
) -> Result<ExecutionFacts, ApiProblem> {
    // D3.7: Critic の対象は文書を作った task の全 run の成果物から導く（衝突した版は対象でない）。
    let events = store.events_for(source.id).map_err(store_problem)?;
    let critiqued = critic_subjects(&events).iter().any(|s| {
        !s.conflict && s.name == strategy.name && bare(&s.sha256) == bare(&strategy.sha256)
    });
    // D2.3: 参照した版と鎖の前の版（`chain[i]` は `version - 1 - i`）の X(v)。
    let mut versions = vec![(
        doc.version,
        strategy.name.as_str(),
        strategy.sha256.as_str(),
    )];
    for (i, prev) in chain.iter().enumerate() {
        let Some(version) = u32::try_from(i + 1)
            .ok()
            .and_then(|back| doc.version.checked_sub(back))
        else {
            break;
        };
        versions.push((version, &prev.pointer.name, &prev.pointer.sha256));
    }
    let operations = strategy_operations(store)?;
    let mut experiments = Vec::new();
    for (version, name, sha256) in versions {
        let of_version = operations
            .iter()
            .filter(|op| op.name == name && bare(&op.sha256) == bare(sha256));
        for op in of_version {
            let task_id = op.task_id.clone();
            let terminal = match task_id.parse::<TaskId>() {
                Ok(id) => store.get(id).map_err(store_problem)?.is_some_and(|t| {
                    matches!(t.status, Status::Done | Status::Failed | Status::Cancelled)
                }),
                Err(_) => false,
            };
            experiments.push(ExperimentTask {
                version,
                task_id,
                candidate: op.candidate.clone(),
                terminal,
            });
        }
    }
    Ok(ExecutionFacts {
        source_done: source.status == Status::Done,
        critiqued,
        experiments,
        execution: body
            .get("execution")
            .and_then(|v| serde_json::from_value::<ExecutionMode>(v.clone()).ok()),
        candidate: strategy.candidate.clone(),
        max_wall_secs: body.get("max_wall_secs").and_then(Value::as_u64),
    })
}

/// `strategy` 付きで作った task の操作（`cos_operations` の 1 行）。
struct StrategyOperation {
    name: String,
    sha256: String,
    task_id: String,
    candidate: Option<String>,
}

/// D2.3 の X(v) の元: `cos_operations` の `task.create`・`applied` の行のうち `payload_json.strategy` のあるものの全件
/// （版ごとの X は呼び出し側が name と sha256 で選ぶ。鎖の版の数だけ走査しない）。子 2 D3.4 の「ある task がどの戦略を
/// 実行するか」と同じ照会で、表も索引も足さない。task-core の store に cos_operations を任意の条件で読む公開の口が無いので、
/// `write_tx` の中で SELECT だけを行う（書かない。即座に commit する）。
fn strategy_operations(store: &SqliteStore) -> Result<Vec<StrategyOperation>, ApiProblem> {
    store
        .write_tx(|tx| -> Result<Vec<StrategyOperation>, StoreError> {
            let mut stmt = tx.prepare(
                "SELECT json_extract(payload_json, '$.strategy.name'), \
                 json_extract(payload_json, '$.strategy.sha256'), target_id, \
                 json_extract(payload_json, '$.strategy.candidate') FROM cos_operations \
                 WHERE action = 'task.create' AND state = 'applied' \
                 AND json_extract(payload_json, '$.strategy.name') IS NOT NULL \
                 ORDER BY created_at, id",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(StrategyOperation {
                        name: r.get(0)?,
                        sha256: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        task_id: r.get(2)?,
                        candidate: r.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .map_err(store_problem)
}

/// 最後の `DecisionAnswered` の `(option, by)`。
fn last_answer(
    store: &SqliteStore,
    row: &DecisionRow,
) -> Result<Option<(String, String)>, ApiProblem> {
    Ok(store
        .events_for(row.task_id)
        .map_err(store_problem)?
        .into_iter()
        .rev()
        .find_map(|(_, e)| match e {
            Event::DecisionAnswered { id, option, by, .. } if id == row.id => Some((option, by)),
            _ => None,
        }))
}

/// D2.2 の 1・2: 文書を作った task の、この版の key の決定（問いの sha256 が文書と同じものを先に選ぶ）。
fn approval(
    store: &SqliteStore,
    rows: &[DecisionRow],
    source: TaskId,
    doc: &StrategyDocument,
    sha256: &str,
) -> Result<Option<ApprovalRecord>, ApiProblem> {
    let key = strategy::approval_decision_key(doc.version);
    let mut own: Vec<&DecisionRow> = rows
        .iter()
        .filter(|r| r.task_id == source && r.key == key)
        .collect();
    own.sort_by_key(|r| {
        strategy::approval_subject(&r.request.question).is_none_or(|s| s.sha256 != bare(sha256))
    });
    let Some(row) = own.first() else {
        return Ok(None);
    };
    Ok(Some(ApprovalRecord {
        key: row.key.clone(),
        question: row.request.question.clone(),
        status: row.status,
        last_answer: last_answer(store, row)?,
    }))
}

/// store と workspace を読むだけの照会（task の案件は 1 回だけ引く）。
struct Reader<'a> {
    store: &'a SqliteStore,
    env: &'a DispatchEnv,
    /// 作る task の案件。
    target: String,
    projects: BTreeMap<TaskId, Option<String>>,
}

impl Reader<'_> {
    fn task(&self, raw: &str) -> Result<Option<Task>, ApiProblem> {
        let Ok(id) = raw.parse::<TaskId>() else {
            return Ok(None);
        };
        self.store.get(id).map_err(store_problem)
    }

    /// task の案件（task が無ければ `None`、案件の無い task は `Some(None)`）。
    fn task_project(&mut self, id: TaskId) -> Result<Option<Option<String>>, ApiProblem> {
        if let Some(p) = self.projects.get(&id) {
            return Ok(Some(p.clone()));
        }
        let Some(task) = self.store.get(id).map_err(store_problem)? else {
            return Ok(None);
        };
        let project = task.project_id.map(|p| p.to_string());
        self.projects.insert(id, project.clone());
        Ok(Some(project))
    }

    fn produced(&self, task: TaskId) -> Result<Vec<(String, ArtifactRef)>, ApiProblem> {
        Ok(self
            .store
            .events_for(task)
            .map_err(store_problem)?
            .into_iter()
            .filter_map(|(_, e)| match e {
                Event::ArtifactProduced { run_id, artifact } => Some((run_id, artifact)),
                _ => None,
            })
            .collect())
    }

    /// 記録された成果物の file の byte 列（workspace の外・無い file は `None`）。
    fn file(&self, task: &Task, recorded: &str) -> Option<Vec<u8>> {
        let roots = &self.env.api.inner.view.workspace_roots;
        let dir = crate::files::task_local_dir(self.store, roots, task).ok()?;
        let ws = crate::files::canonical_workspace(&dir).ok()?;
        let path = crate::files::resolve_artifact(&ws, recorded).ok()?;
        std::fs::read(path).ok()
    }

    /// task の、name と sha256 が一致し、file の今の byte 列もその sha256 の成果物。
    fn artifact_bytes(
        &self,
        pointer: &ArtifactPointer,
    ) -> Result<Option<(Task, Vec<u8>)>, ApiProblem> {
        let Some(task) = self.task(&pointer.task_id)? else {
            return Ok(None);
        };
        let found = self
            .produced(task.id)?
            .into_iter()
            .find(|(_, a)| a.name == pointer.name && bare(&a.sha256) == bare(&pointer.sha256));
        let Some((_, artifact)) = found else {
            return Ok(None);
        };
        let Some(bytes) = self.file(&task, &artifact.path) else {
            return Ok(None);
        };
        let current = format!("{:x}", Sha256::digest(&bytes));
        Ok((current == bare(&pointer.sha256)).then_some((task, bytes)))
    }

    /// `supersedes` を辿った前の版（`chain[0]` が `version - 1`。引けない版で止める。上限 [`strategy::VERSION_MAX`]）。
    fn chain(&mut self, doc: &StrategyDocument) -> Result<Vec<PreviousVersion>, ApiProblem> {
        let mut out = Vec::new();
        let mut next = doc.supersedes.clone();
        while let Some(pointer) = next.take() {
            if out.len() >= strategy::VERSION_MAX as usize {
                break;
            }
            let (bytes, task_project_id) = match self.artifact_bytes(&pointer)? {
                Some((task, bytes)) => (Some(bytes), task.project_id.map(|p| p.to_string())),
                None => (None, None),
            };
            next = bytes
                .as_deref()
                .and_then(|b| serde_json::from_slice::<StrategyDocument>(b).ok())
                .and_then(|d| d.supersedes);
            out.push(PreviousVersion {
                pointer,
                bytes,
                task_project_id,
            });
        }
        Ok(out)
    }

    /// 作る task の案件の、同じ `strategy_id` の決定のうち、D2.2 の 3〜5 を満たす人の承認（問いの文書）。
    fn human_approvals(
        &mut self,
        rows: &[DecisionRow],
        strategy_id: &str,
    ) -> Result<Vec<strategy::ApprovalSubject>, ApiProblem> {
        let mut out = Vec::new();
        for row in rows.iter().filter(|r| r.status == DecisionStatus::Answered) {
            let Some(subject) = strategy::approval_subject(&row.request.question) else {
                continue;
            };
            if subject.strategy_id != strategy_id
                || row.key != strategy::approval_decision_key(subject.version)
                || self.task_project(row.task_id)?.flatten().as_deref() != Some(&self.target)
            {
                continue;
            }
            if let Some((option, by)) = last_answer(self.store, row)?
                && option == APPROVE_OPTION
                && task_core::is_human_principal(&by)
            {
                out.push(subject);
            }
        }
        Ok(out)
    }

    /// 文書の参照（`url` を除く）を引いた結果（D1.2 の表）。
    fn references(
        &mut self,
        project: &Project,
        doc: &StrategyDocument,
    ) -> Result<Vec<(Reference, RefStatus)>, ApiProblem> {
        let all = doc
            .research_question
            .references
            .iter()
            .chain(doc.evidence.iter().flat_map(|e| &e.references))
            .take(strategy::REFS_TOTAL_MAX);
        let mut out: Vec<(Reference, RefStatus)> = Vec::new();
        for r in all {
            if matches!(r, Reference::Url { .. }) || out.iter().any(|(seen, _)| seen == r) {
                continue;
            }
            let status = self.lookup(project, r)?;
            out.push((r.clone(), status));
        }
        Ok(out)
    }

    fn lookup(&mut self, project: &Project, r: &Reference) -> Result<RefStatus, ApiProblem> {
        let found = |project_id: Option<String>| RefStatus::Found { project_id };
        Ok(match r {
            Reference::Url { .. } => found(None),
            Reference::Commit { repo, .. } => {
                let repos = self.store.repo_list(project.id).map_err(store_problem)?;
                if repos.iter().any(|row| &row.name == repo) {
                    found(Some(self.target.clone()))
                } else {
                    RefStatus::Missing
                }
            }
            Reference::Task { task_id } => match task_id.parse::<TaskId>() {
                Ok(id) => self.task_project(id)?.map_or(RefStatus::Missing, found),
                Err(_) => RefStatus::Missing,
            },
            Reference::Artifact {
                task_id,
                name,
                sha256,
            } => {
                let pointer = ArtifactPointer {
                    task_id: task_id.clone(),
                    name: name.clone(),
                    sha256: sha256.clone(),
                };
                match self.artifact_bytes(&pointer)? {
                    Some((task, _)) => found(task.project_id.map(|p| p.to_string())),
                    None => RefStatus::Missing,
                }
            }
            // ADR 2026-11-21-chat-discussion-mode D6: 引いた発言の今の事実。状態（edited・retracted・missing）は
            // task_core::strategy が要約の引用と同じ関数で導く。
            Reference::Chat(c) => {
                match self.store.chat_cited_message(&c.thread_id, &c.message_id) {
                    Ok(Some(m)) => RefStatus::Cited(m),
                    Ok(None) | Err(_) => RefStatus::Missing,
                }
            }
            Reference::Knowledge { path, .. } => self.knowledge(project, path),
        })
    }

    /// page があり、front matter の scope が `project:<slug または id>` ならその案件、それ以外の scope は案件なし。
    fn knowledge(&self, project: &Project, path: &str) -> RefStatus {
        let Ok(root) = self.env.kb_root.as_ref() else {
            return RefStatus::Missing;
        };
        let relative = Path::new(path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return RefStatus::Missing;
        }
        let Some(raw) = task_ops::knowledge::read_page(root, path) else {
            return RefStatus::Missing;
        };
        let (front, _) = task_core::knowledge::front_matter(&raw);
        let project_id = front
            .scope
            .as_deref()
            .and_then(|scope| scope.trim().strip_prefix("project:"))
            .map(|x| {
                if x == project.kb_slug() || x == project.id.to_string() {
                    self.target.clone()
                } else {
                    format!("project:{x}")
                }
            });
        RefStatus::Found { project_id }
    }
}

#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod tests;
