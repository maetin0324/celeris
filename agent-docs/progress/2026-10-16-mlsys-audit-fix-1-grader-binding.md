---
tasks: [01M530Y2MNRW5K6E4ERMSZNVHV]
status: done
updated: 2026-10-16
title: "[監査修正1] grader の結果を最終成功率・export に結合する"
---

# 監査修正 1: grader 結果の結合

## 再確認
着手時の main は 0e73be29（監査と同じ）。`checker::recompute` / `export` は `TrialRecord.outcome` だけから pass_rate を出していた状態を確認した。

## 変更（`crates/campaign`）
- `grades.rs`（新）: grades.jsonl を試行記録へ結合。検査: schema・campaign hash・split・case id（行と grade の両方）・grade-case split・grader hash の存在・trial 存在・成果物 hash == 記録した `workspace_sha256_final`・重複。拒否した grade は未採点扱いで違反に出す。
- `grade_out.rs`: 行に schema / campaign_sha256 / split / trial_no / case_id / `grader_sha256` / `artifact_sha256` を持たせる。case id 不一致・成果物 hash 不一致は採点せず `rejected`。`ok` は処理完了であり合否ではない。
- `checker.rs`: Totals に `final_passed` / `ungraded` / `grader_failed`。`passed` は実行器 passed のまま（summary.json との突き合わせ用）。比較表（pairs）の成功も採点結果から数える。`CheckReport.complete` / `incomplete_reasons`。
- `export.rs`: pass_rate = final_passed / n。未完了は `table.incomplete.*` + `complete:false`、最終版は書かず古い最終版を消す。`real_performance` は complete のときだけ true。
- `report.rs`: 報告の `passed=` は final_passed と突き合わせ、未完了 campaign は最終報告にしない。
- runbook: grade の合否と grade-out の処理成功（exit 0）の区別、結合規則を追記。

## 証拠
- 試験: `tests/grade_binding.rs`（実行器 passed + 不合格 grade で最終 JSON/CSV の成功 0／未採点は成功でも除外でもなく最終版を出さない／別 case・split・campaign・古い hash・未知 trial・重複・grader hash 欠落を拒否／分母の恒等式）。既存試験は採点を付ける（`tests/common`）か incomplete 出力に合わせた。
- 最終の `bash scripts/dev/test-parallel.sh` / clippy / fmt の結果は報告本文に記載。

## 状態
- 実装済み: 上記。
- mock 検証済み: 上記試験（pilot fixture の mock 実行 + 一時 dir）。
- 実経路検証済み: なし（実 LLM・実 Celeris は未使用）。
- 承認待ち: なし（本測定の開始・予算増額は本タスクの対象外）。

## 未解決・提案
- grade-case は本測定の K1〜K4 分が未作成（run-final.md 手順 5）。無いと該当 cell は未採点 → incomplete になる（意図どおり）。
- 実行器 failed でも採点は走る（grades.jsonl に残る）が、成功には数えない。
