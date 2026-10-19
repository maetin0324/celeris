//! dagbench の Celeris 側（`dagbench::celeris_side`）: 固定 scenario を既存 Celeris の通常経路で流し、
//! 外部 ledger（ext-sim）・worker stub ledger・SQLite の三者が整合することを確かめる。
//!
//! 再利用: task-api の実 router と一時 SQLite は `task-api/tests/common`（`TestEnv`。`tests/daemon_identity` と同じ
//! 取り込み方）、投入は `celeris_exec::Transport`、計画・ready 判定・durable wait・continuation は実 Dispatcher。
//! 測定専用の所有資源だけ: tempdir の DB と出力、この process 内の Dispatcher と ext-sim（daemon・userns・network なし、
//! ADR-0126）。時間は論理時計だけで進む（sleep の長さで時間を作らない）。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    dagbench::{
        celeris_side::{
            CelerisSide, Paths, RunSpec, WorkerEntry, load_scenarios, read_worker_ledger,
        },
        ext::{Entry, count, read_ledger},
    },
};
use serde_json::Value;
use task_core::{Status, TaskId, TaskStore, cluster_job::ClusterJobWaitState};

/// task-api の実 router への同期の Transport（要求ごとに専用 thread の runtime で oneshot する）。
struct RouterTransport(axum::Router);

impl Transport for RouterTransport {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        let app = self.0.clone();
        let req = match (method, body) {
            ("POST", Some(b)) => api_common::post_admin(path, b),
            ("GET", None) => api_common::get_admin(path),
            _ => return Err(format!("unsupported {method} {path}")),
        };
        std::thread::scope(|s| {
            s.spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                let r = rt.block_on(api_common::send(&app, req));
                Ok(HttpResponse {
                    status: r.status.as_u16(),
                    body: serde_json::from_slice(&r.body).unwrap_or(Value::Null),
                })
            })
            .join()
            .map_err(|_| "transport thread panicked".to_string())?
        })
    }
}

fn scenarios() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/scenarios.json")
}

struct Outcome {
    spec: RunSpec,
    workers: Vec<WorkerEntry>,
    ext: Vec<Entry>,
    store: Arc<dyn TaskStore>,
    tasks: Vec<TaskId>,
    logical_end: u64,
}

fn run_point(scenario: &str, point: &str) -> Outcome {
    let file = load_scenarios(&scenarios()).unwrap();
    let spec = RunSpec::resolve(&file, scenario, point, file.seed).unwrap();
    let env = api_common::admin_env();
    let out = env.dir.path().join("dagbench");
    std::fs::create_dir_all(&out).unwrap();
    let paths = Paths::under(&env.db_path, &out);
    let store: Arc<dyn TaskStore> = Arc::new(task_core::SqliteStore::open(&env.db_path).unwrap());
    let transport = RouterTransport(env.router());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let report = rt.block_on(async {
        let mut side = CelerisSide::new(spec.clone(), paths, store.clone()).unwrap();
        side.drive(&transport).await
    });
    let report = report.unwrap_or_else(|e| panic!("drive failed: {e}"));
    assert_eq!(report.dataset, "control_measured");
    assert_eq!((report.llm_calls, report.paid_jobs), (0, 0));
    assert_eq!(
        report.measurement_id,
        format!("dagbench-1-{scenario}-{point}-{}-celeris", file.seed)
    );
    for c in &report.campaigns {
        assert_eq!(c.status, "done", "{:?}", report.campaigns);
    }
    Outcome {
        workers: read_worker_ledger(&out.join("worker.jsonl")).unwrap(),
        ext: read_ledger(&out.join("ext.jsonl")).unwrap(),
        tasks: report
            .campaigns
            .iter()
            .map(|c| c.task_id.parse().unwrap())
            .collect(),
        logical_end: report.logical_end_secs,
        spec,
        store,
    }
}

fn at(w: &[WorkerEntry], campaign: &str, node: &str, event: &str) -> Vec<u64> {
    w.iter()
        .filter(|e| e.campaign == campaign && e.node == node && e.event == event)
        .map(|e| e.logical_secs)
        .collect()
}

/// 三者（worker stub ledger・ext-sim ledger・SQLite）の整合と DAG 順序・資源枠。
fn check_consistent(o: &Outcome) {
    let n_campaigns = o.spec.point.concurrent_campaigns;
    let externals: Vec<_> = o
        .spec
        .scenario
        .nodes
        .iter()
        .filter(|n| n.external.is_some())
        .collect();

    // ext-sim の独立記録: 物理起動 = 論理 job = 消費、二重消費なし。
    let c = count(&o.ext);
    let expect_jobs = n_campaigns * externals.len();
    assert_eq!(c.logical_jobs, expect_jobs);
    assert_eq!(
        c.physical_launches, expect_jobs,
        "one physical launch per logical job"
    );
    assert_eq!(c.consumed, expect_jobs, "each result consumed once");
    assert!(
        o.ext
            .iter()
            .all(|e| e.event != "consume_ignored" && e.event != "submit_deduped")
    );
    assert!(o.ext.iter().all(|e| e.dataset == "control_measured"));
    let completed: BTreeMap<String, u64> = o
        .ext
        .iter()
        .filter(|e| e.event == "external_job_completed")
        .filter_map(|e| Some((e.logical_job_id.clone()?, e.logical_secs)))
        .collect();

    for (ci, campaign) in o.spec.campaign_ids().iter().enumerate() {
        let arrival = o.spec.arrival(ci);
        for node in &o.spec.scenario.nodes {
            let started = at(&o.workers, campaign, &node.id, "node_started");
            let finished = at(&o.workers, campaign, &node.id, "node_finished");
            assert_eq!(started.len(), 1, "{campaign}/{} started once", node.id);
            assert_eq!(finished.len(), 1, "{campaign}/{} finished once", node.id);
            assert!(started[0] >= arrival);
            // 全依存の完了より前に始まらない（系の ready 判定ではなく ledger から確かめる）。
            for dep in &node.deps {
                let dep_done = at(&o.workers, campaign, dep, "node_finished")[0];
                assert!(
                    started[0] >= dep_done,
                    "{campaign}/{} before {dep}",
                    node.id
                );
            }
            if let Some(ext) = &node.external {
                let logical = RunSpec::logical_job(campaign, ext);
                let post = at(&o.workers, campaign, &node.id, "post_wait_started");
                assert_eq!(post.len(), 1, "{logical}: one continuation");
                assert!(
                    post[0] >= completed[&logical],
                    "{logical}: resumed after completion"
                );
                assert_eq!(finished[0], post[0] + ext.post_wait_work_secs);
            } else {
                assert_eq!(finished[0], started[0] + node.work_secs);
            }
        }
        // SQLite（状態の真実）: root は done、WorkUnit はすべて done、外部待ちは satisfied。
        let id = o.tasks[ci];
        assert_eq!(o.store.get(id).unwrap().unwrap().status, Status::Done);
        let units = o.store.work_units_for(id).unwrap();
        // node ごとに 1 unit（他は Celeris が stage ごとに足す統合 unit）。
        for node in &o.spec.scenario.nodes {
            assert_eq!(
                units.iter().filter(|u| u.key == node.id).count(),
                1,
                "{}",
                node.id
            );
        }
        assert!(
            units
                .iter()
                .all(|u| u.status == task_core::WorkUnitStatus::Done),
            "{units:?}"
        );
        let waits = o.store.cluster_job_waits_for_task(id).unwrap();
        assert_eq!(waits.len(), externals.len());
        assert!(
            waits
                .iter()
                .all(|w| w.state == ClusterJobWaitState::Satisfied)
        );
    }

    // worker 枠: 同時に働いている worker run は worker_slots 以下（外部 job の待ちは枠を持たない）。
    let mut edges: Vec<(u64, i32)> = Vec::new();
    for e in &o.workers {
        match e.event.as_str() {
            "node_started" | "post_wait_started" => edges.push((e.logical_secs, 1)),
            "external_wait_started" | "node_finished" => edges.push((e.logical_secs, -1)),
            _ => {}
        }
    }
    edges.sort();
    let mut busy = 0;
    for (_, d) in edges {
        busy += d;
        assert!(
            busy <= o.spec.scenario.resources.worker_slots as i32,
            "worker slots exceeded"
        );
    }
}

/// 1 campaign（camp-1）: Celeris 通常経路で完走し、makespan は臨界経路
/// （survey 120 + impl 270 + verify 180 + 実験 60 + job 990 + 後処理 30 + analyze 120 + report 120 + review 90）。
#[test]
fn k2_mini_single_campaign_completes_and_matches_the_external_ledger() {
    let o = run_point("k2-mini", "camp-1");
    check_consistent(&o);
    assert_eq!(o.spec.ext_job_secs, 990);
    let last = o.workers.iter().map(|e| e.logical_secs).max().unwrap();
    let timeline: Vec<String> = o
        .workers
        .iter()
        .map(|e| format!("{} {} {}", e.logical_secs, e.event, e.node))
        .chain(
            o.ext
                .iter()
                .map(|e| format!("{} ext:{} {:?}", e.logical_secs, e.event, e.logical_job_id)),
        )
        .collect();
    assert_eq!(last, 1980, "{timeline:#?}");
    assert_eq!(o.logical_end, 1980);
}

/// center 点（2 campaign、到着間隔 600、worker 枠 2・外部 job 枠 2）でも完走し三者が整合する。
#[test]
fn k2_mini_center_two_campaigns_complete_within_the_shared_slots() {
    let o = run_point("k2-mini", "center");
    assert_eq!(o.spec.point.concurrent_campaigns, 2);
    assert_eq!(o.spec.arrival(1), 600);
    check_consistent(&o);
}
