---
title: 網羅の証拠の型・判定・readiness 100 件化（task-core）
tasks: [01M6J81DZ7C8SMY7HNZTW8GT0Z]
status: done
updated: 2026-11-04
---
# 網羅の証拠の型・判定・readiness 100 件化

完了日: 2026-11-04

## 変更
- `evidence: 網羅の証拠の型・合否判定・readiness 100 件化` — `task_core::script_exhaustive`（`ScriptExhaustiveRun`・`State`・`StopCode`・`Counts::verdict`・`NewScriptExhaustiveRun`・`Standing`・`exhaustive_standing`・`lane_rules_reference_sha256`）、`script_compare::actor_may_run_exhaustive`、`script_guard::readiness` の第 4 引数 `exhaustive` と `SCRIPT_GUARD_READY_PURE_EXHAUSTIVE = 100`、`ScriptGuardReadiness.exhaustive` を足した。
- `exhaustive_standing` は ADR の形に `domain` と `generation_id` の引数を足した（行の絞り込みを関数の中で行うため）。
- 呼び出し側（`store/script_domains.rs` の昇格 gate、`celeris` の readiness log）は `Missing` を渡す。本当の立場への置き換えは `store` 工程の担当。
- 証拠なし・不合格・`lane_rules` 以外は従来の 1000／200 件と 0.1% のまま。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `cargo nextest run -p task-core script_` → 73 件合格
- `bash scripts/dev/test-affected.sh` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決事項
- `script_domain_promote` の gate はまだ `Missing` 固定。`store` 工程で表と standing の読み出しに替える。
