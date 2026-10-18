//! 必要能力の LLM なし probe（本測定移行 P1-3）。
//!
//! case の `required_capabilities` を、A が実際に使う sandbox（`Sandbox::wrap_resolved`）で包んで走らせる。
//! B/C は Celeris worker 側の環境（`CapEnv.worker`）が A と同じ能力を持つことを同じ組立で確かめる。
//! 不足は環境不良（`env_failure`）で、モデル失敗とは別に扱う。有料呼び出しは 0 件。
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{
    cases::{Capability, Case},
    harness_exec::{HarnessCommand, Sandbox, Toolchain},
    manifest::ConfigId,
};

pub const CAP_ENV_SCHEMA: &str = "celeris.campaign-cap-env/1";
pub const CAP_REPORT_SCHEMA: &str = "celeris.campaign-cap-probe/1";
/// モデル失敗と区別する分類名。
pub const ENV_FAILURE: &str = "env_failure";

/// 環境側の設定。`a` は A の sandbox に足す固定 toolchain、`worker` は B/C を実行する worker 環境に
/// 同じ能力を与える設定。両者が同じ集合でなければ読み込みで拒む（B/C に同等の能力を与える）。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapEnv {
    pub schema: String,
    pub a: Toolchain,
    pub worker: Toolchain,
}

fn canon(t: &Toolchain) -> Toolchain {
    let mut t = t.clone();
    t.ro_mounts.sort();
    t.sockets.sort();
    t.env.sort();
    t
}

impl CapEnv {
    pub fn load(p: &Path) -> Result<Self, String> {
        let b = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let e: Self = serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display()))?;
        e.validate(std::env::var_os("HOME").map(PathBuf::from).as_deref())?;
        Ok(e)
    }

    pub fn validate(&self, home: Option<&Path>) -> Result<(), String> {
        if self.schema != CAP_ENV_SCHEMA {
            return Err(format!("cap env schema must be {CAP_ENV_SCHEMA}"));
        }
        self.a.validate(home)?;
        self.worker.validate(home)?;
        if let Some((k, _)) = self.a.env.iter().find(|(k, _)| secret_like(k)) {
            return Err(format!(
                "cap env must not carry secrets ({k:?}); use the existing secret management"
            ));
        }
        if canon(&self.a) != canon(&self.worker) {
            return Err(
                "cap env: worker toolchain must equal A toolchain (B/C need the same capabilities)"
                    .into(),
            );
        }
        Ok(())
    }

    pub fn toolchain(&self, arm: ConfigId) -> &Toolchain {
        if arm == ConfigId::A {
            &self.a
        } else {
            &self.worker
        }
    }
}

/// 固定する mount の実体。path と実 path（symlink 解決後）と直下の名前一覧の sha256（内容・版の差し替えを拾う）。
/// socket は経路の識別（path）だけを固定する。接続先の内容・到達性は固定せず、probe が毎回確かめる。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MountPin {
    pub path: String,
    pub real: String,
    pub entries_sha256: String,
}

/// env-lock に結び付ける正規化済み CAP_ENV（配列順を整えた toolchain）と mount の実体。秘密値・socket の内容は持たない。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapEnvPin {
    pub schema: String,
    pub toolchain: Toolchain,
    pub mounts: Vec<MountPin>,
}

fn secret_like(k: &str) -> bool {
    let u = k.to_ascii_uppercase();
    ["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL"]
        .iter()
        .any(|w| u.contains(w))
}

impl CapEnv {
    /// 正規化した固定値。toolchain 版/実体の取得はここ 1 か所（作業 1 の probe も同じ `CapEnv` を使う）。
    pub fn pin(&self) -> Result<CapEnvPin, String> {
        let t = canon(&self.a);
        let mut mounts = Vec::new();
        for p in &t.ro_mounts {
            let real = std::fs::canonicalize(p)
                .map_err(|e| format!("cap env ro_mount {}: {e}", p.display()))?;
            let mut names: Vec<String> = match std::fs::read_dir(&real) {
                Ok(d) => d
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect(),
                Err(_) => Vec::new(),
            };
            names.sort();
            mounts.push(MountPin {
                path: p.to_string_lossy().into_owned(),
                real: real.to_string_lossy().into_owned(),
                entries_sha256: format!(
                    "{:x}",
                    <sha2::Sha256 as sha2::Digest>::digest(names.join("\n").as_bytes())
                ),
            });
        }
        Ok(CapEnvPin {
            schema: self.schema.clone(),
            toolchain: t,
            mounts,
        })
    }
}

/// `--cap-env` があれば A の sandbox に足す toolchain。実起動と probe が同じ値を使う。
pub fn load_toolchain(args: &[String]) -> Result<Toolchain, String> {
    match args
        .iter()
        .position(|a| a == "--cap-env")
        .and_then(|i| args.get(i + 1))
    {
        Some(p) => Ok(CapEnv::load(Path::new(p))?.a),
        None => Ok(Toolchain::default()),
    }
}

/// probe 1 件の実行結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ran {
    Exit { code: i32, stdout: String },
    Unrunnable(String),
}

/// sandbox 済み argv の実行器。試験は stub を渡す（userns が要る実走は人手順）。
pub trait ProbeRunner {
    fn run(&self, program: &str, args: &[String]) -> Ran;
}

pub struct ProcessProbeRunner {
    pub timeout: Duration,
}

impl ProbeRunner for ProcessProbeRunner {
    fn run(&self, program: &str, args: &[String]) -> Ran {
        let mut c = Command::new(program);
        c.args(args);
        self.run_command(c)
    }
}

impl ProcessProbeRunner {
    pub fn run_command(&self, mut c: Command) -> Ran {
        let program = c.get_program().to_string_lossy().into_owned();
        let mut child = match c
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return Ran::Unrunnable(format!("{program}: {e}")),
        };
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if start.elapsed() > self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ran::Unrunnable(format!("timed out after {:?}", self.timeout));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Ran::Unrunnable(e.to_string()),
            }
        }
        match child.wait_with_output() {
            Ok(o) => Ran::Exit {
                code: o.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            },
            Err(e) => Ran::Unrunnable(e.to_string()),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CapResult {
    pub id: String,
    pub ok: bool,
    pub detail: String,
    /// 実際に runner へ渡した `bwrap` の引数（A の起動と同じ組立）。
    pub argv: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProbeReport {
    pub schema: &'static str,
    pub case: String,
    pub arm: ConfigId,
    pub ok: bool,
    /// 不足があるときだけ `env_failure`。モデル失敗ではない。
    pub failure_class: Option<&'static str>,
    /// 常に 0（LLM も job 投入もしない）。
    pub paid_calls_made: u32,
    pub results: Vec<CapResult>,
}

/// A の sandbox（実起動と同じ `Sandbox`）を arm ごとに作る。probe は `state_dir` を
/// 試行 state dir と認証材料の置き場の両方に使う（LLM を呼ばないので認証材料は空 file でよい）。
pub fn sandbox_for(env: &CapEnv, arm: ConfigId, account_id: &str, state_dir: &Path) -> Sandbox {
    Sandbox {
        account_id: account_id.to_string(),
        auth_dir: state_dir.to_path_buf(),
        state_root: state_dir.to_path_buf(),
        toolchain: env.toolchain(arm).clone(),
    }
}

/// probe の state dir を用意する（無ければ作り、認証材料の mount 元に空 file を置く）。
pub fn prepare_state_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let cred = dir.join(".credentials.json");
    if !cred.exists() {
        std::fs::File::create(&cred).map_err(|e| format!("{}: {e}", cred.display()))?;
    }
    Ok(())
}

fn resolve_in(sb: &Sandbox, program: &str) -> Result<PathBuf, String> {
    let tool_path = sb
        .toolchain
        .env
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone());
    let host = std::env::var_os("PATH");
    let joined = match (tool_path, host) {
        (Some(t), Some(h)) => {
            let mut p = std::env::split_paths(&t).collect::<Vec<_>>();
            p.extend(std::env::split_paths(&h));
            std::env::join_paths(p).ok()
        }
        (Some(t), None) => Some(t.into()),
        (None, h) => h,
    };
    crate::preflight::resolve_executable(Path::new(program), joined.as_deref())
}

/// capability 1 件の sandbox 済み (program, args)。A の `Sandbox::wrap_resolved` を通す。
pub fn sandboxed(
    sb: &Sandbox,
    workspace: &Path,
    cap: &Capability,
) -> Result<(String, Vec<String>), String> {
    let (prog, rest) = cap.argv.split_first().ok_or("empty argv")?;
    let real = resolve_in(sb, prog)?;
    let w = sb.wrap_resolved(
        &HarnessCommand {
            program: PathBuf::from(prog),
            args: rest.to_vec(),
            env: Vec::new(),
        },
        workspace,
        &sb.state_root,
        &real,
    )?;
    Ok((w.program.to_string_lossy().into_owned(), w.args))
}

/// `--dry-argv`: 走らせずに argv だけ組む。
pub fn dry_argv(
    sb: &Sandbox,
    workspace: &Path,
    case: &Case,
) -> Vec<(String, Result<Vec<String>, String>)> {
    case.required_capabilities
        .iter()
        .map(|c| (c.id.clone(), sandboxed(sb, workspace, c).map(|(_, a)| a)))
        .collect()
}

pub fn probe(
    sb: &Sandbox,
    arm: ConfigId,
    workspace: &Path,
    case: &Case,
    runner: &dyn ProbeRunner,
) -> ProbeReport {
    let mut results = Vec::new();
    if case.required_capabilities.is_empty() {
        results.push(CapResult {
            id: "required_capabilities".into(),
            ok: false,
            detail: "the case fixes no required_capabilities".into(),
            argv: Vec::new(),
        });
    }
    for cap in &case.required_capabilities {
        let r = match sandboxed(sb, workspace, cap) {
            Err(e) => CapResult {
                id: cap.id.clone(),
                ok: false,
                detail: format!("not available: {e}"),
                argv: Vec::new(),
            },
            Ok((prog, args)) => {
                let (ok, detail) = judge(cap, runner.run(&prog, &args));
                CapResult {
                    id: cap.id.clone(),
                    ok,
                    detail,
                    argv: args,
                }
            }
        };
        results.push(r);
    }
    let ok = results.iter().all(|r| r.ok);
    ProbeReport {
        schema: CAP_REPORT_SCHEMA,
        case: case.id.clone(),
        arm,
        ok,
        failure_class: (!ok).then_some(ENV_FAILURE),
        paid_calls_made: 0,
        results,
    }
}

/// 1 件の実行結果を能力の期待（exit 0・`expect_stdout`）に照らして判定する（A の sandbox 経路と worker 経路で共通）。
fn judge(cap: &Capability, ran: Ran) -> (bool, String) {
    match ran {
        Ran::Unrunnable(e) => (false, format!("did not run: {e}")),
        Ran::Exit { code, .. } if code != 0 => (false, format!("exit {code}")),
        Ran::Exit { stdout, .. } => match &cap.expect_stdout {
            Some(x) if !stdout.contains(x.as_str()) => (false, format!("stdout lacks {x:?}")),
            _ => (true, "ok".into()),
        },
    }
}

/// B/C を実行する worker 環境そのもの（測定専用 daemon が起動する worker process の環境）。A の sandbox は通さない。
pub trait WorkerHost {
    /// worker 環境から見える承認済み接続 socket か。
    fn socket_present(&self, path: &Path) -> bool;
    /// worker 環境から見える toolchain / library の固定 path か。
    fn dir_present(&self, path: &Path) -> bool;
    /// worker が起動する環境（`env` だけを持つ空環境）で `argv` を走らせる。
    fn run(&self, env: &[(String, String)], argv: &[String], workspace: &Path) -> Ran;
}

/// 実 host 上の worker 環境。`env_clear` した process に worker 設定の env だけを渡す。
pub struct ProcessWorkerHost {
    pub timeout: Duration,
}

impl WorkerHost for ProcessWorkerHost {
    fn socket_present(&self, path: &Path) -> bool {
        use std::os::unix::fs::FileTypeExt;
        std::fs::metadata(path).is_ok_and(|m| m.file_type().is_socket())
    }
    fn dir_present(&self, path: &Path) -> bool {
        path.is_dir()
    }
    fn run(&self, env: &[(String, String)], argv: &[String], workspace: &Path) -> Ran {
        let Some((prog, rest)) = argv.split_first() else {
            return Ran::Unrunnable("empty argv".into());
        };
        let path = env
            .iter()
            .rfind(|(k, _)| k == "PATH")
            .map(|(_, v)| std::ffi::OsString::from(v));
        let real = match crate::preflight::resolve_executable(Path::new(prog), path.as_deref()) {
            Ok(r) => r,
            Err(e) => return Ran::Unrunnable(e),
        };
        let mut c = Command::new(real);
        c.args(rest).env_clear().current_dir(workspace);
        c.envs(env.iter().map(|(k, v)| (k, v)));
        ProcessProbeRunner {
            timeout: self.timeout,
        }
        .run_command(c)
    }
}

/// B/C: 実 worker 環境（`CapEnv.worker`）で必要能力を確かめる。A の probe 結果の arm 付け替えではなく、
/// worker が持つべき socket・固定 path を worker 側で確認し、能力 command を worker の環境で走らせる。
pub fn probe_worker(
    env: &CapEnv,
    arm: ConfigId,
    workspace: &Path,
    case: &Case,
    host: &dyn WorkerHost,
) -> ProbeReport {
    let mut results = Vec::new();
    if case.required_capabilities.is_empty() {
        results.push(CapResult {
            id: "required_capabilities".into(),
            ok: false,
            detail: "the case fixes no required_capabilities".into(),
            argv: Vec::new(),
        });
    }
    let t = &env.worker;
    for s in &t.sockets {
        let ok = host.socket_present(s);
        results.push(CapResult {
            id: format!("worker-socket:{}", s.display()),
            ok,
            detail: if ok {
                "ok"
            } else {
                "approved socket is not present in the worker environment"
            }
            .into(),
            argv: Vec::new(),
        });
    }
    for m in &t.ro_mounts {
        let ok = host.dir_present(m);
        results.push(CapResult {
            id: format!("worker-path:{}", m.display()),
            ok,
            detail: if ok {
                "ok"
            } else {
                "toolchain path is not present in the worker environment"
            }
            .into(),
            argv: Vec::new(),
        });
    }
    let mut wenv: Vec<(String, String)> = vec![
        ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
        ("HOME".into(), "/tmp/home".into()),
    ];
    wenv.extend(t.env.iter().cloned());
    for cap in &case.required_capabilities {
        let (ok, detail) = judge(cap, host.run(&wenv, &cap.argv, workspace));
        results.push(CapResult {
            id: cap.id.clone(),
            ok,
            detail,
            argv: cap.argv.clone(),
        });
    }
    let ok = results.iter().all(|r| r.ok);
    ProbeReport {
        schema: CAP_REPORT_SCHEMA,
        case: case.id.clone(),
        arm,
        ok,
        failure_class: (!ok).then_some(ENV_FAILURE),
        paid_calls_made: 0,
        results,
    }
}
