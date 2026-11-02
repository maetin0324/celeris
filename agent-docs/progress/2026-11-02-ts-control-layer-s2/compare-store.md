---
title: S2 compare-store — script_domains の migration・mode の CAS 更新・compare event の store
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 compare-store

## 結論
- 境界 ADR の付記 S2-4・S2-5 のとおり、task-core に表・store・event・機密除去を作った。dispatcher と API にはまだ結線していない（dispatch-hook・mode-api の unit）。
- migration `crates/task-core/migrations/0103_script_domains.sql`（`SCHEMA_VERSION` 102 → 103）:
  - `script_generations`（`bundle_sha256` UNIQUE、`state` は `loaded`・`rejected`）。
  - `script_domain_modes`（`mode` の CHECK は `rust`・`compare`・`canary`・`ts`、`mode = 'rust' OR generation_id IS NOT NULL`）。`lane_rules`・`route_priority` を `rust`・generation NULL・version 0 で入れる。
  - `script_domain_events`（追記専用。UPDATE・DELETE は trigger が拒む）。
- `crates/task-core/src/script_compare.rs`: `ScriptDomain`・`ScriptMode`・`CompareOutcome`・`ScriptCompareRecord`・`ScriptDomainError` と、比較入力の機密除去。
  - `LaneRulesInput::redacted(&TaskFeatures)` は 9 軸の `Level` だけを持つ。
  - `RoutePriorityInput::redacted(registry, harness, lane, cos, execution_class)` は、その harness の候補列の `source` id・`model` id と `cos_route_candidates` だけを写す。`SourceSpec`（endpoint・credential_ref・accounts・quota）・`HarnessSpec`・`llm_routes`・他の harness の行は読まない。DTO は `deny_unknown_fields`。
  - `recorded_input` は `sha256:` の digest を作り、直列化が 64 KiB を超えたら値を捨てる（`input_truncated`）。`truncate_error_message` は 512 byte で文字の境界で切る。
- `crates/task-core/src/store/script_domains.rs`:
  - 読み出し: `script_domain_modes`（tick ごとの 1 回）・`script_domain_mode`・`script_domain_active_generation`・`script_generation_get`・`script_generation_by_sha256`・`script_domain_events`。
  - `script_generation_record`: 同じ `bundle_sha256` の行があればそれを返す。
  - `script_domain_set_mode`: 1 つの IMMEDIATE transaction で、`UPDATE … WHERE domain=? AND version=?` の CAS と `script_domain_events` への `Event::ScriptDomainModeChanged` の追記を行う。`canary`・`ts` は `ModeNotEnabled` で拒み、表を変えない。同じ mode・同じ generation は `Unchanged`（表も記録も変えない）。`compare` は `loaded` で manifest の `domains` にその領域がある generation を要る。
  - `script_compare_append`: 同じ task・同じ `domain` の直前の記録と `generation`・`input_digest`・`outcome` が同じなら追記しない。
- `Event` に `ScriptCompareRecorded`（record を flatten）と `ScriptDomainModeChanged` を足した。状態は変えない。
- 範囲外の追従（build と試験を通すために必須だった。下の「未解決」1）:
  - `crates/task-api/src/query.rs`・`query/tests.rs`: `EVENT_TYPES`（84 → 86）と網羅 match の `event_type_name`。
  - `web/api/generated/{schema.json,types.ts}`（`node web/scripts/gen-types.mjs`）・`web/api/realtime/event-kinds.ts`・`invalidation-map.ts`（両種とも `T` だけ）。
  - `crates/campaign/src/ops_r1_trace.rs`: schema 版の固定 102 を `SUPPORTED_SCHEMA_VERSIONS = [102, 103]` にした。103 は `script_*` の表を足しただけで、trace が読む表・欄は同じ。凍結した 102 の DB は今までどおり読める。`crates/celeris/tests/ops_r1_trace.rs` の期待値は `task_core::SCHEMA_VERSION` にした。
- 再生成した schema: `docs/api/v1/event.schema.json`（`UPDATE_SCHEMA=1 cargo test -p task-core`）、`docs/api/v1/api-v1.schema.json`（`UPDATE_SCHEMA=1 cargo test -p task-api`）。

## 試験（名前に script_domain か script_compare を含む）
- `script_domain_initial_modes_are_rust`: 初期値は 2 領域とも rust・generation なし・version 0。
- `script_domain_unknown_domain_is_rejected`: 読み出し・変更とも `UnknownDomain`。
- `script_domain_canary_and_ts_are_rejected_without_change`: rust からも compare からも `canary`・`ts` は拒否、表と記録は変わらない。
- `script_domain_compare_cas_updates_row_and_appends_event`・`script_domain_same_mode_is_unchanged`・`script_domain_compare_needs_loaded_generation_declaring_domain`・`script_domain_generation_reused_by_bundle_hash`。
- `script_domain_cas_conflict_leaves_row_and_events`・`script_domain_cas_conflict_across_connections`: 古い version は `VersionConflict`、表と記録は変わらない。
- `script_domain_mode_update_rolls_back_when_event_append_fails`: 記録の INSERT を trigger で落とすと mode の UPDATE も戻る（1 transaction の確認）。
- `script_domain_events_table_is_append_only`: UPDATE・DELETE と CHECK 違反は失敗。
- `script_compare_append_dedups_same_generation_digest_outcome`。
- `script_compare_route_input_keeps_only_allowlisted_ids`・`script_compare_lane_input_is_nine_levels_only`: 機密の文字列（endpoint・credential_ref・account・via・command・他の harness の model）が除去後の入力と記録に残らない。
- `script_compare_recorded_input_digest_and_truncation`・`script_compare_error_message_cut_at_512_bytes_on_char_boundary`・`script_domain_names_and_modes_parse`。
- task-api: `script_compare_event_types_match_their_serde_names`。

## commit（件名）
1. `phase s2: compare-store: script_domains の migration・mode の CAS 更新・compare event の store`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-core -E 'test(script_domain) | test(script_compare)'` — exit 0、17 passed。
- `bash scripts/dev/test-affected.sh` — 結果は報告本文に書く（Cargo.lock は変えていないが task-core の変更で逆依存の全 crate と web が対象）。
- `cargo clippy --workspace -- -D warnings` — exit 0。
- `cargo fmt --all -- --check` — exit 0。
- web: `pnpm exec tsc -b` exit 0、`pnpm exec vitest run api/realtime` 61 passed。

## 未解決
1. この unit の範囲 check は `crates/task-core/`・`docs/api/`・`web/src/`・`agent-docs/progress/`・境界 ADR だけを許す。`Event` の variant を足すと、`crates/task-api/src/query.rs` の網羅 match（compile が落ちる）、`web/api/`（生成型と event 種別の網羅の型検査）、`crates/campaign/src/ops_r1_trace.rs` と `crates/celeris/tests/ops_r1_trace.rs`（schema 版 102 の固定で試験が落ちる）も変える必要がある。web の生成物は `web/src/` ではなく `web/api/` にある。範囲 check に `crates/task-api/src/query.rs`・`crates/task-api/src/query/`・`web/api/`・`crates/campaign/src/ops_r1_trace.rs`・`crates/celeris/tests/ops_r1_trace.rs` を足す replan を申告した。
2. `cargo clippy --workspace --all-targets -- -D warnings` は、この unit が触っていない `crates/task-core/src/chat/plan_rejudge_tests.rs:78` の `cloned_ref_to_slice_refs` で落ちる（必須の検査は `--all-targets` なしで、こちらは exit 0）。

## 提案
- 後続の unit（mode-api・dispatch-hook・route-prio）の範囲 check には、最初から `web/api/` と `crates/task-api/src/query.rs` を許可する。
- `ops_r1_trace` の schema 版の固定は migration を足すたびに落ちる。読む表・欄の存在を確かめる検査に替えるかを campaign 側で決める。
