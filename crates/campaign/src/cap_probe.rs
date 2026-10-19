//! 必要能力の LLM なし probe（本測定移行 P1-3）。
//!
//! case の `required_capabilities` を、A は実際に使う sandbox（`Sandbox::wrap_resolved`）で包んで走らせる。
//! B/C は測定専用 daemon の通常の worker 起動経路（`POST /tasks` → dispatcher の adapter env・run 環境・権限・cwd 組立
//! → worker process）に「LLM を呼ばない能力 probe task」を通し、daemon が起こした worker process の中で確かめる
//! （`probe_worker` / `run_in_daemon` / `cap_worker_main`）。この CLI の環境は使わない。`CapEnv.worker` は worker の
//! 環境と照合する期待値で、配送ではない。不足は環境不良（`env_failure`）で、モデル失敗とは別に扱う。有料呼び出しは 0 件。
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

/// 検査先 daemon の識別。秘密を含まない（pid・identity が答えた instance/release・検査 task の id）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DaemonRef {
    pub pid: u32,
    pub instance_id: Option<String>,
    pub release: Option<String>,
    /// 検査を走らせた task（測定専用 daemon の dispatcher が worker を起こした run の持ち主）。
    pub probe_task_id: Option<String>,
}

pub const WORKER_PROBE_SCHEMA: &str = "celeris.campaign-cap-worker-probe/1";
/// 検査 task の分野の接頭辞。検査する harness `<h>` の task は分野 `cap-probe-<h>`。測定専用 daemon の config に、
/// この分野を `env_of = "<h>"` つきの fake harness（無 LLM の検査 worker `celeris-campaign cap-worker`）へ結ぶ宣言が要る
/// （cap-runbook §1）。worker は `<h>` の adapter 環境で起動する。
pub const PROBE_GENRE_PREFIX: &str = "cap-probe-";

pub fn probe_genre(harness: &str) -> String {
    format!("{PROBE_GENRE_PREFIX}{harness}")
}

/// 測定 config で、分野 `cap-probe-<h>` の検査 task が必ず無 LLM の検査 harness（kind=fake・`env_of = "<h>"`）に結ばれることを
/// 宣言から確かめる（ADR D5: 宣言が無い・別の harness に一致する config では task を投入しない。検査 JSON が実 harness に渡らない）。
/// `harness_selection.rules` を先頭から見て、この分野に当たり得る最初の規則（分野なし or 同じ分野）が検査 harness を指していなければ不合格。
pub fn declaration_problems(config: &toml::Value, harness: &str) -> Vec<String> {
    let genre = probe_genre(harness);
    let at = |m: &str| format!("{genre}: {m}");
    let rule = config
        .get("harness_selection")
        .and_then(|h| h.get("rules"))
        .and_then(|r| r.as_array())
        .and_then(|rules| {
            rules
                .iter()
                .find(|r| match r.get("genre").and_then(|g| g.as_str()) {
                    None => true,
                    Some(g) => g == genre,
                })
        });
    let Some(rule) = rule else {
        return vec![at(
            "measurement config has no harness_selection rule for this genre",
        )];
    };
    if rule.get("genre").and_then(|g| g.as_str()) != Some(genre.as_str()) {
        return vec![at(
            "the first matching harness_selection rule has no genre; the probe task could reach a real harness",
        )];
    }
    let target = rule.get("harness").and_then(|h| h.as_str()).unwrap_or("");
    let Some(h) = config.get("harnesses").and_then(|h| h.get(target)) else {
        return vec![at(&format!("rule harness {target:?} is not declared"))];
    };
    let mut out = Vec::new();
    if h.get("kind").and_then(|k| k.as_str()) != Some("fake") {
        out.push(at(&format!("harness {target:?} is not kind = \"fake\"")));
    }
    if h.get("env_of").and_then(|k| k.as_str()) != Some(harness) {
        out.push(at(&format!(
            "harness {target:?} does not declare env_of = {harness:?}"
        )));
    }
    out
}

/// 測定 config のファイルを読み、`harnesses` すべての検査宣言を確かめる（単独の `cap-probe --arm b|c` が投入の前に使う）。
pub fn declaration_problems_of_file(config: &Path, harnesses: &[String]) -> Vec<String> {
    let c = match crate::measure_env::load_toml(config) {
        Ok(c) => c,
        Err(e) => return vec![format!("measurement config unreadable: {e}")],
    };
    harnesses
        .iter()
        .flat_map(|h| declaration_problems(&c, h))
        .collect()
}

/// worker の中で走らせる検査 1 件。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerCheck {
    pub id: String,
    /// `/bin/sh -c` に渡す。必要能力の argv そのまま、または mount・socket の存在確認。
    pub cmd: String,
    pub expect_stdout: Option<String>,
}

/// 検査 task の中身（task の objective にそのまま入る）。検査の定義だけで、環境変数の値は持たない
/// （`expect_env` は `CapEnv.worker` が固定する非秘密の期待値）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerProbeSpec {
    pub schema: String,
    /// 検査する arm の worker harness（`run_harnesses` の 1 つ）。検査 task はこの harness の adapter 環境で起動する。
    pub harness: String,
    pub checks: Vec<WorkerCheck>,
    pub expect_env: Vec<(String, String)>,
}

/// worker が返す検査結果 1 件。stdout・環境変数の値は含めない（能力の有無と識別だけ）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerCheckResult {
    pub id: String,
    pub ok: bool,
    pub detail: String,
}

/// 検査 task 1 件の結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerProbeRun {
    pub task_id: String,
    pub results: Vec<WorkerCheckResult>,
}

/// 固定した mount の種別（`pin_mount` と同じく symlink は実体まで辿った先の種別）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountKind {
    File,
    Dir,
}

/// `ro_mounts` の各 mount の種別を host で調べる。解決できない mount（欠落・特殊 file）は載せない。
pub fn ro_mount_kinds(t: &Toolchain) -> Vec<(PathBuf, MountKind)> {
    t.ro_mounts
        .iter()
        .filter_map(|p| {
            let md = std::fs::metadata(std::fs::canonicalize(p).ok()?).ok()?;
            let kind = if md.is_dir() {
                MountKind::Dir
            } else if md.is_file() {
                MountKind::File
            } else {
                return None;
            };
            Some((p.clone(), kind))
        })
        .collect()
}

/// mount 1 件の worker 側検査式。file は `test -f` + 可読、directory は `test -d` + 可読 + 走査可。
pub fn ro_mount_check(path: &str, kind: MountKind) -> String {
    let q = sh_quote(path);
    match kind {
        MountKind::File => format!("test -f {q} && test -r {q}"),
        MountKind::Dir => format!("test -d {q} && test -r {q} && test -x {q}"),
    }
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// worker 子 process の中で走らせる検査。必要能力は `case.required_capabilities` そのまま、
/// それに toolchain の mount（pin 時の種別に応じた `test -f`/`test -d` と可読性）と socket（`test -S`）の確認を足す。
/// `kinds` は host で pin 時に分かった mount の種別（`ro_mount_kinds`）。載っていない mount は directory として検査する
/// （host で解決できない mount は worker でも落ちる）。
pub fn worker_spec(
    expected: &Toolchain,
    kinds: &[(PathBuf, MountKind)],
    case: &Case,
    harness: &str,
) -> WorkerProbeSpec {
    let mut checks = Vec::new();
    for m in &expected.ro_mounts {
        let p = m.to_string_lossy();
        let kind = kinds
            .iter()
            .find(|(k, _)| k == m)
            .map_or(MountKind::Dir, |(_, k)| *k);
        checks.push(WorkerCheck {
            id: format!("toolchain:{p}"),
            cmd: ro_mount_check(&p, kind),
            expect_stdout: None,
        });
    }
    for s in &expected.sockets {
        let p = s.to_string_lossy();
        checks.push(WorkerCheck {
            id: format!("socket:{p}"),
            cmd: format!("test -S {}", sh_quote(&p)),
            expect_stdout: None,
        });
    }
    for c in &case.required_capabilities {
        let argv: Vec<_> = c.argv.iter().map(|a| sh_quote(a)).collect();
        checks.push(WorkerCheck {
            id: c.id.clone(),
            cmd: argv.join(" "),
            expect_stdout: c.expect_stdout.clone(),
        });
    }
    WorkerProbeSpec {
        schema: WORKER_PROBE_SCHEMA.into(),
        harness: harness.to_string(),
        checks,
        expect_env: expected.env.clone(),
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

/// 検査 task を測定専用 daemon に走らせる関数（実体は `Host::probe_daemon_worker`。通常の task 投入経路）。
pub type RunInDaemon<'a> = dyn Fn(&WorkerProbeSpec) -> Result<WorkerProbeRun, String> + 'a;

/// B/C の能力 probe。検査は測定専用 daemon の dispatcher が通常どおり worker を起こす経路で走る。arm の worker が使う
/// harness（`harnesses` = `run_harnesses`）ごとに 1 task を投入し、その harness の adapter 環境（`env_of`）・run ごとの env・
/// cwd の組立のもとで検査する。この CLI の環境も `/proc/<pid>/environ` も使わない。`CapEnv.worker` は worker の
/// 環境と照合する期待値で、配送ではない。不足は arm 別の `env_failure`（LLM 呼出し前）。結果 id は `<harness>/<check>`。
pub fn probe_worker(
    env: &CapEnv,
    target: &DaemonRef,
    arm: ConfigId,
    case: &Case,
    harnesses: &[String],
    run: &RunInDaemon,
) -> ProbeReport {
    let mut results = Vec::new();
    if case.required_capabilities.is_empty() {
        results.push(fail(
            "required_capabilities",
            "the case fixes no required_capabilities",
        ));
    }
    if harnesses.is_empty() {
        results.push(fail("harness", "the arm fixes no worker harness"));
    }
    let mut daemon = target.clone();
    let mut task_ids = Vec::new();
    let kinds = ro_mount_kinds(&env.worker);
    for h in harnesses {
        let spec = worker_spec(&env.worker, &kinds, case, h);
        let at = |id: &str| format!("{h}/{id}");
        match run(&spec) {
            Err(e) => results.push(fail(
                &at("worker-run"),
                &format!("probe run did not complete: {e}"),
            )),
            Ok(done) => {
                task_ids.push(done.task_id);
                let want: Vec<&str> = std::iter::once("worker-env")
                    .chain(spec.checks.iter().map(|c| c.id.as_str()))
                    .collect();
                for r in &done.results {
                    if !want.contains(&r.id.as_str()) {
                        results.push(fail(&at(&r.id), "unexpected result id from the worker"));
                    } else if !r.ok {
                        results.push(fail(&at(&r.id), &r.detail));
                    } else {
                        results.push(CapResult {
                            id: at(&r.id),
                            ok: true,
                            detail: r.detail.clone(),
                            argv: Vec::new(),
                        });
                    }
                }
                for c in &spec.checks {
                    if !done.results.iter().any(|r| r.id == c.id) {
                        results.push(fail(&at(&c.id), "no result returned by the worker"));
                    }
                }
            }
        }
    }
    if !task_ids.is_empty() {
        daemon.probe_task_id = Some(task_ids.join(","));
    }
    let ok = results.iter().all(|r| r.ok);
    ProbeReport {
        schema: CAP_REPORT_SCHEMA,
        case: case.id.clone(),
        arm,
        ok,
        failure_class: (!ok).then_some(ENV_FAILURE),
        paid_calls_made: 0,
        daemon: Some(daemon),
        results,
    }
}

// ---------------------------------------------------------------- worker 側（daemon が起こす process の中）

/// 期待 env と、この worker process の環境の食い違い（PATH は期待の各要素が含まれること）。実際の値は出さない。
fn env_mismatch(expected: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (k, want) in expected {
        if k == "PATH" {
            let have: Vec<PathBuf> = std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default();
            for dir in std::env::split_paths(want) {
                if !have.contains(&dir) {
                    out.push(format!("worker PATH lacks {}", dir.display()));
                }
            }
        } else {
            match std::env::var(k) {
                Ok(v) if &v == want => {}
                Ok(_) => out.push(format!("worker env {k} differs from the expected value")),
                Err(_) => out.push(format!("worker env lacks {k}")),
            }
        }
    }
    out
}

/// 検査を worker process の環境（daemon が組んだもの）のまま走らせる。結果は能力の有無だけ（stdout・環境は返さない）。
pub fn run_spec(spec: &WorkerProbeSpec, cwd: &Path) -> Vec<WorkerCheckResult> {
    let mut out = Vec::new();
    let mismatch = env_mismatch(&spec.expect_env);
    if mismatch.is_empty() {
        out.push(WorkerCheckResult {
            id: "worker-env".into(),
            ok: true,
            detail: "ok".into(),
        });
    } else {
        for m in mismatch {
            out.push(WorkerCheckResult {
                id: "worker-env".into(),
                ok: false,
                detail: m,
            });
        }
    }
    for c in &spec.checks {
        let (ok, detail) = match Command::new("/bin/sh")
            .arg("-c")
            .arg(&c.cmd)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            Err(e) => (false, format!("sh: {e}")),
            Ok(o) if !o.status.success() => {
                (false, format!("exit {}", o.status.code().unwrap_or(-1)))
            }
            Ok(o) => match &c.expect_stdout {
                Some(x) if !String::from_utf8_lossy(&o.stdout).contains(x.as_str()) => {
                    (false, format!("stdout lacks {x:?}"))
                }
                _ => (true, "ok".into()),
            },
        };
        out.push(WorkerCheckResult {
            id: c.id.clone(),
            ok,
            detail,
        });
    }
    out
}

/// `celeris-campaign cap-worker`: 測定専用 daemon の probe 用 fake harness（`command`・`env_of`）として起動される無 LLM の検査 worker。
/// stdin の `run` メッセージの task objective から検査を読み、daemon が（検査対象 harness の adapter 環境で）組んだ環境のまま走らせ、結果を `done` の
/// summary に 1 行で返す。環境変数は読み出さず（期待値との照合だけ）、返さない。
pub fn cap_worker_main() -> Result<(), String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("stdin: {e}"))?;
    let req: task_worker::RunRequest =
        serde_json::from_str(line.trim()).map_err(|e| format!("run request: {e}"))?;
    let spec: WorkerProbeSpec = serde_json::from_str(&req.task.objective)
        .map_err(|e| format!("cap-probe objective: {e}"))?;
    if spec.schema != WORKER_PROBE_SCHEMA {
        return Err(format!("cap-probe schema must be {WORKER_PROBE_SCHEMA}"));
    }
    let results = run_spec(&spec, req.cwd());
    let summary = serde_json::to_string(&results).map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::json!({"type": "done", "summary": summary, "evidence": []})
    );
    Ok(())
}

// ---------------------------------------------------------------- 測定専用 daemon への投入（通常の task 投入経路）

/// 検査 task を task-api（`POST /tasks`）で投入し、終端まで待ち、worker が返した結果を読む。
/// 新しい API は使わない。`sleep` は poll の間隔（試験は短くする）。
pub fn run_in_daemon(
    t: &dyn crate::celeris_exec::Transport,
    spec: &WorkerProbeSpec,
    wait: Duration,
    poll: Duration,
) -> Result<WorkerProbeRun, String> {
    use serde_json::json;
    let ok = |r: &crate::celeris_exec::HttpResponse| (200..300).contains(&r.status);
    let objective = serde_json::to_string(spec).map_err(|e| e.to_string())?;
    let body = json!({
        "title": "B/C capability probe (no LLM)",
        "objective": objective,
        "genre": probe_genre(&spec.harness),
        "acceptance": [{"type": "command", "cmd": "true"}],
        "max_retries": 0,
        "max_wall_secs": wait.as_secs().max(10),
    });
    let r = t.request("POST", "/api/v1/tasks", Some(&body))?;
    if !ok(&r) {
        return Err(format!("POST /api/v1/tasks: HTTP {} {}", r.status, r.body));
    }
    let id = r.body["id"]
        .as_str()
        .ok_or("POST /api/v1/tasks: id missing")?
        .to_string();
    let start = Instant::now();
    let path = format!("/api/v1/tasks/{id}");
    loop {
        let r = t.request("GET", &path, None)?;
        if !ok(&r) {
            return Err(format!("GET {path}: HTTP {} {}", r.status, r.body));
        }
        let status = r.body["task"]["status"].as_str().unwrap_or("");
        if matches!(status, "done" | "failed" | "cancelled") {
            break;
        }
        if start.elapsed() > wait {
            let _ = t.request(
                "POST",
                &format!("/api/v1/tasks/{id}/cancel"),
                Some(&json!({})),
            );
            return Err(format!(
                "task {id} did not run within {wait:?} (status {status:?}); is the cap-probe harness declared in the measurement daemon's config?"
            ));
        }
        std::thread::sleep(poll);
    }
    let path = format!("/api/v1/tasks/{id}/events");
    let r = t.request("GET", &path, None)?;
    if !ok(&r) {
        return Err(format!("GET {path}: HTTP {} {}", r.status, r.body));
    }
    let results = r.body["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(find_results)
        .next_back()
        .ok_or_else(|| format!("task {id}: the worker returned no probe result"))?;
    Ok(WorkerProbeRun {
        task_id: id,
        results,
    })
}

/// `worker_finished` event の outcome（`done: <summary>`）から、`cap_worker_main` が書いた結果の JSON 配列を読む。
fn find_results(e: &serde_json::Value) -> Option<Vec<WorkerCheckResult>> {
    let ev = &e["event"];
    if ev["type"].as_str() != Some("worker_finished") {
        return None;
    }
    let summary = ev["outcome"].as_str()?.strip_prefix("done: ")?;
    serde_json::from_str(summary).ok()
}
