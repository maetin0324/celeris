---
task: 01M59KY5FRH6P13NZEH9QMCPBA
tasks: [01M59KY5FRH6P13NZEH9QMCPBA]
title: 実適用証拠の検査と固定手順（application-proof）
work_unit: application-proof
status: done
completed: 2026-10-20
updated: 2026-10-20
---
# application-proof

- `cargo test -p celeris-campaign --test ablation_application --test ablation_contract` → exit 0（4 passed / 17 passed）。
- 手順文書: `docs/ops/mechanism-ablation.md`。selection hash `b6132a44…64bc` を python で再計算して一致を確認。
- 実装は追加していない。再利用: wire-variant の `ablation_application.rs` / `ablation_contract.rs`。
- 状態: 実装済み・無LLM検証済み。実 pilot 未実施、効果未測定、内部レビュー・KB共有は任意延期、実 LLM は承認待ち。

## 未解決事項
- main 取り込み後の再検査（`2026-10-19-wire-variant/final.md` §4）。

## 提案
- なし。
