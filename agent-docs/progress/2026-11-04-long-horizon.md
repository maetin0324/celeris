---
title: 長時間自律化（Project Controller）A0〜A9 の成果まとめ
tasks: [01M6JW13XAQ3169H3TTB3H43ZY]
status: done
completed: 2026-11-04
updated: 2026-11-04
---
# 長時間自律化（Project Controller）A0〜A9 の成果まとめ

完了日: 2026-11-04（A9 の時点。実 LLM の pilot は人の gate として残る）

## 結論
- 案件を長期に運営する Project Controller を、既存の Project・root task・events・CoS chat run・control-ts の上に足した
  （[ADR](../adr/2026-11-04-project-controller.md)）。Task は done / failed で終わり、長期の継続は案件の controller の状態が持つ。
  判定は TS、状態遷移・検証・予算・安全は Rust。既定は無効で、本番の挙動は変えていない。
- 確かめたこと（すべて LLM 0）: event 由来の durable wake と crash 境界（A1）、root1→root2→達成（A2）、shadow・dry_run・
  rollback と CoS の独断の拒否（A3）、統括 CoS の相談の 3 経路（A4）、人が承認する Roadmap（A5）、予算・guard・3 日の時計の
  飛び（A6）、許可の ledger（A7）、旧 CoS と新 Manager の比較の再生（A8、`new` の oracle 合格 10/10、旧 `old.passive` 3/10、
  `old.proactive` 8/10）、画面と 72 時間の仮想時刻の pilot・実 daemon の stub pilot（A9）。
- **未達**: 実 LLM の pilot（token・quota・実 walltime・LLM の判断）は未計測。24〜72 時間の実稼働は未実施。どちらも人の承認と
  実行を待つ明示の gate（手順は [a9](2026-11-04-long-horizon/a9.md) の「人の手順」）。

## 子の進捗
| 子 | 進捗 | 主な数値・未達 |
|---|---|---|
| A0 契約 | [a0](2026-11-04-long-horizon/a0.md) | ADR の責任表と API 契約 |
| A1 durable wake | [a1](2026-11-04-long-horizon/a1.md) | 再配送 10 回で効果 1 回、crash 境界、human_epoch の同じ tx |
| A2 core | [a2](2026-11-04-long-horizon/a2.md) | TS の判定と Rust の検証 |
| A3 案件 CoS | [a3](2026-11-04-long-horizon/a3.md) | report の route は未達 |
| A4 統括 CoS の相談 | [a4](2026-11-04-long-horizon/a4.md) | 相談・読取・委譲 |
| A5 Roadmap | [a5](2026-11-04-long-horizon/a5.md) | 人の承認で版が進む |
| A6 長時間の運用 | [a6](2026-11-04-long-horizon/a6.md) | 100 案件 1 周 約 0.19 秒、予算は自動で戻らない |
| A7 許可 | [a7](2026-11-04-long-horizon/a7.md) | 6 分類の fail closed |
| A8 ベンチ | [a8a](2026-11-04-long-horizon/a8a.md)・[a8b](2026-11-04-long-horizon/a8b.md) | 再生 40 試行、実 LLM は未計測 |
| A9 統合・画面・pilot | [a9](2026-11-04-long-horizon/a9.md) | 画面、72 時間の仮想時刻 7 件、stub の実 daemon で C08 到達。実 LLM は未計測 |

## 未解決（親として残るもの）
1. 実 LLM の pilot と 24〜72 時間の実稼働（人の gate）。
2. brief の更新が controller の wake にならない（A8 の未解決 1）。escalate の後は人が mode を置き直す必要がある。
3. 実 LLM の試行の sandbox（`ops-r2-sandbox.sh` の中の daemon、probe-read）は未実装。
4. 本番への導入は人の手順（`docs/ops/project-controller.md`）で、まだ行っていない。

## 提案
- 実 LLM の pilot の結果を見てから、本番の 1 案件で `shadow` から始める（人の判断）。
