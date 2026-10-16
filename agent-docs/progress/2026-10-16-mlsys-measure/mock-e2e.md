---
title: "MLSys 本測定: mock 統合試験（executor・grader・計測・予算の e2e）と mock 印"
tasks: [01M51QNB8EC4EG0D5GA4BDVWHQ]
unit: mock-e2e
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# mock-e2e

## 状態の区分
| 区分 | 内容 |
|---|---|
| 実装済み | `Executor::simulated()`（mock／mock-async は true、arm-dispatch は A/BC のどちらかが true なら true）。run-meta・summary・export の table.json／CSV に `evidence_kind`（mock/real）・`real_performance` を出力。table.json は mock のとき `notice`（実性能ではない）を持つ。run-meta が読めなければ `unknown`＝実測扱いしない |
| mock 検証済み | `tests/mock_e2e.rs` 5 件: ①A/B/C 通し→checker→export（A の失敗・B の executor error が B/C を止めない、報告の数値は生記録再計算と一致しなければ不合格）②空・1 行欠けの trials は export 不可、欠測 token は missing/null で 0 にしない③予算停止で分母は計画どおり・cancelled に残る④応答喪失＋再起動で外部 task は試行ごとに 1 つ・再投入 0 ⑤final campaign に dev input は拒否、別 hash の out 再利用拒否、集計・報告の split 混在拒否。独立 grader（未実行の Markdown は不合格）は既存 `measure_e2e.rs` が担う |
| 実経路検証済み | なし（この WU は mock のみ） |
| 本測定未実施 | 実 Celeris 実行・実 LLM・実クラスタは未実施 |

## 本番移行
無し。CSV に末尾列 `evidence_kind` が増えた（読み手がいれば列追加に追従）。

## 証拠コマンド
- `cargo test -p celeris-campaign` → 全件 ok（mock_e2e 5 passed）。
