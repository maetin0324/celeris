---
title: "承認された範囲の測定と集計（実測は未実施）"
unit: measure
status: done-with-gaps
updated: 2026-10-15
completed: 2026-10-15
---
# measure

## 結論
- **実 LLM の測定は 0 試行。** 承認範囲内で実行できる経路が無かったため、新しい有料測定は始めていない（承認範囲を超えた実行はしていない）。キー値は扱っていない。
- 実 Celeris 実行器が未実装: `crates/campaign/src/main.rs` は `--executor mock` 以外を拒否する（`only mock is available`）。実行器の実装は measure の範囲外（pilot の提案どおり別作業）。worker は subagent・別 LLM CLI を起動できないので、手で代替測定もしていない。
- 以下の数値は全て **pilot の mock / dry-run 由来で、実タスク性能ではない**。

## 承認範囲（人の決定 measure-budget）
- pilot の見積りどおりの件数・上限（A/B 各 5 問、C は subset 2 問、試行 12、tokens 見積り 603,398 ≤ 上限 1,000,000、wall 3,600 s、disk 4 GiB）。
- 7 日間の使用量が codex 21%・claude 24% を超えない範囲に限る。超える見積りなら reduced。fable は 2026-10-19 23:00Z まで使わない（使っていない）。
- 使用量の現在値はこの run から観測できない（**欠測**）。実測 0 なので、本 unit による消費は 0 token（`llm_calls_made 0`）。reduced への切替は不要（実行していないため）。

## 集計（欠測・推定の明示）
| 項目 | 値 | 種別 |
|---|---|---|
| 実測の試行数 | 0 / 12 | 未実施（分母は 12 のまま。除外していない） |
| 実 token・費用 | 欠測 | 実測なし |
| 見積り tokens 合計 | 603,398（A 250,165 / B 250,165 / C 103,068） | **推定**（仮置き係数） |
| mock 実行 | executed 12、tokens_total 113,000、A 5/5、B failed 1・error 1、主比較 both_pass 3 / only_A 2 / only_B 0 | **mock（実性能でない）**。失敗・error は分母に残す |
| 7 日 quota 使用率 | 欠測 | 観測手段なし |
| 統計的主張 | なし | 5 問・1 試行 |

## 実装済み / 未検証
- 実装済み（mock 試験）: manifest/dry-run/runner/checker/export。
- 未検証・未実施: 実 LLM での A/B/C 比較、実行器（task 作成・RunMetrics からの token/cost 採取）、見積り係数の較正、Jev 実分類の成否。

## 提案
1. `Executor` の Celeris 実装（task 起票 → 完了待ち → RunMetrics 採取）を別 task で作る。実名 model の新 manifest（新 hash）で、まず A/B 各 1〜2 問で係数を較正する。
2. 承認済み予算（件数・上限）はその実装後もそのまま使える。再実行時に quota 使用率を先に取得する。
