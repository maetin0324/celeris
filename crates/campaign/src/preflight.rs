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
    dryrun,
    executor::Executor,
    grader::GradeCase,
    inputs,
    manifest::{CampaignManifest, ConfigId, TokenBasis},
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
    fn program_available(&self, program: &Path) -> bool;
    /// task-api への GET（`path` は `/api/v1/...`）。2xx の本文を返す。token は環境変数名で渡し、値は返さない。
    fn api_get(&self, api_url: &str, token_env: &str, path: &str) -> Result<Value, String>;
    /// A の隔離（bwrap）がこの host で実際に動くか。
    fn sandbox_usable(&self) -> Result<(), String>;
}

pub struct RealHost;

impl Host for RealHost {
    fn env_nonempty(&self, name: &str) -> bool {
        std::env::var_os(name).is_some_and(|v| !v.is_empty())
    }

    fn program_available(&self, program: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        let exec = |p: &Path| {
            fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        };
        if program.components().count() > 1 || program.is_absolute() {
            return exec(program);
        }
        std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|d| exec(&d.join(program))))
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
}

struct Acc {
    checked: Vec<&'static str>,
    missing: Vec<Missing>,
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
        missing: a.missing,
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

    // 5/6. 候補構成・認証/環境。
    check_candidates(a, m, inp, host);
    check_snapshot_src(a, m, inp);

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
    if !host.program_available(&inp.a_program) {
        a.miss(
            "auth-env",
            format!(
                "A harness program {} is not available",
                inp.a_program.display()
            ),
        );
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
    runner::run(m, input, exec, opts)
}

/// 通らなければ Err（不足一覧の JSON）。executor・transport を作る前にこれを呼ぶ。
pub fn gate(inp: &Inputs, host: &dyn Host) -> Result<(), String> {
    let rep = check(inp, host);
    if rep.ok {
        return Ok(());
    }
    Err(format!(
        "preflight failed; no paid execution or task was started\n{}",
        serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?
    ))
}
