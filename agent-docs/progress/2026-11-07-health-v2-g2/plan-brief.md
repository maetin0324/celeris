---
title: 健全性v2 G2 plan-brief — plan /3 の設計 brief と検証
tasks: [01M6TYVJSTYGRPHERB93MM7J59]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G2 plan-brief — plan /3 の設計 brief と検証

完了日: 2026-11-07

WorkUnit `plan-brief`。[ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md) の D3.1〜D3.3 のうち、計画の型と形の検証を実装した。基準は `$CELERIS_WU_BASE`。

## 変更
- 件名「G2 plan-brief: plan /3 の設計 brief と検証」の commit。
- `crates/task-core/src/execution_plan.rs`: `DesignBrief`（`root_cause`・`owner`・`reuse`・`options`・`new_state`・`new_branch`・`new_fallback`・`new_port`・`removals`・`invariants`、`deny_unknown_fields`）と `DesignOption { name, chosen, reason }` を足した。`ExecutionPlanSpec.design_brief: Option<DesignBrief>`（`None` は JSON に出さない）。`DesignBrief::declared_new()` は空でない `new_*` の名前を返す（risk 判定の入力）。`internal_view` と差分の適用（`apply_delta`）は brief を持ち越す。
- `crates/task-core/src/execution_plan/validation.rs`: `PlanValidationError::DesignBrief { field, reason }`・`DesignBriefMissing { classes }` を足した。/1・/2 に brief を書くと `V3FieldNotAllowed { field: "design_brief" }`。`validate_v3` は brief があるときだけ形を見る。
- `crates/task-core/src/execution_plan/design_brief.rs`（新規）: 形の検証 `brief_shape_errors`（D3.2）と `require_design_brief(spec, origin, high_risk_classes)`（D3.3）。高 risk の class に触れる planner の計画に brief が無ければ `DesignBriefMissing` を返す。planner 以外（人の計画など）と、class が空の計画では brief を求めない。どの path がどの class に合うかは dispatch unit の `assess_risk` が決める（この unit では作らない）。
- 試験 `crates/task-core/src/execution_plan/design_brief_tests.rs`（7 件、prefix `design_brief_`）: 正しい brief の受理と往復、必須項目が空の拒否、選んだ選択肢がちょうど 1 件、欠けた brief（planner は拒否、人・class なしは通る）、高 risk 宣言（`new_state`・`new_fallback`・`new_port`）の受理と `declared_new`、/1・/2 での拒否、brief を `max_plan_json_bytes_v3` に数えること。
- 他 crate の `ExecutionPlanSpec { .. }` の構築箇所（celeris・task-api・task-dispatch・task-ops と task-core の試験）に `design_brief: None` を足した。
- 再生成: `docs/protocol/execution-plan.schema.json`、`docs/api/v1/api-v1.schema.json`、`docs/api/v1/event.schema.json`（`UPDATE_SCHEMA=1`）、`web/api/generated/{schema.json,types.ts}`（`node web/scripts/gen-types.mjs`）。`docs/protocol/plan-output.schema.json` は `ExecutionPlanSpec` を含まないので差分なし。`docs/api/v1/api.md` の /3 の検証の節に brief の規則を 1 文足した。

## plan_limits との整合
- `planner_portion_v3` は計画を丸ごと写すので、brief は planner の JSON（`max_plan_json_bytes_v3`、既定 64 KiB）に数える。試験 `design_brief_counts_toward_plan_json_limit` で確かめた。brief 専用の上限は置いていない。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-core -E 'test(design_brief_)'` → 7 passed。
- `UPDATE_SCHEMA=1 cargo nextest run -p task-core -E 'test(schema)'`・同 task-api の後、更新モード無しで再実行 → 一致。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-affected.sh` → 結果は報告本文に書く。

## 未解決事項
- `assess_risk`・`ReviewPolicy`・planner_flow から `require_design_brief` を呼ぶ配線は dispatch unit の範囲。
