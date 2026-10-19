//! 全体台帳の原子的な予約・精算と承認上限（ADR 2026-10-19-experiment-sets D5/D7。契約は global_ledger.rs の module doc）。
//!
//! 構成: 先頭に共通の道具（台帳・要求・子 process の起動）、その後に節ごとの試験。
//! process を跨ぐ試験は `child` で同じ試験 binary を子 process として起動し、子は環境変数
//! `GLA_CHILD` を見て役割（`child_main`）だけを実行して終わる。同期は台帳ファイルと flock などの
//! 出来事で取り、sleep や負荷で交差を待たない。
use std::{fs, path::Path, process::Command};

use celeris_campaign::global_ledger::{
    AttemptId, AttemptState, Caps, Consumed, GLOBAL_LEDGER_FILE, GLOBAL_LEDGER_SCHEMA,
    GLOBAL_LEDGER_SCHEMA_V1, GlobalLedger, Published, ReserveRequest, Reserved, Terminal, Usage,
};
use serde_json::json;

// ---- 共通の道具 ----

const CAPS: Caps = Caps {
    tokens: 1_000,
    trials: 10,
    provider_slots: 2,
};

fn id(m: &str, trial: &str, attempt: u32) -> AttemptId {
    AttemptId {
        measurement_id: m.into(),
        trial: trial.into(),
        attempt,
    }
}

fn req(m: &str, trial: &str, attempt: u32, tokens: u64, caps: Caps) -> ReserveRequest {
    ReserveRequest {
        id: id(m, trial, attempt),
        manifest_sha256: format!("sha-{m}"),
        caps,
        tokens,
        provider_slots: 1,
    }
}

fn publish(g: &GlobalLedger, m: &str, tokens: u64, trials: u64, caps: Caps) {
    g.record(&Published {
        measurement_id: m.into(),
        manifest_sha256: format!("sha-{m}"),
        consumed: Consumed {
            tokens,
            wall_secs: 0,
            trials,
        },
        caps,
    })
    .expect("publish");
}

fn lines(dir: &Path) -> usize {
    fs::read_to_string(dir.join(GLOBAL_LEDGER_FILE))
        .map(|t| t.lines().count())
        .unwrap_or(0)
}

fn finished(tokens: u64) -> Terminal {
    Terminal::Finished(Usage {
        tokens,
        wall_secs: 1,
        ran: true,
    })
}

/// 同じ試験 binary を子 process として `role` で起動し、終了を待つ。
fn child(role: &str, dir: &Path) -> std::process::Output {
    Command::new(std::env::current_exe().expect("exe"))
        .args(["--exact", "child_entry", "--nocapture", "--test-threads=1"])
        .env("GLA_CHILD", role)
        .env("GLA_DIR", dir)
        .output()
        .expect("spawn child")
}

/// 子 process の入口。`GLA_CHILD` が無ければ何もしない（通常の試験としては空振り）。
#[test]
fn child_entry() {
    let Ok(role) = std::env::var("GLA_CHILD") else {
        return;
    };
    let dir = std::path::PathBuf::from(std::env::var("GLA_DIR").expect("GLA_DIR"));
    child_main(&role, &dir);
}

fn child_main(role: &str, dir: &Path) {
    let g = GlobalLedger::open(dir).expect("open");
    match role {
        // 予約を fsync した直後に（投入も精算もせず）落ちる。
        "reserve-then-die" => {
            g.reserve(&req("child", "p0", 1, 300, CAPS))
                .expect("reserve");
            std::process::exit(3);
        }
        other => panic!("unknown child role {other}"),
    }
}

// ---- 確定消費と未精算予約の区別・空 publish ----

#[test]
fn empty_publish_keeps_reservations_and_settled_consumption() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    g.reserve(&req("m1", "p0", 1, 200, CAPS)).expect("r0");
    g.mark_submitted(&id("m1", "p0", 1)).expect("sub");
    g.settle(&id("m1", "p0", 1), finished(120)).expect("settle");
    g.reserve(&req("m1", "p1", 1, 300, CAPS)).expect("r1");
    publish(&g, "m1", 500, 3, CAPS);
    // 最初の投入前と同じ空の再集計。
    publish(&g, "m1", 0, 0, CAPS);
    let v = &g.current().expect("cur")["m1"];
    assert_eq!(v.consumed.tokens, 500, "確定消費は下がらない");
    assert_eq!(v.consumed.trials, 3);
    assert_eq!(v.tokens, 800, "確定 500 + 未精算予約 300");
    assert_eq!(v.provider_slots, 1, "未精算の p1 だけが枠を持つ");
    assert_eq!(v.outstanding, 1);
    let s = g.state().expect("state");
    assert_eq!(s.attempts[&id("m1", "p1", 1)].state, AttemptState::Reserved);
}

// ---- 上限: 古い低 cap・増やせない・拒否で何も残さない ----

#[test]
fn old_low_cap_binds_later_larger_manifest_and_is_persisted_with_the_reservation() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    let low = Caps {
        tokens: 400,
        ..CAPS
    };
    publish(&g, "old", 100, 1, low);
    let big = Caps {
        tokens: 1_000_000,
        trials: 1_000,
        provider_slots: 64,
    };
    let before = lines(d.path());
    let e = g
        .reserve(&req("new", "p0", 1, 301, big))
        .expect_err("100 + 301 > 400");
    assert!(e.contains("tokens") && e.contains("400"), "{e}");
    assert_eq!(lines(d.path()), before, "拒否は何も書かない");
    assert_eq!(
        g.reserve(&req("new", "p0", 1, 300, big)).expect("fits"),
        Reserved::New
    );
    let s = g.state().expect("state");
    assert_eq!(s.caps.tokens, 400);
    assert_eq!(s.attempts[&id("new", "p0", 1)].caps.tokens, 400);
    // 大きい上限で publish しても最小は変わらない。
    publish(&g, "new", 0, 0, big);
    assert_eq!(g.state().expect("state").caps, low.min(CAPS).min(big));
}

#[test]
fn trial_and_provider_slot_caps_are_checked_inside_the_reservation() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    let caps = Caps {
        tokens: 10_000,
        trials: 3,
        provider_slots: 2,
    };
    g.reserve(&req("a", "p0", 1, 10, caps)).expect("a p0");
    g.reserve(&req("b", "p0", 1, 10, caps)).expect("b p0");
    let e = g
        .reserve(&req("b", "p1", 1, 10, caps))
        .expect_err("slot sum 3 > 2");
    assert!(e.contains("provider_slots"), "{e}");
    // 終了を確認した枠だけ解放する。
    g.mark_submitted(&id("a", "p0", 1)).expect("sub");
    g.settle(&id("a", "p0", 1), finished(5)).expect("settle");
    g.reserve(&req("b", "p1", 1, 10, caps)).expect("slot freed");
    g.settle(&id("b", "p1", 1), Terminal::NotSubmitted)
        .expect("never submitted");
    g.settle(&id("b", "p0", 1), finished(5)).expect("b p0 done");
    // 試行数: a/p0・b/p0（動いた）+ 新しい試行 1 = 3。もう 1 つは上限を超える。
    g.reserve(&req("a", "p1", 1, 10, caps))
        .expect("third trial");
    let e = g
        .reserve(&req("b", "p2", 1, 10, caps))
        .expect_err("4 trials > 3");
    assert!(e.contains("trials"), "{e}");
}

// ---- 同じ予約の再実行・冪等精算・retry ----

#[test]
fn same_reservation_rerun_is_idempotent_and_settled_identity_cannot_be_reserved() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    let r = req("m", "p0", 1, 400, CAPS);
    assert_eq!(g.reserve(&r).expect("r"), Reserved::New);
    let n = lines(d.path());
    assert_eq!(
        g.reserve(&r).expect("again"),
        Reserved::Existing(AttemptState::Reserved)
    );
    assert_eq!(lines(d.path()), n, "同じ予約は書かない");
    assert_eq!(g.current().expect("c")["m"].tokens, 400, "二重に積まない");
    g.mark_submitted(&r.id).expect("sub");
    g.mark_submitted(&r.id).expect("sub again");
    assert_eq!(
        g.reserve(&r).expect("resume"),
        Reserved::Existing(AttemptState::Submitted)
    );
    g.settle(&r.id, finished(250)).expect("settle");
    let e = g.reserve(&r).expect_err("settled");
    assert!(e.contains("already settled"), "{e}");
}

#[test]
fn resettlement_replaces_and_other_attempts_accumulate() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    let a1 = id("m", "p0", 1);
    g.reserve(&req("m", "p0", 1, 300, CAPS)).expect("r1");
    g.mark_submitted(&a1).expect("sub");
    g.settle(&a1, finished(100)).expect("settle");
    let n = lines(d.path());
    g.settle(&a1, finished(100)).expect("same settle");
    assert_eq!(lines(d.path()), n, "冪等精算は書かない");
    g.settle(&a1, finished(0)).expect("empty resettle");
    assert_eq!(
        g.current().expect("c")["m"].consumed.tokens,
        100,
        "下げない"
    );
    g.settle(&a1, finished(140)).expect("larger resettle");
    assert_eq!(g.current().expect("c")["m"].consumed.tokens, 140, "置換");
    // retry（別 attempt）の消費は合算し、試行数は同じ試行で 1。
    g.reserve(&req("m", "p0", 2, 300, CAPS)).expect("retry");
    g.settle(&id("m", "p0", 2), finished(60))
        .expect("retry done");
    let v = &g.current().expect("c")["m"];
    assert_eq!(v.consumed.tokens, 200);
    assert_eq!(v.consumed.trials, 1);
    // 投入済みの attempt は「投入していない」で解放できない。
    g.reserve(&req("m", "p1", 1, 10, CAPS)).expect("r");
    g.mark_submitted(&id("m", "p1", 1)).expect("sub");
    let e = g
        .settle(&id("m", "p1", 1), Terminal::NotSubmitted)
        .expect_err("submitted");
    assert!(e.contains("not submitted"), "{e}");
    let e = g
        .settle(&id("m", "ghost", 1), finished(1))
        .expect_err("unknown identity");
    assert!(e.contains("unknown attempt"), "{e}");
}

#[test]
fn crash_after_reserve_keeps_the_reservation_held() {
    let d = tempfile::tempdir().expect("dir");
    let o = child("reserve-then-die", d.path());
    assert_eq!(o.status.code(), Some(3), "{o:?}");
    let g = GlobalLedger::open(d.path()).expect("open");
    let v = &g.current().expect("c")["child"];
    assert_eq!((v.tokens, v.provider_slots, v.outstanding), (300, 1, 1));
    // 再開側が投入の無いことを確かめたときだけ解放できる。
    g.settle(&id("child", "p0", 1), Terminal::NotSubmitted)
        .expect("release");
    assert_eq!(g.current().expect("c")["child"].provider_slots, 0);
}

// ---- 旧 /1 行と破損 ----

#[test]
fn legacy_rows_are_read_conservatively() {
    let d = tempfile::tempdir().expect("dir");
    let old = json!({"schema": GLOBAL_LEDGER_SCHEMA_V1, "measurement_id": "old", "manifest_sha256": "x",
        "tokens": 700, "wall_secs": 5, "trials": 4, "provider_slots": 2});
    fs::write(d.path().join(GLOBAL_LEDGER_FILE), format!("{old}\n")).expect("w");
    let g = GlobalLedger::open(d.path()).expect("open");
    let v = &g.current().expect("c")["old"];
    assert_eq!((v.tokens, v.trials, v.provider_slots), (700, 4, 2));
    assert_eq!(
        v.caps,
        Caps::UNLIMITED,
        "cap 欠落は上限を与えない（増やしもしない）"
    );
    // 旧行の枠は解放の記録が無いので占有のまま。
    let e = g
        .reserve(&req("new", "p0", 1, 100, CAPS))
        .expect_err("legacy slots hold the provider");
    assert!(e.contains("provider_slots"), "{e}");
    // 持ち主の小さい再集計で消費は下がらない。終了確認後に枠だけ解放する。
    publish(&g, "old", 10, 1, Caps::UNLIMITED);
    g.confirm_legacy_idle("old").expect("idle");
    let v = &g.current().expect("c")["old"];
    assert_eq!((v.tokens, v.trials, v.provider_slots), (700, 4, 0));
    let e = g
        .reserve(&req("new", "p0", 1, 301, CAPS))
        .expect_err("700 + 301 > 1000");
    assert!(e.contains("tokens"), "{e}");
    g.reserve(&req("new", "p0", 1, 300, CAPS)).expect("fits");
}

#[test]
fn corrupt_unknown_and_torn_ledgers_are_rejected_not_zeroed() {
    let d = tempfile::tempdir().expect("dir");
    let p = d.path().join(GLOBAL_LEDGER_FILE);
    let g = GlobalLedger::open(d.path()).expect("open");
    fs::write(&p, "{not json\n").expect("w");
    assert!(g.state().expect_err("corrupt").contains("corrupt"));
    assert!(g.reserve(&req("m", "p0", 1, 1, CAPS)).is_err());
    fs::write(&p, "{\"schema\":\"celeris.campaign-global-ledger/9\"}\n").expect("w");
    assert!(g.state().expect_err("unknown").contains("unknown"));
    let bad_kind = json!({"schema": GLOBAL_LEDGER_SCHEMA, "kind": "mystery"});
    fs::write(&p, format!("{bad_kind}\n")).expect("w");
    assert!(g.state().expect_err("kind").contains("unknown"));
    // 完結した JSON だが改行で終わっていない（追記の途中で停止）。読めても追記しない。
    let old = json!({"schema": GLOBAL_LEDGER_SCHEMA_V1, "measurement_id": "o", "manifest_sha256": "x",
        "tokens": 1, "wall_secs": 1, "trials": 1, "provider_slots": 0});
    fs::write(&p, old.to_string()).expect("w");
    assert_eq!(g.current().expect("readable")["o"].tokens, 1);
    let e = g
        .reserve(&req("m", "p0", 1, 1, CAPS))
        .expect_err("torn tail");
    assert!(e.contains("torn"), "{e}");
}

// ---- 同時予約（同じ process の別 fd。flock は fd ごと） ----

#[test]
fn concurrent_reservations_never_exceed_the_provider_slots() {
    let d = tempfile::tempdir().expect("dir");
    let path = d.path().to_path_buf();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
    let ok = (0..6)
        .map(|k| {
            let (p, b) = (path.clone(), barrier.clone());
            std::thread::spawn(move || {
                let g = GlobalLedger::open(&p).expect("open");
                b.wait();
                g.reserve(&req(&format!("m{k}"), "p0", 1, 10, CAPS)).is_ok()
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().expect("join"))
        .filter(|ok| *ok)
        .count();
    assert_eq!(ok, CAPS.provider_slots as usize);
    let g = GlobalLedger::open(d.path()).expect("open");
    let s = g.state().expect("state");
    assert_eq!(
        s.attempts
            .values()
            .filter(|a| a.state.outstanding())
            .count(),
        2
    );
}
