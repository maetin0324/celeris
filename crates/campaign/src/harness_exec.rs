//! A arm 用の実行器: manifest の harness（例 claude code）を subprocess で包み、trial workspace 上で走らせる。
//!
//! A は「既存の強い運用 baseline」であり、Celeris の routing・turn 制限・許可範囲を通さない（後付けの
//! `--max-turns`・許可リスト・tool 制限は足さない）。足すのは次の 3 つだけ（ADR 2026-10-17-mlsys-pilot2-fixes）:
//! - D2 ファイルシステム隔離: `Sandbox` が command を bubblewrap で包み、試行の workspace・A のアカウント dir・
//!   claude の実行ファイル以外の運用者の file（grader-private・他試行・過去 workspace・環境変数の token）を見せない。
//! - D3 token 予算の実行中強制: stdout の stream-json から累積 tokens を逐次読み、`TrialRequest.budget.max_tokens` を
//!   超えたら process group を kill する（監視 thread と `poll` の両方。再開後は `poll` が判定する）。
//! - `trial_timeout_secs`（超過で process group を kill して `Timeout`）。
//!
//! 台帳・再開: 冪等キーごとに `<state_dir>/<key>/` を持ち、`meta.json` に pid / process group /
//! 起動時刻（/proc の start ticks で pid 再利用を見分ける）を fsync で書く。再起動後は `lookup` が meta を読み、
//! 生きていれば同じ process を追跡し、二重に起動しない。子は新しい process group（pgid = pid）で走るので、
//! cancel / timeout は group 全体を kill する。実 LLM はこの crate から呼ばない（command は注入可能で、試験は stub）。
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    aggregate::Outcome,
    executor::{
        ExecError, ExecResult, ExecStatus, Executor, ObservedRun, TaskRef, TokenUsage,
        TrialRequest, Usage,
    },
    manifest::TokenBasis,
    record::REASON_CASE_BUDGET,
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

    /// stdout から (tokens の内訳, cost_micro_usd) を取る。取れなければ None（欠測。推定で埋めない）。
    fn parse_usage(&self, stdout: &str) -> (Option<TokenUsage>, Option<u64>) {
        parse_claude_json_usage(stdout)
    }

    /// 実行中（最終の result 行が無い）の累積 tokens。取れなければ None。
    fn live_usage(&self, stdout: &str) -> Option<TokenUsage> {
        parse_claude_stream_usage(stdout)
    }
}

/// 既定: `claude -p <prompt> --model <model> --output-format stream-json --verbose --permission-mode bypassPermissions`。
/// stream-json は実行中の token を逐次出す（予算の実行中強制 D3）。最後の `result` 行が最終の usage と費用。
/// headless では承認者がいないので、permission mode が無いと書き込み・Bash が全て拒否されて何も作れない
/// （pilot 2026-10-16）。試行は専用の複製 workspace の中で走る。許可リスト・turn の後付け制限は足さない。
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
                "stream-json".into(),
                "--verbose".into(),
                "--permission-mode".into(),
                "bypassPermissions".into(),
            ],
            env: Vec::new(),
        }
    }
}

/// 最後に JSON object として読める行の `usage`（input / output / cache_read / cache_creation）と
/// `total_cost_usd` を取る。input・output が無ければ欠測、cache が無ければその内訳だけ欠測。
pub fn parse_claude_json_usage(stdout: &str) -> (Option<TokenUsage>, Option<u64>) {
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
        (Some(input), Some(output)) => Some(TokenUsage {
            input,
            output,
            cache_read: u["cache_read_input_tokens"].as_u64(),
            cache_creation: u["cache_creation_input_tokens"].as_u64(),
        }),
        _ => None,
    };
    let cost = v["total_cost_usd"]
        .as_f64()
        .filter(|c| c.is_finite() && *c >= 0.0)
        .map(|c| (c * 1_000_000.0).round() as u64);
    (tokens, cost)
}

/// stream-json の `assistant` 行を `message.id` ごとの最新 usage にして足した累積。
/// 同じ message の複数 block は同じ id・同じ usage で出るので、id で重複を除く。
pub fn parse_claude_stream_usage(stdout: &str) -> Option<TokenUsage> {
    let mut by_id: BTreeMap<String, [u64; 4]> = BTreeMap::new();
    for l in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(l.trim()) else {
            continue;
        };
        if v["type"] != "assistant" {
            continue;
        }
        let (Some(id), u) = (v["message"]["id"].as_str(), &v["message"]["usage"]) else {
            continue;
        };
        let g = |k: &str| u[k].as_u64().unwrap_or(0);
        by_id.insert(
            id.to_string(),
            [
                g("input_tokens"),
                g("output_tokens"),
                g("cache_read_input_tokens"),
                g("cache_creation_input_tokens"),
            ],
        );
    }
    if by_id.is_empty() {
        return None;
    }
    let t = by_id.values().fold([0u64; 4], |mut a, x| {
        for i in 0..4 {
            a[i] += x[i];
        }
        a
    });
    Some(TokenUsage::full(t[0], t[1], t[2], t[3]))
}

/// A のファイルシステム隔離（bubblewrap）。ADR 2026-10-17-mlsys-pilot2-fixes D2。
#[derive(Clone, Debug)]
pub struct Sandbox {
    /// A が使う Celeris 管理アカウント（claude_oauth）の id と、その `CLAUDE_CONFIG_DIR`。
    pub account_id: String,
    pub config_dir: PathBuf,
}

/// PATH を引いた実体（symlink 解決済み）。絶対 path / 相対 path はそのまま解決する。
fn resolve_program(p: &Path) -> Result<PathBuf, String> {
    let cand = if p.components().count() > 1 || p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::var_os("PATH")
            .and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|d| d.join(p))
                    .find(|c| c.is_file())
            })
            .ok_or_else(|| format!("{} not found in PATH", p.display()))?
    };
    fs::canonicalize(&cand).map_err(|e| format!("{}: {e}", cand.display()))
}

impl Sandbox {
    /// bwrap に渡す引数（`bwrap` 自身は含まない）。`real` は解決済みの実行ファイル。
    /// 見える物は固定: 読み取り専用の `/usr` と `/etc`・`/bin` 等、`/proc` `/dev`、空の `/tmp`、
    /// workspace と A のアカウント dir（書き込み可）、実行ファイル本体。それ以外の host の path は存在しない。
    pub fn args(&self, workspace: &Path, real: &Path, env: &[(String, String)]) -> Vec<String> {
        let mut a: Vec<String> = Vec::new();
        let mut add = |xs: &[&str]| a.extend(xs.iter().map(|x| x.to_string()));
        add(&[
            "--die-with-parent",
            "--unshare-pid",
            "--unshare-ipc",
            "--new-session",
            "--clearenv",
        ]);
        add(&["--ro-bind", "/usr", "/usr"]);
        for d in ["bin", "sbin", "lib", "lib32", "lib64"] {
            let host = Path::new("/").join(d);
            match fs::read_link(&host) {
                Ok(t) => add(&["--symlink", &t.to_string_lossy(), &format!("/{d}")]),
                Err(_) if host.is_dir() => add(&["--ro-bind", &format!("/{d}"), &format!("/{d}")]),
                Err(_) => {}
            }
        }
        add(&[
            "--ro-bind",
            "/etc",
            "/etc",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
        ]);
        add(&["--tmpfs", "/tmp", "--dir", "/tmp/home"]);
        let (ws, cfg, real_s) = (
            workspace.to_string_lossy().into_owned(),
            self.config_dir.to_string_lossy().into_owned(),
            real.to_string_lossy().into_owned(),
        );
        add(&["--ro-bind", &real_s, &real_s]);
        add(&["--bind", &ws, &ws, "--bind", &cfg, &cfg, "--chdir", &ws]);
        add(&["--setenv", "PATH", "/usr/local/bin:/usr/bin:/bin"]);
        add(&[
            "--setenv",
            "HOME",
            "/tmp/home",
            "--setenv",
            "LANG",
            "C.UTF-8",
        ]);
        add(&[
            "--setenv",
            "TERM",
            "dumb",
            "--setenv",
            "CLAUDE_CONFIG_DIR",
            &cfg,
        ]);
        for (k, v) in env {
            add(&["--setenv", k, v]);
        }
        a
    }

    pub fn wrap(&self, c: &HarnessCommand, workspace: &Path) -> Result<HarnessCommand, String> {
        let real = resolve_program(&c.program)?;
        let mut args = self.args(workspace, &real, &c.env);
        args.push("--".into());
        args.push(real.to_string_lossy().into_owned());
        args.extend(c.args.iter().cloned());
        Ok(HarnessCommand {
            program: "bwrap".into(),
            args,
            env: Vec::new(),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Meta {
    pid: u32,
    pgid: u32,
    /// /proc/<pid>/stat の starttime。pid 再利用で別 process を kill しないための照合値。
    start_ticks: u64,
    started_unix: u64,
    timeout_secs: u64,
    /// この試行の実効 token 上限（`TrialRequest.budget.max_tokens`）。再開後の `poll` も同じ上限で止める。
    #[serde(default)]
    max_tokens: Option<u64>,
}

/// timeout / cancel で executor が確定させた結果（process が exit file を書けないとき）。
#[derive(Serialize, Deserialize)]
struct Final {
    outcome: Outcome,
    wall_secs: u64,
    reason: String,
}

/// token 予算の実行中強制（D3）。`basis` で数えた累積が上限を超えたら kill する。
#[derive(Clone, Copy, Debug)]
pub struct BudgetGuard {
    pub basis: TokenBasis,
    /// 監視 thread が stdout を読む間隔。
    pub interval: Duration,
}

pub struct HarnessExecutor {
    state_dir: PathBuf,
    builder: Arc<dyn CommandBuilder>,
    sandbox: Option<Sandbox>,
    guard: Option<BudgetGuard>,
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

/// 試行の状態 dir の stdout から（最終の usage、無ければ実行中の累積）を取る。
fn read_usage(builder: &dyn CommandBuilder, dir: &Path) -> (Option<TokenUsage>, Option<u64>) {
    let out = fs::read(dir.join("stdout.log")).unwrap_or_default();
    let text = String::from_utf8_lossy(&out);
    let (t, c) = builder.parse_usage(&text);
    match t {
        Some(_) => (t, c),
        None => (builder.live_usage(&text), c),
    }
}

/// process group 全体を終了する。
fn kill_group(m: &Meta) {
    if alive(m) {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", m.pgid)])
            .stderr(Stdio::null())
            .status();
    }
}

fn finalize_dir(dir: &Path, outcome: Outcome, wall: u64, reason: &str) -> Result<(), String> {
    let f = Final {
        outcome,
        wall_secs: wall,
        reason: reason.into(),
    };
    let b = serde_json::to_vec(&f).map_err(|e| e.to_string())?;
    write_synced(&dir.join("final.json"), &b)
}

/// 上限超過なら kill して cancelled を確定する。確定したら true。
fn enforce_budget(
    builder: &dyn CommandBuilder,
    guard: BudgetGuard,
    dir: &Path,
    m: &Meta,
) -> Result<bool, String> {
    let Some(max) = m.max_tokens else {
        return Ok(false);
    };
    if dir.join("final.json").exists() || dir.join("exit.code").exists() {
        return Ok(false);
    }
    let (live, _) = read_usage(builder, dir);
    if live
        .and_then(|u| u.count(guard.basis))
        .is_some_and(|t| t > max)
    {
        kill_group(m);
        finalize_dir(
            dir,
            Outcome::Cancelled,
            now_secs().saturating_sub(m.started_unix),
            REASON_CASE_BUDGET,
        )?;
        return Ok(true);
    }
    Ok(false)
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}

impl HarnessExecutor {
    /// 隔離も予算強制も無い（stub 実行ファイルの試験用）。実 harness は `with_sandbox` と `with_budget_guard` を足す。
    pub fn new(state_dir: PathBuf, builder: Box<dyn CommandBuilder>) -> Self {
        Self {
            state_dir,
            builder: Arc::from(builder),
            sandbox: None,
            guard: None,
        }
    }

    pub fn with_sandbox(mut self, sandbox: Sandbox) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    pub fn with_budget_guard(mut self, guard: BudgetGuard) -> Self {
        self.guard = Some(guard);
        self
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

    fn finalize(&self, key: &str, outcome: Outcome, wall: u64, reason: &str) -> Result<(), String> {
        finalize_dir(&self.dir(key)?, outcome, wall, reason)
    }

    fn read_final(&self, key: &str) -> Result<Option<Final>, String> {
        match fs::read(self.dir(key)?.join("final.json")) {
            Ok(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| e.to_string()),
            Err(_) => Ok(None),
        }
    }

    fn usage_of(&self, key: &str) -> Result<(Option<TokenUsage>, Option<u64>), String> {
        Ok(read_usage(self.builder.as_ref(), &self.dir(key)?))
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

    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, ExecError> {
        if let Some(m) = self.meta(key)? {
            return Ok(Some(Self::task_ref(key, &m)));
        }
        // 起動中の目印だけが残っている: spawn 済みか分からないので二重起動を避けて止める。
        if self.dir(key)?.join("starting").exists() {
            return Err(ExecError::unknown(format!(
                "{key}: start marker without meta (crash during spawn); inspect {} and remove it by hand",
                self.dir(key)?.display()
            )));
        }
        Ok(None)
    }

    fn submit(&self, req: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
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
        // 実際に渡す引数から実測を残す（manifest の宣言名は使わない）。
        let model = c
            .args
            .iter()
            .position(|a| a == "--model")
            .and_then(|i| c.args.get(i + 1))
            .cloned();
        let seen = ObservedRun {
            run_id: key.to_string(),
            role: "worker".into(),
            source: self
                .sandbox
                .as_ref()
                .map(|sb| format!("claude_oauth:{}", sb.account_id)),
            harness: c
                .program
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
            model,
            ..ObservedRun::default()
        };
        let b = serde_json::to_vec(&seen).map_err(|e| e.to_string())?;
        write_synced(&dir.join("observed.json"), &b)?;
        let c = match &self.sandbox {
            Some(sb) => sb.wrap(&c, req.workspace)?,
            None => c,
        };
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
            max_tokens: Some(req.budget.max_tokens),
        };
        let b = serde_json::to_vec(&m).map_err(|e| e.to_string())?;
        write_synced(&dir.join("meta.json"), &b)?;
        if let Some(g) = self.guard {
            // 監視: poll の間隔（runner の 1 周）を待たずに上限で止める。再開後は poll が同じ判定をする。
            let (builder, dir) = (Arc::clone(&self.builder), dir.clone());
            let m2 = Meta {
                pid: m.pid,
                pgid: m.pgid,
                start_ticks: m.start_ticks,
                started_unix: m.started_unix,
                timeout_secs: m.timeout_secs,
                max_tokens: m.max_tokens,
            };
            std::thread::spawn(move || {
                while alive(&m2) && !dir.join("final.json").exists() {
                    std::thread::sleep(g.interval);
                    if !matches!(enforce_budget(builder.as_ref(), g, &dir, &m2), Ok(false)) {
                        break;
                    }
                }
            });
        }
        Ok(Self::task_ref(key, &m))
    }

    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, ExecError> {
        let key = &t.task_id;
        let m = self
            .meta(key)?
            .ok_or_else(|| format!("{key}: no harness state (meta.json)"))?;
        let (tokens, cost) = self.usage_of(key)?;
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
        if let Some(g) = self.guard
            && enforce_budget(self.builder.as_ref(), g, &dir, &m)?
        {
            let (tokens, cost) = self.usage_of(key)?;
            return Ok(ExecStatus::Done(ExecResult {
                outcome: Outcome::Cancelled,
                tokens,
                cost_micro_usd: cost,
                wall_secs: elapsed,
            }));
        }
        if !alive(&m) {
            // exit.code が無いまま消えた（SIGKILL・host 再起動など）。
            self.finalize(key, Outcome::Error, elapsed, "process_vanished")?;
            return done(Outcome::Error, elapsed);
        }
        if elapsed > m.timeout_secs {
            kill_group(&m);
            self.finalize(key, Outcome::Timeout, m.timeout_secs, "trial_timeout")?;
            return done(Outcome::Timeout, m.timeout_secs);
        }
        Ok(ExecStatus::Wait)
    }

    fn usage(&self, t: &TaskRef) -> Result<Option<Usage>, ExecError> {
        let key = &t.task_id;
        let Some(m) = self.meta(key)? else {
            return Ok(None);
        };
        let (tokens, _) = self.usage_of(key)?;
        Ok(Some(Usage {
            tokens,
            wall_secs: now_secs().saturating_sub(m.started_unix),
        }))
    }

    fn observed(&self, t: &TaskRef) -> Result<Vec<ObservedRun>, String> {
        let p = self.dir(&t.task_id)?.join("observed.json");
        match fs::read(&p) {
            Ok(b) => serde_json::from_slice(&b)
                .map(|o| vec![o])
                .map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    fn cancel(&self, t: &TaskRef, reason: &str) -> Result<(), ExecError> {
        let key = &t.task_id;
        let Some(m) = self.meta(key)? else {
            return Ok(());
        };
        kill_group(&m);
        if self.read_final(key)?.is_none() && !self.dir(key)?.join("exit.code").exists() {
            let elapsed = now_secs().saturating_sub(m.started_unix);
            self.finalize(key, Outcome::Cancelled, elapsed, reason)?;
        }
        Ok(())
    }
}
