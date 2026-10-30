//! 試験用の cases root（ADR 2026-10-30-ops-r2-isolation-principal 付記 H6）。
//!
//! repo の `cases/` を写し、K2 の `private_ref`・`grade_case_ref` の sha256 を repo の旧 fixture
//! （`fixtures/grader-private`・`fixtures/grade-cases`。漏洩済みの試験専用。付記 G1・G4）の実 hash に替える。
//! `cases/` の K2 の値は operator が EVAL_ROOT の新しい private・grade-case の値に替えて commit するので、
//! 試験は repo の `cases/` を fixture の private と組で読まない。

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const K2: &str = "final/k2-bf-xattr-restart-campaign/case.json";

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn sha_hex(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

fn files(dir: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
    let mut ents: Vec<_> = fs::read_dir(dir.join(rel))
        .expect("read cases dir")
        .map(|e| e.expect("dir entry").file_name())
        .collect();
    ents.sort();
    for n in ents {
        let r = rel.join(n);
        if dir.join(&r).is_dir() {
            files(dir, &r, out);
        } else {
            out.push(r);
        }
    }
}

fn build() -> PathBuf {
    let src = crate_root().join("cases");
    let mut list = Vec::new();
    files(&src, Path::new(""), &mut list);
    let mut body: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for r in list {
        let mut b = fs::read(src.join(&r)).expect("read case file");
        if r == Path::new(K2) {
            let mut v: serde_json::Value = serde_json::from_slice(&b).expect("k2 case.json");
            for (key, base) in [
                ("private_ref", "fixtures/grader-private"),
                ("grade_case_ref", "fixtures/grade-cases"),
            ] {
                let p = v[key]["path"].as_str().expect("ref path").to_string();
                let h = sha_hex(&fs::read(crate_root().join(base).join(p)).expect("fixture"));
                v[key]["sha256"] = serde_json::Value::String(h);
            }
            b = serde_json::to_vec_pretty(&v).expect("k2 json");
        }
        body.push((r, b));
    }
    let mut h = Sha256::new();
    for (r, b) in &body {
        h.update(r.to_string_lossy().as_bytes());
        h.update([0]);
        h.update(sha_hex(b).as_bytes());
    }
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let dest = base.join(format!(
        "test-cases-{}",
        &format!("{:x}", h.finalize())[..16]
    ));
    if dest.is_dir() {
        return dest;
    }
    let tmp = base.join(format!("test-cases-tmp-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    for (r, b) in &body {
        let p = tmp.join(r);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, b).expect("write case file");
    }
    // 並走する別の試験 process が先に作っていれば、そちらを使う。
    if fs::rename(&tmp, &dest).is_err() {
        let _ = fs::remove_dir_all(&tmp);
        assert!(dest.is_dir(), "test cases root was not created");
    }
    dest
}

/// 試験用の cases root（K2 の参照 hash を repo の旧 fixture に合わせたもの）。
pub fn cases_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(build).clone()
}
