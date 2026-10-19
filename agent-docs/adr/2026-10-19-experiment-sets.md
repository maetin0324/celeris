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

## 付記: 準備系（preflight / lock / env-lock）の有効 arm 化（arm-set-prep）
- preflight: 候補 config は有効 arm だけを要求する。A の実行ファイル・bwrap・`--a-account`・A の能力 probe・trial-state は
  A が有効なときだけ。B/C のみの比較は A の account/環境を要求せず A を呼ばない。B/C の probe は `m.configs`（有効 arm ちょうど）
  だけを回すので、A/B のみでは C の probe は 0 件（C の credential・Jev 鍵も要らない）。
- campaign lock: `ExecutionPin` は有効 arm の config だけを固定し、`sets_sha256`（宣言＋選択の hash）を持つ。宣言なしは空で
  直列化しない（既存 lock の hash は変わらない）。`bind_to_manifest` が set hash の食い違いも拒否する。
- env-lock: `a` は A が有効なときだけ（`Option`、無ければ欄ごと出さない）、`bc.arms` は有効な B/C のみ、`sets_sha256` を持つ。
  CLI `env-lock` は A 無効なら `--a-program` を解決しない。schema 版（celeris-env-lock/3）と CAP_ENV の固定値、arm ID は変えない。

## 付記（arm-set-exec: 実行系の統一と全体台帳の接続）
実装突き合わせ（`crates/campaign/`）:
- D7 実行・照合・表は同じ計画: `runner::plan`・`checker::expected`・`aggregate`・`dryrun`・`final_accounting` の計画 arm・
  `runner` の summary は `active_arms()` / `planned_configs()` だけを使う。未使用 arm は期待試行・分母・費用・表・report の行に出ない
  （`aggregate` と `report` は有効計画に無い arm の記録・result 行を拒否）。`export` の `comparisons` は `compare_pairs()` のみ
  （名前は従来どおり (A,B)=`primary`、(B,C)=`secondary`）。`usage` は A が有効計画（summary の cells に A）のときだけ A アカウントを要求。
- D8 実 run の照合（宣言があるときだけ）: `checker::reconcile_sets`。(A,B) 比較のセットでは B の全 run（worker/reviewer/planner/cos）が
  実測で Sonnet でなければ失敗（model 未観測も失敗）。(B,C) 比較のセットでは B/C の実測 model・harness が共通候補・共通 run_harnesses の
  内側か別に検査する。B0 と B1/C1 は別メッセージ。case/repetition/retry は `(config, problem, rep, trial_no)` で区別（従来どおり）。
- D9 resume: run-meta に `plan_identity` を保存し、同じ測定 ID で有効 arm・比較組・set 宣言・有効設定が変わった再開は
  `check_resume` が変えた欄を名指しして拒否する（hash 不一致の汎用拒否より先）。
- D10 全体台帳: `global_ledger.rs`（`--global-ledger <dir>` / `RunOptions.global_ledger`、追記専用 `global-ledger.jsonl`）。
  測定 ID ごとに最後の行が現在値（確定消費+予約・wall・実行試行数・provider 枠）で、再実行・retry の再集計は上書きなので二重加算しない。
  run 開始時に `admit_start` が他の測定 ID の既消費を manifest の `budget.max_tokens` から引き、`check_shares`（セット `allocation`）と
  provider 枠を検査して、超過なら out の作成・投入より前に拒否する。投入ゲート（`budget::admit`）にも他測定 ID の既消費を加える。
  試行数の全体上限は manifest に無いので trials は記録のみ（上限なし）。単独測定（global_ledger なし）は従来と同じ。
- 互換: `experiment_sets` が無い manifest は従来の A/B/C 全計画。export の JSON/CSV の列・名前は不変（行が有効 arm だけになる）。
  run-meta の `plan_identity` は追加欄で、無い旧 out の再開は従来の hash 照合のみ。
- 再利用: manifest の有効計画（arm-set-core, 3be21634）、既存の `ledger.rs`/`budget.rs`/`final_accounting`/`checker` を拡張。
  試験は `tests/experiment_sets_exec.rs`（A/B のみ・B/C のみ・resume 拒否・B0 不一致・全体台帳の横断）。

## 付記（ledger-state: 全体台帳の確定消費・未精算予約・承認上限）
- 形式 `celeris.campaign-global-ledger/2`（`crates/campaign/src/global_ledger.rs`）。追記専用 JSONL の 1 行は `kind` =
  `measurement`（測定 ID の確定消費・取り分 `allocation`・上限 `caps`・旧枠 `legacy_slots`）か `attempt`（試行 identity
  = 測定 ID・trial・attempt 番号ごとの `state` Reserved/Submitted/Settled、予約 tokens・provider 枠、精算 usage）。
- 確定消費は単調（`record` は下げない）。占有 = max(取り分, max(公開した確定消費, 精算済み attempt の合計) + 未精算予約)。
  試行数は異なる trial の数（retry は 1 試行）。provider 枠は全 ID の未精算 attempt の合計 + 旧枠（宣言 max_parallel の
  max 集計は廃止）。
- 承認上限は全行（置換された行を含む）と manifest の最小。`reserve` は全行読取 → 上限最小化 → tokens・試行数・
  provider 枠の検査 → Reserved 行の追記と fsync を 1 つの flock で行い、拒否時は何も書かない。
- 遷移 `reserve` → 投入 → `mark_submitted` → 終了確認 → `settle`。Reserved は投入済みかもしれないとみなし、
  `Terminal::NotSubmitted` の解放は Reserved のときだけ。同じ identity の再予約は `Existing`、再精算は各欄の大きい方で
  置換、精算済み identity の再予約は拒否（retry は attempt 番号を変える）。
- 旧 `/1` 行: tokens/trials は全部を確定消費として持ち、cap 欠落は上限を与えず、`provider_slots` は持ち主が
  `confirm_legacy_idle` を呼ぶまで占有。読めない行・不明 schema/kind は拒否、改行で終わらない末尾には追記しない。
- runner の `publish_global` は in-flight の予約を除いた確定消費だけを公開する。毎投入の `reserve` / `settle` への配線は
  runner-admission で行う。
