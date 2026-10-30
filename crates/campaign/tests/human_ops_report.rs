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
        "--actor-kind",
        "human",
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
        "--actor-kind",
        "human",
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
        a["interventions"]["value"]["status_check"], 1,
        "再送は 1 件、cutoff 後の op は介入件数に入れない（時間と同じ窓）/protocol は無い: {a}"
    );
    assert_eq!(a["excluded_ops"]["after_window"], 1);
    assert_eq!(a["excluded_ops"]["protocol"], 1);
    assert_eq!(a["observation_window"]["basis"], "cutoff");
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
    // manifest（runner の out dir）を渡していないので成否は欠測。executor の状態は別欄に残る。
    assert_eq!(a["completion"]["source"], "missing");
    assert_eq!(a["success_class"]["source"], "missing");
    assert_eq!(a["executor_status"]["value"], "failed");
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
    assert_eq!(b["executor_status"]["value"], "error");
    assert_eq!(b["completion"]["source"], "missing");
    assert_eq!(
        a["external_queue_wait"]["source"], "evaluator",
        "job 観測があれば evaluator"
    );
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

/// 共有作業（env-lock 修正 71 秒）が A/B に二重記録された生記録を、元の行を書き換えずに
/// correction（取り消し）と shared scope の 1 件で補正し、補正履歴が報告に残ること。
#[test]
fn shared_env_lock_fix_is_counted_once_with_correction_history() {
    let d = tempfile::tempdir().expect("tmp");
    write_trials(d.path());
    let f = d.path().join("human-ops.jsonl");
    let f = f.to_str().expect("p");
    let env_lock = [
        "--op-kind",
        "system_adjustment",
        "--intent",
        "system",
        "--reason",
        "env-lock 再生成",
        "--logical-op",
        "pilot/env-lock-regen",
        "--started-at",
        "2026-10-21T06:54:09Z",
        "--ended-at",
        "2026-10-21T06:55:20Z",
    ];
    rec(f, "A0", &env_lock);
    rec(f, "B0", &env_lock);
    let before = fs::read_to_string(f).expect("raw");
    let o1 = d.path().join("before");
    let j = run_report(f, d.path(), &o1);
    assert_eq!(j["arms"]["A0"]["human_union_secs"], 71);
    assert_eq!(j["arms"]["B0"]["human_union_secs"], 71, "二重計上の再現");

    // 通常 CLI: 取り消しの correction（append-only）と shared scope の 1 件。
    let (c, out, e) = cli(&[
        "correct",
        "--file",
        f,
        "--logical-op",
        "pilot/env-lock-regen",
        "--campaign",
        "c1",
        "--retract",
        "--reason",
        "共有 env-lock 修正を A/B に二重記録した。shared scope へ移す",
    ]);
    assert_eq!(c, 0, "{e}");
    assert_eq!(out.matches("appended").count(), 4, "{out}");
    let mut shared = vec![
        "record",
        "--file",
        f,
        "--campaign",
        "c1",
        "--manifest-sha256",
        H,
        "--scope",
        "shared",
        "--experiment-set",
        "main-ab",
        "--actor",
        "eval1",
        "--actor-kind",
        "human",
        "--mode",
        "manual",
    ];
    shared.extend(env_lock);
    let (c, _, e) = cli(&shared);
    assert_eq!(c, 0, "{e}");
    // shared scope に arm を書くのは拒否（配分しない）。
    let mut bad = shared.clone();
    bad.extend(["--arm", "A0"]);
    assert_ne!(cli(&bad).0, 0);

    let after = fs::read_to_string(f).expect("raw");
    assert!(after.starts_with(&before), "元の行は上書きしない");
    let (c, v, e) = cli(&["validate", "--file", f]);
    assert_eq!(c, 0, "{e}{v}");
    let j = run_report(f, d.path(), &d.path().join("after"));
    for (arm, a) in j["arms"].as_object().expect("arms") {
        assert_eq!(
            a["human_union_secs"].as_u64().unwrap_or(0),
            0,
            "{arm}: 取り消した記録は arm に残らない: {a}"
        );
    }
    let sh = &j["shared_overhead"][0];
    assert_eq!(sh["human_time"]["union_secs"], 71, "{sh}");
    assert_eq!(sh["interventions"]["system_adjustment"], 1);
    let cs = j["corrections"].as_array().expect("history");
    assert_eq!(cs.len(), 4);
    assert!(cs.iter().all(|c| c["set"]["retracted"] == true
        && c["note"].as_str().is_some_and(|n| n.contains("二重記録"))));
    let md = fs::read_to_string(d.path().join("after/report.md")).expect("md");
    assert!(md.contains("補正履歴") && md.contains("二重記録"), "{md}");
}

/// protocol_step・attempt・再作業・介入に共通 filter と共通の観測窓（cutoff が無ければ試行終了）を使う。
#[test]
fn protocol_attempts_and_out_of_window_ops_are_not_counted() {
    let d = tempfile::tempdir().expect("tmp");
    // 試行 A0/p0: 00:00 開始・wall 3600 → 窓の終端 01:00（cutoff なし）
    let trial = json!({"campaign_sha256":H,"problem_id":"p0","config":"A","rep":1,"trial_no":1,
        "outcome":"passed","workspace":"w","wall_secs":3600,"tokens":1,"cost_micro_usd":1,"reason":null});
    fs::write(d.path().join("trials.jsonl"), format!("{trial}\n")).expect("t");
    let start = 1_792_627_200u64; // 2026-10-22T00:00:00Z
    let ledger =
        json!({"manifest_sha256":H,"problem_id":"p0","arm":"A","rep":1,"started_at":start});
    fs::write(d.path().join("ledger.jsonl"), format!("{ledger}\n")).expect("l");
    let f = d.path().join("human-ops.jsonl");
    let f = f.to_str().expect("p");
    for (attempt, at) in [("1", "2026-10-22T00:00:00Z"), ("2", "2026-10-22T00:30:00Z")] {
        rec(
            f,
            "A0",
            &[
                "--op-kind",
                "protocol_step",
                "--intent",
                "protocol",
                "--logical-op",
                "pilot/trial-start",
                "--attempt",
                attempt,
                "--started-at",
                at,
                "--ended-at",
                at,
            ],
        );
    }
    let sc = |at: &'static str, l: &'static str| {
        [
            "--op-kind",
            "status_check",
            "--intent",
            "monitoring",
            "--logical-op",
            l,
            "--started-at",
            at,
            "--ended-at",
            at,
        ]
    };
    rec(f, "A0", &sc("2026-10-22T00:10:00Z", "in"));
    rec(f, "A0", &sc("2026-10-22T02:00:00Z", "late"));
    // 窓の外で始まった attempt 2 の再投入は再作業に数えない
    span(
        f,
        "A0",
        &[
            "--kind",
            "attempt",
            "--logical-op",
            "x",
            "--attempt",
            "1",
            "--result",
            "failed",
            "--started-at",
            "2026-10-22T00:20:00Z",
            "--ended-at",
            "2026-10-22T00:25:00Z",
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
            "--reason",
            "再投入",
            "--logical-op",
            "x",
            "--attempt",
            "2",
            "--started-at",
            "2026-10-22T03:00:00Z",
            "--ended-at",
            "2026-10-22T03:01:00Z",
        ],
    );
    let j = run_report(f, d.path(), &d.path().join("o"));
    let a = &j["trials"][0];
    assert_eq!(a["observation_window"]["basis"], "trial_end", "{a}");
    assert_eq!(
        a["interventions"]["value"],
        json!({"status_check": 1}),
        "{a}"
    );
    assert_eq!(a["excluded_ops"]["protocol"], 2);
    assert_eq!(a["excluded_ops"]["after_window"], 2, "late と x#2");
    assert_eq!(
        a["rework"]["value"]["count"],
        json!({}),
        "protocol の attempt=2 は redo ではない"
    );
    assert_eq!(a["rework"]["value"]["after_window"], 1);
    assert_eq!(
        a["human_wait"]["source"], "missing",
        "wait 記録なしは 0 秒ではない"
    );
    assert_eq!(a["external_queue_wait"]["source"], "missing");
}

/// B の通常 event（export の wait・run 行）から、応答待ち・自動 retry・event 参照を試行に結ぶ。
#[test]
fn b_waits_auto_retries_and_event_refs_join_from_export_rows() {
    let d = tempfile::tempdir().expect("tmp");
    let ex = d.path().join("export");
    fs::create_dir_all(&ex).expect("d");
    let id = |r: &str| json!({"campaign_id":"c1","case_id":"p0","arm":"B","repetition":1,"manifest_hash":H,"task_id":"t1","run_id":r});
    let mut rows = Vec::new();
    for (r, retry, parent) in [("r1", false, Value::Null), ("r2", true, json!("r1"))] {
        let mut v = id(r);
        v["record_type"] = json!("run");
        v["dedup_key"] = json!(format!("run:{r}"));
        v["is_retry"] = json!(retry);
        v["parent_run_id"] = parent;
        v["attempt_no"] = json!(0);
        rows.push(v);
        rows.push(json!({"record_type":"outcome","dedup_key":format!("outcome:{r}"),"run_id":r,"wall_ms":1000}));
    }
    rows.push(
        json!({"record_type":"wait","dedup_key":"wait:t1:5","campaign_id":"c1","task_id":"t1",
        "run_id":"r1","wait_kind":"question","started_at":"2026-10-22T00:10:00Z",
        "ended_at":"2026-10-22T00:12:00Z","start_event":"event:t1:5","end_event":"event:t1:7"}),
    );
    rows.push(
        json!({"record_type":"wait","dedup_key":"wait:t1:9","campaign_id":"c1","task_id":"t1",
        "run_id":null,"wait_kind":"approval","started_at":"2026-10-22T00:20:00Z",
        "ended_at":null,"start_event":"event:t1:9","end_event":null}),
    );
    rows.push(json!({"record_type":"intervention","dedup_key":"intervention:t1:7","campaign_id":"c1",
        "intervention_id":"t1:7","kind":"research_judgment","task_id":"t1","source_event":"event:t1:7"}));
    fs::write(
        ex.join("records.jsonl"),
        rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .expect("w");
    let f = d.path().join("human-ops.jsonl");
    let f = f.to_str().expect("p");
    rec(
        f,
        "B0",
        &[
            "--op-kind",
            "research_direction",
            "--intent",
            "research",
            "--logical-op",
            "answer",
            "--celeris-event-ref",
            "event:t1:7",
            "--started-at",
            "2026-10-22T00:11:00Z",
            "--ended-at",
            "2026-10-22T00:12:00Z",
        ],
    );
    rec(
        f,
        "B0",
        &[
            "--op-kind",
            "status_check",
            "--intent",
            "monitoring",
            "--logical-op",
            "peek",
            "--celeris-event-ref",
            "event:t1:999",
            "--started-at",
            "2026-10-22T00:13:00Z",
            "--ended-at",
            "2026-10-22T00:13:00Z",
        ],
    );
    let j = run_report(f, &ex, &d.path().join("o"));
    let b = &j["trials"][0];
    let jn = &b["join"];
    assert_eq!(
        jn["event_waits"]["value"]["count"],
        json!({"approval":1,"question":1}),
        "{jn}"
    );
    assert_eq!(jn["event_waits"]["value"]["open"], 1);
    assert_eq!(
        b["auto_retries"]["value"]["count"], 1,
        "is_retry の run は自動 retry"
    );
    assert_eq!(b["auto_retries"]["value"]["runs"][0]["parent_run_id"], "r1");
    assert_eq!(
        b["rework"]["value"]["count"],
        json!({}),
        "自動 retry は人の再作業ではない"
    );
    assert_eq!(jn["event_refs"]["value"]["joined"], json!(["event:t1:7"]));
    assert_eq!(
        jn["event_refs"]["value"]["unjoined"],
        json!(["event:t1:999"])
    );
    let w = &b["celeris_wait"]["value"];
    assert_eq!(w["union_secs"], 120, "{w}");
    assert_eq!(w["open"], 1, "決定の無い approval は open（0 秒にしない）");
    assert_eq!(j["arms"]["B0"]["auto_retries"], 1);
}

/// cutoff・job・wait の記録だけでは、介入・人の時間・再作業を観測したことにならない（0 と欠測を区別する）。
/// op を 1 件でも記録した試行は観測済みで、protocol だけなら 0（観測済みの 0）になる。
#[test]
fn cutoff_or_job_only_records_leave_human_time_and_rework_unobserved() {
    let d = tempfile::tempdir().expect("tmp");
    write_trials(d.path());
    let f = d.path().join("human-ops.jsonl");
    let f = f.to_str().expect("p");
    // A0: cutoff と job 観測だけ（op・attempt 記録なし）。
    span(
        f,
        "A0",
        &["--kind", "cutoff", "--occurred-at", "2026-10-22T09:00:00Z"],
    );
    span(
        f,
        "A0",
        &[
            "--kind",
            "job",
            "--job-ref",
            "pbs:sirius:1",
            "--job-submit",
            "2026-10-22T00:01:00Z",
            "--job-start",
            "2026-10-22T00:02:00Z",
            "--job-end",
            "2026-10-22T00:03:00Z",
        ],
    );
    // B0: protocol の op だけ（評価者は見ていた。人の介入は観測済みの 0）。
    rec(
        f,
        "B0",
        &[
            "--op-kind",
            "protocol_step",
            "--intent",
            "protocol",
            "--logical-op",
            "trial-start",
            "--started-at",
            "2026-10-22T00:00:00Z",
            "--ended-at",
            "2026-10-22T00:00:10Z",
        ],
    );
    let j = run_report(f, d.path(), &d.path().join("o"));
    let a = &j["trials"][0];
    assert_eq!(a["arm_labels"][0], "A0", "{a}");
    for k in ["human_time", "interventions", "rework"] {
        assert_eq!(a[k]["source"], "missing", "{k}: {a}");
        assert_eq!(a[k]["value"], Value::Null, "{k}");
    }
    assert_eq!(
        a["external_queue_wait"]["value"]["union_secs"], 60,
        "job 観測は観測済み"
    );
    let arm = &j["arms"]["A0"];
    assert_eq!(arm["human_time_missing_trials"], 1, "{arm}");
    assert_eq!(arm["rework_missing_trials"], 1);
    assert_eq!(arm.get("human_union_secs"), None, "欠測を 0 秒に足さない");

    let b = &j["trials"][1];
    assert_eq!(b["human_time"]["source"], "evaluator", "{b}");
    assert_eq!(b["human_time"]["value"]["union_secs"], 0);
    assert_eq!(b["interventions"]["value"], json!({}));
    assert_eq!(b["rework"]["source"], "evaluator");
    assert_eq!(j["arms"]["B0"]["human_union_secs"], 0);
    assert_eq!(j["arms"]["B0"].get("human_time_missing_trials"), None);

    let md = fs::read_to_string(d.path().join("o/report.md")).expect("md");
    assert!(md.contains("| A0 |"), "{md}");
}

/// 主体の種類を指定する record（`rec` は human 固定）。
fn rec_as(f: &str, kind: &str, actor: &str, extra: &[&str]) {
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
        "A0",
        "--rep",
        "1",
        "--actor-kind",
        kind,
        "--actor",
        actor,
    ];
    a.extend(extra);
    let (c, _, e) = cli(&a);
    assert_eq!(c, 0, "{extra:?}: {e}");
}

/// 介入は論理要求（request_ref）単位: 3 revision は 1 件・history 3。CoS・unknown の操作は人の時間に入らず、
/// 主体別と unknown 欄に別掲。人の時間は質問が開いていた時間（wait）で代用しない。
#[test]
fn request_revisions_count_once_and_only_human_manual_time_counts() {
    let d = tempfile::tempdir().expect("tmp");
    let trial = json!({"campaign_sha256":H,"problem_id":"p0","config":"A","rep":1,"trial_no":1,
        "outcome":"passed","workspace":"w","wall_secs":7200,"tokens":1,"cost_micro_usd":1,"reason":null});
    fs::write(d.path().join("trials.jsonl"), format!("{trial}\n")).expect("t");
    let start = 1_792_627_200u64; // 2026-10-22T00:00:00Z
    let ledger =
        json!({"manifest_sha256":H,"problem_id":"p0","arm":"A","rep":1,"started_at":start});
    fs::write(d.path().join("ledger.jsonl"), format!("{ledger}\n")).expect("l");
    let f = d.path().join("human-ops.jsonl");
    let f = f.to_str().expect("p");
    // 同じ問い question:q1 の 3 revision に人が答える（同じ logical op・attempt。revision で別の行）。
    for (rev, s, e) in [
        ("1", "2026-10-22T00:10:00Z", "2026-10-22T00:15:00Z"),
        ("2", "2026-10-22T00:20:00Z", "2026-10-22T00:22:00Z"),
        ("3", "2026-10-22T00:30:00Z", "2026-10-22T00:33:00Z"),
    ] {
        rec_as(
            f,
            "human",
            "eval1",
            &[
                "--mode",
                "manual",
                "--op-kind",
                "research_direction",
                "--intent",
                "research",
                "--credential",
                "web",
                "--logical-op",
                "q1",
                "--request-ref",
                "question:q1",
                "--request-revision",
                rev,
                "--started-at",
                s,
                "--ended-at",
                e,
            ],
        );
    }
    // 問いが開いていた区間（00:05〜00:40）。人の時間には入れない。
    span(
        f,
        "A0",
        &[
            "--kind",
            "wait",
            "--wait-kind",
            "question",
            "--logical-op",
            "w-q1",
            "--request-ref",
            "question:q1",
            "--request-revision",
            "1",
            "--started-at",
            "2026-10-22T00:05:00Z",
            "--ended-at",
            "2026-10-22T00:40:00Z",
        ],
    );
    // 案件の CoS が q2 に答え、統括の CoS が同じ revision で再対応（差戻し）。
    for (kind, actor, l, s, e) in [
        (
            "cos_project",
            "cos-p",
            "q2",
            "2026-10-22T01:00:00Z",
            "2026-10-22T01:30:00Z",
        ),
        (
            "cos_general",
            "cos-g",
            "q2b",
            "2026-10-22T01:35:00Z",
            "2026-10-22T01:36:00Z",
        ),
    ] {
        rec_as(
            f,
            kind,
            actor,
            &[
                "--mode",
                "automatic",
                "--op-kind",
                "research_direction",
                "--intent",
                "research",
                "--credential",
                "cos_run",
                "--logical-op",
                l,
                "--request-ref",
                "notice:q2",
                "--request-revision",
                "1",
                "--started-at",
                s,
                "--ended-at",
                e,
            ],
        );
    }
    // 主体不明の手作業（10 分）。人にも 0 にも寄せない。
    rec_as(
        f,
        "unknown",
        "who",
        &[
            "--mode",
            "manual",
            "--op-kind",
            "status_check",
            "--intent",
            "monitoring",
            "--logical-op",
            "u1",
            "--started-at",
            "2026-10-22T01:40:00Z",
            "--ended-at",
            "2026-10-22T01:50:00Z",
        ],
    );

    let j = run_report(f, d.path(), &d.path().join("out"));
    let t = &j["trials"][0];
    assert_eq!(
        t["interventions"]["value"],
        json!({"research_direction": 1}),
        "{t}"
    );
    let items = t["intervention_items"]["value"].as_array().expect("items");
    assert_eq!(items.len(), 3, "q1・q2・u1: {items:?}");
    let q1 = items
        .iter()
        .find(|i| i["request_ref"] == "question:q1")
        .expect("q1");
    let h = q1["history"].as_array().expect("history");
    assert_eq!(h.len(), 3);
    let revs: Vec<&str> = h.iter().filter_map(|x| x["revision"].as_str()).collect();
    assert_eq!(revs, ["1", "2", "3"]);
    let evs: Vec<&str> = h.iter().filter_map(|x| x["event"].as_str()).collect();
    assert_eq!(evs, ["initial", "revision", "revision"]);
    let q2 = items
        .iter()
        .find(|i| i["request_ref"] == "notice:q2")
        .expect("q2");
    assert_eq!(q2["human"], false);
    assert_eq!(q2["actor_kinds"], json!(["cos_general", "cos_project"]));
    let evs: Vec<&str> = q2["history"]
        .as_array()
        .expect("h")
        .iter()
        .filter_map(|x| x["event"].as_str())
        .collect();
    assert_eq!(evs, ["initial", "repeat"]);
    assert_eq!(
        t["interventions_by_actor_kind"]["value"],
        json!({"human": 1, "cos_project": 1, "cos_general": 1, "unknown": 1})
    );
    // 人の時間 = 人の manual の区間だけ（300+120+180）。CoS の 30 分・unknown の 10 分・問いの 35 分は入らない。
    assert_eq!(t["human_time"]["value"]["union_secs"], 600, "{t}");
    assert_eq!(t["human_wait"]["value"]["union_secs"], 2100);
    assert_eq!(t["unknown_actor"]["value"]["interventions"], 1);
    assert_eq!(t["unknown_actor"]["value"]["time"]["union_secs"], 600);
    assert_eq!(
        t["excluded_ops"]["non_human"],
        json!({"cos_general": 1, "cos_project": 1, "unknown": 1})
    );
    let a = &j["arms"]["A0"];
    assert_eq!(a["interventions"], json!({"research_direction": 1}), "{a}");
    assert_eq!(a["human_union_secs"], 600);
    assert_eq!(a["unknown_actor_interventions"], 1);
    assert_eq!(a["unknown_actor_union_secs"], 600);
    assert_eq!(a["interventions_by_actor_kind"]["cos_project"], 1);
    let md = fs::read_to_string(d.path().join("out/report.md")).expect("md");
    assert!(
        md.contains("主体別介入") && md.contains("unknown 主体"),
        "{md}"
    );
}

/// 人の時間の記録が無い試行は 0 ではなく欠測（null）で、合計は下限と coverage（記録のある試行・区間の割合）で出す。
/// 時刻の無い人の op・主体不明の op は推測で埋めず、長さを持たない件数と unknown 主体に残す
/// （ADR 2026-10-30-ops-r1-human-time-coverage）。
#[test]
fn unrecorded_human_time_is_missing_with_coverage_not_zero() {
    let d = tempfile::tempdir().expect("tmp");
    write_trials(d.path());
    let f = d.path().join("human-ops.jsonl");
    let f = f.to_str().expect("p");
    // A0: 区間のある人の op（600 秒）と、始点の記録が無い人の op（長さを持たない）。
    rec(
        f,
        "A0",
        &[
            "--op-kind",
            "status_check",
            "--intent",
            "monitoring",
            "--logical-op",
            "h1",
            "--started-at",
            "2026-10-22T00:10:00Z",
            "--ended-at",
            "2026-10-22T00:20:00Z",
        ],
    );
    rec(
        f,
        "A0",
        &[
            "--op-kind",
            "research_direction",
            "--intent",
            "research",
            "--logical-op",
            "h2",
            "--ended-at",
            "2026-10-22T00:40:00Z",
        ],
    );
    // 主体の分からない発言（時刻も無い）。人に寄せない。
    rec_as(
        f,
        "unknown",
        "unrecorded",
        &[
            "--mode",
            "manual",
            "--op-kind",
            "research_direction",
            "--intent",
            "research",
            "--logical-op",
            "u1",
        ],
    );
    // B: 評価者記録なし（arm の label は join の arm 名のまま）。
    let j = run_report(f, d.path(), &d.path().join("o"));
    let a = j["trials"]
        .as_array()
        .expect("t")
        .iter()
        .find(|t| t["arm"] == "A")
        .expect("A");
    assert_eq!(a["human_time"]["value"]["union_secs"], 600, "{a}");
    assert_eq!(a["human_time"]["value"]["missing_start"], 1);
    assert_eq!(a["human_time"]["value"]["lower_bound"], true);
    let c = &a["human_time_coverage"];
    assert_eq!(c["source"], "evaluator", "{c}");
    assert_eq!(c["value"]["bound"], "lower_bound");
    assert_eq!(
        c["value"]["human_ops"], 2,
        "unknown 主体は人の op に入れない"
    );
    assert_eq!(c["value"]["ops_with_interval"], 1);
    assert_eq!(c["value"]["op_interval_ratio"], 0.5);
    assert_eq!(a["unknown_actor"]["value"]["interventions"], 1);
    assert_eq!(a["unknown_actor"]["value"]["time"]["union_secs"], 0);
    assert_eq!(a["unknown_actor"]["value"]["time"]["missing_start"], 1);

    let b = j["trials"]
        .as_array()
        .expect("t")
        .iter()
        .find(|t| t["arm"] == "B")
        .expect("B");
    assert_eq!(b["human_time"]["source"], "missing", "{b}");
    assert_eq!(b["human_time"]["value"], Value::Null);
    let c = &b["human_time_coverage"];
    assert_eq!(c["source"], "missing", "{c}");
    assert_eq!(c["value"]["bound"], "missing");
    assert_eq!(c["value"]["union_secs"], Value::Null, "欠測を 0 秒にしない");
    assert_eq!(c["value"]["op_interval_ratio"], Value::Null);
    assert_eq!(j["arms"]["B"].get("human_union_secs"), None);
    assert_eq!(j["arms"]["B"]["human_time_coverage"]["bound"], "missing");
    assert_eq!(
        j["arms"]["B"]["human_time_coverage"]["union_secs"],
        Value::Null
    );

    let all = &j["human_time_coverage"];
    assert_eq!(all["bound"], "lower_bound", "{all}");
    assert_eq!(all["union_secs"], 600);
    assert_eq!(all["trials"], 2);
    assert_eq!(all["observed_trials"], 1);
    assert_eq!(all["missing_trials"], 1);
    assert_eq!(all["trial_ratio"], 0.5);
    assert_eq!(all["op_interval_ratio"], 0.5);

    let md = fs::read_to_string(d.path().join("o/report.md")).expect("md");
    assert!(
        md.contains("- 全体 lower_bound: union 600 秒、記録のある試行 1/2（0.500）"),
        "{md}"
    );
    assert!(md.contains("- arm B missing: union 欠測 秒"), "{md}");
    let b_row = md.lines().find(|l| l.starts_with("| B |")).expect("B row");
    assert!(
        b_row.contains("| 欠測 (欠測・1) |"),
        "人の時間を 0 と書かない: {b_row}"
    );

    // 同じ入力からは同じ出力。
    let j2 = run_report(f, d.path(), &d.path().join("o2"));
    assert_eq!(j, j2);
}
