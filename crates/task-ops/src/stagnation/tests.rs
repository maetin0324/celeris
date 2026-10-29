//! ADR 2026-10-27-project-stagnation-inbox: 時計（`now`）を差し替えて、各種類が閾値で 1 件だけ出て、再検出で
//! 増えず（同じ key・同じ since）、解消で消えることを確かめる。

use super::*;
use task_core::SqliteStore;

fn t0() -> OffsetDateTime {
    OffsetDateTime::parse("2026-10-25T14:51:00Z", &Rfc3339).expect("t0")
}

fn secs(n: i64) -> time::Duration {
    time::Duration::seconds(n)
}

fn policy() -> StagnationPolicy {
    StagnationPolicy {
        project_idle_secs: 600,
        human_wait_secs: 300,
        release_unpromoted_secs: 900,
        task_unscheduled_secs: 1200,
    }
}

fn facts() -> StagnationFacts {
    StagnationFacts {
        projects: vec![ProjectFact {
            project_id: ProjectId::new(),
            title: "P0-2".to_string(),
            open_tasks: 2,
            last_event_at: t0(),
        }],
        releases: vec![
            ReleaseFact {
                sha12: "ca6f52cb0001".to_string(),
                verified_at: t0(),
            },
            ReleaseFact {
                sha12: "ca6f52cb0002".to_string(),
                verified_at: t0() + secs(60),
            },
        ],
        ready_tasks: vec![ReadyTaskFact {
            task_id: TaskId::new(),
            project_id: None,
            title: "01M5HDSN3A".to_string(),
            last_event_at: t0(),
            in_flight: false,
            waiting: false,
        }],
    }
}

fn of(out: &[StagnationFinding], kind: StagnationKind) -> Vec<&StagnationFinding> {
    out.iter().filter(|f| f.kind == kind).collect()
}

#[test]
fn each_kind_appears_exactly_at_its_threshold() {
    let f = facts();
    let p = policy();
    let cases = [
        (StagnationKind::ProjectStalled, p.project_idle_secs as i64),
        (
            StagnationKind::ReleaseUnpromoted,
            p.release_unpromoted_secs as i64,
        ),
        (
            StagnationKind::TaskUnscheduled,
            p.task_unscheduled_secs as i64,
        ),
    ];
    for (kind, threshold) in cases {
        assert!(
            of(&detect(&f, &p, t0() + secs(threshold - 1)), kind).is_empty(),
            "{kind:?} before threshold"
        );
        let at = detect(&f, &p, t0() + secs(threshold));
        assert_eq!(of(&at, kind).len(), 1, "{kind:?} at threshold: {at:?}");
        assert_eq!(of(&at, kind)[0].since, "2026-10-25T14:51:00Z");
    }
    // 2 件の release は最も古い verify を since にした 1 件（subject は最新）。
    let out = detect(&f, &p, t0() + secs(3600));
    let r = of(&out, StagnationKind::ReleaseUnpromoted);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].key, "release_unpromoted-selfdeploy");
    assert_eq!(r[0].subject, "ca6f52cb0002");
    assert!(r[0].detail.contains("ca6f52cb0001") && r[0].detail.contains("2 件"));
}

#[test]
fn redetection_keeps_the_same_key_and_revision() {
    let f = facts();
    let p = policy();
    let first = detect(&f, &p, t0() + secs(3600));
    assert_eq!(first.len(), 3);
    for later in [3601, 7200, 16 * 3600] {
        let again = detect(&f, &p, t0() + secs(later));
        let keys = |v: &[StagnationFinding]| {
            v.iter()
                .map(|x| (x.key.clone(), x.since.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&again), keys(&first), "no new items at +{later}s");
    }
}

#[test]
fn resolution_closes_each_kind() {
    let p = policy();
    let now = t0() + secs(3600);
    // 案件: 新しい出来事が起きた・開いている task が無くなった。
    let mut f = facts();
    f.projects[0].last_event_at = now - secs(10);
    assert!(of(&detect(&f, &p, now), StagnationKind::ProjectStalled).is_empty());
    let mut f = facts();
    f.projects[0].open_tasks = 0;
    assert!(of(&detect(&f, &p, now), StagnationKind::ProjectStalled).is_empty());
    // release: 昇格された（事実から消える）。
    let mut f = facts();
    f.releases.clear();
    assert!(of(&detect(&f, &p, now), StagnationKind::ReleaseUnpromoted).is_empty());
    // task: run が走った・待ちの理由が付いた。
    let mut f = facts();
    f.ready_tasks[0].in_flight = true;
    assert!(of(&detect(&f, &p, now), StagnationKind::TaskUnscheduled).is_empty());
    let mut f = facts();
    f.ready_tasks[0].waiting = true;
    assert!(of(&detect(&f, &p, now), StagnationKind::TaskUnscheduled).is_empty());
    // 閾値 0 はその種類を止める。
    let off = StagnationPolicy {
        project_idle_secs: 0,
        human_wait_secs: 0,
        release_unpromoted_secs: 0,
        task_unscheduled_secs: 0,
    };
    assert!(detect(&facts(), &off, now).is_empty());
}

#[test]
fn human_wait_aging_groups_by_project_and_takes_the_oldest_as_revision() {
    let w = |id: &str, project: Option<&str>, created: &str, age: u64| HumanWait {
        item_id: id.to_string(),
        title: id.to_string(),
        project_id: project.map(str::to_string),
        created_at: created.to_string(),
        age_secs: age,
    };
    let waits = vec![
        w("plan_gate-a", Some("p1"), "2026-10-25T14:51:00Z", 11_760),
        w("question-b", Some("p1"), "2026-10-25T16:00:00Z", 7_620),
        w("decision-c", Some("p1"), "2026-10-25T18:00:00Z", 60),
        w("question-d", None, "2026-10-25T15:00:00Z", 11_220),
    ];
    let out = human_wait_aging(&waits, 7200);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].key, "human_wait_aging-none");
    assert_eq!(out[1].key, "human_wait_aging-p1");
    assert_eq!(out[1].since, "2026-10-25T14:51:00Z");
    assert_eq!(
        out[1].waits.len(),
        2,
        "the 60 s decision is below threshold"
    );
    // 最も古い待ちが答えられると、同じ key のまま since（revision）が進む。
    let rest: Vec<HumanWait> = waits.iter().skip(1).cloned().collect();
    let out2 = human_wait_aging(&rest, 7200);
    let p1 = out2
        .iter()
        .find(|a| a.key == "human_wait_aging-p1")
        .expect("p1");
    assert_eq!(p1.since, "2026-10-25T16:00:00Z");
    assert!(human_wait_aging(&waits, 0).is_empty());
    assert!(human_wait_aging(&waits, 20_000).is_empty());
}

fn write(dir: &Path, name: &str, body: serde_json::Value) {
    std::fs::create_dir_all(dir).expect("mkdir");
    std::fs::write(dir.join(name), body.to_string()).expect("write");
}

#[test]
fn unpromoted_releases_are_verified_newer_than_the_promoted_one() {
    let root = tempfile::tempdir().expect("tempdir");
    let rel = |sha: &str| root.path().join(sha);
    // 昇格済み（現在）。
    write(
        &rel("aaaaaaaaaaaa"),
        "manifest.json",
        serde_json::json!({"built_at": "2026-10-26T10:00:00Z"}),
    );
    write(
        &rel("aaaaaaaaaaaa"),
        "verify.json",
        serde_json::json!({"ok": true, "at": "2026-10-26T10:30:00Z"}),
    );
    write(
        &rel("aaaaaaaaaaaa"),
        "promoted.json",
        serde_json::json!({"promoted_at": "2026-10-26T11:00:00Z"}),
    );
    // 古い未昇格（現在より前の build）は対象外。
    write(
        &rel("bbbbbbbbbbbb"),
        "manifest.json",
        serde_json::json!({"built_at": "2026-10-26T09:00:00Z"}),
    );
    write(
        &rel("bbbbbbbbbbbb"),
        "verify.json",
        serde_json::json!({"ok": true, "at": "2026-10-26T09:30:00Z"}),
    );
    // 新しい verify 済み（対象）。
    write(
        &rel("cccccccccccc"),
        "manifest.json",
        serde_json::json!({"built_at": "2026-10-26T12:00:00Z"}),
    );
    write(
        &rel("cccccccccccc"),
        "verify.json",
        serde_json::json!({"ok": true, "at": "2026-10-26T12:20:00Z"}),
    );
    // 新しいが verify 失敗・未検証（対象外）。
    write(
        &rel("dddddddddddd"),
        "manifest.json",
        serde_json::json!({"built_at": "2026-10-26T13:00:00Z"}),
    );
    write(
        &rel("dddddddddddd"),
        "verify.json",
        serde_json::json!({"ok": false, "at": "2026-10-26T13:20:00Z"}),
    );
    write(
        &rel("eeeeeeeeeeee"),
        "manifest.json",
        serde_json::json!({"built_at": "2026-10-26T14:00:00Z"}),
    );
    std::fs::create_dir_all(root.path().join(".build")).expect("mkdir");

    let found = read_unpromoted_releases(root.path()).expect("read");
    assert_eq!(
        found,
        vec![ReleaseFact {
            sha12: "cccccccccccc".to_string(),
            verified_at: parse_ts("2026-10-26T12:20:00Z").expect("ts"),
        }]
    );
    // 昇格すれば消える。
    write(
        &rel("cccccccccccc"),
        "promoted.json",
        serde_json::json!({"promoted_at": "2026-10-26T14:04:00Z"}),
    );
    assert!(
        read_unpromoted_releases(root.path())
            .expect("read")
            .is_empty()
    );
    // ADR 2026-10-29-stagnation-triage-notify-wiring D2: 無い dir は空、読めない dir（file）は Err（昇格扱いにしない）。
    assert!(
        read_unpromoted_releases(&root.path().join("missing"))
            .expect("missing")
            .is_empty()
    );
    let file = root.path().join("not-a-dir");
    std::fs::write(&file, "x").expect("file");
    assert!(read_unpromoted_releases(&file).is_err());
}

fn task(status: Status, project: Option<ProjectId>) -> Task {
    let mut t: Task = serde_json::from_value(serde_json::json!({
        "id": TaskId::new(),
        "kind": "execute",
        "title": "t",
        "objective": "o",
        "acceptance": [],
        "inputs": [],
        "depends_on": [],
        "status": "ready",
        "priority": 0,
        "worker_hint": {"tier": "standard"},
        "workspace": {"kind": "local", "path": "w"},
        "budget": {"max_turns": 1, "max_wall_secs": 60, "max_retries": 0},
        "attempts": 0,
        "created_at": "2026-10-25T14:51:00Z",
        "updated_at": "2026-10-25T14:51:00Z",
    }))
    .expect("task json");
    t.status = status;
    t.project_id = project;
    t
}

/// gather: store の events と案件から事実を作る（since は events の ts。メモリに状態を持たない）。
#[test]
fn gather_reads_project_and_ready_task_facts_from_the_store() {
    let store = SqliteStore::open_in_memory().expect("store");
    let now = t0();
    let project = task_core::Project {
        auto_advance: false,
        auto_integrate: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "P0-2".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).expect("project");
    let ready = task(Status::Ready, Some(project.id));
    store.insert(&ready).expect("insert");
    let events = store.latest_event_ts(&[ready.id]).expect("latest");
    let last = events
        .get(&ready.id)
        .and_then(|s| parse_ts(s))
        .unwrap_or(ready.updated_at);

    let p = policy();
    let none = HashSet::new();
    let early = last + secs(5);
    let f = gather(
        &store,
        None,
        &p,
        early,
        &none,
        &none,
        &MemoryWaits::default(),
    )
    .expect("gather");
    assert_eq!(f.projects.len(), 1);
    assert_eq!(f.projects[0].open_tasks, 1);
    assert_eq!(f.projects[0].last_event_at, last);
    assert!(
        f.ready_tasks.is_empty(),
        "below the task threshold: not even a candidate"
    );

    let late = last + secs(p.task_unscheduled_secs as i64);
    let f = gather(
        &store,
        None,
        &p,
        late,
        &none,
        &none,
        &MemoryWaits::default(),
    )
    .expect("gather");
    assert_eq!(f.ready_tasks.len(), 1);
    assert!(!f.ready_tasks[0].waiting);
    let out = detect(&f, &p, late);
    assert_eq!(of(&out, StagnationKind::TaskUnscheduled).len(), 1);
    assert_eq!(of(&out, StagnationKind::ProjectStalled).len(), 1);

    // run が走っている task・生存確認の stalled に載った task は候補にしない。
    let running: HashSet<TaskId> = [ready.id].into_iter().collect();
    let f = gather(
        &store,
        None,
        &p,
        late,
        &running,
        &none,
        &MemoryWaits::default(),
    )
    .expect("gather");
    assert!(f.ready_tasks.is_empty());
    let f = gather(
        &store,
        None,
        &p,
        late,
        &none,
        &running,
        &MemoryWaits::default(),
    )
    .expect("gather");
    assert!(f.ready_tasks.is_empty());
    // dispatcher のメモリの待ちの理由があれば waiting。
    let memory =
        MemoryWaits::from_waits([(ready.id, task_core::TaskWait::Paused { by: ready.id })]);
    let f = gather(&store, None, &p, late, &none, &none, &memory).expect("gather");
    assert!(f.ready_tasks[0].waiting);
}
