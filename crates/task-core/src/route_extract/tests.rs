use super::*;
use crate::store::SqliteStore;

fn fixture() -> SqliteStore {
    let s = SqliteStore::open_in_memory().unwrap();
    let c = s.lock().unwrap();
    c.execute("INSERT INTO tasks(id,status,kind,parent_id,priority,created_at,json) VALUES('task-1','running','task',NULL,0,'2026-01-01T00:00:00Z',?1)",[
        r#"{"title":"A task","objective":"Build it","genre":"coding","depends_on":["upstream-1","upstream-2"],"budget":{"max_turns":22,"max_wall_secs":600,"max_retries":3},"acceptance":[{"text":"works","check":{"type":"command","cmd":"cargo test -p task-core","expect_exit":0}},{"text":"docs","check":{"type":"artifact_exists","name":"docs.md"}},{"text":"review","check":{"type":"reviewer"}},{"text":"human","check":{"type":"human"}}],"repos":["repo"]}"#]).unwrap();
    c.execute("INSERT INTO projects(id,title,request,status,created_at,updated_at) VALUES('project-1','Routing project','Request','active','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",[]).unwrap();
    c.execute(
        "UPDATE tasks SET project_id='project-1' WHERE id='task-1'",
        [],
    )
    .unwrap();
    c.execute("INSERT INTO execution_plans(id,task_id,version,origin,status,json,created_at) VALUES('p1','task-1',1,'fixture','active','{}','2026-01-01T00:00:00Z')",[]).unwrap();
    c.execute("INSERT INTO work_units(id,task_id,plan_id,key,seq,kind,status,depends_on_json,retries,json,created_at,updated_at) VALUES('wu-1','task-1','p1','leaf',1,'implement','ready','[]',2,?1,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",[r#"{"objective":"Do work","budget":{"max_turns":30,"max_wall_secs":900},"checks":[{"cmd":"cargo clippy --workspace","expect_exit":0,"scope":true}],"context":{"paths":["crates/task-core/src/lib.rs"]}}"#]).unwrap();
    c.execute("INSERT INTO work_units(id,task_id,plan_id,key,seq,kind,status,depends_on_json,retries,json,created_at,updated_at) VALUES('wu-repair','task-1','p1','repair',2,'repair','ready','[]',0,'{}','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",[]).unwrap();
    c.execute("INSERT INTO runs(run_id,task_id,work_unit_id,role,seq,status,usage_json,metrics_json,started_at,finished_at) VALUES('r1','task-1','wu-1','worker',1,'completed','{}','{}','2026-01-01T00:01:00Z','2026-01-01T00:02:00Z')",[]).unwrap();
    c.execute("INSERT INTO runs(run_id,task_id,work_unit_id,role,seq,status,started_at) VALUES('r2','task-1','wu-1','worker',2,'failed','2026-01-01T00:03:00Z')",[]).unwrap();
    c.execute(
        "INSERT INTO events(task_id,seq,ts,json) VALUES('task-1',1,'2026-01-01T00:00:01Z',?1)",
        [r#"{"type":"comment","text":"prompt sk_live_abcdefghijk and private instruction"}"#],
    )
    .unwrap();
    c.execute("INSERT INTO events(task_id,seq,ts,json) VALUES('task-1',2,'2026-01-01T00:01:00Z','{\"type\":\"worker_started\",\"run_id\":\"r1\",\"model\":\"model-one\"}')",[]).unwrap();
    c.execute("INSERT INTO events(task_id,seq,ts,json) VALUES('task-1',3,'2026-01-01T00:03:00Z','{\"type\":\"worker_started\",\"run_id\":\"r2\",\"model\":\"model-two\"}')",[]).unwrap();
    c.execute("INSERT INTO events(task_id,seq,ts,json) VALUES('task-1',5,'2026-01-01T00:05:00Z','{\"type\":\"review_verdict\",\"run_id\":\"r2\",\"criterion_idx\":0,\"pass\":false,\"reason\":\"stdout sk_live_supersecret\"}')",[]).unwrap();
    c.execute("INSERT INTO events(task_id,seq,ts,json) VALUES('task-1',4,'2026-01-01T00:04:00Z',?1)",[r#"{"type":"routing_decided","record":{"harness":"old-harness","decision":{"lane":"cheap","features":{"judgment":"low","ambiguity":"low","verifiability":"high","reversibility":"high","consequence":"low","context_size":"low","tool_intensity":"low","expected_length":"low","cross_cutting":"low"}}}}"#]).unwrap();
    drop(c);
    s
}

#[test]
fn route_extract_work_unit_and_task_are_deterministic_and_as_of_excludes_future_runs() {
    let s = fixture();
    let wu = Subject {
        kind: "work_unit".into(),
        id: "wu-1".into(),
    };
    let a = features_for_subject(
        &s,
        wu.clone(),
        Some(AsOf::Timestamp("2026-01-01T00:02:30Z".into())),
        None,
    )
    .unwrap();
    let b = features_for_subject(
        &s,
        wu,
        Some(AsOf::Timestamp("2026-01-01T00:02:30Z".into())),
        None,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&a).unwrap(),
        serde_json::to_vec(&b).unwrap()
    );
    assert_eq!(a.aux.prior_runs_n, Some(1));
    assert_eq!(a.aux.acceptance_n, Some(4));
    assert_eq!(a.aux.check_command_n, Some(2));
    assert_eq!(a.aux.check_artifact_n, Some(1));
    assert_eq!(a.aux.check_reviewer_n, Some(1));
    assert_eq!(a.aux.check_human_n, Some(1));
    assert_eq!(a.aux.retries_n, Some(2));
    assert_eq!(a.aux.repairs_n, Some(1));
    assert_eq!(a.aux.budget_turns, Some(30));
    assert_eq!(a.aux.budget_wall_s, Some(900));
    assert!(a.brief.contains("works") && !a.brief.contains("決定的な検査なし"));
    let feature = crate::model_policy::TaskFeatures {
        judgment: crate::model_policy::Level::Medium,
        ambiguity: crate::model_policy::Level::Medium,
        verifiability: crate::model_policy::Level::High,
        reversibility: crate::model_policy::Level::High,
        consequence: crate::model_policy::Level::Low,
        context_size: crate::model_policy::Level::Medium,
        tool_intensity: crate::model_policy::Level::High,
        expected_length: crate::model_policy::Level::Medium,
        cross_cutting: crate::model_policy::Level::Low,
    };
    let ctx = RouteFeatureContext {
        lane: Some("standard".into()),
        harness: Some("claude-code".into()),
        task_features_v69: Some(feature),
    };
    let with_ctx = features_for_subject(
        &s,
        Subject {
            kind: "work_unit".into(),
            id: "wu-1".into(),
        },
        Some(AsOf::EventSeq(5)),
        Some(ctx),
    )
    .unwrap();
    assert_eq!(with_ctx.aux.lane.as_deref(), Some("standard"));
    assert_eq!(with_ctx.aux.harness.as_deref(), Some("claude-code"));
    assert!(with_ctx.aux.task_features_v69.is_some());
    assert!(with_ctx.brief.contains("model-two") && with_ctx.brief.contains("works"));
    assert!(!with_ctx.brief.contains("stdout") && !with_ctx.brief.contains("supersecret"));
    assert!(!with_ctx.aux.missing.iter().any(|m| {
        [
            "acceptance_n",
            "check_command_n",
            "check_artifact_n",
            "check_reviewer_n",
            "check_human_n",
            "retries_n",
            "repairs_n",
            "budget_turns",
            "budget_wall_s",
            "lane",
            "harness",
        ]
        .contains(&m.as_str())
    }));
    let by_seq = features_for_subject(
        &s,
        Subject {
            kind: "work_unit".into(),
            id: "wu-1".into(),
        },
        Some(AsOf::EventSeq(2)),
        None,
    )
    .unwrap();
    assert_eq!(by_seq.aux.prior_runs_n, Some(1));
    assert!(!a.brief.contains("private instruction") && !a.brief.contains("sk_live_"));
    assert!(
        features_for_subject(
            &s,
            Subject {
                kind: "task".into(),
                id: "task-1".into()
            },
            None,
            None,
        )
        .unwrap()
        .brief
        .contains("A task")
    );
}

#[test]
fn route_extract_outcome_reports_missing_metrics_without_git() {
    let s = fixture();
    let outcome = outcome_labels_for_run(&s, "r1", None).unwrap();
    assert_eq!(outcome.schema, ROUTE_OUTCOME_SCHEMA);
    assert!(
        outcome.labels["missing"]
            .as_array()
            .unwrap()
            .contains(&Value::String("changed_lines".into()))
    );
    let counted = outcome_labels_for_run(
        &s,
        "r1",
        Some(ChangeCount {
            changed_lines: 17,
            changed_files_n: 4,
            changed_crates_n: 2,
        }),
    )
    .unwrap();
    assert_eq!(counted.labels["changed_lines"], 17);
    assert_eq!(counted.labels["changed_files_n"], 4);
    assert_eq!(counted.labels["changed_crates_n"], 2);
    assert!(
        !counted.labels["missing"]
            .as_array()
            .unwrap()
            .contains(&Value::String("changed_lines".into()))
    );
}

#[test]
fn route_outcome_changed_counts_from_write_set_base_head() {
    // task-ops supplies the base..head numstat result; task-core only
    // accepts the values and removes the corresponding missing labels.
    let s = fixture();
    let outcome = outcome_labels_for_run(
        &s,
        "r1",
        Some(ChangeCount {
            changed_lines: 17,
            changed_files_n: 4,
            changed_crates_n: 2,
        }),
    )
    .unwrap();
    assert_eq!(outcome.labels["changed_lines"], 17);
    assert_eq!(outcome.labels["changed_files_n"], 4);
    assert_eq!(outcome.labels["changed_crates_n"], 2);
    let missing = outcome.labels["missing"].as_array().unwrap();
    for field in ["changed_lines", "changed_files_n", "changed_crates_n"] {
        assert!(!missing.contains(&Value::String(field.into())));
    }
}

#[test]
fn route_outcome_retries_after_and_repaired_task_and_wu() {
    let s = fixture();
    {
        let c = s.lock().unwrap();
        c.execute("INSERT INTO runs(run_id,task_id,role,seq,status,started_at) VALUES('task-base','task-1','worker',5,'completed','2026-01-01T00:01:30Z')",[]).unwrap();
    }
    let task_base = outcome_labels_for_run(&s, "task-base", None).unwrap();
    assert_eq!(task_base.labels["retries_after"], 0);
    assert_eq!(task_base.labels["repaired"], false);
    let task = outcome_labels_for_run(&s, "r1", None).unwrap();
    assert_eq!(task.labels["retries_after"], 1);
    assert_eq!(task.labels["repaired"], false);
    {
        let c = s.lock().unwrap();
        c.execute("INSERT INTO work_units(id,task_id,plan_id,key,seq,kind,status,depends_on_json,retries,json,created_at,updated_at) VALUES('repair-later','task-1','p1','repair-later',3,'repair','ready','[]',0,'{}','2026-01-01T00:04:00Z','2026-01-01T00:04:00Z')",[]).unwrap();
        c.execute("INSERT INTO runs(run_id,task_id,role,seq,status,started_at) VALUES('task-later','task-1','worker',3,'completed','2026-01-01T00:03:00Z')",[]).unwrap();
        c.execute("INSERT INTO runs(run_id,task_id,work_unit_id,role,seq,status,started_at) VALUES('wu-later','task-1','wu-1','worker',4,'completed','2026-01-01T00:04:00Z')",[]).unwrap();
    }
    let task = outcome_labels_for_run(&s, "r1", None).unwrap();
    assert_eq!(task.labels["retries_after"], 2);
    assert_eq!(task.labels["repaired"], true);
    let task_after = outcome_labels_for_run(&s, "task-base", None).unwrap();
    assert_eq!(task_after.labels["retries_after"], 1);
    assert_eq!(task_after.labels["repaired"], true);
    let wu = outcome_labels_for_run(&s, "r2", None).unwrap();
    assert_eq!(wu.labels["retries_after"], 1);
    assert_eq!(wu.labels["repaired"], true);
}

#[test]
fn route_outcome_quota_used_from_quota_estimated() {
    let s = fixture();
    {
        let c = s.lock().unwrap();
        c.execute("INSERT INTO events(task_id,seq,ts,json) VALUES('task-1',6,'2026-01-01T00:06:00Z',?1)", [r#"{"type":"quota_estimated","run_id":"r1","windows":[{"window":"five_hour","used_pct":12.5},{"window":"seven_day","used_pct":null}]}"#]).unwrap();
    }
    let labels = outcome_labels_for_run(&s, "r1", None).unwrap().labels;
    assert_eq!(labels["quota_used"], serde_json::json!({"five_hour":12.5}));
    assert!(
        !labels["missing"]
            .as_array()
            .unwrap()
            .contains(&Value::String("quota_used".into()))
    );
    let absent = outcome_labels_for_run(&s, "r2", None).unwrap().labels;
    assert!(absent["quota_used"].is_null());
    assert!(
        absent["missing"]
            .as_array()
            .unwrap()
            .contains(&Value::String("quota_used".into()))
    );
}

#[test]
fn route_extract_task_uses_task_json_dependencies_and_populates_task_fields() {
    let s = fixture();
    let features = features_for_subject(
        &s,
        Subject {
            kind: "task".into(),
            id: "task-1".into(),
        },
        None,
        None,
    )
    .unwrap();
    assert_eq!(features.aux.deps_n, Some(2));
    assert!(!features.aux.missing.iter().any(|name| name == "deps_n"));
    assert_eq!(features.aux.kind.as_deref(), Some("task"));
    assert!(features.brief.contains("- project: Routing project"));
    assert_eq!(features.aux.acceptance_n, Some(4));
    assert_eq!(features.aux.repos_n, Some(1));
    assert_eq!(features.aux.budget_turns, Some(22));
    assert_eq!(features.aux.budget_wall_s, Some(600));
    assert_eq!(features.aux.prior_runs_n, Some(2));
    assert_eq!(features.aux.retries_n, Some(1));
    assert_eq!(features.aux.repairs_n, Some(1));
    assert_eq!(features.aux.check_command_n, Some(1));
    assert_eq!(features.aux.check_artifact_n, Some(1));
    assert_eq!(features.aux.check_reviewer_n, Some(1));
    assert_eq!(features.aux.check_human_n, Some(1));
    assert!(!features.aux.missing.iter().any(|name| {
        [
            "genre",
            "kind",
            "acceptance_n",
            "repos_n",
            "budget_turns",
            "budget_wall_s",
        ]
        .contains(&name.as_str())
    }));
}

#[test]
fn route_extract_task_empty_dependencies_are_zero() {
    let s = fixture();
    s.lock()
        .unwrap()
        .execute(
            "UPDATE tasks SET json=json_set(json, '$.depends_on', json('[]')) WHERE id='task-1'",
            [],
        )
        .unwrap();
    let features = features_for_subject(
        &s,
        Subject {
            kind: "task".into(),
            id: "task-1".into(),
        },
        None,
        None,
    )
    .unwrap();
    assert_eq!(features.aux.deps_n, Some(0));
    assert!(!features.aux.missing.iter().any(|name| name == "deps_n"));
}

#[test]
fn route_extract_task_retry_repair_and_budget_fields_use_task_fixture() {
    let s = fixture();
    let features = features_for_subject(
        &s,
        Subject {
            kind: "task".into(),
            id: "task-1".into(),
        },
        Some(AsOf::Timestamp("2026-01-01T00:04:30Z".into())),
        None,
    )
    .unwrap();
    assert_eq!(features.aux.prior_runs_n, Some(2));
    assert_eq!(features.aux.retries_n, Some(1));
    assert_eq!(features.aux.repairs_n, Some(1));
    assert_eq!(features.aux.budget_turns, Some(22));
    assert_eq!(features.aux.budget_wall_s, Some(600));
}

#[test]
fn route_extract_work_unit_retry_repair_and_budget_fields_use_work_unit_fixture() {
    let s = fixture();
    let features = features_for_subject(
        &s,
        Subject {
            kind: "work_unit".into(),
            id: "wu-1".into(),
        },
        Some(AsOf::Timestamp("2026-01-01T00:04:30Z".into())),
        None,
    )
    .unwrap();
    assert_eq!(features.aux.prior_runs_n, Some(2));
    assert_eq!(features.aux.retries_n, Some(2));
    assert_eq!(features.aux.repairs_n, Some(1));
    assert_eq!(features.aux.budget_turns, Some(30));
    assert_eq!(features.aux.budget_wall_s, Some(900));
    assert!(features.brief.contains("- project: Routing project"));
}

#[test]
fn route_extract_work_unit_dependencies_still_use_work_unit_column() {
    let s = fixture();
    s.lock().unwrap().execute(
        "UPDATE work_units SET depends_on_json='[\"wu-parent-1\",\"wu-parent-2\",\"wu-parent-3\"]' WHERE id='wu-1'", [],
    ).unwrap();
    let features = features_for_subject(
        &s,
        Subject {
            kind: "work_unit".into(),
            id: "wu-1".into(),
        },
        None,
        None,
    )
    .unwrap();
    assert_eq!(features.aux.deps_n, Some(3));
    assert!(!features.aux.missing.iter().any(|name| name == "deps_n"));
}

/// 実データ形（migration 済み store、`record_write_set` と同じ列）の書き込み集合を足す。
/// r1（00:01 開始）の complete、r2（00:03 開始）の incomplete、wu-1 の WU 単位の complete。
fn write_set_fixture() -> SqliteStore {
    let s = fixture();
    let c = s.lock().unwrap();
    c.execute("INSERT INTO run_write_sets(run_id,task_id,work_unit_id,repo_id,base_sha,head_sha,paths_json,status,reason,recorded_at) VALUES('r1','task-1','wu-1','repo','b1','h1',?1,'complete',NULL,'2026-01-01T00:02:00Z')",[r#"["./crates/task-core/src/route_extract.rs","crates/task-core/src/lib.rs","crates/task-core/src/lib.rs","../escape","crates/task-core/"]"#]).unwrap();
    c.execute("INSERT INTO run_write_sets(run_id,task_id,work_unit_id,repo_id,base_sha,head_sha,paths_json,status,reason,recorded_at) VALUES('r2','task-1','wu-1','repo','b2',NULL,?1,'incomplete','git diff failed','2026-01-01T00:03:30Z')",[r#"["crates/future/src/late.rs"]"#]).unwrap();
    c.execute("INSERT INTO work_unit_write_sets(work_unit_id,task_id,repo_id,base_sha,head_sha,paths_json,status,reason,recorded_at) VALUES('wu-1','task-1','repo','b1','h1',?1,'complete',NULL,'2026-01-01T00:02:10Z')",[r#"["notes/wu.md","crates/task-core/src/lib.rs"]"#]).unwrap();
    drop(c);
    s
}

fn subject(kind: &str, id: &str) -> Subject {
    Subject {
        kind: kind.into(),
        id: id.into(),
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn route_extract_write_set_work_unit_prior_attempt_in_brief_and_aux() {
    let s = write_set_fixture();
    let f = features_for_subject(&s, subject("work_unit", "wu-1"), None, None).unwrap();
    assert_eq!(
        f.aux.write_set,
        Some(strings(&[
            "repo:crates/future/src/late.rs",
            "repo:crates/task-core",
            "repo:crates/task-core/src/lib.rs",
            "repo:crates/task-core/src/route_extract.rs",
            "repo:notes/wu.md",
        ]))
    );
    assert_eq!(
        f.aux.write_set_gaps,
        vec![WriteSetGap {
            source: "run_write_sets".into(),
            owner_id: "r2".into(),
            repo: "repo".into(),
            status: "incomplete".into(),
        }]
    );
    assert!(!f.aux.missing.iter().any(|m| m == "write_set"));
    assert!(f.brief.contains(
        "書き込み集合:\n- repo:crates/future/src/late.rs\n- repo:crates/task-core\n- repo:crates/task-core/src/lib.rs\n- repo:crates/task-core/src/route_extract.rs\n- repo:notes/wu.md\n\n# 置かれた位置"
    ));
    let again = features_for_subject(&s, subject("work_unit", "wu-1"), None, None).unwrap();
    assert_eq!(f.hash, again.hash);
}

#[test]
fn route_extract_write_set_task_prior_attempt_in_brief_and_aux() {
    let s = write_set_fixture();
    let f = features_for_subject(&s, subject("task", "task-1"), None, None).unwrap();
    // task subject は run_write_sets（task_id 一致）だけを読み、WU 単位の集合は読まない。
    assert_eq!(
        f.aux.write_set,
        Some(strings(&[
            "repo:crates/future/src/late.rs",
            "repo:crates/task-core",
            "repo:crates/task-core/src/lib.rs",
            "repo:crates/task-core/src/route_extract.rs",
        ]))
    );
    assert_eq!(f.aux.write_set_gaps.len(), 1);
    assert!(!f.aux.missing.iter().any(|m| m == "write_set"));
    assert!(
        f.brief
            .contains("書き込み集合:\n- repo:crates/future/src/late.rs\n")
    );
    assert!(!f.brief.contains("notes/wu.md"));
}

fn insert_fresh_subjects(s: &SqliteStore) {
    let c = s.lock().unwrap();
    c.execute("INSERT INTO tasks(id,status,kind,parent_id,priority,created_at,json) VALUES('task-2','running','task',NULL,0,'2026-01-01T00:00:00Z','{\"title\":\"Fresh\",\"objective\":\"New\"}')",[]).unwrap();
    c.execute("INSERT INTO work_units(id,task_id,plan_id,key,seq,kind,status,depends_on_json,retries,json,created_at,updated_at) VALUES('wu-2','task-1','p1','fresh',3,'implement','ready','[]',0,'{\"objective\":\"New\"}','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",[]).unwrap();
}

#[test]
fn route_extract_write_set_work_unit_without_prior_attempt_is_missing() {
    let s = write_set_fixture();
    insert_fresh_subjects(&s);
    let f = features_for_subject(&s, subject("work_unit", "wu-2"), None, None).unwrap();
    assert_eq!(f.aux.prior_runs_n, Some(0));
    assert_eq!(f.aux.write_set, None);
    assert!(f.aux.write_set_gaps.is_empty());
    assert!(f.aux.missing.iter().any(|m| m == "write_set"));
    assert!(f.brief.contains("書き込み集合:\n範囲の記述なし\n"));
}

#[test]
fn route_extract_write_set_task_without_prior_attempt_is_missing() {
    let s = write_set_fixture();
    insert_fresh_subjects(&s);
    let f = features_for_subject(&s, subject("task", "task-2"), None, None).unwrap();
    assert_eq!(f.aux.prior_runs_n, Some(0));
    assert_eq!(f.aux.write_set, None);
    assert!(f.aux.missing.iter().any(|m| m == "write_set"));
    assert!(f.brief.contains("書き込み集合:\n範囲の記述なし\n"));
}

#[test]
fn route_extract_write_set_excludes_sets_recorded_after_decision_time() {
    let s = write_set_fixture();
    let at = |kind: &str, id: &str, as_of: AsOf| {
        features_for_subject(&s, subject(kind, id), Some(as_of), None)
            .unwrap()
            .aux
    };
    let r1 = [
        "repo:crates/task-core",
        "repo:crates/task-core/src/lib.rs",
        "repo:crates/task-core/src/route_extract.rs",
    ];
    // r1 の集合は 00:02:00 記録。それより前の判断には何も入らない。
    let early = at(
        "work_unit",
        "wu-1",
        AsOf::Timestamp("2026-01-01T00:01:30Z".into()),
    );
    assert_eq!(early.write_set, None);
    assert!(early.missing.iter().any(|m| m == "write_set"));
    // 00:02:05: r1 だけ（WU 集合 00:02:10・r2 00:03:30 は後）。
    let mid = at(
        "work_unit",
        "wu-1",
        AsOf::Timestamp("2026-01-01T00:02:05Z".into()),
    );
    assert_eq!(mid.write_set, Some(strings(&r1)));
    assert!(mid.write_set_gaps.is_empty());
    let task_mid = at(
        "task",
        "task-1",
        AsOf::Timestamp("2026-01-01T00:03:10Z".into()),
    );
    assert_eq!(task_mid.write_set, Some(strings(&r1)));
    // EventSeq: seq 2 の ts は 00:01:00 で r1 の集合（00:02:00）は後。
    let seq2 = at("work_unit", "wu-1", AsOf::EventSeq(2));
    assert_eq!(seq2.write_set, None);
    // seq 3（ts 00:03:00）: r1 と WU 集合は入り、r2（00:03:30 記録）は入らない。
    let seq3 = at("work_unit", "wu-1", AsOf::EventSeq(3));
    let mut expected = r1.to_vec();
    expected.push("repo:notes/wu.md");
    assert_eq!(seq3.write_set, Some(strings(&expected)));
    assert!(seq3.write_set_gaps.is_empty());
    let task_seq3 = at("task", "task-1", AsOf::EventSeq(3));
    assert_eq!(task_seq3.write_set, Some(strings(&r1)));
    // seq 5（ts 00:05:00）: r2 の incomplete も入り gap に残る。
    let seq5 = at("work_unit", "wu-1", AsOf::EventSeq(5));
    assert!(
        seq5.write_set
            .unwrap()
            .contains(&"repo:crates/future/src/late.rs".to_owned())
    );
    assert_eq!(seq5.write_set_gaps.len(), 1);
}
