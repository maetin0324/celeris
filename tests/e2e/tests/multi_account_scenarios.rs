//! 複数アカウント運用。fake ワーカー（`sh` スクリプト）と実バイナリ `celerisctl` / `celeris` だけで動き、
//! ネットワークに出ない。ルートの account ID は選択 event から確認する。
//!
//! 1. 先頭アカウントが並列度の上限に達すると、2 つ目のアカウントがあふれた分を実行する（P-20）
//! 2. レート制限を返したアカウントは cooldown になり、同じタスクは attempts を消費せず次のアカウントで実行される
//! 3. 設定に合うプロバイダが無いタスクは `celeris --until-idle` を止めない（P-33）

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use task_core::{Event, SqliteStore, Status, Task, TaskId, TaskStore};

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

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
    store: Arc<SqliteStore>,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let db = root.join("celeris.sqlite3");
        let store = Arc::new(SqliteStore::open(&db).unwrap());
        let account_root = root.join("claude-accounts");
        for id in ["a", "b"] {
            let dir = account_root.join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".credentials.json"), "{}").unwrap();
        }
        Self {
            _tmp: tmp,
            root,
            db,
            store,
        }
    }

    /// `script` を fake ワーカーにし、設定した source accounts を持つ config を書く。
    fn write_config(&self, script_body: &str, accounts: &str, max_runs: usize) -> PathBuf {
        let script = self.root.join("fake-worker.sh");
        std::fs::write(&script, format!("#!/bin/sh\nset -u\n{script_body}\n")).unwrap();
        let path = self.root.join("config.toml");
        let text = format!(
            r#"db = "celeris.sqlite3"
workspace_root = "workspaces"
tick_ms = 50
max_concurrency = 2
lease_grace_secs = 60
idle_timeout_secs = 30
kill_grace_secs = 1
review_timeout_secs = 30
retry_backoff_base_secs = 0

[accounts]
claude_dir = "{account_root}"
max_runs_per_account = {max_runs}

[adapters.fake]
command = ["sh", "{script}"]

[llm_sources.fake]
kind = "claude_oauth"
accounts = {accounts}
models = ["model-a", "model-b", "fake"]

[harnesses.fake]
kind = "fake"
api_formats = ["claude_oauth"]
source_kinds = ["claude_oauth"]

[harness_selection]
execution_class_defaults = {{}}

[[harness_selection.rules]]
harness = "fake"
required_capabilities = []

[route_defaults.fake]
candidates = [{{ source = "fake", model = "model-a" }}, {{ source = "fake", model = "model-b" }}, {{ source = "fake", model = "fake" }}]

"#,
            script = script.display(),
            account_root = self.root.join("claude-accounts").display(),
        );
        std::fs::write(&path, text).unwrap();
        path
    }

    fn workspace(&self, name: &str) -> String {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    fn celerisctl(&self, args: &[&str]) -> String {
        let out = Command::new(bin("celerisctl"))
            .env_remove("CELERIS_CONFIG")
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

    fn add_approved(&self, args: &[&str]) -> TaskId {
        let mut full = vec!["add", "--objective", "multi-account scenario"];
        full.extend_from_slice(args);
        let id: TaskId = self.celerisctl(&full).trim().parse().unwrap();
        self.celerisctl(&["approve", &id.to_string()]);
        id
    }

    fn run_celeris(&self, config: &std::path::Path, timeout: Duration) -> String {
        let log = self.root.join("celeris.log");
        let mut child = Command::new(bin("celeris"))
            .args([
                "--config",
                config.to_str().unwrap(),
                "--until-idle",
                "--max-ticks",
                "2000",
                "--log-format",
                "text",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                let text = std::fs::read_to_string(&log).unwrap_or_default();
                assert!(status.success(), "celeris exited with {status}\n{text}");
                return text;
            }
            if start.elapsed() > timeout {
                let _ = child.kill();
                let text = std::fs::read_to_string(&log).unwrap_or_default();
                panic!("celeris did not reach idle within {timeout:?}\n{text}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
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

    fn started_models(&self, id: TaskId) -> Vec<String> {
        self.events(id)
            .into_iter()
            .filter_map(|e| match e {
                Event::WorkerStarted { model, .. } => Some(model),
                _ => None,
            })
            .collect()
    }

    fn selected_accounts(&self, id: TaskId) -> Vec<String> {
        self.events(id)
            .into_iter()
            .filter_map(|event| match event {
                Event::LlmRouteSelected { account_id, .. } => Some(account_id),
                _ => None,
            })
            .collect()
    }

    fn transition_reasons(&self, id: TaskId) -> Vec<String> {
        self.events(id)
            .into_iter()
            .filter_map(|e| match e {
                Event::Transitioned { reason, .. } => Some(reason),
                _ => None,
            })
            .collect()
    }

    fn replay_is_consistent(&self) {
        let out = self.celerisctl(&["replay"]);
        assert!(out.contains("replay: 0 mismatches"), "{out}");
    }
}

/// source の 2 account は各 1 run に制限され、1 つ目が稼働中なら次の task は別 account を選ぶ。
#[test]
fn second_account_runs_the_overflow_when_the_first_is_at_capacity() {
    let env = Env::new();
    let config = env.write_config(
        r#"cat >/dev/null
# 両 task のワーカーが起動するまで、どちらも完了させない。
# DB guard は DB の親に新規ファイルを作らせない。既存の workspace 内を共有バリアに使う。
barrier=$(dirname "$0")/ws-1
account=$(basename "$CLAUDE_SECURESTORAGE_CONFIG_DIR")
printf '%s' "$account" > account.txt
touch "$barrier/started-$(basename "$PWD")"
remaining=400
while [ ! -f "$barrier/started-ws-1" ] || [ ! -f "$barrier/started-ws-2" ]; do
  remaining=$((remaining - 1))
  if [ "$remaining" -eq 0 ]; then
    rm -f "$barrier/started-$(basename "$PWD")"
    echo 'both tasks did not start concurrently' >&2
    exit 1
  fi
  sleep 0.05
done
echo '{"type":"done","summary":"ok","evidence":[]}'"#,
        "[\"a\", \"b\"]",
        1,
    );
    let ws1 = env.workspace("ws-1");
    let ws2 = env.workspace("ws-2");
    let t1 = env.add_approved(&[
        "--title",
        "one",
        // task の tier に依存せず、route admission が空き account を選ぶ。
        "--tier",
        "cheap",
        "--check-cmd",
        "true",
        "--workspace",
        &ws1,
    ]);
    let t2 = env.add_approved(&[
        "--title",
        "two",
        "--tier",
        "frontier",
        "--check-cmd",
        "true",
        "--workspace",
        &ws2,
    ]);

    env.run_celeris(&config, Duration::from_secs(60));
    // 両ワーカーがバリアを通過して完了すれば、並列に動いたと分かる。
    for id in [t1, t2] {
        assert_eq!(env.task(id).status, Status::Done, "{:?}", env.events(id));
    }
    let accounts: BTreeSet<String> = [t1, t2]
        .iter()
        .flat_map(|id| env.selected_accounts(*id))
        .collect();
    assert_eq!(accounts, BTreeSet::from(["a".to_string(), "b".to_string()]));
    env.replay_is_consistent();
}

/// 2: アカウント A がレート制限を返すと A は cooldown になり、同じタスクは attempts を消費せずアカウント B で実行される。
#[test]
fn throttled_account_falls_back_to_the_next_account() {
    let env = Env::new();
    let config = env.write_config(
r#"cat >/dev/null
account=$(basename "$CLAUDE_SECURESTORAGE_CONFIG_DIR")
if [ "$account" = "a" ] && [ ! -f attempts ]; then
  touch attempts
  echo '{"type":"error","message":"429 rate limited","retryable":true,"provider_failure":{"kind":"throttled","retry_after_secs":300}}'
else
  echo '{"type":"done","summary":"ok","evidence":[]}'
fi"#,
        "[\"a\", \"b\"]",
        2,
    );
    let ws = env.workspace("ws-fallback");
    let id = env.add_approved(&[
        "--title",
        "fallback",
        "--check-cmd",
        "true",
        "--max-retries",
        "0",
        "--workspace",
        &ws,
    ]);

    env.run_celeris(&config, Duration::from_secs(60));
    let t = env.task(id);
    assert_eq!(
        (t.status, t.attempts),
        (Status::Done, 0),
        "{:?}",
        env.events(id)
    );
    assert_eq!(
        env.transition_reasons(id),
        vec![
            "accept",
            "dispatch",
            "requeue",
            "dispatch",
            "worker_done",
            "review_pass"
        ]
    );
    assert_eq!(env.started_models(id), vec!["model-a", "model-a"]);
    assert_eq!(env.selected_accounts(id), vec!["a", "b"]);
    assert!(env.events(id).iter().any(|e| matches!(
        e,
        Event::LlmRouteSelected { account_id, .. } if account_id == "a"
    )));
    env.replay_is_consistent();
}

/// 3: worker_hint に合うプロバイダが設定に無いタスクは ready のまま残り、`--until-idle` は warn を出して終わる。
#[test]
fn task_without_a_matching_provider_does_not_block_until_idle() {
    let env = Env::new();
    let config = env.write_config(
        r#"cat >/dev/null; echo '{"type":"done","summary":"ok","evidence":[]}'"#,
        "[\"a\", \"b\"]",
        2,
    );
    let config_text = std::fs::read_to_string(&config).unwrap().replace(
        "candidates = [{ source = \"fake\", model = \"model-a\" }, { source = \"fake\", model = \"model-b\" }, { source = \"fake\", model = \"fake\" }]",
        "candidates = []",
    );
    std::fs::write(&config, config_text).unwrap();
    let ws = env.workspace("ws-unroutable");
    let id = env.add_approved(&[
        "--title",
        "cheap",
        "--tier",
        "cheap",
        "--check-cmd",
        "true",
        "--workspace",
        &ws,
    ]);

    let log = env.run_celeris(&config, Duration::from_secs(60));
    assert_eq!(env.task(id).status, Status::Ready);
    assert!(
        log.contains("no route_defaults candidates") || log.contains("route refused"),
        "{log}"
    );
    env.replay_is_consistent();
}
