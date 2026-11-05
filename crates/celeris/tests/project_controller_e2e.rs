//! ADR 2026-11-04-project-controller（A2）: 本物の control-ts の bundle を本物の deno_core host（V8）で動かし、
//! 案件の snapshot（`task_ops::project_snapshot::build`）→ TS の proposal → Rust の検証（`project_proposal::validate`）
//! → 適用（既存の `add::build_task_with_roles`・`SqliteStore::create_task_with`・`idempotency_record_tx`）を通す。
//!
//! - LLM を呼ばない。外部ネットワークに出ない。本番の DB・daemon に触れない（一時 DB）。userns を使わない。
//! - root の完了は store の遷移（Dispatch → WorkerDone → ReviewPass）で模す。取り込みは `IntegrationMerged` の event。
//! - A1（wake・判断の表）はまだ無いので、判断の記録と `revision` は試験の harness が持つ（A3 が A1 の表へ接続する）。
//! - `CELERIS_PC_E2E_OUT=<dir>` があれば、試験ごとに `<dir>/<試験名>.jsonl`（`celeris.long-horizon-record/1`）を書く。
//!   `scripts/dev/test-project-controller-e2e.sh` がこの raw を読んで受け入れ条件を検査する。
//! - bundle が作れないと SKIP になるので、script は `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で走らせる（SKIP でなく失敗）。

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use support::script_compare::{LONG_DEADLINE, dist_bundle, start_code, start_policy};
use task_core::project_proposal::{
    self as pp, ProjectAction, ProjectSnapshot, ProposalViolation, SnapshotController,
    SnapshotDecision, SnapshotLimits,
};
use task_core::store::{IdempotencyLookup, SqliteStore};
use task_core::{
    Event, Project, ProjectId, ProjectStatus, RepoId, StoreError, TaskId, TaskStore, Trigger,
};
use task_dispatch::script_policy::PolicyScript;
use task_ops::project_snapshot::{self as ps, SnapshotContext};
use time::OffsetDateTime;

const DOMAIN: &str = "project_controller";
const RECORD_SCHEMA: &str = "celeris.long-horizon-record/1";

/// 中間 `o1` と最終 `o2`（`o2` は `o1` に依存）。
const REQUEST: &str = "BenchFS の長期改善。最終目標は性能の報告。\n\
```celeris-outcomes\n\
[{\"id\":\"o1\",\"title\":\"計測の土台\",\"acceptance\":[\"計測 script が 3 回の反復で同じ値を出す\"]},\
{\"id\":\"o2\",\"title\":\"性能の報告\",\"acceptance\":[\"報告に p50 と p95 の表がある\",\"表の値は raw から再生成できる\"],\"depends_on\":[\"o1\"]}]\n\
```\n";

/// A1 の表の代わり（判断の記録・revision・human_epoch）と一時 DB。
struct Harness {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: SqliteStore,
    project: ProjectId,
    controller: SnapshotController,
    decisions: Vec<SnapshotDecision>,
    with_repo: Option<RepoId>,
    test: &'static str,
    step: u32,
    cos_inputs: u32,
    escalations: u32,
}

/// 適用の結果。
#[derive(Debug, Clone, PartialEq)]
enum Applied {
    Rejected(Vec<String>),
    Launched { root: TaskId, created: bool },
    Recorded(String),
}

fn project_row(request: &str, title: &str) -> Project {
    let now = OffsetDateTime::now_utc();
    Project {
        auto_advance: false,
        auto_integrate: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: title.into(),
        request: request.into(),
        status: ProjectStatus::Active,
        workspace: None,
        created_at: now,
        updated_at: now,
    }
}

impl Harness {
    fn new(test: &'static str, request: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("celeris.sqlite3");
        let store = SqliteStore::open(&path).expect("open store");
        let project = project_row(request, "pilot:a2-e2e");
        store.project_create(&project).expect("project");
        Self {
            _dir: dir,
            path,
            store,
            project: project.id,
            controller: SnapshotController {
                mode: "enabled".into(),
                phase: "active".into(),
                revision: 0,
                human_epoch: 0,
                retry_count: 0,
                progress_epoch: 0,
            },
            decisions: Vec::new(),
            with_repo: None,
            test,
            step: 0,
            cos_inputs: 0,
            escalations: 0,
        }
    }

    fn snapshot(&self) -> ProjectSnapshot {
        ps::build(
            &self.store,
            self.project,
            SnapshotContext {
                controller: self.controller.clone(),
                decisions: self.decisions.clone(),
                wakes: Vec::new(),
                limits: SnapshotLimits::default(),
                now: OffsetDateTime::now_utc(),
            },
        )
        .expect("snapshot")
    }

    fn roots(&self) -> Vec<task_core::Task> {
        self.store
            .list(None)
            .expect("list")
            .into_iter()
            .filter(|t| t.project_id == Some(self.project))
            .collect()
    }

    /// 1 回の評価: snapshot → TS → 検証 → 適用。raw を 1 行書く。
    async fn step(&mut self, policy: &dyn PolicyScript) -> (ProjectSnapshot, Value, Applied) {
        let snapshot = self.snapshot();
        let raw = policy
            .project_controller(snapshot.clone(), LONG_DEADLINE)
            .await
            .expect("TS reply")
            .output;
        let applied = self.apply(&snapshot, &raw);
        self.record(&snapshot, &raw, &applied, json!({}));
        (snapshot, raw, applied)
    }

    /// 検証して適用する（A3 の `project_decider_ts` の役。A1 の tx1/tx2 の代わりに直列で行う）。
    fn apply(&mut self, snapshot: &ProjectSnapshot, raw: &Value) -> Applied {
        let proposal = match pp::parse_and_validate(snapshot, raw) {
            Ok(p) => p,
            Err(v) => return Applied::Rejected(v.into_iter().map(|v| v.code).collect()),
        };
        // I6・I7: 適用の時点の revision・human_epoch と照合する（A1 の tx1 の CAS の代わり）。
        if proposal.based_on.human_epoch != self.controller.human_epoch {
            return Applied::Rejected(vec!["stale_human_epoch".into()]);
        }
        if proposal.based_on.revision != self.controller.revision {
            return Applied::Rejected(vec!["stale_revision".into()]);
        }
        let mut decision = SnapshotDecision {
            id: format!("dec-{}", self.decisions.len() + 1),
            kind: proposal.action.kind().to_owned(),
            rule_id: proposal.explanation.rule_id.clone(),
            effect_state: "applied".into(),
            root_key: None,
            reason_code: None,
            outcome_id: None,
            request_sha256: snapshot.project.request_sha256.clone(),
            progress_epoch: proposal.gap.progress_epoch,
            at: snapshot.now.clone(),
        };
        let applied = match &proposal.action {
            ProjectAction::LaunchRoot { root } => {
                decision.root_key = Some(root.key.clone());
                let (id, created) =
                    launch(&self.path, self.project, root, self.with_repo).expect("launch");
                Applied::Launched { root: id, created }
            }
            ProjectAction::Replan {
                reason_code,
                context_refs,
                ..
            } => {
                decision.reason_code = Some(reason_code.clone());
                decision.root_key = context_refs.first().and_then(|r| {
                    let id = r.strip_prefix("task:")?;
                    snapshot
                        .roots
                        .iter()
                        .find(|x| x.id == id)?
                        .controller_key
                        .clone()
                });
                self.cos_inputs += 1;
                Applied::Recorded("replan".into())
            }
            ProjectAction::Escalate { question_key, .. } => {
                decision.root_key = Some(question_key.clone());
                self.escalations += 1;
                Applied::Recorded("escalate".into())
            }
            ProjectAction::MarkMilestone { outcome_id, .. } => {
                decision.outcome_id = Some(outcome_id.clone());
                Applied::Recorded("mark_milestone".into())
            }
            ProjectAction::Wait { .. } | ProjectAction::Continue { .. } => {
                decision.effect_state = "none".into();
                Applied::Recorded(proposal.action.kind().into())
            }
        };
        self.decisions.push(decision);
        self.controller.revision += 1;
        self.controller.progress_epoch = proposal.gap.progress_epoch;
        applied
    }

    fn record(&mut self, snapshot: &ProjectSnapshot, raw: &Value, applied: &Applied, extra: Value) {
        self.step += 1;
        let roots = self.roots();
        let line = json!({
            "schema": RECORD_SCHEMA,
            "test": self.test,
            "step": self.step,
            "snapshot_sha256": pp::sha256_hex(&serde_json::to_string(snapshot).unwrap_or_default()),
            "action": raw["action"]["kind"],
            "rule_id": raw["explanation"]["rule_id"],
            "gap": raw["gap"],
            "applied": match applied {
                Applied::Rejected(v) => json!({"rejected": v}),
                Applied::Launched { root, created } => json!({"launched": root.to_string(), "created": created}),
                Applied::Recorded(k) => json!({"recorded": k}),
            },
            "roots_total": roots.len(),
            "roots": roots.iter().map(|t| json!({"id": t.id.to_string(), "status": t.status})).collect::<Vec<_>>(),
            "cos_inputs": self.cos_inputs,
            "escalations": self.escalations,
            "llm_launches": 0,
            "extra": extra,
        });
        write_record(self.test, &line);
    }

    /// root を done・合格・取り込み済みにする（worker と reviewer の代わり）。
    fn complete(&self, id: TaskId, pass: bool, integrate: bool) {
        let s = &self.store;
        let t = s.get(id).expect("get").expect("task");
        if t.status == task_core::Status::Draft {
            s.apply_transition(id, Trigger::Accept, None)
                .expect("accept");
        }
        // review 不合格は再試行の枠を使い切るまで ready に戻るので、終端になるまで繰り返す（上限 10 回）。
        for _ in 0..10 {
            s.apply_transition(id, Trigger::Dispatch, None)
                .expect("dispatch");
            s.apply_transition(id, Trigger::WorkerDone, None)
                .expect("worker done");
            let trigger = if pass {
                Trigger::ReviewPass
            } else {
                Trigger::ReviewFail
            };
            s.apply_transition(id, trigger, None).expect("review");
            if s.get(id).expect("get").expect("task").status.is_terminal() {
                break;
            }
        }
        if integrate && let Some(repo) = self.with_repo {
            s.append_event(
                id,
                &Event::IntegrationMerged {
                    repo_id: repo,
                    branch: format!("celeris/{id}"),
                    head_sha: "0".repeat(40),
                    actor: "test".into(),
                },
            )
            .expect("merged");
        }
    }
}

/// `launch_root` を既存の作成経路で冪等に適用する（I5）。`path` の DB を別の接続で開く（並行の試験が同時に呼ぶ）。
fn launch(
    path: &Path,
    project: ProjectId,
    root: &pp::RootSpec,
    repo: Option<RepoId>,
) -> Result<(TaskId, bool), String> {
    let store = SqliteStore::open(path).map_err(|e| e.to_string())?;
    let (claim, meta) = ps::launch_root_claim(project, root).map_err(|e| e.to_string())?;
    let existing = |found: IdempotencyLookup| match found {
        IdempotencyLookup::Same { task_id, .. } => task_id.parse().map_err(|_| "id".to_owned()),
        IdempotencyLookup::Conflict { task_id } => Err(format!("conflict with {task_id}")),
        IdempotencyLookup::New => Err("no row".into()),
    };
    let found = store
        .idempotency_lookup(&claim)
        .map_err(|e| e.to_string())?;
    if found != IdempotencyLookup::New {
        return existing(found).map(|id| (id, false));
    }
    let spec = ps::launch_root_spec(project, root, &[]).map_err(|e| e.to_string())?;
    let mut task =
        task_ops::add::build_task_with_roles(&store, spec, &[], &[], OffsetDateTime::now_utc())
            .map_err(|e| e.to_string())?;
    if let Some(repo) = repo {
        task.repos = vec![task_core::repos::RepoRef {
            repo_id: repo,
            name: "bench".into(),
        }];
    }
    let now = OffsetDateTime::now_utc().to_string();
    let created = store.create_task_with(&task, "daemon", vec![], |tx| {
        if SqliteStore::idempotency_lookup_tx(tx, &claim)? != IdempotencyLookup::New {
            return Err(StoreError::Invalid("taken".into()));
        }
        SqliteStore::idempotency_record_tx(tx, &claim, &task.id.to_string(), Some(&meta), &now)
    });
    match created {
        Ok(()) => Ok((task.id, true)),
        Err(StoreError::Invalid(_)) => {
            let found = store
                .idempotency_lookup(&claim)
                .map_err(|e| e.to_string())?;
            existing(found).map(|id| (id, false))
        }
        Err(e) => Err(e.to_string()),
    }
}

fn write_record(test: &str, line: &Value) {
    let Some(dir) = std::env::var_os("CELERIS_PC_E2E_OUT") else {
        return;
    };
    use std::io::Write as _;
    let path = PathBuf::from(dir).join(format!("{test}.jsonl"));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    writeln!(f, "{line}").expect("write record");
}

fn real_policy(
    test: &str,
) -> Option<(
    celeris_script::ScriptRuntimeHandle,
    Arc<celeris::ScriptPolicy>,
)> {
    let bundle = dist_bundle(test, DOMAIN)?;
    let sha = bundle.manifest.bundle_sha256.clone();
    Some(start_policy(bundle.source(), sha))
}

fn kind(raw: &Value) -> &str {
    raw["action"]["kind"].as_str().unwrap_or_default()
}

fn launched(a: &Applied) -> TaskId {
    match a {
        Applied::Launched { root, .. } => *root,
        other => panic!("expected a launch, got {other:?}"),
    }
}

/// s1: root 0 件 → `launch_root outcome:o1:1`、root 1 件。acceptance は Outcome の文のまま。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_core_s1_first_launches_root1() {
    let Some((handle, policy)) = real_policy("project_controller_core_s1_first_launches_root1")
    else {
        return;
    };
    let mut h = Harness::new("project_controller_core_s1_first_launches_root1", REQUEST);
    let (_, raw, applied) = h.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "launch_root", "{raw}");
    assert_eq!(raw["action"]["root"]["key"], "outcome:o1:1");
    let id = launched(&applied);
    let roots = h.roots();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].id, id);
    assert_eq!(
        roots[0].acceptance[0].text,
        "計測 script が 3 回の反復で同じ値を出す"
    );
    assert!(roots[0].labels.iter().any(|l| l == pp::CONTROLLER_LABEL));
    assert!(roots[0].parent_id.is_none());
    handle.shutdown();
}

/// s2・s3 の全体: root1 完了 → o1 到達・o2 未達 → mark o1 → root2 を続きとして起票（root1 の done と events は不変）
/// → root2 完了 → mark o2 → `wait(achieved)` → その後の評価を何回しても起票 0。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_core_s2_s3_root1_to_root2_to_achieved() {
    let test = "project_controller_core_s2_s3_root1_to_root2_to_achieved";
    let Some((handle, policy)) = real_policy(test) else {
        return;
    };
    let mut h = Harness::new(test, REQUEST);
    let root1 = launched(&h.step(policy.as_ref()).await.2);
    // root1 が走っている間は見守るだけ（重複起票しない）。
    h.store.apply_transition(root1, Trigger::Accept, None).ok();
    let (_, raw, _) = h.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "continue", "{raw}");
    h.complete(root1, true, true);
    let root1_events = h.store.events_for(root1).expect("events");
    // s2: 中間目標 o1 は到達、最終 o2 は未達。
    let (_, raw, _) = h.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "mark_milestone", "{raw}");
    assert_eq!(raw["action"]["outcome_id"], "o1");
    assert_eq!(raw["gap"]["reached"], json!(["o1"]));
    assert_eq!(raw["gap"]["open"], json!(["o2"]));
    let (_, raw, applied) = h.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "launch_root", "{raw}");
    assert_eq!(raw["action"]["root"]["key"], "outcome:o2:1");
    let root2 = launched(&applied);
    assert_ne!(root1, root2);
    let r1 = h.store.get(root1).expect("get").expect("root1");
    assert_eq!(r1.status, task_core::Status::Done, "root1 stays done");
    assert_eq!(
        h.store.events_for(root1).expect("events"),
        root1_events,
        "root1 events unchanged"
    );
    let snap = h.snapshot();
    let row2 = snap
        .roots
        .iter()
        .find(|r| r.id == root2.to_string())
        .expect("root2 row");
    assert_eq!(row2.continues_root_ids, vec![root1.to_string()]);
    assert_eq!(row2.outcome_ids, vec!["o2".to_owned()]);
    // s3: root2 も完了 → mark o2 → achieved。
    h.complete(root2, true, true);
    let (_, raw, _) = h.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "mark_milestone", "{raw}");
    assert_eq!(raw["action"]["outcome_id"], "o2");
    let before = h.roots().len();
    for _ in 0..5 {
        let (_, raw, applied) = h.step(policy.as_ref()).await;
        assert_eq!(kind(&raw), "wait", "{raw}");
        assert_eq!(raw["action"]["phase"], "achieved");
        assert!(!matches!(applied, Applied::Launched { .. }));
    }
    assert_eq!(
        h.roots().len(),
        before,
        "no extra root after achieving the final goal"
    );
    assert_eq!(before, 2);
    assert_eq!(h.cos_inputs, 0);
    handle.shutdown();
}

/// s4: 同じ snapshot を 8 回並行に評価し、8 本の接続から並行に適用しても root は 1 件（重複 0）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn project_controller_core_s4_concurrent_events_create_one_root() {
    let test = "project_controller_core_s4_concurrent_events_create_one_root";
    let Some((handle, policy)) = real_policy(test) else {
        return;
    };
    let mut h = Harness::new(test, REQUEST);
    let snapshot = h.snapshot();
    let calls: Vec<_> = (0..8)
        .map(|_| {
            let (p, s) = (Arc::clone(&policy), snapshot.clone());
            tokio::spawn(async move { p.project_controller(s, LONG_DEADLINE).await })
        })
        .collect();
    let mut replies: Vec<Value> = Vec::new();
    for c in calls {
        replies.push(c.await.expect("join").expect("reply").output);
    }
    let keys: std::collections::BTreeSet<String> = replies
        .iter()
        .map(|r| {
            r["action"]["root"]["key"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(keys.len(), 1, "all evaluations propose the same key");
    let mut joins = Vec::new();
    for raw in &replies {
        let proposal = pp::parse_and_validate(&snapshot, raw).expect("valid");
        let ProjectAction::LaunchRoot { root } = proposal.action else {
            panic!("launch expected");
        };
        let path = h.path.clone();
        let project = h.project;
        joins.push(std::thread::spawn(move || {
            launch(&path, project, &root, None)
        }));
    }
    let results: Vec<(TaskId, bool)> = joins
        .into_iter()
        .map(|j| j.join().expect("thread").expect("launch"))
        .collect();
    let ids: std::collections::BTreeSet<TaskId> = results.iter().map(|r| r.0).collect();
    let created = results.iter().filter(|r| r.1).count();
    assert_eq!(ids.len(), 1, "{results:?}");
    assert_eq!(created, 1);
    assert_eq!(h.roots().len(), 1);
    let applied = Applied::Launched {
        root: *ids.iter().next().expect("id"),
        created: true,
    };
    h.record(
        &snapshot,
        &replies[0],
        &applied,
        json!({"concurrent_evaluations": 8, "concurrent_applies": 8, "created": created, "distinct_roots": ids.len()}),
    );
    handle.shutdown();
}

/// s5: done だが未取り込み（repo のある案件）→ `continue`、起票 0。取り込み後は続きへ進む。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_core_s5_not_integrated_continues() {
    let test = "project_controller_core_s5_not_integrated_continues";
    let Some((handle, policy)) = real_policy(test) else {
        return;
    };
    let mut h = Harness::new(test, REQUEST);
    h.with_repo = Some(RepoId::new());
    let root1 = launched(&h.step(policy.as_ref()).await.2);
    h.complete(root1, true, false);
    let (_, raw, applied) = h.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "continue", "{raw}");
    assert_eq!(raw["gap"]["reached"], json!([]));
    assert!(!matches!(applied, Applied::Launched { .. }));
    assert_eq!(h.roots().len(), 1);
    h.store
        .append_event(
            root1,
            &Event::IntegrationMerged {
                repo_id: h.with_repo.expect("repo"),
                branch: "b".into(),
                head_sha: "0".repeat(40),
                actor: "test".into(),
            },
        )
        .expect("merged");
    let (_, raw, _) = h.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "mark_milestone", "{raw}");
    handle.shutdown();
}

/// s5b（A9 D1）: repo を持つ案件で root が done・合格でも、delivery が `NoCommits` で見送られただけ（成果が案件の tree に
/// 無い）なら取り込み済みに数えず、achieved にならない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_core_s5b_no_commits_root_is_not_achieved() {
    let test = "project_controller_core_s5b_no_commits_root_is_not_achieved";
    let Some((handle, policy)) = real_policy(test) else {
        return;
    };
    let mut h = Harness::new(test, REQUEST);
    h.with_repo = Some(RepoId::new());
    let root1 = launched(&h.step(policy.as_ref()).await.2);
    h.complete(root1, true, false);
    for reason in [
        task_core::DeliverySkipReason::NoCommits,
        task_core::DeliverySkipReason::NotGit,
    ] {
        h.store
            .append_event(
                root1,
                &Event::DeliverySkipped {
                    reason,
                    detail: String::new(),
                    head: None,
                },
            )
            .expect("skipped");
        let (snap, raw, applied) = h.step(policy.as_ref()).await;
        let row = snap.roots.iter().find(|r| r.id == root1.to_string());
        assert!(row.is_some_and(|r| !r.integrated), "{snap:?}");
        assert_eq!(kind(&raw), "continue", "{raw}");
        assert_eq!(raw["gap"]["reached"], json!([]), "{raw}");
        assert!(!matches!(applied, Applied::Launched { .. }));
    }
    assert_eq!(h.roots().len(), 1);
    handle.shutdown();
}

/// s6: root1 が review 不合格 → `replan(review_failed)`（案件 CoS への入力 1 件）、起票 0、root1 は failed のまま。
/// 同じ失敗で 2 回目は `wait`（CoS 入力を積み増さない）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_core_s6_review_failed_replans_once() {
    let test = "project_controller_core_s6_review_failed_replans_once";
    let Some((handle, policy)) = real_policy(test) else {
        return;
    };
    let mut h = Harness::new(test, REQUEST);
    let root1 = launched(&h.step(policy.as_ref()).await.2);
    h.complete(root1, false, false);
    let r1 = h.store.get(root1).expect("get").expect("root1");
    assert_eq!(r1.status, task_core::Status::Failed);
    let (_, raw, _) = h.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "replan", "{raw}");
    assert_eq!(raw["action"]["reason_code"], "review_failed");
    let (_, raw, _) = h.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "wait", "{raw}");
    assert_eq!(h.roots().len(), 1);
    assert_eq!(h.cos_inputs, 1);
    assert_eq!(
        h.store.get(root1).expect("get").expect("r").status,
        task_core::Status::Failed
    );
    handle.shutdown();
}

/// s7: 基盤の失敗（worker の再試行不可の失敗）→ `wait`（backoff）、起票 0。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_core_s7_infra_failure_waits() {
    let test = "project_controller_core_s7_infra_failure_waits";
    let Some((handle, policy)) = real_policy(test) else {
        return;
    };
    let mut h = Harness::new(test, REQUEST);
    let root1 = launched(&h.step(policy.as_ref()).await.2);
    let s = &h.store;
    if s.get(root1).expect("get").expect("t").status == task_core::Status::Draft {
        s.apply_transition(root1, Trigger::Accept, None)
            .expect("accept");
    }
    s.apply_transition(root1, Trigger::Dispatch, None)
        .expect("dispatch");
    s.apply_transition(root1, Trigger::WorkerError { retryable: false }, None)
        .expect("error");
    assert_eq!(
        s.get(root1).expect("get").expect("t").status,
        task_core::Status::Failed
    );
    let (snap, raw, _) = h.step(policy.as_ref()).await;
    assert_eq!(snap.roots[0].failure_class.as_deref(), Some("infra"));
    assert_eq!(kind(&raw), "wait", "{raw}");
    assert_eq!(raw["action"]["next_wake_after_secs"], 1800);
    assert_eq!(h.roots().len(), 1);
    assert_eq!(h.cos_inputs, 0);
    handle.shutdown();
}

/// s8: Outcome block なし → `replan(no_outcomes)`、同じ request の版で 2 回目は `wait`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_core_s8_no_outcomes_asks_once() {
    let test = "project_controller_core_s8_no_outcomes_asks_once";
    let Some((handle, policy)) = real_policy(test) else {
        return;
    };
    let mut h = Harness::new(test, "途中目標の block が無い依頼");
    let (snap, raw, _) = h.step(policy.as_ref()).await;
    assert!(snap.outcomes.is_empty() && !snap.outcomes_invalid);
    assert_eq!(kind(&raw), "replan", "{raw}");
    assert_eq!(raw["action"]["reason_code"], "no_outcomes");
    for _ in 0..3 {
        let (_, raw, _) = h.step(policy.as_ref()).await;
        assert_eq!(kind(&raw), "wait", "{raw}");
    }
    assert_eq!(h.cos_inputs, 1);
    assert_eq!(h.roots().len(), 0);
    // 壊れた block は replan でなく人へ 1 回だけ知らせる。
    let mut bad = Harness::new(test, "```celeris-outcomes\n[{\"id\":\"o1\"}]\n```");
    let (snap, raw, _) = bad.step(policy.as_ref()).await;
    assert!(snap.outcomes_invalid);
    assert_eq!(kind(&raw), "escalate", "{raw}");
    let (_, raw, _) = bad.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "wait", "{raw}");
    assert_eq!(bad.escalations, 1);
    handle.shutdown();
}

/// s9: 開いた人の判断（root の質問）がある → `wait{waiting_on human}`、起票 0。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_core_s9_open_human_question_waits() {
    let test = "project_controller_core_s9_open_human_question_waits";
    let Some((handle, policy)) = real_policy(test) else {
        return;
    };
    let mut h = Harness::new(test, REQUEST);
    let root1 = launched(&h.step(policy.as_ref()).await.2);
    let s = &h.store;
    if s.get(root1).expect("get").expect("t").status == task_core::Status::Draft {
        s.apply_transition(root1, Trigger::Accept, None)
            .expect("accept");
    }
    s.apply_transition(root1, Trigger::Dispatch, None)
        .expect("dispatch");
    s.apply_transition(
        root1,
        Trigger::WorkerQuestion,
        Some(Event::QuestionRaised {
            run_id: "run-1".into(),
            text: "どちらの計測器を使うか".into(),
        }),
    )
    .expect("question");
    let (snap, raw, _) = h.step(policy.as_ref()).await;
    assert_eq!(snap.waits.len(), 1, "{:?}", snap.waits);
    assert_eq!(kind(&raw), "wait", "{raw}");
    assert_eq!(raw["action"]["waiting_on"][0]["kind"], "human");
    assert_eq!(raw["action"]["waiting_on"][0]["target"], root1.to_string());
    assert!(
        !raw.to_string().contains("どちらの計測器"),
        "worker text is not copied"
    );
    assert_eq!(h.roots().len(), 1);
    handle.shutdown();
}

/// s10: TS の出力を差し替えて acceptance を減らす → Rust の検証で `root_acceptance_narrowed`（安全違反）、起票 0。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_core_s10_narrowed_acceptance_is_rejected() {
    let test = "project_controller_core_s10_narrowed_acceptance_is_rejected";
    let narrowing = r#"globalThis.__celerisDispatch = async (_d, s) => ({
  action: { kind: "launch_root", root: { key: "outcome:o1:1", title: "t", objective: "o",
    acceptance: [{ text: "計測 script が動く", check: { type: "reviewer" } }],
    stages_hint: [], continues_root_ids: [], outcome_ids: ["o1"] } },
  gap: { reached: [], open: ["o1", "o2"], unknown: [], progress_epoch: 0 },
  explanation: { rule_id: "evil/narrow", signals: [] },
  based_on: { revision: s.controller.revision, human_epoch: s.controller.human_epoch,
    request_sha256: s.project.request_sha256 },
});
"#;
    let (handle, policy, _) = start_code(narrowing);
    let mut h = Harness::new(test, REQUEST);
    let (_, raw, applied) = h.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "launch_root");
    let Applied::Rejected(codes) = applied else {
        panic!("must be rejected: {applied:?}");
    };
    assert_eq!(codes, ["root_acceptance_narrowed"]);
    assert!(pp::SAFETY_VIOLATIONS.contains(&codes[0].as_str()));
    assert_eq!(h.roots().len(), 0);
    // DTO に無い欄（DB を書く指示のような）を返す出力も起票しない。
    let extra = r#"globalThis.__celerisDispatch = async () => ({ action: { kind: "wait", waiting_on: [],
  next_wake_after_secs: 0, phase: "waiting", sql: "DELETE FROM tasks" }, gap: { reached: [], open: [], unknown: [],
  progress_epoch: 0 }, explanation: { rule_id: "evil/sql", signals: [] }, based_on: { revision: 0, human_epoch: 0,
  request_sha256: "" } });
"#;
    let (handle2, policy2, _) = start_code(extra);
    let (_, _, applied) = h.step(policy2.as_ref()).await;
    assert!(
        matches!(&applied, Applied::Rejected(c) if c == &["invalid_output"]),
        "{applied:?}"
    );
    assert_eq!(h.roots().len(), 0);
    handle.shutdown();
    handle2.shutdown();
}

/// s11: 別案件の root は snapshot の scope に入らず、別案件の root を指す proposal は検証で拒否される（Denied）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_core_s11_other_project_root_is_denied() {
    let test = "project_controller_core_s11_other_project_root_is_denied";
    let Some((handle, policy)) = real_policy(test) else {
        return;
    };
    let mut h = Harness::new(test, REQUEST);
    // 別案件 B に done・合格の root を作る。
    let other = project_row(REQUEST, "pilot:other");
    h.store.project_create(&other).expect("other project");
    let other_root = {
        let spec: task_ops::add::NewTaskSpec = serde_json::from_value(json!({
            "title": "other", "objective": "o", "project_id": other.id,
            "acceptance": [{"type": "reviewer", "text": "計測 script が 3 回の反復で同じ値を出す"}],
        }))
        .expect("spec");
        let t =
            task_ops::add::create_task(&h.store, spec, OffsetDateTime::now_utc()).expect("create");
        h.complete(t.id, true, true);
        t.id
    };
    let snap = h.snapshot();
    assert!(snap.roots.is_empty());
    assert!(!ps::evidence_scope(&snap).contains(&other_root.to_string()));
    // 本物の TS は B の成果を A の到達に数えない（起票は o1 から）。
    let (_, raw, _) = h.step(policy.as_ref()).await;
    assert_eq!(raw["action"]["root"]["key"], "outcome:o1:1", "{raw}");
    // B の root を根拠に A の到達を主張する・見守る proposal は拒否される。
    let forged = format!(
        r#"globalThis.__celerisDispatch = async (_d, s) => ({{
  action: {{ kind: "mark_milestone", outcome_id: "o1", evidence_refs: ["task:{other_root}"] }},
  gap: {{ reached: ["o1"], open: ["o2"], unknown: [], progress_epoch: 1 }},
  explanation: {{ rule_id: "evil/other-project", signals: [] }},
  based_on: {{ revision: s.controller.revision, human_epoch: s.controller.human_epoch,
    request_sha256: s.project.request_sha256 }},
}});
"#
    );
    let (handle2, policy2, _) = start_code(&forged);
    let (_, _, applied) = h.step(policy2.as_ref()).await;
    assert_eq!(
        applied,
        Applied::Rejected(vec!["milestone_evidence_unknown".into()])
    );
    let watch = pp::ProjectProposal {
        action: ProjectAction::Continue {
            watch_root_ids: vec![other_root.to_string()],
            next_wake_after_secs: 60,
        },
        gap: pp::ProposalGap {
            reached: vec![],
            open: vec!["o1".into(), "o2".into()],
            unknown: vec![],
            progress_epoch: 0,
        },
        explanation: pp::ProposalExplanation {
            rule_id: "evil/watch".into(),
            signals: vec![],
        },
        based_on: pp::ProposalBasedOn {
            revision: h.controller.revision,
            human_epoch: h.controller.human_epoch,
            request_sha256: snap.project.request_sha256.clone(),
        },
    };
    let snap = h.snapshot();
    let denied: Vec<ProposalViolation> = pp::validate(&snap, &watch).expect_err("denied");
    assert_eq!(denied[0].code, "unknown_root");
    handle.shutdown();
    handle2.shutdown();
}

/// paused の案件では本物の TS は起票せず、TS を差し替えて起票させても `launch_on_paused`（安全違反）で拒否される。
/// 人が案件の状態を変えた（human_epoch が進んだ）後の古い proposal は `stale_human_epoch` で捨てる（I6・I7）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_core_s12_paused_and_stale_never_launch() {
    let test = "project_controller_core_s12_paused_and_stale_never_launch";
    let Some((handle, policy)) = real_policy(test) else {
        return;
    };
    let mut h = Harness::new(test, REQUEST);
    let snap = h.snapshot();
    let raw = policy
        .project_controller(snap.clone(), LONG_DEADLINE)
        .await
        .expect("reply")
        .output;
    assert_eq!(kind(&raw), "launch_root");
    // 人が判断と適用の間に案件を止めた。
    h.store
        .project_set_status(h.project, ProjectStatus::Paused)
        .expect("pause");
    h.controller.human_epoch += 1;
    let stale = h.apply(&snap, &raw);
    assert_eq!(stale, Applied::Rejected(vec!["stale_human_epoch".into()]));
    let fresh = h.snapshot();
    let mut forced = raw.clone();
    forced["based_on"]["human_epoch"] = json!(h.controller.human_epoch);
    forced["based_on"]["revision"] = json!(h.controller.revision);
    let applied = h.apply(&fresh, &forced);
    assert_eq!(applied, Applied::Rejected(vec!["launch_on_paused".into()]));
    let (_, raw, _) = h.step(policy.as_ref()).await;
    assert_eq!(kind(&raw), "wait", "{raw}");
    assert_eq!(h.roots().len(), 0);
    handle.shutdown();
}

/// escalate の category は CoS triage の `human_required` の名と同じ（task-core は celeris の config に依存しない）。
#[test]
fn project_controller_core_escalation_categories_match_cos_triage() {
    assert_eq!(
        pp::ESCALATION_CATEGORIES,
        celeris::config::HUMAN_REQUIRED_REASONS
    );
}
