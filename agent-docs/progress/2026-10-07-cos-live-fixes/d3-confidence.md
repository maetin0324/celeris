---
title: 葉 d3-confidence — ResolveBody に任意の confidence
tasks: [01M4APB5FP8T3TAAE51Z20E3M1]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 d3-confidence（ADR 2026-10-07-cos-live-fixes D3）

## したこと
- `crates/task-api/src/cos/inbox.rs`: `ResolveBody.confidence: Option<f64>`（`serde(default)`、`deny_unknown_fields` は維持）。
  範囲外・非有限は 422 `validation`（監査行を書く前に弾く）。本文そのものが `cos_operations.payload` に入るので confidence はそこに残り、
  observe / escalate の `result` にも `confidence` を入れた。`reason` には混ぜない。
- schema（`docs/api/v1/api-v1.schema.json`）・gui `types.ts`・web `generated/{schema.json,types.ts}` を再生成、`docs/api/v1/gui-api.md` §3.130 を更新。

## 証拠
- `cargo test -p task-api --test cos_triage cos_live_fix_d3_` → 3 passed（記録される・省略で従来どおり・範囲外 422）
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema` → ok、`cargo test -p task-api` 全 ok
- `cargo clippy --workspace -- -D warnings` → exit 0

## 未解決・提案
- `Event::CosOperation`（events の audit 行）には confidence を足していない。Event の欄追加は task-core・query.rs・web の
  event-kinds/invalidation-map に波及するため、監査は `cos_operations` の payload/result に限った。events にも要るなら別 task で。
- answer の `result` は共有 dispatch の戻りなので confidence は payload 側のみ。
