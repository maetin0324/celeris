//! `celeris-campaign human-op`: A/B 同一の command 列 → 同形式の JSONL、再実行で行が増えない、missing は null、validate が不正行を拒否。
use std::{fs, path::Path, process::Command};

use serde_json::Value;

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn run(args: &[&str]) -> (i32, String, String) {
    let o = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
        .arg("human-op")
        .args(args)
        .env("CELERIS_HUMAN_OPS_NOW", "2026-10-19T01:00:00Z")
        .env("CELERIS_HUMAN_OPS_HOST", "eval-host")
        .output()
        .expect("spawn");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

/// 同じ command 列（arm だけ違う）。
fn script(file: &Path, arm: &str) -> Vec<i32> {
    let f = file.to_str().unwrap();
    let id = [
        "--file",
        f,
        "--campaign",
        "camp",
        "--manifest-sha256",
        HASH,
        "--case",
        "case1",
        "--arm",
        arm,
        "--rep",
        "1",
    ];
    let with = |extra: &[&'static str]| -> Vec<&str> {
        id.iter().copied().chain(extra.iter().copied()).collect()
    };
    let cmds: Vec<(&str, Vec<&str>)> = vec![
        (
            "record",
            with(&[
                "--op-kind",
                "research_direction",
                "--actor",
                "eval1",
                "--mode",
                "manual",
                "--intent",
                "research",
                "--logical-op",
                "p1/answer",
                "--started-at",
                "2026-10-19T00:10:00Z",
                "--ended-at",
                "2026-10-19T00:12:00Z",
            ]),
        ),
        // 終了時刻が分からない操作: op_start だけが残り、end は missing のまま。
        (
            "record",
            with(&[
                "--op-kind",
                "status_check",
                "--actor",
                "eval1",
                "--mode",
                "manual",
                "--intent",
                "monitoring",
                "--logical-op",
                "p1/look",
                "--started-at",
                "2026-10-19T00:20:00Z",
            ]),
        ),
        (
            "span",
            with(&[
                "--kind",
                "attempt",
                "--logical-op",
                "p1/train",
                "--attempt",
                "2",
                "--result",
                "failed",
                "--started-at",
                "2026-10-19T00:30:00Z",
            ]),
        ),
        (
            "span",
            with(&[
                "--kind",
                "wait",
                "--wait-kind",
                "human_input",
                "--started-at",
                "2026-10-19T00:40:00Z",
            ]),
        ),
    ];
    cmds.iter()
        .map(|(sub, a)| {
            let mut v = vec![*sub];
            v.extend(a);
            let (code, _, err) = run(&v);
            assert_eq!(code, 0, "{sub}: {err}");
            code
        })
        .collect()
}

fn lines(p: &Path) -> Vec<Value> {
    fs::read_to_string(p)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn keys(v: &Value) -> Vec<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn a_and_b_arms_produce_same_shape_and_rerun_adds_no_rows() {
    let d = tempfile::tempdir().unwrap();
    let (fa, fb) = (d.path().join("a.jsonl"), d.path().join("b.jsonl"));
    script(&fa, "A0");
    script(&fb, "B0");
    let (a, b) = (lines(&fa), lines(&fb));
    assert_eq!(a.len(), b.len());
    assert_eq!(a.len(), 5); // op_start+op_end, op_start, attempt, wait_start
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(keys(x), keys(y), "同じ欄の並び");
        for k in [
            "record_type",
            "op_kind",
            "logical_op_id",
            "start",
            "end",
            "clock_source",
            "host",
            "recorded_at",
        ] {
            assert_eq!(x[k], y[k], "{k}");
        }
    }
    assert_eq!(a[0]["arm"], "A");
    assert_eq!(b[0]["arm"], "B");
    assert_eq!(a[0]["experiment_set"], "main-ab");
    assert_eq!(a[0]["host"], "eval-host");
    assert_eq!(a[0]["clock_source"], "system_utc");
    assert_eq!(a[0]["task_id"], Value::Null);
    assert_eq!(a[0]["missing"]["task_id"], "not_applicable");

    // 同じ command 列の再実行: 行は増えず、ファイルも同一。
    let before = fs::read(&fa).unwrap();
    script(&fa, "A0");
    assert_eq!(fs::read(&fa).unwrap(), before);
    let (code, out, _) = run(&["validate", "--file", fa.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(out.contains("ok: 5 records"), "{out}");
}

#[test]
fn unknown_end_stays_null_with_missing_reason_and_is_not_zero() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("h.jsonl");
    script(&f, "B0");
    let rows = lines(&f);
    let open = rows
        .iter()
        .find(|r| r["logical_op_id"] == "p1/look")
        .unwrap();
    assert_eq!(open["record_type"], "op_start");
    assert_eq!(open["end"], Value::Null);
    let att = rows.iter().find(|r| r["record_type"] == "attempt").unwrap();
    assert_eq!(att["end"], Value::Null);
    assert_eq!(att["missing"]["end"], "unknown");
    // list: 所要時間は null（0 でない）。区間が両端ある op は秒数。
    let (code, out, _) = run(&["list", "--file", f.to_str().unwrap(), "--arm", "B0"]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    let ops = v["ops"].as_array().unwrap();
    let look = ops
        .iter()
        .find(|o| o["logical_op_id"] == "p1/look")
        .unwrap();
    assert_eq!(look["duration_secs"], Value::Null);
    let ans = ops
        .iter()
        .find(|o| o["logical_op_id"] == "p1/answer")
        .unwrap();
    assert_eq!(ans["duration_secs"], 120);
    assert_eq!(v["records"][0]["arm_label"], "B0");
    let (_, none, _) = run(&["list", "--file", f.to_str().unwrap(), "--arm", "A0"]);
    assert_eq!(
        serde_json::from_str::<Value>(&none).unwrap()["records"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn validate_rejects_bad_lines_with_nonzero_exit() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("v.jsonl");
    script(&f, "A0");
    let good = fs::read_to_string(&f).unwrap();
    let fs_ = f.to_str().unwrap();
    // 壊れた JSON 行。
    fs::write(&f, format!("{good}{{not json\n")).unwrap();
    let (code, _, err) = run(&["validate", "--file", fs_]);
    assert_ne!(code, 0);
    assert!(err.contains("line 6"), "{err}");
    // 値を改ざんした行（record_id と材料が合わない）。
    fs::write(
        &f,
        good.replacen("\"op_kind\":\"status_check\"", "\"op_kind\":\"resume\"", 1),
    )
    .unwrap();
    assert_ne!(run(&["validate", "--file", fs_]).0, 0);
    // file が無い。
    assert_ne!(
        run(&[
            "validate",
            "--file",
            d.path().join("none.jsonl").to_str().unwrap()
        ])
        .0,
        0
    );
}

#[test]
fn record_rejects_invalid_input_and_conflicting_rewrite() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("r.jsonl");
    let fs_ = f.to_str().unwrap();
    let base = [
        "record",
        "--file",
        fs_,
        "--campaign",
        "camp",
        "--manifest-sha256",
        HASH,
        "--case",
        "c",
        "--arm",
        "A0",
        "--rep",
        "1",
        "--op-kind",
        "resubmit",
        "--actor",
        "e",
        "--mode",
        "manual",
        "--intent",
        "system",
        "--started-at",
        "2026-10-19T00:00:00Z",
    ];
    // resubmit は reason 必須。
    let (code, _, err) = run(&base);
    assert_ne!(code, 0, "{err}");
    assert!(!f.exists() || lines(&f).is_empty());
    // 不正な arm。
    let mut bad = base.to_vec();
    bad[8] = "--arm";
    bad[9] = "C0";
    assert_ne!(run(&bad).0, 0);
    // 同じ論理操作を別の開始時刻で再送すると conflict（黙って上書きしない）。
    let mut ok = base.to_vec();
    ok.extend(["--reason", "sbatch failed"]);
    assert_eq!(run(&ok).0, 0);
    let mut other = ok.clone();
    other[ok.len() - 3] = "2026-10-19T00:05:00Z";
    let (code, _, err) = run(&other);
    assert_ne!(code, 0);
    assert!(err.contains("食い違う"), "{err}");
    assert_eq!(lines(&f).len(), 1);
}
