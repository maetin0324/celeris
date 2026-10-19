---
title: "ADR: 実験セット（有効 arm・比較組）の最小宣言と単一の有効計画"
status: accepted
date: 2026-10-19
tasks: [01M59KXY8345HZEE06FVZ3FWVP]
---
# 実験セット宣言

- 関連: 設定確定 01M58VFRBA6HG0SSF3CG5BKM91（固定値は変えない）、2026-10-16-campaign-budget-reservation

## 文脈
arm ID A/B/C と実行器は保持したまま、A0/B0（主比較）と B1/C1（Jev 副比較）を別の測定 ID で、片方だけ実行したい。
従来の manifest は A/B/C 必須・primary=(A,B)・secondary=(B,C)・subset 非空で、未使用 arm を外せない。

## 決定
- D1 宣言: `experiment_sets`（セット名 → `{arms, compare, fixed}`）と `selected_sets`（今回使うセット。空なら宣言した全セット）。
  `compare` は (A,B) か (B,C) でセット内の arm だけ。`fixed` は方策・モデル候補・harness・初期情報・成功条件・
  `role_models`・`allocation`（全て省略可、宣言した欄だけ検査）。
- D2 互換: `experiment_sets` が無い manifest は従来と同一（A/B/C 全部、primary/secondary 検査も従来どおり、
  新欄は直列化しないので既存 hash は変わらない）。宣言があるとき `primary`/`secondary.pair` は読まない。
- D3 有効計画: 有効 arm=選択セットの和集合、有効比較組=選択セット内の組だけ。`validate`/`planned_configs`/
  `compare_pairs`/`expected_trials` は有効 arm のみを返す。`configs` は有効 arm ちょうど（未使用 arm の config は置かせない、
  欄も要らない）。C は subset のみ。A が無効（B/C のみ）なら B も subset のみ、かつ problems=subset（分母に遊ぶ問題を残さない）。
  C が無効なら subset は空。
- D4 resume: 計画識別 `PlanIdentity`（測定 ID=campaign_id、有効 arm、比較組、set 宣言 hash、有効 arm 設定 hash）を
  `plan_identity()` で作り、`check_resume` が同じ測定 ID での変更を拒否する。set 宣言は manifest の sha256 にも入る。
- D5 配分: 予算・試行数・provider 枠は全体台帳 1 つ。セットの `allocation` は全体の取り分で加算分ではない。
  `check_shares(GlobalCap{total, consumed}, shares)` は 既消費 + 取り分の合計 ≤ 全体（tokens/trials）、provider 枠は
  各取り分 ≤ 全体を要求する（既消費はリセットしない）。manifest 内は expected_trials（共有 arm は 1 回だけ数える）と
  budget.max_tokens/max_parallel を全体とする。
- D6 固定構成の検査は別関数: `check_b0_all_sonnet`/`check_b0_observed`（planner/reviewer/cos/worker 全役割が Sonnet）と
  `check_b1_c1_common`（共通候補 2 件以上・同 harness・同 tier 対応表・根拠表なし、C1 の単一 pin を拒否）。
  final の `check_resolved` が、(A,B) 比較を持つセットに B0 検査、(B,C) 比較を持つセットに B1/C1 検査を呼ぶ。

## 範囲外
runner/checker/export/dry-run/preflight/lock の有効計画への統一は後続 WorkUnit（arm-set-exec / arm-set-prep）。
