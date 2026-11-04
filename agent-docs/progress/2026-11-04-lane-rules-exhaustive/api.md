---
title: 網羅の API と celerisctl
tasks: [01M6J81DZ7C8SMY7HNZTW8GT0Z]
status: done
updated: 2026-11-04
---
# 網羅の API と celerisctl

完了日: 2026-11-04

## 変更
- `api: 網羅の API・celerisctl・schema` — 次を足した。
  - `crates/task-api/src/script_exhaustive.rs`（新規）: `POST`・`GET /api/v1/script/domains/{domain}/exhaustive-runs`。POST は人（`human:*`）と外部 operator（`operator:*`）だけ（`actor_may_run_exhaustive`。他は 403 `script_exhaustive_requires_operator`）。本体は `ScriptGenerationControl::exhaustive_start` を呼び、store の拒否（404・409・422）と runtime の 503 は既存の `domain_problem`・`generation_problem` で code に写す。応答は 202 `ScriptExhaustiveRun`。GET は `standing`・`reference_sha256`・新しい順の最大 50 行（認証された誰でも）。
  - `cos/ops/admin.rs`: `POST …/exhaustive-runs` を EXCLUDED（`script_api_only`）に足した。CoS の operation に包むと 422 `cos_operation_not_allowed`。
  - `celerisctl script exhaustive run <domain> --generation <sha256> --run-reason <text>` と `script exhaustive status <domain> [--json]`。`status` は `cos_read_only`、`run` は `request_of`（CoS の run では `/cos/operations` に包まれて 422）。
  - `docs/api/v1/api-v1.schema.json`・`web/api/generated/{schema.json,types.ts}` を再生成（`UPDATE_SCHEMA=1 cargo test -p task-api`、`node web/scripts/gen-types.mjs`）。`docs/api/v1/api.md` にエンドポイント 236・237 と §3.134.6 を足した。
- 試験:
  - `crates/task-api/tests/script_exhaustive.rs`: `exhaustive_start_requires_human_or_operator`（operator・human は 202、MCP は 403、runtime 無しは 503）、`exhaustive_start_rejects_each_conflict`（404・409 の mode・停止・running、422 の領域・空の reason、400 の本文）、`exhaustive_list_shows_standing`、`exhaustive_start_refused_for_cos`（直接の POST は通らず、包むと 422 `script_api_only`）。単体試験 `exhaustive_start_actor_check_precedes_runtime`（`cos:*`・`mcp:*`・`system:*` を handler が 403 にする）。
  - `crates/task-api/tests/script_domains.rs`: `canary_promote_with_exhaustive_100`（`lane_rules` の `compare→canary`・`canary→ts` が 100 件で通る・99 件は 422・`route_priority` は 1000 のまま）、`canary_promote_without_exhaustive_422`（証拠が無い・不合格・`aborted` は 1000 件）。
  - `crates/celerisctl/src/commands/script_tests.rs`（source-size gate の inline_test_size 300 行の上限のため、script.rs の試験を別 file に移した）: `script_exhaustive_cos_read_only_and_wrapped` ほか（`cos_read_only` と `request_of` の分割が全 subcommand で成り立つ）。
- `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` の表（EXCLUDED と domain 割り当て）に網羅の行を足した（`cos_ops_registry_matches_adr_domain_assignment_and_exclusions` が registry と一致を見る）。
- ADR との違い: 応答の型は別名 `ScriptExhaustiveRunView` を作らず `ScriptExhaustiveRun` をそのまま使う（欄は同じ）。MCP の token は handler の前の middleware で 403 `forbidden`（読み取り専用）になり、`script_exhaustive_requires_operator` を返すのは CoS・`system:*` などの直接の呼び出しだけ。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `cargo test -p task-api --test script_exhaustive` → 4 件合格
- `cargo test -p task-api --test script_domains` → 16 件合格
- `cargo test -p celerisctl --bin celerisctl script` → 10 件合格
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web の tsc・vitest。nextest 5158 件合格、20 件 skip、script bundle の SKIP 0 件）
- `python3 scripts/dev/check-architecture-map.py` → OK

## 未解決事項
- 本番で流す手順は `report` unit の「本番への移行」に書く。worker は本番の daemon・DB・config・release に触れていない。
