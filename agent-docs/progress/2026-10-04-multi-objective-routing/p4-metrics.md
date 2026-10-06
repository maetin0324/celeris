---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: metrics
status: done
completed: 2026-10-05
---

# Phase 4 metrics: paired outcome の比較指標

`task_core::model_router::metrics` に、ADR §7.2・§8(b) の比較指標を独立した純粋関数として追加した。RouteLLM の APGR と目標品質到達時の strong 呼出し率、RouterBench の非減少 cost–quality 上側凸包の AIQ、AutoMix の IBC を別々に返す。式と原典の固定 revision は関数の doc comment に記した。外部 router や dataset は同梱していない。

入力は同じ task 群について weak・strong・routed の outcome が観測された集約点に限定する。paired 件数が無い、点ごとに件数が一致しない、品質が weak より低い、分母がゼロ、追加 cost が非正の場合は `MetricValue::Undefined { reason }` を返し、`value()` は `None` を返す。p50/p95 用の線形補間 percentile も同じ未定義表現を使う。異なる指標を合成しない。

## 検証

- `cargo nextest run --no-fail-fast -p task-core -E 'test(routing_metrics_apgr_aiq_ibc_known_curves)'` → 1 passed
- `cargo nextest run --no-fail-fast -p task-core` → 746 passed
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

試験は APGR 0.6375、AIQ 0.6625、IBC 0.2、品質ギャップ 50% 到達時の strong 呼出し率 0.25 を手計算値と照合し、未定義理由も確認する。
