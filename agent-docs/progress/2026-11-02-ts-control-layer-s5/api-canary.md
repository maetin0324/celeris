---
title: api-canary — canary の受け付け・人だけの昇格・generation の登録 API と CoS registry
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# api-canary — canary の受け付け・人だけの昇格・generation の登録 API と CoS registry

## 結論
付記 S5-1・S5-4 の API 側を実装した。`PUT /script/domains/{domain}/mode` は全領域で `canary`・`ts` を受け、`→canary`・`canary_bps` の変更・`→ts` は人の API token の主体だけ（他は 403 `script_mode_requires_human`）。`canary` は同じ generation の `compare` から、`ts` は同じ generation の `canary` からだけ（S4 の `cos_digest` の `compare→ts` は消した）。generation の一覧・登録・退役の API と、script-guard の停止の解除（人だけ）を足した。CoS registry は `canary`・`ts` を effect の前に 403（`human_only`）で拒み rejected を記録する。どの拒否でも表と記録は変わらない。

範囲 check の正規表現に入っていない 3 file を変えた（下の「範囲外の変更」）。いずれも付記 S5-12 の api-canary の作業範囲に挙がっており、変えないと task-api・task-core の試験が通らない。

## 変更（commit `phase s5: api-canary: …`）
- `crates/task-api/src/script_domains.rs`
  - 本文 `ScriptDomainModeBody` に `canary_bps`。`canary` は `script_domain_set_canary`、他は `script_domain_set_mode`。`canary_bps` の欠け・範囲外・余分は 422 `script_generation_invalid`。
  - 検査の順: 領域名（404）→ `canary`・`ts` の主体（403。generation の解決より先なので未登録の hash でも 403）→ generation の解決（422）→ store の CAS。
  - 応答 `ScriptDomainView` に `generation_state`・`canary_bps`、`ScriptDomainList` に `guard_halted`。
  - error code: `script_mode_not_enabled` を消し、`script_mode_ts_requires_compare` を `script_mode_requires_previous_stage` に改めた（付記 S5-4）。`script_canary_not_ready`（`extra.readiness` に計数）・`script_guard_halted`（`extra.halted`）。
  - `POST /api/v1/script/guard/clear`（本文 `{reason}`、人の主体だけ、止まっていなければ `cleared: false`）。
- `crates/task-api/src/script_generations.rs`（新）: trait `ScriptGenerationControl`（`register(bundle_sha256)`・`retire(id, actor, reason)`。DTO だけで `deno_core` に依存しない）と `SharedScriptGenerationControl`、error `ScriptGenerationError`（`Invalid`・`Domain`・`Runtime`）。`GET /api/v1/script/generations`、`POST /api/v1/script/generations`、`POST /api/v1/script/generations/{id}/retire`。
  - 未接続は登録・退役とも 503 `script_runtime_unavailable` で表を変えない。一覧は store を読むので未接続でも 200。
  - 登録は hash の形（64 桁の小文字 hex）を確かめ、store の行が生きていれば runtime を呼ばずに今の行を返し、`invalid` なら 422 `script_generation_invalidated`。どちらも isolate を作らない。
  - 退役は未知の id を runtime に渡す前に 404 `script_generation_not_found`。`active` は 409 `script_generation_active`（store）。
- `crates/task-api/src/state.rs`: `ApiState.script_generations: Option<SharedScriptGenerationControl>` と `with_script_generations`。daemon の結線は daemon-gen unit が行う。
- `crates/task-api/src/cos/ops/admin.rs`: `script_domain.mode_set` は本文の `mode` が `canary`・`ts` なら 403 `human_only` で rejected。ALLOWED に `script_generation.register`・`script_generation.retire`、EXCLUDED に `POST /script/guard/clear`（`human_only`）。
- `crates/task-core/src/script_compare.rs`・`store/script_domains.rs`: `ts_may_skip_canary`・`ScriptMode::accepted_for`・`ScriptDomainError::ModeNotEnabled` を消した。store の `ts` の元は同じ generation の `canary` だけ。
- 再生成: `docs/api/v1/api-v1.schema.json`（`UPDATE_SCHEMA=1`）、`docs/api/v1/event.schema.json`（`ScriptMode` の説明）、`web/api/generated/{schema.json,types.ts}`（`node web/scripts/gen-types.mjs`）。`docs/api/v1/api.md` の表 228〜232 と §3.134。
- 試験の追従（`ts` は canary を経る）: `crates/task-dispatch/src/dispatcher/tests/mod.rs` に `promote_cos_digest_to_ts`（計数を積み canary 5000 bps → ts）を足し、`cos_digest_script_ts.rs`・`cos_digest_pass.rs`・`cos_digest_trace.rs` が使う。`crates/celeris/tests/support/script_compare.rs` の `Env::set_mode` は `compare→ts` を canary 経由にする。

## 範囲外の変更（範囲 check の正規表現に無い path）
| path | 理由 |
|---|---|
| `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` | `cos_ops_registry_matches_adr_domain_assignment_and_exclusions` が ADR の表と registry の一致を見る。新しい 3 route の行と除外表の 1 行、付記を足した |
| `config/skills/cos-operator/operations.md` | `cos_operator_skill_table_matches_allowed` が skill の操作表と ALLOWED の一致を見る。2 行を足し、mode_set の行を S5-4 に合わせた。`metadata.version`（policy_version）は上げていない |
| `crates/task-core/src/script_compare/tests.rs` | 消した `accepted_for` の表の検査を除いた |

3 file を基準（`$CELERIS_WU_BASE`）の内容に戻して確かめた（確かめた後で戻した）。ADR と skill が基準のままだと `cos_ops_registry_matches_adr_domain_assignment_and_exclusions` と `cos_operator_skill_table_matches_allowed` が落ち（`cargo nextest` exit 100）、`script_compare/tests.rs` が基準のままだと task-core の lib test が `accepted_for` の E0599 で compile できない。範囲 check の正規表現を広げるしかないので、計画の問題として申告した（`plan_issue`）。

## 試験（付記 S5-12 の名前との対応）
フィルタ `test(/script_domain|script_mode|cos_op/)` に入るよう、generation の試験は `script_domain_generation_*` にした。
| 付記 S5-12 | 実装 |
|---|---|
| `script_generation_register_503_when_unwired` | `tests/script_generations.rs` `script_domain_generation_register_503_when_unwired` |
| `script_generation_register_invalid_422` | `script_domain_generation_register_invalid_422` |
| `script_generation_limit_409` | `script_domain_generation_limit_409` |
| `script_domain_mode_canary_requires_human` | `tests/script_domains.rs` 同名 |
| `script_domain_mode_canary_requires_guard_ready` | 同名 |
| `script_domain_mode_ts_requires_same_generation_canary` | 同名 |
| `script_domain_mode_rollback_by_anyone` | 同名（S4 の `script_domain_mode_ts_rollback_by_anyone` を置き換え） |
| `script_domain_mode_refused_while_halted` | 同名 |
| `script_guard_clear_human_only` | `script_domain_guard_clear_human_only` |
| CoS registry の試験の追従 | `script_domain_mode_through_cos_operations`（canary・ts の 403 rejected）、`script_domain_generation_through_cos_operations`、`cos_ops_*`・`cos_operator_skill_table_matches_allowed` |
足した試験: `script_domain_generation_retire_active_409_and_unknown_404`。消した試験: `script_domain_mode_canary_still_rejected`・`script_domain_mode_ts_only_for_cos_digest`・`script_domain_mode_ts_requires_compare_same_generation`（S5-4 で前提が変わった）。task-core の `script_domain_ts_for_cos_digest_needs_human_and_compare_same_generation` は `script_domain_ts_needs_human_and_same_generation_canary` に改め、`cos_digest` の `compare→ts` が 422 になることを見る。

## 証拠（基準は `$CELERIS_WU_BASE`、提出時の HEAD で実行）
- 提出時の HEAD で: `cargo nextest run -p task-api -E 'test(/script_domain|script_mode|cos_op/)'` → 58 passed。
- 提出時の HEAD で: `bash scripts/dev/test-affected.sh` → exit 0。nextest 7151 passed・21 skipped、web は tsc -b と vitest 101 files・697 tests passed、test-ts-verify 18 passed・0 failed。
- 提出時の HEAD で: `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` を unit の正規表現で絞る）→ 上の 3 file だけが残る。
- `cargo nextest run -p task-dispatch -E 'test(/cos_digest/)'` → 48 passed。
- `cargo nextest run -p celeris -E 'test(/script_compare|script_domain/)'` → 25 passed。
- 1 回目の `bash scripts/dev/test-affected.sh` は `task-core store::tests::event_row_schema_matches_committed` だけ落ちた（`ScriptMode` の説明を変えたため）。`UPDATE_SCHEMA=1` で `event.schema.json` を再生成した。

## 未解決
- daemon の `ScriptGenerationControl` の実装と `with_script_generations` の結線は daemon-gen unit。daemon は登録の前に store を見る必要はない（API が生きた行・`invalid` を先に返す）が、store が拒んだら isolate を捨てること。
- celerisctl の `script mode` は `rust|compare` のままで、`canary`・`ts`・generation の登録の subcommand は無い（celerisctl は範囲外）。

## 提案
- 再計画で api-canary の範囲 check に `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md`・`config/skills/cos-operator/operations.md`・`crates/task-core/src/script_compare/` を足す（付記 S5-12 の作業範囲と同じ）。objective には「作り直さず検証して仕上げる」と書く。
- CoS の skill の操作表を変えたので、policy_version を上げるかを s5-close で決める（今回は上げていない。上げると config の既定・評価契約の追従が要る）。
