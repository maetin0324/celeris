//! A arm 用の実行器: manifest の harness（例 claude code）を subprocess で包み、trial workspace 上で走らせる。
//!
//! A は「既存の強い運用 baseline」であり、Celeris の routing・turn 制限・許可範囲を通さない（後付けの
//! `--max-turns`・許可リスト・tool 制限は足さない）。足すのは次の 3 つだけ（ADR 2026-10-17-mlsys-pilot2-fixes）:
//! - D2 ファイルシステム隔離: `Sandbox` が command を bubblewrap で包み、試行の workspace・試行ごとの state dir
//!   （`CLAUDE_CONFIG_DIR`。認証材料 `.credentials.json` だけを read-only で重ねる。ADR 2026-10-18-a-trial-scoped-state）・
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

    /// preflight が解決した実行ファイルの版。実起動の記録（`launch.json`）に残す。
    fn program_version(&self) -> Option<String> {
        None
    }

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
    pub version: Option<String>,
}

impl ClaudeCodeBuilder {
    /// preflight が解決した実行ファイル（絶対 path）と版で作る。PATH を引き直さない。
    pub fn from_resolved(p: &crate::preflight::ResolvedProgram) -> Self {
        Self {
            program: p.path.clone(),
            version: Some(p.version.clone()),
        }
    }
}

impl CommandBuilder for ClaudeCodeBuilder {
    fn program_version(&self) -> Option<String> {
        self.version.clone()
    }

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

/// A のファイルシステム隔離（bubblewrap）。ADR 2026-10-17-mlsys-pilot2-fixes D2、
/// ADR 2026-10-18-a-trial-scoped-state（認証と可変状態の分離）。
#[derive(Clone, Debug)]
pub struct Sandbox {
    /// A が使う Celeris 管理アカウント（claude_oauth）の id。
    pub account_id: String,
    /// そのアカウント dir。ここからは `.credentials.json` だけを read-only で渡す（dir 全体は bind しない）。
    pub auth_dir: PathBuf,
    /// 試行 state dir の親（campaign の出力領域の下）。試行ごとの `CLAUDE_CONFIG_DIR` はこの下に決定的に置く。
    pub state_root: PathBuf,
    /// 固定 toolchain の読み取り専用 mount・管理接続 socket・環境変数（既定は空 = 追加なし）。
    pub toolchain: Toolchain,
}

/// 試行 identity marker の file 名（試行 state dir の直下）。
pub const TRIAL_MARKER: &str = "trial-identity.json";
/// 認証材料の file 名。
const CREDENTIALS: &str = ".credentials.json";

/// 試行 state dir に置く identity marker（lock を兼ねる: 別の識別の試行が同じ dir を使うのを拒む）。
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct TrialMarker {
    pub campaign_id: String,
    pub manifest_sha256: String,
    pub arm: String,
    pub case: String,
    pub rep: u32,
    pub account_id: String,
}

/// 試行 state dir の path（`Sandbox::state_dir` と preflight が同じ規則を使う）。
pub fn trial_state_dir(root: &Path, campaign_id: &str, arm: &str, case: &str, rep: u32) -> PathBuf {
    root.join(path_component(campaign_id))
        .join(path_component(arm))
        .join(path_component(case))
        .join(format!("rep{rep}"))
}

/// path の 1 要素に使える形へ（英数・`-_.` 以外は `%xx`）。`.` 始まりも `%2e` にして `..` を作らせない。
fn path_component(s: &str) -> String {
    let mut o = String::new();
    for (i, b) in s.bytes().enumerate() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || (b == b'.' && i > 0) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02x}"));
        }
    }
    if o.is_empty() { "%".into() } else { o }
}

/// sandbox を無効にせず、必要な道具だけを足す固定の追加分。HOME 全公開や `/` の bind は `validate` が拒む。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Toolchain {
    /// 読み取り専用で足す絶対 path（例: UCX の prefix）。
    #[serde(default)]
    pub ro_mounts: Vec<PathBuf>,
    /// 管理された接続経路の unix socket（そのファイルだけを bind。鍵や ~/.ssh は入れない）。
    #[serde(default)]
    pub sockets: Vec<PathBuf>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

impl Toolchain {
    pub fn validate(&self, home: Option<&Path>) -> Result<(), String> {
        const DENIED: [&str; 8] = [
            "/", "/home", "/root", "/proc", "/dev", "/sys", "/run", "/tmp",
        ];
        for p in self.ro_mounts.iter().chain(&self.sockets) {
            let ok_shape = p.is_absolute()
                && p.components().all(|c| {
                    !matches!(
                        c,
                        std::path::Component::ParentDir | std::path::Component::CurDir
                    )
                });
            if !ok_shape {
                return Err(format!(
                    "toolchain path must be absolute and normal: {}",
                    p.display()
                ));
            }
            if DENIED.iter().any(|d| p == Path::new(d)) {
                return Err(format!("toolchain must not expose {}", p.display()));
            }
            if let Some(h) = home.filter(|h| !h.as_os_str().is_empty() && *h != Path::new("/"))
                && h.starts_with(p)
            {
                return Err(format!("toolchain {} covers HOME", p.display()));
            }
        }
        if let Some((k, _)) = self.env.iter().find(|(k, _)| {
            matches!(
                k.as_str(),
                "HOME" | "CLAUDE_CONFIG_DIR" | SUBMIT_AUDIT_ENV | TRIAL_KEY_ENV
            ) || k.is_empty()
                || k.contains('=')
        }) {
            return Err(format!("toolchain env must not set {k:?}"));
        }
        Ok(())
    }
}

/// 実体の解決は preflight と同じ関数（検査対象と実起動をずらさない）。
fn resolve_program(p: &Path) -> Result<PathBuf, String> {
    crate::preflight::resolve_executable(p, std::env::var_os("PATH").as_deref())
}

impl Sandbox {
    /// bwrap に渡す引数（`bwrap` 自身は含まない）。`real` は解決済みの実行ファイル。
    /// 見える物は固定: 読み取り専用の `/usr` と `/etc`・`/bin` 等、`/proc` `/dev`、空の `/tmp`、
    /// workspace と試行 state dir（書き込み可）、state dir 内に重ねた認証材料（read-only）、実行ファイル本体。
    /// アカウント dir 全体・他試行の state dir・それ以外の host の path は存在しない。
    pub fn args(
        &self,
        workspace: &Path,
        state: &Path,
        real: &Path,
        env: &[(String, String)],
    ) -> Vec<String> {
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
            state.to_string_lossy().into_owned(),
            real.to_string_lossy().into_owned(),
        );
        let cred_src = self
            .auth_dir
            .join(CREDENTIALS)
            .to_string_lossy()
            .into_owned();
        let cred_dst = state.join(CREDENTIALS).to_string_lossy().into_owned();
        add(&["--ro-bind", &real_s, &real_s]);
        add(&["--bind", &ws, &ws, "--bind", &cfg, &cfg]);
        add(&["--ro-bind", &cred_src, &cred_dst, "--chdir", &ws]);
        for m in &self.toolchain.ro_mounts {
            let m = m.to_string_lossy();
            add(&["--ro-bind", &m, &m]);
        }
        for m in &self.toolchain.sockets {
            let m = m.to_string_lossy();
            add(&["--bind", &m, &m]);
        }
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
        for (k, v) in self.toolchain.env.iter().chain(env) {
            add(&["--setenv", k, v]);
        }
        a
    }

    /// 試行の state dir（campaign / arm / case / repetition から決まる安定した path）。
    pub fn state_dir(&self, req: &TrialRequest) -> PathBuf {
        trial_state_dir(
            &self.state_root,
            req.campaign_id,
            &format!("{:?}", req.config.id),
            req.problem_id,
            req.rep,
        )
    }

    /// state dir を用意し、identity marker を書く（既にあれば一致を確かめる）。認証材料の mount 先も空 file で置く。
    /// 同じ試行の再開は dir を保ち、別の識別の marker が居れば拒否する（他試行の状態は使わない）。
    pub fn prepare(&self, req: &TrialRequest) -> Result<PathBuf, String> {
        let dir = self.state_dir(req);
        let want = TrialMarker {
            campaign_id: req.campaign_id.into(),
            manifest_sha256: req.manifest_sha256.into(),
            arm: format!("{:?}", req.config.id),
            case: req.problem_id.into(),
            rep: req.rep,
            account_id: self.account_id.clone(),
        };
        fs::create_dir_all(&dir).map_err(|e| format!("trial state dir {}: {e}", dir.display()))?;
        let mp = dir.join(TRIAL_MARKER);
        match fs::read(&mp) {
            Ok(b) => {
                let have: TrialMarker = serde_json::from_slice(&b)
                    .map_err(|e| format!("{}: unreadable identity marker: {e}", mp.display()))?;
                if have != want {
                    return Err(format!(
                        "{}: identity marker belongs to another trial ({:?}); refusing to reuse",
                        mp.display(),
                        have
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 新規 dir は空でなければならない（marker 無しの既存状態を引き継がない）。
                let stray = fs::read_dir(&dir)
                    .map_err(|e| e.to_string())?
                    .filter_map(Result::ok)
                    .any(|e| e.file_name() != CREDENTIALS);
                if stray {
                    return Err(format!(
                        "{}: state dir has content but no identity marker; refusing to adopt it",
                        dir.display()
                    ));
                }
                let b = serde_json::to_vec(&want).map_err(|e| e.to_string())?;
                write_synced(&mp, &b)?;
            }
            Err(e) => return Err(format!("{}: {e}", mp.display())),
        }
        let src = self.auth_dir.join(CREDENTIALS);
        if !src.is_file() {
            return Err(format!(
                "account {}: {} is not a file (no credentials to pass)",
                self.account_id,
                src.display()
            ));
        }
        let dst = dir.join(CREDENTIALS);
        if !dst.exists() {
            File::create(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        }
        Ok(dir)
    }

    pub fn wrap(
        &self,
        c: &HarnessCommand,
        workspace: &Path,
        state: &Path,
    ) -> Result<HarnessCommand, String> {
        let real = resolve_program(&c.program)?;
        self.wrap_resolved(c, workspace, state, &real)
    }

    /// `wrap` の本体（実行ファイルを解決済みで渡す）。A の起動と cap-probe が同じ組立を通る。
    pub fn wrap_resolved(
        &self,
        c: &HarnessCommand,
        workspace: &Path,
        state: &Path,
        real: &Path,
    ) -> Result<HarnessCommand, String> {
        let real = real.to_path_buf();
        let mut args = self.args(workspace, state, &real, &c.env);
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
    /// submit wrapper が受理した job を追記する file（[`SUBMIT_AUDIT_ENV`]）。`track` が読む。旧 meta は None。
    #[serde(default)]
    submit_audit: Option<PathBuf>,
}

/// 試行内の submit wrapper（`scripts/campaign/celeris-qsub`）に渡す環境変数: 受理記録の追記先と trial の冪等キー。
pub const SUBMIT_AUDIT_ENV: &str = "CELERIS_SUBMIT_AUDIT";
pub const TRIAL_KEY_ENV: &str = crate::observe::TRIAL_MARKER_VAR;
/// 受理記録の file 名（試行 state dir、sandbox 無しは executor の key dir）。
pub const SUBMIT_AUDIT_FILE: &str = "submit-audit.jsonl";
/// 受理記録 1 行の schema。
pub const SUBMIT_AUDIT_SCHEMA: &str = "celeris.submit-audit/1";

/// 受理記録から、この trial の job 参照（`scheduler:cluster:job_id`）を順序どおり重複なく取り出す。
/// 別 trial の行・scheduler の job id の形でない行・壊れた行は採らない（scheduler 側の照合は collect が行う）。
pub fn audited_jobs(text: &str, key: &str) -> Vec<String> {
    let safe = |s: &str| {
        !s.is_empty()
            && !s.starts_with('-')
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    let mut v: Vec<String> = Vec::new();
    for l in text.lines() {
        let Ok(r) = serde_json::from_str::<Value>(l.trim()) else {
            continue;
        };
        let (Some(k), Some(s), Some(c), Some(id)) = (
            r["trial_key"].as_str(),
            r["scheduler"].as_str(),
            r["cluster"].as_str(),
            r["job_id"].as_str(),
        ) else {
            continue;
        };
        if r["schema"] != SUBMIT_AUDIT_SCHEMA
            || k != key
            || !safe(s)
            || !safe(c)
            || !crate::observe::is_scheduler_job_id(id)
        {
            continue;
        }
        let j = format!("{s}:{c}:{id}");
        if !v.contains(&j) {
            v.push(j);
        }
    }
    v
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

        let mut c = self.builder.build(req);
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
        // 実起動の記録: 起動する実行ファイル・版・argv（prompt は長さだけ）。検査した物と照合できる。
        let launch = serde_json::json!({
            "program": c.program,
            "version": self.builder.program_version(),
            "argv": c.args.iter().map(|a| if a == req.prompt {
                format!("<prompt {} bytes>", a.len())
            } else {
                a.clone()
            }).collect::<Vec<_>>(),
            "sandboxed": self.sandbox.is_some(),
        });
        write_synced(
            &dir.join("launch.json"),
            &serde_json::to_vec(&launch).map_err(|e| e.to_string())?,
        )?;
        // 受理記録の追記先: sandbox では試行 state dir（書き込み可で bind 済み）、無ければ key dir。
        let state = match &self.sandbox {
            Some(sb) => Some(sb.prepare(req)?),
            None => None,
        };
        let audit = state.as_deref().unwrap_or(&dir).join(SUBMIT_AUDIT_FILE);
        c.env.push((
            SUBMIT_AUDIT_ENV.into(),
            audit.to_string_lossy().into_owned(),
        ));
        c.env.push((TRIAL_KEY_ENV.into(), key.into()));
        let c = match (&self.sandbox, &state) {
            (Some(sb), Some(state)) => sb.wrap(&c, req.workspace, state)?,
            _ => c,
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
            submit_audit: Some(audit),
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
                submit_audit: m.submit_audit.clone(),
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
            // 上の exit.code 確認の後〜ここまでの間に終了した process は exit.code を書き終えている。
            // 先に確認した結果で「消えた」と決めず、次の poll で exit.code を読ませる。
            if dir.join("exit.code").exists() || dir.join("final.json").exists() {
                return Ok(ExecStatus::Wait);
            }
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

    /// submit wrapper の受理記録から、この trial の cluster job を参照に足す（台帳へ追記される）。既に記録した ID は保つ。
    fn track(&self, t: &TaskRef) -> Result<TaskRef, ExecError> {
        let key = &t.task_id;
        let Some(m) = self.meta(key)? else {
            return Ok(t.clone());
        };
        let path = m
            .submit_audit
            .unwrap_or_else(|| self.state_dir.join(key).join(SUBMIT_AUDIT_FILE));
        let text = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(t.clone()),
            Err(e) => {
                return Err(ExecError::interrupted(format!("{}: {e}", path.display())));
            }
        };
        let mut out = t.clone();
        for j in audited_jobs(&text, key) {
            if !out.cluster_job_ids.contains(&j) {
                out.cluster_job_ids.push(j);
            }
        }
        Ok(out)
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
