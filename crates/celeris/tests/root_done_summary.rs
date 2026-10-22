//! ADR 2026-10-15-root-done-cos-summary の通し試験: root task の done で CoS の run が 1 回、スレッドへの
//! 書込みが 1 回、Discord への通知が 1 回だけ起きる。
//!
//! 本番の組み立て経路（`build_dispatcher`）で dispatcher を作り、CoS は偽の adapter（stub script）、
//! Discord は 127.0.0.1 の偽 webhook server で受ける（外部ネットワークに出ない）。通知の送信は
//! daemon の notifier tick と同じ順（`select_routes_batch` → `render` → `post_webhook` → `record`）。
//! 待ちは DB の状態（run の終了・通知の pending）を出来事として待つ。CPU 負荷は掛けない。
//! 再起動は `build_dispatcher` をもう一度呼んで同じ DB を再走査する（tick 状態は作り直し）。
//!
//! 時計: dispatcher の注入時計は task-dispatch の内部（`test_now`）で外から差せない。この試験の結果は
//! 時間に依存しない（fallback は run の失敗で即決まり、待ち時間の経過に頼らない）。

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use celeris::notify::{self, NotifyConfig, SendResult};
use celeris::{ClusterMasters, Config, build_dispatcher};
use task_core::NoticeStore;
use task_core::chat::{ChatMessageQuery, ChatMessageRole};
use task_core::{
    Budget, Check, Criterion, NotificationStore, SqliteStore, Status, Task, TaskId, TaskKind,
    TaskStore, Tier, Trigger, WorkerHint, WorkspaceSpec,
};
use task_dispatch::Dispatcher;
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const WEB: &str = "http://celeris.test";
const COS_OK: &str = "cat >/dev/null; echo '{\"type\":\"done\",\"summary\":\"まとめ: 調査の仕事を行い、成果が出た\",\"evidence\":[]}'";
const COS_FAIL: &str = "cat >/dev/null; echo cos-broken >&2; exit 1";

/// 偽の Discord webhook: 204 を返し、受けた本文を残す。
struct Hook {
    url: String,
    bodies: Arc<Mutex<Vec<String>>>,
    seen: tokio::sync::mpsc::UnboundedReceiver<()>,
}

async fn hook() -> Hook {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}/hook", listener.local_addr().expect("addr"));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let (tx, seen) = tokio::sync::mpsc::unbounded_channel();
    let kept = bodies.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut raw = Vec::new();
            let mut buf = vec![0; 8192];
            let body = loop {
                let n = stream.read(&mut buf).await.expect("read");
                if n == 0 {
                    break String::new();
                }
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).to_string();
                if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if rest.len() >= len {
                        break rest.to_string();
                    }
                }
            };
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await
                .expect("reply");
            kept.lock().expect("bodies").push(body);
            let _ = tx.send(());
        }
    });
    Hook { url, bodies, seen }
}

struct Env {
    dir: tempfile::TempDir,
    config: Config,
    store: SqliteStore,
    hook: Hook,
}

async fn env(cos_script: &str) -> Env {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let hook = hook().await;
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
    // `[api] listen` は CoS の起動条件（run が叩く API の URL）。待ち受けは作らない（build_dispatcher は bind しない）。
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
listen = "127.0.0.1:17999"
token_file = "api.token"

[notify]
web_base_url = "{WEB}"

[cos]
enabled = true
max_turns = 2
max_wall_secs = 30
route_candidates = [{{ source = "local", model = "model-1" }}]

[knowledge]
root = {knowledge:?}

[adapters.fake]
command = ["sh", "-c", {script:?}]

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
"#,
            knowledge = knowledge.display().to_string(),
            script = cos_script,
        ),
    )
    .expect("config");
    std::fs::write(root.join("api.token"), "root-done-test-token\n").expect("token");
    let config = Config::load(&config_path).expect("config load");
    let store = SqliteStore::open(&config.db.path).expect("store");
    Env {
        dir,
        config,
        store,
        hook,
    }
}

fn dispatcher(env: &Env) -> Dispatcher {
    build_dispatcher(&env.config, ClusterMasters::default()).expect("dispatcher")
}

fn new_task(dir: &Path, parent: Option<&Task>) -> Task {
    let id = TaskId::new();
    let now = OffsetDateTime::now_utc();
    let mut task = Task {
        harness_id: None,
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
        title: "調査の仕事".into(),
        objective: "調べてまとめる".into(),
        acceptance: vec![Criterion {
            text: "人が確かめる".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Reviewing,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: dir.join(id.to_string()),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
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
    };
    if let Some(parent) = parent {
        task.parent_id = Some(parent.id);
        task.tree = Some(task_core::TreeInfo::child_of(
            parent,
            task_core::ParentUnit {
                task_id: parent.id,
                plan_id: "plan".into(),
                unit_key: "c".into(),
                stage: "s1".into(),
                attempt: 1,
            },
            None,
        ));
    }
    task
}

impl Env {
    fn done_task(&self, parent: Option<&Task>) -> Task {
        let task = new_task(self.dir.path(), parent);
        self.store.create_task(&task, vec![]).expect("task");
        self.store
            .apply_transition(task.id, Trigger::ReviewPass, None)
            .expect("done");
        assert_eq!(
            self.store.get(task.id).expect("get").expect("task").status,
            Status::Done
        );
        task
    }

    fn count(&self, sql: &str) -> i64 {
        let conn = rusqlite::Connection::open(&self.config.db.path).expect("db");
        conn.query_row(sql, [], |r| r.get(0)).expect("count")
    }

    fn text(&self, sql: &str) -> String {
        let conn = rusqlite::Connection::open(&self.config.db.path).expect("db");
        conn.query_row(sql, [], |r| r.get(0)).expect("text")
    }

    fn cos_runs(&self) -> i64 {
        // root 完了の入力（client_message_id `root-done:…`）の run だけ。受信箱 triage の run は数えない。
        // ADR 2026-10-22-cos-fixed-threads D3: 書き込み先は案件か統括の CoS スレッド。
        self.count(
            "SELECT COUNT(*) FROM chat_runs r JOIN chat_messages m ON m.id=r.input_message_id \
             WHERE m.client_message_id LIKE 'root-done:%'",
        )
    }

    fn summary_rows(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM root_done_summaries")
    }

    fn notifications(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM notifications WHERE kind='root_done_summary'")
    }

    fn webhook_bodies(&self) -> Vec<String> {
        self.hook.bodies.lock().expect("bodies").clone()
    }

    /// スレッドの assistant 発言（CoS の書込み）の数。
    fn thread_writes(&self, thread: &str) -> usize {
        self.store
            .chat_message_list(
                thread,
                &ChatMessageQuery {
                    limit: Some(200),
                    ..Default::default()
                },
            )
            .expect("messages")
            .items
            .into_iter()
            .filter(|m| m.role == ChatMessageRole::Assistant)
            .count()
    }

    /// daemon の notifier tick（1 通まで）。POST したかを返す。
    async fn notifier_tick(&mut self) -> bool {
        // この試験が見るのは root_done_summary だけ。既存の受信箱 triage の通知（cos_fallback 等）は別経路で、
        // 再起動後の triage の待ち時間次第で現れるので数えない。
        let pending: Vec<_> = self
            .store
            .notification_pending()
            .expect("pending")
            .into_iter()
            .filter(|n| n.kind == task_core::NotificationKind::RootDoneSummary)
            .collect();
        let Some(mut batch) = notify::select_routes_batch(&pending) else {
            return false;
        };
        let row = pending
            .iter()
            .find(|n| batch.ids.contains(&n.id))
            .expect("row")
            .clone();
        let cfg = NotifyConfig {
            web_base_url: Some(WEB.into()),
            ..Default::default()
        };
        batch.content = notify::triage::render(&row, &cfg).expect("render");
        let client = notify::client().expect("client");
        let outcome = notify::post_webhook(&client, &self.hook.url, &batch.content).await;
        self.hook
            .seen
            .recv()
            .await
            .expect("the fake webhook saw the post");
        notify::record(
            &self.store,
            &pending,
            &SendResult {
                ids: batch.ids,
                outcome,
            },
            OffsetDateTime::now_utc(),
        )
        .expect("record");
        true
    }

    /// 通知の行が出るまで dispatcher を tick する（run の終了は DB の状態で待つ。時間では待たない）。
    async fn tick_until_notification(&self, d: &mut Dispatcher) {
        tokio::time::timeout(Duration::from_secs(60), async {
            while self.notifications() == 0 {
                d.tick().expect("tick");
                tokio::task::yield_now().await;
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("a root_done_summary notification appeared");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn root_done_runs_cos_once_and_notifies_once() {
    let mut env = env(COS_OK).await;
    let mut d = dispatcher(&env);
    d.tick().expect("first tick"); // 導入: cursor は現在の末尾から
    let root = env.done_task(None);
    env.tick_until_notification(&mut d).await;
    for _ in 0..3 {
        d.tick().expect("tick");
    }
    assert!(env.notifier_tick().await, "one POST");
    for _ in 0..3 {
        assert!(!env.notifier_tick().await, "no second POST");
        d.tick().expect("tick");
    }
    let thread = env.text("SELECT thread_id FROM root_done_summaries");
    let bodies = env.webhook_bodies();
    assert_eq!(bodies.len(), 1, "Discord: {bodies:?}");
    assert!(
        bodies[0].contains("まとめ: 調査の仕事を行い"),
        "{}",
        bodies[0]
    );
    assert!(!bodies[0].contains("決定的な短文"), "{}", bodies[0]);
    assert!(
        bodies[0].contains(&format!("{WEB}/?thread={thread}")),
        "{}",
        bodies[0]
    );
    assert!(
        bodies[0].contains(&format!("{WEB}/tasks/{}", root.id)),
        "{}",
        bodies[0]
    );
    assert_eq!(env.summary_rows(), 1);
    assert_eq!(env.cos_runs(), 1, "CoS run");
    assert_eq!(env.thread_writes(&thread), 1, "thread write");
    assert_eq!(
        env.text("SELECT state FROM root_done_summaries"),
        "notified"
    );
    assert_eq!(
        env.count("SELECT COUNT(*) FROM notifications WHERE kind='task_ready'"),
        0,
        "the old TaskReady path stays silent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_done_emits_nothing() {
    let mut env = env(COS_OK).await;
    let mut d = dispatcher(&env);
    d.tick().expect("first tick");
    let mut parent = new_task(env.dir.path(), None);
    parent.status = Status::Running;
    env.store.create_task(&parent, vec![]).expect("parent");
    env.done_task(Some(&parent));
    for _ in 0..5 {
        d.tick().expect("tick");
        tokio::task::yield_now().await;
    }
    assert_eq!(env.summary_rows(), 0);
    assert_eq!(env.notifications(), 0);
    assert!(!env.notifier_tick().await, "no POST");
    assert_eq!(env.cos_runs(), 0, "no CoS run");
    assert_eq!(
        env.count("SELECT COUNT(*) FROM chat_threads WHERE title LIKE '完了: %'"),
        0
    );
    assert!(env.webhook_bodies().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_does_not_duplicate() {
    let mut env = env(COS_OK).await;
    let mut d = dispatcher(&env);
    d.tick().expect("first tick");
    env.done_task(None);
    env.tick_until_notification(&mut d).await;
    assert!(env.notifier_tick().await, "one POST");
    drop(d);
    // 再起動: tick 状態を作り直して同じ DB を再走査する。cursor を巻き戻した再走査（最悪の重複）も含める。
    for rewind in [false, true] {
        let mut d = dispatcher(&env);
        if rewind {
            env.store
                .feed_cursor_set("root_done_summary", "0")
                .expect("rewind cursor");
        }
        for _ in 0..5 {
            d.tick().expect("tick after restart");
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            !env.notifier_tick().await,
            "no POST after restart (rewind={rewind})"
        );
    }
    assert_eq!(env.webhook_bodies().len(), 1, "Discord once");
    assert_eq!(env.summary_rows(), 1);
    assert_eq!(env.notifications(), 1);
    assert_eq!(env.cos_runs(), 1, "CoS ran once");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cos_failure_uses_fallback_notice() {
    let mut env = env(COS_FAIL).await;
    let mut d = dispatcher(&env);
    d.tick().expect("first tick");
    let root = env.done_task(None);
    env.tick_until_notification(&mut d).await;
    for _ in 0..3 {
        d.tick().expect("tick");
    }
    assert!(env.notifier_tick().await, "one POST");
    assert!(!env.notifier_tick().await, "no second POST");
    let bodies = env.webhook_bodies();
    assert_eq!(bodies.len(), 1, "Discord: {bodies:?}");
    assert!(bodies[0].contains("決定的な短文"), "{}", bodies[0]);
    assert!(bodies[0].contains("調査の仕事"), "{}", bodies[0]);
    assert!(
        bodies[0].contains(&format!("{WEB}/tasks/{}", root.id)),
        "{}",
        bodies[0]
    );
    assert_eq!(
        env.text("SELECT state FROM root_done_summaries"),
        "fallback"
    );
    assert_eq!(env.notifications(), 1);
    assert_eq!(
        env.count(
            "SELECT COUNT(*) FROM notifications WHERE kind='root_done_summary' AND key LIKE '%:%'"
        ),
        1
    );
}
