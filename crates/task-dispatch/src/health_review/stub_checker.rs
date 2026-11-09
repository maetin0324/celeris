//! 合格の stub の同梱の検査器（ADR 2026-11-08-health-enforcement-loop D1.4 の同梱の検査器の代わり）。
//!
//! health を測らない process だけが使う: task-dispatch の dispatcher 試験の土台（`dispatcher/tests/mod.rs`）と、
//! dagbench の driver（`celeris_campaign::dagbench::celeris_side`）。どちらも base に検査器の無い一時 repo を使うので、
//! 同梱の検査器が無いと適用判定が `Unavailable(checker_missing)` になり、計画は設計 review に、review は policy の
//! 承認待ちに回る（`Unavailable` は合格にしない）。この stub を [`gate::configure_bundled_checker`] で設定すると、
//! 「同梱の検査器があり、`[review]` の無い repo の方針と合格の report を返す」状態になり、通常の repo の流れを通る。
//! 適用判定と `Unavailable` の扱い（本番の規則）は変えない。本番の daemon は release の同梱の検査器
//! （`celeris::daemon::repo_health_path`）を設定し、これを呼ばない。同梱の検査器が無い場合を確かめる試験は呼ばない。
//!
//! stub の `repo_health.py`（隣の JSON を返すだけ。LLM を呼ばず、外部ネットワークに出ない）:
//! - `--print-review-policy`: `source = absent`（`[review]` の無い repo）の方針を出して exit 0。
//! - report の mode（`--out <path>`）: 合格（verdict pass・finding 0・`policy.changed = false`）の report を書いて exit 0。
//!
//! 書く場所は `std::env::temp_dir()` の下の、stub の本文の sha256 で名前を決めた dir（process をまたいで同じ dir を
//! 使い、file は一時 file から rename で置き換える）。版（`CHECKER_TREE` の `repo-health` の行）も同じ sha256 から作る。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use task_core::required_checks::health::{CHECKER_DIR, CHECKER_SCRIPT};

use super::gate::{
    self, BundledChecker, CHECKER_TREE_FILE, POLICY_VERSION, REPORT_SCHEMA, sha256_hex,
};
use super::policy::REVIEW_POLICY_SCHEMA;

/// stub の `repo_health.py`。検査器の dir の [`POLICY_FILE`]・[`REPORT_FILE`] を返す。
const SCRIPT: &str = r#"import os, sys
here = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
if "--print-review-policy" in args:
    with open(os.path.join(here, "stub-review-policy.json")) as f:
        sys.stdout.write(f.read())
    sys.exit(0)
if "--out" not in args:
    sys.stderr.write("stub repo_health.py: --out が要る: %r\n" % args)
    sys.exit(2)
with open(os.path.join(here, "stub-report.json")) as src:
    with open(args[args.index("--out") + 1], "w") as out:
        out.write(src.read())
sys.exit(0)
"#;
/// `--print-review-policy` が出す方針（検査器の dir の中の名前）。
const POLICY_FILE: &str = "stub-review-policy.json";
/// report の mode が書く report（検査器の dir の中の名前）。
const REPORT_FILE: &str = "stub-report.json";

/// 合格の stub の同梱の検査器を書き（process で 1 回）、[`gate::configure_bundled_checker`] で process に設定する。
/// 別の同梱の検査器が既に設定されていれば、その理由の文を返す（上書きしない）。
pub fn configure_passing_stub_checker() -> Result<&'static BundledChecker, String> {
    static WRITTEN: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    let dir = WRITTEN
        .get_or_init(|| write_passing_stub_checker(&std::env::temp_dir()))
        .as_ref()
        .map_err(Clone::clone)?;
    gate::configure_bundled_checker(dir)
}

/// `root` の下に stub の検査器の dir を書き、その dir を返す。
fn write_passing_stub_checker(root: &Path) -> Result<PathBuf, String> {
    let policy = serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA,
        "source": "absent",
        "owner_map": null,
        "high_risk": [],
        "invariants": {},
        "definition_sha256": sha256_hex(SCRIPT.as_bytes()),
    })
    .to_string();
    let report = serde_json::json!({
        "schema": REPORT_SCHEMA,
        "tool": {"version": "passing-stub"},
        "inputs": {"profile": {"resolved": "builtin", "source": "builtin"}},
        "summary": {
            "verdict": "pass",
            "exit_code": 0,
            "reviewable": true,
            "counts": {"findings": 0, "fail_grade": 0, "new": 0, "worsened": 0, "existing": 0},
        },
        "rules": [],
        "findings": [],
        "policy": {"version": POLICY_VERSION, "source": "builtin", "changed": false, "files": []},
    })
    .to_string();
    let mut body = Vec::new();
    for part in [SCRIPT, &policy, &report] {
        body.extend_from_slice(format!("{}\0", part.len()).as_bytes());
        body.extend_from_slice(part.as_bytes());
    }
    let version = sha256_hex(&body);
    let dir = root.join(format!("celeris-passing-stub-checker-{}", &version[..16]));
    let checker = dir.join(CHECKER_DIR);
    std::fs::create_dir_all(&checker)
        .map_err(|e| format!("stub の検査器の dir {} を作れない: {e}", checker.display()))?;
    replace_file(&dir.join(CHECKER_SCRIPT), SCRIPT)?;
    replace_file(&checker.join(POLICY_FILE), &policy)?;
    replace_file(&checker.join(REPORT_FILE), &report)?;
    replace_file(
        &dir.join(CHECKER_TREE_FILE),
        &format!("repo-health {}\n", &version[..40]),
    )?;
    Ok(dir)
}

/// `path` を `body` にする（同じ dir の一時 file に書いて rename する。他の process が読んでいても壊さない）。
fn replace_file(path: &Path, body: &str) -> Result<(), String> {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, body).map_err(|e| format!("{} を書けない: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{} に置けない: {e}", path.display()))
}

#[cfg(test)]
#[path = "stub_checker/tests.rs"]
mod tests;
