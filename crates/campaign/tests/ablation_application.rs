//! 専門分担アブレーションの適用検証（実 HTTP → 実 dispatcher → 実 worker adapter → 非 LLM スタブ process）。
//!
//! fixture は `ablation_application_support`（一時 DB・token・org・workspace・環境・loopback の実 `celeris` daemon と
//! スタブ process を所有し、`Drop` で回収する）。実 LLM・本番・userns は使わない。同期は event 待ち＋期限。
//! fixture の smoke 試験と、専門分担 spec-off / spec-on の適用シナリオ（通常 adoption の子・孫・retry）を置く
//! と、負例（投入・再開・冪等照会、variant の改変・欠落の拒否）・非対象の同一性・隣接 trial の canary を置く。

mod ablation_application_support;

use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpStream;
use std::time::Duration;

use ablation_application_support::{
    DEADLINE, Fixture, FixtureSpec, Http, Launch, StubFail, StubPlan, StubSpec, TRIAL_NOTES,
    events_of, files_containing, pid_alive, read_launches, read_requests,
};
use celeris_campaign::{
    celeris_exec::{CLUSTER_CANCEL_FILE, CelerisExecutor},
    env_lock::{CatalogRow, catalog_from_models},
    executor::{ErrKind, Executor, TaskRef, TrialRequest},
    lock::GraderPin,
    manifest::{ArmRouting, Budget, Cap, Config, ConfigId, Termination, TokenBasis},
    variant::{
        BaselinePin, NonTargetPin, OrgPin, REQUIRED_ROLES, VariantId, VariantRef, VariantSpec,
    },
    variant_pin::OrgSnapshot,
    variant_proof::{FixedEnv, RunProofExpectation},
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
        "idempotency_key": "campaign-0123456789abcdef",
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

// ---------------------------------------------------------------------------------------------
// 負例・非対象の同一性・canary（WU negative）
// ---------------------------------------------------------------------------------------------

/// 負例用の B0 根。review・安全上限（道具の禁止・lane・試行回数）を根に置き、coding 課・専門課はそれを継ぐ
/// （off / on で同じ block。内部 review の設定は variant で変えない）。
const NEG_COS: &str = r#"
[[org]]
id = "cos"
name = "秘書"
kind = "secretary"

[org.profile]
deny_tools = ["docker"]

[org.profile.review]
harness = "reviewer"
tier = "standard"

[org.profile.budget]
max_lane = "standard"
max_attempts = 3
"#;

const NEG_KEY1: &str = "campaign-00000000000000a1";
const NEG_KEY2: &str = "campaign-00000000000000a2";
const NEG_KEY_BAD: &str = "campaign-00000000000000b1";

/// arm B の固定（lock `execution.configs` 相当）。harness・model はスタブだけ。
fn neg_arm() -> Config {
    Config {
        id: ConfigId::B,
        harness: "stub".into(),
        harness_version: "1".into(),
        model: "stub".into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        routing: Some(ArmRouting::default()),
    }
}

/// B0 の非対象（off / on で同一の値を B0 の coding 課の実効 profile と固定値から組む）。
fn b0_non_target(coding: &task_core::profile::EffectiveProfile) -> NonTargetPin {
    let (review, safety) = NonTargetPin::profile_pins(coding).expect("B0 review / safety pins");
    NonTargetPin {
        configs: vec![neg_arm()],
        role_models: REQUIRED_ROLES
            .iter()
            .map(|r| (r.to_string(), "stub".to_string()))
            .collect(),
        review,
        safety,
        materials_sha256: "b0-materials-".repeat(4),
        kb_snapshot: "b0-kb-1".into(),
        budget: Budget {
            token_basis: TokenBasis::InputOutput,
            max_tokens: 1_000_000,
            max_wall_secs: 3600,
            max_disk_bytes: 1 << 30,
            max_parallel: 1,
            per_case: Cap {
                max_tokens: 100_000,
                max_wall_secs: 600,
            },
            per_arm: Cap {
                max_tokens: 500_000,
                max_wall_secs: 1800,
            },
        },
        termination: Termination {
            trial_timeout_secs: 600,
            max_attempts_per_trial: 2,
            stop_on_budget: true,
        },
        graders: BTreeMap::from([(
            "case-a".to_string(),
            GraderPin {
                private_sha256: "e".repeat(64),
                grade_case_sha256: "f".repeat(64),
            },
        )]),
    }
}

fn neg_executor(fx: &Fixture, v: Option<VariantRef>, org: Option<OrgPin>) -> CelerisExecutor<Http> {
    let template = json!({
        "genre": "coding",
        "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    CelerisExecutor::new(
        fx.http.clone(),
        template,
        600,
        fx.dir().join(CLUSTER_CANCEL_FILE),
    )
    .with_variant(v)
    .with_variant_org(org)
}

fn neg_request<'a>(
    cfg: &'a Config,
    prompt: &'a str,
    trial: u32,
    ws: &'a std::path::Path,
    kb: &'a std::path::Path,
    v: Option<&'a VariantRef>,
) -> TrialRequest<'a> {
    TrialRequest {
        campaign_id: "ablation-negative",
        manifest_sha256: "m1",
        config: cfg,
        problem_id: "case-a",
        prompt,
        trial_no: trial,
        rep: trial,
        workspace: ws,
        kb,
        timeout_secs: 600,
        budget: Cap {
            max_tokens: u64::MAX,
            max_wall_secs: 600,
        },
        variant: v,
    }
}

fn task_ids(fx: &Fixture) -> Vec<String> {
    let r = fx.http.get("/api/v1/tasks?archived=1&limit=200");
    assert_eq!(r.status, 200, "{}", r.body);
    let mut ids: Vec<String> = r.body["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|t| t["id"].as_str().map(String::from))
        .collect();
    ids.sort();
    ids
}

fn wait_done(fx: &mut Fixture, task: &str) {
    fx.wait_events(
        &format!("worker_finished(done) of {task}"),
        DEADLINE,
        |ev| {
            task_events(ev, task, "worker_finished").iter().any(|e| {
                e["event"]["outcome"]
                    .as_str()
                    .is_some_and(|o| o.starts_with("done: ablation stub"))
            })
        },
    );
}

/// 1 環境の観測（off / on の比較に使う）。
struct NegObserved {
    spec: VariantSpec,
    /// B0 の `GET /org`（on 環境が B0 そのもの。off はこれを比較元に使う）。
    org_body: Value,
    /// 稼働 coding 課の実効 profile から作った review / 安全上限（daemon が返した値）。
    live_review_safety: (
        celeris_campaign::variant::ReviewPin,
        celeris_campaign::variant::SafetyPin,
    ),
    /// `GET /harnesses`（builtin harness と宣言 harness の一覧）。
    harnesses: Value,
    /// env-lock の catalog（`GET /llm/models` から既存の関数で作る）。
    catalog: Vec<CatalogRow>,
    /// `GET /llm/models`（稼働 catalog。fake source なので空）。
    models: Value,
    /// path・port・identity を除いた `GET /config`。
    daemon_config: Value,
    /// 走った全 run の (role, harness, source, model)（`GET /tasks/{id}` の runs の selection）。
    selections: BTreeSet<(String, String, String, String)>,
    canary: String,
}

/// 1 環境（所有 daemon・DB）で、投入・再開・冪等照会と改変・欠落の拒否、封印照合、隣接 trial の canary を
/// 実 HTTP → 実 dispatcher → 実 worker（スタブ process）で確かめる。
fn run_negative(id: VariantId, b0_org: Option<&Value>, foreign_canaries: &[String]) -> NegObserved {
    let label = match id {
        VariantId::SpecOn => "neg-on",
        VariantId::SpecOff => "neg-off",
    };
    let mut org = format!("{NEG_COS}{B0_CODING}");
    if id == VariantId::SpecOn {
        org.push_str(B0_SPEC);
    }
    let mut fx = Fixture::start(FixtureSpec {
        label: label.into(),
        org_toml: org,
        extra_config: String::new(),
        stub: StubSpec::default(),
    });
    let org_resp = fx.http.get("/api/v1/org");
    assert_eq!(org_resp.status, 200, "{}", org_resp.body);
    let live = OrgSnapshot::from_get_org(&org_resp.body).expect("live org");
    let b0_body = b0_org.cloned().unwrap_or_else(|| org_resp.body.clone());
    let b0 = OrgSnapshot::from_get_org(&b0_body).expect("B0 org");

    // --- 非対象: B0 から基準を組み、稼働 org から作った variant spec が B0 に対して検証を通る。
    let b0_coding = b0.profile("coding").expect("B0 coding profile");
    let nt = b0_non_target(b0_coding);
    let live_coding = live.profile("coding").expect("live coding profile");
    let live_review_safety = NonTargetPin::profile_pins(live_coding).expect("live pins");
    assert_eq!(
        live_review_safety,
        (nt.review.clone(), nt.safety.clone()),
        "{label}: live review / safety differ from B0"
    );
    let baseline = BaselinePin {
        org: b0.org_pin("coding").expect("B0 org pin"),
        non_target: nt.clone(),
    };
    let spec = VariantSpec::new(id, live.org_pin("coding").expect("live org pin"), nt)
        .seal()
        .expect("seal");
    spec.verify(&baseline).expect("variant verifies against B0");
    let vref = spec.to_ref().expect("ref");

    let cfg = neg_arm();
    let trials = fx.dir().join("trials");
    let (ws1, ws2, kb) = (trials.join("t1"), trials.join("t2"), trials.join("kb"));
    for d in [&ws1, &ws2, &kb] {
        std::fs::create_dir_all(d).expect("trial dir");
    }
    let canary = format!("canary-{label}-trial1-5e1f0c");
    let prompt1 = format!("Implement part one. trial marker {canary}");
    let prompt2 = "Implement part two.".to_string();
    let req1 = neg_request(&cfg, &prompt1, 1, &ws1, &kb, Some(&vref));
    let exec = neg_executor(&fx, Some(vref.clone()), Some(spec.org.clone()));

    // --- 投入（通常 POST）・再開・冪等照会: どれも同じ task 1 件に寄る。
    let t1 = exec.submit(&req1, NEG_KEY1).expect("submit").task_id;
    assert_eq!(task_ids(&fx), vec![t1.clone()], "{label}: one task");
    let looked = exec.lookup(NEG_KEY1).expect("lookup").expect("found");
    assert_eq!(looked.task_id, t1, "lookup");
    assert_eq!(exec.resolve(&req1, NEG_KEY1).expect("resolve").task_id, t1);
    assert_eq!(exec.submit(&req1, NEG_KEY1).expect("resubmit").task_id, t1);
    // 再起動後の再開（新しい executor が同じ冪等キーで照会）も POST せず同じ task。
    let restarted = neg_executor(&fx, Some(vref.clone()), Some(spec.org.clone()));
    assert_eq!(
        restarted.resolve(&req1, NEG_KEY1).expect("resume").task_id,
        t1
    );
    let detail = fx.http.get(&format!("/api/v1/tasks/{t1}"));
    assert_eq!(
        detail.body["campaign"]["variant"],
        serde_json::to_value(task_core::variant_wire::CampaignVariantRef::from(&vref))
            .expect("variant json"),
        "{label}: GET returns the submitted variant"
    );

    // --- 通常 POST の冪等再送: 同一 hash = 200（同じ id）、content 違い = 409、pin 欠落 = 拒否（4xx）。
    let body = exec.body(&req1, NEG_KEY1).expect("body");
    let same = fx.post_task(&body);
    assert_eq!(same.status, 200, "{label}: same hash resend: {}", same.body);
    assert_eq!(same.body["id"], t1.as_str());
    let mut other_content = body.clone();
    other_content["campaign"]["variant"]["content_sha256"] = json!("c".repeat(64));
    let r = fx.post_task(&other_content);
    assert_eq!(r.status, 409, "{label}: other content: {}", r.body);
    let mut other_id = body.clone();
    other_id["campaign"]["variant"]["id"] = json!(match id {
        VariantId::SpecOn => "M1-spec-off",
        VariantId::SpecOff => "M1-spec-on",
    });
    let r = fx.post_task(&other_id);
    assert_eq!(r.status, 409, "{label}: other variant id: {}", r.body);
    for (parent, what) in [
        ("/campaign", "variant_org"),
        ("/campaign", "variant"),
        ("/campaign/variant", "selection_sha256"),
    ] {
        let mut b = body.clone();
        b.pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .expect("object")
            .remove(what);
        let r = fx.post_task(&b);
        assert!(
            (400..500).contains(&r.status),
            "{label}: {what} missing must be refused: {} {}",
            r.status,
            r.body
        );
    }
    // 冪等キーを省いた再送は、同じ試行（campaign / case / arm / repetition）の識別で既存 task に寄る（新規なし）。
    let mut no_key = body.clone();
    no_key
        .as_object_mut()
        .expect("body")
        .remove("idempotency_key");
    let r = fx.post_task(&no_key);
    assert_eq!(r.status, 200, "{label}: resend without key: {}", r.body);
    assert_eq!(r.body["id"], t1.as_str());
    // 新しい冪等キーで pin を欠いた投入も拒否され、task は作られない。
    let mut fresh = exec.body(&req1, NEG_KEY_BAD).expect("body");
    fresh["campaign"]
        .as_object_mut()
        .expect("campaign")
        .remove("variant_org");
    let r = fx.post_task(&fresh);
    assert!(
        (400..500).contains(&r.status),
        "{label}: {} {}",
        r.status,
        r.body
    );
    assert_eq!(
        task_ids(&fx),
        vec![t1.clone()],
        "{label}: rejected POSTs left tasks"
    );

    // --- executor 側の改変・欠落の拒否（POST 前・副作用なし）と、保存済み variant と違う照会の拒否。
    let mut bad_sel = vref.clone();
    bad_sel.selection_sha256 = "0".repeat(64);
    let mut bad_hash = vref.clone();
    bad_hash.content_sha256 = "abc".into();
    for bad in [bad_sel, bad_hash] {
        let x = neg_executor(&fx, Some(bad.clone()), Some(spec.org.clone()));
        let e = x
            .submit(
                &neg_request(&cfg, &prompt1, 1, &ws1, &kb, Some(&bad)),
                NEG_KEY_BAD,
            )
            .expect_err("tampered variant must be refused");
        assert_eq!(e.kind, ErrKind::Refused, "{label}: {e:?}");
    }
    let no_org = neg_executor(&fx, Some(vref.clone()), None);
    let e = no_org
        .submit(&req1, NEG_KEY_BAD)
        .expect_err("missing org pin");
    assert_eq!(e.kind, ErrKind::Refused);
    let e = exec
        .submit(
            &neg_request(&cfg, &prompt1, 1, &ws1, &kb, None),
            NEG_KEY_BAD,
        )
        .expect_err("request without the variant");
    assert_eq!(e.kind, ErrKind::Refused);
    let mut drifted = vref.clone();
    drifted.content_sha256 = "d".repeat(64);
    let x = neg_executor(&fx, Some(drifted.clone()), Some(spec.org.clone()));
    let dreq = neg_request(&cfg, &prompt1, 1, &ws1, &kb, Some(&drifted));
    assert_eq!(
        x.lookup(NEG_KEY1).expect_err("lookup").kind,
        ErrKind::Refused
    );
    assert_eq!(
        x.resolve(&dreq, NEG_KEY1).expect_err("resume").kind,
        ErrKind::Refused
    );
    assert_eq!(
        x.submit(&dreq, NEG_KEY1).expect_err("submit").kind,
        ErrKind::Refused
    );
    assert_eq!(
        task_ids(&fx),
        vec![t1.clone()],
        "{label}: refusals left tasks"
    );

    // --- 受理した 1 件だけが実 worker（スタブ）で走る。拒否した要求からは run が起きない。
    wait_done(&mut fx, &t1);
    let events = fx.refresh_events().to_vec();
    for e in events_of(&events, "worker_started") {
        assert_eq!(e["task_id"], t1.as_str(), "{label}: unexpected run {e}");
    }
    assert!(
        fx.launches().iter().all(|l| l.request_task_id == t1),
        "{label}: stub launched for a refused request"
    );

    // --- 既存の封印照合（`CelerisExecutor::prove_variant` → `variant_proof::prove_runs`）を実 daemon の GET に。
    let config = fx.http.get("/api/v1/config");
    assert_eq!(config.status, 200, "{}", config.body);
    let health = fx.http.get("/api/v1/health");
    let env = FixedEnv {
        instance_id: health.body["instance_id"]
            .as_str()
            .expect("instance")
            .into(),
        config_sha256: config.body["identity"]["config_sha256"]
            .as_str()
            .expect("config sha")
            .into(),
    };
    // env-lock の catalog: fake source は model discovery を持たないので、稼働 daemon の `GET /llm/models` は
    // 空（`catalog_from_models` は空 catalog を拒否する）。封印側の catalog は B0 の固定値（stub × stub）とし、
    // 稼働側は off / on で `GET /llm/models` が同一であることを比べる。
    let models = fx.http.get("/api/v1/llm/models");
    assert_eq!(models.status, 200, "{}", models.body);
    assert!(
        catalog_from_models(&models.body).is_err(),
        "{label}: unexpected discovered models: {}",
        models.body
    );
    let catalog = vec![CatalogRow {
        model_id: "stub".into(),
        tier: "unset".into(),
        source_id: "stub".into(),
    }];
    let t1_ref = TaskRef {
        task_id: t1.clone(),
        ..TaskRef::default()
    };
    let exp = RunProofExpectation {
        variant: &vref,
        spec: &spec,
        b0_org: &b0,
        arm: &cfg,
        catalog: &catalog,
        env: Some(&env),
    };
    exec.prove_variant(&t1_ref, &exp)
        .unwrap_or_else(|e| panic!("{label}: sealed proof: {e:?}"));
    // 負: 別 daemon の固定・別の封印参照・catalog 外は、同じ実観測でも Refused。
    let other_env = FixedEnv {
        instance_id: "another-daemon".into(),
        ..env.clone()
    };
    let other_catalog = vec![CatalogRow {
        model_id: "other-model".into(),
        tier: "unset".into(),
        source_id: "stub".into(),
    }];
    for (what, exp) in [
        (
            "env",
            RunProofExpectation {
                env: Some(&other_env),
                ..exp
            },
        ),
        (
            "variant",
            RunProofExpectation {
                variant: &drifted,
                ..exp
            },
        ),
        (
            "catalog",
            RunProofExpectation {
                catalog: &other_catalog,
                ..exp
            },
        ),
        ("unfixed env", RunProofExpectation { env: None, ..exp }),
    ] {
        let e = exec
            .prove_variant(&t1_ref, &exp)
            .expect_err(&format!("{label}: {what} must be refused"));
        assert_eq!(e.kind, ErrKind::Refused, "{label}: {what}");
    }

    // --- 隣接 trial の canary: trial 1 は objective に canary を持ち、スタブが artifacts・作業 dir に痕跡を残す。
    // 次の trial（別冪等キー・別作業 dir、同じ daemon）の入力・作業場所・成果物に canary が流れ込まない。
    let req2 = neg_request(&cfg, &prompt2, 2, &ws2, &kb, Some(&vref));
    let t2 = exec
        .submit(&req2, NEG_KEY2)
        .expect("submit trial 2")
        .task_id;
    assert_ne!(t2, t1);
    wait_done(&mut fx, &t2);
    let requests = read_requests(fx.dir());
    let of =
        |t: &str| -> Vec<&Value> { requests.iter().filter(|r| r["task"]["id"] == t).collect() };
    let (r1, r2) = (of(&t1), of(&t2));
    assert!(!r1.is_empty() && !r2.is_empty(), "{label}: {requests:?}");
    // 陽性対照: trial 1 自身には canary が届き、痕跡も残っている（検出器が働く）。
    assert!(r1.iter().all(|r| r.to_string().contains(&canary)));
    let r1_artifacts = r1[0]["artifacts_dir"].as_str().expect("artifacts");
    assert!(
        !files_containing(std::path::Path::new(r1_artifacts), &canary).is_empty(),
        "{label}: trial 1 left no {TRIAL_NOTES}"
    );
    for r in &r2 {
        assert!(
            !r.to_string().contains(&canary),
            "{label}: trial 2 RunRequest carries the trial 1 canary: {r}"
        );
        for key in ["workspace", "work_dir", "artifacts_dir"] {
            if let Some(p) = r[key].as_str() {
                let hit = files_containing(std::path::Path::new(p), &canary);
                assert!(
                    hit.is_empty(),
                    "{label}: trial 2 {key} has the canary: {hit:?}"
                );
            }
        }
        for f in foreign_canaries {
            assert!(
                !r.to_string().contains(f.as_str()),
                "{label}: foreign canary {f}"
            );
        }
    }
    // 別所有の環境（隣の arm）の canary は、この daemon の DB・ログ・作業場所のどこにも無い。
    for f in foreign_canaries {
        let hit = files_containing(fx.dir(), f);
        assert!(hit.is_empty(), "{label}: foreign canary {f} in {hit:?}");
    }
    fx.assert_only_stub_launches();

    // --- 走った全 run の役割と selection（harness・source・model）。
    let mut selections = BTreeSet::new();
    for t in [&t1, &t2] {
        let d = fx.http.get(&format!("/api/v1/tasks/{t}"));
        assert_eq!(d.status, 200, "{}", d.body);
        for run in d.body["runs"].as_array().into_iter().flatten() {
            let s = &run["selection"];
            let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
            selections.insert((
                text(&run["role"]),
                text(&s["harness_id"]),
                text(&s["source_id"]),
                text(&s["model_id"]),
            ));
        }
    }
    // 稼働 config（builtin 役割・genre・内部 reviewer・委譲・再試行の上限）。所有物ごとに違う path・port・
    // identity だけを除いて比べる。
    let mut daemon_config = config.body.clone();
    for k in ["api", "config_path", "db", "identity", "workspace_root"] {
        daemon_config.as_object_mut().expect("config").remove(k);
    }
    let harnesses = fx.http.get("/api/v1/harnesses");
    assert_eq!(harnesses.status, 200, "{}", harnesses.body);
    NegObserved {
        spec,
        org_body: org_resp.body,
        live_review_safety,
        harnesses: harnesses.body,
        catalog,
        models: models.body,
        daemon_config,
        selections,
        canary,
    }
}

/// 負例・非対象の同一性・canary（off / on を別所有の daemon・DB で）。投入・再開・冪等照会は同じ task に寄り、
/// 同一 hash の再送は 200、content 違いは 409、pin 欠落は拒否で run も task も作らない。off / on の非対象
/// （builtin を含む harness 一覧・全役割のモデル・harness・資料・KB・予算・安全上限・外部 grader・env-lock の
/// catalog・内部 review）は同一で、実 run の selection も同じ。隣の trial・隣の環境の canary は流れ込まない。
#[test]
fn negative_paths_non_target_identity_and_canary_through_real_daemon() {
    let on = run_negative(VariantId::SpecOn, None, &[]);
    let off = run_negative(
        VariantId::SpecOff,
        Some(&on.org_body),
        std::slice::from_ref(&on.canary),
    );

    // variant は違う（封印 hash・org の node 集合）。
    assert_ne!(on.spec.sha256, off.spec.sha256);
    assert_ne!(on.spec.org.nodes, off.spec.org.nodes);
    // 非対象は同一: 全欄の diff が空、coding 課の実効 profile も同値。
    assert!(
        off.spec.non_target.diff(&on.spec.non_target).is_empty(),
        "{:?}",
        off.spec.non_target.diff(&on.spec.non_target)
    );
    assert_eq!(off.spec.non_target, on.spec.non_target);
    assert_eq!(
        off.spec.org.effective_profile_sha256,
        on.spec.org.effective_profile_sha256
    );
    // 稼働 daemon が返した review / 安全上限・builtin を含む harness 一覧・env-lock catalog・実 run の selection。
    assert_eq!(off.live_review_safety, on.live_review_safety);
    assert_eq!(
        off.harnesses, on.harnesses,
        "harness catalog (builtin + declared)"
    );
    assert_eq!(off.catalog, on.catalog, "env-lock catalog");
    assert_eq!(off.models, on.models, "live model catalog");
    assert_eq!(
        off.daemon_config, on.daemon_config,
        "builtin roles / genres / reviewer / limits"
    );
    assert_eq!(
        on.daemon_config["roles"][0]["id"], "implementer",
        "{}",
        on.daemon_config
    );
    assert_eq!(off.selections, on.selections, "observed run selections");
    assert!(!on.selections.is_empty());
    for (role, harness, source, model) in &on.selections {
        assert_eq!(on.spec.non_target.role_models[role], *model, "{role}");
        assert_eq!(
            (harness.as_str(), source.as_str()),
            ("stub", "stub"),
            "{role}"
        );
    }

    // 内部 review は既定を維持（variant で変えない。品質向上は主張しない）: off / on とも同じ review pin で、
    // escalate_on_fail は未指定の既定（true）のまま。
    let (review, _) = &on.live_review_safety;
    assert_eq!(review.harness, "reviewer");
    assert!(
        review.escalate_on_fail,
        "review escalation stays at its default"
    );

    // K2 の判定範囲は変えない: analysis・review は not_judged のまま、review の検証を主張する報告は違反。
    let k2: celeris_campaign::cases::Case = serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("cases/final/k2-bf-xattr-restart-campaign/case.json"),
        )
        .expect("K2 case"),
    )
    .expect("K2 case json");
    assert_eq!(
        k2.not_judged(),
        vec!["analysis".to_string(), "review".to_string()]
    );
    let (judged, not_judged) = (k2.judged_scope(), k2.not_judged());
    let claim = format!(
        "{}\nverified stage=review\n",
        celeris_campaign::report::scope_line(&judged, &not_judged)
    );
    assert!(
        !celeris_campaign::report::check_scope_claims(&judged, &not_judged, &claim).is_empty(),
        "a review-quality claim must be flagged"
    );
}
