---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: audit-api
status: done
completed: 2026-10-05
---

# Phase 5 audit-api: routing 監査に estimator shadow の欄を optional で足す

## 変更

- `crates/task-ops/src/routing_audit/estimator.rs`（新設）: `RoutingShadowAudit.estimator`（`kind = estimator` のときだけ）。
  - `estimator_id` / `estimator_version`: `policy_version = estimator:<id>/<version>`（llm-proxy `EstimatorShadow::policy_version`）から読む。書式外は `None`。
  - `outcome`: completed / failed / timeout / dropped / prompt_required（status・reason・detail の理由語から）。`reason` は `ShadowReason` の写し。
  - `unavailable_reason`: failed/dropped の detail の先頭語のうち llm-proxy `SidecarUnavailable::reason` の語と `no_eligible_candidate` だけ（自由文は出さない）。
  - `vs_primary` / `vs_heuristic`: completed の detail `<primary との比較>;heuristic:<heuristic 首位との比較>` を same / differs / no_candidate に。
  - `dependencies`: `needs_prompt`（prompt_required のとき true）、`not_allowed`（dependencies_not_allowed / dependency_mismatch）。`needs_network`・`external_embeddings` は記録に無いので推定せず省く。
  - `overhead_ms`: `latency_ms`（sidecar 往復）。
- `TaskRoutingView.estimator_shadow`（optional）: run を跨いだ要約（targets・completed/failed/timeout/dropped/prompt_required・coverage・primary/heuristic と違った件数・平均 overhead・estimator 一覧）。
- primary の outcome・attempts・review・`runs[]` の旧欄は変えない。HTTP/LLM 呼び出しなし（store の events を読むだけ）。
- `docs/api/v1/api-v1.schema.json`・`gui/app/celeris/types.ts`・`web/api/generated/{schema.json,types.ts}` を再生成。

## 証拠

- `cargo nextest run -p task-api --no-fail-fast -E 'test(routing_estimator_shadow_report_records_coverage_and_limits) | test(routing_shadow_audit_keeps_primary_outcome_separate)'` → 2 tests run: 2 passed。
- `UPDATE_SCHEMA=1 cargo nextest run -p task-api -E 'test(schema)'` → 4 passed。再生成後に UPDATE_SCHEMA なしで同じ試験 → 4 passed（差分なし）。
- `corepack pnpm@11.27.0 -C gui install --offline && … gen:types`、`corepack pnpm@12.6.0 -C web install --offline && … gen:types` → exit 0。
- `cargo nextest run -p task-api --no-fail-fast` → 463 passed, 2 skipped。
- `bash scripts/dev/test-parallel.sh` → 4030 tests run: 4030 passed, 12 skipped、`test-parallel: ok`。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 未解決事項

- descriptor の `needs_network`・`external_embeddings` は `ShadowRecord` に載っていないため API では常に省略される。載せるには task-core `model_router/shadow.rs` と llm-proxy `estimator_shadow.rs` の変更が要る（本 unit の範囲外）。
- task-ops `routing_replay/estimator.rs` の `DETAIL_CODES`（unreachable・protocol_error・external_dependency 等）は llm-proxy の実際の理由語（timeout・max_inflight・transport・dependencies_not_allowed 等）と一致していない。replay report では timeout 以外の理由語が落ちる。integrate-wire か close で揃える必要がある。

## 提案

- `ShadowRecord` に optional の `estimator: { estimator_id, version, needs_prompt, dependencies }` を持たせ、`policy_version` と detail の文字列解析をやめる（後方互換は serde default）。
