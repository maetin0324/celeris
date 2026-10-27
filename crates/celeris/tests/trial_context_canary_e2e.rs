//! ADR 2026-10-17-mlsys-trial-context-isolation 通し試験（P0-3 canary e2e）: 実 task-api router（正式 DTO の
//! `POST /api/v1/tasks` + `campaign`）で A/B/C 相当の試行 task（親・子・planner）を作り、本番の組み立て経路
//! （`build_dispatcher`）の実 Dispatcher で走らせて、reviewer を含む全 run の `RunContext`・env・
//! knowledge 検索・workspace 読取を観測する。外部 LLM は呼ばない（捕捉するだけの stub adapter）。
//!
//! 通常運用の KB・skill・記憶・会話履歴・永続の認可、他の試行（他 arm・他反復）の成果、grader 私有領域にだけ
//! 置いた識別文字列は、どの run の文脈にも検索結果にも読取にも出ない。試行用 `kb/` にだけ置いた識別文字列は
//! 前置きの索引・`knowledge search`・`knowledge get` で得られる。測定 daemon の再起動（再開）と、環境を捨てて
//! 作り直した（リセット）後も同じ結果になる。task も org も DB 補完や手書き JSON では作らない
//! （task は HTTP、org と通常運用の記憶・会話は通常の store API）。

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use celeris::{ClusterMasters, Config, api_settings, build_dispatcher};
use serde_json::{Value, json};
use task_core::{
    ApprovalStore, DaemonMode, InstanceRole, Message, MessageId, MessageRole, OrgKind, OrgNode,
    SharedRole, SqliteStore, StandingRule, StandingRuleId, TaskId, TaskKind, TaskStore,
};
use task_dispatch::Dispatcher;
use task_worker::{
    AdapterError, EventSink, RunLimits, RunOutcome, RunRequest, Terminal, WorkerAdapter,
};
use time::OffsetDateTime;

const TOKEN: &str = "trial-canary-token";
const CAMPAIGN: &str = "canary-campaign";
const NODE: &str = "software-engineering";
const SHARED_KB_FILE: &str = "TRIAL_KB_CANARY_e19f.md";

// 通常運用側・grader 側にだけ置く識別文字列（試行に出てはいけない）。
const NORMAL_KB: &str = "NORMAL_KB_CANARY_7f3a";
const SKILL: &str = "SKILL_CANARY_51c2";
const MEMORY: &str = "MEMORY_CANARY_9d04";
const CHAT: &str = "CHAT_CANARY_2b8e";
const RULE: &str = "RULE_CANARY_c6a1";
const GRADER: &str = "GRADER_PRIVATE_CANARY_44aa";
// 試行用 KB（`--kb-src` 由来。A/B/C・全反復で同じ）にだけ置く識別文字列（試行に出る）。
const TRIAL_KB: &str = "TRIAL_KB_CANARY_e19f";

const ARMS: [&str; 3] = ["A", "B", "C"];

/// 試行ごとの成果（他試行には見えてはいけない）の識別文字列。
fn trial_output(arm: &str, rep: u32) -> String {
    format!("TRIAL_OUTPUT_CANARY_{arm}{rep}_b7d2")
}

fn forbidden_for(arm: &str, rep: u32, reps: &[u32]) -> Vec<String> {
    let mut out: Vec<String> = [NORMAL_KB, SKILL, MEMORY, CHAT, RULE, GRADER]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for a in ARMS {
        for r in reps {
            if (a, *r) != (arm, rep) {
                out.push(trial_output(a, *r));
            }
        }
    }
    out
}

/// (run の task id, kind, RunContext の JSON, adapter に重ねた env, 親 task)。reviewer の task は
/// 対象 task を親に持つ（store には無い）。
type Captured = Vec<(
    TaskId,
    TaskKind,
    String,
    Vec<(String, String)>,
    Option<TaskId>,
)>;

#[derive(Clone)]
struct CaptureAdapter {
    seen: Arc<Mutex<Captured>>,
    env: Vec<(String, String)>,
}

#[async_trait]
impl WorkerAdapter for CaptureAdapter {
    fn id(&self) -> &str {
        "capture"
    }

    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut next = self.clone();
        next.env.extend(extra.iter().cloned());
        Some(Arc::new(next))
    }

    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.seen.lock().expect("seen").push((
            req.task.id,
            req.task.kind,
            serde_json::to_string(&req.context).expect("context json"),
            self.env.clone(),
            req.task.parent_id,
        ));
        if req.task.kind == TaskKind::Review {
            std::fs::create_dir_all(&req.artifacts_dir).expect("artifacts");
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#,
            )
            .expect("review.json");
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

struct Env {
    dir: tempfile::TempDir,
    config: Config,
    normal_kb: PathBuf,
    base: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<Result<(), task_api::ApiError>>>,
    _daemon_tx: tokio::sync::watch::Sender<Option<task_ops::daemon::DaemonSnapshot>>,
}

fn org_node(id: &str, parent: Option<&str>, kind: OrgKind) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        profile: Default::default(),
        id: id.into(),
        parent_id: parent.map(str::to_string),
        name: format!("{id} 課"),
        kind,
        genre: None,
        brief: format!("{id} の担当"),
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

/// 測定専用の構成ではなく、**通常運用に近い構成**（通常 KB・記憶・会話・認可・skill が全部ある daemon）で
/// 立てる。試行の隔離は構成の良さに頼らず、dispatcher の文脈生成で成立することを示すため。
async fn env() -> Env {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("ws")).expect("ws");
    std::fs::write(root.join("api.token"), format!("{TOKEN}\n")).expect("token");

    // 通常運用の KB（git の KB。`kb:user` の default mount の下に canary）と skill。
    let normal_kb = root.join("normal-kb");
    task_ops::knowledge::init(&normal_kb).expect("kb init");
    std::fs::create_dir_all(normal_kb.join("user")).expect("user dir");
    std::fs::write(
        normal_kb.join("user/normal.md"),
        format!("# {NORMAL_KB}\n\nbody\n"),
    )
    .expect("normal page");
    task_ops::knowledge::reindex(&normal_kb).expect("reindex");
    let skill_dir = normal_kb.join("skills/canary-skill");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!(
            "---\nname: canary-skill\ndescription: {SKILL}\nceleris-use: work, review\n---\nbody\n"
        ),
    )
    .expect("skill");

    // 通常運用の記憶（担当の notes.md）。
    let memory = root.join("memory");
    std::fs::create_dir_all(memory.join(NODE)).expect("memory dir");
    std::fs::write(
        memory.join(NODE).join("notes.md"),
        format!("- 2026-10-01: {MEMORY}\n"),
    )
    .expect("notes");

    std::fs::write(
        root.join("config.toml"),
        format!(
            r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 20
max_concurrency = 4

[api]
listen = "127.0.0.1:17998"
token_file = "api.token"

[knowledge]
root = {kb:?}

[memory]
dir = {mem:?}

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["model-1"]

[harnesses.claude_code]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.claude_code]
candidates = [{{ source = "local", model = "model-1" }}]

[harness_selection]
execution_class_defaults = {{}}
rules = [{{ harness = "claude_code", required_capabilities = [] }}]
"#,
            kb = normal_kb.display().to_string(),
            mem = memory.display().to_string(),
        ),
    )
    .expect("config");
    let config = Config::load(&root.join("config.toml")).expect("load");
    let store = SqliteStore::open(&config.db.path).expect("store");

    // 担当（profile に通常 KB の mount と skill を持つ）・通常運用の会話・永続の認可。
    let mut se = org_node(NODE, Some("engineering"), OrgKind::Section);
    se.profile.skills_mounts = vec!["canary-skill".into()];
    se.profile.knowledge = vec![task_core::KnowledgeMount::kb("user")];
    for node in [
        org_node("cos", None, OrgKind::Secretary),
        org_node("engineering", Some("cos"), OrgKind::Department),
        se,
    ] {
        store.org_upsert(&node).expect("org");
    }
    let now = OffsetDateTime::now_utc();
    store
        .message_append(&Message {
            id: MessageId::new(),
            node_id: NODE.into(),
            project_id: None,
            role: MessageRole::User,
            text: CHAT.into(),
            run_id: None,
            task_id: None,
            metadata: None,
            created_at: now,
        })
        .expect("message");
    store
        .standing_rule_append(&StandingRule {
            id: StandingRuleId::new(),
            node_id: None,
            rule: RULE.into(),
            created_at: now,
        })
        .expect("rule");

    // 実 task-api router（正式 DTO）。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("addr");
    let settings = api_settings(
        &config,
        addr,
        vec![task_api::ApiToken::new(
            task_api::Principal::Human {
                name: "test".into(),
            },
            TOKEN,
        )],
        "01J00000000000000000000000".to_string(),
        "2026-10-17T00:00:00Z".to_string(),
        None,
        "dev".to_string(),
        DaemonMode::Normal,
        SharedRole::new(InstanceRole::Active),
        None,
    );
    let (daemon_tx, daemon_rx) = tokio::sync::watch::channel(None);
    let state = task_api::ApiState::new(settings, daemon_rx).expect("state");
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(task_api::serve_with_listener(listener, state, async move {
        let _ = stopped.await;
    }));
    Env {
        dir,
        config,
        normal_kb,
        base: format!("http://{addr}"),
        stop: Some(stop),
        handle: Some(handle),
        _daemon_tx: daemon_tx,
    }
}

impl Env {
    async fn post(&self, body: Value) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .post(format!("{}/api/v1/tasks", self.base))
            .header("authorization", format!("Bearer {TOKEN}"))
            .json(&body)
            .send()
            .await
            .expect("post");
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            tokio::time::timeout(Duration::from_secs(60), handle)
                .await
                .expect("server stopped")
                .expect("server task")
                .expect("server result");
        }
    }

    fn trial_ws(&self, arm: &str, rep: u32) -> PathBuf {
        self.dir.path().join("trials").join(format!("{arm}-{rep}"))
    }

    /// runner の `build_seed` と同じ形の試行作業場所: `materials/`・`kb/`（A/B/C・全反復で同一）と、この試行の
    /// 成果（`output/`。他試行に見えてはいけない）。grader 私有領域は試行の外に置く。
    fn seed_trial(&self, arm: &str, rep: u32) -> PathBuf {
        let ws = self.trial_ws(arm, rep);
        std::fs::create_dir_all(ws.join("kb")).expect("kb");
        std::fs::create_dir_all(ws.join("materials")).expect("materials");
        std::fs::create_dir_all(ws.join("output")).expect("output");
        std::fs::write(
            ws.join("kb").join(SHARED_KB_FILE),
            format!("# {TRIAL_KB}\n\ntrial kb body\n"),
        )
        .expect("trial kb page");
        std::fs::write(ws.join("materials/brief.md"), "shared materials\n").expect("materials");
        std::fs::write(
            ws.join("output/result.md"),
            format!("{}\n", trial_output(arm, rep)),
        )
        .expect("output");
        let grader = self.dir.path().join("grader-private");
        std::fs::create_dir_all(&grader).expect("grader");
        std::fs::write(grader.join("answers.md"), format!("{GRADER}\n")).expect("grader file");
        ws
    }

    /// A/B/C 相当の 1 試行: 親（reviewer 付き）・子・planner を `POST /tasks`（campaign 付き）で作る。
    /// 子は campaign 識別を持たず親から継ぐ。planner は別の根 task（同じ試行の識別）。
    async fn post_trial(&self, arm: &str, rep: u32) -> Trial {
        let ws = self.seed_trial(arm, rep);
        let plan_ws = ws.join("plan-run");
        std::fs::create_dir_all(plan_ws.join("kb")).expect("plan kb");
        std::fs::write(
            plan_ws.join("kb").join(SHARED_KB_FILE),
            format!("# {TRIAL_KB}\n\ntrial kb body\n"),
        )
        .expect("plan kb page");
        let identity = |sub: &str| {
            json!({"campaign_id": CAMPAIGN, "case_id": format!("case-{sub}"), "arm": arm,
                   "repetition": rep, "manifest_hash": "mh-1", "routing_setting": "fixed"})
        };
        let key = |kind: &str| format!("campaign-{}-{rep}-{kind}", arm.to_lowercase());
        let (s, root) = self
            .post(json!({
                "title": format!("trial {arm} {rep}"), "objective": "o",
                "acceptance": [{"type": "reviewer", "text": "reviewed"}],
                "assignee": NODE, "workspace": ws, "max_retries": 0,
                "campaign": identity("w"), "idempotency_key": key("w"),
            }))
            .await;
        assert_eq!(s, 201, "{root}");
        let root_id: TaskId = root["id"].as_str().expect("id").parse().expect("task id");
        let (s, child) = self
            .post(json!({
                "title": "child", "objective": "child", "parent": root_id.to_string(),
                "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}],
                "assignee": NODE, "max_retries": 0,
            }))
            .await;
        assert_eq!(s, 201, "{child}");
        let child_id: TaskId = child["id"].as_str().expect("id").parse().expect("task id");
        let (s, plan) = self
            .post(json!({
                "title": format!("plan {arm} {rep}"), "objective": "plan", "kind": "plan",
                "acceptance": [{"type": "artifact_exists", "name": "plan.json"}],
                "assignee": NODE, "workspace": plan_ws, "max_retries": 0,
                "campaign": identity("p"), "idempotency_key": key("p"),
            }))
            .await;
        assert_eq!(s, 201, "{plan}");
        let plan_id: TaskId = plan["id"].as_str().expect("id").parse().expect("task id");
        Trial {
            arm: arm.into(),
            rep,
            ws,
            plan_ws,
            root: root_id,
            child: child_id,
            planner: plan_id,
        }
    }
}

struct Trial {
    arm: String,
    rep: u32,
    ws: PathBuf,
    plan_ws: PathBuf,
    root: TaskId,
    child: TaskId,
    planner: TaskId,
}

fn tick_dispatcher(env: &Env, seen: &Arc<Mutex<Captured>>) -> Dispatcher {
    let mut d = build_dispatcher(&env.config, ClusterMasters::default()).expect("dispatcher");
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(CaptureAdapter {
        seen: seen.clone(),
        env: Vec::new(),
    });
    let mut adapters = std::collections::HashMap::new();
    adapters.insert(
        task_core::llm_route::HarnessId::from("claude_code"),
        adapter,
    );
    d.set_harness_adapters(adapters);
    d
}

/// 出来事（run の終了）で待つ。固定時間では待たず、idle になるまで tick する。
async fn run_until_idle(d: &mut Dispatcher) {
    for _ in 0..2000 {
        let report = d.tick().expect("tick");
        if report.idle {
            return;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("dispatcher did not become idle");
}

fn tree_text(dir: &Path) -> String {
    let mut out = String::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&tree_text(&path));
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            out.push_str(&text);
        }
    }
    out
}

/// 1 試行分の観測を検査し、観測の署名（再開・リセットで同じになるべき値）を返す。
fn assert_trial(
    env: &Env,
    captured: &Captured,
    t: &Trial,
    reps: &[u32],
    started: &mut BTreeSet<TaskId>,
) -> Vec<String> {
    let forbidden = forbidden_for(&t.arm, t.rep, reps);
    let label = format!("{}{}", t.arm, t.rep);
    let mut signature = Vec::new();

    // 1. 実際に生成された文脈（worker・子 task・planner・reviewer）。
    let mut kinds = BTreeSet::new();
    let review_targets: BTreeSet<TaskId> = [t.root, t.child, t.planner].into_iter().collect();
    for (id, kind, ctx, run_env, parent) in captured {
        let mine =
            review_targets.contains(id) || parent.is_some_and(|p| review_targets.contains(&p));
        if !mine {
            continue;
        }
        started.insert(*id);
        kinds.insert(format!("{kind:?}"));
        for canary in &forbidden {
            assert!(
                !ctx.contains(canary),
                "{label} {kind:?} の RunContext に {canary} が出た: {ctx}"
            );
        }
        for (k, v) in run_env {
            for canary in &forbidden {
                assert!(!v.contains(canary), "{label} env {k} に {canary}");
            }
        }
        if *kind != TaskKind::Review {
            assert!(
                ctx.contains(TRIAL_KB),
                "{label} {kind:?} の knowledge index に試行用 kb/ の canary が無い: {ctx}"
            );
            let kb_env = run_env
                .iter()
                .find(|(k, _)| k == "CELERIS_KNOWLEDGE_ROOT")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let expect = if *id == t.planner {
                t.plan_ws.join("kb")
            } else {
                t.ws.join("kb")
            };
            assert_eq!(
                kb_env,
                expect.display().to_string(),
                "{label} {kind:?} の KB の根が試行用 kb/ ではない"
            );
        }
        signature.push(format!("{kind:?}"));
    }
    for kind in ["Execute", "Plan", "Review"] {
        assert!(
            kinds.contains(kind),
            "{label} に {kind} の run が走っていない: {kinds:?}"
        );
    }

    // 2. knowledge 検索と読取（worker が `celerisctl knowledge` で解決する根 = env の
    // `CELERIS_KNOWLEDGE_ROOT`）。禁止の識別文字列は出ず、試行用 KB の識別文字列は得られる。
    let root_kb = t.ws.join("kb");
    for canary in &forbidden {
        assert!(
            task_ops::knowledge::search(&root_kb, canary, None, 20).is_empty(),
            "{label}: knowledge search で {canary} が出た"
        );
    }
    let hits = task_ops::knowledge::search(&root_kb, TRIAL_KB, None, 20);
    assert!(
        hits.iter().any(|h| h.item.path == SHARED_KB_FILE),
        "{label}: knowledge search で試行用 KB が得られない: {hits:?}"
    );
    let page = task_ops::knowledge::read_page(&root_kb, SHARED_KB_FILE).expect("trial page");
    assert!(page.contains(TRIAL_KB));
    // 対照: 通常 KB を根にすれば同じ検索で NORMAL_KB は見つかる（canary は検索できる状態にある）。
    assert!(
        !task_ops::knowledge::search(&env.normal_kb, NORMAL_KB, None, 20).is_empty(),
        "対照: 通常 KB の canary が検索で見つからない（試験が空振りしている）"
    );

    // 3. workspace 読取（worker が cwd から読めるもの）。
    for dir in [&t.ws, &t.plan_ws] {
        let text = tree_text(dir);
        for canary in &forbidden {
            // 試行自身の成果は自分の workspace にあってよい。禁止は他試行・通常運用・grader のもの。
            assert!(!text.contains(canary), "{label} の workspace に {canary}");
        }
    }
    assert!(tree_text(&t.ws.join("kb")).contains(TRIAL_KB));
    assert!(tree_text(&t.ws.join("materials")).contains("shared materials"));
    signature.sort();
    signature
}

/// 1 回の環境（作成 → 試行投入 → 実行）で全 canary の検査を通し、試行ごとの署名を返す。
async fn run_scenario(env: &Env, reps: &[u32], d: &mut Dispatcher, seen: &Arc<Mutex<Captured>>) {
    let mut trials = Vec::new();
    for rep in reps {
        for arm in ARMS {
            trials.push(env.post_trial(arm, *rep).await);
        }
    }
    run_until_idle(d).await;
    let captured = seen.lock().expect("seen").clone();
    let mut started = BTreeSet::new();
    let all_reps: Vec<u32> = reps.to_vec();
    for t in &trials {
        assert_trial(env, &captured, t, &all_reps, &mut started);
    }
}

#[tokio::test]
async fn trial_tasks_through_the_real_router_see_only_the_trial_kb() {
    let env = env().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut d = tick_dispatcher(&env, &seen);
    run_scenario(&env, &[1], &mut d, &seen).await;
    env.shutdown().await;
}

/// 再開: dispatcher（daemon）を作り直し、同じ DB・同じ out に別の反復を足しても、同じ結果になる
/// （前の反復の成果・KB が次の反復に混ざらない）。
#[tokio::test]
async fn resumed_daemon_gives_the_same_canary_result() {
    let env = env().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut d = tick_dispatcher(&env, &seen);
    run_scenario(&env, &[1], &mut d, &seen).await;
    drop(d);
    // 再起動: 本番の組み立てをもう一度行い、同じ DB を引き継ぐ。2 反復目を投入する。
    let seen2 = Arc::new(Mutex::new(Vec::new()));
    let mut d2 = tick_dispatcher(&env, &seen2);
    let mut trials = Vec::new();
    for arm in ARMS {
        trials.push(env.post_trial(arm, 2).await);
    }
    run_until_idle(&mut d2).await;
    let captured = seen2.lock().expect("seen2").clone();
    let mut started = BTreeSet::new();
    for t in &trials {
        let sig = assert_trial(&env, &captured, t, &[1, 2], &mut started);
        assert!(!sig.is_empty());
    }
    // 再開後の run が 1 反復目の task を再実行していない（再開で二重実行しない）。
    assert!(
        captured.iter().all(|(id, ..)| !seen
            .lock()
            .expect("seen")
            .iter()
            .any(|(old, ..)| old == id)),
        "再開後に既に走った task が再実行された"
    );
    env.shutdown().await;
}

/// リセット: 環境を捨てて作り直しても、同じ試行が同じ canary 結果になる（署名が同じ）。
#[tokio::test]
async fn reset_environment_gives_the_same_canary_result() {
    let mut signatures = Vec::new();
    for _ in 0..2 {
        let env = env().await;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut d = tick_dispatcher(&env, &seen);
        run_scenario(&env, &[1], &mut d, &seen).await;
        let captured = seen.lock().expect("seen").clone();
        let mut kinds: Vec<String> = captured.iter().map(|c| format!("{:?}", c.1)).collect();
        kinds.sort();
        // 文脈に載った試行用 KB の索引項目も署名に含める（環境が違っても同じ）。
        let with_trial_kb = captured
            .iter()
            .filter(|c| c.1 != TaskKind::Review && c.2.contains(TRIAL_KB))
            .count();
        signatures.push((kinds, with_trial_kb));
        env.shutdown().await;
    }
    assert_eq!(signatures[0], signatures[1]);
}
