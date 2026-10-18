//! 必要能力の LLM なし probe（本測定移行 P1-3）。
//!
//! case の `required_capabilities` を、A が実際に使う sandbox（`Sandbox::wrap_resolved`）で包んで走らせる。
//! B/C は Celeris worker の実起動関数（`task_worker::FakeAdapter` → `run_subprocess`）に「LLM を呼ばない
//! 能力 probe run」を通し、測定専用 daemon の PATH・env で起こした子 process の中で確かめる
//! （`probe_worker`）。環境は検査先の測定専用 daemon process から（`WorkerDaemonEnv::from_daemon_pid`）取り、
//! この CLI の環境は使わない。`CapEnv.worker` は daemon の環境と照合する期待値で、配送ではない。
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

/// 固定する mount の実体。path と実 path（symlink 解決後）と、mount 配下の内容 snapshot の sha256。
///
/// 固定範囲は `ro_mounts` に挙げた各 mount の配下だけ（`/usr` 全体のような親は走査しない。広すぎる mount は
/// 上限超過で検証不能にする）。snapshot は相対 path の昇順に、通常 file は内容、directory は名前、
/// symbolic link は link の文字列（辿らない。辿った先が通常 file ならその内容も）を畳む。
/// 未検査: mount 外、directory を指す symlink の先、file の mode/owner/mtime、特殊 file（socket 等）の中身。
/// socket は経路の識別（path）だけを固定する。接続先の内容・到達性は固定せず、probe が毎回確かめる。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MountPin {
    pub path: String,
    pub real: String,
    pub content_sha256: String,
    pub entries: u64,
    pub bytes: u64,
}

/// 1 mount あたりの走査上限。超えたら空や成功にせず検証不能（mount を絞る）。
const PIN_MAX_ENTRIES: u64 = 50_000;
const PIN_MAX_BYTES: u64 = 1 << 30;

struct Walk {
    h: sha2::Sha256,
    entries: u64,
    bytes: u64,
}

impl Walk {
    fn put(&mut self, tag: &str, rel: &str, extra: &[u8]) {
        use sha2::Digest;
        self.h.update(tag.as_bytes());
        self.h.update((rel.len() as u64).to_le_bytes());
        self.h.update(rel.as_bytes());
        self.h.update((extra.len() as u64).to_le_bytes());
        self.h.update(extra);
    }

    fn file(&mut self, tag: &str, rel: &str, p: &Path) -> Result<(), String> {
        use sha2::Digest;
        use std::io::Read;
        let mut f =
            std::fs::File::open(p).map_err(|e| format!("cap env pin {}: {e}", p.display()))?;
        let mut inner = sha2::Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = f
                .read(&mut buf)
                .map_err(|e| format!("cap env pin {}: {e}", p.display()))?;
            if n == 0 {
                break;
            }
            self.bytes += n as u64;
            if self.bytes > PIN_MAX_BYTES {
                return Err(format!(
                    "cap env pin: mount exceeds {PIN_MAX_BYTES} bytes; narrow ro_mounts"
                ));
            }
            inner.update(&buf[..n]);
        }
        self.put(tag, rel, &inner.finalize());
        Ok(())
    }

    fn visit(&mut self, dir: &Path, rel: &str) -> Result<(), String> {
        let rd =
            std::fs::read_dir(dir).map_err(|e| format!("cap env pin {}: {e}", dir.display()))?;
        let mut names = Vec::new();
        for e in rd {
            let e = e.map_err(|e| format!("cap env pin {}: {e}", dir.display()))?;
            names.push(e.file_name());
        }
        names.sort();
        for n in names {
            let path = dir.join(&n);
            let r = format!("{rel}/{}", n.to_string_lossy());
            self.entries += 1;
            if self.entries > PIN_MAX_ENTRIES {
                return Err(format!(
                    "cap env pin: mount exceeds {PIN_MAX_ENTRIES} entries; narrow ro_mounts"
                ));
            }
            let md = std::fs::symlink_metadata(&path)
                .map_err(|e| format!("cap env pin {}: {e}", path.display()))?;
            let ft = md.file_type();
            if ft.is_symlink() {
                let t = std::fs::read_link(&path)
                    .map_err(|e| format!("cap env pin {}: {e}", path.display()))?;
                self.put("L", &r, t.to_string_lossy().as_bytes());
                // 辿った先が通常 file ならその内容も畳む。directory・dangling は link 文字列だけ（未検査）。
                if let Ok(m) = std::fs::metadata(&path)
                    && m.is_file()
                {
                    self.file("T", &r, &path)?;
                }
            } else if ft.is_dir() {
                self.put("D", &r, b"");
                self.visit(&path, &r)?;
            } else if ft.is_file() {
                self.file("F", &r, &path)?;
            } else {
                self.put("S", &r, b"");
            }
        }
        Ok(())
    }
}

fn pin_mount(p: &Path) -> Result<MountPin, String> {
    use sha2::Digest;
    let real =
        std::fs::canonicalize(p).map_err(|e| format!("cap env ro_mount {}: {e}", p.display()))?;
    let md =
        std::fs::metadata(&real).map_err(|e| format!("cap env ro_mount {}: {e}", p.display()))?;
    let mut w = Walk {
        h: sha2::Sha256::new(),
        entries: 0,
        bytes: 0,
    };
    if md.is_dir() {
        w.visit(&real, "")?;
    } else if md.is_file() {
        w.entries = 1;
        w.file("F", "", &real)?;
    } else {
        return Err(format!(
            "cap env ro_mount {}: neither a file nor a directory",
            p.display()
        ));
    }
    Ok(MountPin {
        path: p.to_string_lossy().into_owned(),
        real: real.to_string_lossy().into_owned(),
        content_sha256: format!("{:x}", w.h.finalize()),
        entries: w.entries,
        bytes: w.bytes,
    })
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
            mounts.push(pin_mount(p)?);
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
    /// B/C: 検査先の測定専用 daemon の識別（A は None）。
    pub daemon: Option<DaemonRef>,
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
        daemon: None,
        results,
    }
}

/// 測定専用 daemon の worker 環境（daemon process が持つ PATH と、期待 toolchain が名指す変数）。probe は
/// 同じ値だけで子 process を起こす（CLI 自身の環境は使わない）。campaign はこれを worker に配送しない。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkerDaemonEnv {
    pub path: Option<String>,
    pub env: Vec<(String, String)>,
    /// 検査先の daemon（`from_daemon_pid` が埋める。報告に残す識別）。
    pub source: Option<DaemonRef>,
}

/// 検査先 daemon の識別。秘密を含まない（pid・開始時刻・identity が答えた instance/release）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DaemonRef {
    pub pid: u32,
    /// `/proc/<pid>/stat` の starttime（pid 再利用の区別）。
    pub starttime: u64,
    pub instance_id: Option<String>,
    pub release: Option<String>,
}

/// 子 process の環境に持ち込んでよい、daemon の非秘密変数（ホーム・言語・端末）。期待 toolchain が名指す
/// 変数と PATH に加えて、daemon の値をそのまま写す。それ以外（token・credential 等）は一切写さない。
const DAEMON_ENV_ALLOW: &[&str] = &["HOME", "LANG", "LC_ALL", "TERM", "USER", "LOGNAME"];

impl WorkerDaemonEnv {
    /// 測定専用 daemon の process（`/proc/<pid>/environ`）から、PATH・期待 toolchain が名指す変数・
    /// `DAEMON_ENV_ALLOW` だけを取り出す。環境全体は保持も記録もしない（他の変数は読んだ buffer ごと捨てる）。
    /// CLI 自身の環境は見ない。pid が測定 daemon のものであることは呼び出し側が identity 照合で保証する。
    pub fn from_daemon_pid(pid: u32, expected: &Toolchain) -> Result<Self, String> {
        let environ = std::fs::read(format!("/proc/{pid}/environ"))
            .map_err(|e| format!("daemon pid {pid}: cannot read its worker environment: {e}"))?;
        let starttime = proc_starttime(pid)?;
        let want = |k: &str| {
            k == "PATH"
                || DAEMON_ENV_ALLOW.contains(&k)
                || expected.env.iter().any(|(ek, _)| ek == k)
        };
        let (mut path, mut env) = (None, Vec::new());
        for kv in environ.split(|b| *b == 0).filter(|x| !x.is_empty()) {
            let Some(eq) = kv.iter().position(|b| *b == b'=') else {
                continue;
            };
            let (Ok(k), Ok(v)) = (
                std::str::from_utf8(&kv[..eq]),
                std::str::from_utf8(&kv[eq + 1..]),
            ) else {
                continue;
            };
            if !want(k) {
                continue;
            }
            if k == "PATH" {
                path = Some(v.to_string());
            } else {
                env.push((k.to_string(), v.to_string()));
            }
        }
        Ok(Self {
            path,
            env,
            source: Some(DaemonRef {
                pid,
                starttime,
                instance_id: None,
                release: None,
            }),
        })
    }
}

/// `/proc/<pid>/stat` の starttime（22 番目の欄。comm に空白・括弧が入るので最後の `)` の後を数える）。
fn proc_starttime(pid: u32) -> Result<u64, String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|e| format!("daemon pid {pid}: {e}"))?;
    let rest = stat
        .rsplit_once(')')
        .map(|(_, r)| r)
        .ok_or_else(|| format!("daemon pid {pid}: unreadable /proc stat"))?;
    rest.split_whitespace()
        .nth(19)
        .and_then(|x| x.parse().ok())
        .ok_or_else(|| format!("daemon pid {pid}: no starttime in /proc stat"))
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
        daemon: daemon.source.clone(),
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
    // 各検査は `env -i` で daemon の環境だけにする（この CLI process の環境が子に残って、daemon に無い変数・
    // PATH の道具を「ある」ことにしない）。
    let mut denv: Vec<String> = daemon
        .env
        .iter()
        .map(|(k, v)| sh_quote(&format!("{k}={v}")))
        .collect();
    if let Some(p) = &daemon.path {
        denv.push(sh_quote(&format!("PATH={p}")));
    }
    let denv = denv.join(" ");
    let mut script = String::from("cat >/dev/null\nd=\"$1\"\n");
    for (i, (_, cmd, _)) in checks.iter().enumerate() {
        script.push_str(&format!(
            "( /usr/bin/env -i {denv} /bin/sh -c {} ) >\"$d/{i}.out\" 2>/dev/null </dev/null; echo $? >\"$d/{i}.code\"\n",
            sh_quote(cmd)
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
