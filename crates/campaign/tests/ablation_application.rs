//! 専門分担アブレーションの適用検証（実 HTTP → 実 dispatcher → 実 worker adapter → 非 LLM スタブ process）。
//!
//! fixture は `ablation_application_support`（一時 DB・token・org・workspace・環境・loopback の実 `celeris` daemon と
//! スタブ process を所有し、`Drop` で回収する）。実 LLM・本番・userns は使わない。同期は event 待ち＋期限。
//! fixture の smoke 試験と、専門分担 spec-off / spec-on の適用シナリオ（通常 adoption の子・孫・retry）を置く
//! （負例・非対象同一性・canary は後続の葉が足す）。

mod ablation_application_support;

use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpStream;
use std::time::Duration;

use ablation_application_support::{
    DEADLINE, Fixture, FixtureSpec, Launch, StubFail, StubPlan, StubSpec, events_of, pid_alive,
    read_launches,
};
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

// ---------------------------------------------------------------------------------------------
// 適用シナリオ: 専門分担 spec-off / spec-on（WU scenario）
// ---------------------------------------------------------------------------------------------

/// B0 の org 資料（spec-off / spec-on の両方がここから作る）。根 cos・coding 課・専門課。
/// coding 課の実効 profile は off / on で同値（根と coding の block が同じ）。専門課は coding 課と同じ
/// harness 許可に skill `spec-skill` を 1 つ足しただけの profile なので、on 側で課が分かれる理由は
/// task の skill と課の skill の重なり（既存 matching の決定的な規則）だけになる。
const B0_COS: &str = r#"
[[org]]
id = "cos"
name = "秘書"
kind = "secretary"
"#;
const B0_CODING: &str = r#"
[[org]]
id = "coding"
name = "開発課"
kind = "section"
parent_id = "cos"
genre = "coding"

[org.profile.harnesses]
allowed = ["coding"]
"#;
const B0_SPEC: &str = r#"
[[org]]
id = "coding-spec"
name = "専門課"
kind = "section"
parent_id = "cos"
genre = "coding"

[org.profile]
skills = ["spec-skill"]

[org.profile.harnesses]
allowed = ["coding"]
"#;

/// 非対象の設定（off / on で同一）: gate は on（root は人の明示 `execution: compound` で planner へ）、木は有効。
const NON_TARGET_CONFIG: &str = r#"
[execution]
gate = "on"

[execution.tree]
enabled = true
"#;

const ROOT_TITLE: &str = "Ablation application root";
const CHILD_GEN: &str = "Child general part";
const CHILD_SPEC: &str = "Child specialist part";
const GRAND_SPEC: &str = "Grandchild specialist part";

fn artifact_criterion() -> Value {
    json!({"text": "result.md exists", "check": {"type": "artifact_exists", "name": "result.md"}})
}

fn task_unit(key: &str, title: &str, skill: &str, gate: &str) -> Value {
    json!({
        "key": key,
        "stage": "s1",
        "kind": "task",
        "skills": [skill],
        "gate": gate,
        "title": title,
        "objective": format!("Deliver the {key} part as its own reviewed task with its own result"),
        "depends_on": [],
        "acceptance": [artifact_criterion()],
    })
}

fn plan(units: Vec<Value>) -> Value {
    json!({
        "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
        "rationale": "each part is owned by the section whose skills match it",
        "stages": [{"key": "s1", "kind": "implement", "title": "Stage s1"}],
        "units": units,
    })
}

/// スタブの決定的な振る舞い（off / on 同一）: root の planner run は子 2 件（一般・専門）の計画、
/// 専門の子（compound）の planner run は孫 1 件の計画を書く。孫は最初の 1 回だけ `retryable` で失敗する。
fn scenario_stub() -> StubSpec {
    StubSpec {
        plans: vec![
            StubPlan {
                title: ROOT_TITLE.into(),
                plan: plan(vec![
                    task_unit("gen", CHILD_GEN, "gen-skill", "atomic"),
                    task_unit("spec", CHILD_SPEC, "spec-skill", "compound"),
                ]),
            },
            StubPlan {
                title: CHILD_SPEC.into(),
                plan: plan(vec![task_unit("grand", GRAND_SPEC, "spec-skill", "atomic")]),
            },
        ],
        fail: vec![StubFail {
            title: GRAND_SPEC.into(),
            times: 1,
        }],
    }
}

fn scenario_spec(spec_on: bool) -> FixtureSpec {
    let mut org = format!("{B0_COS}{B0_CODING}");
    if spec_on {
        org.push_str(B0_SPEC);
    }
    FixtureSpec {
        label: if spec_on { "spec-on" } else { "spec-off" }.into(),
        org_toml: org,
        extra_config: NON_TARGET_CONFIG.into(),
        stub: scenario_stub(),
    }
}

/// 1 環境（所有 daemon・DB）の観測結果。全部 daemon の events / runs / GET とスタブの起動記録から読んだもの。
struct Observed {
    /// title → (task id, `assigned` event の node 列)。木の全 task（root・子・孫）。
    assigned: BTreeMap<String, (String, Vec<String>)>,
    /// GET `/org` から組んだ coding 課の実効 profile hash（同一 B0 資料の確認）。
    coding_profile_sha256: String,
    /// daemon の instance id（GET `/health`）。off / on が別 daemon であることの確認。
    instance_id: String,
    /// fixture の一時 dir（off / on が別 DB であることの確認）。
    db: std::path::PathBuf,
}

fn variant_identity(spec_on: bool, org_pin: &Value) -> Value {
    json!({
        "campaign_id": "ablation-application",
        "case_id": "case-a",
        "arm": "B",
        "repetition": 1,
        "manifest_hash": "m1",
        "routing_setting": "fixed",
        "variant": {
            "comparison_id": "M1",
            "id": if spec_on { "M1-spec-on" } else { "M1-spec-off" },
            "content_sha256": if spec_on { "a1" } else { "b2" }.repeat(32),
            "selection_sha256": celeris_campaign::variant::selection_sha256(),
        },
        "variant_org": org_pin,
    })
}

/// root から GET `/tasks/{id}` の `children` で木をたどる（root が先頭）。
fn walk_tree(fx: &Fixture, root: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut queue = vec![root.to_string()];
    while !queue.is_empty() {
        let id = queue.remove(0);
        let d = fx.http.get(&format!("/api/v1/tasks/{id}"));
        assert_eq!(d.status, 200, "{}", d.body);
        for c in d.body["children"].as_array().into_iter().flatten() {
            queue.push(c["id"].as_str().expect("child id").to_string());
        }
        out.push(d.body);
    }
    out
}

/// 1 環境を所有して走らせる: 通常 POST → 実 dispatcher → 実 fake adapter → スタブ process。計画の通常採用で
/// 子・孫が生成され、孫の決定的失敗で retry が起き、retry 後の run が done になるまで出来事で待ってから検査する。
fn run_scenario(spec_on: bool) -> Observed {
    let mut fx = Fixture::start(scenario_spec(spec_on));
    let health = fx.http.get("/api/v1/health");
    let instance_id = health.body["instance_id"]
        .as_str()
        .expect("instance id")
        .to_string();
    let config = fx.http.get("/api/v1/config");
    assert_eq!(config.status, 200, "{}", config.body);
    let config_sha = config.body["identity"]["config_sha256"]
        .as_str()
        .expect("config sha256")
        .to_string();
    // 投入する variant の構成 pin は、この daemon の GET `/org` から既存の共有関数で組む。
    let org = fx.http.get("/api/v1/org");
    assert_eq!(org.status, 200, "{}", org.body);
    let snap =
        celeris_campaign::variant_pin::OrgSnapshot::from_get_org(&org.body).expect("org snapshot");
    let want_nodes: Vec<String> = if spec_on {
        vec!["coding".into(), "coding-spec".into(), "cos".into()]
    } else {
        vec!["coding".into(), "cos".into()]
    };
    assert_eq!(snap.nodes(), want_nodes, "seeded org");
    let pin = snap.org_pin("coding").expect("org pin");
    let org_pin = serde_json::to_value(task_core::variant_wire::CampaignOrgPin::from(&pin))
        .expect("org pin json");
    let campaign = variant_identity(spec_on, &org_pin);

    // 通常の POST（人の明示 compound → root の planner run）。投入はこの 1 件だけ。
    let r = fx.post_task(&json!({
        "title": ROOT_TITLE,
        "objective": "Split the application work by section and deliver each part.",
        "genre": "coding",
        "execution": "compound",
        "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
        "campaign": campaign,
        "campaign_idempotency_key": "campaign-0123456789abcdef",
    }));
    assert_eq!(r.status, 201, "{}", r.body);
    let root = r.body["id"].as_str().expect("root id").to_string();

    // 出来事待ち＋期限: 一般の子と、retry 後の孫の run がスタブで done になり、その task の worker_finished が
    // done で届く（孫は error → done の 2 回）。
    let dir = fx.dir().to_path_buf();
    fx.wait_events("child and retried grandchild finished", DEADLINE, |ev| {
        let launches = read_launches(&dir);
        [CHILD_GEN, GRAND_SPEC].iter().all(|title| {
            launches.iter().any(|l| {
                l.title == *title
                    && l.outcome.as_deref() == Some("done")
                    && task_events(ev, &l.request_task_id, "worker_finished")
                        .iter()
                        .any(|e| {
                            e["event"]["outcome"]
                                .as_str()
                                .is_some_and(|o| o.starts_with("done: ablation stub"))
                        })
            })
        })
    });
    fx.assert_only_stub_launches();
    let events = fx.refresh_events().to_vec();
    let tree = walk_tree(&fx, &root);
    let by_title: BTreeMap<String, &Value> = tree
        .iter()
        .map(|d| {
            (
                d["task"]["title"].as_str().unwrap_or_default().to_string(),
                d,
            )
        })
        .collect();
    let id_of = |title: &str| -> String {
        by_title
            .get(title)
            .unwrap_or_else(|| panic!("{title} not in the tree: {tree:?}"))["task"]["id"]
            .as_str()
            .expect("id")
            .to_string()
    };
    assert_eq!(tree.len(), 4, "root + 2 children + 1 grandchild: {tree:?}");
    let (gen_id, spec_id, grand_id) = (id_of(CHILD_GEN), id_of(CHILD_SPEC), id_of(GRAND_SPEC));

    // --- 子・孫は通常 adoption から生成（POST は root の 1 件だけ）。
    let created = events_of(&events, "created");
    assert_eq!(created.len(), 4, "{created:?}");
    for e in &created {
        let origin = &e["event"]["origin"];
        if e["task_id"] == root.as_str() {
            assert!(origin.is_null(), "root is the POSTed task: {e}");
        } else {
            assert_eq!(origin, "plan_unit", "descendant not from a plan unit: {e}");
        }
    }
    for (parent, child, depth, unit) in [
        (&root, &gen_id, 2, "gen"),
        (&root, &spec_id, 2, "spec"),
        (&spec_id, &grand_id, 3, "grand"),
    ] {
        let planned = task_events(&events, parent, "execution_planned");
        assert!(
            planned.iter().any(|e| e["event"]["origin"] == "planner"),
            "{parent} has no planner-adopted plan: {planned:?}"
        );
        let made = task_events(&events, parent, "child_task_created");
        assert!(
            made.iter()
                .any(|e| e["event"]["child_task_id"] == child.as_str()
                    && e["event"]["depth"] == depth
                    && e["event"]["unit_key"] == unit),
            "{child} was not created by adopting {parent}'s plan: {made:?}"
        );
        let d = by_title
            .values()
            .find(|d| d["task"]["id"] == child.as_str())
            .expect("child detail");
        assert_eq!(d["task"]["parent_id"], parent.as_str(), "{d}");
    }
    // 計画は planner run（スタブ）が書いた: root と専門の子の planner 起動がある。
    let launch_runs = fx.launch_runs();
    for (task, title) in [(&root, ROOT_TITLE), (&spec_id, CHILD_SPEC)] {
        assert!(
            launch_runs
                .iter()
                .any(|(l, _)| l.planner && l.request_task_id == *task && l.title == title),
            "no planner stub run for {title}"
        );
    }

    // --- retry は孫の決定的失敗から実際に起きた: error の run → ready へ戻る → 同じ task の次の run が done。
    let grand_runs: Vec<&(Launch, String)> = launch_runs
        .iter()
        .filter(|(l, _)| l.request_task_id == grand_id)
        .collect();
    assert_eq!(grand_runs.len(), 2, "{grand_runs:?}");
    assert_eq!(grand_runs[0].0.outcome.as_deref(), Some("error"));
    assert_eq!(grand_runs[1].0.outcome.as_deref(), Some("done"));
    assert_ne!(grand_runs[0].1, grand_runs[1].1, "retry is a new run");
    let finished = task_events(&events, &grand_id, "worker_finished");
    let outcome_of = |run: &str| -> String {
        finished
            .iter()
            .find(|e| e["event"]["run_id"] == run)
            .and_then(|e| e["event"]["outcome"].as_str())
            .unwrap_or_else(|| panic!("no worker_finished for {run}: {finished:?}"))
            .to_string()
    };
    assert!(
        outcome_of(&grand_runs[0].1).starts_with("error(retryable=true): ablation stub"),
        "{finished:?}"
    );
    assert!(outcome_of(&grand_runs[1].1).starts_with("done: ablation stub"));
    assert!(
        task_events(&events, &grand_id, "transitioned")
            .iter()
            .any(|e| e["event"]["reason"] == "worker_error"
                && e["event"]["from"] == "running"
                && e["event"]["to"] == "ready"),
        "no retry transition for the grandchild"
    );
    // retry の assignee / hash 維持: `assigned` は 1 回だけ（再 matching しない）、retry 後も同じ assignee、
    // 2 つの run が受け取った campaign pin（variant hash・org pin・観測値・全 node digest）は同一。
    let grand_assigned: Vec<String> = task_events(&events, &grand_id, "assigned")
        .iter()
        .map(|e| e["event"]["node"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(grand_assigned.len(), 1, "{grand_assigned:?}");
    assert_eq!(
        by_title[GRAND_SPEC]["task"]["assignee"],
        grand_assigned[0].as_str(),
        "assignee after retry"
    );
    assert_eq!(
        grand_runs[0].0.campaign, grand_runs[1].0.campaign,
        "campaign pin changed across the retry"
    );
    // 全 run の pin は投入した識別と、この daemon の観測値（GET `/health`・`/config`）に一致する。
    for (l, run) in &launch_runs {
        let c = &l.campaign;
        assert_eq!(c["identity"]["variant"], campaign["variant"], "run {run}");
        assert_eq!(c["identity"]["variant_org"], org_pin, "run {run}");
        assert_eq!(c["daemon_instance_id"], instance_id.as_str(), "run {run}");
        assert_eq!(c["config_sha256"], config_sha.as_str(), "run {run}");
        assert_eq!(
            c["org_nodes_sha256"],
            snap.digest().expect("digest").as_str()
        );
        assert_eq!(c["selected"]["harness_id"], "stub", "run {run}");
    }

    // --- 実 worker の PID / 終了と run id / event の対応。
    for (l, run) in &launch_runs {
        assert_eq!(l.ppid, fx.daemon_pid(), "stub parent is the daemon: {l:?}");
        assert!(!pid_alive(l.pid), "stub pid {} still alive", l.pid);
        let fin = task_events(&events, &l.request_task_id, "worker_finished");
        assert!(
            fin.iter().any(|e| e["event"]["run_id"] == run.as_str()),
            "no worker_finished for run {run} (pid {})",
            l.pid
        );
        let runs = fx
            .http
            .get(&format!("/api/v1/tasks/{}/runs", l.request_task_id));
        assert_eq!(runs.status, 200, "{}", runs.body);
        assert!(
            runs.body.to_string().contains(run.as_str()),
            "run {run} not listed for {}",
            l.request_task_id
        );
    }
    let mut pids: Vec<u32> = launch_runs.iter().map(|(l, _)| l.pid).collect();
    pids.sort_unstable();
    pids.dedup();
    assert_eq!(pids.len(), launch_runs.len(), "one process per run");

    // --- 全子孫の Assigned（event 由来。手書きなし）。
    let mut assigned = BTreeMap::new();
    for d in &tree {
        let id = d["task"]["id"].as_str().expect("id").to_string();
        let nodes: Vec<String> = task_events(&events, &id, "assigned")
            .iter()
            .map(|e| e["event"]["node"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(nodes.len(), 1, "{id}: {nodes:?}");
        assert_eq!(d["task"]["assignee"], nodes[0].as_str(), "{id}");
        assigned.insert(
            d["task"]["title"].as_str().unwrap_or_default().to_string(),
            (id, nodes),
        );
    }
    // event 全体でも、木の外の task への割当は無い（この DB の task は木だけ）。
    for e in events_of(&events, "assigned") {
        assert!(
            assigned.values().any(|(id, _)| e["task_id"] == id.as_str()),
            "assignment outside the tree: {e}"
        );
    }
    Observed {
        assigned,
        coding_profile_sha256: pin.effective_profile_sha256.clone(),
        instance_id,
        db: fx.dir().join("celeris.sqlite3"),
    }
}

/// 専門分担 spec-off / spec-on を別所有の daemon・DB（同一 B0 資料・同一非対象設定・同一スタブ）で走らせ、
/// events / runs を読んで比べる。off: 全子孫の Assigned が指定単一 node（coding）で他 node への割当 0、
/// retry で assignee / hash 維持。on: 子孫が複数課に割り当たり off と差がある（専門の子・孫だけ専門課）。
#[test]
fn spec_off_and_on_apply_through_real_dispatcher_and_worker() {
    let off = run_scenario(false);
    let on = run_scenario(true);

    // 別所有の daemon・DB。B0 の coding 課は同値 profile。
    assert_ne!(off.instance_id, on.instance_id, "separate daemons");
    assert_ne!(off.db, on.db, "separate DBs");
    assert_eq!(off.coding_profile_sha256, on.coding_profile_sha256);
    assert_eq!(
        off.assigned.keys().collect::<Vec<_>>(),
        on.assigned.keys().collect::<Vec<_>>(),
        "same tree shape"
    );

    // off: 全子孫（root・子・孫）の Assigned が指定単一 node、他 node 割当 0。
    for (title, (_, nodes)) in &off.assigned {
        assert_eq!(nodes, &vec!["coding".to_string()], "spec-off {title}");
    }

    // on: 複数課に割り当たる。課差は skill の重なり（同値 profile の決定的 fixture）だけから来る。
    let on_node = |t: &str| on.assigned[t].1[0].clone();
    assert_eq!(on_node(ROOT_TITLE), "coding");
    assert_eq!(on_node(CHILD_GEN), "coding");
    assert_eq!(on_node(CHILD_SPEC), "coding-spec");
    assert_eq!(on_node(GRAND_SPEC), "coding-spec");
    let on_nodes: BTreeSet<String> = on.assigned.values().map(|(_, n)| n[0].clone()).collect();
    assert_eq!(
        on_nodes,
        BTreeSet::from(["coding".to_string(), "coding-spec".to_string()])
    );
    // off と on の差: 専門の子・孫の割当先だけが違う。
    let differ: Vec<&String> = off
        .assigned
        .iter()
        .filter(|(t, (_, n))| on.assigned[*t].1 != *n)
        .map(|(t, _)| t)
        .collect();
    assert_eq!(differ, vec![CHILD_SPEC, GRAND_SPEC]);
}
