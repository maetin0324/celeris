---
title: main-4 の未確定 pin と測定環境を解消し、実 pilot 開始条件を揃える（IEEE Access 準備 P0-2）
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
status: blocked
updated: 2026-10-25
---
# main-4 の未確定 pin と測定環境を解消し、実 pilot 開始条件を揃える（IEEE Access 準備 P0-2）

更新日: 2026-10-25。状態: blocked（measurement_ready=false、実測定は開始不可）。
WorkUnit ごとの記録は [2026-10-25-main4-pilot-readiness/](2026-10-25-main4-pilot-readiness/) にある。
この file は task 全体の現在地だけを書く。基準は `git merge-base HEAD main`（WU では `$CELERIS_WU_BASE`）。

## WorkUnit の記録
- [ab-contract](2026-10-25-main4-pilot-readiness/ab-contract.md): A0/B0（Jev 無効）の主比較契約と費用区分。
- [pin-audit](2026-10-25-main4-pilot-readiness/pin-audit.md): freeze-status の pending 14 件の現物照合（pin-reconcile.json）。
- [fix-hash-selftest](2026-10-25-main4-pilot-readiness/fix-hash-selftest.md): 進捗 hash 自己試験の flake 修正。
- [env-nollm](2026-10-25-main4-pilot-readiness/env-nollm.md): 測定専用環境と無評価 LLM の通し（nollm-pass.json）。
- [grade-integrate](2026-10-25-main4-pilot-readiness/grade-integrate.md): checker・export・report の件数と分母の照合（consistency.json）。
- [final-freeze](2026-10-25-main4-pilot-readiness/final-freeze.md): P0-1 の K4 v2 を取り込んだ新 revision と測定 ID。

## 最終 freeze（final-freeze WU）の要点
- 新 revision: `/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready/bundle/final-freeze-1`。
  旧 main-4 bundle（judged-scope-v2、campaign_id mlsys-main-4-v2）・旧 main-3 raw・採点は変えていない。
- 測定 ID: campaign_id = measurement_id = `mlsys-main-4-v3`。旧 v2 の試行記録とは混ぜない。
- P0-1（01M5SRJMA9CFNG2C9KCCS5VS4B）の採用版 K4 v2（case・materials・grader・private・入力 tgz）を取り込み、
  sha256 が P0-1 の adoption.json と全項目一致した。
- 結び付けの記録: `/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready/freeze-report.json`
  （measurement_id、bindings、check_main_pins、measurement_ready、blocked）。
- `python3 scripts/campaign/check-main-pins.py --root <新 revision> --pins <新 revision>/pinned/main/pins.json`
  → exit 1（`NG pins.json: RELEASE: pending`）。よって measurement_ready=false。

## 未解決事項（freeze-report.json の blocked、詳細は final-freeze.md）
1. release（人）: 本 task と P0-1 を含む release を昇格し、その sha12 を pin する（d-release=post-merge）。
2. a-account-login（人）: claude_max_lab への login。
3. claude-oauth-catalog（人）: OAuth token 更新後に env-lock を作る。
4. sirius-controlmaster（人）: ControlMaster を張り直し cap-probe を採り直す。
5. real-preflight（1〜4 の後）: 実構成 preflight exit 0 と新 DB での project_id 作り直し。
6. shared-ledger（人）: 承認の出典・残枠の記録と ledger 初期化。
7. repo-src-submodule-commits（人）: K2 BenchFS submodule の commit object。
8. consistency-rerun（5 の後）: 新 revision の out で件数照合をやり直す。

## 提案
- check-main-pins の lock 規則（measurement_id = campaign_id）と P0-1 の `<campaign_id>-<sha12>` 形式を統一する。
