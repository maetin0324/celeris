//! ADR 2026-11-04-project-controller（A2）: 案件の [`ProjectSnapshot`] を store から組み立てる。
//!
//! - task ledger: `Project.request`（正本）と、その中の Outcome block（[`parse_outcomes`]）、root の acceptance。
//! - progress ledger: root の状態・reviewer の合格・取り込み・失敗の分類・待ち・子の数を **events と行から毎回**作る。
//!   独立の Goal も別のイベントストアも持たない（D1・I2）。worker の報告・ログの文は入れない（I11）。
//! - controller の状態（revision・human_epoch・mode）と過去の判断は A1 の表が正本なので、呼び出し側が渡す。
//! - controller が起票した root は冪等キー `pc:<project>:<root.key>`（`task_idempotency_keys`）で引き、
//!   その行の `result_json` に起票時の `outcome_ids`・`continues_root_ids` を持つ（[`LaunchRootMeta`]）。
//!
//! 起票の DTO 変換（[`launch_root_spec`]・[`launch_root_claim`]）もここに置く。task の作成そのものは既存の
//! `add::build_task_with_roles` と `SqliteStore::create_task_with`＋`idempotency_record_tx` が行う。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use task_core::project_proposal::{
    self as pp, CONTROLLER_LABEL, LAUNCH_ROOT_OPERATION, Outcome, ProjectSnapshot, RootCheck,
    RootSpec, SnapshotController, SnapshotCriterion, SnapshotDecision, SnapshotLimits,
    SnapshotProject, SnapshotRoot, SnapshotWait, SnapshotWake, parse_outcomes,
};
use task_core::store::{IdempotencyClaim, SqliteStore};
use task_core::{Check, Event, OrgDirectory, ProjectId, Status, Task, TaskId, TaskStore};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::OpsError;
use crate::add::{CriterionSpec, NewTaskSpec};

/// A7: snapshot の `authority`。判定は Rust が持つので、TS には結果と許可の種類・id だけを渡す
/// （根拠の本文は入れない。I11）。TS はこれで問いを省けるが、起票の可否は Rust がもう一度判定する。
pub fn authority_section(
    store: &SqliteStore,
    project: &str,
    now: OffsetDateTime,
) -> serde_json::Value {
    use task_core::authority::{ActionClass, GrantScope, ScopeKind, grant_denial};
    let launch_root = store.authority_check(
        project,
        ActionClass::LaunchRoot.as_str(),
        &GrantScope {
            kind: ScopeKind::Project,
            reference: project.to_owned(),
        },
        now,
    );
    let grants: Vec<serde_json::Value> = store
        .authority_grants(project)
        .unwrap_or_default()
        .iter()
        .filter(|g| grant_denial(g, now).is_none())
        .map(|g| {
            serde_json::json!({
                "id": g.id,
                "action_class": g.action_class,
                "scope": g.scope,
                "source_kind": g.source.kind,
                "question_key": g.question_key,
                "expires_at": g.expires_at,
                "budget_remaining": g.budget - g.consumed,
            })
        })
        .collect();
    serde_json::json!({
        "launch_root": {"allow": launch_root.allow, "reason": launch_root.reason, "grant_id": launch_root.grant_id},
        "grants": grants,
    })
}

/// 呼び出し側（A1 の tick・試験）が渡す controller の値。
#[derive(Debug, Clone)]
pub struct SnapshotContext {
    pub controller: SnapshotController,
    pub decisions: Vec<SnapshotDecision>,
    pub wakes: Vec<SnapshotWake>,
    pub limits: SnapshotLimits,
    pub now: OffsetDateTime,
}

/// 冪等キーの行の `result_json`（起票時の controller の値）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRootMeta {
    pub root_key: String,
    pub outcome_ids: Vec<String>,
    pub continues_root_ids: Vec<String>,
    /// 起票時の Outcome の条件の digest（`outcome_id` → [`pp::outcome_spec_digest`]。第 4 フェーズ P4-B）。
    /// 今の版の値と違う・記録が無い Outcome は、この root の `stale_outcome_ids` になる。
    #[serde(default)]
    pub outcome_specs: BTreeMap<String, String>,
}

impl LaunchRootMeta {
    /// controller の root の記録。`outcomes` は起票を決めた snapshot の Outcome（条件の digest をここで取る）。
    pub fn for_root(root: &RootSpec, outcomes: &[Outcome]) -> Self {
        Self {
            root_key: root.key.clone(),
            outcome_ids: root.outcome_ids.clone(),
            continues_root_ids: root.continues_root_ids.clone(),
            outcome_specs: outcomes
                .iter()
                .filter(|o| root.outcome_ids.contains(&o.id))
                .map(|o| (o.id.clone(), pp::outcome_spec_digest(o)))
                .collect(),
        }
    }

    /// `outcome_ids` のうち、今の Outcome（`outcomes`）の条件が起票時と違うもの。今の版に無い Outcome は数えない。
    pub fn stale_outcome_ids(&self, outcomes: &[Outcome]) -> Vec<String> {
        self.outcome_ids
            .iter()
            .filter(|id| {
                outcomes.iter().any(|o| {
                    &o.id == *id && self.outcome_specs.get(*id) != Some(&pp::outcome_spec_digest(o))
                })
            })
            .cloned()
            .collect()
    }
}

fn status_str(s: Status) -> String {
    serde_json::to_value(s)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn check_kind(c: &Check) -> &'static str {
    match c {
        Check::Command { .. } => "command",
        Check::ArtifactExists { .. } => "artifact_exists",
        Check::KnowledgePage { .. } => "knowledge_page",
        Check::Reviewer => "reviewer",
        Check::Human => "human",
    }
}

/// worker の run が毎回書く機械向けの file。I4 の「成果物」に数えない。
const MACHINE_ARTIFACTS: [&str; 4] = [
    "result.json",
    "checkpoint.json",
    "delegate.json",
    "followups.json",
];

/// root の progress ledger の 1 行を events から作る。
///
/// 第 3 フェーズ I3（ADR 2026-11-05-phase3-achievement-invariants、付記 P0-B）:
/// - `reviewer_passed` は「最後の終端遷移が `review_pass` による done」で、かつ最後の review の判定に不合格が無く、
///   `Check::Command` の条件が全部合格のときだけ真。reopen（終端から非終端への遷移）で前の合格と判定を捨てる。
/// - `integrated` は、repo を持つ root では最後の `IntegrationMerged` の後に `IntegrationReopened` が無いこと、
///   repo の無い root（調査の案件）では成果物（機械向けの file 以外の `ArtifactProduced`、または合格した
///   `artifact_exists`・`knowledge_page` の条件）があること（I4）。
fn root_row(
    store: &SqliteStore,
    task: &Task,
    meta: Option<&LaunchRootMeta>,
    now: OffsetDateTime,
    waits: &mut Vec<SnapshotWait>,
) -> Result<SnapshotRoot, OpsError> {
    let events = store.events_for(task.id)?;
    // 今の試行の review の判定（criterion ごとに最新）。非終端への遷移（reopen・retry）で捨てる。
    let mut verdicts: BTreeMap<usize, bool> = BTreeMap::new();
    let mut passed_by_review = false;
    let mut merged = false;
    let mut produced = false;
    let mut last_fail_reason: Option<String> = None;
    let mut attempts = 0u32;
    let mut open_questions: Vec<(String, &'static str)> = Vec::new();
    for (seq, e) in &events {
        match e {
            Event::ReviewVerdict {
                criterion_idx,
                pass,
                ..
            } => {
                verdicts.insert(*criterion_idx, *pass);
            }
            // A9 D1: repo を持つ案件で取り込み済みと数えるのは IntegrationMerged だけ。見送り（NotGit・NoCommits）や
            // IntegrationNotApplicable は、成果が案件の tree に載ったことを示さない。
            Event::IntegrationMerged { .. } => merged = true,
            Event::IntegrationReopened { .. } => merged = false,
            Event::ArtifactProduced { artifact, .. } => {
                let name = artifact.name.rsplit('/').next().unwrap_or_default();
                if !MACHINE_ARTIFACTS.contains(&name) {
                    produced = true;
                }
            }
            Event::Transitioned { to, reason, .. } => {
                if to.is_terminal() {
                    // I3(b): 最後の終端遷移だけを見る（reopen の前の合格を持ち越さない）。
                    passed_by_review = *to == Status::Done && reason == "review_pass";
                } else if *to != Status::Reviewing {
                    passed_by_review = false;
                    verdicts.clear();
                }
                if *to == Status::Failed {
                    last_fail_reason = Some(reason.clone());
                }
                if !to.is_terminal() && *to != Status::Reviewing {
                    last_fail_reason = None;
                }
            }
            Event::Retried { .. } => attempts = attempts.saturating_add(1),
            Event::QuestionRaised { .. } => open_questions.push((seq.to_string(), "question")),
            Event::Answered { .. } => {
                open_questions.retain(|(_, k)| *k != "question");
            }
            Event::ApprovalRequested => open_questions.push((seq.to_string(), "approval")),
            Event::ApprovalDecided { .. } => open_questions.retain(|(_, k)| *k != "approval"),
            _ => {}
        }
    }
    // I3(c)・I7: 最後の review に不合格が無く、決定的な検査（command）は全部が合格の判定を持つ。
    let checks_passed = !verdicts.values().any(|p| !p)
        && task
            .acceptance
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c.check, Check::Command { .. }))
            .all(|(i, _)| verdicts.get(&i) == Some(&true));
    let reviewer_passed = task.status == Status::Done && passed_by_review && checks_passed;
    // I4: repo の無い root の成果物。合格した artifact_exists・knowledge_page の条件も成果物の参照に数える。
    let deliverable = produced
        || task.acceptance.iter().enumerate().any(|(i, c)| {
            matches!(
                c.check,
                Check::ArtifactExists { .. } | Check::KnowledgePage { .. }
            ) && verdicts.get(&i) == Some(&true)
        });
    let status = status_str(task.status);
    let failure_class = match task.status {
        Status::Failed => Some(
            match last_fail_reason.as_deref() {
                Some("review_fail") => "review_failed",
                _ => "infra",
            }
            .to_owned(),
        ),
        Status::Cancelled => Some("cancelled".to_owned()),
        _ => None,
    };
    let acceptance = task
        .acceptance
        .iter()
        .enumerate()
        .map(|(i, c)| SnapshotCriterion {
            text: c.text.clone(),
            kind: check_kind(&c.check).to_owned(),
            result: match verdicts.get(&i) {
                Some(true) => "pass",
                Some(false) => "fail",
                None => "unknown",
            }
            .to_owned(),
        })
        .collect();
    let children = store.children(task.id)?;
    let (leaf_done, leaf_total) = if children.is_empty() {
        (u32::from(task.status == Status::Done), 1)
    } else {
        let done = children.iter().filter(|c| c.status == Status::Done).count();
        (
            u32::try_from(done).unwrap_or(u32::MAX),
            u32::try_from(children.len()).unwrap_or(u32::MAX),
        )
    };
    // 終端の root に開いた質問は残っていても待ちに数えない。
    if task.status.is_terminal() {
        open_questions.clear();
    }
    let age = (now - task.updated_at).whole_seconds().max(0);
    for (seq, kind) in &open_questions {
        waits.push(SnapshotWait {
            kind: (*kind).to_owned(),
            id: format!("{}:{seq}", task.id),
            task_id: task.id.to_string(),
            age_secs: u64::try_from(age).unwrap_or(0),
        });
    }
    Ok(SnapshotRoot {
        id: task.id.to_string(),
        status,
        title: task.title.clone(),
        controller_key: meta.map(|m| m.root_key.clone()),
        outcome_ids: meta.map(|m| m.outcome_ids.clone()).unwrap_or_default(),
        // Outcome の版が決まった後に `build` が埋める。
        stale_outcome_ids: Vec::new(),
        acceptance,
        reviewer_passed,
        integrated: task.status == Status::Done
            && if task.repos.is_empty() {
                deliverable
            } else {
                merged
            },
        failure_class,
        attempts,
        open_waits: open_questions
            .iter()
            .map(|(_, k)| (*k).to_owned())
            .collect(),
        leaf_done,
        leaf_total,
        continues_root_ids: meta
            .map(|m| m.continues_root_ids.clone())
            .unwrap_or_default(),
    })
}

/// 案件 `project_id` の snapshot を作る。上限（1 MiB）を超える分は古い done の root から落とし `truncated` を真にする。
pub fn build(
    store: &SqliteStore,
    project_id: ProjectId,
    ctx: SnapshotContext,
) -> Result<ProjectSnapshot, OpsError> {
    let project = store
        .project_get(project_id)?
        .ok_or(OpsError::ProjectNotFound(project_id))?;
    let pid = project.id.to_string();
    let brief = store
        .project_brief_get(project_id)?
        .and_then(|b| serde_json::to_value(b).ok());
    let authority = authority_section(store, &pid, ctx.now);
    let (mut outcomes, mut outcomes_invalid) = match parse_outcomes(&project.request) {
        Ok(o) => (o, false),
        Err(_) => (Vec::new(), true),
    };
    let metas: BTreeMap<String, LaunchRootMeta> = store
        .idempotency_keys_with_prefix(&format!("{}{pid}:", pp::ROOT_KEY_PREFIX))?
        .into_iter()
        .filter_map(|(_, task_id, json)| {
            let meta: LaunchRootMeta = serde_json::from_str(json.as_deref()?).ok()?;
            Some((task_id, meta))
        })
        .collect();
    let mut tasks: Vec<Task> = store
        .list(None)?
        .into_iter()
        .filter(|t| t.project_id == Some(project_id) && task_core::is_root_task(t))
        .collect();
    tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    let mut waits = Vec::new();
    let mut roots = Vec::with_capacity(tasks.len());
    for t in &tasks {
        roots.push(root_row(
            store,
            t,
            metas.get(&t.id.to_string()),
            ctx.now,
            &mut waits,
        )?);
    }
    // 付記 A5: approved の Roadmap があれば request の block より優先する（block は読まない）。
    let roadmap = match store.roadmap_approved(&pid)? {
        Some(version) => {
            outcomes = task_core::roadmap::snapshot_outcomes(&version);
            outcomes_invalid = false;
            let view = crate::roadmap::view_of(store, &version, &tasks, &metas)?;
            serde_json::to_value(view).ok()
        }
        None => None,
    };
    // N5: 同じ版の条件と実際の command の両方を持つ root だけを根拠にする。
    let captured = store.task_roadmap_specs(&pid)?;
    let versions = if roadmap.is_some() {
        store.task_roadmap_versions(&pid)?
    } else {
        BTreeMap::new()
    };
    for (r, task) in roots.iter_mut().zip(&tasks) {
        if let Some(meta) = metas.get(&r.id) {
            r.stale_outcome_ids = meta.stale_outcome_ids(&outcomes);
        } else if roadmap.is_some() {
            let original = captured.get(&r.id).and_then(|v| versions.get(v));
            r.stale_outcome_ids = outcomes
                .iter()
                // acceptance が一致しない Outcome は元から満たせない。無関係な ID で snapshot を膨らませない。
                .filter(|o| {
                    o.acceptance
                        .iter()
                        .all(|text| task.acceptance.iter().any(|a| &a.text == text))
                })
                .filter(|o| {
                    let same = original
                        .and_then(|specs| specs.iter().find(|s| s.id == o.id))
                        .is_some_and(|s| s.acceptance == o.acceptance && s.checks == o.checks);
                    let commands = o.checks.iter().all(|c| {
                        task.acceptance.iter().any(|a| match &a.check {
                            Check::Command { cmd, expect_exit } => {
                                cmd == &c.cmd && expect_exit == &c.expect_exit
                            }
                            _ => false,
                        })
                    });
                    !same || !commands
                })
                .map(|o| o.id.clone())
                .collect();
        }
    }
    let mut snapshot = ProjectSnapshot {
        now: ctx.now.format(&Rfc3339).unwrap_or_default(),
        truncated: false,
        project: SnapshotProject {
            id: pid,
            title: project.title.clone(),
            status: serde_json::to_value(project.status)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            request_sha256: pp::sha256_hex(&project.request),
            request: project.request.clone(),
            auto_integrate: project.auto_integrate,
        },
        brief,
        outcomes,
        outcomes_invalid,
        roots,
        waits,
        stagnation: Vec::new(),
        wakes: ctx.wakes,
        decisions: ctx.decisions,
        controller: ctx.controller,
        limits: ctx.limits,
        roadmap,
        authority: Some(authority),
    };
    while serde_json::to_vec(&snapshot).map_or(0, |v| v.len()) > pp::SNAPSHOT_MAX_BYTES {
        let Some(i) = snapshot.roots.iter().position(|r| r.status == "done") else {
            break;
        };
        snapshot.roots.remove(i);
        snapshot.truncated = true;
    }
    Ok(snapshot)
}

/// `launch_root` の root を `POST /tasks` と同じ DTO にする。acceptance は Outcome の文のまま（I8）、label は
/// `project-controller`。親・依存・担当は持たない（root）。
/// 対象 Outcome の `checks` を `Check::Command` として足す（ADR 2026-11-05-phase3-p0e）。command は依頼文の block が
/// 正本で、TS の proposal からは作らない・落とせない。`outcomes` は判断に使った snapshot の Outcome。
pub fn launch_root_spec(
    project_id: ProjectId,
    root: &RootSpec,
    outcomes: &[Outcome],
) -> Result<NewTaskSpec, OpsError> {
    let mut acceptance: Vec<CriterionSpec> = root
        .acceptance
        .iter()
        .map(|c| match c.check {
            RootCheck::Reviewer => CriterionSpec::Reviewer {
                text: c.text.clone(),
            },
            RootCheck::Human => CriterionSpec::Human {
                text: c.text.clone(),
            },
        })
        .collect();
    let mut seen = BTreeSet::new();
    for check in outcomes
        .iter()
        .filter(|o| root.outcome_ids.contains(&o.id))
        .flat_map(|o| o.checks.iter())
    {
        if seen.insert((check.cmd.as_str(), check.expect_exit)) {
            acceptance.push(CriterionSpec::Command {
                cmd: check.cmd.clone(),
                expect_exit: check.expect_exit,
            });
        }
    }
    let body = serde_json::json!({
        "title": root.title,
        "objective": root.objective,
        "acceptance": acceptance,
        "project_id": project_id,
        "labels": [CONTROLLER_LABEL],
    });
    serde_json::from_value(body).map_err(|e| OpsError::Validation(format!("root spec: {e}")))
}

/// 起票の冪等キーの claim と、行に残す [`LaunchRootMeta`] の JSON。`request_hash` は root の中身の SHA-256
/// （同じ key・同じ中身の再送は既存を返し、中身が違えば衝突）。
pub fn launch_root_claim(
    project_id: ProjectId,
    root: &RootSpec,
    outcomes: &[Outcome],
) -> Result<(IdempotencyClaim, String), OpsError> {
    let body = serde_json::to_string(root).map_err(|e| OpsError::Validation(e.to_string()))?;
    let meta = LaunchRootMeta::for_root(root, outcomes);
    let meta = serde_json::to_string(&meta).map_err(|e| OpsError::Validation(e.to_string()))?;
    Ok((
        IdempotencyClaim {
            key: pp::root_idempotency_key(&project_id.to_string(), &root.key),
            operation: LAUNCH_ROOT_OPERATION.to_owned(),
            request_hash: pp::sha256_hex(&body),
        },
        meta,
    ))
}

/// snapshot の root の id のうち、`project_controller:read_root_evidence` が読んでよいもの（op の scope）。
pub fn evidence_scope(snapshot: &ProjectSnapshot) -> BTreeSet<String> {
    snapshot.roots.iter().map(|r| r.id.clone()).collect()
}

/// `TaskId` の文字列を読む（snapshot の id から）。
pub fn parse_task_id(id: &str) -> Option<TaskId> {
    id.parse().ok()
}

#[cfg(test)]
mod launch_root_spec_tests {
    use super::*;
    use task_core::project_proposal::{OutcomeCheck, RootCriterion};

    fn root(outcome_ids: &[&str]) -> RootSpec {
        RootSpec {
            key: "outcome:o1:1".into(),
            title: "t".into(),
            objective: "o".into(),
            acceptance: vec![RootCriterion {
                text: "A".into(),
                check: RootCheck::Reviewer,
            }],
            stages_hint: vec![],
            continues_root_ids: vec![],
            outcome_ids: outcome_ids.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn outcome(id: &str, cmds: &[&str]) -> Outcome {
        Outcome {
            id: id.into(),
            title: "t".into(),
            acceptance: vec!["A".into()],
            depends_on: vec![],
            priority: 0,
            source: "request_block".into(),
            checks: cmds
                .iter()
                .map(|c| OutcomeCheck {
                    cmd: (*c).into(),
                    expect_exit: 0,
                })
                .collect(),
        }
    }

    fn commands(spec: NewTaskSpec) -> Vec<String> {
        serde_json::to_value(&spec).expect("json")["acceptance"]
            .as_array()
            .expect("acceptance")
            .iter()
            .filter_map(|c| c["cmd"].as_str().map(str::to_owned))
            .collect()
    }

    /// 対象 Outcome の検査だけを足し、同じ command は 1 回にする。TS の proposal には command が無い。
    #[test]
    fn launch_root_spec_adds_the_checks_of_the_named_outcomes_only() {
        let outcomes = [
            outcome("o1", &["check-1", "check-shared"]),
            outcome("o2", &["check-2"]),
            outcome("o3", &["check-shared"]),
        ];
        let id = ProjectId::new();
        let spec = launch_root_spec(id, &root(&["o1", "o3"]), &outcomes).expect("spec");
        assert_eq!(commands(spec), vec!["check-1", "check-shared"]);
        let spec = launch_root_spec(id, &root(&["o9"]), &outcomes).expect("spec");
        assert!(commands(spec).is_empty());
    }
}

#[cfg(test)]
mod root_row_tests {
    use super::*;
    use crate::add::create_task;
    use task_core::DeliverySkipReason;
    use task_core::RepoId;
    use task_core::repos::RepoRef;

    /// done の root 1 件と、その events を積んだ store で `integrated` を返す。
    fn integrated(with_repo: bool, events: &[Event]) -> bool {
        let store = SqliteStore::open_in_memory().expect("store");
        let spec: NewTaskSpec = serde_json::from_value(serde_json::json!({
            "title": "t", "objective": "o",
            "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}]
        }))
        .expect("spec");
        let mut task = create_task(&store, spec, OffsetDateTime::now_utc()).expect("task");
        for e in events {
            store.append_event(task.id, e).expect("event");
        }
        task.status = Status::Done;
        if with_repo {
            task.repos = vec![RepoRef {
                repo_id: RepoId::new(),
                name: "r".into(),
            }];
        }
        root_row(
            &store,
            &task,
            None,
            OffsetDateTime::now_utc(),
            &mut Vec::new(),
        )
        .expect("row")
        .integrated
    }

    fn skipped(reason: DeliverySkipReason) -> Event {
        Event::DeliverySkipped {
            reason,
            detail: String::new(),
            head: None,
        }
    }

    #[test]
    fn root_row_repo_no_commits_is_not_integrated() {
        assert!(!integrated(true, &[skipped(DeliverySkipReason::NoCommits)]));
    }

    #[test]
    fn root_row_repo_not_git_is_not_integrated() {
        assert!(!integrated(true, &[skipped(DeliverySkipReason::NotGit)]));
    }

    #[test]
    fn root_row_repo_integration_merged_is_integrated() {
        let merged = Event::IntegrationMerged {
            repo_id: RepoId::new(),
            branch: "b".into(),
            head_sha: "0".repeat(40),
            actor: "test".into(),
        };
        assert!(integrated(true, &[merged]));
    }

    #[test]
    fn root_row_repo_not_applicable_is_not_integrated() {
        let na = Event::IntegrationNotApplicable {
            repo_id: RepoId::new(),
            branch: "b".into(),
            head_sha: "0".repeat(40),
            reason: "remote".into(),
            actor: "test".into(),
        };
        assert!(!integrated(true, &[na]));
    }

    fn artifact(name: &str) -> Event {
        Event::ArtifactProduced {
            run_id: "r".into(),
            artifact: task_core::ArtifactRef {
                name: name.into(),
                path: format!("artifacts/{name}"),
                sha256: "0".repeat(64),
                kind: "file".into(),
                declared: true,
            },
        }
    }

    /// K41（I4）: repo の無い調査の root は成果物があれば取り込み済み。
    #[test]
    fn root_row_without_repo_with_deliverable_is_integrated() {
        assert!(integrated(false, &[artifact("report.md")]));
    }

    /// K41（I4）: repo の無い root に成果物が無い（機械向けの result.json だけ）なら取り込み済みにしない。
    #[test]
    fn root_row_without_repo_without_deliverable_is_not_integrated() {
        assert!(!integrated(false, &[]));
        assert!(!integrated(
            false,
            &[artifact("result.json"), artifact("checkpoint.json")]
        ));
    }

    fn transitioned(from: Status, to: Status, reason: &str) -> Event {
        Event::Transitioned {
            from,
            to,
            reason: reason.into(),
        }
    }

    fn verdict(idx: usize, pass: bool) -> Event {
        Event::ReviewVerdict {
            run_id: "rev".into(),
            criterion_idx: idx,
            pass,
            reason: String::new(),
        }
    }

    /// done の root の行（acceptance は command 1 件と reviewer 1 件）。
    fn done_row(events: &[Event]) -> SnapshotRoot {
        let store = SqliteStore::open_in_memory().expect("store");
        let spec: NewTaskSpec = serde_json::from_value(serde_json::json!({
            "title": "t", "objective": "o",
            "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0},
                           {"type": "reviewer", "text": "成果が要件を満たす"}]
        }))
        .expect("spec");
        let mut task = create_task(&store, spec, OffsetDateTime::now_utc()).expect("task");
        for e in events {
            store.append_event(task.id, e).expect("event");
        }
        task.status = Status::Done;
        root_row(
            &store,
            &task,
            None,
            OffsetDateTime::now_utc(),
            &mut Vec::new(),
        )
        .expect("row")
    }

    /// I3(b)(c): review で全部合格して done になった root は合格。
    #[test]
    fn root_row_review_pass_with_checks_is_passed() {
        let row = done_row(&[
            transitioned(Status::Running, Status::Reviewing, "worker_done"),
            verdict(0, true),
            verdict(1, true),
            transitioned(Status::Reviewing, Status::Done, "review_pass"),
        ]);
        assert!(row.reviewer_passed);
    }

    /// K3（I3(c)・I7）: reviewer は合格でも command の検査が落ちていれば合格にしない。
    #[test]
    fn root_row_command_check_fail_is_not_passed() {
        let row = done_row(&[
            transitioned(Status::Running, Status::Reviewing, "worker_done"),
            verdict(0, false),
            verdict(1, true),
            transitioned(Status::Reviewing, Status::Done, "review_pass"),
        ]);
        assert!(!row.reviewer_passed);
        // command の判定が無い（検査が走っていない）ときも合格にしない（fail closed）。
        let row = done_row(&[
            transitioned(Status::Running, Status::Reviewing, "worker_done"),
            verdict(1, true),
            transitioned(Status::Reviewing, Status::Done, "review_pass"),
        ]);
        assert!(!row.reviewer_passed);
    }

    /// K9（I3(b)）: done の root を reopen し、2 回目の review が落ちたら前の合格を持ち越さない。
    #[test]
    fn root_row_reopen_drops_previous_pass() {
        let first = [
            transitioned(Status::Running, Status::Reviewing, "worker_done"),
            verdict(0, true),
            verdict(1, true),
            Event::ReviewerPassed {
                review_run: "rev".into(),
                repo_id: task_core::RepoId::new(),
                target_sha: "0".repeat(40),
                reviewed_sha: "0".repeat(40),
                criteria: vec![0, 1],
            },
            transitioned(Status::Reviewing, Status::Done, "review_pass"),
            transitioned(Status::Done, Status::Ready, "reopen"),
            transitioned(Status::Running, Status::Reviewing, "worker_done"),
            verdict(0, true),
            verdict(1, false),
            transitioned(Status::Reviewing, Status::Failed, "review_fail"),
        ];
        assert!(!done_row(&first).reviewer_passed);
        // reopen の後に終端へ来ていない（review 待ち）なら、前の合格は使わない。
        assert!(!done_row(&first[..6]).reviewer_passed);
        // reopen の後に review で合格し直せば合格。
        let mut again = first[..6].to_vec();
        again.extend([
            transitioned(Status::Running, Status::Reviewing, "worker_done"),
            verdict(0, true),
            verdict(1, true),
            transitioned(Status::Reviewing, Status::Done, "review_pass"),
        ]);
        assert!(done_row(&again).reviewer_passed);
    }

    /// I3(b): review を通らない done（理由が `review_pass` でない）は合格にしない。
    #[test]
    fn root_row_done_without_review_pass_is_not_passed() {
        let row = done_row(&[
            verdict(0, true),
            verdict(1, true),
            transitioned(Status::Ready, Status::Done, "approve"),
        ]);
        assert!(!row.reviewer_passed);
    }
}
