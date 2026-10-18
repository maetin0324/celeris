//! 必要能力の LLM なし probe（本測定移行 P1-3）。
//!
//! case の `required_capabilities` を、A が実際に使う sandbox（`Sandbox::wrap_resolved`）で包んで走らせる。
//! B/C は Celeris worker の実起動関数（`task_worker::FakeAdapter` → `run_subprocess`）に「LLM を呼ばない
//! 能力 probe run」を通し、daemon 設定の `[worker]` env・PATH で起こした子 process の中で確かめる
//! （`probe_worker`）。`CapEnv.worker` は daemon 設定と照合する期待値で、配送ではない。
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

/// daemon 設定の `[worker]` が worker 子 process に渡す PATH と env。`CapEnv.worker` の期待値と照合する
/// 対象で、probe は同じ値で子 process を起こす（campaign はこれを worker に配送しない）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkerDaemonEnv {
    pub path: Option<String>,
    pub env: Vec<(String, String)>,
}

impl WorkerDaemonEnv {
    /// この process の環境から、期待 `worker` toolchain が名指す変数だけを写す。daemon と同じ
    /// `EnvironmentFile` で起動した前提（変数が無ければ `env` に入らず、照合で食い違いになる）。
    pub fn from_process(expected: &Toolchain) -> Self {
        let env = expected
            .env
            .iter()
            .filter(|(k, _)| k != "PATH")
            .filter_map(|(k, _)| std::env::var(k).ok().map(|v| (k.clone(), v)))
            .collect();
        Self {
            path: std::env::var("PATH").ok(),
            env,
        }
    }
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 期待 env と daemon 設定の食い違い（PATH は期待の各要素が daemon の PATH に含まれること）。
fn env_mismatch(expected: &Toolchain, daemon: &WorkerDaemonEnv) -> Vec<String> {
    let mut out = Vec::new();
    for (k, want) in &expected.env {
        if k == "PATH" {
            let have: Vec<_> = daemon
                .path
                .as_deref()
                .map(|p| std::env::split_paths(p).collect())
                .unwrap_or_default();
            for dir in std::env::split_paths(want) {
                if !have.contains(&dir) {
                    out.push(format!("daemon PATH lacks {}", dir.display()));
                }
            }
        } else {
            match daemon.env.iter().find(|(dk, _)| dk == k) {
                Some((_, v)) if v == want => {}
                Some((_, v)) => out.push(format!("daemon env {k}={v:?}, expected {want:?}")),
                None => out.push(format!("daemon env lacks {k}")),
            }
        }
    }
    out
}

/// worker 子 process の中で走らせる検査（id・shell 片）。必要能力は `case.required_capabilities` そのまま、
/// それに toolchain の mount（`test -d`）と socket（`test -S`）の存在確認を足す。
fn worker_checks(expected: &Toolchain, case: &Case) -> Vec<(String, String, Option<String>)> {
    let mut v = Vec::new();
    for m in &expected.ro_mounts {
        let p = m.to_string_lossy();
        v.push((
            format!("toolchain:{p}"),
            format!("test -d {}", sh_quote(&p)),
            None,
        ));
    }
    for s in &expected.sockets {
        let p = s.to_string_lossy();
        v.push((
            format!("socket:{p}"),
            format!("test -S {}", sh_quote(&p)),
            None,
        ));
    }
    for c in &case.required_capabilities {
        let argv: Vec<_> = c.argv.iter().map(|a| sh_quote(a)).collect();
        v.push((c.id.clone(), argv.join(" "), c.expect_stdout.clone()));
    }
    v
}

struct NoSink;
impl task_worker::EventSink for NoSink {
    fn progress(&self, _: &str) {}
    fn artifact(&self, _: &task_core::ArtifactRef) {}
}

/// B/C の能力 probe。LLM を呼ばず、worker の実起動関数（`FakeAdapter` → `run_subprocess`）で、daemon 設定の
/// PATH・env・cwd（`workspace`）の子 process に検査 script を走らせ、結果を file で受け取る。
/// `CapEnv.worker` と daemon 設定の食い違い・mount/socket/道具の欠落は arm 別の `env_failure`。
pub fn probe_worker(
    env: &CapEnv,
    daemon: &WorkerDaemonEnv,
    arm: ConfigId,
    workspace: &Path,
    case: &Case,
) -> ProbeReport {
    let expected = &env.worker;
    let mut results = Vec::new();
    if case.required_capabilities.is_empty() {
        results.push(fail(
            "required_capabilities",
            "the case fixes no required_capabilities",
        ));
    }
    for m in env_mismatch(expected, daemon) {
        results.push(fail("worker-env", &m));
    }
    let checks = worker_checks(expected, case);
    match run_worker_script(daemon, workspace, &checks) {
        Err(e) => results.push(fail(
            "worker-run",
            &format!("probe run did not complete: {e}"),
        )),
        Ok(outs) => {
            for ((id, _, want), out) in checks.iter().zip(outs) {
                results.push(match out {
                    None => fail(id, "no result written by the probe script"),
                    Some((code, _)) if code != 0 => fail(id, &format!("exit {code}")),
                    Some((_, stdout)) => match want {
                        Some(x) if !stdout.contains(x.as_str()) => {
                            fail(id, &format!("stdout lacks {x:?}"))
                        }
                        _ => CapResult {
                            id: id.clone(),
                            ok: true,
                            detail: "ok".into(),
                            argv: Vec::new(),
                        },
                    },
                });
            }
        }
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

fn fail(id: &str, detail: &str) -> CapResult {
    CapResult {
        id: id.to_string(),
        ok: false,
        detail: detail.to_string(),
        argv: Vec::new(),
    }
}

type ScriptOut = Vec<Option<(i32, String)>>;

/// `FakeAdapter` に検査 script を通す。async の実起動関数を、呼び出し側の runtime とは別 thread の
/// current-thread runtime で駆動する（preflight は同期 API で、tokio の中から呼ばれることもある）。
fn run_worker_script(
    daemon: &WorkerDaemonEnv,
    workspace: &Path,
    checks: &[(String, String, Option<String>)],
) -> Result<ScriptOut, String> {
    use task_worker::{FakeAdapter, RunContext, RunLimits, RunRequest, WorkerAdapter};
    let results_dir = workspace.join("artifacts").join("cap-probe");
    let _ = std::fs::remove_dir_all(&results_dir);
    std::fs::create_dir_all(&results_dir).map_err(|e| format!("{}: {e}", results_dir.display()))?;
    let mut script = String::from("cat >/dev/null\nd=\"$1\"\n");
    for (i, (_, cmd, _)) in checks.iter().enumerate() {
        script.push_str(&format!(
            "( {cmd} ) >\"$d/{i}.out\" 2>/dev/null </dev/null; echo $? >\"$d/{i}.code\"\n"
        ));
    }
    script.push_str("echo '{\"type\":\"done\",\"summary\":\"cap-probe\",\"evidence\":[]}'\n");
    let mut adapter = FakeAdapter::new(vec![
        "/bin/sh".into(),
        "-c".into(),
        script,
        "cap-probe".into(),
        results_dir.to_string_lossy().into_owned(),
    ]);
    let mut env = daemon.env.clone();
    if let Some(p) = &daemon.path {
        env.push(("PATH".into(), p.clone()));
    }
    adapter.set_env(env);
    // `RunRequest` の Task は試験用の最小値を公開している関数で組む（この run は task として保存されない）。
    let req = RunRequest {
        protocol: task_worker::PROTOCOL_VERSION,
        task: task_worker::protocol::sample_task_for_tests(),
        workspace: workspace.to_path_buf(),
        work_dir: None,
        artifacts_dir: workspace.join("artifacts"),
        context: RunContext::default(),
        llm_route: None,
        cargo_target_dir: None,
    };
    let limits = RunLimits {
        wall_clock: Duration::from_secs(300),
        idle_timeout: Duration::from_secs(300),
        kill_grace: Duration::from_secs(5),
    };
    let outcome = std::thread::scope(|s| {
        s.spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            rt.block_on(adapter.run(req, "cap-probe", limits, &NoSink))
                .map_err(|e| e.to_string())
        })
        .join()
        .map_err(|_| "probe thread panicked".to_string())?
    })?;
    if !matches!(outcome.terminal, task_worker::Terminal::Done { .. }) {
        return Err(format!("unexpected terminal {:?}", outcome.terminal));
    }
    Ok((0..checks.len())
        .map(|i| {
            let code = std::fs::read_to_string(results_dir.join(format!("{i}.code"))).ok()?;
            let stdout =
                std::fs::read_to_string(results_dir.join(format!("{i}.out"))).unwrap_or_default();
            Some((code.trim().parse().ok()?, stdout))
        })
        .collect())
}
