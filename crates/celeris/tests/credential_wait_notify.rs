//! ADR 2026-10-15-mlsys-min-decision-assisted D4 / 2026-10-15-credential-wait-skip-triage の通し試験（偽 webhook 経由）。
//!
//! 本番の組み立て経路（`build_dispatcher`）で作った dispatcher の `tick` だけで、credential 未登録の work unit が
//! 待機へ移り、受信箱 item と escalation outbox の行が作られる。通知は daemon の notifier tick と同じ順
//! （`select_routes_batch` → `withdraw_if_resolved` → `render` → webhook）で 127.0.0.1 の偽 webhook server へ送る
//! （外部ネットワークに出ない）。その後 PUT /api/v1/secrets/{id} 相当（秘密の保存 + `secret_stored` event）→ mock 検証成功
//! → 同じ work unit が一度だけ再キューされる。待ちは DB の状態の出来事で判定し、時間では待たない。

#[cfg(test)]
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "../../../tests/support/http_async.rs"]
mod fake_http;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use celeris::notify::{self, SendResult};
use celeris::{ClusterMasters, Config, build_dispatcher};
use task_core::{
    Budget, Check, Criterion, NotificationStore, SqliteStore, Status, Task, TaskId, TaskKind,
    TaskStore, Tier, WorkerHint, WorkspaceSpec,
};
use task_dispatch::Dispatcher;
use task_dispatch::credential_resume::{
    CredentialResumer, CredentialVerifier, FileSecretSource, VerifyOutcome, secret_fingerprint,
};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const WEB: &str = "http://celeris.test";
const SECRET: &str = "jev_api_key";
const DUMMY: &str = "sk-dummy-notify-e2e-never-leak-9911";
const PURPOSE: &str = "Jev で依頼を分類する";

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

#[tokio::test]
async fn webhook_ignores_eof_before_complete_request() {
    let hook = hook().await;
    let address = hook
        .url
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/hook")
        .unwrap();
    for partial in [
        "",
        "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "POST /hook HTTP/1.1\r\nContent-Length: 4\r\n\r\nx",
    ] {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(partial.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).await.unwrap();
        assert!(
            reply.starts_with(b"HTTP/1.1 404"),
            "foreign or incomplete request must be rejected"
        );
    }
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream
        .write_all(b"POST /hook HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}")
        .await
        .unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await.unwrap();
    assert!(reply.starts_with(b"HTTP/1.1 204"));
    assert_eq!(*hook.bodies.lock().unwrap(), vec!["{}"]);
}

/// mock 検証（provider adapter 境界）。呼ばれた回数と受け取った値を内側にだけ残す。
struct Scripted {
    outcomes: Mutex<VecDeque<VerifyOutcome>>,
    calls: AtomicUsize,
    seen: Mutex<Vec<String>>,
}

impl CredentialVerifier for Scripted {
    fn verify(&self, _secret_id: &str, value: &str) -> VerifyOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().expect("seen").push(value.to_string());
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
}

async fn env() -> Env {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let hook = hook().await;
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
listen = "127.0.0.1:17998"

[[api.tokens]]
principal = "operator:test"
token_file = "api.token"

[secrets]
dir = {secrets:?}

# 本番 host の release 置き場を読まない（verify 済みで未昇格の release があると停滞の通知が混ざる）。
[selfdeploy]
releases_dir = "releases"
repo = "repo"
detach = "inline"

[notify]
web_base_url = "{WEB}"

# host の本番 release 置き場（既定 /local/celeris/state/releases）を読まない。読むと未昇格 release の停滞通知が混ざる。
[selfdeploy]
releases_dir = "releases"

[cos]
enabled = true
max_turns = 2
max_wall_secs = 30
route_candidates = [{{ source = "jev", model = "model-1" }}]

[knowledge]
root = {knowledge:?}

[adapters.fake]
command = ["sh", "-c", "cat >/dev/null; exit 1"]

[llm_sources.jev]
kind = "fake"
accounts = ["a1"]
models = ["model-1"]
credential_ref = "{SECRET}"
wait_for_credential = true
credential_purpose = "{PURPOSE}"

[harnesses.claude_code]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.claude_code]
candidates = [{{ source = "jev", model = "model-1" }}]
"#,
            secrets = secrets.display().to_string(),
            knowledge = knowledge.display().to_string(),
        ),
    )
    .expect("config");
    std::fs::write(root.join("api.token"), "credential-wait-test-token\n").expect("token");
    let config = Config::load(&config_path).expect("config load");
    // 偽 webhook の URL は本物と同じく secrets の 1 file（値は loopback の URL）。
    std::fs::write(
        secrets.join(&config.notify.discord_webhook_secret),
        format!("{}\n", hook.url),
    )
    .expect("webhook secret");
    let store = Arc::new(SqliteStore::open(&config.db.path).expect("store"));
    Env {
        dir,
        config,
        store,
        hook,
        secrets,
    }
}

fn new_task(dir: &std::path::Path, title: &str) -> Task {
    let id = TaskId::new();
    let now = OffsetDateTime::now_utc();
    Task {
        harness_id: Some("claude_code".into()),
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: title.into(),
        objective: "Jev で分類する".into(),
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
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

impl Env {
    fn wait_state(&self, wu_id: &str) -> task_core::CredentialWaitState {
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

    fn inbox_items(&self) -> Vec<task_ops::human_inbox::InboxItem> {
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
        .collect()
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

    /// daemon の notifier tick（1 通まで）。POST したかを返す（送信は偽 webhook まで実際に通す）。
    async fn notifier_tick(&self) -> bool {
        // 見るのは credential 待ちの escalation だけ。再開後に偽 worker が失敗して出る別種の通知は数えない。
        let pending: Vec<_> = self
            .store
            .notification_pending()
            .expect("pending")
            .into_iter()
            .filter(|n| n.kind == task_core::NotificationKind::CosEscalation)
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
            None,
            &row,
            now,
        )
        .expect("recheck")
        .is_send()
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

fn tick(d: &mut Dispatcher) {
    d.tick().expect("tick");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wait_notify_register_verify_and_resume_once_through_the_fake_webhook() {
    let env = env().await;
    let mut d = build_dispatcher(&env.config, ClusterMasters::default()).expect("dispatcher");
    // 検証だけ mock（provider adapter 境界）。resumer の配線は本番と同じ（store・FileSecretSource）。
    let verifier = Arc::new(Scripted {
        outcomes: Mutex::new(VecDeque::from([VerifyOutcome::Valid])),
        calls: AtomicUsize::new(0),
        seen: Mutex::new(Vec::new()),
    });
    d.set_credential_resumer(Arc::new(
        CredentialResumer::new(
            env.store.clone(),
            Arc::new(FileSecretSource {
                dir: env.secrets.clone(),
            }),
            Arc::new(OffsetDateTime::now_utc),
        )
        .with_verifier("jev", verifier.clone()),
    ));
    tick(&mut d); // 導入: 空の受信箱

    // 同じ秘密を待つ 2 つの task（計画なし = task 自身が 1 つの work unit `task:<id>`。WorkUnit の event が出ない経路）。
    // 計画つき unit だけが待ち兄弟が止まらない点は task-dispatch の credential_gate / credential_e2e が示す。
    let (t1, t2) = (
        new_task(env.dir.path(), "分類 A"),
        new_task(env.dir.path(), "分類 B"),
    );
    env.store.insert(&t2).expect("t2");
    // 待機に入るまで tick（出来事 = outbox の行が 1 件出る、で待つ。計画なしの task だけでも通知が出る）。
    for _ in 0..200 {
        tick(&mut d);
        if !env
            .store
            .notification_pending()
            .expect("pending")
            .is_empty()
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(env.store.notification_pending().expect("pending").len(), 1);
    env.store.insert(&t1).expect("t1");
    for _ in 0..200 {
        tick(&mut d);
        if env.count("SELECT COUNT(*) FROM credential_waits") >= 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    for _ in 0..4 {
        tick(&mut d);
    }
    let w1 = format!("task:{}", t1.id);
    let w2 = format!("task:{}", t2.id);
    assert_eq!(env.wait_state(&w1), task_core::CredentialWaitState::Waiting);
    assert_eq!(env.wait_state(&w2), task_core::CredentialWaitState::Waiting);

    // 待機: 受信箱 item は 1 件（2 task を束ねる）。run は 0 本（worker・CoS・LLM とも）。
    assert_eq!(env.count("SELECT COUNT(*) FROM credential_waits"), 2);
    let items = env.inbox_items();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].blocking.tasks.len(), 2);
    assert_eq!(env.count("SELECT COUNT(*) FROM runs"), 0);
    assert_eq!(env.count("SELECT COUNT(*) FROM chat_runs"), 0);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM chat_messages WHERE role='system'"),
        0
    );
    assert_eq!(
        env.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state IN ('pending','running')"),
        0
    );

    // 通知: outbox の行は 1 件。偽 webhook への POST も 1 件で、再 tick・再 notifier でも増えない。
    assert_eq!(env.store.notification_pending().expect("pending").len(), 1);
    assert!(env.notifier_tick().await, "one POST");
    for _ in 0..3 {
        tick(&mut d);
        assert!(!env.notifier_tick().await, "no second POST");
    }
    // 通知は最初の待機（分類 B）で 1 件出る。あとから同じ秘密を待った分類 A では増えない。
    let bodies = env.webhook_bodies();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    for needle in [
        PURPOSE,
        "分類 B",
        "/accounts",
        "http://celeris.test/accounts",
        "自動で続行",
    ] {
        assert!(bodies[0].contains(needle), "{needle}: {}", bodies[0]);
    }
    assert!(!bodies[0].contains(DUMMY));
    assert_eq!(
        verifier.calls.load(Ordering::SeqCst),
        0,
        "未登録の間は検証しない"
    );

    // 登録（PUT 相当）→ durable event → mock 検証 1 回 → 同じ work unit が一度だけ再キュー。
    env.put_secret(DUMMY);
    for _ in 0..3 {
        tick(&mut d);
    }
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *verifier.seen.lock().expect("seen"),
        vec![DUMMY.to_string()]
    );
    assert_eq!(env.wait_state(&w1), task_core::CredentialWaitState::Resumed);
    assert_eq!(env.wait_state(&w2), task_core::CredentialWaitState::Resumed);
    // 重複 event（同じ fingerprint の再保存）と再 tick でも二重再開しない・再検証しない。
    env.put_secret(DUMMY);
    for _ in 0..4 {
        tick(&mut d);
    }
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
    assert!(env.inbox_items().is_empty(), "登録で受信箱 item は消える");
    assert!(!env.notifier_tick().await, "再開後に通知は増えない");
    assert_eq!(env.webhook_bodies().len(), 1);

    // 値は DB・event・通知本文のどこにも出ない。
    for body in env.webhook_bodies() {
        assert!(!body.contains(DUMMY));
    }
    let conn = rusqlite::Connection::open(&env.config.db.path).expect("db");
    let leaked: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM events WHERE json LIKE ?1) + (SELECT COUNT(*) FROM notifications WHERE body LIKE ?1)",
            [format!("%{DUMMY}%")],
            |r| r.get(0),
        )
        .expect("leak scan");
    assert_eq!(leaked, 0);
}
