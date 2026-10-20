//! `human-op report`: 重複・区間の重なり・待機・再開・途中終了を含む fixture の期待値と、同じ入力からの再生成（手編集なし）。
//! 外部 network・LLM は使わない。Celeris 側の記録は trials.jsonl の行を直接書く（join が読む欄だけ）。
use std::{fs, path::Path, process::Command};

use serde_json::{Value, json};

const H: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn cli(args: &[&str]) -> (i32, String, String) {
    let o = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
        .arg("human-op")
        .args(args)
        .env("CELERIS_HUMAN_OPS_NOW", "2026-10-22T12:00:00Z")
        .env("CELERIS_HUMAN_OPS_HOST", "eval-host")
        .output()
        .expect("spawn");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn rec(f: &str, arm: &str, extra: &[&str]) {
    let mut a = vec![
        "record",
        "--file",
        f,
        "--campaign",
        "c1",
        "--manifest-sha256",
        H,
        "--case",
        "p0",
        "--arm",
        arm,
        "--rep",
        "1",
        "--actor",
        "eval1",
        "--mode",
        "manual",
    ];
    a.extend(extra);
    let (c, _, e) = cli(&a);
    assert_eq!(c, 0, "{extra:?}: {e}");
}

fn span(f: &str, arm: &str, extra: &[&str]) {
    let mut a = vec![
        "span",
        "--file",
        f,
        "--campaign",
        "c1",
        "--manifest-sha256",
        H,
        "--case",
        "p0",
        "--arm",
        arm,
        "--rep",
        "1",
        "--actor",
        "eval1",
    ];
    a.extend(extra);
    let (c, _, e) = cli(&a);
    assert_eq!(c, 0, "{extra:?}: {e}");
}

fn write_trials(dir: &Path) {
    let rows = [
        json!({"campaign_sha256":H,"problem_id":"p0","config":"A","rep":1,"trial_no":1,"outcome":"failed",
               "workspace":"w","wall_secs":600,"tokens":1000,"cost_micro_usd":5,"reason":null}),
        // 再起動で同じ行が 2 回出る
        json!({"campaign_sha256":H,"problem_id":"p0","config":"A","rep":1,"trial_no":1,"outcome":"failed",
               "workspace":"w","wall_secs":600,"tokens":1000,"cost_micro_usd":5,"reason":null}),
        // 実行しなかった B: tokens・費用は欠測（0 にしない）
        json!({"campaign_sha256":H,"problem_id":"p0","config":"B","rep":1,"trial_no":1,"outcome":"error",
               "workspace":"","wall_secs":0,"tokens":null,"cost_micro_usd":null,"reason":"executor_error"}),
    ];
    let text: String = rows.iter().map(|r| format!("{r}\n")).collect();
    fs::write(dir.join("trials.jsonl"), text).expect("trials");
}

fn fixture(dir: &Path) -> String {
    let f = dir.join("human-ops.jsonl");
    let f = f.to_str().expect("p");
    // 重なる 2 つの op（00:10-00:20 と 00:15-00:25 → union 900、単純和 1200）
    rec(
        f,
        "A0",
        &[
            "--op-kind",
            "research_direction",
            "--intent",
            "research",
            "--logical-op",
            "o1",
            "--started-at",
            "2026-10-22T00:10:00Z",
            "--ended-at",
            "2026-10-22T00:20:00Z",
        ],
    );
    let sc = [
        "--op-kind",
        "status_check",
        "--intent",
        "monitoring",
        "--logical-op",
        "o2",
        "--started-at",
        "2026-10-22T00:15:00Z",
        "--ended-at",
        "2026-10-22T00:25:00Z",
    ];
    rec(f, "A0", &sc);
    rec(f, "A0", &sc); // 再送
    // 評価者の測定作業は人の時間・介入に数えない
    rec(
        f,
        "A0",
        &[
            "--op-kind",
            "protocol_step",
            "--intent",
            "protocol",
            "--logical-op",
            "o3",
            "--started-at",
            "2026-10-22T00:00:00Z",
            "--ended-at",
            "2026-10-22T00:05:00Z",
        ],
    );
    // 終了不明（途中終了）の再開
    rec(
        f,
        "A0",
        &[
            "--op-kind",
            "resume",
            "--intent",
            "system",
            "--trigger",
            "failure",
            "--reason",
            "session 切れ",
            "--logical-op",
            "o4",
            "--started-at",
            "2026-10-22T05:00:00Z",
        ],
    );
    // cutoff 以降
    rec(
        f,
        "A0",
        &[
            "--op-kind",
            "status_check",
            "--intent",
            "monitoring",
            "--logical-op",
            "o5",
            "--started-at",
            "2026-10-22T10:00:00Z",
            "--ended-at",
            "2026-10-22T10:01:00Z",
        ],
    );
    span(
        f,
        "A0",
        &["--kind", "cutoff", "--occurred-at", "2026-10-22T09:00:00Z"],
    );
    // 人待ち 300 秒
    span(
        f,
        "A0",
        &[
            "--kind",
            "wait",
            "--wait-kind",
            "permission_prompt",
            "--logical-op",
            "w1",
            "--started-at",
            "2026-10-22T01:00:00Z",
            "--ended-at",
            "2026-10-22T01:05:00Z",
        ],
    );
    // 外部 job（queue 3600 / run 3600）。同じ観測を 2 回書いても 1 件
    let job = [
        "--kind",
        "job",
        "--job-ref",
        "slurm:pegasus:1",
        "--job-submit",
        "2026-10-22T03:00:00Z",
        "--job-start",
        "2026-10-22T04:00:00Z",
        "--job-end",
        "2026-10-22T05:00:00Z",
        "--time-basis",
        "scheduler_accounting",
        "--time-precision",
        "second",
    ];
    span(f, "A0", &job);
    span(f, "A0", &job);
    // 再作業: 基盤起因（attempt 2 が system）と研究上の修正（attempt 2 が research）
    span(
        f,
        "A0",
        &[
            "--kind",
            "attempt",
            "--logical-op",
            "s1",
            "--attempt",
            "1",
            "--result",
            "failed",
            "--started-at",
            "2026-10-22T02:00:00Z",
            "--ended-at",
            "2026-10-22T02:40:00Z",
        ],
    );
    rec(
        f,
        "A0",
        &[
            "--op-kind",
            "resubmit",
            "--intent",
            "system",
            "--trigger",
            "failure",
            "--reason",
            "queue 満杯",
            "--logical-op",
            "s1",
            "--attempt",
            "2",
            "--started-at",
            "2026-10-22T02:50:00Z",
            "--ended-at",
            "2026-10-22T02:52:00Z",
        ],
    );
    span(
        f,
        "A0",
        &[
            "--kind",
            "attempt",
            "--logical-op",
            "r1",
            "--attempt",
            "1",
            "--result",
            "failed",
            "--started-at",
            "2026-10-22T06:00:00Z",
            "--ended-at",
            "2026-10-22T06:10:00Z",
        ],
    );
    rec(
        f,
        "A0",
        &[
            "--op-kind",
            "resubmit",
            "--intent",
            "research",
            "--trigger",
            "failure",
            "--reason",
            "条件変更",
            "--logical-op",
            "r1",
            "--attempt",
            "2",
            "--started-at",
            "2026-10-22T06:20:00Z",
            "--ended-at",
            "2026-10-22T06:21:00Z",
        ],
    );
    f.to_string()
}

fn run_report(f: &str, ev: &Path, out: &Path) -> Value {
    let (c, _, e) = cli(&[
        "report",
        "--file",
        f,
        "--events",
        ev.to_str().expect("p"),
        "--out",
        out.to_str().expect("p"),
    ]);
    assert_eq!(c, 0, "{e}");
    serde_json::from_str(&fs::read_to_string(out.join("report.json")).expect("json"))
        .expect("parse")
}

#[test]
fn fixture_matches_expected_and_regenerates_identically() {
    let d = tempfile::tempdir().expect("tmp");
    write_trials(d.path());
    let f = fixture(d.path());
    let (o1, o2) = (d.path().join("o1"), d.path().join("o2"));
    let j = run_report(&f, d.path(), &o1);
    let a = j["trials"]
        .as_array()
        .expect("t")
        .iter()
        .find(|t| t["arm"] == "A")
        .expect("A");
    let h = &a["human_time"]["value"];
    // o1 ∪ o2 = 00:10–00:25 = 900。単純和 = 600+600+120+... は union との差で重なりを確かめる
    assert_eq!(h["union_secs"], 900 + 120 + 60, "{h}");
    assert_eq!(h["sum_secs"], 600 + 600 + 120 + 60, "{h}");
    assert_eq!(h["overlap_secs"], 300);
    assert_eq!(
        h["open"], 1,
        "終了不明の resume は長さに入れず open に数える"
    );
    assert_eq!(h["after_cutoff"], 1);
    assert_eq!(h["lower_bound"], true);
    assert_eq!(
        a["interventions"]["value"]["status_check"], 2,
        "再送は 1 件、cutoff 後も介入件数に残る/protocol は無い: {a}"
    );
    assert!(a["interventions"]["value"].get("protocol_step").is_none());
    assert_eq!(a["human_wait"]["value"]["union_secs"], 300);
    assert_eq!(a["external_queue_wait"]["value"]["union_secs"], 3600);
    assert_eq!(a["external_run"]["value"]["union_secs"], 3600);
    assert_eq!(
        a["time_quality"]["jobs_deduped"], 0,
        "同一行の再送は resolve 時点で潰れる"
    );
    assert_eq!(a["rework"]["value"]["count"]["infrastructure"], 1);
    assert_eq!(a["rework"]["value"]["count"]["research"], 1);
    assert_eq!(
        a["rework"]["value"]["failed_attempt_secs"]["union_secs"],
        2400 + 600
    );
    assert_eq!(a["failure_cause"]["value"], "verification_failed");
    assert_eq!(a["failure_cause"]["schema"], "provisional-failure-cause/1");
    assert_eq!(
        a["tokens"]["value"]["observed_sum"], 1000,
        "trials.jsonl の重複行は 1 件"
    );
    assert_eq!(a["internal_wait"]["source"], "missing");
    // B: 評価者記録なし・未実行 → 欠測のまま 0 にしない
    let b = j["trials"]
        .as_array()
        .expect("t")
        .iter()
        .find(|t| t["arm"] == "B")
        .expect("B");
    assert_eq!(b["human_time"]["source"], "missing");
    assert_eq!(b["tokens"]["source"], "missing");
    assert_eq!(b["failure_cause"]["value"], "infrastructure_error");
    assert_eq!(j["arms"]["A0"]["human_union_secs"], 1080);
    assert_eq!(j["arms"]["B"]["trials_without_evaluator_records"], 1);
    // 注意書き（下限・A/B 観測差・共有 overhead）
    let md = fs::read_to_string(o1.join("report.md")).expect("md");
    for needle in ["下限", "総労働時間", "観測方法", "共有 overhead"] {
        assert!(md.contains(needle), "{needle}");
    }
    let tl = fs::read_to_string(o1.join("timeline.jsonl")).expect("tl");
    assert!(
        tl.contains("\"status\":\"open\"") && tl.contains("external_queue_wait"),
        "{tl}"
    );
    // 同じ raw から同じ出力（手編集の集計値を正本にしない）
    run_report(&f, d.path(), &o2);
    for n in ["report.json", "report.md", "timeline.jsonl"] {
        assert_eq!(
            fs::read(o1.join(n)).expect("a"),
            fs::read(o2.join(n)).expect("b"),
            "{n}"
        );
    }
}
