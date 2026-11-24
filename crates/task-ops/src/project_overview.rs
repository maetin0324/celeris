//! ADR 2026-11-23-project-observability-publish D1: 案件の概要 `celeris.project-overview/1`（読み取り専用）。
//!
//! 14 項目（D1.3）を既存の行と成果物から読むたびに導く。どの表にも書かず、cache も持たない（D5）。
//! 既存の GET（project detail・roadmap・controller）にある欄は写さず、導いた印・強い警告・参照の id だけを返す（D1.1）。
//! 戦略の版と manifest の一覧は `decisions` の key から引き、案件の全 task の成果物は走査しない（D1.5）。
//! manifest の検査と署名の状態は `task_api::publish::manifest_status` が持つので、呼び出し側が [`ManifestFacts`] を渡す。
//! 実験の task（X(v)）の drill-down（最後の run の `log`・`.ipynb` の `notebook`・取り込みの `git`）は、その task の
//! `runs` の索引と event から読む（D1.4）。

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};
use task_core::ProjectId;
use task_core::chat::{ChatMessageQuery, ChatThreadQuery};
use task_core::chat_citation::ChatCitationState;
use task_core::decision::{DecisionRow, DecisionStatus};
use task_core::local_machine::WorkspaceRoots;
use task_core::model::{ArtifactRef, Event, Status, Task, TaskId};
use task_core::parent_checkpoint::{
    self as pc, Next, ParentEvaluationRecord, RecordStatus, Verdict,
};
use task_core::project_proposal::{self as pp, ProjectAction};
use task_core::proposal_review_wire::ProposalReviewState;
use task_core::publish::{self, SignoffState};
use task_core::store::{ListFilter, ListOrder, SqliteStore};
use task_core::strategy::{
    self, ArtifactFacts, CandidateState, CheckContext, Confidence, Intent, Reference, Rule, Stance,
    StrategyDocument,
};

use crate::OpsError;
use crate::project_snapshot::{artifact_bytes, parse_task_id};

pub const SCHEMA: &str = "celeris.project-overview/1";
/// D1.2: 1 項目の `entries` の上限。
pub const ENTRIES_MAX: usize = 50;
/// D1.3 の 14 項目の key（この順で返す）。
pub const ITEM_KEYS: [&str; 14] = [
    "original_goal",
    "approved_strategy",
    "candidates",
    "hypotheses",
    "verification",
    "refutations",
    "latest_findings",
    "discussion_sources",
    "experiments_and_tasks",
    "budget_spent",
    "next_verification",
    "pending_human_decisions",
    "risks",
    "publish_readiness",
];
/// 案件の task を読む上限（`GET /projects/{id}` の一覧と同じ照会）。
const TASKS_MAX: usize = 10_000;
/// 評価の判断を探す controller の判断の数（新しい順）。
const CONTROLLER_DECISIONS_MAX: usize = 500;
const THREAD_PAGE: u32 = 100;

/// D1.2 の印（4 語。子 2 の `confidence` と同じ語）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ProjectOverviewMark")]
pub enum Mark {
    Confirmed,
    Estimated,
    NeedsDecision,
    Contradicted,
}

impl Mark {
    fn of(c: Confidence) -> Self {
        match c {
            Confidence::Confirmed => Self::Confirmed,
            Confidence::Estimated => Self::Estimated,
            Confidence::NeedsDecision => Self::NeedsDecision,
            Confidence::Contradicted => Self::Contradicted,
        }
    }

    fn strong(self) -> bool {
        matches!(self, Self::NeedsDecision | Self::Contradicted)
    }

    fn code(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Estimated => "estimated",
            Self::NeedsDecision => "needs_decision",
            Self::Contradicted => "contradicted",
        }
    }
}

/// D1.4 の drill-down の link（7 種）。Web は kind ごとに既存の画面へ移る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[schemars(rename = "ProjectOverviewLink")]
pub enum Link {
    Task {
        task_id: String,
    },
    Artifact {
        task_id: String,
        name: String,
        sha256: String,
    },
    Notebook {
        task_id: String,
        name: String,
    },
    Log {
        task_id: String,
        run_id: String,
    },
    Git {
        repo: String,
        sha: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },
    Literature {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha256: Option<String>,
    },
    Chat {
        thread_id: String,
        message_id: String,
        revision: u64,
        sha256: String,
        /// 引用の今の状態（`current`・`edited`・`retracted`・`missing`）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<ChatCitationState>,
    },
}

/// 項目の 1 件。`detail` は項目ごとの欄（D1.3 の「概要が返すもの」）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[schemars(rename = "ProjectOverviewEntry")]
pub struct Entry {
    pub id: String,
    pub mark: Mark,
    pub detail: Value,
    pub links: Vec<Link>,
}

/// 14 項目の 1 つ。記録が無ければ `recorded: false` で `mark` は無い（D1.2）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[schemars(rename = "ProjectOverviewItem")]
pub struct Item {
    pub key: String,
    pub recorded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<Mark>,
    pub entries: Vec<Entry>,
}

/// D1.2 の警告。文は Web が code から組む。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(rename = "ProjectOverviewWarning")]
pub struct Warning {
    pub code: String,
    pub item: String,
    pub strong: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    pub links: Vec<Link>,
}

/// `GET /api/v1/projects/{id}/overview` の応答（D1.1）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ProjectOverview {
    pub schema: String,
    pub project_id: String,
    pub generated_at: String,
    pub request_sha256: String,
    pub items: Vec<Item>,
    pub warnings: Vec<Warning>,
    pub truncated: Vec<String>,
}

/// O14 の入力: manifest の 1 版の検査と署名の状態（`task_api::publish::manifest_status` から写す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFacts {
    pub name: String,
    pub sha256: String,
    pub signoff: SignoffState,
    pub violations: Vec<String>,
    /// `manifest_confidential`・`manifest_blocking_question`。
    pub strong: Vec<String>,
    pub file_matches_record: bool,
}

/// manifest を作った task・manifest の id・版から [`ManifestFacts`] を引く関数（呼び出し側が渡す）。
pub type ManifestReader<'a> = dyn FnMut(TaskId, &str, u32) -> Option<ManifestFacts> + 'a;

/// 戦略の 1 系列（文書を作った task と strategy_id）の、概要に出す版。
struct Series {
    task_id: String,
    strategy_id: String,
    approved: Option<(u32, String, String, String, String)>,
    pending: Vec<(u32, String)>,
    doc: Option<(StrategyDocument, String, bool)>,
    stale: Vec<String>,
}

struct Out {
    items: Vec<Item>,
    warnings: Vec<Warning>,
}

impl Out {
    fn push(&mut self, item: usize, entry: Entry) {
        self.items[item].entries.push(entry);
    }

    fn warn(&mut self, item: usize, code: &str, strong: bool, entry: Option<&str>, links: &[Link]) {
        self.warnings.push(Warning {
            code: code.to_string(),
            item: ITEM_KEYS[item].to_string(),
            strong,
            entry: entry.map(str::to_string),
            links: links.to_vec(),
        });
    }
}

/// 読み取りの文脈（案件の task・thread と実験の task の drill-down）。別の案件の資料を link にしない（D1.4）。
struct Scope<'a> {
    tasks: &'a BTreeMap<String, Task>,
    threads: &'a BTreeSet<String>,
    /// X(v) の task ごとの drill-down（[`experiment_drills`]）。
    drills: BTreeMap<String, Drill>,
}

/// D1.4: 実験の task（X(v)）1 件の drill-down の元。
struct Drill {
    /// 最後の run（`runs` の索引の `started_at` の順）。run が無ければ `None`。
    run_id: Option<String>,
    /// name が `.ipynb` で終わる成果物の `notebook` の link（[`artifact_link`] が作る。初出の順）。
    notebooks: Vec<Link>,
    /// 最後の `IntegrationMerged` の repo の名前と head の sha。
    merged: Option<(String, String)>,
}

impl Scope<'_> {
    fn has_task(&self, id: &str) -> bool {
        self.tasks.contains_key(id)
    }

    /// `repo` の `sha` を取り込んだ実験の task（`git` の link の `task_id`）。
    fn integrated_by(&self, repo: &str, sha: &str) -> Option<String> {
        self.drills.iter().find_map(|(id, d)| {
            d.merged
                .as_ref()
                .filter(|(r, s)| r == repo && s.eq_ignore_ascii_case(sha))
                .map(|_| id.clone())
        })
    }

    /// D1.3 の drill-down の列: 実験の task と、その最後の run の `log`。`notebook`（O5）と `git`（O9）は呼び出し側が選ぶ。
    fn experiment_links(&self, id: &str, notebooks: bool, git: bool) -> Vec<Link> {
        let mut links = vec![task_link(id)];
        let Some(d) = self.drills.get(id) else {
            return links;
        };
        if notebooks {
            links.extend(d.notebooks.iter().cloned());
        }
        links.extend(d.run_id.as_ref().map(|run_id| Link::Log {
            task_id: id.to_string(),
            run_id: run_id.clone(),
        }));
        if git && let Some((repo, sha)) = &d.merged {
            links.push(Link::Git {
                repo: repo.clone(),
                sha: sha.clone(),
                task_id: Some(id.to_string()),
            });
        }
        links
    }

    /// 子 2 の参照を link にする。別の案件の task・thread と `https://` でない URL は出さない。
    /// `commit` は実験の task の取り込みの sha と一致すれば、その task の id を持つ。
    fn link(&self, r: &Reference) -> Option<Link> {
        match r {
            Reference::Commit { repo, sha } => Some(Link::Git {
                repo: repo.clone(),
                sha: sha.clone(),
                task_id: self.integrated_by(repo, sha),
            }),
            Reference::Url { url } => url.starts_with("https://").then(|| Link::Literature {
                url: Some(url.clone()),
                path: None,
                sha256: None,
            }),
            Reference::Knowledge { path, sha256 } => Some(Link::Literature {
                url: None,
                path: Some(path.clone()),
                sha256: Some(sha256.clone()),
            }),
            Reference::Task { task_id } => self.has_task(task_id).then(|| Link::Task {
                task_id: task_id.clone(),
            }),
            Reference::Artifact {
                task_id,
                name,
                sha256,
            } => self
                .has_task(task_id)
                .then(|| artifact_link(task_id, name, sha256)),
            Reference::Chat(c) => self.threads.contains(&c.thread_id).then(|| Link::Chat {
                thread_id: c.thread_id.clone(),
                message_id: c.message_id.clone(),
                revision: c.revision,
                sha256: c.sha256.clone(),
                state: None,
            }),
        }
    }

    fn links(&self, refs: &[Reference]) -> Vec<Link> {
        refs.iter().filter_map(|r| self.link(r)).collect()
    }
}

fn artifact_link(task_id: &str, name: &str, sha256: &str) -> Link {
    if name.ends_with(".ipynb") {
        Link::Notebook {
            task_id: task_id.to_string(),
            name: name.to_string(),
        }
    } else {
        Link::Artifact {
            task_id: task_id.to_string(),
            name: name.to_string(),
            sha256: sha256.to_string(),
        }
    }
}

fn task_link(id: &str) -> Link {
    Link::Task {
        task_id: id.to_string(),
    }
}

fn is_internal(r: &Reference) -> bool {
    matches!(
        r,
        Reference::Commit { .. } | Reference::Artifact { .. } | Reference::Task { .. }
    )
}

fn entry(id: impl Into<String>, mark: Mark, detail: Value, links: Vec<Link>) -> Entry {
    Entry {
        id: id.into(),
        mark,
        detail,
        links,
    }
}

/// task の event（1 task 1 回だけ読む）。
struct Events<'a> {
    store: &'a SqliteStore,
    cache: BTreeMap<TaskId, Vec<Event>>,
}

impl Events<'_> {
    fn of(&mut self, id: TaskId) -> Result<&[Event], OpsError> {
        if !self.cache.contains_key(&id) {
            let events = self
                .store
                .events_for(id)?
                .into_iter()
                .map(|(_, e)| e)
                .collect();
            self.cache.insert(id, events);
        }
        Ok(self.cache.get(&id).map_or(&[][..], Vec::as_slice))
    }

    /// 最後の `ArtifactProduced`（name で引く）。
    fn artifact(&mut self, id: TaskId, name: &str) -> Result<Option<ArtifactRef>, OpsError> {
        Ok(self.of(id)?.iter().rev().find_map(|e| match e {
            Event::ArtifactProduced { artifact, .. } if artifact.name == name => {
                Some(artifact.clone())
            }
            _ => None,
        }))
    }
}

/// D1: 案件の概要。案件が無ければ `None`。どの表にも書かない。
pub fn overview(
    store: &SqliteStore,
    roots: &WorkspaceRoots,
    project_id: ProjectId,
    generated_at: &str,
    manifests: &mut ManifestReader<'_>,
) -> Result<Option<ProjectOverview>, OpsError> {
    let Some(project) = store.project_get(project_id)? else {
        return Ok(None);
    };
    let project_key = project_id.to_string();
    let request_sha256 = task_core::project_proposal::sha256_hex(&project.request);
    let filter = ListFilter {
        project_id: Some(project_id),
        ..ListFilter::default()
    };
    let tasks: BTreeMap<String, Task> = store
        .list_page(&filter, ListOrder::CreatedDesc, None, TASKS_MAX)?
        .items
        .into_iter()
        .map(|t| (t.id.to_string(), t))
        .collect();
    let threads = project_threads(store, &project_key)?;
    let mut scope = Scope {
        tasks: &tasks,
        threads: &threads,
        drills: BTreeMap::new(),
    };
    let decisions: Vec<DecisionRow> = store
        .decisions_list(None)?
        .into_iter()
        .filter(|d| scope.has_task(&d.task_id.to_string()))
        .collect();
    let mut events = Events {
        store,
        cache: BTreeMap::new(),
    };
    let mut out = Out {
        items: ITEM_KEYS
            .iter()
            .map(|k| Item {
                key: (*k).to_string(),
                recorded: false,
                mark: None,
                entries: Vec::new(),
            })
            .collect(),
        warnings: Vec::new(),
    };

    // O1 原目的: Project.request の正本の sha256 だけ（本文は `GET /projects/{id}` にある）。
    out.push(
        0,
        entry(
            "goal",
            Mark::Confirmed,
            json!({ "request_sha256": request_sha256 }),
            Vec::new(),
        ),
    );

    // O2 承認戦略: 版の一覧は decisions の key から引く（D1.5）。
    let roadmap_version = store.roadmap_approved(&project_key)?.map(|r| r.version);
    let series = strategy_series(
        store,
        roots,
        &decisions,
        &mut events,
        &scope,
        &request_sha256,
        roadmap_version,
    )?;
    for s in &series {
        o2_approved(&mut out, s);
    }

    // O9 現在の実験と Task: strategy 付きの task.create（X(v) の元）。
    let creates: Vec<(String, task_core::strategy_ref::StrategyRef)> =
        task_core::strategy_reads::strategy_task_creates(store, None)?
            .into_iter()
            .filter(|(target, r)| scope.has_task(target) && scope.has_task(&r.task_id))
            .collect();
    scope.drills = experiment_drills(store, &mut events, &tasks, &creates)?;
    for (target, r) in &creates {
        let (sid, version) =
            task_core::strategy_ref::strategy_artifact(&r.name).unwrap_or((String::new(), 0));
        out.push(
            8,
            entry(
                target.clone(),
                Mark::Confirmed,
                json!({ "task_id": target, "strategy_task_id": r.task_id, "strategy_name": r.name,
                        "strategy_id": sid, "version": version, "candidate": r.candidate }),
                scope.experiment_links(target, false, true),
            ),
        );
    }

    // O7 最新に得た知見（評価の記録）。O6・O11 も読む。
    let evaluation = latest_evaluation(store, roots, &project_key, &scope, &mut events)?;

    for s in &series {
        let Some((doc, name, approved)) = &s.doc else {
            continue;
        };
        let experiments: Vec<&(String, task_core::strategy_ref::StrategyRef)> = creates
            .iter()
            .filter(|(_, r)| r.task_id == s.task_id && &r.name == name)
            .collect();
        o3_to_o6(
            &mut out,
            &scope,
            s,
            doc,
            name,
            *approved,
            &experiments,
            &evaluation,
        );
        o10_budget(store, &mut out, &scope, doc, &experiments)?;
        o11_next(&mut out, &scope, doc, &experiments);
    }
    if let Some(ev) = &evaluation {
        o7_findings(&mut out, &scope, ev);
    }

    // O8 議論の出典・O12 の案。
    let pending_proposals = o8_discussion(store, &mut out, &threads)?;

    // O12 人の判断待ち。
    o12_pending(&mut out, &decisions, pending_proposals);

    // O14 公開の準備度: manifest の一覧も decisions の key から引く（D1.5）。
    o14_publish(&mut out, &decisions, &scope, manifests);

    // 規則 1: needs_decision・contradicted の印は強い警告（O3 の却下と O13 は除く）。
    rule_one_warnings(&mut out);

    // O13 危険。
    let blocked = store
        .project_controller_state(&project_key)?
        .is_some_and(|s| s.phase == task_core::project_wake::ControllerPhase::Blocked);
    if blocked {
        out.warn(12, "controller_blocked", false, None, &[]);
    }
    o13_risks(&mut out);

    let mut truncated = Vec::new();
    for item in &mut out.items {
        if item.entries.len() > ENTRIES_MAX {
            let cut = item.entries.len() - ENTRIES_MAX;
            item.entries.drain(..cut);
            truncated.push(item.key.clone());
        }
        item.recorded = !item.entries.is_empty();
        item.mark = item.entries.iter().map(|e| e.mark).max();
    }
    // O12: 1 件以上なら要判断（D1.3）。O13 は下の規則。
    Ok(Some(ProjectOverview {
        schema: SCHEMA.to_string(),
        project_id: project_key,
        generated_at: generated_at.to_string(),
        request_sha256,
        items: out.items,
        warnings: out.warnings,
        truncated,
    }))
}

/// D1.4: X(v) の task ごとの drill-down。run は `runs` の索引の最後、`.ipynb` は `ArtifactProduced`、取り込みは最後の
/// `IntegrationMerged`（delivery と sweep が取り込みを書く event。repo の名前は task の `repos` から引く）。
fn experiment_drills(
    store: &SqliteStore,
    events: &mut Events<'_>,
    tasks: &BTreeMap<String, Task>,
    creates: &[(String, task_core::strategy_ref::StrategyRef)],
) -> Result<BTreeMap<String, Drill>, OpsError> {
    let mut out = BTreeMap::new();
    for (target, _) in creates {
        let Some(task) = tasks.get(target).filter(|_| !out.contains_key(target)) else {
            continue;
        };
        let run_id = store.runs_for_task(task.id)?.pop().map(|r| r.run_id);
        let mut notebooks: Vec<Link> = Vec::new();
        let mut merged = None;
        for e in events.of(task.id)? {
            match e {
                Event::ArtifactProduced { artifact, .. } if artifact.name.ends_with(".ipynb") => {
                    let link = artifact_link(target, &artifact.name, &artifact.sha256);
                    if !notebooks.contains(&link) {
                        notebooks.push(link);
                    }
                }
                Event::IntegrationMerged {
                    repo_id, head_sha, ..
                } => {
                    let repo = task
                        .repos
                        .iter()
                        .find(|r| r.repo_id == *repo_id)
                        .map_or_else(|| repo_id.to_string(), |r| r.name.clone());
                    merged = Some((repo, head_sha.clone()));
                }
                _ => {}
            }
        }
        out.insert(
            target.clone(),
            Drill {
                run_id,
                notebooks,
                merged,
            },
        );
    }
    Ok(out)
}

/// O12 人の判断待ち: 案件の task の未回答の決定と、採否の無い chat の案。
fn o12_pending(out: &mut Out, decisions: &[DecisionRow], pending_proposals: Vec<Link>) {
    for d in decisions
        .iter()
        .filter(|d| d.status == DecisionStatus::Open)
    {
        let kind = if d.key.starts_with("strategy-approve-v") {
            "strategy-approve"
        } else if publish::is_signoff_key(&d.key) {
            "publish-signoff"
        } else {
            "other"
        };
        let tid = d.task_id.to_string();
        out.push(
            11,
            entry(
                d.id.clone(),
                Mark::NeedsDecision,
                json!({ "decision_id": d.id, "task_id": tid, "key_kind": kind }),
                vec![task_link(&tid)],
            ),
        );
    }
    for link in pending_proposals {
        let Link::Chat { message_id, .. } = &link else {
            continue;
        };
        out.push(
            11,
            entry(
                message_id.clone(),
                Mark::NeedsDecision,
                json!({ "message_id": message_id, "key_kind": "chat_proposal" }),
                vec![link.clone()],
            ),
        );
    }
}

/// 案件の thread（`chat_threads.project_id`）。
fn project_threads(store: &SqliteStore, project: &str) -> Result<BTreeSet<String>, OpsError> {
    let mut out = BTreeSet::new();
    let mut before = None;
    loop {
        let page = store
            .chat_thread_list(&ChatThreadQuery {
                before: before.take(),
                limit: Some(THREAD_PAGE),
                ..ChatThreadQuery::default()
            })
            .map_err(|e| OpsError::Validation(e.to_string()))?;
        out.extend(
            page.items
                .into_iter()
                .filter(|t| t.project_id.as_deref() == Some(project))
                .map(|t| t.id),
        );
        match page.next_cursor {
            Some(c) => before = Some(c),
            None => return Ok(out),
        }
    }
}

/// O2 の入力: `strategy-approve-v<n>` の決定から系列ごとの承認済みの最新の版と回答待ちの版を導き、文書を 1 件読む。
fn strategy_series(
    store: &SqliteStore,
    roots: &WorkspaceRoots,
    decisions: &[DecisionRow],
    events: &mut Events<'_>,
    scope: &Scope<'_>,
    request_sha256: &str,
    roadmap_version: Option<i64>,
) -> Result<Vec<Series>, OpsError> {
    let mut by: BTreeMap<(String, String), Series> = BTreeMap::new();
    for d in decisions
        .iter()
        .filter(|d| d.key.starts_with("strategy-approve-v"))
    {
        let Some(subject) = strategy::approval_subject(&d.request.question) else {
            continue;
        };
        if strategy::approval_decision_key(subject.version) != d.key {
            continue;
        }
        let tid = d.task_id.to_string();
        let s = by
            .entry((tid.clone(), subject.strategy_id.clone()))
            .or_insert_with(|| Series {
                task_id: tid,
                strategy_id: subject.strategy_id.clone(),
                approved: None,
                pending: Vec::new(),
                doc: None,
                stale: Vec::new(),
            });
        match d.status {
            DecisionStatus::Open => s.pending.push((subject.version, d.id.clone())),
            DecisionStatus::Answered => {
                // 子 2 D2.2 の承認の照合（option と人の主体）をそのまま使う（規約 2）。
                let answer = d.last_answer();
                if let Some(by) = strategy::approving_human(answer.as_ref())
                    && s.approved.as_ref().is_none_or(|a| a.0 < subject.version)
                {
                    s.approved = Some((
                        subject.version,
                        d.id.clone(),
                        by.to_string(),
                        d.answered_at.clone().unwrap_or_default(),
                        subject.sha256.clone(),
                    ));
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for mut s in by.into_values() {
        let latest_approved = s.approved.as_ref().map(|a| a.0);
        let shown = latest_approved.or_else(|| s.pending.iter().map(|p| p.0).max());
        if let (Some(version), Some(tid)) = (shown, parse_task_id(&s.task_id))
            && let Some(task) = scope.tasks.get(&s.task_id)
        {
            let name = task_core::strategy_ref::artifact_name(&s.strategy_id, version);
            if let Some(artifact) = events.artifact(tid, &name)?
                && let Some(bytes) = artifact_bytes(store, roots, task, &artifact)
            {
                // 子 2 D3.3 の照合の関数（`validate_document`）の stale と content hash だけを使う（規約 2）。
                let ctx = CheckContext {
                    intent: Intent::Record,
                    project_id: s.task_id.clone(),
                    request_sha256: request_sha256.to_string(),
                    roadmap_version,
                    artifact: ArtifactFacts {
                        name: name.clone(),
                        sha256: artifact.sha256.clone(),
                        run_id: String::new(),
                    },
                    key_sha256s: vec![artifact.sha256.clone()],
                    chain: Vec::new(),
                    approval: None,
                    latest_approved_version: latest_approved,
                    references: Vec::new(),
                };
                let mut stale: Vec<String> = strategy::validate_document(&bytes, &ctx)
                    .into_iter()
                    .filter(|v| matches!(v.rule, Rule::Stale | Rule::ContentHash))
                    .map(|v| v.path)
                    .collect();
                if let Some(a) = &s.approved
                    && a.4.strip_prefix("sha256:").unwrap_or(&a.4) != artifact.sha256
                {
                    stale.push("approval.sha256".into());
                }
                stale.sort();
                stale.dedup();
                s.stale = stale;
                if let Ok(doc) = serde_json::from_slice::<StrategyDocument>(&bytes) {
                    s.doc = Some((doc, name, latest_approved.is_some()));
                }
            } else {
                s.stale = vec!["artifact".into()];
            }
        }
        out.push(s);
    }
    Ok(out)
}

fn o2_approved(out: &mut Out, s: &Series) {
    let name = s
        .approved
        .as_ref()
        .map(|a| task_core::strategy_ref::artifact_name(&s.strategy_id, a.0));
    let mut links = vec![task_link(&s.task_id)];
    if let (Some(name), Some(a)) = (&name, &s.approved) {
        links.insert(0, artifact_link(&s.task_id, name, &a.4));
    }
    let pending: Vec<u32> = s.pending.iter().map(|p| p.0).collect();
    let mark = match &s.approved {
        Some(_) if !s.stale.is_empty() => Mark::Contradicted,
        Some(_) => Mark::Confirmed,
        None => Mark::NeedsDecision,
    };
    let id = format!("{}/{}", s.task_id, s.strategy_id);
    let detail = json!({
        "task_id": s.task_id,
        "strategy_id": s.strategy_id,
        "approved": s.approved.as_ref().map(|a| json!({
            "version": a.0, "decision_id": a.1, "approved_by": a.2, "approved_at": a.3,
            "name": name, "sha256": a.4,
        })),
        "pending_versions": pending,
        "stale": s.stale,
    });
    if mark == Mark::Contradicted {
        out.warn(1, "strategy_stale", true, Some(&id), &links);
    }
    out.push(1, entry(id, mark, detail, links));
}

#[allow(clippy::too_many_arguments)]
fn o3_to_o6(
    out: &mut Out,
    scope: &Scope<'_>,
    s: &Series,
    doc: &StrategyDocument,
    name: &str,
    approved: bool,
    experiments: &[&(String, task_core::strategy_ref::StrategyRef)],
    evaluation: &Option<Evaluation>,
) {
    let doc_link = artifact_link(&s.task_id, name, "");
    let doc_links = vec![doc_link.clone()];
    let prefix = format!("{}/v{}", doc.strategy_id, doc.version);
    // O3 候補の比較。
    for c in &doc.candidates {
        let mark = match (approved, c.state, c.selected) {
            (false, _, _) => Mark::NeedsDecision,
            (true, CandidateState::Rejected, _) => Mark::Contradicted,
            (true, _, true) => Mark::Confirmed,
            (true, CandidateState::Open, false) => Mark::Estimated,
        };
        let tasks: Vec<&str> = experiments
            .iter()
            .filter(|(_, r)| r.candidate.as_deref() == Some(c.id.as_str()))
            .map(|(t, _)| t.as_str())
            .collect();
        let mut links = doc_links.clone();
        links.extend(tasks.iter().map(|t| task_link(t)));
        out.push(
            2,
            entry(
                format!("{prefix}/{}", c.id),
                mark,
                json!({ "strategy_id": doc.strategy_id, "version": doc.version, "id": c.id,
                        "title": c.title, "state": c.state, "selected": c.selected,
                        "cost": c.cost, "experiment_budget": c.experiment.budget,
                        "rejected_by": c.rejected_by, "experiment_tasks": tasks }),
                links,
            ),
        );
    }
    // O4 仮説: 文書の confidence をそのまま使う。
    for h in &doc.hypotheses {
        out.push(
            3,
            entry(
                format!("{prefix}/{}", h.id),
                Mark::of(h.confidence),
                json!({ "strategy_id": doc.strategy_id, "version": doc.version, "id": h.id,
                        "statement": h.statement, "confidence": h.confidence }),
                doc_links.clone(),
            ),
        );
    }
    // O5 検証の状況。
    for h in &doc.hypotheses {
        let tasks: Vec<&Task> = doc
            .candidates
            .iter()
            .filter(|c| c.experiment.discriminates.contains(&h.id))
            .flat_map(|c| {
                experiments
                    .iter()
                    .filter(move |(_, r)| r.candidate.as_deref() == Some(c.id.as_str()))
            })
            .filter_map(|(t, _)| scope.tasks.get(t))
            .collect();
        let evidence: Vec<_> = doc
            .evidence
            .iter()
            .filter(|e| e.hypothesis == h.id)
            .collect();
        let cites = |task: &str| {
            evidence.iter().any(|e| {
                e.references.iter().any(|r| match r {
                    Reference::Task { task_id } | Reference::Artifact { task_id, .. } => {
                        task_id == task
                    }
                    _ => false,
                })
            })
        };
        let done = tasks
            .iter()
            .any(|t| t.status == Status::Done && cites(&t.id.to_string()));
        let running = tasks
            .iter()
            .any(|t| !matches!(t.status, Status::Done | Status::Failed | Status::Cancelled));
        let mark = if done {
            Mark::Confirmed
        } else if running {
            Mark::Estimated
        } else {
            Mark::NeedsDecision
        };
        let count = |st: Stance| evidence.iter().filter(|e| e.stance == st).count();
        let ids: Vec<String> = tasks.iter().map(|t| t.id.to_string()).collect();
        out.push(
            4,
            entry(
                format!("{prefix}/{}", h.id),
                mark,
                json!({ "hypothesis": h.id, "experiment_tasks": ids,
                        "support": count(Stance::Support), "refute": count(Stance::Refute),
                        "unknown": count(Stance::Unknown) }),
                ids.iter()
                    .flat_map(|t| scope.experiment_links(t, true, false))
                    .collect(),
            ),
        );
    }
    // O6 反証: 文書の refute の証拠と評価の記録の refuted[]。
    let selected: BTreeSet<&str> = doc
        .candidates
        .iter()
        .filter(|c| c.selected)
        .flat_map(|c| c.hypotheses.iter().map(String::as_str))
        .collect();
    let unapplied = |hyp: &str| {
        doc.hypotheses
            .iter()
            .any(|h| h.id == hyp && h.confidence == Confidence::Confirmed)
            || selected.contains(hyp)
    };
    let mut refutes: Vec<(String, String, String, Vec<Reference>)> = doc
        .evidence
        .iter()
        .filter(|e| e.stance == Stance::Refute)
        .map(|e| {
            (
                format!("{prefix}/{}", e.id),
                e.hypothesis.clone(),
                e.statement.clone(),
                e.references.clone(),
            )
        })
        .collect();
    if let Some(ev) = evaluation
        && let Some(parsed) = &ev.parsed
    {
        refutes.extend(parsed.refuted.iter().enumerate().map(|(i, r)| {
            (
                format!("{}/refuted[{i}]", ev.decision_id),
                r.hypothesis.clone(),
                r.statement.clone(),
                r.references.clone(),
            )
        }));
    }
    for (id, hyp, statement, refs) in refutes {
        let links = scope.links(&refs);
        let mark = if unapplied(&hyp) {
            out.warn(5, "refute_unapplied", true, Some(&id), &links);
            Mark::Contradicted
        } else if refs.iter().any(is_internal) {
            Mark::Confirmed
        } else {
            Mark::Estimated
        };
        out.push(
            5,
            entry(
                id.clone(),
                mark,
                json!({ "evidence_id": id, "hypothesis": hyp, "statement": statement,
                        "references": refs.len() }),
                links,
            ),
        );
    }
}

/// O10: X(v) の task の木の消費（`project_root_totals` と同じ集計）と候補の予算。
fn o10_budget(
    store: &SqliteStore,
    out: &mut Out,
    scope: &Scope<'_>,
    doc: &StrategyDocument,
    experiments: &[&(String, task_core::strategy_ref::StrategyRef)],
) -> Result<(), OpsError> {
    if experiments.is_empty() {
        return Ok(());
    }
    let tasks: Vec<Task> = experiments
        .iter()
        .filter_map(|(t, _)| scope.tasks.get(t).cloned())
        .collect();
    let totals = crate::tree_view::project_root_totals(store, &tasks)?.totals;
    let usd_max: f64 = doc
        .candidates
        .iter()
        .map(|c| c.experiment.budget.usd_max)
        .sum();
    let wall_hours_max: f64 = doc
        .candidates
        .iter()
        .map(|c| c.experiment.budget.wall_hours_max)
        .sum();
    let id = format!("{}/v{}", doc.strategy_id, doc.version);
    let links: Vec<Link> = tasks
        .iter()
        .flat_map(|t| scope.experiment_links(&t.id.to_string(), false, false))
        .collect();
    let mark = if totals.cost_usd > usd_max {
        out.warn(9, "budget_exceeded", true, Some(&id), &links);
        Mark::Contradicted
    } else if !totals.cost_usd_complete {
        out.warn(9, "cost_unknown", true, Some(&id), &links);
        Mark::NeedsDecision
    } else {
        Mark::Estimated
    };
    out.push(
        9,
        entry(
            id,
            mark,
            json!({ "strategy_id": doc.strategy_id, "version": doc.version,
                    "budget": { "usd_max": usd_max, "wall_hours_max": wall_hours_max },
                    "spent": { "tokens": totals.tokens, "list_price_usd": totals.cost_usd,
                               "list_price_complete": totals.cost_usd_complete, "runs": totals.runs },
                    "marks": { "tokens": Mark::Confirmed, "list_price_usd": Mark::Estimated } }),
            links,
        ),
    );
    Ok(())
}

/// O11: task の無い open の候補の実験と、保留した前提の再検討の条件。評価の next は [`o7_findings`] が足す。
fn o11_next(
    out: &mut Out,
    _scope: &Scope<'_>,
    doc: &StrategyDocument,
    experiments: &[&(String, task_core::strategy_ref::StrategyRef)],
) {
    let prefix = format!("{}/v{}", doc.strategy_id, doc.version);
    for c in doc.candidates.iter().filter(|c| {
        c.state == CandidateState::Open
            && !experiments
                .iter()
                .any(|(_, r)| r.candidate.as_deref() == Some(c.id.as_str()))
    }) {
        out.push(
            10,
            entry(
                format!("{prefix}/{}", c.id),
                Mark::Estimated,
                json!({ "source": "candidate_experiment", "statement": c.experiment.content }),
                Vec::new(),
            ),
        );
    }
    for a in &doc.held_assumptions {
        out.push(
            10,
            entry(
                format!("{prefix}/{}", a.id),
                Mark::Estimated,
                json!({ "source": "held_assumption", "statement": a.revisit_when }),
                Vec::new(),
            ),
        );
    }
}

/// O7 の入力: strategy root の最新の評価の記録。
struct Evaluation {
    decision_id: String,
    root_id: String,
    name: String,
    sha256: String,
    record: ParentEvaluationRecord,
    parsed: Option<pc::ParentEvaluation>,
}

fn latest_evaluation(
    store: &SqliteStore,
    roots: &WorkspaceRoots,
    project: &str,
    scope: &Scope<'_>,
    events: &mut Events<'_>,
) -> Result<Option<Evaluation>, OpsError> {
    for d in store.project_controller_decisions(project, None, CONTROLLER_DECISIONS_MAX)? {
        let Ok(proposal) = serde_json::from_str::<pp::ProjectProposal>(&d.proposal_json) else {
            continue;
        };
        let ProjectAction::Evaluate { root_id, .. } = proposal.action else {
            continue;
        };
        let (Some(task), Some(tid)) = (scope.tasks.get(&root_id), parse_task_id(&root_id)) else {
            continue;
        };
        let name = pc::evaluation_artifact_name(&d.id);
        let Some(artifact) = events.artifact(tid, &name)? else {
            continue;
        };
        let Some(record) = artifact_bytes(store, roots, task, &artifact)
            .and_then(|b| serde_json::from_slice::<ParentEvaluationRecord>(&b).ok())
        else {
            continue;
        };
        let parsed = record
            .evaluation
            .clone()
            .and_then(|v| serde_json::from_value(v).ok());
        return Ok(Some(Evaluation {
            decision_id: d.id,
            root_id,
            name,
            sha256: artifact.sha256,
            record,
            parsed,
        }));
    }
    Ok(None)
}

/// O7 最新に得た知見: 3 軸と negative_result を別の entry に分ける（子 6 D7）。
fn o7_findings(out: &mut Out, scope: &Scope<'_>, ev: &Evaluation) {
    let links = vec![
        artifact_link(&ev.root_id, &ev.name, &ev.sha256),
        task_link(&ev.root_id),
    ];
    let e = &ev.record.effective;
    let invalid = ev.record.status == RecordStatus::Invalid || e.verdict == Verdict::Unknown;
    let verdict_mark = if invalid {
        out.warn(6, "evaluation_invalid", true, Some(&ev.decision_id), &links);
        Mark::NeedsDecision
    } else if e.verdict == Verdict::OffTrack {
        out.warn(
            6,
            "evaluation_off_track",
            true,
            Some(&ev.decision_id),
            &links,
        );
        Mark::Contradicted
    } else if e.unsupported > 0 {
        Mark::Estimated
    } else {
        Mark::Confirmed
    };
    let base = |what: &str| format!("{}/{what}", ev.decision_id);
    out.push(
        6,
        entry(
            base("verdict"),
            verdict_mark,
            json!({ "decision_id": ev.decision_id, "root_id": ev.root_id, "status": ev.record.status,
                    "verdict": e.verdict, "next": e.next, "unsupported": e.unsupported }),
            links.clone(),
        ),
    );
    // 3 軸は評価の値のまま出す（goal の軸から Outcome の到達を導かない。D1.3 の注）。
    let axis_mark = if invalid {
        Mark::NeedsDecision
    } else {
        Mark::Estimated
    };
    for (axis, value) in [
        ("execution", json!(e.axes.execution)),
        ("epistemic", json!(e.axes.epistemic)),
        ("goal", json!(e.axes.goal)),
    ] {
        out.push(
            6,
            entry(
                base(&format!("axis/{axis}")),
                axis_mark,
                json!({ "axis": axis, "value": value }),
                links.clone(),
            ),
        );
    }
    out.push(
        6,
        entry(
            base("negative_result"),
            axis_mark,
            json!({ "negative_result": e.negative_result }),
            links.clone(),
        ),
    );
    if e.unsupported > 0 {
        out.warn(
            6,
            "evaluation_unsupported",
            true,
            Some(&ev.decision_id),
            &links,
        );
    }
    if let Some(parsed) = &ev.parsed {
        let claims = parsed
            .measured
            .iter()
            .map(|m| ("measured", m.claim.clone(), &m.references))
            .chain(
                parsed
                    .learned
                    .iter()
                    .map(|l| ("learned", l.statement.clone(), &l.references)),
            );
        for (i, (kind, statement, refs)) in claims.enumerate() {
            let supported = refs.iter().any(is_internal);
            let id = base(&format!("{kind}[{i}]"));
            let links = scope.links(refs);
            let mark = if supported {
                Mark::Confirmed
            } else {
                out.warn(6, "evaluation_unsupported", true, Some(&id), &links);
                Mark::Estimated
            };
            out.push(
                6,
                entry(
                    id,
                    mark,
                    json!({ "kind": kind, "statement": statement, "supported": supported }),
                    links,
                ),
            );
        }
        // O11: 評価の next と不確実性の解き方。
        let next_mark = if parsed.next == Next::AskHuman {
            Mark::NeedsDecision
        } else {
            Mark::Estimated
        };
        out.push(
            10,
            entry(
                base("next"),
                next_mark,
                json!({ "source": "evaluation_next", "statement": parsed.next }),
                links.clone(),
            ),
        );
        for (i, u) in parsed.uncertainty.iter().enumerate() {
            out.push(
                10,
                entry(
                    base(&format!("uncertainty[{i}]")),
                    Mark::Estimated,
                    json!({ "source": "evaluation_uncertainty", "statement": u.resolve_by }),
                    links.clone(),
                ),
            );
        }
    }
}

/// O8 議論の出典: 案件 thread の新しい 50 件の message のうち、外部の文・案・引用を持つもの。採否の無い案の link を返す。
fn o8_discussion(
    store: &SqliteStore,
    out: &mut Out,
    threads: &BTreeSet<String>,
) -> Result<Vec<Link>, OpsError> {
    let mut pending = Vec::new();
    for thread in threads {
        let page = store
            .chat_message_list(
                thread,
                &ChatMessageQuery {
                    limit: Some(u32::try_from(ENTRIES_MAX).unwrap_or(50)),
                    ..ChatMessageQuery::default()
                },
            )
            .map_err(|e| OpsError::Validation(e.to_string()))?;
        for m in page.items.iter().rev() {
            let marks = &m.marks;
            let link = Link::Chat {
                thread_id: m.thread_id.clone(),
                message_id: m.id.clone(),
                revision: m.edit_state.revision,
                sha256: task_core::chat_citation::body_sha256(&m.text),
                state: None,
            };
            if marks.external.is_some() || marks.proposal.is_some() {
                let review_state = marks.proposal.as_ref().map(|p| p.review_state);
                if review_state == Some(ProposalReviewState::Pending) {
                    pending.push(link.clone());
                }
                out.push(
                    7,
                    entry(
                        m.id.clone(),
                        Mark::Estimated,
                        json!({ "message_id": m.id, "thread_id": m.thread_id,
                                "revision": m.edit_state.revision,
                                "sha256": task_core::chat_citation::body_sha256(&m.text),
                                "source_kind": marks.external.as_ref().map(|x| x.source_kind.clone()),
                                "source_label": marks.external.as_ref().map(|x| x.source_label.clone()),
                                "author": m.author, "review_state": review_state,
                                "untrusted": marks.external.is_some() }),
                        vec![link],
                    ),
                );
            }
            for c in &m.cited.citations {
                let state = c.state;
                let cl = Link::Chat {
                    thread_id: c.citation.thread_id.clone(),
                    message_id: c.citation.message_id.clone(),
                    revision: c.citation.revision,
                    sha256: c.citation.sha256.clone(),
                    state: Some(state),
                };
                let links: Vec<Link> = if threads.contains(&c.citation.thread_id) {
                    vec![cl]
                } else {
                    Vec::new()
                };
                let id = format!("{}/{}", m.id, c.path);
                let mark = if state == ChatCitationState::Current {
                    Mark::Estimated
                } else {
                    out.warn(7, "citation_stale", true, Some(&id), &links);
                    Mark::Contradicted
                };
                out.push(
                    7,
                    entry(
                        id,
                        mark,
                        json!({ "message_id": m.id, "path": c.path, "cited_message_id": c.citation.message_id,
                                "revision": c.citation.revision, "state": state }),
                        links,
                    ),
                );
            }
        }
    }
    Ok(pending)
}

/// O14 公開の準備度: `publish-signoff-v<n>` の決定から manifest ごとの最新の版を引き、状態は呼び出し側の検査で導く。
fn o14_publish(
    out: &mut Out,
    decisions: &[DecisionRow],
    scope: &Scope<'_>,
    manifests: &mut ManifestReader<'_>,
) {
    let mut latest: BTreeMap<(String, String), u32> = BTreeMap::new();
    for d in decisions.iter().filter(|d| publish::is_signoff_key(&d.key)) {
        let Some(subject) = publish::signoff_subject(&d.request.question) else {
            continue;
        };
        let v = latest
            .entry((d.task_id.to_string(), subject.manifest_id))
            .or_insert(0);
        *v = (*v).max(subject.version);
    }
    for ((task_id, manifest_id), version) in latest {
        let Some(tid) = parse_task_id(&task_id).filter(|_| scope.has_task(&task_id)) else {
            continue;
        };
        let id = format!("{task_id}/{manifest_id}");
        let Some(f) = manifests(tid, &manifest_id, version) else {
            let links = vec![task_link(&task_id)];
            out.warn(13, "manifest_invalid", true, Some(&id), &links);
            out.push(
                13,
                entry(
                    id,
                    Mark::Contradicted,
                    json!({ "task_id": task_id, "manifest_id": manifest_id, "version": version,
                            "signoff": null, "violations": ["publish_manifest_not_found"] }),
                    links,
                ),
            );
            continue;
        };
        let links = vec![
            artifact_link(&task_id, &f.name, &f.sha256),
            task_link(&task_id),
        ];
        let mark = if !f.violations.is_empty() || !f.file_matches_record {
            out.warn(13, "manifest_invalid", true, Some(&id), &links);
            Mark::Contradicted
        } else {
            match f.signoff {
                SignoffState::Signed => Mark::Confirmed,
                SignoffState::Unsigned => Mark::NeedsDecision,
                SignoffState::Stale | SignoffState::Superseded => {
                    out.warn(13, "signoff_stale", true, Some(&id), &links);
                    Mark::Contradicted
                }
                SignoffState::Rejected => Mark::Contradicted,
            }
        };
        for code in &f.strong {
            out.warn(13, code, true, Some(&id), &links);
        }
        out.push(
            13,
            entry(
                id,
                mark,
                json!({ "task_id": task_id, "manifest_id": manifest_id, "version": version,
                        "name": f.name, "sha256": f.sha256, "signoff": f.signoff,
                        "violations": f.violations }),
                links,
            ),
        );
    }
}

/// D1.2 規則 1: `needs_decision`・`contradicted` の entry は強い警告（O3 の却下の候補と O13 は除く）。
fn rule_one_warnings(out: &mut Out) {
    let mut add = Vec::new();
    for (i, item) in out.items.iter().enumerate() {
        if i == 2 || i == 12 {
            continue;
        }
        for e in item.entries.iter().filter(|e| e.mark.strong()) {
            add.push((i, e.mark.code(), e.id.clone(), e.links.clone()));
        }
    }
    for (i, code, id, links) in add {
        out.warn(i, code, true, Some(&id), &links);
    }
    // O3: 回答待ちの版の候補だけは要判断の強い警告にする（却下は反証の反映で危険ではない）。
    let pending: Vec<(String, Vec<Link>)> = out.items[2]
        .entries
        .iter()
        .filter(|e| e.mark == Mark::NeedsDecision)
        .map(|e| (e.id.clone(), e.links.clone()))
        .collect();
    for (id, links) in pending {
        out.warn(2, Mark::NeedsDecision.code(), true, Some(&id), &links);
    }
}

/// O13 危険: 警告の code ごとの件数。
fn o13_risks(out: &mut Out) {
    let mut counts: BTreeMap<(String, bool), usize> = BTreeMap::new();
    for w in &out.warnings {
        *counts.entry((w.code.clone(), w.strong)).or_default() += 1;
    }
    let any_strong = out.warnings.iter().any(|w| w.strong);
    let recorded = out.items.iter().any(|i| !i.entries.is_empty());
    if !recorded {
        return;
    }
    let mark = if any_strong {
        Mark::Contradicted
    } else if out.warnings.is_empty() {
        Mark::Confirmed
    } else {
        Mark::Estimated
    };
    let by_code: Vec<Value> = counts
        .into_iter()
        .map(|((code, strong), count)| json!({ "code": code, "strong": strong, "count": count }))
        .collect();
    out.push(
        12,
        entry("warnings", mark, json!({ "by_code": by_code }), Vec::new()),
    );
}
