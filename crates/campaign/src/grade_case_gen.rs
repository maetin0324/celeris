//! 公開 case（`celeris.campaign-case/1`）から実行可能な `celeris.grade-case/1` を作る変換。
//! 型・schema 名は `celeris.campaign-case-private/1`（期待値・欠陥 list の置き場）と別物で、grade-case は
//! 評価 command・テスト hash・隠しテストの挿入だけを持つ。模範解答は作らない（テスト file は呼び出し側が渡す）。
//! 公開テストは成果物 repo に在るはずのもの（改変検知）、隠しテストは評価側だけが持ち作業 dir に挿入するもの
//! （成果物に無くても削除判定にしない）。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    cases::{Case, CaseKind, CasePrivate},
    grader::{
        GradeCase,
        case::{CaseSplit, ExperimentSpec, ImplementationSpec, PristineTest},
    },
    manifest::Split,
};

pub const PUBLIC_DIR: &str = "pristine";
pub const HIDDEN_DIR: &str = "hidden";

/// 変換の入力。テスト file は `files_dir/public/<path>`・`files_dir/hidden/<path>` に在る。
#[derive(Clone, Debug)]
pub struct GenSource {
    /// 成果物 dir 内の repo の相対 path。
    pub repo: String,
    pub public_tests: Vec<String>,
    pub hidden_tests: Vec<String>,
    /// 隠しテストを挿入する grader 側の script（`hidden_tests` にも含める）。あれば評価 command は
    /// `sh <wrapper> <private の command>` になる（script が挿入後に `exec "$@"` する）。
    /// 実装内の単体テスト（`#[cfg(test)] mod`）は file 単位で重ねられないので、これで子 module として足す。
    pub wrapper: Option<String>,
    pub files_dir: PathBuf,
}

#[derive(Debug)]
pub struct Generated {
    pub case: GradeCase,
    /// grade-case の内容（評価 command・全テスト hash・実験条件）の hash。grader の定義 hash として結合に使う。
    pub definition_sha256: String,
}

/// 評価 command を argv に分ける。shell を介さないので、shell の記法を含む command は拒否する。
fn split_command(cmd: &str) -> Result<Vec<String>, String> {
    if cmd.chars().any(|c| "|&;<>$`()\"'\\*?{}\n".contains(c)) {
        return Err(format!("command needs a shell, not supported: {cmd:?}"));
    }
    let v: Vec<String> = cmd.split_whitespace().map(String::from).collect();
    if v.is_empty() {
        return Err("empty command".into());
    }
    Ok(v)
}

/// cargo の `-- <filter>...` に並ぶ試験名（`-` で始まらないもの）。
fn must_pass_names(argv: &[String]) -> Vec<String> {
    match argv.iter().position(|a| a == "--") {
        Some(i) => argv[i + 1..]
            .iter()
            .filter(|a| !a.starts_with('-'))
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

fn copy_listed(
    from: &Path,
    to: &Path,
    paths: &[String],
    what: &str,
) -> Result<Vec<PristineTest>, String> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_empty()
            || Path::new(p)
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(format!("{what} test path must be relative: {p:?}"));
        }
        let b = fs::read(from.join(p)).map_err(|e| format!("{what} test {p}: {e}"))?;
        let dst = to.join(p);
        if let Some(d) = dst.parent() {
            fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        fs::write(&dst, &b).map_err(|e| format!("{}: {e}", dst.display()))?;
        out.push(PristineTest {
            path: p.clone(),
            sha256: format!("{:x}", Sha256::digest(&b)),
        });
    }
    Ok(out)
}

/// 公開 case と grader 側の private（隠し受入 command）から grade-case を作り、`out_dir` に
/// `<id>.json`・`pristine/`・`hidden/` を書く。実装工程の無い case は implementation を持たない。
pub fn generate(
    case: &Case,
    private: &CasePrivate,
    src: &GenSource,
    out_dir: &Path,
) -> Result<Generated, String> {
    let id = &case.id;
    if private.case_id != case.id || private.split != case.split {
        return Err(format!("{id}: private does not belong to this case"));
    }
    let split = match (case.kind, case.split) {
        (CaseKind::Control, _) => CaseSplit::Control,
        (_, Split::Dev) => CaseSplit::Dev,
        (_, Split::Final) => CaseSplit::Final,
    };
    let has = |s: &str| case.stages.iter().any(|x| x == s);
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let implementation = if has("implementation") {
        let command = split_command(&private.hidden_command).map_err(|e| format!("{id}: {e}"))?;
        let tests = copy_listed(
            &src.files_dir.join("public"),
            &out_dir.join(PUBLIC_DIR),
            &src.public_tests,
            "public",
        )?;
        let hidden_tests = copy_listed(
            &src.files_dir.join("hidden"),
            &out_dir.join(HIDDEN_DIR),
            &src.hidden_tests,
            "hidden",
        )?;
        let must_pass = must_pass_names(&command);
        let baseline_command = private
            .baseline_command
            .as_deref()
            .map(split_command)
            .transpose()
            .map_err(|e| format!("{id}: baseline: {e}"))?;
        let command = match &src.wrapper {
            Some(w) => ["sh".to_string(), w.clone()]
                .into_iter()
                .chain(command)
                .collect(),
            None => command,
        };
        Some(ImplementationSpec {
            pristine_dir: PUBLIC_DIR.into(),
            repo: src.repo.clone(),
            tests,
            hidden_dir: (!hidden_tests.is_empty()).then(|| HIDDEN_DIR.to_string()),
            hidden_tests,
            must_pass,
            command,
            baseline_command,
        })
    } else {
        None
    };
    let experiment = match (&case.experiment, has("experiment")) {
        (Some(e), true) => {
            let min_jobs = private.expected["reference_jobs"]
                .as_array()
                .map_or(1, |a| a.len().max(1));
            Some(ExperimentSpec {
                jobs_file: "evidence/jobs.json".into(),
                min_jobs,
                conditions: [("nodes".to_string(), Value::from(e.nodes))].into(),
            })
        }
        _ => None,
    };
    let g = GradeCase {
        schema: crate::grader::case::GRADE_CASE_SCHEMA.into(),
        id: case.id.clone(),
        split,
        implementation,
        experiment,
        recompute: None,
        review: None,
        judged_scope: case.judged_scope.clone(),
    };
    g.validate()?;
    let body = serde_json::to_string_pretty(&g).map_err(|e| e.to_string())?;
    fs::write(out_dir.join(format!("{id}.json")), &body).map_err(|e| e.to_string())?;
    Ok(Generated {
        definition_sha256: format!("{:x}", Sha256::digest(body.as_bytes())),
        case: g,
    })
}

/// 隠しテストの file を `dir` の下から相対 path で集める（symlink・特殊 file は拒否）。
fn list_hidden(dir: &Path, rel: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let rd = fs::read_dir(dir.join(rel)).map_err(|_| "cannot read hidden tests".to_string())?;
    for e in rd {
        let e = e.map_err(|_| "cannot read hidden tests".to_string())?;
        let t = e
            .file_type()
            .map_err(|_| "cannot read hidden tests".to_string())?;
        let r = rel.join(e.file_name());
        if t.is_dir() {
            list_hidden(dir, &r, out)?;
        } else if t.is_file() {
            out.push(r.to_string_lossy().into_owned());
        } else {
            return Err("hidden tests must be regular files".into());
        }
    }
    Ok(())
}

/// `celeris-campaign gen-grade-case`（ADR 2026-10-30-ops-r2-isolation-principal 付記 G1）。評価器 uid が
/// `EVAL_ROOT` で走らせ、`<private-root>/<split>/<id>/private.json` と `<hidden-root>/<split>/<id>/hidden/` から
/// `<out-root>/<split>/<id>/` に grade-case を作る。返す（stdout に出す）のは `<id>.json` の sha256 だけ。
/// 誤りは固定の短い文だけを返し、private の中身・path の一覧を出さない。隠しテストが無い・空なら拒否する。
pub fn cli(args: &[String]) -> Result<String, String> {
    let mut opt = std::collections::BTreeMap::new();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let v = it
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--case" | "--cases-root" | "--private-root" | "--hidden-root" | "--out-root"
            | "--repo" => {
                opt.insert(flag.as_str(), v.clone());
            }
            _ => return Err(format!("unknown gen-grade-case option: {flag}")),
        }
    }
    let get = |k: &str| opt.get(k).cloned().ok_or(format!("{k} is required"));
    let id = get("--case")?;
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || id.is_empty()
    {
        return Err("invalid case id".into());
    }
    let split = "final";
    let case_path = Path::new(&get("--cases-root")?)
        .join(split)
        .join(&id)
        .join("case.json");
    let case: Case = fs::read_to_string(&case_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .ok_or("cannot load public case.json")?;
    let private: CasePrivate = fs::read_to_string(
        Path::new(&get("--private-root")?)
            .join(split)
            .join(&id)
            .join("private.json"),
    )
    .ok()
    .and_then(|s| serde_json::from_str(&s).ok())
    .ok_or("cannot load private")?;
    let files_dir = Path::new(&get("--hidden-root")?).join(split).join(&id);
    let mut hidden = Vec::new();
    if files_dir.join("hidden").is_dir() {
        list_hidden(&files_dir.join("hidden"), Path::new(""), &mut hidden)?;
    }
    if hidden.is_empty() {
        return Err(
            "hidden tests are missing; refusing to build a hidden_command-only grade-case".into(),
        );
    }
    hidden.sort();
    let wrapper = ".grader/run.sh".to_string();
    let wrapper = hidden.contains(&wrapper).then_some(wrapper);
    let out_dir = Path::new(&get("--out-root")?).join(split).join(&id);
    if out_dir.exists() {
        return Err("output grade-case dir already exists; move it away first".into());
    }
    let src = GenSource {
        repo: opt.get("--repo").cloned().unwrap_or_else(|| "src".into()),
        public_tests: vec![],
        hidden_tests: hidden,
        wrapper,
        files_dir,
    };
    // generate の誤りは private の command を含みうるので、そのまま返さない。
    generate(&case, &private, &src, &out_dir)
        .map_err(|_| "grade-case generation failed".to_string())?;
    let body = fs::read(out_dir.join(format!("{id}.json")))
        .map_err(|_| "cannot read generated grade-case".to_string())?;
    Ok(format!("{:x}", Sha256::digest(body)))
}
