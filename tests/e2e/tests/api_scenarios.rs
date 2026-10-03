//! DESIGN §6 Phase 9 の受け入れ 4〜8（ADR-0013、`docs/api/v1/gui-api.md`）のうち、実バイナリ `celeris`（`[api]` 有効）/ `celerisctl` と
//! fake ワーカー（`sh` スクリプト）と `curl` で再現するもの。接続先は 127.0.0.1 だけで、外部ネットワークに出ない。
//!
//! 4. `[api]` が無ければリッスンしない。有効なら `/health` が `api_version` と `schema_version` を返す
//! 5. approve / reject / answer / cancel / タスク作成 / plan が状態機械を通る。無効な遷移と `expected_status` の不一致は 409（problem+json）
//! 6. SSE 購読中の `celerisctl add` が 2 秒以内に `Created` として届き、`Last-Event-ID` での再接続で取りこぼさない
//! 7. レート制限シナリオで `/daemon` に実行中の run と cooldown が現れ、`ProviderThrottled` がイベントに残る
//! 8. loopback 以外で `token_file` 無しは設定エラー、許可されない `Host` は 400、ワークスペース外の成果物は 403、`env` の値は応答に出ない

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use task_core::{ArtifactRef, Event, SCHEMA_VERSION, SqliteStore, Status, Task, TaskId, TaskStore};
use task_core::{RunIndexRole, RunIndexStatus, RunRow};

/// worker run の印（`task_worker::db_guard::WORKER_DB_GUARD_ENV`、ADR-0126 A1-1）。
const WORKER_DB_GUARD_ENV: &str = "CELERIS_WORKER_DB_GUARD";

fn bin(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug_dir = exe.parent().unwrap().parent().unwrap();
    let path = debug_dir.join(name);
    assert!(
        path.exists(),
        "{} not found; run `cargo test --workspace`",
        path.display()
    );
    path
}

/// celeris に渡すポートを予約する。`Env` が持ち続け、並走する別のテストの celeris と
/// 同じポートを共有しない（celeris は `SO_REUSEPORT` で bind する。`e2e::PortReservation`）。
fn reserve_port() -> e2e::PortReservation {
    e2e::PortReservation::new().unwrap()
}

fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cond()
}

/// 固定の wall-clock 期限ではなく、`count()` が増え続ける間だけ待つ出来事待ち（agent-docs/guides/testing.md 方法 2）。
/// `target` に達し次第 true を返す。`stall_limit` の間進捗が無ければ打ち切り、`overall_limit` は安全弁。
fn wait_for_progress(
    overall_limit: Duration,
    stall_limit: Duration,
    target: usize,
    mut count: impl FnMut() -> usize,
) -> bool {
    let start = Instant::now();
    let mut last = count();
    let mut last_change = Instant::now();
    loop {
        let now_count = count();
        if now_count >= target {
            return true;
        }
        if now_count > last {
            last = now_count;
            last_change = Instant::now();
        } else if last_change.elapsed() >= stall_limit {
            return false;
        }
        if start.elapsed() >= overall_limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// drop で kill する子プロセス（celeris / SSE の curl）。
struct Proc {
    child: Child,
    log: PathBuf,
}

impl Proc {
    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Resp {
    status: u16,
    /// 応答のヘッダ行（`Name: value`）。
    headers: Vec<String>,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {} {}", self.status, self.body))
    }

    /// ヘッダ名は大文字小文字を区別せず、値はそのまま返す。
    fn header(&self, name: &str) -> Option<String> {
        self.headers.iter().find_map(|h| {
            let (n, v) = h.split_once(':')?;
            n.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    }

    /// problem+json の `code` を確かめる。
    fn assert_problem(&self, status: u16, code: &str) -> Value {
        assert_eq!(self.status, status, "{}", self.body);
        let ct = self.header("content-type").unwrap_or_default();
        assert!(
            ct.starts_with("application/problem+json"),
            "content-type {ct}: {}",
            self.body
        );
        let v = self.json();
        assert_eq!(v["code"], code, "{v}");
        assert_eq!(v["status"], status, "{v}");
        v
    }
}

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
    store: Arc<SqliteStore>,
    port: u16,
    _port: e2e::PortReservation,
    token: Option<String>,
}

impl Env {
    fn new() -> Self {
        let reserved = reserve_port();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        for name in ["home", "config-home", "state", "cache"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        let db = root.join("celeris.sqlite3");
        let store = Arc::new(SqliteStore::open(&db).unwrap());
        Self {
            _tmp: tmp,
            root,
            db,
            store,
            port: reserved.port(),
            _port: reserved,
            token: None,
        }
    }

    fn write_script(&self, body: &str) -> PathBuf {
        let path = self.root.join("fake-worker.sh");
        std::fs::write(&path, format!("#!/bin/sh\nset -u\n{body}\n")).unwrap();
        path
    }

    /// `api` は `[api]` 節の本体（空なら節を書かない）。`provider_env` は `[[providers]]` の `env` の TOML インライン表。
    fn write_config(&self, script: &Path, api: &str, provider_env: &str) -> PathBuf {
        let path = self.root.join("config.toml");
        // The worker db guard stays on (ADR-0126): this daemon uses only a private test DB,
        // state dir and test token, so inside a guarded worker run it is exempt from the
        // user namespace probe; outside one it probes as in production.
        let api_section = if api.is_empty() {
            String::new()
        } else {
            format!("[api]\n{api}\n")
        };
        let text = format!(
            r#"workspace_root = "workspaces"
tick_ms = 50
max_concurrency = 2
lease_grace_secs = 60
idle_timeout_secs = 30
kill_grace_secs = 1
review_timeout_secs = 30
retry_backoff_base_secs = 0

[db]
path = "celeris.sqlite3"

{api_section}
[adapters.fake]
command = ["sh", "{script}"]
env = {{ ADAPTER_SECRET = "adapter-s3cr3t-value" }}

[[providers]]
id = "fake-local"
adapter = "fake"
tiers = ["frontier", "standard", "cheap"]
concurrency = 2
model = "fake"
env = {provider_env}
"#,
            script = script.display(),
            provider_env = if provider_env.is_empty() {
                "{}"
            } else {
                provider_env
            },
        );
        std::fs::write(&path, text).unwrap();
        path
    }

    fn command(&self, name: &str) -> Command {
        let mut cmd = Command::new(bin(name));
        // A worker run can carry production state and credentials in CELERIS_*. Keep only the
        // worker run marker: the guard needs it to exempt this test DB (ADR-0126 A3).
        for (key, _) in std::env::vars_os() {
            let key_str = key.to_string_lossy();
            if key_str.starts_with("CELERIS_") && key_str != WORKER_DB_GUARD_ENV {
                cmd.env_remove(key);
            }
        }
        cmd.env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config-home"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("CELERIS_STATE_DIR", self.root.join("state"));
        cmd
    }

    fn api_listen(&self) -> String {
        format!("listen = \"127.0.0.1:{}\"", self.port)
    }

    /// ADR-0044 §5 Phase 53 追記（Phase 55）: **変更を伴う API はすべて管理系（bearer 必須）**。
    /// `[api]` に `token_file` を足し、以後の要求に `Authorization: Bearer` を付ける。
    fn api_listen_with_token(&mut self) -> String {
        // テストごとに違う token にする。万一別のテストの celeris に要求が届いても、黙って
        // 別の DB を書き換えずに 401 で表に出る。
        let token = format!("tok-e2e-phase55-{}-{}", std::process::id(), self.port);
        std::fs::write(self.root.join("api.token"), format!("{token}\n")).unwrap();
        self.token = Some(token);
        format!("{}\ntoken_file = \"api.token\"", self.api_listen())
    }

    fn workspace(&self, name: &str) -> String {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    fn celerisctl(&self, args: &[&str]) -> String {
        let out = self
            .command("celerisctl")
            .arg("--db")
            .arg(&self.db)
            .args(args)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "celerisctl {args:?} failed: {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        stdout
    }

    fn add(&self, args: &[&str]) -> TaskId {
        let mut full = vec!["add", "--objective", "phase 9 api scenario"];
        full.extend_from_slice(args);
        self.celerisctl(&full).trim().parse().unwrap()
    }

    fn start_celeris(&self, config: &Path) -> Proc {
        static STARTS: AtomicUsize = AtomicUsize::new(0);
        let log = self.root.join(format!(
            "celeris-{}.log",
            STARTS.fetch_add(1, Ordering::Relaxed)
        ));
        let child = self
            .command("celeris")
            .args(["--config", config.to_str().unwrap(), "--log-format", "text"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        Proc { child, log }
    }

    /// `/health` が 200 を返すまで待つ（無認証）。
    fn wait_api(&self, daemon: &mut Proc) {
        let listening = wait_until(Duration::from_secs(120), || {
            if let Ok(Some(status)) = daemon.child.try_wait() {
                panic!("celeris exited early with {status}\n{}", daemon.log_text());
            }
            std::net::TcpStream::connect(("127.0.0.1", self.port)).is_ok()
        });
        assert!(listening, "API did not listen\n{}", daemon.log_text());
        let healthy = wait_until(Duration::from_secs(120), || {
            if let Ok(Some(status)) = daemon.child.try_wait() {
                panic!(
                    "celeris exited before health check with {status}\n{}",
                    daemon.log_text()
                );
            }
            self.request("GET", "/health", None, &[]).status == 200
        });
        assert!(healthy, "API did not become healthy\n{}", daemon.log_text());
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/api/v1{path}", self.port)
    }

    /// `curl` で 1 要求。接続できなければ `status = 0`。`token` があれば `Authorization` を付ける（`headers` に明示があればそちら）。
    fn request(&self, method: &str, path: &str, body: Option<&str>, headers: &[&str]) -> Resp {
        let mut cmd = Command::new("curl");
        // -q must be first so curl does not read the runner's ~/.curlrc.
        cmd.args(["-q", "--noproxy", "*"]);
        cmd.args([
            "-s",
            "-S",
            "-D",
            "-",
            "--max-time",
            "10",
            "-H",
            "Expect:",
            "-X",
            method,
        ]);
        let has = |name: &str| {
            headers
                .iter()
                .any(|h| h.to_ascii_lowercase().starts_with(&format!("{name}:")))
        };
        if let Some(token) = &self.token
            && !has("authorization")
        {
            cmd.args(["-H", &format!("Authorization: Bearer {token}")]);
        }
        for h in headers {
            cmd.args(["-H", h]);
        }
        if let Some(body) = body {
            if !has("content-type") {
                cmd.args(["-H", "Content-Type: application/json"]);
            }
            cmd.args(["--data-binary", body]);
        }
        let out = cmd.arg(self.url(path)).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let Some((head, body)) = text.split_once("\r\n\r\n") else {
            return Resp {
                status: 0,
                headers: vec![],
                body: String::from_utf8_lossy(&out.stderr).into_owned(),
            };
        };
        let mut lines = head.lines();
        let status = lines
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Resp {
            status,
            headers: lines.map(str::to_string).collect(),
            body: body.to_string(),
        }
    }

    fn get(&self, path: &str) -> Resp {
        self.request("GET", path, None, &[])
    }

    fn post(&self, path: &str, body: Value) -> Resp {
        self.request("POST", path, Some(&body.to_string()), &[])
    }

    fn task(&self, id: TaskId) -> Task {
        self.store.get(id).unwrap().unwrap()
    }

    fn events(&self, id: TaskId) -> Vec<Event> {
        self.store
            .events_for(id)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }

    fn replay_is_consistent(&self) {
        let out = self.celerisctl(&["replay"]);
        assert!(out.contains("replay: 0 mismatches"), "{out}");
    }
}

fn id_of(v: &Value) -> TaskId {
    v["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no id: {v}"))
        .parse()
        .unwrap()
}

/// SSE の本文を `(event, id, data)` に分ける。
fn sse_events(text: &str) -> Vec<(String, Option<u64>, Value)> {
    text.split("\n\n")
        .filter_map(|block| {
            let mut event = None;
            let mut id = None;
            let mut data = String::new();
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    event = Some(v.trim().to_string());
                } else if let Some(v) = line.strip_prefix("id:") {
                    id = v.trim().parse().ok();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push_str(v.trim_start());
                }
            }
            let data = serde_json::from_str(&data).ok()?;
            Some((event?, id, data))
        })
        .collect()
}

/// 受け入れ 4: `[api]` が無ければリッスンしない。有効にすると `/health` が版と版数を返す。
/// ADR-0126 A3: guard 有効の daemon が試験用の一時 DB で起動する。worker run の中（印あり）では
/// userns の probe をせず免除行を出し、印が無ければ `CELERIS_USERNS_TESTS=1` のときだけ probe 経路を確かめる。
#[test]
fn worker_guard_exempt_daemon_starts_on_a_test_db_with_the_guard_on() {
    let in_worker_run = std::env::var_os(WORKER_DB_GUARD_ENV).is_some_and(|v| !v.is_empty());
    let userns_opt_in = std::env::var("CELERIS_USERNS_TESTS").as_deref() == Ok("1");
    if !in_worker_run && !userns_opt_in {
        eprintln!(
            "SKIPPED (worker_guard_exempt): {WORKER_DB_GUARD_ENV} is not set (not in a worker run); \
             set CELERIS_USERNS_TESTS=1 to check the probe path (ADR-0126)"
        );
        return;
    }
    let env = Env::new();
    let script = env.write_script("exit 0");
    let config = env.write_config(&script, &env.api_listen(), "");
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(!text.contains("worker_read_only"), "{text}");
    let mut daemon = env.start_celeris(&config);
    env.wait_api(&mut daemon);
    let log = daemon.log_text();
    if in_worker_run {
        assert!(
            log.contains(
                "the daemon does not use the production DB/token; worker db guard not installed"
            ),
            "expected the ADR-0126 exemption line\n{log}"
        );
    } else {
        assert!(
            log.contains("worker runs see the db directory read-only"),
            "expected the guard to be installed after the probe\n{log}"
        );
    }
}

#[test]
fn api_is_off_by_default_and_health_reports_versions_when_enabled() {
    let env = Env::new();
    let script = env.write_script(
        "cat >/dev/null\necho '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'",
    );

    let config = env.write_config(&script, "", "");
    let mut daemon = env.start_celeris(&config);
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        daemon.child.try_wait().unwrap().is_none(),
        "celeris should keep running\n{}",
        daemon.log_text()
    );
    assert_eq!(
        env.get("/health").status,
        0,
        "nothing listens without [api]"
    );
    drop(daemon);

    let config = env.write_config(&script, &env.api_listen(), "");
    let mut daemon = env.start_celeris(&config);
    env.wait_api(&mut daemon);
    let health = env.get("/health");
    assert_eq!(health.status, 200, "{}", health.body);
    let v = health.json();
    assert_eq!(v["api_version"], "1", "{v}");
    assert_eq!(v["schema_version"], json!(SCHEMA_VERSION), "{v}");
    assert_eq!(v["db"]["journal_mode"], "wal", "{v}");
    assert_eq!(health.header("cache-control").as_deref(), Some("no-store"));
    assert!(
        health.header("access-control-allow-origin").is_none(),
        "no CORS headers"
    );

    // 最初の tick の後はメモリ上のスナップショットが見える（DB を読まない。ADR-0013 D4）。
    assert!(wait_until(Duration::from_secs(120), || !env
        .get("/daemon")
        .json()["snapshot"]
        .is_null()));
    let snap = env.get("/daemon").json()["snapshot"].clone();
    assert_eq!(snap["providers"][0]["id"], "fake-local", "{snap}");
    assert_eq!(snap["tick_ms"], 50, "{snap}");
    assert_eq!(
        env.get("/config").json()["db"],
        env.db.to_string_lossy().as_ref()
    );
    assert_eq!(env.get("/no-such-endpoint").status, 404);
}

/// 受け入れ 5: API からの作成・承認・却下・回答・取消・plan が状態機械を通る。無効な遷移と `expected_status` 不一致は 409。
#[test]
fn api_mutations_go_through_the_state_machine() {
    let mut env = Env::new();
    let script = env.write_script(
        r#"input=$(cat)
case "$input" in
  *'"answers"'*) echo '{"type":"done","summary":"used the answer","evidence":[]}' ;;
  *) echo '{"type":"question","text":"which version should I target?"}' ;;
esac"#,
    );
    // ADR-0044 Phase 53 追記（Phase 55）: 変更系はトークンが要る。
    let api = env.api_listen_with_token();
    let config = env.write_config(&script, &api, "");
    let mut daemon = env.start_celeris(&config);
    env.wait_api(&mut daemon);
    let ws = env.workspace("ws-q");

    // 作成。ADR-0044 D1（Phase 53）: **人が作ったタスクは `ready`**（Go を挟まない）。
    // `status: "draft"` を明示したときだけ従来どおり draft で止まる。
    let drafted = env.post(
        "/tasks",
        json!({"title": "draft on purpose", "objective": "api", "acceptance": [{"type": "human", "text": "t"}, {"type": "artifact_exists", "name": "result.md"}], "status": "draft"}),
    );
    assert_eq!(drafted.status, 201, "{}", drafted.body);
    assert_eq!(drafted.json()["status"], "draft");
    let created = env.post(
        "/tasks",
        json!({"title": "ask me", "objective": "api", "acceptance": [{"type": "command", "cmd": "true"}], "workspace": ws, "status": "draft"}),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let task = created.json();
    let id = id_of(&task);
    assert_eq!(task["status"], "draft");
    assert!(
        created
            .header("location")
            .unwrap_or_default()
            .ends_with(&format!("/api/v1/tasks/{id}"))
    );
    env.post(
        "/tasks",
        json!({"title": "x", "objective": "y", "acceptance": []}),
    )
    .assert_problem(422, "validation");
    // ADR-0014 D3（P-G16）: 空白だけの title と存在しない親は 422（field 付き）で、何も作らない。
    let human =
        json!([{"type": "human", "text": "t"}, {"type": "artifact_exists", "name": "result.md"}]);
    let v = env
        .post(
            "/tasks",
            json!({"title": "  ", "objective": "y", "acceptance": human}),
        )
        .assert_problem(422, "validation");
    assert_eq!(v["errors"][0]["field"], "title", "{v}");
    let v = env
        .post("/tasks", json!({"title": "x", "objective": "y", "acceptance": human, "parent": TaskId::new().to_string()}))
        .assert_problem(422, "validation");
    assert_eq!(v["errors"][0]["field"], "parent", "{v}");
    // ADR-0014 D2（P-G15）: q は objective も対象（objective "api" を持つのは上で作った 2 件）。
    let found = env.get("/tasks?q=api").json();
    assert_eq!(found["total"].as_u64(), Some(2), "{found}");
    // ADR-0044 D4（Phase 53）: ラベル・種類・優先度で絞れる（AND）。
    let labelled = env.post(
        "/tasks",
        json!({"title": "board card", "objective": "board", "acceptance": [{"type": "human", "text": "t"}, {"type": "artifact_exists", "name": "result.md"}],
               "labels": ["infra", "urgent"], "category": "ops", "priority": "P0"}),
    );
    assert_eq!(labelled.status, 201, "{}", labelled.body);
    let labelled = labelled.json();
    assert_eq!(labelled["status"], "ready", "人が作ったタスクは ready");
    assert_eq!(labelled["priority"], 30);
    assert_eq!(labelled["category"], "ops");
    let by_label = env
        .get("/tasks?label=infra&label=urgent&category=ops&priority=P0")
        .json();
    assert_eq!(by_label["total"].as_u64(), Some(1), "{by_label}");
    assert_eq!(by_label["items"][0]["priority_label"], "P0", "{by_label}");
    assert_eq!(
        env.get("/tasks?label=infra&label=nope").json()["total"].as_u64(),
        Some(0)
    );
    env.post("/tasks", json!({"title": "x", "objective": "y", "acceptance": [{"type": "human", "text": "t"}, {"type": "artifact_exists", "name": "result.md"}], "bogus": 1}))
        .assert_problem(400, "bad_request");

    // expected_status の不一致は状態を変えずに 409 conflict。
    let conflict = env
        .post(
            &format!("/tasks/{id}/approve"),
            json!({"expected_status": "ready"}),
        )
        .assert_problem(409, "conflict");
    assert_eq!(
        (conflict["expected"].as_str(), conflict["actual"].as_str()),
        (Some("ready"), Some("draft")),
        "{conflict}"
    );
    assert_eq!(env.task(id).status, Status::Draft);

    let accepted = env.post(
        &format!("/tasks/{id}/approve"),
        json!({"expected_status": "draft"}),
    );
    assert_eq!(accepted.status, 200, "{}", accepted.body);
    assert_eq!(
        (
            accepted.json()["from"].clone(),
            accepted.json()["to"].clone()
        ),
        (json!("draft"), json!("ready"))
    );

    // 二度目の approve と execute への reject は状態機械が拒否する。
    let invalid = env
        .post(&format!("/tasks/{id}/approve"), json!({}))
        .assert_problem(409, "invalid_transition");
    assert_eq!(invalid["trigger"], "approve", "{invalid}");
    env.post(&format!("/tasks/{id}/reject"), json!({}))
        .assert_problem(409, "invalid_transition");

    // ワーカーの質問 → API で回答 → done。
    assert!(
        wait_until(Duration::from_secs(120), || env.task(id).status
            == Status::Blocked),
        "{:?}",
        env.events(id)
    );
    let detail = env.get(&format!("/tasks/{id}")).json();
    assert!(
        detail["actions"]
            .as_array()
            .unwrap()
            .contains(&json!("answer")),
        "{detail}"
    );
    env.post(&format!("/tasks/{id}/answer"), json!({"answer": "   "}))
        .assert_problem(422, "validation");
    let answered = env.post(
        &format!("/tasks/{id}/answer"),
        json!({"answer": "target v2", "expected_status": "blocked"}),
    );
    assert_eq!(answered.status, 200, "{}", answered.body);
    assert_eq!(answered.json()["to"], "ready");
    assert!(env.events(id).iter().any(|e| matches!(
        e,
        Event::Answered { question, answer } if question == "which version should I target?" && answer == "target v2"
    )));
    assert!(
        wait_until(Duration::from_secs(120), || env.task(id).status
            == Status::Done),
        "{:?}",
        env.events(id)
    );

    // Approval の承認と却下。
    let approval = |title: &str| {
        let r = env.post(
            "/tasks",
            json!({"title": title, "objective": "decide", "kind": "approval", "acceptance": [{"type": "human", "text": "ok?"}, {"type": "artifact_exists", "name": "result.md"}]}),
        );
        assert_eq!(r.status, 201, "{}", r.body);
        assert_eq!(r.json()["status"], "ready");
        id_of(&r.json())
    };
    let a = approval("approve me");
    let r = env.post(
        &format!("/tasks/{a}/approve"),
        json!({"note": "lgtm", "expected_status": "ready"}),
    );
    assert_eq!(
        (r.status, r.json()["to"].clone()),
        (200, json!("done")),
        "{}",
        r.body
    );
    assert!(
        env.events(a)
            .iter()
            .any(|e| matches!(e, Event::ApprovalDecided { approved: true, .. }))
    );
    let b = approval("reject me");
    let r = env.post(&format!("/tasks/{b}/reject"), json!({"note": "no"}));
    assert_eq!(
        (r.status, r.json()["to"].clone()),
        (200, json!("failed")),
        "{}",
        r.body
    );

    // cancel は非終端だけ（`status: "draft"` を明示して draft のまま止めておく。ADR-0044 D1）。
    let draft = id_of(&env.post("/tasks", json!({"title": "c", "objective": "o", "acceptance": [{"type": "human", "text": "t"}, {"type": "artifact_exists", "name": "result.md"}], "status": "draft"})).json());
    let r = env.post(
        &format!("/tasks/{draft}/cancel"),
        json!({"expected_status": "draft"}),
    );
    assert_eq!(
        (r.status, r.json()["to"].clone()),
        (200, json!("cancelled")),
        "{}",
        r.body
    );
    assert!(r.json()["cascaded"].is_array(), "{}", r.body);
    env.post(&format!("/tasks/{draft}/cancel"), json!({}))
        .assert_problem(409, "invalid_transition");
    env.post(&format!("/tasks/{}/cancel", TaskId::new()), json!({}))
        .assert_problem(404, "task_not_found");

    // ADR-0079 U-R6（Phase R5a）: `POST /plans` は 410（分解は root task の gate と planner）。
    env.post("/plans", json!({"goal": "split the work\nsecond line"}))
        .assert_problem(410, "removed_by_adr_0079");
    env.post("/plans", json!({"goal": "  "}))
        .assert_problem(410, "removed_by_adr_0079");

    // 変更系の前提（Content-Type と Origin）。
    let r = env.request(
        "POST",
        &format!("/tasks/{draft}/cancel"),
        Some("{}"),
        &["Content-Type: text/plain"],
    );
    r.assert_problem(415, "unsupported_media_type");
    let r = env.request(
        "POST",
        "/plans",
        Some(r#"{"goal":"g"}"#),
        &["Origin: http://evil.example"],
    );
    r.assert_problem(403, "origin_forbidden");

    let replay = env.post("/replay", json!({}));
    assert_eq!(
        (replay.status, replay.json()["mismatches"].clone()),
        (200, json!([])),
        "{}",
        replay.body
    );
    env.replay_is_consistent();
}

/// 受け入れ 6: SSE 購読中の `celerisctl add` が 2 秒以内に届き、`Last-Event-ID` で再接続しても取りこぼさない。
#[test]
fn sse_delivers_created_quickly_and_resumes_from_last_event_id() {
    let env = Env::new();
    let script = env.write_script(
        "cat >/dev/null\necho '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'",
    );
    let config = env.write_config(&script, &env.api_listen(), "");
    let mut daemon = env.start_celeris(&config);
    env.wait_api(&mut daemon);

    let subscribe = |name: &str, last_event_id: Option<u64>| {
        let log = env.root.join(name);
        let mut cmd = Command::new("curl");
        cmd.args(["-q", "--noproxy", "*", "-s", "-N", "--max-time", "30"]);
        if let Some(id) = last_event_id {
            cmd.args(["-H", &format!("Last-Event-ID: {id}")]);
        }
        let child = cmd
            .arg(env.url("/stream"))
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&log).unwrap())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let p = Proc { child, log };
        assert!(
            wait_until(Duration::from_secs(120), || p
                .log_text()
                .contains("event: hello")),
            "no hello: {}",
            p.log_text()
        );
        p
    };
    let created_id = |p: &Proc, task: TaskId| {
        sse_events(&p.log_text())
            .into_iter()
            .find_map(|(event, id, data)| {
                (event == "task.event"
                    && data["event"]["type"] == "created"
                    && data["task_id"] == task.to_string())
                .then_some(id)
            })
    };

    let first = subscribe("sse-1.txt", None);
    let started = Instant::now();
    let t1 = env.add(&["--title", "sse one", "--check-cmd", "true"]);
    assert!(
        wait_until(Duration::from_secs(2), || created_id(&first, t1).is_some()),
        "Created not delivered: {}",
        first.log_text()
    );
    let elapsed = started.elapsed();
    assert!(elapsed <= Duration::from_secs(2), "took {elapsed:?}");
    let last_seen = created_id(&first, t1)
        .flatten()
        .expect("task.event carries an id");
    let hello = sse_events(&first.log_text())
        .into_iter()
        .find(|(e, _, _)| e == "hello")
        .unwrap()
        .2;
    assert!(hello["cursor"].as_u64().unwrap() < last_seen, "{hello}");
    drop(first);

    // 切断中に起きたことを、Last-Event-ID からの再接続で全て受け取る。
    let t2 = env.add(&["--title", "sse two", "--check-cmd", "true"]);
    env.celerisctl(&["approve", &t1.to_string()]);
    let second = subscribe("sse-2.txt", Some(last_seen));
    assert!(
        wait_until(Duration::from_secs(120), || created_id(&second, t2)
            .is_some()),
        "missed Created: {}",
        second.log_text()
    );
    assert!(
        wait_until(Duration::from_secs(120), || sse_events(&second.log_text())
            .iter()
            .any(|(e, _, d)| e == "task.event"
                && d["task_id"] == t1.to_string()
                && d["event"]["type"] == "transitioned")),
        "missed the approval of t1: {}",
        second.log_text()
    );
    let events = sse_events(&second.log_text());
    let hello = &events.iter().find(|(e, _, _)| e == "hello").unwrap().2;
    assert_eq!(hello["cursor"].as_u64(), Some(last_seen), "{hello}");
    let ids: Vec<u64> = events
        .iter()
        .filter(|(e, _, _)| e == "task.event")
        .filter_map(|(_, id, _)| *id)
        .collect();
    assert!(
        ids.iter().all(|id| *id > last_seen),
        "replayed an already-seen event: {ids:?}"
    );
    assert!(
        ids.windows(2).all(|w| w[0] < w[1]),
        "ids must increase: {ids:?}"
    );
    drop(second);
    env.replay_is_consistent();
}

/// 受け入れ 7: レート制限で `/daemon` に実行中の run とプロバイダの cooldown が現れ、`ProviderThrottled` が残る。
#[test]
fn daemon_view_shows_in_flight_runs_and_cooldowns_and_throttle_is_recorded() {
    let env = Env::new();
    let script = env.write_script(
        r#"cat >/dev/null
if [ -f throttle-me ]; then
  echo '{"type":"error","message":"429 rate limited","retryable":true,"provider_failure":{"kind":"throttled","retry_after_secs":30}}'
else
  # 壁時計の sleep ではなく、試験が cooldown と in_flight を確かめ終えて release を置くまで待つ（agent-docs/guides/testing.md）。
  # 上限は試験が途中で落ちたときの保険。release が無ければ done を返さない。
  timeout 300 sh -c 'until test -f release; do sleep 0.1; done' || exit 1
  echo '{"type":"done","summary":"slow ok","evidence":[]}'
fi"#,
    );
    let config = env.write_config(&script, &env.api_listen(), "");
    let ws_slow = env.workspace("ws-slow");
    let ws_throttle = env.workspace("ws-throttle");
    std::fs::write(Path::new(&ws_throttle).join("throttle-me"), "").unwrap();
    let slow = env.add(&[
        "--title",
        "slow",
        "--check-cmd",
        "true",
        "--workspace",
        &ws_slow,
    ]);
    let throttled = env.add(&[
        "--title",
        "throttled",
        "--check-cmd",
        "true",
        "--max-retries",
        "0",
        "--workspace",
        &ws_throttle,
    ]);

    let mut daemon = env.start_celeris(&config);
    env.wait_api(&mut daemon);
    // 先に slow を走らせてから、同じプロバイダで throttled を走らせる（先に cooldown に入ると slow が dispatch されない）。
    env.celerisctl(&["approve", &slow.to_string()]);
    let in_flight_has_slow = |snap: &Value| {
        snap["in_flight"].as_array().is_some_and(|v| {
            v.iter().any(|r| {
                r["task_id"] == slow.to_string()
                    && r["kind"] == "worker"
                    && r["provider"] == "fake-local"
            })
        })
    };
    assert!(
        wait_until(Duration::from_secs(120), || in_flight_has_slow(
            &env.get("/daemon").json()["snapshot"]
        )),
        "slow run never appeared in /daemon: {}",
        env.get("/daemon").body
    );
    env.celerisctl(&["approve", &throttled.to_string()]);

    let mut snap = Value::Null;
    // slow は release を置くまで止まっているので、tick が遅くても in_flight のまま cooldown を待てる。
    let seen = wait_until(Duration::from_secs(120), || {
        snap = env.get("/daemon").json()["snapshot"].clone();
        snap["cooldowns"].as_array().is_some_and(|c| {
            c.iter()
                .any(|c| c["provider"] == "fake-local" && c["reason"] == "throttled")
        })
    });
    assert!(seen, "no cooldown in /daemon: {snap}");
    assert!(
        in_flight_has_slow(&snap),
        "the slow run is still in flight during the cooldown: {snap}"
    );
    assert!(
        snap["providers"][0]["in_use"].as_u64().unwrap() >= 1,
        "{snap}"
    );
    assert!(
        snap["cooldowns"][0]["until"].as_str().unwrap() > snap["last_tick_at"].as_str().unwrap(),
        "cooldown ends in the future: {snap}"
    );

    assert!(env.events(throttled).iter().any(|e| matches!(
        e,
        Event::ProviderThrottled { provider, reason, .. } if provider == "fake-local" && reason.as_deref() == Some("throttled")
    )), "{:?}", env.events(throttled));
    let page = env
        .get(&format!(
            "/events?task_id={throttled}&types=provider_throttled"
        ))
        .json();
    assert_eq!(page["items"].as_array().map(Vec::len), Some(1), "{page}");
    assert_eq!(
        env.task(throttled).attempts,
        0,
        "a requeue does not consume attempts"
    );
    // 確認が済んだので slow を終わらせる。
    std::fs::write(Path::new(&ws_slow).join("release"), "").unwrap();
    drop(daemon);
    env.replay_is_consistent();
}

/// 受け入れ 8: 設定エラー、Bearer、Host 検査、ワークスペース外の成果物、`env` の値を出さないこと。
#[test]
fn api_enforces_token_host_and_workspace_boundaries_without_leaking_env_values() {
    let mut env = Env::new();
    let script = env.write_script(
        "cat >/dev/null\necho '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'",
    );

    // loopback 以外で token_file 無し → 設定エラー（exit 2）。
    let config = env.write_config(&script, &format!("listen = \"0.0.0.0:{}\"", env.port), "");
    let mut bad = env.start_celeris(&config);
    assert!(
        wait_until(Duration::from_secs(120), || bad
            .child
            .try_wait()
            .unwrap()
            .is_some()),
        "celeris must refuse the config"
    );
    assert_eq!(bad.child.wait().unwrap().code(), Some(2));
    assert!(
        bad.log_text().contains("token_file is required"),
        "{}",
        bad.log_text()
    );
    drop(bad);

    std::fs::write(env.root.join("api.token"), "  tok-9f8e7d\n").unwrap();
    let api = format!("{}\ntoken_file = \"api.token\"", env.api_listen());
    let config = env.write_config(
        &script,
        &api,
        r#"{ SECRET_TOKEN = "s3cr3t-provider-value" }"#,
    );
    let mut daemon = env.start_celeris(&config);
    env.wait_api(&mut daemon);

    // Bearer。
    let r = env.get("/tasks");
    r.assert_problem(401, "unauthorized");
    assert!(
        r.header("www-authenticate")
            .unwrap_or_default()
            .starts_with("Bearer"),
        "{:?}",
        r.headers
    );
    env.request("GET", "/tasks", None, &["Authorization: Bearer wrong"])
        .assert_problem(401, "unauthorized");
    env.token = Some("tok-9f8e7d".into());
    assert_eq!(env.get("/tasks").status, 200);

    // Host 検査（/health も対象）。
    env.request("GET", "/tasks", None, &["Host: evil.example"])
        .assert_problem(400, "host_not_allowed");
    env.request("GET", "/health", None, &["Host: evil.example:80"])
        .assert_problem(400, "host_not_allowed");
    let localhost = format!("Host: localhost:{}", env.port);
    assert_eq!(
        env.request("GET", "/health", None, &[localhost.as_str()])
            .status,
        200
    );

    // env の値・トークン・token_file の場所は応答に出ない（キー名は出る）。
    for path in ["/config", "/providers", "/daemon", "/health"] {
        let r = env.get(path);
        assert_eq!(r.status, 200, "{path}: {}", r.body);
        for secret in [
            "s3cr3t-provider-value",
            "adapter-s3cr3t-value",
            "tok-9f8e7d",
            "api.token",
        ] {
            assert!(
                !r.body.contains(secret),
                "{path} leaks {secret}: {}",
                r.body
            );
        }
    }
    let cfg = env.get("/config").json();
    assert!(
        cfg.to_string().contains("SECRET_TOKEN"),
        "env keys are listed: {cfg}"
    );
    assert_eq!(cfg["api"]["auth_required"], true, "{cfg}");

    // ワークスペース外を指す成果物は 403（`..` と symlink の両方）。中のものは読める。
    let ws = env.workspace("ws-files");
    std::fs::write(env.root.join("outside.txt"), "top secret").unwrap();
    std::fs::write(Path::new(&ws).join("inside.txt"), "inside").unwrap();
    std::os::unix::fs::symlink(
        env.root.join("outside.txt"),
        Path::new(&ws).join("link.txt"),
    )
    .unwrap();
    let created = env.post("/tasks", json!({"title": "files", "objective": "o", "acceptance": [{"type": "human", "text": "t"}, {"type": "artifact_exists", "name": "inside.txt"}], "workspace": ws}));
    assert_eq!(created.status, 201, "{}", created.body);
    let id = id_of(&created.json());
    let run_id = TaskId::new().to_string();
    for path in ["inside.txt", "../outside.txt", "link.txt"] {
        let artifact = ArtifactRef {
            name: path.into(),
            path: path.into(),
            sha256: "0".repeat(64),
            kind: "file".into(),
            declared: true,
        };
        env.store
            .append_event(
                id,
                &Event::ArtifactProduced {
                    run_id: run_id.clone(),
                    artifact,
                },
            )
            .unwrap();
    }
    let inside = env.get(&format!("/tasks/{id}/artifacts/0"));
    assert_eq!((inside.status, inside.body.as_str()), (200, "inside"));
    for idx in [1, 2] {
        let r = env.get(&format!("/tasks/{id}/artifacts/{idx}"));
        let v = r.assert_problem(403, "path_forbidden");
        assert!(!r.body.contains("top secret"), "{v}");
    }
    let list = env.get(&format!("/tasks/{id}/artifacts")).json();
    assert_eq!(list["items"][1]["forbidden"], true, "{list}");
    env.get(&format!("/tasks/{id}/runs/not-a-ulid/stdout"))
        .assert_problem(403, "path_forbidden");
    // replay は tasks と events を別々に読むので、dispatch 中の celeris と競合しないよう止めてから比べる。
    drop(daemon);
    env.replay_is_consistent();
}

/// 受け入れ 2（Phase 9 監査の再現手順）: 速い tick で動く celeris に、別プロセスの `celerisctl` と API から書き込み続けても、
/// どちらにも `database is locked` が出ず celeris も落ちない（書き込みトランザクションは IMMEDIATE + busy_timeout）。
#[test]
fn writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked() {
    let mut env = Env::new();
    let script = env.write_script(
        "cat >/dev/null\necho '{\"type\":\"done\",\"summary\":\"ok\",\"evidence\":[]}'",
    );
    // ADR-0044 Phase 53 追記（Phase 55）: `POST /tasks` は管理系になったのでトークンを持たせる。
    let api = env.api_listen_with_token();
    let config = env.write_config(&script, &api, "");
    let text = std::fs::read_to_string(&config)
        .unwrap()
        .replace("tick_ms = 50", "tick_ms = 20");
    std::fs::write(&config, text).unwrap();
    let mut daemon = env.start_celeris(&config);
    env.wait_api(&mut daemon);
    let ws = env.workspace("ws-lock");

    let mut ids = Vec::new();
    for i in 0..150 {
        let id = env.add(&[
            "--title",
            &format!("cli {i}"),
            "--check-cmd",
            "true",
            "--workspace",
            &ws,
        ]);
        env.celerisctl(&["approve", &id.to_string()]);
        ids.push(id);
        if i % 5 == 0 {
            let r = env.post(
                "/tasks",
                json!({"title": format!("api {i}"), "objective": "o", "acceptance": [{"type": "command", "cmd": "true"}], "workspace": ws}),
            );
            assert_eq!(r.status, 201, "{}", r.body);
            // ADR-0044 D1（Phase 53）: `POST /tasks` は `ready` で作るので approve は要らない。
            let created = r.json();
            assert_eq!(created["status"], "ready", "{created}");
            ids.push(id_of(&created));
        }
        assert!(
            daemon.child.try_wait().unwrap().is_none(),
            "celeris exited at iteration {i}\n{}",
            daemon.log_text()
        );
    }
    assert!(
        wait_for_progress(
            Duration::from_secs(600),
            Duration::from_secs(60),
            ids.len(),
            || {
                ids.iter()
                    .filter(|id| env.task(**id).status == Status::Done)
                    .count()
            }
        ),
        "not all tasks finished (no progress for 60s, or overall 600s exceeded)\n{}",
        daemon.log_text()
    );
    assert!(
        daemon.child.try_wait().unwrap().is_none(),
        "celeris must still be running\n{}",
        daemon.log_text()
    );
    assert!(
        !daemon.log_text().contains("database is locked"),
        "{}",
        daemon.log_text()
    );
    env.replay_is_consistent();
}

/// Phase 12 第 2 段階（ADR-0018 実装メモ）8・9・11: `GET /clusters`、受信箱の `attention[].cluster_unavailable`、`TaskDetail.cluster`。
/// 多重接続が**無い**クラスタを使うので、ssh の設定も外部ネットワークも要らない。
#[test]
fn clusters_endpoint_inbox_attention_and_task_detail_show_an_offline_cluster() {
    let env = Env::new();
    let script = env
        .write_script(r#"cat >/dev/null; echo '{"type":"done","summary":"unused","evidence":[]}'"#);
    let config = env.write_config(&script, &env.api_listen(), "");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(
        "\n[[clusters]]\nid = \"offline\"\nhost = \"celeris-no-such-host-for-tests\"\nconcurrency = 1\n\
         setup = [\"true\"]\nenv = { SECRET_CLUSTER_VALUE = \"cluster-s3cr3t-value\" }\nrsync_excludes = [\".git/\"]\n",
    );
    std::fs::write(&config, text).unwrap();
    let remote = env.workspace("remote-project");
    let id = env.add(&[
        "--title",
        "offline work",
        "--check-cmd",
        "true",
        "--cluster",
        "offline",
        "--workspace",
        &remote,
    ]);

    let mut daemon = env.start_celeris(&config);
    env.wait_api(&mut daemon);
    env.celerisctl(&["approve", &id.to_string()]);

    // 8. /clusters: 設定 + 接続の有無 + cooldown。env の値は出ない。
    let mut clusters = Value::Null;
    let seen = wait_until(Duration::from_secs(120), || {
        clusters = env.get("/clusters").json();
        clusters["items"][0]["cooldown_until"].is_string()
    });
    assert!(
        seen,
        "the offline cluster never entered cooldown: {clusters}"
    );
    let c = &clusters["items"][0];
    assert_eq!(c["id"], "offline", "{clusters}");
    assert_eq!(c["host"], "celeris-no-such-host-for-tests");
    assert_eq!(c["connected"], false);
    assert_eq!(c["in_use"], 0);
    assert_eq!(c["concurrency"], 1);
    assert_eq!(c["sync"], "rsync");
    assert_eq!(c["delete_on_push"], false);
    assert_eq!(c["has_setup"], true);
    assert_eq!(c["env_keys"], json!(["SECRET_CLUSTER_VALUE"]));
    assert_eq!(c["rsync_excludes"], json!([".git/"]));
    assert!(
        c["cooldown_remaining_secs"].as_u64().is_some(),
        "{clusters}"
    );
    assert!(
        !clusters.to_string().contains("cluster-s3cr3t-value"),
        "env values must not leak: {clusters}"
    );
    let config_view = env.get("/config");
    assert!(
        !config_view.body.contains("cluster-s3cr3t-value"),
        "{}",
        config_view.body
    );
    assert!(
        !config_view.body.contains("\"setup\""),
        "{}",
        config_view.body
    );
    assert_eq!(
        config_view.json()["clusters"][0]["has_setup"],
        true,
        "{}",
        config_view.body
    );
    let snap = env.get("/daemon").json()["snapshot"].clone();
    assert_eq!(snap["clusters"][0]["connected"], false, "{snap}");
    assert_eq!(
        snap["clusters"][0]["host"], "celeris-no-such-host-for-tests",
        "{snap}"
    );

    // 9. 受信箱の注意: クラスタごとに 1 件、host と対象タスク数。
    let inbox = env.get("/inbox").json();
    let attention = inbox["attention"]
        .as_array()
        .unwrap_or_else(|| panic!("{inbox}"));
    let items: Vec<&Value> = attention
        .iter()
        .filter(|a| a["type"] == "cluster_unavailable")
        .collect();
    assert_eq!(items.len(), 1, "{inbox}");
    // ADR-0018 M8: 人のログイン待ちは経路なし（unroutable）ではないので、同じタスクが 2 件に出ない。
    assert!(
        !attention.iter().any(|a| a["type"] == "unroutable"),
        "{inbox}"
    );
    assert_eq!(items[0]["cluster"], "offline");
    assert_eq!(items[0]["host"], "celeris-no-such-host-for-tests");
    assert_eq!(items[0]["tasks"], 1);
    assert!(items[0]["at"].is_string(), "{inbox}");
    assert_eq!(
        inbox["counts"]["attention"].as_u64().unwrap() as usize,
        attention.len()
    );

    // 11. 詳細にクラスタが出る（API と `celerisctl show --json` の両方）。`workspace_dir` は手元の写し。
    let detail = env.get(&format!("/tasks/{id}")).json();
    assert_eq!(detail["cluster"], "offline", "{detail}");
    assert_eq!(
        detail["workspace_dir"],
        env.root
            .join("workspaces")
            .join(id.to_string())
            .to_string_lossy()
            .as_ref(),
        "{detail}"
    );
    assert_eq!(detail["task"]["workspace"]["path"], remote, "{detail}");
    let show: Value =
        serde_json::from_str(env.celerisctl(&["show", "--json", &id.to_string()]).trim()).unwrap();
    assert_eq!(show["cluster"], "offline", "{show}");

    // タスクは ready のまま（attempts も消費しない）。
    let t = env.task(id);
    assert_eq!((t.status, t.attempts), (Status::Ready, 0));
    drop(daemon);
    env.replay_is_consistent();
}

// Phase 3: real daemon/API, local fake harness and a signed GUI assertion.
// The socket path is deliberately inert: identity sealing does not contact the broker.
struct BrowserFixture {
    env: Env,
    daemon: Proc,
    signer: Ed25519KeyPair,
    task_a: TaskId,
    task_b: TaskId,
    run_a: String,
    run_b: String,
}

impl BrowserFixture {
    fn new() -> Self {
        let mut env = Env::new();
        let signer = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
        std::fs::write(env.root.join("gui.pub"), signer.public_key().as_ref()).unwrap();
        let script = env.write_script("sleep 30");
        let api = format!(
            "{}\nbrowser_attestation_public_key_file = \"gui.pub\"\nbrowser_credentiald_control_socket = \"broker/control.sock\"",
            env.api_listen_with_token()
        );
        let config = env.write_config(&script, &api, "");
        let seed = env.add(&["--title", "fixture", "--check-cmd", "true"]);
        let template = env.task(seed);
        let make_running = |id: TaskId, run: &str| {
            let mut task = template.clone();
            task.id = id;
            task.status = Status::Running;
            env.store.insert(&task).unwrap();
            env.store
                .run_index_start(RunRow {
                    run_id: run.into(),
                    task_id: id.to_string(),
                    work_unit_id: None,
                    role: RunIndexRole::Worker,
                    seq: 1,
                    status: RunIndexStatus::Running,
                    adapter: Some("fake".into()),
                    model: None,
                    account: None,
                    session_id: None,
                    checkpoint: None,
                    usage: None,
                    metrics: None,
                    started_at: "2026-09-29T00:00:00Z".into(),
                    finished_at: None,
                })
                .unwrap();
        };
        let task_a = TaskId::new();
        let task_b = TaskId::new();
        let run_a = "phase3-run-a".to_string();
        let run_b = "phase3-run-b".to_string();
        make_running(task_a, &run_a);
        make_running(task_b, &run_b);
        let mut daemon = env.start_celeris(&config);
        env.wait_api(&mut daemon);
        Self {
            env,
            daemon,
            signer,
            task_a,
            task_b,
            run_a,
            run_b,
        }
    }

    fn path(&self, task: TaskId, run: &str, kind: &str) -> String {
        format!("/tasks/{task}/browser/{kind}/{run}/browser-session")
    }

    fn assertion(&self, task: TaskId, run: &str, holder: &str) -> Value {
        let expires_at = time::OffsetDateTime::now_utc().unix_timestamp() + 20;
        let payload = json!({
            "task_id": task.to_string(), "run_id": run,
            "browser_session_id": "browser-session", "owner_session_id": holder,
            "owner_session": true, "origin_ok": true, "expires_at": expires_at
        })
        .to_string();
        let signature = self.signer.sign(payload.as_bytes());
        let hex: String = signature
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        json!({"payload": payload, "signature": hex})
    }

    fn control(&self, holder: &str, command: Value, version: u64, key: &str) -> Resp {
        self.env.post(
            &self.path(self.task_a, &self.run_a, "control"),
            json!({
                "assertion": self.assertion(self.task_a, &self.run_a, holder),
                "command": command, "expected_version": version, "idempotency_key": key
            }),
        )
    }
}

#[test]
fn phase3_live_grant_is_task_scoped_scrubbed_and_reconnects_from_last_seen() {
    let f = BrowserFixture::new();
    let a = f.path(f.task_a, &f.run_a, "live");
    let b = f.path(f.task_b, &f.run_b, "live");
    let assertion = f.assertion(f.task_a, &f.run_a, "owner-a");
    let grant = f
        .env
        .post(&format!("{a}/grant"), json!({"assertion": assertion}));
    assert_eq!(grant.status, 200, "{} {}", grant.body, f.daemon.log_text());
    let grant_id = grant.json()["grant_id"].as_str().unwrap().to_string();
    let request = json!({"assertion": assertion, "grant_id": grant_id});
    assert_eq!(
        f.env.post(&format!("{a}/check"), request.clone()).status,
        200
    );
    let cross_task = json!({
        "assertion": f.assertion(f.task_b, &f.run_b, "owner-a"),
        "grant_id": grant_id
    });
    let other = f.env.post(&format!("{b}/check"), cross_task.clone());
    assert_ne!(other.status, 200, "cross-task grant accepted");
    let other = f.env.post(&format!("{b}/read?after=0"), cross_task);
    assert_ne!(other.status, 200, "cross-task events exposed");
    let event = f.env.post(
        &format!("{a}/events"),
        json!({
            "kind": "url", "url": "https://example.test/page?token=top-secret&x=1"
        }),
    );
    assert_eq!(event.status, 200, "{}", event.body);
    let first = event.json()["seq"].as_u64().unwrap();
    let event = f.env.post(
        &format!("{a}/events"),
        json!({
            "kind": "console", "level": "info", "text": "cookie=top-secret token=top-secret"
        }),
    );
    assert_eq!(event.status, 200, "{}", event.body);
    let page = f.env.post(&format!("{a}/read?after=0"), request.clone());
    assert_eq!(page.status, 200, "{}", page.body);
    assert!(!page.body.contains("top-secret"), "{}", page.body);
    assert!(!page.body.contains("?token="), "{}", page.body);
    let persisted = std::fs::read(&f.env.db).unwrap();
    assert!(
        !persisted
            .windows(b"top-secret".len())
            .any(|w| w == b"top-secret")
    );
    let replay = f.env.post(&format!("{a}/read?after={first}"), request);
    assert_eq!(replay.status, 200, "{}", replay.body);
    assert_eq!(replay.json()["plan"]["kind"], "replay");
    assert_eq!(replay.json()["events"].as_array().unwrap().len(), 1);
}

/// ADR-0080 H3 / ADR-0081: while the worker's auth section is active the task-api refuses
/// takeover and renew (not only the GUI), revokes a held lease, and reopens after it ends.
#[test]
fn phase3_auth_section_refuses_takeover_and_renew_until_left() {
    let f = BrowserFixture::new();
    let path = f.path(f.task_a, &f.run_a, "control");
    let pause = f.control("owner-a", json!({"kind":"pause"}), 0, "as-pause");
    assert_eq!(pause.status, 200, "{}", pause.body);
    let v = pause.json()["version"].as_u64().unwrap();
    let held = f.control("owner-a", json!({"kind":"takeover"}), v, "as-take-1");
    assert_eq!(held.status, 200, "{}", held.body);
    assert_eq!(held.json()["phase"], "human_control");
    let v = held.json()["version"].as_u64().unwrap();
    let on = f
        .env
        .post(&format!("{path}/auth-section"), json!({"active": true}));
    assert_eq!(on.status, 200, "{}", on.body);
    assert_eq!(on.json()["auth_section"], true);
    assert_eq!(on.json()["phase"], "paused", "held lease is revoked");
    let status = f.env.get(&path);
    assert_eq!(status.json()["auth_section"], true, "{}", status.body);
    let v2 = on.json()["version"].as_u64().unwrap();
    assert!(v2 > v);
    f.control("owner-a", json!({"kind":"renew"}), v2, "as-renew")
        .assert_problem(409, "auth_section_active");
    f.control("owner-a", json!({"kind":"takeover"}), v2, "as-take-2")
        .assert_problem(409, "auth_section_active");
    let off = f
        .env
        .post(&format!("{path}/auth-section"), json!({"active": false}));
    assert_eq!(off.status, 200, "{}", off.body);
    assert_eq!(off.json()["auth_section"], false);
    let v3 = off.json()["version"].as_u64().unwrap();
    let again = f.control("owner-a", json!({"kind":"takeover"}), v3, "as-take-3");
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!(again.json()["phase"], "human_control");
}

#[test]
fn phase3_control_converges_rejects_competition_and_cancel_stops() {
    let f = BrowserFixture::new();
    let path = f.path(f.task_a, &f.run_a, "control");
    // in-flight の agent 操作は `agent/end` を呼ぶまで残る（daemon の tick は control 状態に触れない）。
    // pause が `pausing` で止まることは時刻に依らない。
    let begin = f.env.post(&format!("{path}/agent/begin"), json!({}));
    assert_eq!(begin.status, 200, "{}", begin.body);
    assert_eq!(begin.json()["in_flight"], 1, "{}", begin.body);
    let pause = f.control("owner-a", json!({"kind":"pause"}), 0, "pause-1");
    assert_eq!(pause.status, 200, "{}", pause.body);
    assert_eq!(pause.json()["phase"], "pausing", "{}", pause.body);
    let v = pause.json()["version"].as_u64().unwrap();
    f.control("owner-a", json!({"kind":"takeover"}), v, "early")
        .assert_problem(409, "not_converged");
    let end = f.env.post(&format!("{path}/agent/end"), json!({}));
    assert_eq!(end.status, 200, "{}", end.body);
    assert_eq!(end.json()["phase"], "paused");
    let converged_version = end.json()["version"].as_u64().unwrap();
    // 競合の検査の間に lease が切れないよう上限の 300 秒で取る。切れると owner-b の拒否が
    // 競合ではなく失効による version_conflict になり、検査が弱まる。
    let acquired = f.control(
        "owner-a",
        json!({"kind":"takeover", "ttl_secs":300}),
        converged_version,
        "take-1",
    );
    assert_eq!(acquired.status, 200, "{}", acquired.body);
    assert_eq!(acquired.json()["phase"], "human_control");
    let replay = f.control(
        "owner-a",
        json!({"kind":"takeover", "ttl_secs":300}),
        converged_version,
        "take-1",
    );
    assert_eq!(replay.status, 200, "{}", replay.body);
    assert_eq!(replay.json()["replayed"], true);
    assert_eq!(replay.json()["version"], acquired.json()["version"]);
    let version = acquired.json()["version"].as_u64().unwrap();
    f.control("owner-b", json!({"kind":"takeover"}), version, "take-2")
        .assert_problem(403, "not_lease_holder");
    f.control("owner-a", json!({"kind":"stop"}), v, "stale")
        .assert_problem(409, "version_conflict");
    // GET /control が lease の期限切れを適用する。paused と lease の消滅を観測して進む。
    let short = f.control(
        "owner-a",
        json!({"kind":"renew", "ttl_secs":1}),
        version,
        "renew-short",
    );
    assert_eq!(short.status, 200, "{}", short.body);
    let mut expired = f.env.get(&path);
    assert!(
        wait_until(Duration::from_secs(120), || {
            expired = f.env.get(&path);
            expired.status == 200
                && expired.json()["phase"] == "paused"
                && expired.json()["lease_holder"].is_null()
        }),
        "lease did not expire: {}",
        expired.body
    );
    assert_eq!(expired.status, 200, "{}", expired.body);
    assert_eq!(expired.json()["phase"], "paused");
    assert_eq!(expired.json()["agent_may_act"], false);
    let cancel = f.env.post(
        &format!("/tasks/{}/cancel", f.task_a),
        json!({"expected_status":"running"}),
    );
    assert_eq!(cancel.status, 200, "{}", cancel.body);
    let stopped = f.env.get(&path);
    assert_eq!(stopped.status, 200, "{}", stopped.body);
    assert_eq!(stopped.json()["phase"], "stopped");
}

#[test]
fn phase3_identity_register_revoke_delete_and_trusted_local_restore_denied() {
    let f = BrowserFixture::new();
    let base = "/browser/identities";
    let secret = "phase3-cookie-secret";
    let registered = f.env.post(base, json!({
        "identity_id":"identity-a", "project_id":"project-a", "origin":"https://example.test",
        "demand_confirmed_by":"human", "ttl_secs":60,
        "state":{"entries":[{"origin":"https://example.test","kind":"cookie","name":"session","value":secret}]}
    }));
    assert_eq!(
        registered.status,
        201,
        "{} {}",
        registered.body,
        f.daemon.log_text()
    );
    assert!(!registered.body.contains(secret));
    let listed = f.env.get(&format!("{base}?project_id=project-a"));
    assert_eq!(listed.status, 200, "{}", listed.body);
    assert_eq!(listed.json()["identities"][0]["identity_id"], "identity-a");
    let restore = f.env.post(
        &format!("{base}/identity-a/restore"),
        json!({
            "project_id":"project-a", "origin":"https://example.test"
        }),
    );
    restore.assert_problem(403, "isolation_required");
    let revoked = f.env.post(&format!("{base}/identity-a/revoke"), json!({}));
    assert_eq!(revoked.status, 200, "{}", revoked.body);
    assert_eq!(revoked.json()["identity"]["state"], "revoked");
    let deleted = f
        .env
        .request("DELETE", &format!("{base}/identity-a"), None, &[]);
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    assert_eq!(deleted.json()["identity"]["state"], "deleted");
    let persisted = std::fs::read(&f.env.db).unwrap();
    assert!(
        !persisted
            .windows(secret.len())
            .any(|w| w == secret.as_bytes())
    );
}
