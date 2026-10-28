---
title: 行為者と消費の通し試験の土台と流れ1
tasks: [01M60AKVZEB61PRBKX5AX44H3A]
status: running
updated: 2026-10-28
---
# 行為者と消費の通し試験の土台と流れ1

## 変更

- `phase 1: 行為者と消費を評価出力まで照合する受け入れ試験を追加` — 5つの流れの入口と照合規則をADRで先に定義し、共有helperと流れ1を追加。
- 一時DB、固定UTC、既存の偽HTTPを使い、認可済みqueueのdecisionを案件CoSが回答する。decisionの計画・transaction適用、CoS操作監査、triage、chat run消費、decision log投影を通す。
- decision log export・human-ops report・eval requestsで要求・操作・主体を照合。人の介入0件、案件CoS操作1件、消費のthread・案件・token・費用・壁時間と共有小計を確認する。人の総労働時間が0とは主張しない。
- 製品側の時計注入の不足を小さく修正。event追記・decision transaction・CoS操作保存に `_at` を追加し、従来入口は同じ実装へ壁時計を渡す。イベントを更新・削除しない。ADR付記に記載。
- 試験境界はドメイン処理から評価出力まで。HTTP認証と実LLM harness起動は対象外。流れ2〜5は後続WU。

## 証拠（基準は $CELERIS_WU_BASE）

- `cargo test -p celeris --test actor_trace_flow1` → 1 passed（初回の生記録fixture修正後）。最終照合追加後の検査を実行中。
- `bash scripts/dev/test-affected.sh --list` → celerisを含む逆依存13 crateとweb型検査・unitが対象。
- `git diff --check` → exit 0。

## 未解決

必須検査を実行中。

## 提案

なし。残り4流れは既存の計画どおり後続WUで実装する。

## 本番への移行

設定・DB migration・本番操作は不要。
