---
title: "ADR: MLSys 本測定への最終接続 — 完了の区分と、実経路・本測定を done にしない規則"
status: accepted
date: 2026-10-16
tasks: [01M51QNB8EC4EG0D5GA4BDVWHQ]
---
# MLSys 本測定への最終接続: 設計判断のまとめ

- 関連: 2026-10-15-mlsys-min-campaign（D8〜D13 追補）、2026-10-16-mlsys-measure-task-routing、2026-10-16-mlsys-independent-grader、2026-10-16-metering-attribution

## 文脈
前 root は mock 試験の通過で done になり、実行器・実測・書き手接続が延期のまま残った。本 root は接続を実装したが、実 LLM・本番 DB・クラスタを使う検証は昇格と認証の前で実行できない。

## 決定
- D1 **B/C は通常の task 経路で走らせる**。`POST /api/v1/tasks`（監査付き API）で投入し、待機・再開・子 task 追跡・cancel も API 経由。本番 SQLite への直接書き込みも新スケジューラも置かない。
- D2 **冪等な試行台帳**: key = (manifest sha256, case, arm, repetition) → task/run。投入前に planned を fsync で追記し、冪等キーで照会してから投入する（追記専用）。task-api に専用欄が無いので label で代用（提案: `idempotency_key`）。
- D3 **routing は試行（task）単位で固定**: `routing_override`（migration 0084）。全体 config（`decision_assisted`）の切替に頼らない。B=fixed、C=decision_assisted を同じ daemon で比べる。
- D4 **採点は独立 grader**。実装テスト・実行証拠・生データ再計算・既知欠陥の 4 検査で、未実行なのにもっともらしい Markdown は不合格。評価 command を走らせる実行器は src の process 禁止 guard を保つため grader 本体に入れない（`NoRunner`、fail-closed）。
- D5 **計測**: campaign 識別を子 task・全 run・再試行・レビューへ伝搬。欠測は NULL のまま 0 と区別し、同じ event の再処理で二重計上しない。CoS は共有オーバーヘッドとして別集計。空 export は失敗（`--verify-connected`）。
- D6 **予算**: per_case／per_arm／全体を実行中に適用。独立反復と内部再試行（`max_attempts_per_trial=1`）を分け、失敗・timeout・cancelled・未完了を分母に残す。A の失敗・待機は B/C を止めない。
- D7 **完了の区分**: 報告は「実装済み／mock 検証済み／実経路検証済み／本測定未実施」を項目ごとに分ける。**mock 試験の通過だけでは実経路検証済み・本測定済みにしない**。実経路が認証・予算・環境待ちなら、未実施の項目と手順を残し done で消さない。区分表は `agent-docs/progress/2026-10-16-mlsys-final-connect/final.md`。
- D8 **承認範囲**: 実消費は承認済み予算（試行 12・tokens 1,000,000・使用率上限）内のみ。超える本測定は新しい決定 `final-measure-budget` の承認まで始めない。final case が 2 件のため試行は 6 に固定した（5 問化は別作業・人の承認）。
- D9 **本番への移行は人の手順**: 昇格（GUI「リリース」）、runbook Part H/P/M、`CELERIS_CAMPAIGN_TOKEN` の設定。agent は本番 host・本番キーに触れず、worker から LLM CLI を起動しない（ADR 2026-10-07-worker-no-subagents-no-llm-cli）。

## 結果（現状）
実装済み・mock 検証済みの範囲は final.md §1。実経路検証と本測定は全て未実施で、消費は 0。残りは final.md §2。

## 却下した案
- mock の通過を完了の根拠にする（前 root の done-with-gaps の再発）。
- worker から実 LLM の pilot を走らせる（subagent/LLM CLI 禁止・quota 記録外）。
- 全体 config の切替で B/C を分ける（同時比較できない、記録が試行に残らない）。
