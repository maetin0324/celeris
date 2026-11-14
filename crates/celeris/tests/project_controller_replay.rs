//! ADR 2026-11-04-project-controller A6: 本物の control-ts の bundle を本物の V8 で 1 個だけ動かし、100 案件の判定
//! （`TsProjectDecider`）で RSS・isolate 数・thread 数・判定の遅延（Rust の snapshot の組み立て＝native の部分と、
//! TS の呼び出し）・100 案件 1 周の loop cost を測る（接頭辞 `project_controller_replay_`）。
//! 故障の注入と仮想時刻の replay は `task-dispatch` の `project_controller_replay_`（dispatcher の時計を注入できる
//! crate の中）にある。
//!
//! - LLM を呼ばない。外部ネットワークに出ない。本番の DB・daemon に触れない（一時 DB）。
//! - bundle が作れないと SKIP になるので、script は `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で走らせる（SKIP でなく失敗）。
//! - `PROJECT_CONTROLLER_REPLAY_RAW_DIR` があれば `resource.jsonl`（`celeris.long-horizon-record/1`）を書く。

mod support;

use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};
use support::proc_stats::{pct, status_kb, threads};
use support::script_compare::{dist_bundle, start_policy};
use task_core::project_wake::{ControllerMode, ProjectDecideInput};
use task_core::store::SqliteStore;
use task_core::{Project, ProjectId, ProjectStatus, TaskStore};
use task_dispatch::project_controller::ProjectDecider;
use task_dispatch::project_decider_ts::{DeciderLimits, TsProjectDecider, build_snapshot};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const DOMAIN: &str = "project_controller";
const PROJECTS: usize = 100;
const ROUNDS: usize = 3;
const REQUEST: &str = "計測の土台を作る。\n```celeris-outcomes\n[{\"id\":\"o1\",\"title\":\"土台\",\"acceptance\":[\"計測 script が同じ値を 3 回出す\"]}]\n```\n";

fn raw(value: Value) {
    let Some(dir) = std::env::var_os("PROJECT_CONTROLLER_REPLAY_RAW_DIR") else {
        return;
    };
    let mut v = value;
    v["schema"] = json!("celeris.long-horizon-record/1");
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join("resource.jsonl"))
        .and_then(|mut f| f.write_all(format!("{v}\n").as_bytes()))
        .expect("raw");
}

fn project(store: &SqliteStore, n: usize) -> ProjectId {
    let now = OffsetDateTime::now_utc();
    let p = Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: format!("pilot:a6-resource-{n}"),
        request: REQUEST.into(),
        status: ProjectStatus::Active,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&p).expect("project");
    store
        .project_controller_set_mode(
            &p.id.to_string(),
            ControllerMode::Enabled,
            None,
            "human:test",
            now,
        )
        .expect("mode");
    p.id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_controller_replay_resource_v8_100_projects() {
    let test = "project_controller_replay_resource_v8_100_projects";
    let rss_before = status_kb("VmRSS:");
    let threads_before = threads();
    let Some(bundle) = dist_bundle(test, DOMAIN) else {
        return;
    };
    let sha = bundle.manifest.bundle_sha256.clone();
    let (handle, policy) = start_policy(bundle.source(), sha);
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.sqlite3")).expect("open"));
    let projects: Vec<ProjectId> = (0..PROJECTS).map(|n| project(&store, n)).collect();
    let mut decider = TsProjectDecider::with_fixed_script(DeciderLimits::default(), policy);
    decider.attach(&store, None);
    // 1 回目の呼び出しで isolate を温める（以後の測定に起動の費用を入れない）。
    let input = |p: &ProjectId| {
        let state = store
            .project_controller_state(&p.to_string())
            .expect("state")
            .expect("row");
        ProjectDecideInput {
            project_id: p.to_string(),
            revision: state.revision,
            human_epoch: state.human_epoch,
            mode: ControllerMode::Enabled,
            wakes: Vec::new(),
            now: OffsetDateTime::now_utc().format(&Rfc3339).expect("now"),
        }
    };
    decider.decide(&input(&projects[0])).expect("warm");
    let rss_idle = status_kb("VmRSS:");
    let threads_idle = threads();
    raw(json!({
        "test": test, "kind": "ts_decider", "scenario": "idle", "projects": 0,
        "rss_kb_before_isolate": rss_before, "rss_kb": rss_idle, "isolates": 1,
        "threads_before_isolate": threads_before, "threads": threads_idle, "llm_launches": 0,
    }));
    let mut decide_us = Vec::new();
    let mut snapshot_us = Vec::new();
    let mut loop_us = Vec::new();
    let mut max_threads = threads_idle;
    let limits = DeciderLimits::default();
    for _ in 0..ROUNDS {
        let round = Instant::now();
        for p in &projects {
            let i = input(p);
            let t = Instant::now();
            build_snapshot(&store, &i, &limits, OffsetDateTime::now_utc()).expect("snapshot");
            snapshot_us.push(t.elapsed().as_micros() as u64);
            let t = Instant::now();
            decider.decide(&i).expect("decide");
            decide_us.push(t.elapsed().as_micros() as u64);
            max_threads = max_threads.max(threads());
        }
        loop_us.push(round.elapsed().as_micros() as u64);
    }
    let rss_busy = status_kb("VmRSS:");
    let threads_busy = threads();
    // 案件ごとに isolate も常駐の thread も増えない（呼び出しの間だけ待ちの thread が 1 本）。
    assert!(
        threads_busy <= threads_idle + 1,
        "threads grew: idle {threads_idle}, after {threads_busy}"
    );
    assert!(max_threads <= threads_idle + 2, "max {max_threads}");
    raw(json!({
        "test": test, "kind": "ts_decider", "scenario": "projects_100", "projects": PROJECTS,
        "rounds": ROUNDS, "rss_kb": rss_busy, "rss_kb_delta_from_idle": rss_busy as i64 - rss_idle as i64,
        "isolates": 1, "threads": threads_busy, "threads_max": max_threads, "llm_launches": 0,
        "decide_micros_p50": pct(&decide_us, 50), "decide_micros_p95": pct(&decide_us, 95),
        "native_snapshot_micros_p50": pct(&snapshot_us, 50),
        "native_snapshot_micros_p95": pct(&snapshot_us, 95),
        "loop_micros_per_100_projects_p50": pct(&loop_us, 50),
        "loop_micros_per_100_projects_p95": pct(&loop_us, 95),
        "samples": decide_us.len(),
    }));
    handle.shutdown();
}
