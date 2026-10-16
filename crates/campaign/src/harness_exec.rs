//! A arm 用の実行器: manifest の harness（例 claude code）を subprocess で包み、trial workspace 上で走らせる。
//!
//! 【制限を足していない（ADR 追補に書く注記）】A は「既存の強い運用 baseline」であり、Celeris の routing・
//! 制限・許可範囲・turn 制限を一切通さない。ここが適用するのは manifest の `trial_timeout_secs`
//! （`TrialRequest::timeout_secs`、超過で process group を kill して `Timeout`）だけで、
//! budget（token / wall の合計）は runner が試行の前後で判定する。後付けの許可リスト・`--max-turns`・
//! sandbox・作業範囲の制限は、この module も既定の command builder も足さない。harness 側の既定の挙動のまま走らせる。
//!
//! 台帳・再開: 冪等キーごとに `<state_dir>/<key>/` を持ち、`meta.json` に pid / process group /
//! 起動時刻（/proc の start ticks で pid 再利用を見分ける）を fsync で書く。再起動後は `lookup` が meta を読み、
//! 生きていれば同じ process を追跡し、二重に起動しない。子は新しい process group（pgid = pid）で走るので、
//! cancel / timeout は group 全体を kill する。実 LLM はこの crate から呼ばない（command は注入可能で、試験は stub）。
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    aggregate::Outcome,
    executor::{ExecResult, ExecStatus, Executor, TaskRef, TrialRequest},
};

/// 起動する command（注入点）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarnessCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// command の組み立てと出力からの使用量の取り出し。試験は stub 実行ファイルを返す実装を渡す。
pub trait CommandBuilder: Send + Sync {
    fn build(&self, req: &TrialRequest) -> HarnessCommand;

    /// stdout から (tokens, cost_micro_usd) を取る。取れなければ None（欠測。推定で埋めない）。
    fn parse_usage(&self, stdout: &str) -> (Option<u64>, Option<u64>) {
        parse_claude_json_usage(stdout)
    }
}

/// 既定: `claude -p <prompt> --model <model> --output-format json`。許可・turn の制限は足さない。
pub struct ClaudeCodeBuilder {
    pub program: PathBuf,
}

impl Default for ClaudeCodeBuilder {
    fn default() -> Self {
        Self {
            program: "claude".into(),
        }
    }
}

impl CommandBuilder for ClaudeCodeBuilder {
    fn build(&self, req: &TrialRequest) -> HarnessCommand {
        HarnessCommand {
            program: self.program.clone(),
            args: vec![
                "-p".into(),
                req.prompt.into(),
                "--model".into(),
                req.config.model.clone(),
                "--output-format".into(),
                "json".into(),
            ],
            env: Vec::new(),
        }
    }
}

/// 最後に JSON object として読める行の `usage`（input+output token）と `total_cost_usd` を取る。
pub fn parse_claude_json_usage(stdout: &str) -> (Option<u64>, Option<u64>) {
    let Some(v) = stdout
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .filter(Value::is_object)
    else {
        return (None, None);
    };
    let u = &v["usage"];
    let tokens = match (u["input_tokens"].as_u64(), u["output_tokens"].as_u64()) {
        (Some(i), Some(o)) => Some(i + o),
        _ => None,
    };
    let cost = v["total_cost_usd"]
        .as_f64()
        .filter(|c| c.is_finite() && *c >= 0.0)
        .map(|c| (c * 1_000_000.0).round() as u64);
    (tokens, cost)
}

#[derive(Serialize, Deserialize)]
struct Meta {
    pid: u32,
    pgid: u32,
    /// /proc/<pid>/stat の starttime。pid 再利用で別 process を kill しないための照合値。
    start_ticks: u64,
    started_unix: u64,
    timeout_secs: u64,
}

/// timeout / cancel で executor が確定させた結果（process が exit file を書けないとき）。
#[derive(Serialize, Deserialize)]
struct Final {
    outcome: Outcome,
    wall_secs: u64,
    reason: String,
}

pub struct HarnessExecutor {
    state_dir: PathBuf,
    builder: Box<dyn CommandBuilder>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// (state, start_ticks)。/proc に無ければ None。
fn proc_stat(pid: u32) -> Option<(char, u64)> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &s[s.rfind(')')? + 1..];
    let mut f = rest.split_whitespace();
    let state = f.next()?.chars().next()?;
    // field 3 が state、field 22 が starttime。
    let start = f.nth(18)?.parse().ok()?;
    Some((state, start))
}

fn alive(m: &Meta) -> bool {
    matches!(proc_stat(m.pid), Some((s, t)) if s != 'Z' && s != 'X' && t == m.start_ticks)
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}

impl HarnessExecutor {
    pub fn new(state_dir: PathBuf, builder: Box<dyn CommandBuilder>) -> Self {
        Self { state_dir, builder }
    }

    fn dir(&self, key: &str) -> Result<PathBuf, String> {
        // key は台帳の冪等キー（campaign-<hex>）。path 区切りを持つものは拒否する。
        if key.is_empty() || key.contains(['/', '\\']) || key.starts_with('.') {
            return Err(format!("invalid task key {key:?}"));
        }
        Ok(self.state_dir.join(key))
    }

    fn meta(&self, key: &str) -> Result<Option<Meta>, String> {
        let p = self.dir(key)?.join("meta.json");
        match fs::read(&p) {
            Ok(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    fn task_ref(key: &str, m: &Meta) -> TaskRef {
        TaskRef {
            task_id: key.into(),
            run_id: Some(format!("pid:{}/pgid:{}", m.pid, m.pgid)),
            ..TaskRef::default()
        }
    }

    fn kill_group(m: &Meta) {
        if alive(m) {
            // 新しい process group（pgid = pid）全体を終了する。
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", m.pgid)])
                .stderr(Stdio::null())
                .status();
        }
    }

    fn finalize(&self, key: &str, outcome: Outcome, wall: u64, reason: &str) -> Result<(), String> {
        let f = Final {
            outcome,
            wall_secs: wall,
            reason: reason.into(),
        };
        let b = serde_json::to_vec(&f).map_err(|e| e.to_string())?;
        write_synced(&self.dir(key)?.join("final.json"), &b)
    }

    fn read_final(&self, key: &str) -> Result<Option<Final>, String> {
        match fs::read(self.dir(key)?.join("final.json")) {
            Ok(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| e.to_string()),
            Err(_) => Ok(None),
        }
    }

    fn usage(&self, key: &str) -> Result<(Option<u64>, Option<u64>), String> {
        let out = fs::read(self.dir(key)?.join("stdout.log")).unwrap_or_default();
        Ok(self.builder.parse_usage(&String::from_utf8_lossy(&out)))
    }
}

impl Executor for HarnessExecutor {
    fn name(&self) -> &'static str {
        "harness"
    }

    fn execute(&self, _req: &TrialRequest) -> Result<ExecStatus, String> {
        Err("harness is driven via submit/poll (ledger)".into())
    }

    fn is_async(&self) -> bool {
        true
    }

    fn routing(&self, req: &TrialRequest) -> Result<Value, String> {
        Ok(serde_json::json!({
            "mode": "none",
            "note": "A arm bypasses Celeris routing and limits",
            "harness": req.config.harness,
            "model": req.config.model,
        }))
    }

    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, String> {
        if let Some(m) = self.meta(key)? {
            return Ok(Some(Self::task_ref(key, &m)));
        }
        // 起動中の目印だけが残っている: spawn 済みか分からないので二重起動を避けて止める。
        if self.dir(key)?.join("starting").exists() {
            return Err(format!(
                "{key}: start marker without meta (crash during spawn); inspect {} and remove it by hand",
                self.dir(key)?.display()
            ));
        }
        Ok(None)
    }

    fn submit(&self, req: &TrialRequest, key: &str) -> Result<TaskRef, String> {
        if let Some(t) = self.lookup(key)? {
            return Ok(t);
        }
        let dir = self.dir(key)?;
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // spawn の『前』に目印を fsync（create_new で二重 submit も弾く）。
        let mut mark = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("starting"))
            .map_err(|e| format!("{key}: starting marker: {e}"))?;
        mark.write_all(b"1").map_err(|e| e.to_string())?;
        mark.sync_all().map_err(|e| e.to_string())?;
        File::open(&dir)
            .and_then(|d| d.sync_all())
            .map_err(|e| e.to_string())?;

        let c = self.builder.build(req);
        let open = |n: &str| File::create(dir.join(n)).map_err(|e| e.to_string());
        // wrapper: command の終了コードを exit.code に原子的に書く（親 process が死んでも残る）。
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(r#"d=$1; shift; "$@"; echo $? > "$d/exit.code.tmp" && mv "$d/exit.code.tmp" "$d/exit.code""#)
            .arg("sh")
            .arg(&dir)
            .arg(&c.program)
            .args(&c.args)
            .envs(c.env.iter().map(|(k, v)| (k, v)))
            .current_dir(req.workspace)
            .stdin(Stdio::null())
            .stdout(open("stdout.log")?)
            .stderr(open("stderr.log")?)
            .process_group(0);
        let child = cmd
            .spawn()
            .map_err(|e| format!("spawn {:?}: {e}", c.program))?;
        let pid = child.id();
        let start_ticks = proc_stat(pid).map_or(0, |(_, t)| t);
        let m = Meta {
            pid,
            pgid: pid,
            start_ticks,
            started_unix: now_secs(),
            timeout_secs: req.timeout_secs,
        };
        let b = serde_json::to_vec(&m).map_err(|e| e.to_string())?;
        write_synced(&dir.join("meta.json"), &b)?;
        Ok(Self::task_ref(key, &m))
    }

    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, String> {
        let key = &t.task_id;
        let m = self
            .meta(key)?
            .ok_or_else(|| format!("{key}: no harness state (meta.json)"))?;
        let (tokens, cost) = self.usage(key)?;
        let done = |outcome, wall_secs| {
            Ok(ExecStatus::Done(ExecResult {
                outcome,
                tokens,
                cost_micro_usd: cost,
                wall_secs,
            }))
        };
        if let Some(f) = self.read_final(key)? {
            return done(f.outcome, f.wall_secs);
        }
        let dir = self.dir(key)?;
        if let Ok(code) = fs::read_to_string(dir.join("exit.code")) {
            let wall = fs::metadata(dir.join("exit.code"))
                .and_then(|x| x.modified())
                .ok()
                .and_then(|mt| mt.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs().saturating_sub(m.started_unix));
            // 暫定の結果。pass/fail の正本は独立採点（checker）で、ここは harness の終了コード。
            let outcome = if code.trim() == "0" {
                Outcome::Passed
            } else {
                Outcome::Failed
            };
            return done(outcome, wall);
        }
        let elapsed = now_secs().saturating_sub(m.started_unix);
        if !alive(&m) {
            // exit.code が無いまま消えた（SIGKILL・host 再起動など）。
            self.finalize(key, Outcome::Error, elapsed, "process_vanished")?;
            return done(Outcome::Error, elapsed);
        }
        if elapsed > m.timeout_secs {
            Self::kill_group(&m);
            self.finalize(key, Outcome::Timeout, m.timeout_secs, "trial_timeout")?;
            return done(Outcome::Timeout, m.timeout_secs);
        }
        Ok(ExecStatus::Wait)
    }

    fn cancel(&self, t: &TaskRef, reason: &str) -> Result<(), String> {
        let key = &t.task_id;
        let Some(m) = self.meta(key)? else {
            return Ok(());
        };
        Self::kill_group(&m);
        if self.read_final(key)?.is_none() && !self.dir(key)?.join("exit.code").exists() {
            let elapsed = now_secs().saturating_sub(m.started_unix);
            self.finalize(key, Outcome::Cancelled, elapsed, reason)?;
        }
        Ok(())
    }
}
