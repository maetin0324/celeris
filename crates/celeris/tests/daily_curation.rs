//! 日次整理 job の結合試験。時計と worker の成果だけを注入し、外部通信や sleep は使わない。

use std::path::{Path, PathBuf};

use celeris::config::Config;
use celeris::knowledge_curation;
use celeris::reports::{self, ReportsConfig};
use task_core::org::{OrgKind, OrgNode};
use task_core::report::{Report, ReportFilter, ReportId, ReportKind, ReportStore};
use task_core::{
    CronJobStore, CronOverlap, DecisionStatus, Event, KnowledgeRunStore, SqliteStore, Status, Task,
    TaskStore, Tier, Trigger, WorkspaceSpec,
};
use task_ops::knowledge_curation::{
    self as curation, Action, CurationPlan, HumanDecision, KbAction,
};
use task_ops::view::ViewContext;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const PAGE_A: &str = "---\ntitle: A\n---\n古い記述\n";
const PAGE_B: &str = "---\ntitle: B\n---\n重複した記述\n";

fn at(text: &str) -> OffsetDateTime {
    OffsetDateTime::parse(text, &Rfc3339).expect("fixed test time")
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: SqliteStore,
    kb: PathBuf,
    ws: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                r#"db = "test.sqlite3"

[[providers]]
id = "fake"
adapter = "fake"

[[harnesses]]
id = "knowledge-curation"
tier = "cheap"

[[cron.seed]]
name = "daily-curation"
enabled = true
schedule = "30 4 * * *"
timezone = "UTC"
overlap = "skip"
catch_up = "latest"

[cron.seed.template]
title = "日次整理: {{date}}"
objective = "KB と受信箱を整理する"
harness = "knowledge-curation"
lane = "cheap"
assignee = "coding"
mode = "{mode}"
acceptance = [{{ type = "artifact_exists", name = "curation-plan.json" }}]
"#
            ),
        )
        .expect("config");
        let config = Config::load(&config_path).expect("load seed config");
        let store = SqliteStore::open(&dir.path().join("test.sqlite3")).expect("store");
        let start = at("2026-10-03T00:00:00Z");
        for (id, parent, kind) in [
            ("secretary", None, OrgKind::Secretary),
            ("coding", Some("secretary"), OrgKind::Section),
        ] {
            store
                .org_upsert(&OrgNode {
                    profile: Default::default(),
                    id: id.into(),
                    parent_id: parent.map(str::to_string),
                    name: id.into(),
                    kind,
                    genre: None,
                    brief: String::new(),
                    position: 0,
                    created_at: start,
                    updated_at: start,
                })
                .expect("org");
        }
        assert_eq!(
            celeris::seed_cron_if_empty(&store, &config, start).unwrap(),
            1
        );
        assert_eq!(
            celeris::seed_cron_if_empty(&store, &config, start).unwrap(),
            0
        );
        let kb = dir.path().join("kb");
        std::fs::create_dir_all(kb.join("projects")).expect("kb");
        std::fs::write(kb.join("README.md"), "# KB\n").expect("readme");
        std::fs::write(kb.join("projects/a.md"), PAGE_A).expect("page a");
        std::fs::write(kb.join("projects/b.md"), PAGE_B).expect("page b");
        let ws = dir.path().join("ws");
        let state = dir.path().join("curation-state.json");
        Self {
            _dir: dir,
            store,
            kb,
            ws,
            state,
        }
    }

    fn tick(&self, now: OffsetDateTime) -> knowledge_curation::TickOutcome {
        let view = ViewContext {
            workspace_root: self.ws.clone(),
            retry_backoff_base: std::time::Duration::from_secs(1),
            retry_backoff_max: std::time::Duration::from_secs(1),
            max_requeues: 3,
            clusters: Default::default(),
        };
        knowledge_curation::tick(&self.store, &self.kb, &self.ws, &self.state, &view, now)
            .expect("curation tick")
    }

    fn task_dir(&self, task: &Task) -> PathBuf {
        match &task.workspace {
            WorkspaceSpec::Local { path, .. } if path.is_absolute() => path.clone(),
            WorkspaceSpec::Local { path, .. } if !path.as_os_str().is_empty() => self.ws.join(path),
            WorkspaceSpec::Local { .. } => self.ws.join(task.id.to_string()),
            other => panic!("unexpected workspace: {other:?}"),
        }
    }

    fn fake_worker_done(&self, task: &Task) {
        self.fake_worker_done_with(task, Vec::new());
    }

    fn fake_worker_done_with(&self, task: &Task, human_decisions: Vec<HumanDecision>) {
        let artifacts = self.task_dir(task).join("artifacts");
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        let plan = CurationPlan {
            version: 1,
            kb: vec![KbAction {
                path: "projects/b.md".into(),
                action: Action::Merge,
                target: Some("projects/a.md".into()),
                reason: "重複を統合".into(),
                content: Some("---\ntitle: A\n---\n統合した記述\n".into()),
                expected_hash: Some(curation::content_hash(PAGE_B)),
                target_hash: Some(curation::content_hash(PAGE_A)),
            }],
            inbox: Vec::new(),
            human_decisions,
        };
        std::fs::write(
            artifacts.join("curation-plan.json"),
            serde_json::to_string_pretty(&plan).expect("plan json"),
        )
        .expect("plan");
        std::fs::write(artifacts.join("daily-summary.md"), "# 担当の日次整理\n").expect("summary");
        for trigger in [Trigger::Dispatch, Trigger::WorkerDone, Trigger::ReviewPass] {
            self.store
                .apply_transition(task.id, trigger, None)
                .expect("transition");
        }
        assert_eq!(
            self.store.get(task.id).unwrap().unwrap().status,
            Status::Done
        );
    }

    fn reports(&self, task: &Task) -> Vec<Report> {
        self.store
            .report_list(&ReportFilter {
                task_id: Some(task.id),
                ..Default::default()
            })
            .expect("reports")
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).expect("read json")).expect("json")
}

#[test]
fn knowledge_curation_job_seed_schedule_inputs_dry_run_and_summary() {
    let f = Fixture::new("dry_run");
    let [job] = f
        .store
        .cron_job_list()
        .unwrap()
        .try_into()
        .expect("one seed");
    assert_eq!(job.overlap, CronOverlap::Skip);
    assert_eq!(job.template.lane, Some(Tier::Cheap));
    assert!(task_ops::cron_jobs::fire_due(&f.store, at("2026-10-03T04:29:59Z")).is_empty());
    assert!(f.store.list(None).unwrap().is_empty());

    let fire = task_ops::cron_jobs::fire_due(&f.store, at("2026-10-03T04:30:00Z"));
    assert_eq!(fire.len(), 1);
    let task_id = fire[0]
        .as_ref()
        .expect("fire")
        .task_id
        .expect("created task");
    let task = f.store.get(task_id).unwrap().unwrap();
    assert_eq!(task_ops::cron_jobs::task_mode(&task), "dry_run");
    assert!(curation::is_curation_task(&task));
    assert_eq!(f.tick(at("2026-10-03T04:30:01Z")).prepared, vec![task_id]);
    let inputs = f.task_dir(&task).join("inputs");
    assert_eq!(
        std::fs::read_to_string(inputs.join("kb/projects/a.md")).unwrap(),
        PAGE_A
    );
    for name in ["inbox.json", "reports.json", "manifest.json"] {
        assert!(inputs.join(name).exists(), "{name}");
    }
    assert!(read_json(&inputs.join("inbox.json"))["attention"].is_array());
    assert!(read_json(&inputs.join("reports.json"))["reports"].is_array());
    assert_eq!(read_json(&inputs.join("manifest.json"))["mode"], "dry_run");

    // 実行中は翌日の tick でも overlap=skip。新しい task は増えない。
    let next = task_ops::cron_jobs::fire_due(&f.store, at("2026-10-04T04:30:00Z"));
    assert_eq!(next.len(), 1);
    assert!(next[0].as_ref().unwrap().task_id.is_none());
    assert_eq!(f.store.list(None).unwrap().len(), 1);

    let original_a = std::fs::read_to_string(f.kb.join("projects/a.md")).unwrap();
    f.fake_worker_done(&task);
    assert_eq!(f.tick(at("2026-10-04T04:31:00Z")).reported, vec![task_id]);
    assert_eq!(
        std::fs::read_to_string(f.kb.join("projects/a.md")).unwrap(),
        original_a
    );
    assert_eq!(
        std::fs::read_to_string(f.kb.join("projects/b.md")).unwrap(),
        PAGE_B
    );
    assert_eq!(
        std::fs::read_to_string(f.kb.join("README.md")).unwrap(),
        "# KB\n"
    );
    assert!(!f.kb.join("_curation").exists());
    assert!(!f.kb.join("index.json").exists());
    let artifacts = f.task_dir(&task).join("artifacts");
    assert!(
        std::fs::read_to_string(artifacts.join("curation.diff"))
            .unwrap()
            .contains("projects/b.md")
    );
    assert_eq!(f.reports(&task).len(), 1);
    assert!(f.reports(&task)[0].body.contains("統合 1"));
    assert!(f.tick(at("2026-10-04T05:00:00Z")).reported.is_empty());
    assert_eq!(f.reports(&task).len(), 1);
}

#[test]
fn knowledge_curation_job_apply_without_approval_commits_pushes_and_terminal_task_maintenance() {
    let f = Fixture::new("apply");
    // KB を git にし、tempdir の bare repo を origin にする（network には出ない）。
    git(&f.kb, &["init", "-q", "-b", "main"]);
    std::fs::write(f.kb.join(".gitignore"), "index.json\n").expect("gitignore");
    git(&f.kb, &["add", "-A"]);
    git(&f.kb, &["commit", "-q", "-m", "init"]);
    let bare = f._dir.path().join("kb-remote.git");
    git(
        f._dir.path(),
        &["init", "-q", "--bare", &bare.to_string_lossy()],
    );
    git(&f.kb, &["remote", "add", "origin", &bare.to_string_lossy()]);

    let fire = task_ops::cron_jobs::fire_due(&f.store, at("2026-10-03T04:30:00Z"));
    let task_id = fire[0].as_ref().unwrap().task_id.unwrap();
    let task = f.store.get(task_id).unwrap().unwrap();
    f.tick(at("2026-10-03T04:30:01Z"));
    f.fake_worker_done(&task);
    assert_eq!(
        f.tick(at("2026-10-03T04:31:00Z")).applied,
        vec![task_id],
        "承認を待たずに適用する"
    );
    assert!(
        !f.store
            .decisions_list(Some(task_id))
            .expect("decisions")
            .iter()
            .any(|d| d.request.key == knowledge_curation::APPROVAL_KEY),
        "curation-apply の決定は出さない"
    );
    assert!(!f.kb.join("projects/b.md").exists());
    assert!(
        std::fs::read_to_string(f.kb.join("projects/a.md"))
            .unwrap()
            .contains("統合した記述")
    );
    assert!(
        std::fs::read_to_string(f.kb.join("_curation/2026-10-03.md"))
            .unwrap()
            .contains("projects/b.md")
    );
    assert!(read_json(&f.kb.join("index.json")).is_object());
    assert!(
        std::fs::read_to_string(f.kb.join("README.md"))
            .unwrap()
            .contains("## ページ一覧")
    );
    // 1 commit（題に日付と件数、本文に task id）を origin へ push した。
    let head = git(&f.kb, &["rev-parse", "HEAD"]);
    let subject = git(&f.kb, &["log", "-1", "--format=%s"]);
    assert!(
        subject.contains("2026-10-03") && subject.contains("統合 1"),
        "{subject}"
    );
    assert!(git(&f.kb, &["log", "-1", "--format=%b"]).contains(&task_id.to_string()));
    assert_eq!(git(&bare, &["rev-parse", "refs/heads/main"]), head);
    let applied: Vec<_> = f
        .store
        .events_for(task_id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::KnowledgeCurationApplied {
                commit_sha, push, ..
            } => Some((commit_sha, push)),
            _ => None,
        })
        .collect();
    assert_eq!(applied, vec![(Some(head.clone()), "pushed".to_string())]);
    assert_eq!(f.reports(&task).len(), 1);
    assert!(f.reports(&task)[0].body.contains(&head));
    assert!(f.tick(at("2026-10-03T04:32:00Z")).applied.is_empty());

    // 普通の task を終端にしても、旧式の task ごとの知識整理は起動しない。
    let now = at("2026-10-03T04:33:00Z");
    let spec: task_ops::add::NewTaskSpec = serde_json::from_value(serde_json::json!({
        "title": "通常の仕事", "objective": "実装する", "acceptance": [], "assignee": "coding"
    }))
    .expect("ordinary task spec");
    let ordinary =
        task_ops::add::create_support_task(&f.store, spec, &[], &[], now).expect("ordinary task");
    for trigger in [Trigger::Dispatch, Trigger::WorkerError { retryable: false }] {
        f.store
            .apply_transition(ordinary.id, trigger, None)
            .expect("ordinary terminal transition");
    }
    assert!(
        f.store
            .get(ordinary.id)
            .unwrap()
            .unwrap()
            .status
            .is_terminal()
    );
    assert!(f.store.knowledge_run_recent(10).unwrap().is_empty());
    f.store
        .report_append(&Report {
            id: ReportId::new(),
            project_id: None,
            node_id: "coding".into(),
            task_id: Some(ordinary.id),
            kind: ReportKind::Result,
            level: 1,
            headline: "通常の報告".into(),
            body: "作業結果".into(),
            sources: Vec::new(),
            read_at: None,
            created_at: now,
        })
        .expect("ordinary report");
    let created = reports::schedule_report_compaction(
        &f.store,
        &ReportsConfig {
            compress_after: 1,
            compress_after_secs: 0,
        },
        &[],
        &[],
        now,
    )
    .expect("compaction");
    assert_eq!(created.len(), 1);
    let compaction = f.store.get(created[0]).unwrap().unwrap();
    assert!(compaction.title.starts_with("報告のまとめ:"));
    assert!(!compaction.objective.contains("日次整理"));
    assert!(
        !f.store
            .list(None)
            .unwrap()
            .iter()
            .any(|t| t.title.starts_with("知識整理:"))
    );
}

#[test]
fn knowledge_curation_job_dry_run_bundles_human_decisions_into_one_decision() {
    let f = Fixture::new("dry_run");
    let fire = task_ops::cron_jobs::fire_due(&f.store, at("2026-10-03T04:30:00Z"));
    let task_id = fire[0].as_ref().unwrap().task_id.unwrap();
    let task = f.store.get(task_id).unwrap().unwrap();
    f.tick(at("2026-10-03T04:30:01Z"));
    f.fake_worker_done_with(
        &task,
        vec![
            HumanDecision {
                subject: "user/goals.md".into(),
                proposal: "古い目標の節を削除".into(),
                reason: "人が書いたページ".into(),
            },
            HumanDecision {
                subject: "user/profile.md".into(),
                proposal: "所属の記述を書き換え".into(),
                reason: "人が書いたページ".into(),
            },
        ],
    );
    assert_eq!(f.tick(at("2026-10-03T04:31:00Z")).reported, vec![task_id]);
    f.tick(at("2026-10-03T04:32:00Z"));
    let decisions = f.store.decisions_list(Some(task_id)).expect("decisions");
    let human: Vec<_> = decisions
        .iter()
        .filter(|d| d.request.key == knowledge_curation::HUMAN_KEY)
        .collect();
    assert_eq!(human.len(), 1, "人への候補は 1 件の decision に束ねる");
    assert_eq!(human[0].request.key, "curation-human");
    assert_eq!(human[0].status, DecisionStatus::Open);
    assert!(human[0].request.question.contains("user/goals.md"));
    assert!(human[0].request.question.contains("user/profile.md"));
    assert!(
        !decisions
            .iter()
            .any(|d| d.request.key == knowledge_curation::APPROVAL_KEY),
        "dry_run は承認を求めない"
    );
    assert_eq!(
        std::fs::read_to_string(f.kb.join("projects/b.md")).unwrap(),
        PAGE_B
    );
    assert_eq!(f.reports(&task).len(), 1);
}
