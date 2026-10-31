//! 正式試行の preflight の隔離検査（ADR 2026-10-30-ops-r2-isolation-principal D3）。
//! 主体ごとに、その主体を起動するのと同じ包み方で `probe-read` を走らせ、deny list の path が読めるか、
//! mountinfo に許可表の外の source があるかを調べる。1 件でも残れば `deny_readable` で止める（fail-closed）。
//! - A: `Sandbox::runtime_args`（A の実起動と同じ関数）
//! - B・operator: `scripts/campaign/ops-r2-sandbox.sh`（`--print-args` と同じ許可表で実際に起動する）
//! - L1: 人の uid の process 自身から `--eval-root` の下の deny path を開き、持ち主の uid と書き換えの可否を lstat で確かめる
//!   （同じ uid の chmod 000 では通さない。`l1_owner`）
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    harness_exec::Sandbox,
    manifest::{CampaignManifest, ConfigId},
    runtime_probe::{self, RuntimeProbe, unescape},
};

pub const CHECK: &str = "deny_readable";

/// `ops-r2-sandbox.sh` に渡す許可表（B の測定 daemon・外部 operator の専用 session）。
#[derive(Clone, Debug, Default)]
pub struct ScriptSandbox {
    pub state: Option<PathBuf>,
    pub release: Option<PathBuf>,
    pub account: Option<PathBuf>,
    pub ro: Vec<PathBuf>,
}

/// preflight の隔離検査の入力（CLI の `--deny-list` ほか）。
#[derive(Clone, Debug, Default)]
pub struct IsolationInputs {
    /// 拒否すべき path（1 行 1 絶対 path、`#` 行と空行は無視）。承認票と同じ内容。
    pub deny_list: Option<PathBuf>,
    /// 評価器の root（`EVAL_ROOT`）。deny list のうちこの下の path を L1 で人の uid から開く。
    pub eval_root: Option<PathBuf>,
    /// 測定用 claude の固定 copy（local）。`claude_sha256` と一緒に指定する。
    pub claude: Option<PathBuf>,
    /// 承認票の claude copy の sha256。copy の内容 hash と一致しなければ `deny_readable` で止める。
    pub claude_sha256: Option<String>,
    /// 既定 `scripts/campaign/ops-r2-sandbox.sh`。
    pub sandbox_script: Option<PathBuf>,
    pub b: ScriptSandbox,
    pub operator: ScriptSandbox,
    /// probe を走らせる `celeris-campaign`（既定はこの process の実行ファイル）。
    pub probe_program: Option<PathBuf>,
}

/// probe の包み方。
#[derive(Clone, Debug)]
pub enum Wrap {
    /// L1: この process 自身（包まない）。mount の照合はしない。
    Host,
    /// A: bwrap の引数（command は含まない）と、probe binary を置く workspace（引数の中で bind 済み）。
    Bwrap { args: Vec<String>, stage: PathBuf },
    /// B・operator: sandbox script と許可表。probe binary は `state` の下に一時的に置く。
    Script {
        script: PathBuf,
        sandbox: ScriptSandbox,
    },
}

#[derive(Clone, Debug)]
pub struct ProbeRequest {
    pub principal: &'static str,
    /// `probe-read` を持つ実行ファイル。包みの中へ stage して起動する。
    pub program: PathBuf,
    pub wrap: Wrap,
    pub deny: Vec<PathBuf>,
}

impl ProbeRequest {
    /// 全 path を拒否した結果（試験の Host が使う）。
    pub fn all_denied(&self) -> RuntimeProbe {
        RuntimeProbe {
            schema: "celeris.runtime-probe/1".into(),
            principal: self.principal.into(),
            uid: 0,
            paths: self
                .deny
                .iter()
                .map(|p| runtime_probe::PathProbe {
                    path: p.clone(),
                    readable: false,
                    listable: false,
                    errno: Some(13),
                })
                .collect(),
            mounts_outside_allow: Vec::new(),
            ok: true,
        }
    }
}

/// deny list を読む。空・相対 path・読めない file は Err。
pub fn load_deny_list(p: &Path) -> Result<Vec<PathBuf>, String> {
    let out = runtime_probe::load_deny_file(p)?;
    if out.is_empty() {
        return Err(format!("--deny-list {}: no path", p.display()));
    }
    Ok(out)
}

/// 主体ごとの probe 要求を組む。組めなければ（入力不足）`deny_readable` の詳細を Err で返す。
/// `a_real` は preflight が解決した A の実行ファイル、`ws` は probe 用の一時 workspace、`cfg` は試行 state dir の代わり。
pub fn requests(
    iso: &IsolationInputs,
    m: &CampaignManifest,
    cap_env: Option<&Path>,
    a_real: Option<&Path>,
    (ws, cfg): (&Path, &Path),
) -> Vec<Result<ProbeRequest, String>> {
    let deny = match iso.deny_list.as_deref() {
        None => {
            return vec![Err(format!(
                "{CHECK}:--deny-list <file> is required for a measured run (fail-closed)"
            ))];
        }
        Some(p) => match load_deny_list(p) {
            Ok(d) => d,
            Err(e) => return vec![Err(format!("{CHECK}:{e}"))],
        },
    };
    let program = match iso.probe_program.clone().map_or_else(
        || std::env::current_exe().map_err(|e| format!("{CHECK}:probe binary: {e}")),
        Ok,
    ) {
        Ok(p) => p,
        Err(e) => return vec![Err(e)],
    };
    let mut out = Vec::new();
    if m.is_active(ConfigId::A) {
        out.push(a_request(cap_env, a_real, (ws, cfg), &deny, &program));
    }
    let script = iso
        .sandbox_script
        .clone()
        .unwrap_or_else(|| PathBuf::from("scripts/campaign/ops-r2-sandbox.sh"));
    let mut principals = vec![("operator", &iso.operator, "--operator")];
    if m.configs
        .iter()
        .any(|c| c.id != ConfigId::A && m.is_active(c.id))
    {
        principals.insert(0, ("B", &iso.b, "--b"));
    }
    for (principal, sb, flag) in principals {
        out.push(match (&sb.state, &sb.release, &sb.account) {
            (Some(_), Some(_), Some(_)) => Ok(ProbeRequest {
                principal,
                program: program.clone(),
                wrap: Wrap::Script {
                    script: script.clone(),
                    sandbox: sb.clone(),
                },
                deny: deny.clone(),
            }),
            _ => Err(format!(
                "{CHECK}:{principal}:{flag}-state, {flag}-release and {flag}-account are required (fail-closed)"
            )),
        });
    }
    let eval: Vec<_> = match &iso.eval_root {
        Some(root) => deny
            .iter()
            .filter(|p| p.starts_with(root))
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    out.push(if eval.is_empty() {
        Err(format!(
            "{CHECK}:L1:--eval-root is required and the deny list must contain a path under it"
        ))
    } else {
        Ok(ProbeRequest {
            principal: "L1",
            program,
            wrap: Wrap::Host,
            deny: eval,
        })
    });
    out
}

fn a_request(
    cap_env: Option<&Path>,
    a_real: Option<&Path>,
    (ws, cfg): (&Path, &Path),
    deny: &[PathBuf],
    program: &Path,
) -> Result<ProbeRequest, String> {
    let toolchain = match cap_env {
        Some(p) => {
            crate::cap_probe::CapEnv::load(p)
                .map_err(|e| format!("{CHECK}:A:{e}"))?
                .a
        }
        None => Default::default(),
    };
    crate::cap_probe::prepare_state_dir(cfg).map_err(|e| format!("{CHECK}:A:{e}"))?;
    let sb = Sandbox {
        account_id: "probe".into(),
        auth_dir: cfg.to_path_buf(),
        state_root: cfg.to_path_buf(),
        toolchain,
    };
    let real = a_real.unwrap_or(Path::new("/usr/bin/true"));
    Ok(ProbeRequest {
        principal: "A",
        program: program.to_path_buf(),
        wrap: Wrap::Bwrap {
            args: sb.runtime_args(ws, cfg, real, &[]),
            stage: ws.to_path_buf(),
        },
        deny: deny.to_vec(),
    })
}

/// probe の結果を `missing` の詳細に直す（読めた path・一覧できた親・許可表の外の mount source）。
pub fn findings(r: &RuntimeProbe) -> Vec<String> {
    let p = &r.principal;
    let mut out: Vec<String> = r
        .paths
        .iter()
        .filter(|x| x.readable || x.listable)
        .map(|x| format!("{CHECK}:{p}:{}", x.path.display()))
        .collect();
    out.extend(
        r.mounts_outside_allow
            .iter()
            .map(|s| format!("{CHECK}:{p}:mount:{s}")),
    );
    if out.is_empty() && !r.ok {
        out.push(format!("{CHECK}:{p}:probe reported ok=false"));
    }
    out
}

/// L1 の持ち主の判定（`l1_owner`）。L1 以外の主体と入力不足の要求は空を返す（後者は `requests` が止める）。
/// open が拒否されても、path と EVAL_ROOT までの dir の持ち主が run の uid・0 なら、また祖先を run の uid が書き換えられるなら止める。
pub fn owner_findings(
    r: &ProbeRequest,
    iso: &IsolationInputs,
    host: &dyn crate::preflight::Host,
) -> Vec<String> {
    let (Wrap::Host, Some(root)) = (&r.wrap, iso.eval_root.as_deref()) else {
        return Vec::new();
    };
    match host.run_ids() {
        Ok(run) => crate::l1_owner::owner_findings(&run, root, &r.deny, &crate::l1_owner::lstat),
        Err(e) => vec![format!("{CHECK}:L1:{e}")],
    }
}

// ---- 実行（RealHost の既定実装） ----

/// mountinfo の (mount point, source)。octal escape は戻す。
fn mount_points(info: &str) -> Vec<(PathBuf, String)> {
    info.lines()
        .filter_map(|l| {
            let (left, right) = l.split_once(" - ")?;
            let mp = left.split_whitespace().nth(4)?;
            let src = right.split_whitespace().nth(1)?;
            Some((PathBuf::from(unescape(mp)), unescape(src)))
        })
        .collect()
}

/// bwrap 引数の bind 元（host path）が載っている host の mount source と、bwrap が作る pseudo fs の名前。
/// source 欄は device 名なので、これは「許可表の外の filesystem が見えていないか」の照合になる（D3）。
pub fn allow_sources(bwrap_args: &[String], host_mountinfo: &str) -> Vec<PathBuf> {
    let mounts = mount_points(host_mountinfo);
    let mut srcs: Vec<PathBuf> = Vec::new();
    let mut i = 0;
    while i < bwrap_args.len() {
        let host = match bwrap_args[i].as_str() {
            "--ro-bind" | "--bind" | "--ro-bind-try" | "--bind-try" | "--dev-bind" => {
                bwrap_args.get(i + 1).map(PathBuf::from)
            }
            "--dev" => Some(PathBuf::from("/dev")),
            _ => None,
        };
        if bwrap_args[i] == "--dev" {
            // `--dev` は host の /dev から device node（null・zero・tty 等）を bind する。
            srcs.extend(
                mounts
                    .iter()
                    .filter(|(mp, _)| mp.starts_with("/dev"))
                    .map(|(_, s)| PathBuf::from(s)),
            );
        }
        if let Some(host) = host {
            let host = fs::canonicalize(&host).unwrap_or(host);
            if let Some((_, s)) = mounts
                .iter()
                .filter(|(mp, _)| host.starts_with(mp))
                .max_by_key(|(mp, _)| mp.as_os_str().len())
            {
                srcs.push(PathBuf::from(s));
            }
        }
        i += 1;
    }
    for p in ["tmpfs", "proc", "devpts", "mqueue"] {
        srcs.push(PathBuf::from(p));
    }
    srcs.sort();
    srcs.dedup();
    srcs
}

/// `deny_file` は包みの中から見える path（stage dir の中）。deny list 全体を 1 file として渡し、引数に並べない。
fn probe_args(r: &ProbeRequest, deny_file: &Path, allow: &[PathBuf]) -> Vec<String> {
    let mut a = vec![
        "probe-read".into(),
        "--principal".into(),
        r.principal.into(),
    ];
    a.extend([
        "--deny-readable-file".into(),
        deny_file.to_string_lossy().into_owned(),
    ]);
    for s in allow {
        a.extend(["--allow".into(), s.to_string_lossy().into_owned()]);
    }
    a
}

/// deny list を `dir/deny.list` に 1 行 1 path で書く。
fn write_deny_file(dir: &Path, deny: &[PathBuf]) -> Result<PathBuf, String> {
    let mut text = String::new();
    for d in deny {
        text.push_str(&d.to_string_lossy());
        text.push('\n');
    }
    let f = dir.join("deny.list");
    fs::write(&f, text).map_err(|e| format!("stage deny list: {e}"))?;
    Ok(f)
}

/// 包んだ probe を走らせ、stdout の JSON を返す（exit 1 は「拒否されていない」の意味なので JSON を読む）。
fn run_json(program: &str, args: &[String]) -> Result<RuntimeProbe, String> {
    let o = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    serde_json::from_slice(&o.stdout).map_err(|e| {
        format!(
            "{program} exit {:?}: no probe JSON ({e}): {}",
            o.status.code(),
            String::from_utf8_lossy(&o.stderr).trim()
        )
    })
}

fn script_base(script: &Path, sb: &ScriptSandbox) -> Vec<String> {
    let mut a = vec![script.to_string_lossy().into_owned()];
    for (flag, v) in [
        ("--state", &sb.state),
        ("--release", &sb.release),
        ("--account", &sb.account),
    ] {
        if let Some(v) = v {
            a.extend([flag.into(), v.to_string_lossy().into_owned()]);
        }
    }
    for r in &sb.ro {
        a.extend(["--ro".into(), r.to_string_lossy().into_owned()]);
    }
    a
}

/// `ProbeRequest` を実際に走らせる（`Host::runtime_probe` の既定）。
pub fn run(r: &ProbeRequest) -> Result<RuntimeProbe, String> {
    let host_info =
        fs::read_to_string("/proc/self/mountinfo").map_err(|e| format!("mountinfo: {e}"))?;
    match &r.wrap {
        Wrap::Host => {
            // 人の uid の process 自身。host の mount は全て「在るもの」なので照合しない。
            let all = runtime_probe::mounts_outside_allow(&host_info, &[])?;
            let all: Vec<_> = all.into_iter().map(PathBuf::from).collect();
            runtime_probe::probe(r.principal, &r.deny, &all)
        }
        Wrap::Bwrap { args, stage } => {
            let staged = stage.join("celeris-campaign-probe");
            fs::copy(&r.program, &staged).map_err(|e| format!("stage probe: {e}"))?;
            let deny_file = write_deny_file(stage, &r.deny)?;
            let mut a = args.clone();
            a.push("--".into());
            a.push(staged.to_string_lossy().into_owned());
            a.extend(probe_args(r, &deny_file, &allow_sources(args, &host_info)));
            let res = run_json("bwrap", &a);
            let _ = fs::remove_file(&staged);
            let _ = fs::remove_file(&deny_file);
            res
        }
        Wrap::Script { script, sandbox } => {
            let state = sandbox.state.as_deref().ok_or("state is required")?;
            let mut print = script_base(script, sandbox);
            print.extend(["--print-args".into(), "--".into(), "true".into()]);
            let o = Command::new("sh")
                .args(&print)
                .output()
                .map_err(|e| format!("sh: {e}"))?;
            if !o.status.success() {
                return Err(format!(
                    "{} --print-args: {}",
                    script.display(),
                    String::from_utf8_lossy(&o.stderr).trim()
                ));
            }
            let printed: Vec<String> = String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_string)
                .collect();
            let stage = state.join(".preflight-probe");
            fs::create_dir_all(&stage).map_err(|e| format!("stage probe: {e}"))?;
            let staged = stage.join("celeris-campaign-probe");
            let res = fs::copy(&r.program, &staged)
                .map_err(|e| format!("stage probe: {e}"))
                .and_then(|_| write_deny_file(&stage, &r.deny))
                .and_then(|deny_file| {
                    let mut a = script_base(script, sandbox);
                    a.push("--".into());
                    a.push(staged.to_string_lossy().into_owned());
                    a.extend(probe_args(
                        r,
                        &deny_file,
                        &allow_sources(&printed, &host_info),
                    ));
                    run_json("sh", &a)
                });
            let _ = fs::remove_dir_all(&stage);
            res
        }
    }
}

/// `--claude` の copy の sha256 を承認票の `--claude-sha256` と照合する。どちらも無ければ検査しない。
/// 片方だけ、読めない、不一致はすべて `deny_readable:claude:...` の拒否理由で返す。
pub fn claude_findings(iso: &IsolationInputs) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let want = iso.claude_sha256.as_deref().map(str::to_lowercase);
    match (&iso.claude, want) {
        (None, None) => Vec::new(),
        (Some(_), None) => vec![format!(
            "{CHECK}:claude:--claude-sha256 is required with --claude"
        )],
        (None, Some(_)) => vec![format!(
            "{CHECK}:claude:--claude <local copy> is required with --claude-sha256"
        )],
        (Some(p), Some(want)) => match fs::read(p) {
            Err(e) => vec![format!("{CHECK}:claude:{}: {e}", p.display())],
            Ok(b) => {
                let got = format!("{:x}", Sha256::digest(b));
                if got == want {
                    Vec::new()
                } else {
                    vec![format!(
                        "{CHECK}:claude:{} sha256 {got} differs from the approved {want}",
                        p.display()
                    )]
                }
            }
        },
    }
}
