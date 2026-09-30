use super::*;
use task_core::org::OrgKind;
use task_core::report::{ReportKind, ReportStore};
use task_core::{Report, ReportFilter, ReportId, SqliteStore};

fn org_node(id: &str, parent: Option<&str>, kind: OrgKind) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        profile: Default::default(),
        id: id.into(),
        parent_id: parent.map(str::to_string),
        name: id.into(),
        kind,
        genre: None,
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

fn store_with_org() -> SqliteStore {
    let store = SqliteStore::open_in_memory().expect("open");
    for n in [
        org_node("secretary", None, OrgKind::Secretary),
        org_node("coding", Some("secretary"), OrgKind::Department),
        org_node("coding-poc", Some("coding"), OrgKind::Section),
    ] {
        store.org_upsert(&n).expect("seed");
    }
    store
}

/// 案件を 1 件作って id を返す（まとめタスクの作成経路が案件の実在を検証するため）。
fn seed_project(store: &SqliteStore) -> ProjectId {
    let now = OffsetDateTime::now_utc();
    let project = task_core::Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "案件".into(),
        request: "依頼".into(),
        status: task_core::ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).expect("project");
    project.id
}

fn child_report(project: Option<ProjectId>, at: OffsetDateTime, n: usize) -> Report {
    Report {
        id: ReportId::new(),
        project_id: project,
        node_id: "coding-poc".into(),
        task_id: None,
        kind: ReportKind::Result,
        level: 2,
        headline: format!("結果 {n}"),
        body: format!("本文 {n}"),
        sources: Vec::new(),
        read_at: None,
        created_at: at,
    }
}

#[test]
fn four_reports_schedule_a_run_but_three_do_not() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let project = seed_project(&store);
    let cfg = ReportsConfig::default();

    for n in 0..3 {
        store
            .report_append(&child_report(Some(project), now, n))
            .expect("append");
    }
    assert!(
        schedule_report_compaction(&store, &cfg, &[], &[], now)
            .expect("schedule")
            .is_empty(),
        "3 件では起きない"
    );

    store
        .report_append(&child_report(Some(project), now, 3))
        .expect("append");
    let created = schedule_report_compaction(&store, &cfg, &[], &[], now).expect("schedule");
    assert_eq!(created.len(), 1, "4 件で起きる");

    let task = store.get(created[0]).expect("get").expect("some");
    assert_eq!(task.kind, TaskKind::Execute);
    assert_eq!(task.assignee.as_deref(), Some("coding"));
    assert_eq!(task.project_id, Some(project));
    assert_eq!(task.role.as_deref(), Some(report::COMPACTION_ROLE));
    for n in 0..4 {
        assert!(
            task.objective.contains(&format!("結果 {n}")),
            "{}",
            task.objective
        );
        assert!(
            task.objective.contains(&format!("本文 {n}")),
            "{}",
            task.objective
        );
    }

    // 開いているまとめがある間は二重に作らない。
    assert!(
        schedule_report_compaction(&store, &cfg, &[], &[], now)
            .expect("schedule")
            .is_empty()
    );
}

#[test]
fn an_old_report_schedules_a_run_even_below_the_count() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let cfg = ReportsConfig::default();
    store
        .report_append(&child_report(
            Some(seed_project(&store)),
            now - time::Duration::hours(1),
            0,
        ))
        .expect("append");
    assert!(
        schedule_report_compaction(&store, &cfg, &[], &[], now)
            .expect("schedule")
            .is_empty(),
        "1 時間では起きない"
    );
    let created =
        schedule_report_compaction(&store, &cfg, &[], &[], now + time::Duration::hours(2))
            .expect("schedule");
    assert_eq!(created.len(), 1, "2 時間経過で起きる");
}

/// 監査 H-2: まとめ run が失敗し続けても、`compress_after_secs` が経つまでは次のまとめを作らない。
#[test]
fn a_recently_failed_compaction_task_backs_off_until_compress_after_secs_passes() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let project = seed_project(&store);
    let cfg = ReportsConfig::default();

    for n in 0..4 {
        store
            .report_append(&child_report(Some(project), now, n))
            .expect("append");
    }

    // 同じノード・同じ案件の、直近失敗したまとめタスク。
    let node = OrgNode {
        profile: Default::default(),
        id: "coding".into(),
        parent_id: Some("secretary".into()),
        name: "coding".into(),
        kind: OrgKind::Department,
        genre: None,
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    };
    let failed = task_ops::add::create_support_task(
        &store,
        compaction_spec(&node, Some(project), &[]),
        &[],
        &[],
        now - time::Duration::minutes(10),
    )
    .expect("support task");
    store
        .acquire_lease(failed.id, "run-failed", std::time::Duration::from_secs(60))
        .expect("lease");
    let outcome = store
        .apply_transition(
            failed.id,
            task_core::Trigger::WorkerError { retryable: false },
            None,
        )
        .expect("fail");
    assert_eq!(outcome.next, Status::Failed);

    assert!(
        schedule_report_compaction(&store, &cfg, &[], &[], now)
            .expect("schedule")
            .is_empty(),
        "失敗直後はバックオフされ、次のまとめを作らない"
    );

    // `compress_after_secs` が経てば作られる。
    let later =
        now + time::Duration::seconds(cfg.compress_after_secs as i64) + time::Duration::minutes(11);
    let created = schedule_report_compaction(&store, &cfg, &[], &[], later).expect("schedule");
    assert_eq!(created.len(), 1, "バックオフ期間を過ぎればまた作られる");
}

#[test]
fn reports_of_different_projects_get_one_run_each() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let cfg = ReportsConfig::default();
    let a = seed_project(&store);
    let b = seed_project(&store);
    for n in 0..4 {
        store
            .report_append(&child_report(Some(a), now, n))
            .expect("append");
        store
            .report_append(&child_report(Some(b), now, n))
            .expect("append");
    }
    let created = schedule_report_compaction(&store, &cfg, &[], &[], now).expect("schedule");
    assert_eq!(created.len(), 2);
    let projects: Vec<_> = created
        .iter()
        .filter_map(|id| store.get(*id).ok().flatten())
        .filter_map(|t| t.project_id)
        .collect();
    assert!(projects.contains(&a) && projects.contains(&b));
}

#[test]
fn once_the_summary_exists_the_children_are_no_longer_pending() {
    let store = store_with_org();
    let now = OffsetDateTime::now_utc();
    let project = seed_project(&store);
    let cfg = ReportsConfig::default();
    let children: Vec<Report> = (0..4)
        .map(|n| child_report(Some(project), now, n))
        .collect();
    store.report_append_all(&children).expect("append");
    let created = schedule_report_compaction(&store, &cfg, &[], &[], now).expect("schedule");
    assert_eq!(created.len(), 1);

    // まとめの run が done になったときに作られる報告（`task-dispatch` と同じ形）を手で入れる。
    let summary = Report {
        id: ReportId::new(),
        project_id: Some(project),
        node_id: "coding".into(),
        task_id: Some(created[0]),
        kind: ReportKind::Result,
        level: 1,
        headline: "まとめ".into(),
        body: "まとめ本文".into(),
        sources: children.iter().map(|c| c.id).collect(),
        read_at: None,
        created_at: now,
    };
    store.report_append(&summary).expect("append");
    assert!(
        store
            .report_unreviewed_children("coding")
            .expect("pending")
            .is_empty()
    );

    // まとめの報告は、その 1 段上（秘書）のレビュー対象になる。
    let up = store
        .report_unreviewed_children("secretary")
        .expect("pending");
    assert_eq!(up.len(), 1);
    assert_eq!(up[0].id, summary.id);
    assert_eq!(
        store
            .report_list(&ReportFilter::default())
            .expect("list")
            .len(),
        5
    );
}
