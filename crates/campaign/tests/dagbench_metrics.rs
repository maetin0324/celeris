//! dagbench の指標再生成: 生ログ（ext ledger・worker ledger・control・/proc 標本）だけから計算し、2 回で同一。
//! 負荷生成なし。時間は論理時計だけ（sleep しない）。標本は /proc を読むだけ。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::{fs, path::Path, sync::Arc};

use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    dagbench::{
        celeris_side::{CelerisSide, Paths, RunSpec, load_scenarios},
        metrics::{
            METRICS_FILE, METRICS_SCHEMA, METRICS_TSV, RunHeader, append_control, append_sample,
            compute, read_proc_sample, sweep_items, write_tables,
        },
        run::{CELERIS_SCOPE, run_baseline},
    },
};
use serde_json::{Value, json};
use task_core::TaskStore;

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

fn scenarios() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/scenarios.json")
}

fn spec(scenario: &str, point: &str) -> RunSpec {
    let file = load_scenarios(&scenarios()).unwrap();
    RunSpec::resolve(&file, scenario, point, file.seed).unwrap()
}

/// 同じ raw から 2 回再生成して、表（JSON・TSV）が byte 単位で同一であることを確かめる。
fn regenerate_twice(raw: &Path) -> celeris_campaign::dagbench::metrics::Metrics {
    let (a, b) = (raw.join("../out-a"), raw.join("../out-b"));
    let m1 = compute(raw).unwrap();
    write_tables(&a, &m1).unwrap();
    let m2 = compute(raw).unwrap();
    write_tables(&b, &m2).unwrap();
    for f in [METRICS_FILE, METRICS_TSV] {
        assert_eq!(
            fs::read(a.join(f)).unwrap(),
            fs::read(b.join(f)).unwrap(),
            "{f}"
        );
    }
    assert_eq!(m1, m2);
    m1
}

#[test]
fn baseline_metrics_are_regenerated_identically_from_raw_logs() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("raw");
    run_baseline(&spec("k2-mini", "center"), &raw).unwrap();
    let m = regenerate_twice(&raw);

    assert_eq!(m.dataset, "control_measured");
    assert_eq!(m.schema, METRICS_SCHEMA);
    assert_eq!((m.llm_calls, m.paid_jobs), (0, 0));
    assert_eq!(m.condition, "normal");
    // 2 campaign × 8 node。全部終わる。
    assert_eq!((m.completed.nodes, m.completed.campaigns), (16, 2));
    // 最初の campaign は競合前に流れ始めるので makespan は単独実行（1320）以上。
    assert!(m.makespan.per_campaign["c0"] >= 1320, "{:?}", m.makespan);
    // 外部 job は 2 campaign × 2 本、起動も消費も 1 回ずつ。
    assert_eq!(m.duplicates.extra_physical_launches, 0);
    assert_eq!(m.duplicates.double_counted, 0);
    assert_eq!(m.lost_progress.total_secs, 0);
    assert_eq!(m.interventions, 0);
    // 標本は開始と終了の 2 点（sampling のみ）。
    assert_eq!(m.control_overhead.sample_count, 2);
    assert!(m.control_overhead.peak_rss_kb > 0);
    // 生ログの hash が表に載る（どの raw から作ったかが分かる）。
    assert_ne!(m.raw_sha256["worker.jsonl"], "absent");
    // raw を 1 行書き換えると表も変わる（自己申告の集計値を読んでいない）。
    let w = fs::read_to_string(raw.join("worker.jsonl")).unwrap();
    let cut: String = w
        .lines()
        .filter(|l| !l.contains("\"node_finished\"") || !l.contains("\"review\""))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(raw.join("worker.jsonl"), cut).unwrap();
    let m2 = compute(&raw).unwrap();
    assert!(m2.completed.nodes < m.completed.nodes);
}

#[test]
fn celeris_metrics_come_from_the_same_raw_format() {
    let s = spec("k2-mini", "camp-1");
    let env = api_common::admin_env();
    let raw = env.dir.path().join("raw");
    fs::create_dir_all(&raw).unwrap();
    RunHeader::from_spec(&s, "celeris", CELERIS_SCOPE)
        .write(&raw)
        .unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(task_core::SqliteStore::open(&env.db_path).unwrap());
    let transport = RouterTransport(env.router());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    append_sample(&raw, std::process::id(), 0).unwrap();
    rt.block_on(async {
        let mut side =
            CelerisSide::new(s.clone(), Paths::under(&env.db_path, &raw), store).unwrap();
        side.drive(&transport).await.unwrap();
    });
    append_sample(&raw, std::process::id(), 1).unwrap();
    let m = regenerate_twice(&raw);
    assert_eq!((m.completed.nodes, m.completed.campaigns), (8, 1));
    // 理論値 1980（celeris_drv の試験と同じ）。
    assert_eq!(m.makespan.max, 1980);
    assert_eq!(m.duplicates.extra_physical_launches, 0);
    assert_eq!(m.duplicates.double_counted, 0);
    assert_eq!(
        m.measurement_id,
        "dagbench-1-k2-mini-camp-1-20261019-celeris"
    );
    assert_eq!(m.condition, "normal");
}

fn line(v: Value) -> String {
    format!("{v}\n")
}

/// 障害・再起動・介入・重複・標本を手で組んだ raw（期待値は手計算）。
fn synthetic_fault_raw(raw: &Path) {
    let mut h = RunHeader::from_spec(&spec("k2-mini", "camp-1"), "baseline", "test");
    h.campaigns = vec![("c0".into(), 0)];
    h.worker_slots = 1;
    h.write(raw).unwrap();
    let w = |seq: u64, t: u64, ev: &str, node: &str| {
        line(
            json!({"seq": seq, "logical_secs": t, "event": ev, "dataset": "control_measured", "campaign": "c0", "node": node}),
        )
    };
    // survey 0-120、impl は 120 に ready だが枠 1 の survey は終わっている → 制御遅れ 30。impl は 150 開始、
    // 200 で制御停止、同じ node を 200 からやり直し（270 秒の作業を失う分は rerun として数える）。
    let mut worker = String::new();
    worker += &w(1, 0, "campaign_submitted", "");
    worker += &w(2, 0, "node_started", "survey");
    worker += &w(3, 120, "node_finished", "survey");
    worker += &w(4, 150, "node_started", "impl");
    worker += &w(5, 200, "node_started", "impl");
    worker += &w(6, 470, "node_finished", "impl");
    fs::write(raw.join("worker.jsonl"), worker).unwrap();
    let e = |seq: u64, t: u64, ev: &str, l: &str, d: Value| {
        line(
            json!({"seq": seq, "logical_secs": t, "event": ev, "dataset": "control_measured", "logical_job_id": l, "physical_id": "phys-1", "detail": d}),
        )
    };
    let mut ext = String::new();
    ext += &e(
        1,
        100,
        "physical_launch",
        "c0/exp-uring",
        json!({"due_secs": 430, "job_secs": 330}),
    );
    ext += &e(
        2,
        100,
        "fault_fired",
        "c0/exp-uring",
        json!({"fault_id": "b-test"}),
    );
    ext += &e(
        3,
        200,
        "lookup",
        "c0/exp-uring",
        json!({"result": "running"}),
    );
    ext += &e(
        4,
        230,
        "lookup",
        "c0/exp-uring",
        json!({"result": "running"}),
    );
    ext += &e(
        5,
        300,
        "physical_launch",
        "c0/exp-uring",
        json!({"due_secs": 630, "job_secs": 330}),
    );
    ext += &e(6, 430, "notice_sent", "c0/exp-uring", json!({"copy": 1}));
    ext += &e(7, 440, "consumed", "c0/exp-uring", json!(null));
    ext += &e(8, 450, "consumed", "c0/exp-uring", json!(null));
    fs::write(raw.join("ext.jsonl"), ext).unwrap();
    append_control(raw, "control_restart", 200, Some(1000), None).unwrap();
    append_control(raw, "control_resumed", 200, Some(1250), None).unwrap();
    append_control(raw, "intervention", 210, None, Some("iv-1")).unwrap();
    append_control(raw, "intervention", 211, None, Some("iv-1")).unwrap();
    append_control(raw, "intervention", 212, None, Some("iv-2")).unwrap();
    let s = |seq: u64, pid: u32, ticks: u64, hwm: u64| {
        line(
            json!({"seq": seq, "wall_ms": seq, "dataset": "control_measured", "role": "control", "pid": pid,
            "clk_tck": 100, "utime_ticks": ticks, "stime_ticks": 0, "vm_rss_kb": hwm / 2, "vm_hwm_kb": hwm}),
        )
    };
    // pid 1 は 100→350 tick（2500 ms）、restart 後の pid 2 は 10→60 tick（500 ms）。
    let samples =
        s(1, 1, 100, 5000) + &s(2, 1, 350, 7000) + &s(3, 2, 10, 4000) + &s(4, 2, 60, 6000);
    fs::write(raw.join("samples.jsonl"), samples).unwrap();
}

#[test]
fn fault_recovery_duplicates_reconciliation_and_overhead_from_raw() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("raw");
    synthetic_fault_raw(&raw);
    let m = regenerate_twice(&raw);

    assert_eq!(m.condition, "fault:b-test,control_restart");
    // ready 待ち: survey 0、impl は 120 に ready で 150 開始 → 30（枠 1 は空いていた → 制御遅れ）。
    assert_eq!(
        (
            m.ready_wait.all.n,
            m.ready_wait.all.sum,
            m.ready_wait.all.max
        ),
        (2, 30, 30)
    );
    assert_eq!(
        (
            m.ready_wait.control_delay_nodes,
            m.ready_wait.control_delay_secs
        ),
        (1, 30)
    );
    assert_eq!(m.ready_wait.slot_wait_nodes, 0);
    // 重複: 論理 job は 2 回起動（+1）、消費記録が 2 件（二重計上 1）、重複通知 1 通。
    assert_eq!(m.duplicates.extra_physical_launches, 1);
    assert_eq!(m.duplicates.double_counted, 1);
    assert_eq!(m.duplicates.duplicate_notices_sent, 1);
    // 再照合: 障害（t=100）以後の lookup 2 件。
    assert!(m.reconciliation.disturbed);
    assert_eq!(m.reconciliation.lookups_after_disturbance, 2);
    assert_eq!(m.reconciliation.by_result["running"], 2);
    // 復旧: 最初の撹乱は t=100（fault_fired）。実時間は restart→resumed の 250 ms。
    assert_eq!(m.recovery.disturbance_logical_secs, Some(100));
    assert_eq!(m.recovery.wall_ms, Some(250));
    // 失われた進捗: impl のやり直し 1 回（270 秒）＋ 余分に起動した job（330 秒）。
    assert_eq!(m.lost_progress.rerun_node_starts, 1);
    assert_eq!(m.lost_progress.rerun_work_secs, 270);
    assert_eq!(m.lost_progress.restarted_job_secs, 330);
    assert_eq!(m.lost_progress.total_secs, 600);
    // 介入は intervention_id で 1 件に潰す（iv-1 が 2 回 → 2 件）。
    assert_eq!(m.interventions, 2);
    // CPU は pid ごとの tick 差の和（2500 + 500 ms）。RSS は peak。
    assert_eq!(m.control_overhead.cpu_ms, 3000);
    assert_eq!(m.control_overhead.peak_rss_kb, 7000);
    assert_eq!(m.control_overhead.sample_count, 4);
}

#[test]
fn tables_are_labelled_and_separate_from_the_llm_success_tables() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("raw");
    run_baseline(&spec("k2-mini", "camp-1"), &raw).unwrap();
    let out = dir.path().join("out");
    write_tables(&out, &compute(&raw).unwrap()).unwrap();
    let json: Value =
        serde_json::from_str(&fs::read_to_string(out.join(METRICS_FILE)).unwrap()).unwrap();
    assert_eq!(json["dataset"], "control_measured");
    assert_eq!(json["schema"], "celeris.dagbench-metrics/1");
    // 成功率・費用・token の欄は持たない（stub の成功を能力として混ぜない）。
    let text = json.to_string();
    for bad in [
        "success",
        "pass_rate",
        "tokens",
        "cost",
        "trial",
        "llm_campaign",
    ] {
        assert!(!text.contains(bad), "unexpected {bad} in {text}");
    }
    // 別 file: LLM 側の summary.json / final accounting とは名前も schema も違う。
    assert_eq!(METRICS_FILE, "control_measured_metrics.json");
    assert!(!out.join(celeris_campaign::record::SUMMARY_FILE).exists());
    let tsv = fs::read_to_string(out.join(METRICS_TSV)).unwrap();
    assert!(
        tsv.lines()
            .nth(1)
            .unwrap()
            .starts_with("control_measured\t")
    );
    // dataset が違う raw は読まない。
    let mut hdr: Value =
        serde_json::from_str(&fs::read_to_string(raw.join("run.json")).unwrap()).unwrap();
    hdr["dataset"] = json!("llm_campaign");
    fs::write(raw.join("run.json"), hdr.to_string()).unwrap();
    assert!(compute(&raw).unwrap_err().contains("control_measured"));
}

#[test]
fn proc_sampling_only_reads_and_cli_regenerates() {
    // 自分自身の /proc を 2 回読むだけ（負荷を掛けない）。
    let a = read_proc_sample(std::process::id(), "control", 0, 1).unwrap();
    assert!(a.vm_rss_kb > 0 && a.vm_hwm_kb >= a.vm_rss_kb / 2 && a.clk_tck == 100);
    // CLI: run → metrics。
    let dir = tempfile::tempdir().unwrap();
    let (raw, out) = (dir.path().join("raw"), dir.path().join("out"));
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    celeris_campaign::dagbench::run::cli(&args(&[
        "dagbench",
        "run",
        "--scenario",
        "k2-mini",
        "--point",
        "camp-1",
        "--arm",
        "baseline",
        "--out",
        raw.to_str().unwrap(),
    ]))
    .unwrap();
    let a = celeris_campaign::dagbench::run::cli(&args(&[
        "dagbench",
        "metrics",
        "--raw",
        raw.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]))
    .unwrap();
    let first = fs::read(out.join(METRICS_FILE)).unwrap();
    let b = celeris_campaign::dagbench::run::cli(&args(&[
        "dagbench",
        "metrics",
        "--raw",
        raw.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]))
    .unwrap();
    assert_eq!(a, b);
    assert_eq!(first, fs::read(out.join(METRICS_FILE)).unwrap());
}

#[test]
fn sweep_lists_the_fixed_points_without_running_them() {
    let file = load_scenarios(&scenarios()).unwrap();
    let items = sweep_items(&file, &file_seeds());
    // 2 scenario × 7 点 × 3 seed × 2 系。総当たり（3×3×3）ではない。
    assert_eq!(items.len(), 2 * 7 * 3 * 2);
    assert!(
        items
            .iter()
            .any(|i| i.measurement_id == "dagbench-1-k2-mini-center-20261019-baseline")
    );
    let ids: std::collections::BTreeSet<_> =
        items.iter().map(|i| i.measurement_id.clone()).collect();
    assert_eq!(ids.len(), items.len());
}

fn file_seeds() -> Vec<u64> {
    vec![20261019, 1, 2]
}
