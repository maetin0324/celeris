---
title: canary-dispatch — dispatcher の canary の選択・副作用の前後の扱い・checkpoint の版照合・tick の遅れ
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
updated: 2026-11-03
completed: 2026-11-03
---
# canary-dispatch — dispatcher の canary・ts

## 結論
dispatcher が 3 領域（`lane_rules`・`route_priority`・`cos_digest`）で mode `canary`・`ts` を扱うようにした（境界 ADR の付記 S5-3・S5-5・S5-6・S5-8）。TS の側に選ばれた単位だけ、Rust の検証を通った TS の結果を使う。残りは Rust の結果を使い、TS は影で走らせて記録する。記録の追記は store-gen の `script_compare_append_observed`・`script_compare_log_append_observed`（追記と script-guard の計数を 1 transaction）へ呼び替えた。

## 変えたこと
- task-core `script_compare.rs`: `canary_selected(domain, generation, unit_key, bps)`（S5-3 の sha256 の式）。`ScriptCompareRecord` に `applied_by`・`fallback`・`canary_selected`・`canary_bps`（serde default）、`CosDigestCompareRecord` に `canary_selected`・`canary_bps` を足した。`docs/api/v1/*.schema.json` と `web/api/generated/` を再生成した。
- task-core `store/script_domains.rs`: `script_checkpoint_versions_stored`（S5 では常に空）。
- task-dispatch `script_policy.rs`: `ScriptDomainStore` を `*_append_observed`・`script_domain_rollback`（actor `system:script-guard`）・`script_checkpoint_versions_stored` に改めた。
- task-dispatch `dispatcher/script_canary.rs`（新）: 単位の選択（`lane_rules`・`route_priority` は task id、`cos_digest` は pass id）、TS の側の同期の待ち（1 回 50 ms、1 tick 合計 200 ms の `ScriptTickWait`）、fallback の code（`ts_error`・`invalid_output`・`safety_violation`・`denied`・`unavailable`・`tick_budget`）、戻しの書き込みに失敗した領域を process の中で `rust` として扱い tick ごとに書き直す処理。
- `decide_lane`（dispatch_run.rs）は TS の lane を天井で丸めて使う。`decide_lane_for_work_unit`（work_units.rs）の上限は、同じ tick に TS の lane を使った task ならその lane。
- `route_priority`: TS の側で検証を通った候補列が Rust と違えば、`RouteContext::priority_override` でその harness の列を差し替えて選び直す。
- `cos_digest`: canary の pass は pass id で選ぶ。TS の側の pass は tick の予算の残りまで待つ。denied の op を読んだ出力は使わない。TS の command は冪等 key と記録で照合して書く（`chat_assistant_message_add_once` の `Conflict`、または保存済み message の thread・run が違えば照合不能）。照合不能なら pass の残りを書かず、`digest_due` を下ろさず、`system:script-guard` の `script_effect_unverified: …` で `rust` に戻す（受信箱の項目は script-inbox の経路がこの記録から作る）。書き込みの失敗（transaction の中で何も残らない）は従来どおり次の pass で再試行する。
- `canary`・`ts` の generation は、manifest の `checkpoint_versions` が保存済みの版を含むときだけ呼ぶ（S5-5）。pass の future は deadline で捨て、Promise を持ち越さない（S4 のまま）。
- tick の始めを `begin_tick` にまとめ、前の tick の待ちを集計する（直近 1000 tick の p50・p95・最大と `tick_budget` の件数、`Dispatcher::script_tick_wait_summary`）。

## 試験（提出時の HEAD で実行）
- `cargo nextest run -p task-dispatch -E 'test(/canary|cos_digest|script/)'`: 87 件すべて合格。
- 新しい試験: `canary_selection_is_deterministic_per_unit`（3 領域）、`canary_ts_side_uses_validated_ts_result`、`canary_rust_side_still_compares`、`canary_fallback_before_effect_uses_rust`、`canary_denied_never_fails_open`、`tick_budget_200ms_falls_back_after_budget`、`rollback_write_failure_treated_as_rust`（`tests/script_canary.rs`）、`canary_digest_pass_selection_matches_record`、`canary_digest_no_double_message_or_effect`、`canary_effect_unverified_stops_and_rolls_back`（`tests/cos_digest_script_canary.rs`）。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`: 報告本文に結果を書く。

## 未解決
- `route_priority` の canary は単体の選択の試験だけで、選び直しの経路（`priority_override`）を通す dispatcher の試験は無い（未確認）。
- `tick_budget_200ms_falls_back_after_budget` は `ScriptTickWait` を待ち時間の値の差し替えで確かめる。dispatcher の中で 50 ms の判定を 5 回待たせる試験は無い（TS の待ちは別の current-thread runtime で走るため、試験の `tokio::time::pause` が届かない）。
- daemon が 10 分ごとに `script_tick_wait` の log 行を出す部分は書いていない（`crates/celeris/src/daemon/tick_loop.rs` は範囲外）。`script_tick_wait_summary` を呼べばよい。
- 範囲外の 1 file を変えた: `crates/task-api/src/query/tests.rs`（`ScriptCompareRecord` の literal に新しい 4 欄を足しただけ）。範囲 check がこれで落ちる。

## 提案
- `script_tick_wait` の log 行は guard か s5-close で `tick_loop.rs` に足す。
