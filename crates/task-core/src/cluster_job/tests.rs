use super::*;

/// PBS Pro 2022 の `qstat -xf` の出力の形（sirius の BenchFS の job を模した。長い値の折り返しを含む）。
const QSTAT_XF_FIXTURE: &str = "Job Id: 42634.sirius-pbs
    Job_Name = benchfs-e1-v2
    Job_Owner = rmaeda@sirius-login1
    resources_used.cpupercent = 0
    resources_used.walltime = 00:00:00
    job_state = Q
    queue = regular
    server = sirius-pbs
    Checkpoint = u
    ctime = Wed Sep 30 05:02:11 2026
    Error_Path = sirius-login1:/work/NBB/rmaeda/workspace/benchfs/results/e1/benc
\thfs-e1-v2.e42634
    Resource_List.ncpus = 48
    Resource_List.nodect = 2
    Resource_List.walltime = 04:00:00
    substate = 10

Job Id: 42635.sirius-pbs
    Job_Name = benchfs-e1-v2-b
    job_state = R
    queue = regular
    exec_host = snode12/0*48+snode13/0*48
    resources_used.walltime = 01:12:44
    stime = Wed Sep 30 05:03:40 2026

Job Id: 42636.sirius-pbs
    Job_Name = benchfs-a0-v2
    job_state = F
    queue = regular
    Exit_status = 0
    resources_used.walltime = 02:01:13
    substate = 92

Job Id: 42637.sirius-pbs
    Job_Name = benchfs-a0-v2-b
    job_state = F
    Exit_status = 271
    substate = 91
";

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn pbs_qstat_xf_is_parsed_per_job() {
    let jobs = ids(&["42634", "42635", "42636", "42637", "42638"]);
    let got = parse_pbs_qstat_xf(
        QSTAT_XF_FIXTURE,
        "qstat: Unknown Job Id 42638.sirius-pbs\n",
        &jobs,
    );
    assert_eq!(got.len(), 5);
    assert_eq!(got[0].state, ClusterJobState::Queued);
    assert_eq!(got[0].raw_state.as_deref(), Some("Q"));
    assert_eq!(got[0].exit_status, None);
    assert_eq!(got[1].state, ClusterJobState::Running);
    assert_eq!(got[2].state, ClusterJobState::Finished);
    assert_eq!(got[2].exit_status, Some(0));
    assert_eq!(got[3].state, ClusterJobState::Finished);
    assert_eq!(got[3].exit_status, Some(271));
    assert_eq!(got[4].state, ClusterJobState::Gone);
    assert!(!all_finished(&jobs, &got));
    assert_eq!(
        status_line(&got),
        "42634 (Q) 42635 (R) 42636 (F, exit 0) 42637 (F, exit 271) 42638 (gone)"
    );
    let done = ids(&["42636", "42637", "42638"]);
    let got = parse_pbs_qstat_xf(
        QSTAT_XF_FIXTURE,
        "qstat: Unknown Job Id 42638.sirius-pbs\n",
        &done,
    );
    assert!(all_finished(&done, &got));
}

#[test]
fn pbs_job_missing_from_the_output_is_unknown_not_finished() {
    let jobs = ids(&["99999"]);
    let got = parse_pbs_qstat_xf("", "", &jobs);
    assert_eq!(got[0].state, ClusterJobState::Unknown);
    assert!(!all_finished(&jobs, &got));
    // server 名を付けて申告しても同じ job に当たる。
    let jobs = ids(&["42636.sirius-pbs"]);
    let got = parse_pbs_qstat_xf(QSTAT_XF_FIXTURE, "", &jobs);
    assert_eq!(got[0].state, ClusterJobState::Finished);
}

#[test]
fn slurm_sacct_is_parsed() {
    let out = "101|COMPLETED|0:0\n102|RUNNING|0:0\n103|FAILED|2:0\n104|CANCELLED by 1001|0:15\n105|PENDING|0:0\n";
    let jobs = ids(&["101", "102", "103", "104", "105", "106"]);
    let got = parse_slurm_sacct(out, &jobs);
    assert_eq!(got[0].state, ClusterJobState::Finished);
    assert_eq!(got[0].exit_status, Some(0));
    assert_eq!(got[1].state, ClusterJobState::Running);
    assert_eq!(got[2].exit_status, Some(2));
    assert_eq!(got[3].state, ClusterJobState::Finished);
    assert_eq!(got[4].state, ClusterJobState::Queued);
    assert_eq!(got[5].state, ClusterJobState::Unknown);
}

#[test]
fn poll_commands_only_carry_valid_ids() {
    assert_eq!(
        poll_command(ClusterScheduler::Pbs, &ids(&["42634", "42635"])),
        "qstat -xf 42634 42635"
    );
    assert_eq!(
        poll_command(ClusterScheduler::Slurm, &ids(&["1", "2"])),
        "sacct -n -P -X -o JobID,State,ExitCode -j 1,2"
    );
    assert_eq!(
        poll_command(ClusterScheduler::Pbs, &ids(&["1; rm -rf ~", "2"])),
        "qstat -xf 2"
    );
    assert!(!valid_job_id("$(id)"));
    assert!(valid_job_id("1234[]"));
}

#[test]
fn wait_requests_are_parsed_in_both_shapes() {
    let top = serde_json::json!({
        "type": "wait", "kind": "cluster_job", "cluster": "sirius",
        "jobs": ["42634", 42635], "scheduler": "pbs", "poll_secs": 300, "timeout_secs": 43200,
        "checkpoint": {"completed": ["submitted E1"], "next_action": "collect results"},
        "summary": "submitted 2 jobs"
    });
    let (req, cp) = parse_wait_request(&top).unwrap().unwrap();
    assert_eq!(req.cluster.as_deref(), Some("sirius"));
    assert_eq!(req.jobs, ids(&["42634", "42635"]));
    assert_eq!(req.scheduler, ClusterScheduler::Pbs);
    assert_eq!(req.poll_secs, Some(300));
    assert_eq!(req.timeout_secs, Some(43200));
    assert_eq!(req.summary, "submitted 2 jobs");
    assert_eq!(cp.unwrap()["next_action"], "collect results");

    let nested = serde_json::json!({
        "summary": "s", "wait": {"kind": "cluster_job", "jobs": ["7"]}
    });
    let (req, cp) = parse_wait_request(&nested).unwrap().unwrap();
    assert_eq!(req.scheduler, ClusterScheduler::Pbs);
    assert_eq!(req.summary, "s");
    assert!(cp.is_none());

    assert!(parse_wait_request(&serde_json::json!({"summary": "done"})).is_none());
    assert!(
        parse_wait_request(&serde_json::json!({"type": "wait", "jobs": []}))
            .unwrap()
            .is_err()
    );
    assert!(
        parse_wait_request(&serde_json::json!({"type": "wait", "jobs": ["1;ls"]}))
            .unwrap()
            .is_err()
    );
    assert!(
        parse_wait_request(&serde_json::json!({"type": "wait", "kind": "http", "jobs": ["1"]}))
            .unwrap()
            .is_err()
    );
    assert!(
        parse_wait_request(&serde_json::json!({"type": "wait", "jobs": ["1"], "scheduler": "lsf"}))
            .unwrap()
            .is_err()
    );
}

fn task(status: crate::Status) -> crate::Task {
    use crate::model::{Budget, Task, TaskKind, Tier, WorkerHint, WorkspaceSpec};
    let now = time::OffsetDateTime::now_utc();
    Task {
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "benchfs".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "ws".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 2,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
        tree: None,
        paused_at: None,
    }
}

fn wait_for(task_id: TaskId) -> ClusterJobWait {
    ClusterJobWait {
        wait_id: "w1".into(),
        task_id,
        work_unit_id: None,
        run_id: "run-1".into(),
        cluster: "sirius".into(),
        scheduler: ClusterScheduler::Pbs,
        jobs: ids(&["42634", "42635"]),
        poll_secs: 300,
        timeout_secs: 3600,
        summary: "submitted".into(),
        checkpoint: None,
        created_at: "2026-09-30T05:00:00Z".into(),
        deadline: "2026-09-30T06:00:00Z".into(),
        last_polled_at: None,
        finished_at: None,
        state: ClusterJobWaitState::Waiting,
        last_status: Vec::new(),
    }
}

/// `ClusterJobWaitStarted` / `Polled` / `Finished` の追記が同じトランザクションで表を書き、終端への遷移が
/// `waiting` の wait を `cancelled` に閉じ（`ClusterJobWaitFinished{cancelled}` を残す）、`waiting` の間は
/// 一般の回答で `ready` に戻せない。
#[test]
fn events_project_into_the_table_and_terminal_transitions_cancel() {
    use crate::{Status, TaskStore, Trigger};
    let store = SqliteStore::open_in_memory().expect("open");
    let t = task(Status::Ready);
    store.insert(&t).expect("insert");
    assert!(
        store
            .acquire_lease(t.id, "run-1", std::time::Duration::from_secs(60))
            .expect("lease")
    );
    let outcome = store
        .apply_transition_with_events(
            t.id,
            Trigger::ClusterJobWait,
            vec![Event::ClusterJobWaitStarted {
                wait: Box::new(wait_for(t.id)),
            }],
        )
        .expect("park");
    assert_eq!(outcome.next, Status::Blocked);
    assert_eq!(outcome.reason, REASON_WAITING);
    let waits = store.cluster_job_waits_waiting().expect("waiting");
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].jobs, ids(&["42634", "42635"]));
    assert!(store.get(t.id).unwrap().unwrap().lease.is_none());

    // 一般の回答では戻せない（wait が waiting の間）。
    assert!(
        store
            .apply_transition_with_events(t.id, Trigger::Answer, vec![])
            .is_err()
    );

    let polled = vec![
        ClusterJobStatus {
            job_id: "42634".into(),
            state: ClusterJobState::Running,
            exit_status: None,
            raw_state: Some("R".into()),
        },
        ClusterJobStatus::unknown("42635"),
    ];
    store
        .append_event(
            t.id,
            &Event::ClusterJobWaitPolled {
                wait_id: "w1".into(),
                jobs: polled.clone(),
            },
        )
        .expect("polled");
    let w = store.cluster_job_wait_get("w1").unwrap().unwrap();
    assert_eq!(w.last_status, polled);
    assert!(w.last_polled_at.is_some());
    store
        .cluster_job_wait_touch("w1", "2026-09-30T05:10:00Z", &polled)
        .expect("touch");
    assert_eq!(
        store
            .cluster_job_wait_get("w1")
            .unwrap()
            .unwrap()
            .last_polled_at
            .as_deref(),
        Some("2026-09-30T05:10:00Z")
    );

    // 中止 → wait は cancelled（event 付き）。
    store
        .apply_transition_with_events(t.id, Trigger::Cancel, vec![])
        .expect("cancel");
    let w = store.cluster_job_wait_get("w1").unwrap().unwrap();
    assert_eq!(w.state, ClusterJobWaitState::Cancelled);
    assert!(store.cluster_job_waits_waiting().unwrap().is_empty());
    let events = store.events_for(t.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::ClusterJobWaitFinished {
            state: ClusterJobWaitState::Cancelled,
            ..
        }
    )));
}

/// satisfied の `ClusterJobWaitFinished` と `ClusterJobResume` を同じトランザクションで書くと、task は
/// `ready` に戻り、表の行は `satisfied` になる。
#[test]
fn a_satisfied_wait_resumes_the_task() {
    use crate::{Status, TaskStore, Trigger};
    let store = SqliteStore::open_in_memory().expect("open");
    let t = task(Status::Ready);
    store.insert(&t).expect("insert");
    store
        .acquire_lease(t.id, "run-1", std::time::Duration::from_secs(60))
        .expect("lease");
    store
        .apply_transition_with_events(
            t.id,
            Trigger::ClusterJobWait,
            vec![Event::ClusterJobWaitStarted {
                wait: Box::new(wait_for(t.id)),
            }],
        )
        .expect("park");
    let done = parse_pbs_qstat_xf(QSTAT_XF_FIXTURE, "", &ids(&["42636", "42637"]));
    let outcome = store
        .apply_transition_with_events(
            t.id,
            Trigger::ClusterJobResume,
            vec![Event::ClusterJobWaitFinished {
                wait_id: "w1".into(),
                state: ClusterJobWaitState::Satisfied,
                jobs: done.clone(),
                detail: String::new(),
            }],
        )
        .expect("resume");
    assert_eq!(outcome.next, Status::Ready);
    assert_eq!(outcome.reason, REASON_RESUME);
    let w = store.cluster_job_wait_get("w1").unwrap().unwrap();
    assert_eq!(w.state, ClusterJobWaitState::Satisfied);
    assert_eq!(w.last_status, done);
    assert!(w.finished_at.is_some());
}

/// ADR-0090 D2: migration 0034 は schema 33 の DB に `cluster_job_waits` を足し（既存の表・行には触れない）、
/// 版数は 34 になる（その後の browser の 0035/0036 も続けて当たり、最新版になる）。
#[test]
fn migration_0034_adds_cluster_job_waits_to_a_schema_33_db() {
    use crate::TaskStore;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.db");
    let t = task(crate::Status::Ready);
    {
        let store = SqliteStore::open(&path).unwrap();
        store.insert(&t).unwrap();
    }
    {
        // schema 33 の DB に戻す（34 以降の表・列を落とし、版数 34 以降の記録を消す）。
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE cluster_job_waits; \
             DROP TABLE browser_live_events; DROP TABLE browser_control_state; \
             DROP TABLE browser_control_actions; DROP TABLE browser_identities; \
             ALTER TABLE browser_waits DROP COLUMN trusted_login_json; \
             DELETE FROM schema_migrations WHERE version >= 34;",
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::SCHEMA_VERSION);
    assert_eq!(crate::SCHEMA_VERSION, 36);
    assert!(store.cluster_job_waits_waiting().unwrap().is_empty());
    assert!(store.get(t.id).unwrap().is_some());
}

#[test]
fn limits_clamp_and_validate() {
    let limits = ClusterJobWaitLimits {
        poll_secs: 300,
        max_wait_secs: 86_400,
    };
    assert!(limits.validate().is_ok());
    let req = |poll: Option<u64>, timeout: Option<u64>| ClusterJobWaitRequest {
        cluster: None,
        scheduler: ClusterScheduler::Pbs,
        jobs: ids(&["1"]),
        poll_secs: poll,
        timeout_secs: timeout,
        summary: String::new(),
    };
    assert_eq!(limits.clamp(&req(None, None)), (300, 86_400));
    assert_eq!(limits.clamp(&req(Some(10), Some(10))), (300, 300));
    assert_eq!(limits.clamp(&req(Some(900), Some(999_999))), (900, 86_400));
    assert!(
        ClusterJobWaitLimits {
            poll_secs: 5,
            max_wait_secs: 100
        }
        .validate()
        .is_err()
    );
    assert!(
        ClusterJobWaitLimits {
            poll_secs: 300,
            max_wait_secs: 60
        }
        .validate()
        .is_err()
    );
    assert!(
        ClusterJobWaitLimits {
            poll_secs: 300,
            max_wait_secs: MAX_WAIT_SECS_CEILING + 1
        }
        .validate()
        .is_err()
    );
}
