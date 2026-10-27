//! 分類段の一連試験（ADR 2026-10-15-jev-wire D2〜D4）: dispatch tick 全体で
//! 「鍵未登録 → credential 待ち → 通知 → 鍵登録 → 同じ WU が一度だけ再開 → 分類」を示す。
//!
//! 本番の組み立て（`build_dispatcher`・`ClassifyTick::build`・`SecretsDirKeyProvider`・`JevClassifier`）をそのまま使い、
//! Jev は 127.0.0.1 の fake HTTP server、通知は fake webhook、鍵の検証だけ mock（provider adapter 境界）にする。
//! 外部ネットワークには出ない。待ちは DB の状態で判定し、時間では待たない。

#[cfg(test)]
#[path = "../../../../../tests/support/http_async.rs"]
mod fake_http;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use task_core::route_select::{ClassifierStatus, RoutingMode};
use task_core::{
    Budget, Check, CredentialWaitState, Criterion, NotificationKind, NotificationStore,
    SqliteStore, Status, Task, TaskId, TaskKind, TaskStore, Tier, WorkerHint, WorkspaceSpec,
};
use task_dispatch::Dispatcher;
use task_dispatch::credential_resume::{
    CredentialResumer, CredentialVerifier, FileSecretSource, VerifyOutcome, secret_fingerprint,
};
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;

use super::ClassifyTick;
use crate::notify::{self, SendResult};
use crate::{ClusterMasters, Config, build_dispatcher};

const WEB: &str = "http://celeris.test";
const SECRET: &str = "jev_api_key";
const DUMMY: &str = "sk-dummy-jev-e2e-never-leak-4242";
const OBJECTIVE: &str = "Jev で分類する";
const PURPOSE: &str = "Jev で依頼を分類する";
const OK: &str = r#"{"model":"typesafe/jev-1.14","answers":{"specificity":{"choice":"specified"},"work_nature":{"choice":"implementation"}}}"#;

/// 偽 webhook: 204 を返し、受けた本文を残す。
struct Hook {
    url: String,
    bodies: Arc<Mutex<Vec<String>>>,
}

async fn hook() -> Hook {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}/hook", listener.local_addr().expect("addr"));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let kept = bodies.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let kept = kept.clone();
            tokio::spawn(async move {
                let Some(raw) =
                    fake_http::read(&mut stream, |r| fake_http::matches(r, "POST", "/hook")).await
                else {
                    return;
                };
                let body = raw.split_once("\r\n\r\n").unwrap().1.to_owned();
                kept.lock().expect("bodies").push(body);
                let _ = stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    Hook { url, bodies }
}

fn new_task(dir: &std::path::Path, title: &str) -> Task {
    let id = TaskId::new();
    let now = OffsetDateTime::now_utc();
    Task {
        harness_id: Some("claude_code".into()),
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        // lane 層（`apply_lane_assist`）は routing を持つ Execute task だけが通る。
        routing: Some(task_core::TaskRouting {
            tier_source: task_core::TierSource::Hint,
            ..Default::default()
        }),
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: title.into(),
        objective: OBJECTIVE.into(),
        acceptance: vec![Criterion {
            text: "人が確かめる".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: dir.join(id.to_string()),
            mode: None,
        },
        budget: Budget {
            max_turns: 3,
            max_wall_secs: 60,
            max_retries: 0,
            max_tokens: None,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: Some("coding".into()),
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

/// fake Jev server: 受けた request 全文を残し、`status` と `body` を返す。
struct JevServer {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl JevServer {
    fn count(&self) -> usize {
        self.requests.lock().expect("requests").len()
    }

    /// `instruction` が `objective` そのものの request の数。同じ入力は classifier の cache と
    /// `max_retries = 0` で再送されないので、総数が増えるのは別の subject の分類（負荷で時機のずれた
    /// daemon 側の task 等。release gate の並列実行で総数 3 を観測）。試験の task だけを数える。
    fn count_for(&self, objective: &str) -> usize {
        let needle = format!("\"instruction: {objective}\"");
        self.requests
            .lock()
            .expect("requests")
            .iter()
            .filter(|r| r.contains(&needle))
            .count()
    }
}

async fn jev_server(status: u16, body: &'static str) -> JevServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}/decisions", listener.local_addr().expect("addr"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let kept = requests.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let kept = kept.clone();
            tokio::spawn(async move {
                let Some(raw) =
                    fake_http::read(&mut sock, |r| fake_http::matches(r, "POST", "/decisions"))
                        .await
                else {
                    return;
                };
                kept.lock().expect("requests").push(raw);
                let resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    JevServer { url, requests }
}

/// mock 検証（provider adapter 境界）。呼ばれた回数だけ数える。
struct Scripted {
    outcomes: Mutex<VecDeque<VerifyOutcome>>,
    calls: AtomicUsize,
}

impl CredentialVerifier for Scripted {
    fn verify(&self, _secret_id: &str, _value: &str) -> VerifyOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.outcomes
            .lock()
            .expect("outcomes")
            .pop_front()
            .unwrap_or(VerifyOutcome::Transient)
    }
}

struct Env {
    dir: tempfile::TempDir,
    config: Config,
    store: Arc<SqliteStore>,
    hook: Hook,
    secrets: std::path::PathBuf,
    jev: JevServer,
    tick: ClassifyTick,
    dispatcher: Dispatcher,
}

async fn env(jev_status: u16, jev_body: &'static str, register_key: bool) -> Env {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let hook = hook().await;
    let jev = jev_server(jev_status, jev_body).await;
    let secrets = root.join("secrets");
    std::fs::create_dir_all(&secrets).expect("secrets dir");
    let knowledge = root.join("knowledge");
    for name in ["cos-operator", "cos-inbox-triage"] {
        let skill = knowledge.join("skills").join(name);
        std::fs::create_dir_all(&skill).expect("skill dir");
        std::fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test skill\n---\n# {name}\n"),
        )
        .expect("skill");
    }
    // `[llm_sources.jev]` は置かない（ADR jev-wire D5）。worker の source は別名の fake。
    let config_path = root.join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 20
max_concurrency = 2

[api]
listen = "127.0.0.1:17997"

[[api.tokens]]
principal = "operator:test"
token_file = "api.token"

[secrets]
dir = {secrets:?}

[notify]
web_base_url = "{WEB}"

[routing]
mode = "decision_assisted"

[jev]
enabled = true
endpoint = "{endpoint}"
model_id = "typesafe/jev-1.14"
max_retries = 0

[cos]
enabled = true
max_turns = 2
max_wall_secs = 30
route_candidates = [{{ source = "fakesrc", model = "model-1" }}]

[knowledge]
root = {knowledge:?}

[adapters.fake]
command = ["sh", "-c", "cat >/dev/null; exit 1"]

[llm_sources.fakesrc]
kind = "fake"
accounts = ["a1"]
models = ["model-1"]

[harnesses.claude_code]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.claude_code]
candidates = [{{ source = "fakesrc", model = "model-1" }}]
"#,
            secrets = secrets.display().to_string(),
            knowledge = knowledge.display().to_string(),
            endpoint = jev.url,
        ),
    )
    .expect("config");
    std::fs::write(root.join("api.token"), "jev-e2e-test-token\n").expect("token");
    let config = Config::load(&config_path).expect("config load");
    std::fs::write(
        secrets.join(&config.notify.discord_webhook_secret),
        format!("{}\n", hook.url),
    )
    .expect("webhook secret");
    if register_key {
        std::fs::write(secrets.join(SECRET), format!("{DUMMY}\n")).expect("key");
    }
    let store = Arc::new(SqliteStore::open(&config.db.path).expect("store"));
    let dispatcher = build_dispatcher(&config, ClusterMasters::default()).expect("dispatcher");
    let tick = ClassifyTick::build(
        &config.jev,
        config.secrets.as_ref().map(|s| s.dir.clone()),
        dispatcher.classification_board(),
    )
    .expect("build")
    .expect("jev enabled");
    Env {
        dir,
        config,
        store,
        hook,
        secrets,
        jev,
        tick,
        dispatcher,
    }
}

impl Env {
    /// daemon の tick ループと同じ順: 分類段（`Dispatcher::tick` の直前）→ dispatch tick。
    async fn step(&mut self) {
        let store = self.dispatcher.store();
        self.tick
            .run(store.as_ref(), self.dispatcher.routing_mode())
            .await;
        self.dispatcher.tick().expect("tick");
    }

    fn wait_state(&self, wu_id: &str) -> CredentialWaitState {
        self.store
            .credential_wait_get(wu_id, SECRET)
            .expect("wait")
            .expect("row")
            .state
    }

    fn count(&self, sql: &str) -> i64 {
        let conn = rusqlite::Connection::open(&self.config.db.path).expect("db");
        conn.query_row(sql, [], |r| r.get(0)).expect("count")
    }

    fn view(&self) -> task_ops::view::ViewContext {
        task_ops::view::ViewContext {
            workspace_roots: task_core::local_machine::WorkspaceRoots::new(
                self.dir.path().to_path_buf(),
                None,
            ),
            retry_backoff_base: std::time::Duration::from_secs(1),
            retry_backoff_max: std::time::Duration::from_secs(60),
            max_requeues: 3,
            clusters: Default::default(),
        }
    }

    fn credential_inbox_items(&self) -> usize {
        task_ops::human_inbox::human_inbox(
            self.store.as_ref(),
            None,
            &self.view(),
            OffsetDateTime::now_utc(),
            &|_, _| Vec::new(),
        )
        .expect("inbox")
        .items
        .into_iter()
        .filter(|i| i.kind == task_ops::human_inbox::InboxKind::CredentialWait)
        .count()
    }

    fn webhook_bodies(&self) -> Vec<String> {
        self.hook.bodies.lock().expect("bodies").clone()
    }

    /// PUT /api/v1/secrets/{id} の効果: 値を secrets の file に置き、`secret_stored`（id と fingerprint のみ）を記録する。
    fn put_secret(&self, value: &str) {
        std::fs::write(self.secrets.join(SECRET), format!("{value}\n")).expect("put");
        self.store
            .credential_record_secret_saved(
                SECRET,
                &secret_fingerprint(value),
                OffsetDateTime::now_utc(),
            )
            .expect("secret_stored");
    }

    /// daemon の notifier tick（1 通まで）。credential 待ちの escalation だけを送る。
    async fn notifier_tick(&self) -> bool {
        let pending: Vec<_> = self
            .store
            .notification_pending()
            .expect("pending")
            .into_iter()
            .filter(|n| n.kind == NotificationKind::CosEscalation)
            .collect();
        let Some(mut batch) = notify::select_routes_batch(&pending) else {
            return false;
        };
        let row = pending
            .iter()
            .find(|n| batch.ids.contains(&n.id))
            .expect("row")
            .clone();
        let now = OffsetDateTime::now_utc();
        if !notify::triage::withdraw_if_resolved(
            &self.store,
            &self.config.db.path,
            &self.view(),
            &row,
            now,
        )
        .expect("recheck")
        {
            return false;
        }
        batch.content = notify::triage::render(&row, &self.config.notify).expect("render");
        let url = notify::webhook_url(
            Some(&self.secrets),
            &self.config.notify.discord_webhook_secret,
        )
        .expect("webhook url");
        let client = notify::client().expect("client");
        let outcome = notify::post_webhook(&client, &url, &batch.content).await;
        notify::record(
            self.store.as_ref(),
            &pending,
            &SendResult {
                ids: batch.ids,
                outcome,
            },
            now,
        )
        .expect("record");
        true
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unregistered_key_waits_notifies_then_resumes_once_and_classifies() {
    let mut env = env(200, OK, false).await;
    let verifier = Arc::new(Scripted {
        outcomes: Mutex::new(VecDeque::from([VerifyOutcome::Valid])),
        calls: AtomicUsize::new(0),
    });
    env.dispatcher.set_credential_resumer(Arc::new(
        CredentialResumer::new(
            env.store.clone(),
            Arc::new(FileSecretSource {
                dir: env.secrets.clone(),
            }),
            Arc::new(OffsetDateTime::now_utc),
        )
        .with_verifier("jev", verifier.clone()),
    ));
    env.step().await; // 導入

    let task = new_task(env.dir.path(), "分類");
    env.store.insert(&task).expect("insert");
    let subject = task.id.to_string();
    let wu = format!("task:{}", task.id);

    // 未登録 → credential 待ち（出来事 = credential_waits の行、で待つ）。
    for _ in 0..200 {
        env.step().await;
        if env.count("SELECT COUNT(*) FROM credential_waits") >= 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    for _ in 0..4 {
        env.step().await;
    }
    assert_eq!(env.wait_state(&wu), CredentialWaitState::Waiting);
    assert_eq!(
        env.jev.count_for(OBJECTIVE),
        0,
        "鍵が無い間は Jev を呼ばない"
    );
    let item = env.dispatcher.classification_board().get(&subject);
    assert!(item.is_some_and(|i| i.credential.is_some()));
    assert_eq!(
        env.count("SELECT COUNT(*) FROM runs"),
        0,
        "待ちの間 run は 0 本"
    );
    assert_eq!(env.credential_inbox_items(), 1);

    // 通知: 1 件だけ（再 tick・再 notifier でも増えない）。
    assert!(env.notifier_tick().await, "one POST");
    for _ in 0..3 {
        env.step().await;
        assert!(!env.notifier_tick().await, "no second POST");
    }
    let bodies = env.webhook_bodies();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].contains(PURPOSE), "{}", bodies[0]);
    assert!(bodies[0].contains("http://celeris.test/accounts"));
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 0);

    // 鍵登録 → 検証 1 回 → 同じ WU が一度だけ再開 → 分類（Jev へ 1 回、bearer 付き）。
    env.put_secret(DUMMY);
    for _ in 0..6 {
        env.step().await;
    }
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
    assert_eq!(env.wait_state(&wu), CredentialWaitState::Resumed);
    let classified = env.dispatcher.classification_board().get(&subject);
    let classified = classified.expect("classified after resume");
    assert_eq!(classified.entry.status, ClassifierStatus::JevOk);
    assert!(classified.credential.is_none());
    assert_eq!(env.jev.count_for(OBJECTIVE), 1, "分類は 1 回だけ");
    let request = env.jev.requests.lock().expect("requests")[0].clone();
    assert!(request.contains(&format!("Bearer {DUMMY}")), "{request}");

    // 重複 event（同じ fingerprint の再保存）・再 tick でも再開は 1 回のまま。
    env.put_secret(DUMMY);
    for _ in 0..6 {
        env.step().await;
    }
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
    assert_eq!(env.count("SELECT COUNT(*) FROM credential_waits"), 1);
    assert_eq!(env.jev.count_for(OBJECTIVE), 1);
    assert_eq!(env.credential_inbox_items(), 0);
    assert_eq!(env.webhook_bodies().len(), 1, "再開後に通知は増えない");
    assert_eq!(RoutingMode::DecisionAssisted, env.dispatcher.routing_mode());

    // 鍵の値は DB・event・通知本文のどこにも出ない。
    for body in env.webhook_bodies() {
        assert!(!body.contains(DUMMY));
    }
    let leaked = env.count(&format!(
        "SELECT (SELECT COUNT(*) FROM events WHERE json LIKE '%{DUMMY}%') + (SELECT COUNT(*) FROM notifications WHERE body LIKE '%{DUMMY}%')"
    ));
    assert_eq!(leaked, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rate_limit_never_becomes_a_credential_wait() {
    let mut env = env(429, "{}", true).await;
    fake_http::inject_noise(&env.jev.url).await;
    assert_eq!(env.jev.count(), 0);
    env.step().await;
    let task = new_task(env.dir.path(), "分類 429");
    env.store.insert(&task).expect("insert");
    let subject = task.id.to_string();
    for _ in 0..200 {
        env.step().await;
        if env
            .dispatcher
            .classification_board()
            .get(&subject)
            .is_some()
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    for _ in 0..8 {
        env.step().await;
    }
    let item = env
        .dispatcher
        .classification_board()
        .get(&subject)
        .expect("item");
    assert_eq!(
        item.entry.status,
        ClassifierStatus::FellBackToFixed {
            reason: "rate_limited".into()
        }
    );
    assert!(item.credential.is_none());
    assert_eq!(env.count("SELECT COUNT(*) FROM credential_waits"), 0);
    assert_eq!(env.credential_inbox_items(), 0);
    let escalations = env
        .store
        .notification_pending()
        .expect("pending")
        .into_iter()
        .filter(|n| n.kind == NotificationKind::CosEscalation)
        .count();
    assert_eq!(escalations, 0, "鍵の再登録を求める通知は出ない");
    assert_eq!(env.jev.count(), 1, "429 は再分類も再試行もしない");
}
