---
title: store-gen — generation の状態・canary_bps・script_guard の計数を store に足す
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# store-gen — generation の状態・canary_bps・script_guard の計数を store に足す

## 結論
付記 S5-1・S5-3・S5-4・S5-5・S5-7 の store 側（migration 0105 と task-core の関数）を実装し、範囲を広げた再計画の後に範囲外の追従（task-dispatch の generation の判定・campaign の版 105・試験 3 件）を仕上げた。提出時の HEAD で test-affected・clippy・fmt が通る。

## 変更（commit `phase s5: store-gen: …`）
- `crates/task-core/migrations/0105_script_generations_s5.sql`
  - `script_generations.state` を `loaded`・`active`・`draining`・`retired`・`invalid` に改めた。旧 `rejected` → `invalid`、mode の行が指す旧 `loaded` → `active`、指されない旧 `loaded` → `retired`（再起動で isolate は残らないため。再登録で `loaded` に戻る）。
  - 親の作り直しは `PRAGMA defer_foreign_keys = ON` の下で、同じ名前の親へ行を入れ直す（bundled SQLite の foreign_keys は on。新しい名前で作って RENAME すると commit で外部キー違反になる。移行の試験で見つけた）。
  - 追記専用の `script_generation_events`（UPDATE・DELETE を trigger が拒む）。移した状態は actor `system:migration` で記録する。
  - `script_guard_state`（(domain, generation_id) ごとの計数、領域 `*` の行が全体の停止）。checkpoint の表は作らない（S5-5）。
  - `script_domain_modes.canary_bps` は `apply_migration_version` が列の有無を見て足す（ADD COLUMN は当て直せず、版を巻き戻して当て直す既存の試験が落ちるため）。CHECK は `canary` なら 1〜9999、他は NULL。
- `crates/task-core/src/script_compare.rs`: 状態の enum・遷移表 `may_become`・上限 `SCRIPT_GENERATIONS_LIVE_MAX = 2`、`ScriptGenerationEvent`・`ScriptGenerationTransition`、`checkpoint_versions_cover`、`ts_may_skip_canary`、error の種類（`CanaryRequiresHuman`・`RequiresPreviousStage`・`CanaryBpsInvalid`・`CanaryNotReady`・`GuardHalted`・`GuardClearRequiresHuman`・`CheckpointIncompatible`・`GenerationLimit`・`GenerationInvalidated`・`GenerationActive`・`IllegalTransition`）。
- `crates/task-core/src/script_guard.rs`（新）: (f) の数値の定数、`ScriptGuardCall::classify`（S2-8）、計数、`evaluate`（戻す 6 条件）、`readiness`（昇格の前提。純粋判定 1000 件、CoS 200 件）。
- `crates/task-core/src/store/script_domains.rs`: 登録 `script_generation_record`（上限・`retired` の再登録・`invalid` の拒否）、`script_generation_record_invalid`、`script_generation_transition`（`retired`・`invalid` だけ。`invalid` は指す全領域を `rust` に戻す）、`script_domain_set_canary`、CAS の中での状態の移動（新しい generation を `active`、指されなくなった `active` を `draining`）、`script_domain_active_generations`、`script_generations`、cursor 読み `script_domain_events_after`・`script_generation_events_after`。状態を書くのは `transition_tx` の 1 か所だけ。
- `crates/task-core/src/store/script_guard.rs`（新）: `script_guard_observe`（数え始めた mode と今の mode が違えば数え直す。`canary`・`ts` で条件に当たれば同じ transaction で actor `system:script-guard` で `rust` へ戻す。安全違反は mode にかかわらず `invalid`）、`script_compare_append_observed`・`script_compare_log_append_observed`（追記と計数を 1 transaction）、`script_guard_halt`・`script_guard_clear`（人だけ）・`script_guard_halted`・`script_guard_counters`。
- `crates/task-api/src/script_domains.rs`: 新しい error の写し（最小の追従）。API の受け付けは変えていない（`canary` は `accepted_for` で今も 422）。
- task-core の版数の固定（`SCHEMA_VERSION` 104 → 105）を直した。
- 再計画で範囲に入れた追従: `crates/task-dispatch/src/dispatcher/script_compare.rs` の generation の判定を `Loaded` から `Active` へ、`crates/campaign/src/ops_r1_trace.rs` の `SUPPORTED_SCHEMA_VERSIONS` に 105。

## 付記 S5 との対応と決めたこと
- 付記 S5-12 の試験名はそのまま使った（`script_generation_state_transitions`・`script_generation_rejected_rows_migrate_to_invalid`・`script_generation_events_append_only`・`script_domain_canary_bps_check`・`script_domain_mode_cas_moves_generation_states`・`script_guard_observe_counts_per_generation`・`script_guard_evaluate_each_stop_condition`・`script_guard_state_survives_reopen`）。すべて `store::script_domains::tests::` の下にあり、`test(script_domain)` の filter に入る。足した試験: `script_generation_state_transition_table`・`script_domain_illegal_generation_transition_changes_nothing`・`script_domain_generation_limit_two_live`・`script_domain_cas_conflict_leaves_generation_state_and_records`・`script_domain_mode_rolls_back_when_generation_event_fails`・`script_domain_promote_through_canary_to_ts_and_back`・`script_domain_stage_and_human_rules_change_nothing`・`script_domain_checkpoint_versions_must_cover_stored`・`script_guard_observe_rolls_back_canary_with_guard_actor`・`script_guard_safety_violation_invalidates_generation`・`script_domain_refused_while_guard_halted`・`script_guard_append_observed_counts_after_dedup`。
- 記録の置き場は付記 S5-1 の `script_generation_events`（unit の目的文の「script_domain_events（script_generation_* の kind）」ではなく ADR の決定に従った）。`script_domain_mode_changed` の `canary_bps` は `Event` を変えず（`docs/api/v1/event.schema.json` が範囲外）、`script_domain_events.json` に欄を 1 つ足して書く。
- `NewScriptGeneration.state` は他 crate の試験が struct literal で書くので残した（`loaded` だけ受ける）。
- S4 の `compare→ts`（`cos_digest` だけ）は API の受け付けを変えないため store に残した（`ts_may_skip_canary`）。

## 証拠（基準は `$CELERIS_WU_BASE`、提出時の HEAD で実行）
- `cargo nextest run -p task-api -p task-dispatch -p celeris-campaign -E 'test(/script_domain|cos_digest|script_compare|ops_r1/)'` → 128 passed。
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 7130 passed、21 skipped。test-ts-verify 18 passed、0 failed。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲 check の path（`sh "$CELERIS_WU_SCOPE_PATHS"`）は再計画で許した場所だけ。

## 試験の追従（commit `phase s5: store-gen: draining の generation へ戻す試験 3 件を retired→再登録の形に直す`）
- `crates/task-api/tests/script_domains.rs`: 助け関数 `rearm_generation`（`retired` にしてから `script_generation_record` で `loaded` に戻す）を足し、`script_domain_mode_ts_rollback_by_anyone` の `rust` の後の compare の前で呼ぶ。
- `crates/task-dispatch/src/dispatcher/tests/cos_digest_script.rs`: `set_mode` が、同じ bundle の generation が `draining` なら `retired` にしてから登録し直す。`cos_digest_script_mode_back_to_rust_and_generation_change_mid_compare` と `cos_digest_script_ts.rs` の `cos_digest_script_ts_mode_and_generation_change_during_op_await`（`enter_ts` 経由）がこれで通る。ADR の規則（`draining` は行き先にできない）は store で弱めていない。
- `crates/celeris/tests/` の Loaded を前提とする assert は test-affected で落ちなかったので直していない。

## 未解決
- なし（store-gen の範囲）。dispatcher から `script_compare_append_observed` 等への呼び替えは下の提案のとおり canary-dispatch に残る。

## 提案
- 再計画で store-gen の範囲に `crates/task-dispatch/src/dispatcher/script_compare.rs`・`crates/task-dispatch/src/dispatcher/tests/cos_digest_script.rs`（と `ts` の子 module）・`crates/task-api/tests/script_domains.rs`・`crates/campaign/src/ops_r1_trace.rs` を足し、objective に「作り直さず、試験の追従と検証で仕上げる」と書く。
- `ts_may_skip_canary` は api-canary が API で `compare→ts` を拒むときに消す。消すと task-dispatch・celeris の `ts` の試験が `canary` を経る形に変わるので、その unit の範囲に試験を入れる。
- guard unit の範囲は `crates/celeris/` だけなので、dispatcher から `script_compare_append_observed`・`script_compare_log_append_observed` へ呼び替えるのは canary-dispatch unit に置く。
