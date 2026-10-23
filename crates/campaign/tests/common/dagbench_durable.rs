//! Formal A2: separate external process, durable state observation and owned controller kill.
use super::*;
use celeris_campaign::dagbench::{ipc::IpcExt, process::*};
use std::process::Command;
#[path = "dagbench_durable_backend.rs"]
mod backend;
const ENV: &str = "DAGBENCH_DURABLE_CONFIG";
const SCOPE: &str = "controller_process_only; external=independent_process; clock=logical";

pub fn header(p: &Params, s: &RunSpec) -> RunHeader {
    let mut h = RunHeader::from_spec(s, &p.arm, SCOPE);
    h.schema = celeris_campaign::dagbench::metrics::DURABLE_RAW_SCHEMA.into();
    h
}
fn limits(s: &RunSpec) -> Limits {
    Limits {
        wall: Duration::from_secs(s.caps.max_wall_secs),
        logical_secs: s.caps.max_logical_secs,
        max_restarts: 1,
        ipc_io_timeout: Duration::from_secs(60),
        ipc_retries: 1,
        stop_grace: Duration::from_secs(2),
        kill_wait: Duration::from_secs(20),
    }
}
fn command(role: &str, cfg: &Value) -> Command {
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args(["--exact", "durable::child", "--nocapture"])
        .env(ENV, cfg.to_string())
        .env(ROLE_ENV, role);
    c
}
fn line(
    p: &OwnedProc,
    marker: &str,
    timeout: Duration,
) -> Result<String, Box<dyn std::error::Error>> {
    let until = std::time::Instant::now() + timeout;
    loop {
        let l = p.next_line(until.saturating_duration_since(std::time::Instant::now()))?;
        if let Some(i) = l.find(marker) {
            return Ok(l[i + marker.len()..].into());
        }
    }
}

#[test]
fn child() {
    let Ok(cfg) = std::env::var(ENV) else { return };
    let cfg: Value = serde_json::from_str(&cfg).unwrap();
    if std::env::var(ROLE_ENV).unwrap() == "external" {
        run_external_role_from_env().unwrap();
        return;
    }
    let p: Params = serde_json::from_value(cfg["params"].clone()).unwrap();
    let s = p.spec().unwrap();
    let inc = cfg["inc"].as_u64().unwrap();
    let mut ext = backend::DurableBackend {
        inner: IpcExt::connect_unix(
            Path::new(cfg["socket"].as_str().unwrap()),
            &format!("control-{inc}"),
            limits(&s).ipc_options(),
        ),
        params: p.clone(),
        nth: if inc == 1 {
            cfg["nth"].as_u64().map(|v| v as usize)
        } else {
            None
        },
        seen: Default::default(),
        selected: None,
    };
    let result = if p.arm == "baseline" {
        let mut b = Baseline::open(
            load_scenario(&scenarios(), &p.scenario).unwrap(),
            BaselineConfig {
                poll_secs: s.poll_secs,
                ext_job_secs: s.ext_job_secs,
            },
            &p.out.join("state.json"),
        )
        .unwrap();
        b.set_progress_log(&p.out.join("worker.jsonl")).unwrap();
        b.set_logical_cap(s.caps.max_logical_secs);
        if let Some(g) = &s.grading {
            g.attach_baseline(&mut b);
        }
        b.run(&mut ext)
            .map(|r| r == Outcome::Complete)
            .map_err(|e| format!("{e:?}"))
    } else {
        let paths = Paths::under(&p.out.join("celeris.db"), &p.out);
        let store: Arc<dyn TaskStore> = Arc::new(task_core::SqliteStore::open(&paths.db).unwrap());
        let transport = RouterTransport(router_at(&p.out));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            CelerisSide::with_backend(s.clone(), paths, store, Box::new(ext))?
                .drive(&transport)
                .await
                .map(|r| r.campaigns.iter().all(|c| c.status == "done"))
        })
    };
    fs::write(
        p.out.join("controller-outcome.json"),
        serde_json::to_vec(&json!({"completed": result == Ok(true), "result": result})).unwrap(),
    )
    .unwrap();
}

pub fn run(p: &Params, s: &RunSpec) -> Result<(), String> {
    run_inner(p, s).map_err(|e| e.to_string())
}
fn run_inner(p: &Params, s: &RunSpec) -> Result<(), Box<dyn std::error::Error>> {
    if s.point.concurrent_campaigns != 1 {
        return Err("A2 requires camp-1".into());
    }
    let count = s
        .scenario
        .nodes
        .iter()
        .filter(|n| n.external.is_some())
        .count();
    let nth = 1 + s.seed % count as u64;
    let lim = limits(s);
    let started = std::time::Instant::now();
    let log = EventLog::create(&p.out)?;
    let mut sup = Supervisor::new(log.clone(), lim.clone())?;
    // Only a short-lived Unix socket goes under /tmp, never raw or build output.
    let socket_dir = tempfile::Builder::new().prefix("a2-").tempdir_in("/tmp")?;
    let socket = socket_dir.path().join("ext.sock");
    let ledger = p.out.join("ext.jsonl");
    let cfg = json!({"params": p, "socket": socket, "nth": nth, "inc": 1});
    let mut ext_cmd = command("external", &cfg);
    ext_cmd
        .env(SOCKET_ENV, &socket)
        .env(LEDGER_ENV, &ledger)
        .env(
            EXT_CFG_ENV,
            serde_json::to_string(&ExtConfig {
                poll_secs: s.poll_secs,
                job_slots: s.scenario.resources.external_job_slots,
                faults: vec![],
            })?,
        );
    let mut external = sup.spawn(Role::External, ext_cmd)?;
    line(&external, READY_LINE, lim.wall)?;
    let mut c1 = sup.spawn(Role::Control, command("control", &cfg))?;
    append_sample(&p.out, c1.pid(), 0)?;
    let candidate: Value = serde_json::from_str(&line(&c1, "DURABLE ", lim.wall)?)?;
    let physical = candidate["physical_id"].as_str().ok_or("no physical ID")?;
    let logical = candidate["logical_job_id"]
        .as_str()
        .ok_or("no logical ID")?;
    let snapshot = backend::saved(p, physical)?.ok_or("parent could not confirm durable wait")?;
    fs::write(
        p.out.join("durable-wait.json"),
        serde_json::to_vec_pretty(&snapshot)?,
    )?;
    if let Some(g) = &s.grading {
        if p.out
            .join(format!("artifacts/c0/{}.attempts", g.node))
            .exists()
        {
            return Err("graded downstream ran before external recovery".into());
        }
    }
    let before = read_ledger(&ledger)?;
    if before.iter().any(|e| {
        e.event == "external_job_completed" && e.logical_job_id.as_deref() == Some(logical)
    }) {
        return Err("selected job completed before controller stop".into());
    }
    let now = before.last().ok_or("no external events")?.logical_secs;
    log.barrier(B_CONTROL_ACKED, json!({"external_pid": external.pid(), "external_incarnation": external.incarnation(),
        "fault_id": "a2-restart-after-wait", "trigger": "durable_wait_saved", "nth": nth,
        "logical_job_id": logical, "physical_id": physical, "saved": snapshot, "ledger_seq_before": before.last().map(|e| e.seq)}))?;
    append_sample(&p.out, c1.pid(), started.elapsed().as_millis() as u64)?;
    let rep = c1.kill("a2-durable-wait-saved")?;
    if rep.exit_signal != Some(9) {
        return Err("controller was not killed".into());
    }
    log.barrier(B_CONTROL_EXIT_ACK, json!({"pid": c1.pid()}))?;
    append_control(
        &p.out,
        "control_restart",
        now,
        Some(started.elapsed().as_millis() as u64),
        None,
    )?;
    let launched: Vec<String> = before
        .iter()
        .filter(|e| e.event == "physical_launch")
        .filter_map(|e| e.logical_job_id.clone())
        .collect();
    let mut observer = IpcExt::connect_unix(&socket, "observer", lim.ipc_options());
    let out = observe_until_complete(&mut observer, &launched, &lim, sup.wall_deadline(), &log)?;
    if external.try_exit()?.is_some() {
        return Err("external exited during outage".into());
    }
    let entries = read_ledger(&ledger)?;
    let done: Vec<_> = entries
        .iter()
        .filter(|e| e.event == "external_job_completed")
        .collect();
    log.barrier(B_LEDGER_SYNC_ACK, json!({"external_pid": external.pid(), "external_incarnation": external.incarnation(),
        "ledger_seq": out.ledger_seq, "ledger_file_seq": entries.last().map(|e| e.seq),
        "max_completed_seq": done.iter().map(|e| e.seq).max(), "completed": done.iter().filter_map(|e| e.logical_job_id.clone()).collect::<Vec<_>>(),
        "final_now": out.final_now}))?;
    log.barrier(
        B_CONTROL_RESUME,
        json!({"physical_id": physical, "logical_job_id": logical, "saved": snapshot}),
    )?;
    append_control(
        &p.out,
        "control_resumed",
        out.final_now,
        Some(started.elapsed().as_millis() as u64),
        None,
    )?;
    let mut resumed = cfg.clone();
    resumed["inc"] = json!(2);
    let mut c2 = sup.spawn(Role::Control, command("control", &resumed))?;
    append_sample(&p.out, c2.pid(), started.elapsed().as_millis() as u64)?;
    let status = c2
        .wait_exit(
            sup.wall_deadline()
                .saturating_duration_since(std::time::Instant::now()),
        )?
        .ok_or("controller timeout")?;
    if !status.success() {
        return Err(format!("controller failed: {status}").into());
    }
    log.barrier(B_CONTROL_DONE, json!({"exit_code": status.code()}))?;
    external.stop(Some(&mut || observer.shutdown_server()), "finished")?;
    verify_survival_barriers(&read_events(log.path())?)?;
    let result: Value = serde_json::from_slice(&fs::read(p.out.join("controller-outcome.json"))?)?;
    if result["completed"] != true {
        return Err(format!("controller incomplete: {result}").into());
    }
    Ok(())
}
