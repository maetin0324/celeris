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
        "sacct -n -P -X -o JobID,State%64,ExitCode -j 1,2"
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
        "jobs": ["42634", 42635], "poll_secs": 300, "timeout_secs": 43200,
        "checkpoint": {"completed": ["submitted E1"], "next_action": "collect results"},
        "summary": "submitted 2 jobs"
    });
    let (req, cp) = parse_wait_request(&top).unwrap().unwrap();
    assert_eq!(req.cluster.as_deref(), Some("sirius"));
    assert_eq!(req.jobs, ids(&["42634", "42635"]));
    assert_eq!(req.poll_secs, Some(300));
    assert_eq!(req.timeout_secs, Some(43200));
    assert_eq!(req.summary, "submitted 2 jobs");
    assert_eq!(cp.unwrap()["next_action"], "collect results");

    let nested = serde_json::json!({
        "summary": "s", "wait": {"kind": "cluster_job", "jobs": ["7"]}
    });
    let (req, cp) = parse_wait_request(&nested).unwrap().unwrap();
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
}

/// ADR 2026-11-25-cluster-scheduler-from-config D2: 申告の `scheduler` の欄は消した。値によらず（正しい値でも、
/// 2026-11-25 の事象の `pbs` でも、未知の値でも、入れ子でも）不正な wait になり、理由の文は設定の欄を指す。
/// 旧い試験は `scheduler` の値を読んでいたので、欄を受け付けて黙って使う・無視する形を検出できない。
#[test]
fn wait_request_naming_a_scheduler_is_rejected() {
    for spec in [
        serde_json::json!({"type": "wait", "jobs": ["318985.nqsv"], "scheduler": "pbs"}),
        serde_json::json!({"type": "wait", "jobs": ["315477.nqsv"], "scheduler": "NQSV"}),
        serde_json::json!({"type": "wait", "jobs": ["1"], "scheduler": "lsf"}),
        serde_json::json!({"summary": "s", "wait": {"jobs": ["1"], "scheduler": "slurm"}}),
    ] {
        let err = parse_wait_request(&spec).unwrap().unwrap_err();
        assert!(err.contains("[[clusters]] scheduler"), "{err}");
    }
}

/// ADR 2026-11-25-cluster-scheduler-from-config D4: 失敗の poll の記録と起床の条件。最初の失敗では起きず、
/// 連続の最初の時刻から `threshold` 秒の失敗で起きる。前の観測（Q と queued_since）は残す。
#[test]
fn poll_failures_wake_only_after_the_threshold_from_the_first_failure() {
    let t0 = time::OffsetDateTime::parse(
        "2026-11-25T16:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let mut w = wait_with(&["318985", "318986"], Vec::new());
    w.scheduler = ClusterScheduler::Pbs;
    let (jobs, wake) =
        wakeup::observe_poll_failure(&w, "exit 127: bash: qstat: command not found", t0, 1800);
    assert_eq!(wake, None, "the first failure never wakes");
    assert!(jobs.iter().all(|j| j.state == ClusterJobState::Unknown));
    let since = jobs[0].poll_failure.clone().unwrap();
    assert_eq!(since.since, "2026-11-25T16:00:00Z");
    assert!(since.error.contains("exit 127"));
    w.last_status = jobs;
    let (jobs, wake) =
        wakeup::observe_poll_failure(&w, "exit 127", t0 + time::Duration::seconds(1500), 1800);
    assert_eq!(wake, None);
    assert_eq!(
        jobs[1].poll_failure.as_ref().unwrap().since,
        "2026-11-25T16:00:00Z"
    );
    w.last_status = jobs;
    let (jobs, wake) =
        wakeup::observe_poll_failure(&w, "exit 127", t0 + time::Duration::seconds(1800), 1800);
    assert_eq!(wake, Some(WAKE_POLL_FAILED));
    assert!(is_wake_reason(WAKE_POLL_FAILED));
    assert!(jobs.iter().all(|j| j.poll_failure.is_some()));

    // 前の観測（Q・queued_since）は失敗の poll で消えない。
    let mut q = ClusterJobStatus::unknown("318985");
    q.state = ClusterJobState::Queued;
    q.queued_since = Some("2026-11-25T15:00:00Z".into());
    let w = wait_with(&["318985"], vec![q]);
    let (jobs, _) = wakeup::observe_poll_failure(&w, "exit 2: usage", t0, 1800);
    assert_eq!(jobs[0].state, ClusterJobState::Queued);
    assert_eq!(
        jobs[0].queued_since.as_deref(),
        Some("2026-11-25T15:00:00Z")
    );
    // 状態の JSON は読み書きできる（`last_status_json` と event に残る）。
    let back: Vec<ClusterJobStatus> =
        serde_json::from_str(&serde_json::to_string(&jobs).unwrap()).unwrap();
    assert_eq!(back, jobs);
}

fn task(status: crate::Status) -> crate::Task {
    use crate::model::{Budget, Task, TaskKind, Tier, WorkerHint, WorkspaceSpec};
    let now = time::OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
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
        },
        workspace: WorkspaceSpec::Local {
            path: "ws".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 2,
            max_tokens: None,
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

fn wait_with(jobs: &[&str], last_status: Vec<ClusterJobStatus>) -> ClusterJobWait {
    let mut w = wait_for(TaskId::new());
    w.jobs = ids(jobs);
    w.last_status = last_status;
    w
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
    use crate::{SqliteStore, Status, Trigger};
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
            scheduling: Vec::new(),
            queued_since: None,
            queue_notified: false,
            poll_failure: None,
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
    use crate::{SqliteStore, Status, Trigger};
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
/// 版数は 34 になる（その後の browser の 0035/0036・delivery_skipped index の 0037・通知の 0041・cron の 0046 も続けて当たり、最新版になる）。
#[test]
fn migration_0034_adds_cluster_job_waits_to_a_schema_33_db() {
    use crate::SqliteStore;
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
            "DROP TRIGGER IF EXISTS task_roadmap_specs_capture; DROP TABLE IF EXISTS task_roadmap_specs; \
             DROP TRIGGER IF EXISTS script_compare_log_keep_max; DROP TRIGGER IF EXISTS project_wakes_keep_max; \
             DROP TRIGGER IF EXISTS project_controller_decisions_keep_max; \
             DROP TABLE task_integrations; \
             ALTER TABLE task_change_integrations RENAME TO task_integrations; \
             DROP TABLE account_limits; \
             DROP INDEX idx_eval_trials_status; DROP INDEX idx_eval_trials_batch; DROP TABLE eval_trials; \
             DROP TRIGGER chat_node_session_thread_insert; \
             DROP TRIGGER chat_node_session_thread_update; \
             DROP INDEX idx_node_sessions_cos_chat_active; \
             ALTER TABLE node_sessions DROP COLUMN thread_id; \
             ALTER TABLE node_sessions DROP COLUMN llm_source; \
             ALTER TABLE node_sessions DROP COLUMN model; \
             ALTER TABLE node_sessions DROP COLUMN summary_through_seq; \
             DROP TABLE chat_search; DROP TABLE chat_upload_reservations; \
             DROP TABLE chat_client_requests; DROP TABLE cos_legacy_notification_links; DROP TABLE cos_notification_routes; \
             DROP TABLE cos_run_credentials; DROP TABLE cos_operations; DROP TABLE cos_inbox_items; \
             DROP TABLE chat_attachment_refs; DROP TABLE chat_attachments; \
             DROP TABLE chat_events; DROP TABLE chat_runs; \
             DROP TABLE chat_messages; DROP TABLE chat_threads; \
             DROP TABLE task_behind_targets; \
             DROP TABLE routing_stage; DROP TABLE routing_evidence; \
             DROP TABLE run_write_sets; DROP TABLE work_unit_write_sets; \
             DROP TABLE task_write_hints; \
             DROP TABLE cluster_job_waits; \
             DROP TABLE browser_live_events; DROP TABLE browser_control_state; \
             DROP TABLE browser_control_actions; DROP TABLE browser_identities; \
             ALTER TABLE browser_waits DROP COLUMN trusted_login_json; \
             ALTER TABLE deliveries DROP COLUMN target_sha; \
             ALTER TABLE deliveries DROP COLUMN reviewed_sha; \
             ALTER TABLE deliveries DROP COLUMN merge_candidate_sha; \
             DROP INDEX idx_node_sessions_work_unit_active; \
             ALTER TABLE browser_site_policies DROP COLUMN consent_json; \
             ALTER TABLE browser_site_policies DROP COLUMN post_login_json; \
             ALTER TABLE browser_site_policies DROP COLUMN username_selector; \
             ALTER TABLE node_sessions DROP COLUMN delivered_through_seq; \
             ALTER TABLE node_sessions DROP COLUMN last_context_tokens; \
             ALTER TABLE node_sessions DROP COLUMN billed_input_tokens; \
             ALTER TABLE node_sessions DROP COLUMN task_id; \
             ALTER TABLE node_sessions DROP COLUMN work_unit_id; \
             ALTER TABLE node_sessions DROP COLUMN provider; \
             ALTER TABLE node_sessions DROP COLUMN cwd; \
             DROP INDEX idx_events_delivery_skipped; \
             DROP TABLE cron_job_runs; DROP TABLE cron_jobs; \
             DROP INDEX idx_events_integration_request; \
             DROP INDEX idx_events_routing_decided; DROP INDEX idx_llm_proxy_requests_decision; \
             ALTER TABLE llm_proxy_requests DROP COLUMN model; ALTER TABLE llm_proxy_requests DROP COLUMN source_id; \
             ALTER TABLE llm_proxy_requests DROP COLUMN task_id; ALTER TABLE llm_proxy_requests DROP COLUMN run_id; \
             ALTER TABLE llm_proxy_requests DROP COLUMN snapshot_id; ALTER TABLE llm_proxy_requests DROP COLUMN decision_id; \
             ALTER TABLE project_repos DROP COLUMN auto_integrate; \
             DROP TABLE task_campaigns; \
             ALTER TABLE decision_log_runs DROP COLUMN variant_id; ALTER TABLE decision_log_runs DROP COLUMN variant_content_sha256; \
             DROP TABLE decision_log_campaign_activity; ALTER TABLE decision_log_usage DROP COLUMN attribution_rule; ALTER TABLE decision_log_usage DROP COLUMN shared_overhead; \
             ALTER TABLE decision_log_runs DROP COLUMN case_id; ALTER TABLE decision_log_runs DROP COLUMN repetition; \
             ALTER TABLE decision_log_runs DROP COLUMN manifest_hash; ALTER TABLE decision_log_runs DROP COLUMN routing_setting; \
             ALTER TABLE decision_log_runs DROP COLUMN classifier_status; ALTER TABLE decision_log_runs DROP COLUMN classifier_call_id; \
             ALTER TABLE decision_log_runs DROP COLUMN fixed_lane; ALTER TABLE decision_log_runs DROP COLUMN jev_lane; \
             ALTER TABLE decision_log_runs DROP COLUMN constrained_lane; ALTER TABLE decision_log_runs DROP COLUMN lane_basis; \
             DROP TABLE task_routing_overrides; \
             ALTER TABLE tasks DROP COLUMN harness_id; \
             ALTER TABLE runs DROP COLUMN selected_harness_id; \
             ALTER TABLE runs DROP COLUMN selected_source_id; \
             ALTER TABLE runs DROP COLUMN selected_model_id; \
             ALTER TABLE runs DROP COLUMN selected_account_id; \
             ALTER TABLE runs DROP COLUMN selection_metadata_json; \
             DROP TABLE route_outcomes; DROP TABLE route_judgments; DROP TABLE route_decisions; DROP TABLE route_options; DROP TABLE route_features; \
             DROP INDEX idx_completion_outcomes_leaf; DROP TABLE completion_outcomes; DROP TABLE harness_balance_trips; DROP TABLE config_events; \
             ALTER TABLE work_units DROP COLUMN rebase_pending; \
             ALTER TABLE mcp_calls DROP COLUMN request; \
             DROP TABLE task_idempotency_keys; DROP TABLE task_unblocks; \
             DROP TABLE project_briefs; ALTER TABLE projects ADD COLUMN secretary_summary TEXT; \
             DROP TABLE terminal_session_events; ALTER TABLE node_sessions DROP COLUMN policy_key; \
             DROP TABLE health_inventory_runs; \
             DELETE FROM schema_migrations WHERE version >= 34;",
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::SCHEMA_VERSION);
    assert_eq!(crate::SCHEMA_VERSION, 127);
    assert!(store.cluster_job_waits_waiting().unwrap().is_empty());
    assert!(store.get(t.id).unwrap().is_some());
}

#[test]
fn limits_clamp_and_validate() {
    let limits = ClusterJobWaitLimits::new(300, 86_400);
    assert!(limits.validate().is_ok());
    let req = |poll: Option<u64>, timeout: Option<u64>| ClusterJobWaitRequest {
        cluster: None,
        jobs: ids(&["1"]),
        poll_secs: poll,
        timeout_secs: timeout,
        summary: String::new(),
    };
    assert_eq!(limits.clamp(&req(None, None)), (300, 86_400));
    assert_eq!(limits.clamp(&req(Some(10), Some(10))), (300, 300));
    assert_eq!(limits.clamp(&req(Some(900), Some(999_999))), (900, 86_400));
    assert!(ClusterJobWaitLimits::new(5, 100).validate().is_err());
    assert!(ClusterJobWaitLimits::new(300, 60).validate().is_err());
    let mut zero = ClusterJobWaitLimits::new(300, 86_400);
    zero.poll_failure_wake_secs = 0;
    assert!(
        zero.validate()
            .unwrap_err()
            .contains("poll_failure_wake_secs")
    );
    assert_eq!(limits.poll_failure_wake_secs, 1800);
    assert!(
        ClusterJobWaitLimits::new(300, MAX_WAIT_SECS_CEILING + 1)
            .validate()
            .is_err()
    );
}

/// A scheduler failure can have exit 0, and a signal is distinct from success.
#[test]
fn slurm_failure_states_and_signals_are_not_success() {
    let jobs = ids(&["1", "2", "3", "4", "5", "6", "7"]);
    let statuses = parse_slurm_sacct(
        "1|FAILED|0:0\n2|NODE_FAIL|0:0\n3|TIMEOUT|0:0\n4|OUT_OF_MEMORY|0:0\n5|CANCELLED by 123|0:0\n6|COMPLETED|0:9\n7|RUNNING|0:0\n",
        &jobs,
    );
    let mut wait = wait_for(task(crate::Status::Running).id);
    wait.jobs = jobs;
    let now = time::OffsetDateTime::now_utc();
    for failed in &statuses[..6] {
        let mut jobs = vec![failed.clone(), statuses[6].clone()];
        assert_eq!(
            wakeup::observe(&wait, &[], &mut jobs, now, 60),
            Some("job_failed")
        );
    }
    assert_eq!(statuses[5].exit_status, Some(-9));
}

/// Observed running/unknown breaks the queue clock; durable receipts are scoped to the owner.
#[test]
fn queue_clock_resets_and_receipts_survive_serialization() {
    let now = time::OffsetDateTime::now_utc();
    let mut wait = wait_for(task(crate::Status::Running).id);
    wait.jobs = ids(&["1"]);
    let observe = |wait: &ClusterJobWait, state: &str, at: i64| {
        let mut statuses = parse_pbs_qstat_xf(
            &format!("Job Id: 1\n job_state = {state}\n"),
            "",
            &wait.jobs,
        );
        let reason = wakeup::observe(
            wait,
            std::slice::from_ref(wait),
            &mut statuses,
            now + time::Duration::seconds(at),
            60,
        );
        (reason, statuses)
    };
    let (reason, statuses) = observe(&wait, "Q", 0);
    assert_eq!(reason, None);
    wait.last_status = statuses;
    let (reason, statuses) = observe(&wait, "R", 59);
    assert_eq!(reason, None);
    assert!(statuses[0].queued_since.is_none());
    wait.last_status = statuses;
    let (_, statuses) = observe(&wait, "H", 60);
    wait.last_status = statuses;
    assert_eq!(observe(&wait, "Q", 119).0, None);
    let (reason, statuses) = observe(&wait, "Q", 120);
    assert_eq!(reason, Some("queue_stalled"));
    wait.last_status = statuses;
    let restored: ClusterJobWait =
        serde_json::from_str(&serde_json::to_string(&wait).unwrap()).unwrap();
    assert_eq!(observe(&restored, "H", 240).0, None);
    let mut qualified = restored.clone();
    qualified.wait_id = "qualified-rewait".into();
    qualified.jobs = ids(&["1.sirius-pbs"]);
    qualified.last_status.clear();
    let mut statuses = parse_pbs_qstat_xf(
        "Job Id: 1.sirius-pbs\n job_state = Q\n",
        "",
        &qualified.jobs,
    );
    assert_eq!(
        wakeup::observe(
            &qualified,
            std::slice::from_ref(&restored),
            &mut statuses,
            now + time::Duration::seconds(300),
            60
        ),
        None
    );
    assert!(statuses[0].queue_notified);
    let mut next = restored.clone();
    next.wait_id = "another-wait".into();
    next.work_unit_id = Some("another-owner".into());
    next.last_status.clear();
    let mut statuses = parse_pbs_qstat_xf("Job Id: 1\n job_state = Q\n", "", &next.jobs);
    assert_eq!(
        wakeup::observe(&next, &[restored], &mut statuses, now, 60),
        None
    );
    assert!(!statuses[0].queue_notified);
}

/// Invariant: one failure wakes the worker once. Re-waiting with the inspected failed job
/// (here the original `[1(F, -14), 2(Q), 3(R)]`) must not wake on every poll, and a poll that
/// misses the job (`Unknown`) is not an observation that re-arms the wake. The removed test
/// asserted the opposite (every failed observation woke), which made runs loop each poll_secs.
#[test]
fn the_same_failure_does_not_wake_again_when_waited_again() {
    let now = time::OffsetDateTime::now_utc();
    let mut first = wait_for(task(crate::Status::Running).id);
    first.jobs = ids(&["1", "2", "3"]);
    let output = "Job Id: 1\n job_state = F\n Exit_status = -14\n\
                  Job Id: 2\n job_state = Q\nJob Id: 3\n job_state = R\n";
    let mut statuses = parse_pbs_qstat_xf(output, "", &first.jobs);
    assert_eq!(
        wakeup::observe(&first, &[], &mut statuses, now, 3600),
        Some("job_failed")
    );
    // The wake closes the first wait with these statuses; the continuation waits again.
    first.last_status = statuses;
    first.state = ClusterJobWaitState::Satisfied;
    let mut rewait = first.clone();
    rewait.wait_id = "rewait".into();
    rewait.last_status.clear();
    rewait.state = ClusterJobWaitState::Waiting;
    let history = vec![first.clone(), rewait.clone()];
    let mut statuses = parse_pbs_qstat_xf(output, "", &rewait.jobs);
    assert_eq!(
        wakeup::observe(&rewait, &history, &mut statuses, now, 3600),
        None
    );
    // Later polls of the same wait read the failure from this wait's own last_status.
    rewait.last_status = statuses;
    let mut statuses = parse_pbs_qstat_xf(output, "", &rewait.jobs);
    assert_eq!(
        wakeup::observe(&rewait, &[first.clone()], &mut statuses, now, 3600),
        None
    );
    // A poll that does not report job 1 leaves no observation; the failure stays known.
    let missing = parse_pbs_qstat_xf(
        "Job Id: 2\n job_state = Q\nJob Id: 3\n job_state = R\n",
        "",
        &rewait.jobs,
    );
    assert_eq!(missing[0].state, ClusterJobState::Unknown);
    rewait.last_status = missing;
    let mut statuses = parse_pbs_qstat_xf(output, "", &rewait.jobs);
    assert_eq!(
        wakeup::observe(&rewait, &[first.clone()], &mut statuses, now, 3600),
        None
    );
    // Another task, WorkUnit or cluster has no shared history: its failure still wakes.
    let mut other = rewait.clone();
    other.wait_id = "other-owner".into();
    other.work_unit_id = Some("another-unit".into());
    other.last_status.clear();
    let mut statuses = parse_pbs_qstat_xf(output, "", &other.jobs);
    assert_eq!(
        wakeup::observe(&other, &[first], &mut statuses, now, 3600),
        Some("job_failed")
    );
}

/// Invariant: a same-ID rerun (PBS rerun keeps the ID) that is observed in Q/R/H and then
/// fails again is a new failure and wakes the worker once more, in the same wait or a re-wait.
#[test]
fn a_rerun_seen_queued_that_fails_again_wakes_again() {
    let now = time::OffsetDateTime::now_utc();
    let mut wait = wait_for(task(crate::Status::Running).id);
    wait.jobs = ids(&["1", "2"]);
    let failed = "Job Id: 1\n job_state = F\n Exit_status = -14\nJob Id: 2\n job_state = R\n";
    let mut statuses = parse_pbs_qstat_xf(failed, "", &wait.jobs);
    assert_eq!(
        wakeup::observe(&wait, &[], &mut statuses, now, 3600),
        Some("job_failed")
    );
    let mut done = wait.clone();
    done.last_status = statuses;
    done.state = ClusterJobWaitState::Satisfied;
    let mut rewait = done.clone();
    rewait.wait_id = "rewait".into();
    rewait.last_status.clear();
    rewait.state = ClusterJobWaitState::Waiting;
    for (state, at) in [("Q", 300), ("R", 600), ("H", 900)] {
        let mut statuses = parse_pbs_qstat_xf(
            &format!("Job Id: 1\n job_state = {state}\nJob Id: 2\n job_state = R\n"),
            "",
            &rewait.jobs,
        );
        assert_eq!(
            wakeup::observe(
                &rewait,
                &[done.clone()],
                &mut statuses,
                now + time::Duration::seconds(at),
                3600
            ),
            None
        );
        rewait.last_status = statuses;
        let mut statuses = parse_pbs_qstat_xf(failed, "", &rewait.jobs);
        assert_eq!(
            wakeup::observe(
                &rewait,
                &[done.clone()],
                &mut statuses,
                now + time::Duration::seconds(at + 1),
                3600
            ),
            Some("job_failed"),
            "a failure after observing {state} is a new failure"
        );
    }
}

// ---------------------------------------------------------------------------
// ADR 2026-11-24-cluster-job-wait-nqsv: NEC NQSV（Pegasus）
// ---------------------------------------------------------------------------

/// 実機の出力: 2026-11-24T12:28:33Z に Pegasus で `bash -lc "qstat -f 315477 315478.nqsv"`（stdout、exit 0、stderr 空）。
/// 終わった request は server 名の無い id で聞いても `<番号>.nqsv` の形で答える。
const NQSV_DOES_NOT_EXIST: &str = "Batch Request: 315477.nqsv does not exist on nqsv.\n\
Batch Request: 315478.nqsv does not exist on nqsv.\n";

/// 実機の出力: 2026-11-24T12:28:33Z に Pegasus で `bash -lc "qstat -xf 315477.nqsv"`（NQSV に `-x` は無い。exit 0）の先頭。
const NQSV_QSTAT_X_USAGE: &str = "qstat: invalid option -- 'x'\nusage: \n\
qstat    [-P privilege] [-V] [-f] [-n] [-l] [-d] [-m] [-s] [-a]\n\
         [-u userlist] [-q quelist] [-c cpumode] [-F Item list]\n\
         [-o Item list] [-O Item list] [--group[=group-name]]\n\
         [--adjust-column] [--planned-start-time] [batch_request_identifier ...]\n";

/// **合成**（実機の書式ではない。Q・R の request の `qstat -f` は未取得。ADR D4 の想定の形）。状態名は Pegasus の
/// `qstat -Q` の列名（`ARR WAI QUE PRR RUN POR EXT HLD HOL SUS MIG STG`）から取った。
const NQSV_LIVE_SYNTHETIC: &str = "Request ID: 315480.nqsv\n\
    Request Name: pilot-run.sh\n\
    Owner: rmaeda\n\
    Current State: QUE\n\
    Queue: gpu\n\
\n\
Request ID: 315481.nqsv\n\
    Request Name: pilot-run-b.sh\n\
    Current State: RUNNING (Since 2026/11/24 09:00:00)\n\
\n\
Request ID: 315482.nqsv\n\
    State = HLD\n\
\n\
Request ID: 315483.nqsv\n\
    Request Name: no-state-line.sh\n";

/// 不変条件: NQSV の終わった request（`does not exist`）は `gone` で、wait を満たす。PBS の parser は `Unknown Job Id`
/// しか読まないので、この出力では全 job が `unknown` のまま（既存の試験はこの文言を持たない）。
#[test]
fn nqsv_request_that_does_not_exist_is_gone_and_satisfies_the_wait() {
    let jobs = ids(&["315477", "315478.nqsv"]);
    let got = parse_poll_output(ClusterScheduler::Nqsv, NQSV_DOES_NOT_EXIST, "", &jobs);
    assert_eq!(got[0].state, ClusterJobState::Gone);
    assert_eq!(got[1].state, ClusterJobState::Gone);
    assert_eq!(got[0].exit_status, None, "NQSV gives no exit status");
    assert_eq!(
        got[0].raw_state.as_deref(),
        Some("Batch Request: 315477.nqsv does not exist on nqsv.")
    );
    assert!(all_finished(&jobs, &got));
    // 同じ行が stderr に出ても読む（2>&1 で見た運用者の確認では stream が分からなかった）。
    let got = parse_poll_output(ClusterScheduler::Nqsv, "", NQSV_DOES_NOT_EXIST, &jobs);
    assert!(all_finished(&jobs, &got));
    // 1 件だけ消えていれば、もう 1 件は unknown のまま（解けない）。
    let jobs = ids(&["315477", "315479"]);
    let got = parse_poll_output(ClusterScheduler::Nqsv, NQSV_DOES_NOT_EXIST, "", &jobs);
    assert_eq!(got[1].state, ClusterJobState::Unknown);
    assert!(!all_finished(&jobs, &got));
}

/// 不変条件: `does not exist` の無い request（Q・R・H、状態の読めない段落、出力に無い）では wait が解けない。
#[test]
fn nqsv_queued_running_and_unread_requests_do_not_satisfy_the_wait() {
    let jobs = ids(&["315480", "315481", "315482", "315483", "315484"]);
    let got = parse_poll_output(ClusterScheduler::Nqsv, NQSV_LIVE_SYNTHETIC, "", &jobs);
    assert_eq!(got[0].state, ClusterJobState::Queued);
    assert_eq!(got[0].raw_state.as_deref(), Some("QUE"));
    assert_eq!(got[1].state, ClusterJobState::Running);
    assert_eq!(got[2].state, ClusterJobState::Held);
    assert_eq!(got[3].state, ClusterJobState::Unknown);
    assert_eq!(
        got[3].raw_state.as_deref(),
        Some("listed by qstat (state not read)")
    );
    assert_eq!(got[4].state, ClusterJobState::Unknown);
    assert!(got.iter().all(|s| !s.state.is_finished()));
    // Q と R の request が 1 つでも残れば、他が消えていても解けない。
    let jobs = ids(&["315477", "315481"]);
    let out = format!("{NQSV_DOES_NOT_EXIST}{NQSV_LIVE_SYNTHETIC}");
    let got = parse_poll_output(ClusterScheduler::Nqsv, &out, "", &jobs);
    assert_eq!(got[0].state, ClusterJobState::Gone);
    assert_eq!(got[1].state, ClusterJobState::Running);
    assert!(!all_finished(&jobs, &got));
    // `qstat -xf`（PBS の形）を NQSV に流した usage は、どの request も終わったことにしない。
    let got = parse_poll_output(ClusterScheduler::Nqsv, "", NQSV_QSTAT_X_USAGE, &jobs);
    assert!(got.iter().all(|s| s.state == ClusterJobState::Unknown));
}

/// 不変条件: NQSV の poll は非ログインの ssh でも qstat を見つける形（ログインシェルの中で env・setup・`qstat -f`）で、
/// `-x` を使わない。PBS・Slurm の script は従来どおり（前置き + poll_command）。
#[test]
fn nqsv_poll_script_runs_qstat_f_inside_a_login_shell() {
    let prelude = "export NQSV_X='it'\\''s' && { module load nqsv; } && ";
    let script = poll_script(
        ClusterScheduler::Nqsv,
        prelude,
        &ids(&["315477", "315478.nqsv", "1; rm -rf ~"]),
    );
    assert_eq!(
        script,
        "bash -lc 'export NQSV_X='\\''it'\\''\\'\\'''\\''s'\\'' && { module load nqsv; } && qstat -f 315477 315478.nqsv'"
    );
    assert!(!script.contains("-xf"));
    assert_eq!(
        poll_command(ClusterScheduler::Nqsv, &ids(&["315477"])),
        "qstat -f 315477"
    );
    assert_eq!(
        poll_script(
            ClusterScheduler::Pbs,
            "{ module load openpbs; } && ",
            &ids(&["1"])
        ),
        "{ module load openpbs; } && qstat -xf 1"
    );
    assert_eq!(
        poll_script(ClusterScheduler::Slurm, "", &ids(&["1", "2"])),
        "sacct -n -P -X -o JobID,State%64,ExitCode -j 1,2"
    );
    // scheduler は申告せず設定で決まる（ADR 2026-11-25-cluster-scheduler-from-config。申告の拒否は
    // `wait_request_naming_a_scheduler_is_rejected`）。
    assert_eq!(
        ClusterScheduler::parse("nqsv"),
        Some(ClusterScheduler::Nqsv)
    );
    assert_eq!(ClusterScheduler::Nqsv.as_str(), "nqsv");
}

/// 不変条件（no-data-loss）: migration 0127 は schema 126 の `cluster_job_waits` の行を残し、`nqsv` の行を入れられる
/// ようにする（126 の CHECK は `nqsv` を拒む）。
#[test]
fn migration_0127_keeps_wait_rows_and_accepts_nqsv() {
    use crate::SqliteStore;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("celeris.db");
    let t = task(crate::Status::Blocked);
    {
        let store = SqliteStore::open(&path).unwrap();
        store.insert(&t).unwrap();
    }
    let insert = |conn: &Connection, wait_id: &str, scheduler: &str| {
        conn.execute(
            "INSERT INTO cluster_job_waits (wait_id, task_id, run_id, cluster, scheduler, jobs_json, poll_secs, \
             timeout_secs, created_at, deadline, state) VALUES (?1, ?2, 'run-1', 'pegasus', ?3, '[\"315477\"]', \
             300, 3600, '2026-11-24T09:00:00Z', '2026-11-24T12:30:00Z', 'waiting')",
            params![wait_id, t.id.to_string(), scheduler],
        )
    };
    {
        // schema 126 の表に戻す（0034 の CHECK）。
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE cluster_job_waits; \
             CREATE TABLE cluster_job_waits (wait_id TEXT PRIMARY KEY, task_id TEXT NOT NULL, work_unit_id TEXT, \
             run_id TEXT NOT NULL, cluster TEXT NOT NULL, \
             scheduler TEXT NOT NULL CHECK (scheduler IN ('pbs', 'slurm')), jobs_json TEXT NOT NULL, \
             poll_secs INTEGER NOT NULL, timeout_secs INTEGER NOT NULL, summary TEXT NOT NULL DEFAULT '', \
             checkpoint_json TEXT, created_at TEXT NOT NULL, deadline TEXT NOT NULL, last_polled_at TEXT, \
             finished_at TEXT, \
             state TEXT NOT NULL CHECK (state IN ('waiting', 'satisfied', 'timed_out', 'cancelled')), \
             last_status_json TEXT NOT NULL DEFAULT '[]'); \
             DELETE FROM schema_migrations WHERE version = 127;",
        )
        .unwrap();
        insert(&conn, "w-pbs", "pbs").unwrap();
        assert!(
            insert(&conn, "w-nqsv-old", "nqsv").is_err(),
            "schema 126 rejects nqsv"
        );
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), 127);
    let kept = store.cluster_job_wait_get("w-pbs").unwrap().unwrap();
    assert_eq!(kept.scheduler, ClusterScheduler::Pbs);
    assert_eq!(kept.jobs, ids(&["315477"]));
    drop(store);
    let conn = Connection::open(&path).unwrap();
    insert(&conn, "w-nqsv", "nqsv").unwrap();
    drop(conn);
    let store = SqliteStore::open(&path).unwrap();
    let w = store.cluster_job_wait_get("w-nqsv").unwrap().unwrap();
    assert_eq!(w.scheduler, ClusterScheduler::Nqsv);
    assert_eq!(store.cluster_job_waits_waiting().unwrap().len(), 2);
}

/// ADR 2026-11-25-cluster-scheduler-from-config D4: 失敗の poll の分け方。2026-11-25 の事象の exit 127 と NQSV の
/// usage は失敗（要約は `exit N: <stderr の末尾>`）、job の状態を 1 つでも読めた poll と exit 0 の poll は失敗でない。
#[test]
fn poll_results_without_job_states_are_failures_only_when_the_command_failed() {
    let jobs = ids(&["318985", "318986"]);
    let err = read_poll_output(
        ClusterScheduler::Pbs,
        Some(127),
        "",
        "bash: 行 1: qstat: コマンドが見つかりません\n",
        &jobs,
    )
    .unwrap_err();
    assert_eq!(err, "exit 127: bash: 行 1: qstat: コマンドが見つかりません");
    let usage = read_poll_output(
        ClusterScheduler::Nqsv,
        Some(2),
        "",
        "qstat: invalid option -- 'x'\nusage: qstat [-f] ...",
        &jobs,
    )
    .unwrap_err();
    assert!(
        usage.starts_with("exit 2: qstat: invalid option"),
        "{usage}"
    );
    let long = "e".repeat(POLL_ERROR_MAX_CHARS + 50);
    let tail = read_poll_output(ClusterScheduler::Pbs, None, &long, "", &jobs).unwrap_err();
    assert_eq!(
        tail,
        format!("no exit code: {}", "e".repeat(POLL_ERROR_MAX_CHARS))
    );
    // exit 0 で何も読めない poll は失敗に数えない（`Unknown` のまま）。
    let silent = read_poll_output(ClusterScheduler::Nqsv, Some(0), "", "", &jobs).unwrap();
    assert!(silent.iter().all(|s| s.state == ClusterJobState::Unknown));
    // exit ≠ 0 でも job の状態を読めれば失敗でない（PBS の一部の job が Unknown Job Id のとき）。
    let partial = read_poll_output(
        ClusterScheduler::Pbs,
        Some(153),
        "",
        "qstat: Unknown Job Id 318985.sirius-pbs\n",
        &jobs,
    )
    .unwrap();
    assert_eq!(partial[0].state, ClusterJobState::Gone);
}
