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
        let mut child = match Command::new(program)
            .args(args)
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

/// A の sandbox（実起動と同じ `Sandbox`）を arm ごとに作る。
pub fn sandbox_for(env: &CapEnv, arm: ConfigId, account_id: &str, config_dir: &Path) -> Sandbox {
    Sandbox {
        account_id: account_id.to_string(),
        config_dir: config_dir.to_path_buf(),
        toolchain: env.toolchain(arm).clone(),
    }
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
                let (ok, detail) = match runner.run(&prog, &args) {
                    Ran::Unrunnable(e) => (false, format!("sandbox did not run: {e}")),
                    Ran::Exit { code, .. } if code != 0 => (false, format!("exit {code}")),
                    Ran::Exit { stdout, .. } => match &cap.expect_stdout {
                        Some(x) if !stdout.contains(x.as_str()) => {
                            (false, format!("stdout lacks {x:?}"))
                        }
                        _ => (true, "ok".into()),
                    },
                };
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
