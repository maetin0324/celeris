---
title: guard-fix — s5-guard 統合後の compile 失敗（guard の試験の CosDigestCompareRecord に canary 2 欄）を直す
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# guard-fix — s5-guard 統合後の compile 失敗を直す

## 結論
原因は並行した 2 unit の意味の衝突で、本体の不具合ではない。canary-dispatch が付記 S5-3 に従い `task_core::script_compare::CosDigestCompareRecord` に `canary_selected`・`canary_bps` を足し、並行した guard はその前の形で `crates/celeris/src/daemon/script_guard_stop_tests.rs` の `digest_record` を struct literal で書いた。統合後に E0063（欄の欠け）で `cargo test -p celeris --no-run` が落ちた。literal に 2 欄を足し、canary の記録で停止を確かめる試験を 1 件足した。停止基準の数値・安全検査・store の判定は変えていない。

## 直し方
1. `digest_record`（mode `ts` の記録）に `canary_selected: true`・`canary_bps: None` を足した。付記 S5-3 と `dispatcher/script_canary.rs` の `unit_selected` では mode `ts` の単位は全部 TS の側で、`canary_bps` は mode `canary` のときだけ持つ（CHECK も同じ）。
2. 他の struct literal を grep で確かめた（`CosDigestCompareRecord {`・`ScriptCompareRecord {` を crates 全体で）。該当は task-core（script_compare.rs・cos_digest_record.rs・store/script_domains_tests.rs）、task-dispatch（script_policy.rs・dispatcher/tests/script_compare*.rs・script_canary.rs）、task-api（query/tests.rs）、celeris（tests/script_compare_route_priority.rs）で、canary-dispatch がすでに追従していた。`cargo test -p celeris -p task-core -p task-dispatch -p task-api --no-run` が exit 0 で、残る欠けは無い。
3. guard の計数が canary の記録を正しく数えるかを読んだ。
   - dispatcher は記録の追記と計数を同じ transaction で行う（`script_compare_append_observed`・`script_compare_log_append_observed`）。canary の Rust 側の単位（`shadow_compare`、digest の pass）は `ts_side=false`、TS 側は `true` で渡す。`unavailable` は数えない（S2-8）。
   - `ScriptGuardCounters::observe` は側にかかわらず不一致・timeout・失敗・安全違反を数え、`ts_side` は `canary→ts` の前提の `ts_side_clean_streak` にだけ効く。`evaluate` は側を見ないので、Rust 側の影の不一致・安全違反でも (f) のとおり `rust` へ戻す。
   - 既存の停止試験は `script_guard_observe` を直に呼ぶだけで、canary の記録（`canary_selected` を持つ record）を追記経路で数える試験が無かった。`script_guard_counts_canary_records_on_both_sides` を足した。確かめる点: (a) Rust 側（`canary_selected=false`）の一致は数えるが TS 側の連続に入れない（calls 2・clean 2・ts_side 1）、(b) Rust 側の影の不一致 1 件で `guard_mismatch` により `rust` へ戻り受信箱に項目が出る、(c) Rust 側の安全違反で generation が `invalid` になり `guard_safety_violation` で戻る、(d) TS 側の不一致でも戻る。

## 証拠（提出時の HEAD で実行）
- `cargo test -p celeris -p task-core -p task-dispatch -p task-api --no-run` → exit 0
- `cargo test -p celeris --lib script_guard` → 12 passed（新しい試験を含む）
- `bash scripts/dev/test-affected.sh` → exit 0（対象 crate: celeris・celerisctl、909 tests run: 909 passed、0 skipped、script bundle SKIP 0 件）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決事項
- なし。

## 提案
- 記録の struct に欄を足す unit と、その struct を literal で組む試験を足す unit を同じ段で並列にするときは、試験側に `..Default::default()` が使えないので、範囲 check と計画の objective に互いの欄の追従を書いておく。
