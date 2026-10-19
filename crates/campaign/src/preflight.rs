//! 本測定の前検査（preflight）。run の前に必ず通す: 一件でも不足があれば A を含め有料実行・task 作成を
//! 0 件で止め、不足一覧を返す（`run_gated`）。LLM には出ない。task-api へは GET だけ（案件・repo・アカウントの
//! 参照。書き込みも有料実行もしない）。
//! 認証は環境変数の「名前」の有無・非空だけを見る（値は読まない＝出力・ログに出せない）。
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use routing_eval::manifest::Manifest as InputManifest;

use crate::{
    cases::{CaseKind, CaseSet},
    celeris_exec::{Transport, check_template},
    curl_transport::{CurlTransport, validate_target},
    dryrun, env_check,
    executor::Executor,
    grader::GradeCase,
    harness_exec, inputs,
    isolation::{self, InitialSources},
    lock::CampaignLock,
    lock_exec,
    manifest::{CampaignManifest, ConfigId, TokenBasis},
    measure_env,
    runner::{self, RunOptions, RunReport},
    snapshot::hash_tree,
};

pub const PREFLIGHT_SCHEMA: &str = "celeris.campaign-preflight/1";

/// 人が承認した上限（2026-10-16: cache 込み全 token、全体 300 万・case 30 万・arm 130 万、並列 1）。
/// manifest がこれを超える、または token_basis が all でないと起動しない。枠の変更は新しい決定が要る。
pub const APPROVED_MAX_TOKENS: u64 = 3_000_000;
pub const APPROVED_PER_CASE_TOKENS: u64 = 300_000;
pub const APPROVED_PER_ARM_TOKENS: u64 = 1_300_000;
pub const APPROVED_PARALLEL: u32 = 1;

/// 環境の問い合わせ。値は返さない（漏れようが無い）。
pub trait Host {
    fn env_nonempty(&self, name: &str) -> bool;
    /// A の実行ファイルを解決する（絶対 path・symlink 解決済み・実行可）と、その `--version`。
    /// preflight の検査と A の実起動は、ここで返した同じ path・版を使う。
    fn resolve_program(&self, program: &Path) -> Result<ResolvedProgram, String>;
    /// task-api への GET（`path` は `/api/v1/...`）。2xx の本文を返す。token は環境変数名で渡し、値は返さない。
    fn api_get(&self, api_url: &str, token_env: &str, path: &str) -> Result<Value, String>;
    /// A の隔離（bwrap）がこの host で実際に動くか。
    fn sandbox_usable(&self) -> Result<(), String>;
    /// B/C の能力検査 task を測定専用 daemon（identity 照合済みの接続先）に通常の task 投入経路で走らせ、daemon が
    /// 起こした worker process の検査結果を返す。この CLI の環境は検査に使わない。
    fn probe_daemon_worker(
        &self,
        api_url: &str,
        token_env: &str,
        spec: &crate::cap_probe::WorkerProbeSpec,
    ) -> Result<crate::cap_probe::WorkerProbeRun, String> {
        let t = CurlTransport::new(api_url, token_env)?;
        crate::cap_probe::run_in_daemon(
            &t,
            spec,
            std::time::Duration::from_secs(300),
            std::time::Duration::from_millis(500),
        )
    }
    /// A の cap-probe の実行器。既定は実 bwrap（試験は stub を返す）。
    fn probe_runner(&self) -> Box<dyn crate::cap_probe::ProbeRunner> {
        Box::new(crate::cap_probe::ProcessProbeRunner {
            timeout: std::time::Duration::from_secs(300),
        })
    }
}

/// 解決済みの A の実行ファイル。実起動 argv の program と版の記録（`launch.json`）の元。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ResolvedProgram {
    pub path: PathBuf,
    pub version: String,
}

/// `program` を絶対 path（symlink 解決済み）にする。1 要素の名前は `path_var`（PATH の値）を引き、
/// 実行可能な file だけを採る。preflight と A の起動で同じ関数を通す（検査対象と実起動をずらさない）。
pub fn resolve_executable(
    program: &Path,
    path_var: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let exec = |p: &Path| {
        fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    let cand = if program.components().count() > 1 || program.is_absolute() {
        Some(program.to_path_buf()).filter(|p| exec(p))
    } else {
        path_var.and_then(|paths| {
            std::env::split_paths(paths)
                .map(|d| d.join(program))
                .find(|c| exec(c))
        })
    }
    .ok_or_else(|| {
        format!(
            "{} is not an executable file (or not in PATH)",
            program.display()
        )
    })?;
    fs::canonicalize(&cand).map_err(|e| format!("{}: {e}", cand.display()))
}

/// 解決（`resolve_executable`）と `--version` の取得。`RealHost` は PATH を渡す（試験は任意の PATH を渡す）。
pub fn resolve_program_in(
    program: &Path,
    path_var: Option<&std::ffi::OsStr>,
) -> Result<ResolvedProgram, String> {
    let path = resolve_executable(program, path_var)?;
    let out = Command::new(&path)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("{} --version: {e}", path.display()))?;
    let version = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if !out.status.success() || version.is_empty() {
        return Err(format!(
            "{} --version did not report a version",
            path.display()
        ));
    }
    Ok(ResolvedProgram { path, version })
}

pub struct RealHost;

impl Host for RealHost {
    fn env_nonempty(&self, name: &str) -> bool {
        std::env::var_os(name).is_some_and(|v| !v.is_empty())
    }

    fn resolve_program(&self, program: &Path) -> Result<ResolvedProgram, String> {
        resolve_program_in(program, std::env::var_os("PATH").as_deref())
    }

    fn api_get(&self, api_url: &str, token_env: &str, path: &str) -> Result<Value, String> {
        let r = CurlTransport::new(api_url, token_env)?.request("GET", path, None)?;
        if (200..300).contains(&r.status) {
            Ok(r.body)
        } else {
            Err(format!("GET {path}: HTTP {} {}", r.status, r.body))
        }
    }

    fn sandbox_usable(&self) -> Result<(), String> {
        let ok = Command::new("bwrap")
            .args([
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--unshare-pid",
                "--",
            ])
            .arg("true")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("bwrap: {e}"))?;
        if ok.status.success() {
            Ok(())
        } else {
            Err(format!(
                "bwrap cannot create the isolated namespace: {}",
                String::from_utf8_lossy(&ok.stderr).trim()
            ))
        }
    }
}

/// preflight の入力。run と同じ引数から作る（run が同じ値で検査を通ってから始まる）。
#[derive(Clone, Debug)]
pub struct Inputs {
    pub manifest_path: PathBuf,
    pub input_override: Option<PathBuf>,
    pub cases_root: PathBuf,
    pub grader_root: PathBuf,
    pub grade_cases_root: PathBuf,
    /// A arm の実行ファイル（既定 `claude`）。
    pub a_program: PathBuf,
    pub api_url: Option<String>,
    pub token_env: Option<String>,
    pub task_template: Option<PathBuf>,
    pub repo_src: Option<PathBuf>,
    pub materials_src: Option<PathBuf>,
    pub kb_src: Option<PathBuf>,
    /// 外部入力の取り込み元（`<dir>/<name>`）。選んだ case が入力を宣言していれば必須。
    pub inputs_src: Option<PathBuf>,
    /// A が使う Celeris 管理アカウント（claude_oauth の account_id）。
    pub a_account: Option<String>,
    /// 測定専用の daemon 構成と canary（P0-3）。測定では必須。
    pub measure: Option<MeasureEnv>,
    /// 測定 ID の明示（省略時は campaign hash から決まる）。
    pub measurement_id: Option<String>,
    /// 副比較（B1/C1）の系列台帳の root（launcher.env の SERIES_ROOT）。副比較は必須で、測定 ID の下に固定する。
    pub series_root: Option<PathBuf>,
    /// 必要能力 probe の環境設定（`celeris.campaign-cap-env/1`）。case が required_capabilities を持てば必須。
    pub cap_env: Option<PathBuf>,
}

/// 測定環境の汚染検査の入力。
#[derive(Clone, Debug)]
pub struct MeasureEnv {
    /// 測定専用 daemon の config.toml。
    pub config: PathBuf,
    /// 通常運用の config.toml（突き合わせ用。読むだけ）。
    pub normal_config: PathBuf,
    /// canary 識別文字列の JSON（`{"forbidden":[…],"required":[…]}`）。
    pub canaries: PathBuf,
    /// 再開時の既存 out dir。在れば中の全 file を禁止 canary で走査する。
    pub out: Option<PathBuf>,
    /// launcher が起動した測定 daemon の PID。`GET /config` の `identity.pid` と一致しなければ拒否する。
    pub daemon_pid: Option<u32>,
}

impl Inputs {
    pub fn lock_sources(&self) -> lock_exec::Sources<'_> {
        lock_exec::Sources {
            repo: self.repo_src.as_deref(),
            materials: self.materials_src.as_deref(),
            kb: self.kb_src.as_deref(),
            inputs: self.inputs_src.as_deref(),
        }
    }
}

/// 通常起動（CLI）とライブラリ経路が共有する期待 lock。宣言と grader 実体の不一致はここで Err。
pub fn expected_lock(inp: &Inputs, m: &CampaignManifest) -> Result<CampaignLock, String> {
    let lock = lock_exec::expected_lock(
        &lock_exec::LockSpec {
            cases_root: &inp.cases_root,
            grader_root: &inp.grader_root,
            grade_cases_root: &inp.grade_cases_root,
            measurement_id: inp.measurement_id.as_deref(),
        },
        m,
    )?;
    if !m.is_side() {
        return Ok(lock);
    }
    // 副比較: 系列台帳の位置を測定 ID ごとに固定して lock に封印する（hash を変えても同じ位置）。
    let mut lock = lock;
    lock.series = Some(series_pin(inp, m)?);
    lock.seal().map_err(|e| e.to_string())
}

/// 副比較の系列台帳の固定位置。測定 ID（明示）と系列 root が無ければ Err（既消費 0 にしない）。
pub fn series_pin(inp: &Inputs, m: &CampaignManifest) -> Result<crate::series::SeriesPin, String> {
    let (Some(root), Some(id)) = (inp.series_root.as_deref(), inp.measurement_id.as_deref()) else {
        return Err(format!(
            "side comparison {} needs --series-root and --measurement-id (SERIES_ROOT / MEASUREMENT_ID in launcher.env): the series ledger is pinned per measurement id",
            m.campaign_id
        ));
    };
    crate::series::SeriesPin::new(root, id)
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Missing {
    pub check: &'static str,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema: &'static str,
    pub ok: bool,
    /// 常に 0（preflight は有料実行も task 作成もしない）。
    pub paid_calls_made: u32,
    pub campaign_sha256: Option<String>,
    pub checked: Vec<&'static str>,
    pub missing: Vec<Missing>,
    /// 能力不足（check `capability`）があるとき `env_failure`。モデル失敗とは別分類。
    pub failure_class: Option<&'static str>,
    /// 検査した A の実行ファイルと版。`run` はこれと同じものを起動する。
    pub a_program: Option<ResolvedProgram>,
    /// B/C の能力 probe の検査先 daemon（case・arm ごと）。
    pub probe_targets: Vec<ProbeTarget>,
}

struct Acc {
    checked: Vec<&'static str>,
    missing: Vec<Missing>,
    a_program: Option<ResolvedProgram>,
    probe_targets: Vec<ProbeTarget>,
    /// 接続先 daemon の同一性照合の結果（preflight 中 1 回だけ GET して、env-check と B/C probe で共有する）。
    identity: Option<Result<crate::cap_probe::DaemonRef, Vec<String>>>,
}

/// B/C の能力 probe を実際に走らせた検査先 daemon の識別（case・arm ごと。秘密を含まない）。
#[derive(Clone, Debug, Serialize)]
pub struct ProbeTarget {
    pub case: String,
    pub arm: ConfigId,
    pub daemon: crate::cap_probe::DaemonRef,
}

impl Acc {
    fn miss(&mut self, check: &'static str, detail: impl Into<String>) {
        self.missing.push(Missing {
            check,
            detail: detail.into(),
        });
    }

    fn res<T>(&mut self, check: &'static str, r: Result<T, String>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.miss(check, e);
                None
            }
        }
    }
}

fn sha256_file(p: &Path) -> Result<String, String> {
    let b = fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(format!("{:x}", Sha256::digest(b)))
}

fn rel_ok(p: &str) -> bool {
    use std::path::Component;
    !p.is_empty()
        && Path::new(p)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// `<…>` を含む文字列が 1 つでもあれば、その JSON path を返す。
fn find_placeholder(v: &Value, at: &str) -> Option<String> {
    match v {
        Value::String(s) if s.contains('<') && s.contains('>') => Some(at.to_string()),
        Value::Array(a) => a
            .iter()
            .enumerate()
            .find_map(|(i, x)| find_placeholder(x, &format!("{at}[{i}]"))),
        Value::Object(m) => m
            .iter()
            .find_map(|(k, x)| find_placeholder(x, &format!("{at}.{k}"))),
        _ => None,
    }
}

/// 全件検査。`missing` が空のときだけ `ok`。
pub fn check(inp: &Inputs, host: &dyn Host) -> Report {
    let mut a = Acc {
        checked: Vec::new(),
        missing: Vec::new(),
        a_program: None,
        probe_targets: Vec::new(),
        identity: None,
    };
    let mut sha = None;
    if let Some(m) = a.res("manifest", CampaignManifest::load(&inp.manifest_path)) {
        sha = m.compute_sha256().ok();
        run_checks(&mut a, &m, inp, host);
    }
    Report {
        schema: PREFLIGHT_SCHEMA,
        ok: a.missing.is_empty(),
        paid_calls_made: 0,
        campaign_sha256: sha,
        checked: a.checked,
        failure_class: a
            .missing
            .iter()
            .any(|x| x.check == "capability")
            .then_some(crate::cap_probe::ENV_FAILURE),
        missing: a.missing,
        a_program: a.a_program,
        probe_targets: a.probe_targets,
    }
}

fn run_checks(a: &mut Acc, m: &CampaignManifest, inp: &Inputs, host: &dyn Host) {
    // 1. manifest・版（placeholder / alias / routing）の解決。split に関わらず本測定の基準で見る。
    a.checked.push("manifest");
    let _ = a.res("manifest", m.validate());
    a.checked.push("versions");
    let _ = a.res("versions", m.check_resolved());

    // 2. 入力 manifest（hash・split・自己完結）。
    a.checked.push("input-manifest");
    let input: Option<InputManifest> = a
        .res(
            "input-manifest",
            m.load_input(&inp.manifest_path, inp.input_override.as_deref()),
        )
        .and_then(|i| {
            a.res("input-manifest", m.check_against_input(&i))
                .map(|()| i)
        });

    // 3. 全 case 入力。
    a.checked.push("case-inputs");
    let set = a.res(
        "case-inputs",
        CaseSet::load(&inp.cases_root, &inp.grader_root),
    );
    if let Some(set) = &set {
        for id in &m.problems {
            match set.get(id) {
                None => a.miss(
                    "case-inputs",
                    format!("{id}: no case.json under cases root"),
                ),
                Some(c) => {
                    if c.case.kind == CaseKind::Control {
                        a.miss(
                            "case-inputs",
                            format!("{id}: control case in a measurement"),
                        );
                    }
                    if c.case.split != m.split {
                        a.miss(
                            "case-inputs",
                            format!(
                                "{id}: case split {:?} != campaign {:?}",
                                c.case.split, m.split
                            ),
                        );
                    }
                }
            }
        }
        // 4. grader 対応（grade-case/1 全件・固定 hash・公開/隠しテストの実体）。
        a.checked.push("graders");
        for id in &m.problems {
            if let Some(c) = set.get(id) {
                check_grader(a, &c.case, &inp.grade_cases_root);
            }
        }
        // 7. 予算・時間（case 側）。
        a.checked.push("case-time");
        let _ = a.res("budget", set.check_within_budget(&m.problems, &m.budget));
        for id in &m.problems {
            if let Some(e) = set.get(id).and_then(|c| c.case.experiment.as_ref())
                && e.walltime_secs > m.termination.trial_timeout_secs
            {
                a.miss(
                    "budget",
                    format!(
                        "{id}: experiment walltime {}s exceeds termination.trial_timeout_secs {}s",
                        e.walltime_secs, m.termination.trial_timeout_secs
                    ),
                );
            }
        }
    }

    // 外部入力・初期 repo（case が宣言したものが全部届くか）。
    a.checked.push("inputs");
    if let Some(set) = &set {
        check_inputs(a, m, set, inp);
    }

    // 能力 probe は最初の LLM 呼出しより前（preflight は常に先）。
    if let Some(set) = &set {
        check_capabilities(a, m, set, inp, host);
    }

    // 5/6. 候補構成・認証/環境。
    check_candidates(a, m, inp, host);
    check_snapshot_src(a, m, inp);
    check_measure_env(a, m, inp, host);
    if let Some(out) = inp.measure.as_ref().and_then(|me| me.out.as_deref()) {
        check_trial_state(a, m, inp, out);
    }

    // 初期 tree・submodule・外部入力・資料・KB・grader/test を campaign lock の期待値に照合する。
    a.checked.push("lock");
    if let Some(l) = a.res("lock", expected_lock(inp, m)) {
        let _ = a.res(
            "lock",
            lock_exec::verify_sources(&l, inp.lock_sources(), true),
        );
    }

    // 7. 予算・時間（manifest 側）。
    a.checked.push("budget");
    check_budget(a, m);
    if let Some(i) = &input
        && let Ok(v) = dryrun::estimate(m, i)
        && v["acceptance"]["within_budget"] != true
    {
        a.miss(
            "budget",
            format!("dry-run estimate: {}", v["acceptance"]["reasons"]),
        );
    }

    // 測定専用 daemon に検査 task を作る B/C の能力 probe は、他の検査が全て通った後（最後）に走らせる。
    if a.missing.is_empty()
        && let Some(set) = &set
    {
        check_bc_capabilities(a, m, set, inp, host);
    }
}

/// P0-3: 測定専用構成・A/B/C の試行用 KB の同一性・canary。
fn check_measure_env(a: &mut Acc, m: &CampaignManifest, inp: &Inputs, host: &dyn Host) {
    a.checked.push("measure-env");
    let Some(me) = &inp.measure else {
        a.miss(
            "measure-env",
            "--measure-config, --normal-config and --canaries are required (measurement daemon isolation)",
        );
        return;
    };
    for e in measure_env::check_config(&me.config, &me.normal_config, inp.kb_src.as_deref()) {
        a.miss("measure-env", e);
    }
    // 接続先 daemon が測定 config で起動したその daemon か（api-url は config から導いた値と一致必須）。
    a.checked.push("env-check");
    // 欠落・不正形式は auth-env が報告済み（verified_identity は None）。
    if let Some(Err(es)) = verified_identity(a, m, inp, host) {
        for e in es {
            a.miss("env-check", e);
        }
    }
    let Some(can) = a.res("measure-env", measure_env::Canaries::load(&me.canaries)) else {
        return;
    };
    // 試行 workspace の木と同じ作り方（A/B/C）で作って検査する。repo 無しの case は空の repo にする。
    if let (Some(mat), Some(kb)) = (&inp.materials_src, &inp.kb_src) {
        let r = tempfile::tempdir()
            .map_err(|e| e.to_string())
            .and_then(|t| {
                let repo = match &inp.repo_src {
                    Some(r) => r.clone(),
                    None => {
                        let e = t.path().join("empty-repo");
                        fs::create_dir_all(&e).map_err(|e| e.to_string())?;
                        e
                    }
                };
                let src = InitialSources {
                    repo,
                    materials: mat.clone(),
                    kb: kb.clone(),
                    budget: m.budget.clone(),
                    withheld_names: Vec::new(),
                };
                isolation::check_trial_context(
                    &src,
                    &t.path().join("trials"),
                    &can.forbidden,
                    &can.required,
                )
            });
        match r {
            Ok(v) => v.into_iter().for_each(|e| a.miss("measure-env", e)),
            Err(e) => a.miss("measure-env", e),
        }
    }
    // 再開: 既存 out dir の試行木に禁止 canary が無い。
    if let Some(out) = me.out.as_ref().filter(|o| o.is_dir()) {
        match isolation::scan_canaries(out, &can.forbidden, &[]) {
            Ok(r) => {
                for (f, c) in r.present_forbidden {
                    a.miss(
                        "measure-env",
                        format!("existing out dir: forbidden canary {c:?} in {f}"),
                    );
                }
            }
            Err(e) => a.miss("measure-env", e),
        }
    }
}

fn check_budget(a: &mut Acc, m: &CampaignManifest) {
    let b = &m.budget;
    if b.token_basis != TokenBasis::All {
        a.miss(
            "budget",
            "budget.token_basis must be `all` (approved: cache included)",
        );
    }
    for (n, v, max) in [
        ("max_tokens", b.max_tokens, APPROVED_MAX_TOKENS),
        (
            "per_case.max_tokens",
            b.per_case.max_tokens,
            APPROVED_PER_CASE_TOKENS,
        ),
        (
            "per_arm.max_tokens",
            b.per_arm.max_tokens,
            APPROVED_PER_ARM_TOKENS,
        ),
    ] {
        if v > max {
            a.miss(
                "budget",
                format!("budget.{n} {v} exceeds the approved {max}"),
            );
        }
    }
    if b.max_parallel != APPROVED_PARALLEL {
        a.miss(
            "budget",
            format!(
                "budget.max_parallel must be {APPROVED_PARALLEL}, got {}",
                b.max_parallel
            ),
        );
    }
    if m.termination.trial_timeout_secs > b.per_case.max_wall_secs {
        a.miss(
            "budget",
            format!(
                "termination.trial_timeout_secs {} exceeds budget.per_case.max_wall_secs {}",
                m.termination.trial_timeout_secs, b.per_case.max_wall_secs
            ),
        );
    }
}

fn check_candidates(a: &mut Acc, m: &CampaignManifest, inp: &Inputs, host: &dyn Host) {
    a.checked.push("candidates");
    for id in [ConfigId::A, ConfigId::B, ConfigId::C] {
        if !m.configs.iter().any(|c| c.id == id) {
            a.miss(
                "candidates",
                format!("config {id:?} is not in the manifest"),
            );
        }
    }
    // A: 既存ハーネスの実行ファイル。
    a.checked.push("auth-env");
    match host.resolve_program(&inp.a_program) {
        Ok(p) => a.a_program = Some(p),
        Err(e) => a.miss(
            "auth-env",
            format!("A harness program {}: {e}", inp.a_program.display()),
        ),
    }
    // B/C: task-api の origin と token の環境変数（名前だけ）。
    match (&inp.api_url, &inp.token_env) {
        (Some(url), Some(tok)) => {
            if url.contains('@') {
                a.miss("auth-env", "--api-url must not embed credentials");
            } else if let Err(e) = validate_target(url, tok) {
                a.miss("auth-env", e);
            }
            if !host.env_nonempty(tok) {
                a.miss(
                    "auth-env",
                    format!("environment variable {tok} is not set or empty"),
                );
            }
        }
        _ => a.miss("auth-env", "B/C need --api-url and --token-env <ENV_NAME>"),
    }
    if let Err(e) = host.sandbox_usable() {
        a.miss("a-arm", format!("A isolation (bwrap) is not usable: {e}"));
    }
    let mut tv: Option<Value> = None;
    match &inp.task_template {
        None => a.miss(
            "auth-env",
            "--task-template is required (project_id, acceptance)",
        ),
        Some(p) => {
            let v = fs::read(p)
                .map_err(|e| format!("{}: {e}", p.display()))
                .and_then(|b| {
                    serde_json::from_slice::<Value>(&b).map_err(|e| format!("{}: {e}", p.display()))
                });
            if let Some(v) = a.res("auth-env", v) {
                match v.as_object() {
                    None => a.miss("auth-env", "--task-template must be a JSON object"),
                    Some(t) => {
                        let _ = a.res("auth-env", check_template(t));
                        if !t.get("project_id").is_some_and(Value::is_string) {
                            a.miss("auth-env", "--task-template needs a string project_id");
                        }
                    }
                }
                if let Some(at) = find_placeholder(&v, "task-template") {
                    a.miss("versions", format!("{at} is an unresolved <placeholder>"));
                }
                tv = Some(v);
            }
        }
    }
    check_api_dependent(a, inp, host, tv.as_ref());
}

/// 選んだ問題の必要能力を LLM なしに確かめる（A は A の sandbox 組立。B/C は `check_bc_capabilities` が測定専用 daemon の worker で）。
fn check_capabilities(
    a: &mut Acc,
    m: &CampaignManifest,
    set: &CaseSet,
    inp: &Inputs,
    host: &dyn Host,
) {
    let cases: Vec<_> = m
        .problems
        .iter()
        .filter_map(|id| set.get(id))
        .filter(|c| !c.case.required_capabilities.is_empty())
        .collect();
    if cases.is_empty() {
        return;
    }
    a.checked.push("capability");
    let Some(path) = &inp.cap_env else {
        a.miss(
            "capability",
            "--cap-env is required: the selected cases fix required_capabilities",
        );
        return;
    };
    let Some(env) = a.res("capability", crate::cap_probe::CapEnv::load(path)) else {
        return;
    };
    let tmp = match tempfile::tempdir() {
        Ok(t) => t,
        Err(e) => return a.miss("capability", e.to_string()),
    };
    let (ws, cfg) = (tmp.path().join("ws"), tmp.path().join("cfg"));
    if let Err(e) = fs::create_dir_all(&ws) {
        return a.miss("capability", e.to_string());
    }
    if let Err(e) = crate::cap_probe::prepare_state_dir(&cfg) {
        return a.miss("capability", e);
    }
    let runner = host.probe_runner();
    for c in cases {
        let sb = crate::cap_probe::sandbox_for(
            &env,
            ConfigId::A,
            inp.a_account.as_deref().unwrap_or("probe"),
            &cfg,
        );
        if !m.configs.iter().any(|x| x.id == ConfigId::A) {
            continue;
        }
        let rep = crate::cap_probe::probe(&sb, ConfigId::A, &ws, &c.case, runner.as_ref());
        for r in rep.results.iter().filter(|r| !r.ok) {
            a.miss(
                "capability",
                format!(
                    "{} arm {:?}: {}: {}",
                    c.case.id,
                    ConfigId::A,
                    r.id,
                    r.detail
                ),
            );
        }
    }
}

/// B/C の能力 probe。検査先は identity 照合に通した測定専用 daemon だけで、検査 task はその daemon の通常の worker 起動経路
/// （dispatcher の env 組立・権限・cwd）で走る。測定専用 daemon に実際に（無 LLM の）task を作るので、他の検査の後（最後）に走らせる。通らなければ B/C は env_failure。
fn check_bc_capabilities(
    a: &mut Acc,
    m: &CampaignManifest,
    set: &CaseSet,
    inp: &Inputs,
    host: &dyn Host,
) {
    let cases: Vec<_> = m
        .problems
        .iter()
        .filter_map(|id| set.get(id))
        .filter(|c| !c.case.required_capabilities.is_empty())
        .collect();
    if cases.is_empty() || !m.configs.iter().any(|x| x.id != ConfigId::A) {
        return;
    }
    // 能力不足・cap env の不備は `check_capabilities` が報告済み（env を読めなければここでは何もしない）。
    let Some(env) = inp
        .cap_env
        .as_deref()
        .and_then(|p| crate::cap_probe::CapEnv::load(p).ok())
    else {
        return;
    };
    let Some(target) = bc_probe_target(a, m, inp, host) else {
        return;
    };
    let (Some(url), Some(tok)) = (inp.api_url.as_deref(), inp.token_env.as_deref()) else {
        return;
    };
    // 投入の前に、検査分野が無 LLM の検査 harness に結ばれていることを測定 config の宣言から確かめる。
    let Some(measure) = inp.measure.as_ref() else {
        return;
    };
    let config = match measure_env::load_toml(&measure.config) {
        Ok(c) => c,
        Err(e) => {
            a.miss(
                "capability",
                format!("B/C probe: measurement config unreadable: {e}"),
            );
            return;
        }
    };
    let mut declared = true;
    for h in m
        .configs
        .iter()
        .filter(|x| x.id != ConfigId::A)
        .flat_map(|x| x.worker_harnesses())
        .collect::<std::collections::BTreeSet<_>>()
    {
        for e in crate::cap_probe::declaration_problems(&config, &h) {
            declared = false;
            a.miss("capability", format!("B/C probe not submitted: {e}"));
        }
    }
    if !declared {
        return;
    }
    for c in cases {
        for cfg_id in m.configs.iter().map(|x| x.id).filter(|x| *x != ConfigId::A) {
            let harnesses = m
                .configs
                .iter()
                .find(|x| x.id == cfg_id)
                .map(|x| x.worker_harnesses())
                .unwrap_or_default();
            let rep = crate::cap_probe::probe_worker(
                &env,
                &target,
                cfg_id,
                &c.case,
                &harnesses,
                &|spec| host.probe_daemon_worker(url, tok, spec),
            );
            if let Some(d) = &rep.daemon {
                a.probe_targets.push(ProbeTarget {
                    case: c.case.id.clone(),
                    arm: cfg_id,
                    daemon: d.clone(),
                });
            }
            for r in rep.results.iter().filter(|r| !r.ok) {
                a.miss(
                    "capability",
                    format!("{} arm {:?}: {}: {}", c.case.id, cfg_id, r.id, r.detail),
                );
            }
        }
    }
}

fn bc_probe_target(
    a: &mut Acc,
    m: &CampaignManifest,
    inp: &Inputs,
    host: &dyn Host,
) -> Option<crate::cap_probe::DaemonRef> {
    if inp.measure.is_none() || inp.api_url.is_none() || inp.token_env.is_none() {
        a.miss(
            "capability",
            "B/C worker probe needs the measurement daemon (--measure-config, --daemon-pid, --api-url, --token-env); the CLI's own environment is not a stand-in",
        );
        return None;
    }
    // 欠落・不正形式は auth-env が報告済み（None）。
    match verified_identity(a, m, inp, host)? {
        Ok(d) => Some(d),
        Err(es) => {
            for e in es {
                a.miss(
                    "capability",
                    format!("B/C probe target is not the measurement daemon: {e}"),
                );
            }
            None
        }
    }
}

/// 接続先 daemon の同一性照合（1 回だけ実行して保持する）。測定 config / api 指定が無ければ None。
fn verified_identity(
    a: &mut Acc,
    m: &CampaignManifest,
    inp: &Inputs,
    host: &dyn Host,
) -> Option<Result<crate::cap_probe::DaemonRef, Vec<String>>> {
    let (Some(me), Some(url), Some(tok)) = (&inp.measure, &inp.api_url, &inp.token_env) else {
        return None;
    };
    if url.contains('@') || validate_target(url, tok).is_err() || !host.env_nonempty(tok) {
        return None;
    }
    if a.identity.is_none() {
        a.identity = Some(env_check::verify(me, m, url, tok, host));
    }
    a.identity.clone()
}

/// 選んだ問題の外部入力が `--inputs-src` に揃い sha256 が宣言と一致するか、初期 repo の取り込み元があるか。
fn check_inputs(a: &mut Acc, m: &CampaignManifest, set: &CaseSet, inp: &Inputs) {
    let declared = inputs::for_problems(set, &m.problems);
    if !declared.is_empty() {
        match &inp.inputs_src {
            None => a.miss(
                "inputs",
                format!(
                    "--inputs-src is required: {} declare external inputs ({})",
                    declared.keys().cloned().collect::<Vec<_>>().join(", "),
                    declared
                        .values()
                        .flatten()
                        .map(|x| x.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
            Some(src) => {
                for e in inputs::check_src(src, &declared) {
                    a.miss("inputs", e);
                }
            }
        }
    }
    for id in &m.problems {
        if set.get(id).is_some_and(|c| c.case.initial.is_some()) && inp.repo_src.is_none() {
            a.miss(
                "inputs",
                format!("{id}: the case has an initial repo; --repo-src is required"),
            );
        }
    }
}

/// `POST /tasks` が 422 になる組合せと、自動取り込みされる案件を、案件（`ProjectDetail`）・repo 一覧の JSON から判定する。
/// executor は常に local の `workspace` を渡すので、選ばれる repo（template の `repos`、無ければ primary）に
/// リモートが 1 つでもあれば task-ops `create` が拒否する。
pub fn check_project_rules(
    template: &Map<String, Value>,
    detail: &Value,
    repos: &Value,
) -> Vec<String> {
    let mut v = Vec::new();
    // 実 `GET /projects/{id}` は `ProjectDetail`（`{"project":{"auto_integrate":..},"repos":[..],..}`）。
    // 欠落・bool でない・project が無い場合も on 扱い（安全側）。
    if detail["project"]["auto_integrate"]
        .as_bool()
        .unwrap_or(true)
    {
        v.push(
            "the project has auto_integrate on (or unknown); trial tasks could be merged into the repo's main. Use a dedicated campaign project with auto_integrate=false"
                .into(),
        );
    }
    let items: Vec<&Value> = repos["items"]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let names: Vec<&str> = template
        .get("repos")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let selected: Vec<&Value> = if names.is_empty() {
        items
            .iter()
            .copied()
            .filter(|r| r["is_primary"].as_bool().unwrap_or(false))
            .collect()
    } else {
        let mut sel = Vec::new();
        for n in &names {
            match items.iter().find(|r| r["name"] == *n) {
                Some(r) => sel.push(*r),
                None => v.push(format!(
                    "template repos: {n:?} is not a repo of the project"
                )),
            }
        }
        sel
    };
    for r in selected {
        if r["location"]["kind"] == "remote" {
            v.push(format!(
                "a local workspace cannot be combined with the remote repo {} (POST /tasks would answer 422); use a project whose selected repo is local, or no repo",
                r["name"]
            ));
        }
    }
    v
}

/// `GET /api/v1/llm/accounts` から A のアカウント（claude_oauth）の config dir を引く。
pub fn find_a_account(accounts: &Value, id: &str) -> Result<PathBuf, String> {
    let item = accounts["items"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|x| x["source_id"] == "claude_oauth" && x["account_id"] == id)
        .ok_or_else(|| {
            format!("--a-account {id}: no such claude_oauth account in GET /llm/accounts")
        })?;
    if item["logged_in"] != true {
        return Err(format!("--a-account {id}: the account is not logged in"));
    }
    let dir = item["dir"]
        .as_str()
        .filter(|d| !d.is_empty())
        .ok_or_else(|| format!("--a-account {id}: the account has no directory"))?;
    let p = PathBuf::from(dir);
    if !p.is_absolute() || !p.is_dir() {
        return Err(format!(
            "--a-account {id}: {dir} is not an existing absolute directory"
        ));
    }
    Ok(p)
}

/// 試行 state（A の `CLAUDE_CONFIG_DIR`）の検査。試行ごとに「開始済みか」を台帳（`<out>/ledger.jsonl`）で決める。
/// - 開始済み（台帳に行がある）→ 再開検査: state dir の marker が当該 campaign/manifest/arm/case/rep/account と
///   保存 lock に一致すること。marker 欠落・不一致・state dir 不在は拒否。
/// - 未開始 → 新規検査: state dir は不在か空（認証 mount 先の空 file と自分の marker だけは可）。
///   CLAUDE.md・projects・他試行の marker があれば混入として拒否。
///
/// 何も消さず、memory も無効にしない。アカウント dir は認証材料 1 件しか試行に渡らないので、その残渣は試行へ届かない。
fn check_trial_state(a: &mut Acc, m: &CampaignManifest, inp: &Inputs, out: &Path) {
    a.checked.push("trial-state");
    let Some(account) = inp.a_account.as_deref() else {
        return; // a-arm で報告済み
    };
    let Ok(sha) = m.compute_sha256() else {
        return;
    };
    // 保存 lock が別 campaign のものなら、この out の state は再開できない。
    let lock_path = out.join(crate::lock_exec::LOCK_FILE);
    if lock_path.is_file() {
        match fs::read(&lock_path)
            .map_err(|e| e.to_string())
            .and_then(|b| {
                serde_json::from_slice::<crate::lock::CampaignLock>(&b).map_err(|e| e.to_string())
            }) {
            Ok(l) if l.campaign_sha256 != sha => {
                a.miss(
                    "trial-state",
                    format!(
                        "{}: saved lock belongs to campaign {}, not {sha}",
                        lock_path.display(),
                        l.campaign_sha256
                    ),
                );
                return;
            }
            Ok(_) => {}
            Err(e) => {
                a.miss(
                    "trial-state",
                    format!("{}: unreadable lock: {e}", lock_path.display()),
                );
                return;
            }
        }
    }
    let started = match started_a_trials(out) {
        Ok(v) => v,
        Err(e) => {
            a.miss("trial-state", e);
            return;
        }
    };
    let root = out.join("trial-state");
    let arm = format!("{:?}", ConfigId::A);
    let want = |case: &str, rep: u32| harness_exec::TrialMarker {
        campaign_id: m.campaign_id.clone(),
        manifest_sha256: sha.clone(),
        arm: arm.clone(),
        case: case.to_string(),
        rep,
        account_id: account.to_string(),
    };
    // 再開: 開始済みの試行は保存 state が自分のものであること。
    for (case, rep) in &started {
        let dir = harness_exec::trial_state_dir(&root, &m.campaign_id, &arm, case, *rep);
        let label = format!("{case}/rep{rep}");
        match read_marker(&dir) {
            Ok(Some(have)) if have == want(case, *rep) => {}
            Ok(Some(have)) => a.miss(
                "trial-state",
                format!(
                    "{label}: resume refused, identity marker in {} belongs to another trial/lock ({}/{}/{}/rep{})",
                    dir.display(),
                    have.campaign_id,
                    have.arm,
                    have.case,
                    have.rep
                ),
            ),
            // 台帳に行があっても state が無い（投入直後の crash）か空なら、引き継ぐ状態は無い。
            Ok(None) if fresh_state_residue(&root, &dir).is_empty() => {}
            Ok(None) => a.miss(
                "trial-state",
                format!(
                    "{label}: resume refused, started trial has state without an identity marker in {}",
                    dir.display()
                ),
            ),
            Err(e) => a.miss("trial-state", format!("{label}: {e}")),
        }
    }
    // 新規: 開始していない試行の state dir は空でなければならない。
    let arm_dir = harness_exec::trial_state_dir(&root, &m.campaign_id, &arm, "x", 0)
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let Ok(cases) = fs::read_dir(arm_dir) else {
        return;
    };
    for case_dir in cases.filter_map(Result::ok) {
        let Ok(reps) = fs::read_dir(case_dir.path()) else {
            continue;
        };
        for rep_dir in reps.filter_map(Result::ok) {
            let dir = rep_dir.path();
            let is_started = started.iter().any(|(c, r)| {
                harness_exec::trial_state_dir(&root, &m.campaign_id, &arm, c, *r) == dir
            });
            if is_started {
                continue;
            }
            for e in fresh_state_residue(&root, &dir) {
                a.miss("trial-state", e);
            }
        }
    }
}

fn read_marker(dir: &Path) -> Result<Option<harness_exec::TrialMarker>, String> {
    let p = dir.join(harness_exec::TRIAL_MARKER);
    match fs::read(&p) {
        Ok(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|e| format!("{}: unreadable identity marker: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

/// 台帳（読むだけ。crash で切れた末尾行は無視）で開始済みの A 試行 `(case, rep)`。
fn started_a_trials(out: &Path) -> Result<Vec<(String, u32)>, String> {
    let p = out.join(crate::ledger::LEDGER_FILE);
    let text = match fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", p.display())),
    };
    let keep = text.rfind('\n').map_or(0, |i| i + 1);
    let mut v = Vec::new();
    for (i, l) in text[..keep].lines().enumerate() {
        let r: crate::ledger::LedgerRow =
            serde_json::from_str(l).map_err(|e| format!("{} line {}: {e}", p.display(), i + 1))?;
        if r.key.arm == ConfigId::A {
            let k = (r.key.problem_id, r.key.rep);
            if !v.contains(&k) {
                v.push(k);
            }
        }
    }
    Ok(v)
}

/// 未開始の試行の state dir の残渣（認証 mount 先の空 file と自分の marker は可）。
fn fresh_state_residue(root: &Path, dir: &Path) -> Vec<String> {
    let mut v = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return v;
    };
    for e in rd.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        match name.as_str() {
            ".credentials.json" => {}
            n if n == harness_exec::TRIAL_MARKER => {
                // 自分の dir の marker は path の識別と一致するときだけ可。
                let ok = matches!(read_marker(dir), Ok(Some(h))
                    if harness_exec::trial_state_dir(root, &h.campaign_id, &h.arm, &h.case, h.rep) == dir);
                if !ok {
                    v.push(format!(
                        "{}: new trial state holds another trial's identity marker",
                        path.display()
                    ));
                }
            }
            n => v.push(format!(
                "{}: new trial state is not empty ({n}); transcripts/memory/CLAUDE.md of another run would leak in",
                dir.display()
            )),
        }
    }
    v
}

fn check_api_dependent(a: &mut Acc, inp: &Inputs, host: &dyn Host, template: Option<&Value>) {
    let (Some(url), Some(tok)) = (&inp.api_url, &inp.token_env) else {
        return;
    };
    if url.contains('@') || validate_target(url, tok).is_err() || !host.env_nonempty(tok) {
        return; // 既に auth-env で報告済み
    }
    // B/C: 案件と repo。
    if let Some(t) = template.and_then(Value::as_object)
        && let Some(pid) = t.get("project_id").and_then(Value::as_str)
    {
        let project = host.api_get(url, tok, &format!("/api/v1/projects/{pid}"));
        let repos = host.api_get(url, tok, &format!("/api/v1/projects/{pid}/repos"));
        match (project, repos) {
            (Ok(p), Ok(r)) => {
                for e in check_project_rules(t, &p, &r) {
                    a.miss("task-template", e);
                }
            }
            (p, r) => {
                for e in [p.err(), r.err()].into_iter().flatten() {
                    a.miss("task-template", format!("project {pid}: {e}"));
                }
            }
        }
    }
    // A: Celeris 管理アカウント。
    match &inp.a_account {
        None => a.miss(
            "a-arm",
            "--a-account <account_id> is required: A runs under a Celeris-managed claude_oauth account",
        ),
        Some(id) => match host.api_get(url, tok, "/api/v1/llm/accounts") {
            Ok(v) => {
                if let Err(e) = find_a_account(&v, id) {
                    a.miss("a-arm", e);
                }
            }
            Err(e) => a.miss("a-arm", format!("GET /llm/accounts: {e}")),
        },
    }
}

/// 配る入力が揃い、材料の hash が manifest の固定値と一致するか（run が拒否する前に見る）。
fn check_snapshot_src(a: &mut Acc, m: &CampaignManifest, inp: &Inputs) {
    a.checked.push("snapshot-src");
    match &inp.materials_src {
        None => a.miss(
            "case-inputs",
            "--materials-src is required for a measured run",
        ),
        Some(p) => {
            if let Some(h) = a.res("case-inputs", hash_tree(p))
                && h != m.snapshot.materials_sha256
            {
                a.miss(
                    "case-inputs",
                    format!(
                        "materials hash {h} does not match the manifest pin {}",
                        m.snapshot.materials_sha256
                    ),
                );
            }
        }
    }
    for (name, p, required) in [
        ("--kb-src", &inp.kb_src, true),
        ("--repo-src", &inp.repo_src, false),
    ] {
        match p {
            None if required => a.miss(
                "case-inputs",
                format!("{name} is required for a measured run"),
            ),
            Some(p) if !p.is_dir() => a.miss(
                "case-inputs",
                format!("{name} {} is not a directory", p.display()),
            ),
            _ => {}
        }
    }
}

fn check_grader(a: &mut Acc, case: &crate::cases::Case, root: &Path) {
    let id = &case.id;
    let Some(r) = &case.grade_case_ref else {
        a.miss(
            "graders",
            format!("{id}: no grade_case_ref (celeris.grade-case/1)"),
        );
        return;
    };
    if !rel_ok(&r.path) {
        a.miss("graders", format!("{id}: grade_case_ref.path is unsafe"));
        return;
    }
    let p = root.join(&r.path);
    match sha256_file(&p) {
        Err(e) => {
            a.miss("graders", format!("{id}: {e}"));
            return;
        }
        Ok(h) if h != r.sha256 => {
            a.miss(
                "graders",
                format!(
                    "{id}: grade-case hash {h} differs from the pinned {}",
                    r.sha256
                ),
            );
            return;
        }
        Ok(_) => {}
    }
    let g = match GradeCase::load(&p) {
        Ok(g) => g,
        Err(e) => {
            a.miss("graders", format!("{id}: {e}"));
            return;
        }
    };
    if g.id != *id {
        a.miss("graders", format!("{id}: grade-case id is {}", g.id));
    }
    let has = |s: &str| case.stages.iter().any(|x| x == s);
    if has("implementation") && g.implementation.is_none() {
        a.miss(
            "graders",
            format!("{id}: implementation stage without an implementation check"),
        );
    }
    if has("experiment") && g.experiment.is_none() {
        a.miss(
            "graders",
            format!("{id}: experiment stage without an experiment check"),
        );
    }
    if g.implementation.is_none()
        && g.recompute.is_none()
        && g.experiment.is_none()
        && g.review.is_none()
    {
        a.miss("graders", format!("{id}: grade-case has no check"));
    }
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    if let Some(i) = &g.implementation {
        let groups = [
            (&i.pristine_dir, &i.tests),
            (
                i.hidden_dir.as_ref().unwrap_or(&i.pristine_dir),
                &i.hidden_tests,
            ),
        ];
        for (sub, tests) in groups {
            for t in tests {
                if !rel_ok(sub) || !rel_ok(&t.path) {
                    a.miss("graders", format!("{id}: unsafe test path {}", t.path));
                    continue;
                }
                match sha256_file(&dir.join(sub).join(&t.path)) {
                    Ok(h) if h == t.sha256 => {}
                    Ok(_) => a.miss(
                        "graders",
                        format!("{id}: grader copy of {} differs from its hash", t.path),
                    ),
                    Err(e) => a.miss("graders", format!("{id}: {e}")),
                }
            }
        }
    }
}

/// run の入口。preflight を通らなければ executor に一度も触れず、out も作らず Err（不足一覧の JSON）。
pub fn run_gated(
    inp: &Inputs,
    host: &dyn Host,
    m: &CampaignManifest,
    input: &InputManifest,
    exec: &dyn Executor,
    opts: &RunOptions,
) -> Result<RunReport, String> {
    gate(inp, host)?;
    let mut o = opts.clone();
    o.lock = Some(expected_lock(inp, m)?);
    runner::run(m, input, exec, &o)
}

/// 通らなければ Err（不足一覧の JSON）。executor・transport を作る前にこれを呼ぶ。
pub fn gate(inp: &Inputs, host: &dyn Host) -> Result<Report, String> {
    let rep = check(inp, host);
    if rep.ok {
        return Ok(rep);
    }
    Err(format!(
        "preflight failed; no paid execution or task was started\n{}",
        serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?
    ))
}
