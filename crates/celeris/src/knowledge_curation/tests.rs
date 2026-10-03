//! ADR-0131 付記 D10: 日次整理 job の入力準備・dry_run・承認後の apply・1 件の報告。
//! KB は tempdir、store はメモリ上、時計は `now` 引数で注入する（実時間の sleep・network は使わない）。

use super::*;
use task_core::NotificationStore;
use task_core::org::{OrgKind, OrgNode};
use task_core::report::ReportStore;
use task_core::{CronCatchUp, CronOverlap, CronTaskTemplate, SqliteStore, Tier, Trigger};
use task_ops::cron_jobs::{CronFireContext, NewCronJob};
use task_ops::knowledge_curation::{InboxProposal, KbAction};

struct Env {
    store: SqliteStore,
    kb: tempfile::TempDir,
    ws: tempfile::TempDir,
    state: tempfile::TempDir,
    job: CronJob,
    /// job を作った時刻（注入した時計の起点）。
    t0: OffsetDateTime,
}

const PAGE_A: &str = "---\ntitle: A\n---\n古い記述\n";
const PAGE_B: &str = "---\ntitle: B\n---\nA と同じ趣旨\n";

fn view(ws: &Path) -> ViewContext {
    ViewContext {
        workspace_root: ws.to_path_buf(),
        retry_backoff_base: std::time::Duration::from_secs(1),
        retry_backoff_max: std::time::Duration::from_secs(1),
        max_requeues: 3,
        clusters: Default::default(),
    }
}

fn setup(mode: &str) -> Env {
    let store = SqliteStore::open_in_memory().expect("open");
    // 時計: 実時間の 2 日前に job を作り、以後は `t0` からの相対で進める（store が自分で付ける
    // `updated_at` は実時間なので、それより後の時刻で tick する）。
    let t0 = OffsetDateTime::now_utc() - time::Duration::days(2);
    store
        .org_upsert(&OrgNode {
            profile: Default::default(),
            id: "coding".into(),
            parent_id: None,
            name: "coding".into(),
            kind: OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            position: 0,
            created_at: t0,
            updated_at: t0,
        })
        .expect("org");
    let kb = tempfile::tempdir().expect("kb");
    std::fs::write(kb.path().join("README.md"), "# KB\n").expect("readme");
    std::fs::create_dir_all(kb.path().join("projects")).expect("dir");
    std::fs::write(kb.path().join("projects/a.md"), PAGE_A).expect("a");
    std::fs::write(kb.path().join("projects/b.md"), PAGE_B).expect("b");
    let mut extra = BTreeMap::new();
    extra.insert("mode".to_string(), serde_json::json!(mode));
    let job = task_ops::cron_jobs::create_job(
        &store,
        &CronFireContext::default(),
        NewCronJob {
            name: "daily-curation".into(),
            schedule: "30 4 * * *".into(),
            timezone: "UTC".into(),
            overlap: CronOverlap::Skip,
            catch_up: CronCatchUp::Latest,
            enabled: true,
            template: CronTaskTemplate {
                title: "日次整理: {date}".into(),
                objective: "KB と受信箱を整理する".into(),
                acceptance: vec![
                    serde_json::json!({"type": "artifact_exists", "name": "curation-plan.json"}),
                ],
                harness: Some(curation::CURATION_HARNESS.into()),
                lane: Some(Tier::Cheap),
                assignee: Some("coding".into()),
                extra,
                ..CronTaskTemplate::default()
            },
        },
        t0,
    )
    .expect("job");
    Env {
        store,
        kb,
        ws: tempfile::tempdir().expect("ws"),
        state: tempfile::tempdir().expect("state"),
        job,
        t0,
    }
}

impl Env {
    fn now(&self, hours: i64) -> OffsetDateTime {
        self.t0 + time::Duration::hours(hours)
    }

    fn tick(&self, now: OffsetDateTime) -> TickOutcome {
        tick(
            &self.store,
            self.kb.path(),
            self.ws.path(),
            &self.state.path().join("state.json"),
            &view(self.ws.path()),
            now,
        )
        .expect("tick")
    }

    /// cron を発火させて日次整理 task を 1 件作る。
    fn fire(&self, now: OffsetDateTime) -> Task {
        let ids: Vec<TaskId> = task_ops::cron_jobs::fire_due(&self.store, now)
            .into_iter()
            .map(|r| r.expect("fire"))
            .filter_map(|o| o.task_id)
            .collect();
        assert_eq!(ids.len(), 1, "one task per fire");
        self.store.get(ids[0]).expect("get").expect("task")
    }

    fn dir(&self, task: &Task) -> PathBuf {
        workspace_dir(task, self.ws.path()).expect("local")
    }

    fn artifacts(&self, task: &Task) -> PathBuf {
        let dir = artifacts_dir(task, self.ws.path()).expect("local");
        std::fs::create_dir_all(&dir).expect("artifacts");
        dir
    }

    /// 偽 worker: 写しを見て計画を書き、task を done にする。
    fn worker_done(&self, task: &Task, plan: &CurationPlan, summary: Option<&str>) {
        let dir = self.artifacts(task);
        std::fs::write(
            dir.join("curation-plan.json"),
            serde_json::to_string_pretty(plan).expect("plan"),
        )
        .expect("write plan");
        if let Some(s) = summary {
            std::fs::write(dir.join("daily-summary.md"), s).expect("summary");
        }
        for trigger in [Trigger::Dispatch, Trigger::WorkerDone, Trigger::ReviewPass] {
            self.store
                .apply_transition(task.id, trigger, None)
                .expect("transition");
        }
    }

    fn kb_snapshot(&self) -> BTreeMap<String, String> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
            for e in std::fs::read_dir(dir).expect("read_dir").flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(root, &p, out);
                } else {
                    let rel = p.strip_prefix(root).expect("rel").display().to_string();
                    out.insert(rel, std::fs::read_to_string(&p).unwrap_or_default());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(self.kb.path(), self.kb.path(), &mut out);
        out
    }

    fn reports(&self, task: &Task) -> Vec<Report> {
        self.store
            .report_list(&task_core::ReportFilter {
                task_id: Some(task.id),
                ..Default::default()
            })
            .expect("reports")
    }

    fn approval(&self, task: &Task) -> Option<task_core::DecisionRow> {
        open_or_answered(&self.store, task).expect("decisions")
    }

    fn answer(&self, task: &Task, option: &str) {
        let row = self.approval(task).expect("decision");
        self.store
            .append_event(
                task.id,
                &Event::DecisionAnswered {
                    id: row.id,
                    option: option.to_string(),
                    note: None,
                    by: "human".into(),
                },
            )
            .expect("answer");
    }
}

/// b を a に統合し、a を直す計画（ハッシュは本番 KB の今の内容）。
fn merge_plan(inbox: Vec<InboxProposal>) -> CurationPlan {
    CurationPlan {
        version: 1,
        kb: vec![KbAction {
            path: "projects/b.md".into(),
            action: Action::Merge,
            target: Some("projects/a.md".into()),
            reason: "a と同じ趣旨".into(),
            content: Some("---\ntitle: A\n---\n統合した記述\n".into()),
            expected_hash: Some(curation::content_hash(PAGE_B)),
            target_hash: Some(curation::content_hash(PAGE_A)),
        }],
        inbox,
        human_decisions: Vec::new(),
    }
}

#[test]
fn knowledge_curation_job_prepares_inputs_before_the_run() {
    let env = setup("dry_run");
    // 前回の基準（job 作成時刻）より後に終端になった普通の task。報告の抜粋に入る。
    let spec: task_ops::add::NewTaskSpec = serde_json::from_value(serde_json::json!({
        "title": "普通の仕事", "objective": "実装する", "acceptance": [], "assignee": "coding"
    }))
    .expect("spec");
    let other =
        task_ops::add::create_support_task(&env.store, spec, &[], &[], env.t0).expect("other");
    for trigger in [Trigger::Dispatch, Trigger::WorkerError { retryable: false }] {
        env.store
            .apply_transition(other.id, trigger, None)
            .expect("transition");
    }
    let now = env.now(50);
    let task = env.fire(now);
    let out = env.tick(now);
    assert_eq!(out.prepared, vec![task.id]);

    let inputs = env.dir(&task).join("inputs");
    assert_eq!(
        std::fs::read_to_string(inputs.join("kb/projects/a.md")).expect("copy"),
        PAGE_A
    );
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(inputs.join("manifest.json")).expect("m"))
            .expect("json");
    assert_eq!(manifest["mode"], "dry_run");
    assert_eq!(manifest["job_id"], env.job.id.to_string());
    let inbox: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(inputs.join("inbox.json")).expect("i"))
            .expect("json");
    assert!(inbox["suppressed"].is_object());
    assert!(inbox["attention"].is_array());
    let candidates = inbox["candidates"].as_array().expect("candidates");
    assert!(
        candidates
            .iter()
            .any(|c| c["task_id"] == other.id.to_string() && c["kind"] == "failed"),
        "{inbox}"
    );
    let reports: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(inputs.join("reports.json")).expect("r"))
            .expect("json");
    let ids: Vec<&str> = reports["reports"]
        .as_array()
        .expect("reports")
        .iter()
        .filter_map(|r| r["task_id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec![other.id.to_string().as_str()],
        "日次整理 task 自身は入らない"
    );

    // 2 回目の tick では作り直さない。
    assert!(
        env.tick(now + time::Duration::minutes(1))
            .prepared
            .is_empty()
    );
}

#[test]
fn knowledge_curation_job_dry_run_leaves_the_kb_unchanged_and_writes_the_diff() {
    let env = setup("dry_run");
    let now = env.now(50);
    let task = env.fire(now);
    env.tick(now);
    let before = env.kb_snapshot();
    env.worker_done(&task, &merge_plan(Vec::new()), None);
    let task = env.store.get(task.id).expect("get").expect("task");
    assert_eq!(task.status, Status::Done);

    let out = env.tick(now + time::Duration::hours(1));
    assert_eq!(out.reported, vec![task.id]);
    assert!(out.applied.is_empty());
    assert_eq!(env.kb_snapshot(), before, "dry_run は本番 KB を書かない");
    let diff = std::fs::read_to_string(env.artifacts(&task).join("curation.diff")).expect("diff");
    assert!(diff.contains("--- a/projects/b.md"), "{diff}");
    assert!(env.approval(&task).is_none(), "dry_run は承認を求めない");
    let summary =
        std::fs::read_to_string(env.artifacts(&task).join("daily-summary.md")).expect("summary");
    assert!(summary.contains("統合 1"), "{summary}");
    assert!(summary.contains("`projects/b.md`"), "{summary}");
    assert!(summary.contains("## 人が判断すべき残り"), "{summary}");
}

#[test]
fn knowledge_curation_job_reports_exactly_one_summary_per_task() {
    let env = setup("dry_run");
    let now = env.now(50);
    let task = env.fire(now);
    env.tick(now);
    env.worker_done(
        &task,
        &merge_plan(Vec::new()),
        Some("# 日次整理（担当）\n\n- 統合 1\n"),
    );
    for h in 1..=3 {
        env.tick(now + time::Duration::hours(h));
    }
    let reports = env.reports(&task);
    assert_eq!(reports.len(), 1, "報告は 1 件だけ");
    assert!(reports[0].headline.starts_with("日次整理"));
    assert!(reports[0].body.contains("担当の要約"));
    // 報告のまとめの対象にもならない（`reports::schedule_report_compaction` が飛ばす）。
    assert!(curation::is_curation_task(&task));
    // 日次整理専用の通知は作らない。
    assert!(
        env.store
            .notification_pending()
            .expect("notifications")
            .is_empty()
    );
}

#[test]
fn knowledge_curation_job_apply_waits_for_the_human_approval() {
    let env = setup("apply");
    let now = env.now(50);
    let task = env.fire(now);
    assert_eq!(task_ops::cron_jobs::task_mode(&task), "apply");
    env.tick(now);
    let before = env.kb_snapshot();
    env.worker_done(&task, &merge_plan(Vec::new()), None);

    env.tick(now + time::Duration::hours(1));
    assert_eq!(env.kb_snapshot(), before, "承認前は適用しない");
    let row = env.approval(&task).expect("decision");
    assert_eq!(row.status, DecisionStatus::Open);
    let approve = row
        .request
        .options
        .iter()
        .find(|o| o.key.starts_with(APPROVE_PREFIX))
        .map(|o| o.key.clone())
        .expect("approve option");
    assert!(env.reports(&task)[0].body.contains(&approve));
    // 未回答のまま何度 tick しても適用しない。
    for h in 2..=4 {
        assert!(env.tick(now + time::Duration::hours(h)).applied.is_empty());
    }
    assert_eq!(env.kb_snapshot(), before);

    env.answer(&task, &approve);
    let out = env.tick(now + time::Duration::hours(5));
    assert_eq!(out.applied, vec![task.id]);
    assert!(!env.kb.path().join("projects/b.md").exists());
    assert!(
        std::fs::read_to_string(env.kb.path().join("projects/a.md"))
            .expect("a")
            .contains("統合した記述")
    );
    let date = local_date(&env.job, now + time::Duration::hours(5));
    let log = std::fs::read_to_string(env.kb.path().join(format!("_curation/{date}.md")))
        .expect("curation log");
    assert!(log.contains("projects/b.md"));
    assert_eq!(env.reports(&task).len(), 1, "反映しても報告は増やさない");
}

#[test]
fn knowledge_curation_job_apply_is_not_applied_when_rejected() {
    let env = setup("apply");
    let now = env.now(50);
    let task = env.fire(now);
    env.tick(now);
    let before = env.kb_snapshot();
    env.worker_done(&task, &merge_plan(Vec::new()), None);
    env.tick(now + time::Duration::hours(1));
    env.answer(&task, REJECT_OPTION);
    assert!(env.tick(now + time::Duration::hours(2)).applied.is_empty());
    assert_eq!(env.kb_snapshot(), before);
}

#[test]
fn knowledge_curation_job_apply_is_not_applied_when_the_page_changed_after_the_report() {
    let env = setup("apply");
    let now = env.now(50);
    let task = env.fire(now);
    env.tick(now);
    env.worker_done(&task, &merge_plan(Vec::new()), None);
    env.tick(now + time::Duration::hours(1));
    let approve = env
        .approval(&task)
        .expect("decision")
        .request
        .options
        .iter()
        .find(|o| o.key.starts_with(APPROVE_PREFIX))
        .map(|o| o.key.clone())
        .expect("approve");
    // 報告の後に人が元ページを直した。
    std::fs::write(
        env.kb.path().join("projects/b.md"),
        "---\ntitle: B\n---\n人の加筆\n",
    )
    .expect("edit");
    let before = env.kb_snapshot();
    env.answer(&task, &approve);
    assert!(env.tick(now + time::Duration::hours(2)).applied.is_empty());
    assert_eq!(env.kb_snapshot(), before, "元ページが変わったら適用しない");
    let state = load_state(&env.state.path().join("state.json"));
    assert_eq!(state.tasks[&task.id.to_string()].phase, Phase::Stale);
}

#[test]
fn knowledge_curation_job_rejects_inbox_proposals_outside_the_inputs() {
    let env = setup("dry_run");
    let now = env.now(50);
    let task = env.fire(now);
    env.tick(now);
    let before = env.kb_snapshot();
    let stranger = TaskId::new().to_string();
    env.worker_done(
        &task,
        &merge_plan(vec![InboxProposal {
            task_id: stranger,
            proposal: "cancel".into(),
            reason: "古い".into(),
        }]),
        None,
    );
    env.tick(now + time::Duration::hours(1));
    assert_eq!(env.kb_snapshot(), before);
    let reports = env.reports(&task);
    assert_eq!(reports.len(), 1);
    assert!(
        reports[0].body.contains("検証できなかった"),
        "{}",
        reports[0].body
    );
}
