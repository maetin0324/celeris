---
title: main-4 の未確定 pin と測定環境を解消し、実 pilot 開始条件を揃える（IEEE Access 準備 P0-2）
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
status: blocked
updated: 2026-10-25
---
# main-4 の未確定 pin と測定環境を解消し、実 pilot 開始条件を揃える（IEEE Access 準備 P0-2）

更新日: 2026-10-25（closeout WU）。状態: blocked。measurement_ready=false で、実測定はまだ開始できない。
WorkUnit ごとの記録は [2026-10-25-main4-pilot-readiness/](2026-10-25-main4-pilot-readiness/) にある。この file はその集約。
基準は `git merge-base HEAD main`（WU では `$CELERIS_WU_BASE`）。新しい実装はこの WU ではしていない。

証跡の置き場（以下 E=`/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready`）:
`E/pin-reconcile.json`（pin 照合）、`E/nollm-pass.json`（段・pin 解消・人の手順）、`E/consistency.json`（件数照合）、
`E/freeze-report.json`（最終 freeze の結び付け）、測定 root `E/measure`、新 revision `E/bundle/final-freeze-1`（以下 R）。

## WorkUnit の記録
- [pin-audit](2026-10-25-main4-pilot-readiness/pin-audit.md): freeze-status の pending 14 件の現物照合。
- [ab-contract](2026-10-25-main4-pilot-readiness/ab-contract.md): A0/B0（Jev 無効）の主比較契約と費用区分。
- [fix-hash-selftest](2026-10-25-main4-pilot-readiness/fix-hash-selftest.md): 進捗 hash 自己試験の flake 修正。
- [env-nollm](2026-10-25-main4-pilot-readiness/env-nollm.md): 測定専用環境と、無評価 LLM での通し。
- [grade-integrate](2026-10-25-main4-pilot-readiness/grade-integrate.md): checker・export・report の件数と分母の照合。
- [final-freeze](2026-10-25-main4-pilot-readiness/final-freeze.md): P0-1 の K4 v2 を取り込んだ新 revision と測定 ID。
- [pilot-ticket](2026-10-25-main4-pilot-readiness/pilot-ticket.md): K4 A0→B0、通過後に K2 A0/B0 の pilot 実行票。

## 受入条件 1: pending 14 件の現物照合と解消
- pin-audit: 14 key すべてに status・evidence・next を付けた（`E/pin-reconcile.json`、verify.py exit 0）。
  版の差: 稼働 health は release b7b9c3b0d0ff・schema 93。10/24 の GitHub main（ユーザー観測）から release までの差は 95 件。
  release から監査時 main までの差は 20 件。全件を `versions` に記録した。
- env-nollm と final-freeze で解消した項目: A_VERSION（2.1.303 (Claude Code)）、A_PROGRAM、A_PROGRAM_SHA（実 file と一致）、
  CAMPAIGN_BIN_SHA（HEAD から作り直した celeris-campaign。R の pins.json に記入）、daemon.toml・canaries.json（config-check exit 0）。
- 未解消（推測値で埋めていない）:
  - RELEASE（人）: 未昇格のため R の pins.json では空。
  - launcher.env: RELEASE に依存する。
  - env-lock.json・cap-env.json: OAuth の期限切れで model catalog が空。lock exit 1、--verify exit 1。
  - cap-probe/preflight の証跡: sirius の ControlMaster socket が dangling。A の login も無い。
  - K2 repo-src: 主 tree と submodule 5 件の content hash は一致した。submodule の commit object が無い。
  - task-template.project_id: 新しい測定 DB で作り直すまでは仮置き。
  - shared-ledger（人）: 承認の出典・残枠が無く、ledger dir も未作成。
- closeout での再確認（提出時の HEAD）: `python3 scripts/campaign/check-main-pins.py --root R --pins R/pinned/main/pins.json`
  → exit 1（`NG pins.json: RELEASE: pending: 人が記入`、1 件）。health の release は b7b9c3b0d0ff のまま。
  ControlMaster socket も alive でない。
- 判定: 未達。測定に必須の pending が残っている。

## 受入条件 2: 一つの測定 ID への結び付けと P0-1 の取り込み
- 測定 ID: campaign_id = measurement_id = `mlsys-main-4-v3`。旧 `mlsys-main-4-v2`（pilot1・nollm1）とは混ぜない。
  campaign_sha256 は be513f522ec2e6441f31b843cfda25ae626095d40ffc2f9f23df5bae98c5f3a2。
  RELEASE を昇格した後に B の harness_version を書き換えると、この値は変わる。
- P0-1（01M5SRJMA9CFNG2C9KCCS5VS4B）の取り込み: 採用版 K4 v2 を R に取り込んだ（case.json・materials・grade-case・private・`main-v3-n2-43706-public2.tgz`）。
  sha256 は adoption.json の全項目と一致した。manifest の `snapshot.materials_sha256` と `fixed.initial_info` は、新しい materials の木の hash（値は final-freeze.md と freeze-report.json）に揃えた
  （ab-contract U6 と grade-integrate の拒否原因を解消）。
- 結び付けは `E/freeze-report.json` の `bindings` に記録した: 研究 source、celeris・celeris-campaign の sha256、case/grader、入力、manifest、harness/model。
  validate・dry-run・stage-grader-inputs・lock・config-check は exit 0。
- 判定: P0-1 の取り込みは達成。freeze 自体は RELEASE が pending なので未完了（check-main-pins exit 1）。

## 受入条件 3: 測定専用環境での全段の通し（各段の exit と ID）
測定 DB・port 7741・state・token は本番と分けた。paid_calls_made = 0。

実構成の段（実 daemon。fixture は使っていない）:

| 段 | exit | ID |
|---|---|---|
| prepare | 0 | run 01M5T5ZB64ZETYJE1JDJKFDVJR、measurement mlsys-main-4-v2-pilot1 |
| start | 0（候補写しの env。正確な env では 1） | daemon instance 01M5T6B5BQBN120MA2BXNYQE75、project 01M5T6BJ5Y23SXE2F7RT4JJ47N |
| cap-probe | 1 | probe task 01M5T6BZP4RF1NJSGGEE75RV29（B/K2）、01M5T6C5Q446CGA7011MP3CDZB（B/K4） |
| preflight | **1** | run 01M5T65F1ZBC8VTRYQXXE60CM2（A の capability が不足、a-arm 未ログイン） |
| env-lock | 1（--verify も 1） | 同上 |

stub harness の段（無 LLM。実 daemon binary を `M/nollm/` の写しで動かした。port 7742、measurement mlsys-main-4-v2-nollm1）:

| 段 | exit | 要点 |
|---|---|---|
| nollm prepare・start・preflight | 0 | `M/nollm/logs/` |
| watch | 0 | B/K2 task 01M5T7T04PHDDEW157CMCFWK3J が settled |
| collect | 1 | not_submitted（stub は job を出さない） |
| grade | 1 | 3 件を採点。最後の check で 1 |
| check・export・final-accounting | 1 | 旧 manifest の initial_info が pin に違反（final-freeze で直したが、新 revision で再通しはしていない） |
| human-op-report | 0 | human-ops.jsonl は空 |

- fixture/stub と実 daemon の確認を分けて記録した。実構成の preflight は非 0 なので、ready とはしていない（`E/nollm-pass.json` の `ready_basis`）。
- 判定: 未達。実構成の preflight exit 0 と、新 revision（mlsys-main-4-v3）での通しが残っている。

## 受入条件 4: checker・export・report の一致と比較契約
- 同じ nollm out で、checker・human-op report・生記録の再計算を照合した（`E/consistency.json`、`consistent: true`、mismatches 空）。

| arm | n | validated success | ran | tokens 欠測 (checker / report) | 未実行 (report) |
|---|---|---|---|---|---|
| A | 2 | 0 | 2 | 0 / 0 | 0 |
| B | 2 | 0 | 1 | 1 / 1 | 1 |

- ずれを 2 件直した（grade-integrate）:
  - report が未実行 cell を欠測に数えていた。分母を ran_trials に揃えた。
  - export が records 無しのとき消費を黙って落としていた。今は欠測と明記する。
- 欠測は null のまま数え、観測済み 0 にしていない。
- export・final-accounting の表は旧 out では pin 違反で拒否されるので、まだ照合していない（新 revision の out で再照合する。残件 8）。
- 比較契約: `docs/ops/mlsys-main-4-ab-contract.md` に次を書いた（ab-contract）。
  - A0・B0・Jev 無効の差の表。
  - token の 4 欄。
  - 費用の 4 区分（定価換算・実支出・harness 報告額・quota）と、CoS/共有消費の範囲。
  - 未整合 U1〜U11。U1（A の定価換算が無い）と U3（A の subagent 消費）は、費用比較の前に実装が要る。
- 判定: 契約の文書化と、旧 out での件数一致は達成。export・final-accounting の表の一致は未確認。

## 受入条件 5: pilot 実行票と開始可否
- `docs/ops/mlsys-main-4-pilot-runbook.md` に次を書いた（pilot-ticket）。
  - 順序は K4 A0→B0、通過後に K2 A0/B0。
  - 構成と hash（freeze-report から写した）、停止条件、出力先。
  - 共有台帳 `/local/celeris/data/measure/main-4/global-ledger`（未初期化）。
  - 既承認枠と残枠: 承認記録が無いため「未取得」。参考として main-2・main-3 の過去消費を載せた。
  - 人操作の記録方法。
- 開始可否: **開始不可**（条件は C1〜C8）。現 manifest は K2 を先に計画し、approved_trials=2 のため K4 に届かない。
  K4 先行には問題ごとの pilot manifest と lock の作り直しが要る（C7）。
- 主測定 12/20 試行と追加予算は実行していない。pilot の LLM 試行もしていない（人の指示）。

## 受入条件 6: done の必須条件 (a)〜(d)
- (a) P0-1 の採用資産・hash の取り込み: 達成（受入条件 2）。
- (b) 測定に必須の pending の解消: 未達（RELEASE・env-lock/cap-env・cap-probe/preflight・repo-src・project_id・shared-ledger）。
- (c) 実測定構成の preflight exit 0: 未達（exit 1）。
- (d) 採点・集計の通し証跡: 一部のみ。旧 out で checker と report は一致した。新 revision で export・final-accounting を含めた通しは未実施。
- よって合格・done にせず、blocked で待機する。人の操作が済むまで、worker が進められる手は無い。

## 受入条件 7: blocked 事項と、目標日 10/29 に対する残件
`E/freeze-report.json` の `blocked`。手順は `E/nollm-pass.json` の `blocked[].steps` にある。
1. release（人）: 本 task を統合した後、release gate と昇格を行う（d-release=post-merge）。
   その sha12 を pins・launcher.env・manifest B に書き、lock を作り直す。P0-1 だけを含む release は pin しない。
2. a-account-login（人）: claude_max_lab に login する。
3. claude-oauth-catalog（人）: OAuth token を更新する。そのあと env-lock と --verify を行う。
4. sirius-controlmaster（人）: TOTP で ControlMaster を張り直し、cap-probe（A/B）を採り直す。
5. real-preflight（1〜4 の後、worker）: 実構成の preflight exit 0 を取り、新しい測定 DB で project_id を作り直す。
6. shared-ledger（人）: 承認の出典・範囲・期限・消費・残枠を記録し、ledger を初期化する。approved_trials=2 は 4 trial に足りない。
7. repo-src-submodule-commits（人）: K2 BenchFS の submodule 5 件の commit object を含む repo か bundle を用意する。
8. consistency-rerun（5 の後、worker）: mlsys-main-4-v3 の out で、checker・export・report・final-accounting の照合をやり直す。
9. pilot manifest（worker、実行票 C7）: K4 だけ・K2 だけの manifest を同じ shared_ledger で作り、lock する。

目標日 10/29 に対する見通し: P0-1 の取り込みは済んでいる（最終 freeze は RELEASE 待ち）。
1〜4・6・7 は人の操作が要る。10/27 時点で 1〜4 が済んでいなければ、5・8・9 の worker 作業（半日程度）を考えると 10/29 は厳しい。

## 検査（提出時の HEAD で実行）
closeout WU の検査結果は WU の result に記録する。この WU の変更はこの file だけ。

## commit（件名）
- `phase 1: reconcile main-4 pending pins and runtime versions`
- `phase 1: main-4 の A0/B0 主比較契約と費用区分を文書化`
- `progress: ab-contract の検査結果を記録`
- `fix: progress_index_commit_hashes の短縮 hash を検出条件を満たす長さまで伸ばす`
- `campaign: A の cap-probe 失敗に bind できない cap env の元（socket・mount）を添える`
- `progress: env-nollm の段・pin 解消表と人の手順をまとめる`
- `campaign: 同じ out で checker・export・human-op report の件数と分母を照合し、未実行の消費と records 欠如を明記する`
- `progress: grade-integrate の content hash を件名検査に掛からない書き方にする`
- `progress: grade-integrate の検査結果を記録する`
- `progress: final-freeze で main-4 の新 revision（mlsys-main-4-v3、K4 v2 取り込み）を固定し、未昇格 release 等の残件を記録する`
- `progress: main-4 pilot readiness の task 全体の進捗 file を置く`
- `docs: main-4 pilot 実行票（K4 A0→B0、通過後に K2 A0/B0）を置く`

## 提案
- check-main-pins の lock 規則（measurement_id = campaign_id）と、P0-1 の `<campaign_id>-<sha12>` 形式を統一する。
- initial_info の不一致は preflight の段で先に検出させる（今は watch の後で初めて分かる）。
- launcher に records の段（測定 DB の read-only export）を足し、export で RECORDS を必須にする（運用手順が変わるので人が決める）。
- pilot 用 manifest を問題ごとに分ける手順を、freeze の段の標準にする。
