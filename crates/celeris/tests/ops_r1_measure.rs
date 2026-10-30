//! ops-r1 測定 daemon の通し試験（stub harness・無 LLM）。
//!
//! `scripts/campaign/ops-r1-measure-config.sh --harness stub` で一時 state に daemon.toml を生成し、実 `celeris`
//! binary（`CARGO_BIN_EXE_celeris`）を `env_clear`・loopback・一時 DB の daemon として起こす（起動の型は
//! `cap_probe_real_daemon.rs`）。案件 CoS thread に人の message を送り、stub の CoS run が
//! `/api/v1/cos/operations` で task を起票し、worker run（fake adapter の stub）が終わり、root 完了の CoS run の
//! 報告 message が thread に残るまでを、API の poll（出来事待ち。上限は長い保険）で確かめる。起票が CoS 由来
//! （cos operation の監査行・actor=cos）であることと、`GET /api/v1/health` の `cos_policy_version` が "6" で
//! あることも確かめる。LLM・外部ネットワーク・本番の state/DB/port/token は使わない。
//!
//! 試験だけの追加: 生成した daemon.toml の末尾に人の token（`human:ops-r1-e2e`）を 1 つ足す。人の message は
//! 人の principal で送る（`celeris-campaign ops-r1-intake` は author が `human:` の message だけを記録する）。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

/// 出来事待ちの上限（長い保険）。通常は数秒で終わる。
const DEADLINE: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_millis(200);
/// 人の依頼の目印。stub の CoS run はこれを含む入力でだけ起票する。
const REQUEST_MARK: &str = "OPS_R1_E2E_REQUEST";
const REPORT_MARK: &str = "OPS_R1_E2E_REPORT";
const TASK_TITLE: &str = "ops-r1 e2e の調べもの";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 実行可能 file は別 process で書く（ETXTBSY 回避）。
fn write_executable(path: &Path, body: &str) {
    let mut c = Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn");
    c.stdin
        .take()
        .expect("stdin")
        .write_all(body.as_bytes())
        .expect("write");
    assert!(c.wait().expect("wait").success());
}

/// 3 役を受ける stub（CoS run・worker run・root 完了の CoS run）。役は env で分ける: CoS run だけが
/// `CELERIS_COS_RUN_CREDENTIAL` を持つ。CoS run は、人の依頼の目印を含む最初の入力でだけ起票し
/// （`mkdir` で 1 回に限る）、それ以外の CoS run は報告を返す。stdin（RunRequest）は記録に残す。
const STUB: &str = r#"#!/bin/sh
set -u
d=$OPS_R1_STUB_DIR
input=$(cat)
n=$(ls "$d/runs" | wc -l)
if [ -n "${CELERIS_COS_RUN_CREDENTIAL:-}" ]; then role=cos; else role=worker; fi
printf '%s' "$input" >"$d/runs/$n-$role.json"
if [ "$role" = worker ]; then
    echo '{"type":"progress","msg":"stub worker","kind":"status","summary":"stub worker"}'
    echo '{"type":"done","summary":"OPS_R1_E2E_WORKER: 調べてまとめた","evidence":[]}'
    exit 0
fi
case $input in
    *OPS_R1_E2E_REQUEST*) want=1 ;;
    *) want=0 ;;
esac
if [ "$want" = 1 ] && mkdir "$d/created" 2>/dev/null; then
    printf '{"idempotency_key":"ops-r1-e2e-create","expected_revision":null,"reason":"人の依頼を起票する","policy_version":"%s","request":{"method":"POST","path":"/api/v1/tasks","body":%s}}' \
        "$CELERIS_COS_POLICY_VERSION" "$(cat "$d/task.json")" >"$d/created/request.json"
    curl -sS -o "$d/created/response.json" -w '%{http_code}' \
        -H "Authorization: Bearer $CELERIS_COS_RUN_CREDENTIAL" -H 'Content-Type: application/json' \
        --data-binary @"$d/created/request.json" "$CELERIS_API_URL/cos/operations" >"$d/created/status" 2>"$d/created/stderr"
    echo '{"type":"done","summary":"依頼を task として起票しました。","evidence":[]}'
    exit 0
fi
echo '{"type":"done","summary":"OPS_R1_E2E_REPORT: 依頼の task が完了しました。","evidence":[]}'
"#;

struct Daemon {
    child: Child,
    dir: tempfile::TempDir,
    url: String,
    operator: String,
    human: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Daemon {
    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn stub_dir(&self) -> PathBuf {
        self.dir.path().join("stub")
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.path().join("daemon.log")).unwrap_or_default()
    }

    /// 失敗時の手がかり: stub が受けた run・起票の応答・CoS run の状態。
    fn diag(&self) -> String {
        let mut out = String::new();
        let stub = self.stub_dir();
        for sub in ["runs", "created"] {
            if let Ok(entries) = fs::read_dir(stub.join(sub)) {
                for e in entries.flatten() {
                    let text = fs::read_to_string(e.path()).unwrap_or_default();
                    let head: String = text.chars().take(600).collect();
                    out.push_str(&format!(
                        "--- stub/{sub}/{} ---\n{head}\n",
                        e.file_name().to_string_lossy()
                    ));
                }
            }
        }
        if let Ok(conn) = rusqlite::Connection::open_with_flags(
            self.state().join("celeris.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) && let Ok(mut stmt) = conn.prepare(
            "SELECT run_id, state, COALESCE(reason, '') FROM chat_runs ORDER BY started_at",
        ) {
            let rows = stmt.query_map([], |r| {
                Ok(format!(
                    "chat_run {} {} {}\n",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?
                ))
            });
            for row in rows.into_iter().flatten().flatten() {
                out.push_str(&row);
            }
            if let Ok(mut stmt) = conn.prepare(
                "SELECT task_id, substr(json, 1, 400) FROM events ORDER BY id DESC LIMIT 15",
            ) {
                let rows = stmt.query_map([], |r| {
                    Ok(format!(
                        "event {} {}\n",
                        r.get::<_, String>(0).unwrap_or_default(),
                        r.get::<_, String>(1).unwrap_or_default()
                    ))
                });
                for row in rows.into_iter().flatten().flatten() {
                    out.push_str(&row);
                }
            }
        }
        out
    }

    /// loopback の daemon への HTTP（curl）。`(status, body)` を返す。
    fn call(&self, token: &str, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let mut c = Command::new("curl");
        c.args(["-sS", "-o", "-", "-w", "\n%{http_code}", "-X", method])
            .arg("-H")
            .arg(format!("Authorization: Bearer {token}"))
            .arg(format!("{}{path}", self.url));
        if let Some(body) = body {
            c.args(["-H", "Content-Type: application/json", "--data-binary"])
                .arg(body.to_string());
        }
        let out = c.output().expect("curl");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let (body, status) = text.rsplit_once('\n').unwrap_or(("", "0"));
        let status = status.trim().parse().unwrap_or(0);
        (status, serde_json::from_str(body).unwrap_or(Value::Null))
    }

    fn get(&self, path: &str) -> (u16, Value) {
        self.call(&self.operator, "GET", path, None)
    }

    /// `probe` が `Some` を返すまで poll する（出来事待ち。sleep で待ち、CPU を焼かない）。
    fn wait_for<T>(&self, what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            if let Some(v) = probe() {
                return v;
            }
            assert!(
                start.elapsed() < DEADLINE,
                "timed out waiting for {what}\n{}\n--- daemon.log ---\n{}",
                self.diag(),
                self.log()
            );
            std::thread::sleep(POLL);
        }
    }
}

/// 生成 script で daemon.toml を作り、実 `celeris` を `env_clear` の一時 daemon として起こす。
fn start_daemon() -> Daemon {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path();
    let state = root.join("state");
    let stub_dir = root.join("stub");
    let home = root.join("home");
    let tmp = root.join("tmp");
    for d in [&stub_dir.join("runs"), &home, &tmp] {
        fs::create_dir_all(d).expect("mkdir");
    }
    let stub = stub_dir.join("stub.sh");
    write_executable(&stub, STUB);
    // 合成の release dir（bin/celeris は試験の binary、manifest.json は sha12 だけ）。
    let release = root.join("releases").join("0e2e0e2e0e2e");
    fs::create_dir_all(release.join("bin")).expect("release bin");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_celeris"), release.join("bin/celeris"))
        .expect("bin symlink");
    fs::write(
        release.join("manifest.json"),
        r#"{"sha12": "0e2e0e2e0e2e"}"#,
    )
    .expect("manifest");
    let config = root.join("daemon.toml");
    let out = Command::new("sh")
        .arg(repo_root().join("scripts/campaign/ops-r1-measure-config.sh"))
        .arg("--state")
        .arg(&state)
        .arg("--release-dir")
        .arg(&release)
        .args([
            "--models",
            "cos-opus",
            "--harness",
            "stub",
            "--stub-command",
        ])
        .arg(&stub)
        .arg("--out")
        .arg(&config)
        .env("HOME", &home)
        .output()
        .expect("config script");
    assert!(
        out.status.success(),
        "ops-r1-measure-config.sh: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let generated = fs::read_to_string(&config).expect("daemon.toml");
    for line in [
        "[cos]\nenabled = true",
        "policy_version = \"6\"",
        "max_concurrency = 1",
        "max_parallel_work_units = 1",
        "max_parallel_child_tasks = 1",
        "[harnesses.claude_code]\nkind = \"fake\"",
    ] {
        assert!(generated.contains(line), "{line:?} in\n{generated}");
    }
    for skill in ["cos-operator", "cos-inbox-triage"] {
        assert!(
            state
                .join("kb/skills")
                .join(skill)
                .join("SKILL.md")
                .is_file(),
            "skill {skill} delivered"
        );
    }
    let port = generated
        .lines()
        .find_map(|l| l.strip_prefix("listen = \"127.0.0.1:"))
        .and_then(|l| l.strip_suffix('"'))
        .expect("loopback listen")
        .to_owned();
    // 試験だけの追加: 人の token。
    let human = "ops-r1-e2e-human-token".to_owned();
    fs::write(root.join("human.token"), format!("{human}\n")).expect("human token");
    fs::write(
        &config,
        format!(
            "{generated}\n[[api.tokens]]\nprincipal = \"human:ops-r1-e2e\"\ntoken_file = {:?}\n",
            root.join("human.token").display().to_string()
        ),
    )
    .expect("append human token");
    let operator = fs::read_to_string(state.join("token"))
        .expect("token")
        .trim()
        .to_owned();
    let log = fs::File::create(root.join("daemon.log")).expect("log");
    let child = Command::new(release.join("bin/celeris"))
        .arg("--config")
        .arg(&config)
        .args(["--log-format", "text"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .env("TMPDIR", &tmp)
        .env("OPS_R1_STUB_DIR", &stub_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().expect("log")))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("spawn celeris");
    let d = Daemon {
        child,
        dir,
        url: format!("http://127.0.0.1:{port}"),
        operator,
        human,
    };
    d.wait_for("the daemon to answer /api/v1/health", || {
        (d.get("/api/v1/health").0 == 200).then_some(())
    });
    d
}

fn messages(d: &Daemon, thread: &str) -> Vec<Value> {
    let (s, v) = d.get(&format!("/api/v1/chat/threads/{thread}/messages"));
    if s != 200 {
        return Vec::new();
    }
    v["items"].as_array().cloned().unwrap_or_default()
}

fn db_rows(d: &Daemon, sql: &str) -> Vec<Vec<String>> {
    let conn = rusqlite::Connection::open_with_flags(
        d.state().join("celeris.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("db");
    let mut stmt = conn.prepare(sql).expect("prepare");
    let n = stmt.column_count();
    stmt.query_map([], |r| {
        (0..n)
            .map(|i| r.get::<_, Option<String>>(i).map(|v| v.unwrap_or_default()))
            .collect::<Result<Vec<_>, _>>()
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .expect("rows")
}

#[test]
fn ops_r1_measure_cos_path_with_stub_harness() {
    let d = start_daemon();

    // health: CoS が有効で policy 6。
    let (s, health) = d.get("/api/v1/health");
    assert_eq!(s, 200, "{health}");
    assert_eq!(health["cos_policy_version"], "6", "{health}");

    // 試行専用の案件と案件 CoS thread（docs/ops/ops-r1-measure-daemon.md の手順どおり API で作る）。
    let project_body: Value = serde_json::from_str(
        &fs::read_to_string(d.state().join("setup/project.json")).expect("project.json"),
    )
    .expect("project body");
    let (s, project) = d.call(&d.operator, "POST", "/api/v1/projects", Some(&project_body));
    assert!(s == 200 || s == 201, "{s} {project}");
    let project_id = project["id"].as_str().expect("project id").to_owned();
    let (s, thread) = d.call(
        &d.operator,
        "POST",
        "/api/v1/chat/threads",
        Some(&json!({"project_id": project_id})),
    );
    assert!(s == 200 || s == 201, "{s} {thread}");
    let thread_id = thread["thread"]["id"]
        .as_str()
        .or(thread["id"].as_str())
        .expect("thread id")
        .to_owned();

    // stub CoS の起票 body（試行案件の task。試行 1 回 = max_retries 0。決定的な検査で完了）。
    fs::write(
        d.stub_dir().join("task.json"),
        json!({
            "title": TASK_TITLE,
            "objective": "依頼の内容を調べて短くまとめる",
            "acceptance": [{"type": "command", "cmd": "true", "expect_exit": 0}],
            "project_id": project_id,
            "max_retries": 0,
        })
        .to_string(),
    )
    .expect("task body");

    // 人の message。
    let (s, posted) = d.call(
        &d.human,
        "POST",
        &format!("/api/v1/chat/threads/{thread_id}/messages"),
        Some(&json!({
            "client_message_id": "ops-r1-e2e-1",
            "text": format!("{REQUEST_MARK}: 調べものを 1 件お願いします"),
            "attachment_ids": [], "reply_to_id": null, "mode": "queue", "resume_queue": false,
        })),
    );
    assert_eq!(s, 202, "{posted}");
    assert_eq!(posted["message"]["role"], "user", "{posted}");
    let human_message_id = posted["message"]["id"]
        .as_str()
        .expect("message id")
        .to_owned();

    // CoS 起票: stub の CoS run が /cos/operations を叩き、task が案件に現れる。
    let task = d.wait_for("the task created by the CoS run", || {
        let (s, v) = d.get(&format!("/api/v1/tasks?project={project_id}"));
        (s == 200)
            .then(|| v["items"].as_array().cloned().unwrap_or_default())
            .and_then(|items| items.into_iter().find(|t| t["title"] == TASK_TITLE))
    });
    let task_id = task["id"].as_str().expect("task id").to_owned();
    // stub の curl は応答を受けてから status を書くので、task が見えた後も書き終わりを待つ。
    let status = d.wait_for("the stub to record the operation status", || {
        fs::read_to_string(d.stub_dir().join("created/status"))
            .ok()
            .filter(|s| !s.trim().is_empty())
    });
    let response = fs::read_to_string(d.stub_dir().join("created/response.json")).expect("resp");
    assert_eq!(status.trim(), "200", "{response}");
    let op: Value = serde_json::from_str(&response).expect("operation json");
    let op = &op["operation"];
    assert_eq!(op["actor"], "cos", "{op}");
    assert_eq!(op["action"], "task.create", "{op}");
    assert_eq!(op["state"], "applied", "{op}");
    assert_eq!(op["policy_version"], "6", "{op}");
    assert_eq!(op["thread_id"], thread_id.as_str(), "{op}");
    assert_eq!(op["target_id"], task_id.as_str(), "{op}");
    // 監査行は DB にも残り、起票した CoS run は人の message を入力にした run。
    let audit = db_rows(
        &d,
        &format!(
            "SELECT o.action, o.state, o.policy_version, r.input_message_id FROM cos_operations o \
             JOIN chat_runs r ON r.run_id = o.run_id WHERE o.target_id = '{task_id}'"
        ),
    );
    assert_eq!(
        audit,
        vec![vec![
            "task.create".to_owned(),
            "applied".to_owned(),
            "6".to_owned(),
            human_message_id.clone()
        ]],
        "audit rows"
    );

    // worker run が終わり、task が done になる。
    let done = d.wait_for("the task to be done", || {
        let (s, v) = d.get(&format!("/api/v1/tasks/{task_id}"));
        let task = if v["task"].is_object() {
            &v["task"]
        } else {
            &v
        };
        (s == 200 && task["status"] == "done").then(|| task.clone())
    });
    assert_eq!(done["project_id"], project_id.as_str(), "{done}");
    // stub は worker 役で、この task の RunRequest を受けた。
    let worker_inputs: Vec<String> = fs::read_dir(d.stub_dir().join("runs"))
        .expect("runs")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with("-worker.json"))
        .map(|e| fs::read_to_string(e.path()).expect("worker input"))
        .collect();
    assert!(
        worker_inputs
            .iter()
            .any(|i| i.contains(&format!("\"id\":\"{task_id}\""))),
        "a worker run of the task went through the stub"
    );
    // daemon の runs 台帳でも、この task の worker run が終わっている。
    let runs = db_rows(
        &d,
        &format!(
            "SELECT role, status FROM runs WHERE task_id = '{task_id}' AND finished_at IS NOT NULL"
        ),
    );
    assert!(
        runs.iter().any(|r| r[0] == "worker" && r[1] == "completed"),
        "a finished worker run of the task: {runs:?}"
    );

    // 報告: root 完了の CoS run の返事が案件 CoS thread に残る。
    let report = d.wait_for("the CoS report message in the thread", || {
        messages(&d, &thread_id).into_iter().find(|m| {
            m["role"] == "assistant" && m["text"].as_str().is_some_and(|t| t.contains(REPORT_MARK))
        })
    });
    assert_eq!(report["thread_id"], thread_id.as_str(), "{report}");
    let report_run = report["run_id"].as_str().expect("report run").to_owned();
    let input = db_rows(
        &d,
        &format!(
            "SELECT m.client_message_id FROM chat_runs r JOIN chat_messages m \
             ON m.id = r.input_message_id WHERE r.run_id = '{report_run}'"
        ),
    );
    assert!(
        input.len() == 1 && input[0][0].starts_with(&format!("root-done:{task_id}:")),
        "the report answers the root-done input: {input:?}"
    );
    // 人の message への CoS の返事（起票の知らせ）も同じ thread に残る。
    assert!(
        messages(&d, &thread_id)
            .iter()
            .any(|m| m["role"] == "assistant"
                && m["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("起票しました"))),
        "the CoS reply to the human message"
    );
}
