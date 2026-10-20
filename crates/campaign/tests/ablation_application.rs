//! 専門分担アブレーションの適用検証（実 HTTP → 実 dispatcher → 実 worker adapter → 非 LLM スタブ process）。
//!
//! fixture は `ablation_application_support`（一時 DB・token・org・workspace・環境・loopback の実 `celeris` daemon と
//! スタブ process を所有し、`Drop` で回収する）。実 LLM・本番・userns は使わない。同期は event 待ち＋期限。
//! この段では fixture 自体の smoke 試験だけを置く（spec-off/on の適用シナリオ・負例は後続の葉が足す）。

mod ablation_application_support;

use std::net::TcpStream;
use std::time::Duration;

use ablation_application_support::{DEADLINE, Fixture, FixtureSpec, events_of, pid_alive};
use serde_json::{Value, json};

fn task_events<'a>(events: &'a [Value], task: &str, ty: &str) -> Vec<&'a Value> {
    events_of(events, ty)
        .into_iter()
        .filter(|e| e["task_id"] == task)
        .collect()
}

/// smoke: 通常の POST → 実 dispatcher → 実 worker（スタブ process）の spawn と終了。スタブが記録した PID・
/// 親 PID・run id と、daemon の `worker_started` / `worker_finished` event・`GET /tasks/{id}/runs` が対応し、
/// 起動はスタブだけで、fixture の drop で daemon・スタブ・port・一時 dir が回収される。
#[test]
fn smoke_post_spawns_real_stub_worker_and_events_match_its_pid_and_run() {
    let mut fx = Fixture::start(FixtureSpec::minimal("smoke"));
    let r = fx.post_task(&json!({
        "title": "smoke",
        "objective": "fixture smoke: stub writes result.md",
        "genre": "coding",
        "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
    }));
    assert_eq!(r.status, 201, "{}", r.body);
    let task = r.body["id"].as_str().expect("task id").to_string();

    fx.wait_events("worker_finished of the smoke task", DEADLINE, |ev| {
        !task_events(ev, &task, "worker_finished").is_empty()
    });

    let launches = fx.launches();
    let mine: Vec<_> = launches.iter().filter(|l| l.task_id == task).collect();
    assert!(!mine.is_empty(), "no stub launch for {task}: {launches:?}");
    let events = fx.events().to_vec();
    let started = task_events(&events, &task, "worker_started");
    let finished = task_events(&events, &task, "worker_finished");
    for l in &mine {
        // 実 process: daemon が直接起こした子で、run の終了後には存在しない。
        assert_eq!(l.ppid, fx.daemon_pid(), "stub parent is the daemon: {l:?}");
        assert_eq!(l.request_task_id, task, "RunRequest.task.id: {l:?}");
        assert_eq!(l.outcome.as_deref(), Some("done"), "{l:?}");
        assert!(!pid_alive(l.pid), "stub pid {} still alive", l.pid);
        // event の run id と、スタブの環境に渡った CELERIS_RUN_ID が一致する。
        assert!(
            started
                .iter()
                .any(|e| e["event"]["run_id"] == l.run_id.as_str()),
            "no worker_started for run {}: {started:?}",
            l.run_id
        );
        assert!(
            finished
                .iter()
                .any(|e| e["event"]["run_id"] == l.run_id.as_str()),
            "no worker_finished for run {}: {finished:?}",
            l.run_id
        );
    }
    // 走った run の一覧（GET /runs）にもスタブの run がある。
    let runs = fx.http.get(&format!("/api/v1/tasks/{task}/runs"));
    assert_eq!(runs.status, 200, "{}", runs.body);
    let listed = runs.body.to_string();
    for l in &mine {
        assert!(
            listed.contains(&l.run_id),
            "run {} not in {listed}",
            l.run_id
        );
    }
    fx.assert_only_stub_launches();

    // 回収: daemon・スタブ process・loopback port・一時 dir（DB・token・workspace）。
    let owned = fx.owned();
    assert!(owned.dir.join("celeris.sqlite3").exists());
    drop(fx);
    assert!(!pid_alive(owned.daemon_pid), "daemon survived drop");
    for pid in &owned.stub_pids {
        assert!(!pid_alive(*pid), "stub {pid} survived drop");
    }
    assert!(
        TcpStream::connect_timeout(&owned.addr, Duration::from_secs(2)).is_err(),
        "loopback port still accepts after drop"
    );
    assert!(!owned.dir.exists(), "fixture dir survived drop");
}

/// LLM の実行先はスタブだけ: daemon の PATH にある LLM CLI の罠が起動されたら、fixture の検査が失敗にする。
#[test]
fn trapped_llm_cli_launch_fails_the_fixture_check() {
    let mut fx = Fixture::start(FixtureSpec::minimal("trap"));
    fx.assert_only_stub_launches();
    let st = std::process::Command::new(fx.dir().join("trap/claude"))
        .arg("-p")
        .status()
        .expect("run trap");
    assert_eq!(st.code(), Some(97));
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fx.assert_only_stub_launches()
    }));
    assert!(caught.is_err(), "trap launch was not detected");
}
