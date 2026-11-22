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

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use task_core::decision::{DecisionRow, DecisionStatus};
use task_core::model::{ArtifactRef, Event, Task, TaskId};
use task_core::project_proposal::sha256_hex;
use task_core::store::SqliteStore;
use task_core::strategy::{
    self, APPROVE_OPTION, ApprovalRecord, ArtifactFacts, ArtifactPointer, CheckContext, Intent,
    PreviousVersion, RefStatus, Reference, Rule, StrategyDocument, Violation,
};
use task_core::{Project, ProjectId};

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
    Ok(out)
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
mod tests;
