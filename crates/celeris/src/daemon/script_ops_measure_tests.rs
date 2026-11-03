//! P1-C: 再起動・切替・drain・退役を繰り返したときの観測（RSS の推移・error/timeout 率・呼び出しの p95・tick の待ちの
//! 集計・受信箱の項目の開き方・canary の選択の比率）。
//!
//! 既定では 3 周だけ回して不変条件（error 0・timeout 0・生きた generation は 2 以下・受信箱の項目は起動時の戻し
//! 1 回につき 1 件だけ開き、2 回目の取り込みは何も開かない・canary の比率が bps に近い）を確かめる。`CELERIS_P1C_MEASURE_OUT=<path>` を渡すと周ごとの raw を jsonl で
//! 書き、`CELERIS_P1C_CYCLES=<n>` で周の数を変える。bundle は手書きの JS（業務 TS ではない）。CPU を焼く負荷は
//! かけない（1 周に lane_rules の呼び出し 200 回。呼び出しの間で待たない単純な列）。drain の時計は `ManualClock`。

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use celeris_script::manifest::{deployed_dir, host_schema_sha256, sha256_hex};
use celeris_script::{GenerationRegistry, ManualClock, RuntimeLimits};
use serde_json::json;
use task_core::SqliteStore;
use task_core::script_compare::{
    LaneRulesInput, ScriptDomain, ScriptGenerationState, ScriptMode, ScriptModeChange,
    canary_selected,
};
use task_core::script_inbox::ScriptInboxStore;
use task_dispatch::script_policy::{PolicyScriptSource, SCRIPT_CALL_DEADLINE};
use time::OffsetDateTime;

use super::ingest;
use crate::ScriptGenerations;

const FIXTURE: &str = r#"
import { hostVersion, selftestEcho } from "celeris:host";
const TAG = "__TAG__";
let count = 0;
globalThis.__celerisDispatch = async (domain, input, ctx) => {
  if (domain === "host_selftest") {
    return { host_api: hostVersion(), reply: await selftestEcho(ctx, input) };
  }
  if (domain === "lane_rules") {
    count += 1;
    return { tag: TAG, count };
  }
  throw new Error(`unknown domain: ${domain}`);
};
"#;

/// 1 周の lane_rules の呼び出しの数と、tick の待ちの集計で 1 tick にまとめる呼び出しの数。
const CALLS: usize = 200;
const CALLS_PER_TICK: usize = 5;

fn deploy(state: &Path, tag: &str) -> String {
    let code = FIXTURE.replace("__TAG__", tag);
    let sha256 = sha256_hex(code.as_bytes());
    let manifest = json!({
        "schema": "celeris.script-manifest/1",
        "bundle_sha256": sha256,
        "host_api": "^1",
        "schema_sha256": host_schema_sha256(),
        "toolchain": {"typescript": "fixture", "esbuild": "fixture", "node": "fixture"},
        "source_commit": "fixture",
        "domains": ["host_selftest", "lane_rules"],
        "capabilities": ["selftest:echo_task"],
        "checkpoint_versions": [],
    });
    let dir = deployed_dir(state, &sha256);
    std::fs::create_dir_all(&dir).expect("deployed dir");
    std::fs::write(dir.join("bundle.mjs"), code).expect("bundle");
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).expect("manifest"),
    )
    .expect("manifest");
    sha256
}

fn lane_input() -> LaneRulesInput {
    serde_json::from_value(json!({"features": {
        "judgment": "medium", "ambiguity": "medium", "verifiability": "medium",
        "reversibility": "medium", "consequence": "medium", "context_size": "medium",
        "tool_intensity": "medium", "expected_length": "medium", "cross_cutting": "medium"
    }}))
    .expect("lane input")
}

fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1).map(str::to_owned))
        })
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn percentile(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (p * sorted.len()).div_ceil(100).max(1) - 1;
    sorted[idx.min(sorted.len() - 1)]
}

fn set_compare(db: &SqliteStore, sha256: &str) {
    let generation = db
        .script_generation_by_sha256(sha256)
        .expect("read")
        .expect("registered");
    let row = db.script_domain_mode("lane_rules").expect("mode");
    db.script_domain_set_mode(
        &ScriptModeChange {
            domain: "lane_rules".into(),
            to_mode: ScriptMode::Compare,
            to_generation: Some(generation.id),
            expected_version: row.version,
            actor: "human:measure".into(),
            reason: "p1c measure".into(),
        },
        OffsetDateTime::now_utc(),
    )
    .expect("set compare");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_ops_restart_drain_retire_cycles() {
    let cycles: usize = std::env::var("CELERIS_P1C_CYCLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let mut out = std::env::var_os("CELERIS_P1C_MEASURE_OUT")
        .map(|p| std::fs::File::create(p).expect("measure out"));
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("state");
    let db = Arc::new(SqliteStore::open(&dir.path().join("t.sqlite3")).expect("store"));
    let rss_start = rss_kib();
    // 周をまたぐ受信箱の項目の累計と、起動時の戻しが起きた周の数（restore の後の mode が rust で前の周が bundle を消した周）。
    let mut inbox_opened_cumulative = 0u64;
    let mut restore_rollbacks = 0u64;
    for cycle in 0..cycles {
        let started = Instant::now();
        // 再起動: 新しい登録簿で復元する（前の周の isolate は shutdown で捨てた）。
        let g = ScriptGenerations::new(
            GenerationRegistry::with_clock(RuntimeLimits::default(), Arc::new(ManualClock::new())),
            state.clone(),
            Arc::clone(&db),
        );
        g.restore().await.expect("restore");
        let restored_mode = db.script_domain_mode("lane_rules").expect("mode").mode;
        let rolled_back = cycle > 0 && (cycle - 1) % 3 == 2;
        if rolled_back {
            assert_eq!(restored_mode, ScriptMode::Rust, "cycle {cycle}");
            restore_rollbacks += 1;
        }
        let sha = deploy(&state, &format!("c{cycle}"));
        let control = g.control();
        let register_sha = sha.clone();
        let row = tokio::task::spawn_blocking(move || control.register(&register_sha))
            .await
            .expect("register task")
            .expect("register");
        assert_eq!(row.state, ScriptGenerationState::Loaded);
        // 切替: 前の generation は draining になり、見回りが in-flight 0 で retired にする。
        set_compare(&db, &sha);
        g.reconcile().await;
        let live = g.registry().live_count();
        assert!(live <= 2, "live generations {live}");
        let row = db
            .script_generation_by_sha256(&sha)
            .expect("read")
            .expect("row");
        let script = PolicyScriptSource::pinned(g.as_ref(), &row).expect("pinned");
        let mut lat_us = Vec::with_capacity(CALLS);
        let (mut errors, mut timeouts) = (0u64, 0u64);
        // tick の待ち（付記 S5-8）: `CALLS_PER_TICK` 回の待ちの和を 1 tick とみなす（dispatcher の `ScriptTickWait` と同じ和）。
        let mut tick_us: Vec<u64> = Vec::with_capacity(CALLS / CALLS_PER_TICK);
        let mut tick_sum = 0u64;
        let budget_us = 200_000u64;
        for n in 0..CALLS {
            let t = Instant::now();
            let result = script.lane_rules(lane_input(), SCRIPT_CALL_DEADLINE).await;
            let waited = t.elapsed();
            let waited_us = u64::try_from(waited.as_micros()).unwrap_or(u64::MAX);
            tick_sum = tick_sum.saturating_add(waited_us);
            if (n + 1) % CALLS_PER_TICK == 0 {
                tick_us.push(tick_sum);
                tick_sum = 0;
            }
            lat_us.push(waited_us);
            match result {
                Ok(_) => {}
                Err(e) if e.kind == "timeout" => timeouts += 1,
                Err(_) => errors += 1,
            }
        }
        lat_us.sort_unstable();
        // 受信箱: 起動時の戻しの周だけ 1 件開き、2 回目の取り込みは何も開かない。開いた項目は周の中で閉じるので
        // `inbox_open_items` は 0 のことが多く、重複の根拠にはならない。根拠は周ごとの `inbox_opened` と、周をまたぐ累計が
        // 戻しの周の数に一致すること。
        let first = ingest(db.as_ref()).expect("ingest");
        let second = ingest(db.as_ref()).expect("ingest again");
        assert_eq!(second.opened, 0, "the second ingest opened items");
        assert_eq!(first.opened, usize::from(rolled_back), "cycle {cycle}");
        inbox_opened_cumulative += first.opened as u64;
        let items = db.script_inbox_open().expect("items");
        tick_us.sort_unstable();
        let row = json!({
            "schema": "celeris.p1c-ops-cycle/1",
            "cycle": cycle,
            "restored_mode": restored_mode.as_str(),
            "live_generations": live,
            "calls": CALLS,
            "errors": errors,
            "timeouts": timeouts,
            "error_rate": errors as f64 / CALLS as f64,
            "timeout_rate": timeouts as f64 / CALLS as f64,
            "call_p50_us": percentile(&lat_us, 50),
            "call_p95_us": percentile(&lat_us, 95),
            "call_max_us": lat_us.last().copied().unwrap_or(0),
            "deadline_us": SCRIPT_CALL_DEADLINE.as_micros() as u64,
            "tick_wait_ticks": tick_us.len(),
            "tick_wait_p50_us": percentile(&tick_us, 50),
            "tick_wait_p95_us": percentile(&tick_us, 95),
            "tick_wait_max_us": tick_us.last().copied().unwrap_or(0),
            "tick_wait_over_budget": tick_us.iter().filter(|w| **w > budget_us).count(),
            "tick_wait_budget_us": budget_us,
            "inbox_opened": first.opened,
            "inbox_reopened_on_second_ingest": second.opened,
            "inbox_open_items": items.len(),
            "inbox_opened_cumulative": inbox_opened_cumulative,
            "restore_rollbacks_cumulative": restore_rollbacks,
            "rss_kib": rss_kib(),
            "rss_start_kib": rss_start,
            "cycle_ms": started.elapsed().as_millis() as u64,
        });
        if let Some(f) = out.as_mut() {
            writeln!(f, "{row}").expect("write");
        }
        assert_eq!(errors, 0, "{row}");
        assert_eq!(inbox_opened_cumulative, restore_rollbacks, "{row}");
        drop(script);
        // 3 周に 1 回は再起動の前に bundle を消す（起動時の戻しと受信箱の項目 1 件を起こす）。
        if cycle % 3 == 2 {
            std::fs::remove_dir_all(deployed_dir(&state, &sha)).expect("remove bundle");
        }
        g.shutdown().await;
    }
    // 起動時の戻しは 4 周目から 3 周ごと（20 周で 6 回）。累計はその数に一致する（重複して開かない）。
    assert_eq!(
        inbox_opened_cumulative,
        (cycles.saturating_sub(1) / 3) as u64,
        "inbox_opened_cumulative over {cycles} cycles"
    );
    // route_priority の canary の選択の比率（決定的。bps 500 = 5%）。
    let generation = "01P1CMEASUREGEN";
    for bps in [100u32, 500, 5000] {
        let units = 20_000u32;
        let selected = (0..units)
            .filter(|n| {
                canary_selected(
                    ScriptDomain::RoutePriority,
                    generation,
                    &format!("task-{n}"),
                    bps,
                )
            })
            .count() as f64;
        let ratio = selected / f64::from(units);
        let expected = f64::from(bps) / 10_000.0;
        if let Some(f) = out.as_mut() {
            writeln!(
                f,
                "{}",
                json!({"schema": "celeris.p1c-canary-ratio/1", "domain": "route_priority",
                       "bps": bps, "units": units, "selected": selected as u64, "ratio": ratio})
            )
            .expect("write");
        }
        assert!(
            (ratio - expected).abs() < 0.01,
            "bps {bps}: ratio {ratio} vs {expected}"
        );
    }
}
