---
title: main-4 の未確定 pin と測定環境を解消し、実 pilot 開始条件を揃える（IEEE Access 準備 P0-2）
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
status: done
updated: 2026-10-26
---
# main-4 の未確定 pin と測定環境を解消し、実 pilot 開始条件を揃える（IEEE Access 準備 P0-2）

完了日: 2026-10-26（final-close WU で集約）。done の必須条件 (a)〜(d) はすべて新しい証跡で満たした。
pilot の開始には、本 task の統合後に 1 件の運用条件（collect 修正入り campaign binary の再固定、実行票 §6 C9）が残る。
WorkUnit ごとの記録は [2026-10-25-main4-pilot-readiness/](2026-10-25-main4-pilot-readiness/) にある。この file はその集約。
基準は `git merge-base HEAD main`（WU では `$CELERIS_WU_BASE`）。新しい実装はこの WU ではしていない。

証跡の置き場（以下 E=`/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready`）:
- 新 revision R2 = `E/bundle/final-freeze-2`（measurement_id = campaign_id = `mlsys-main-4-v4`）。旧 R1 = `E/bundle/final-freeze-1`（`mlsys-main-4-v3`）は読むだけ。
- 機械向けの記録 RR = `E/rerun`: `revision.json`・`env-lock.json`・`cap-probe.json`・`ledger.json`・`repo-src.json`・`preflight.json`・
  `pin-reconcile.json`・`check-main-pins.{stdout,exit}`・`nollm-pass.json`・`consistency.json`・`pilot-manifests.json`、作業記録 `RR/notes/<unit>.md`。
- 前回（10/25）の `E/pin-reconcile.json`・`E/nollm-pass.json`・`E/consistency.json` は旧 revision の記録で、判定には使わない。

## WorkUnit の記録
- [pin-audit](2026-10-25-main4-pilot-readiness/pin-audit.md): freeze-status の pending 14 件の現物照合（10/25）。
- [ab-contract](2026-10-25-main4-pilot-readiness/ab-contract.md): A0/B0（Jev 無効）の主比較契約と費用区分。
- [fix-hash-selftest](2026-10-25-main4-pilot-readiness/fix-hash-selftest.md): 進捗 hash 自己試験の flake 修正。
- [env-nollm](2026-10-25-main4-pilot-readiness/env-nollm.md): 測定専用環境と、旧 revision での無評価 LLM の通し。
- [grade-integrate](2026-10-25-main4-pilot-readiness/grade-integrate.md): checker・export・report の件数と分母の照合の仕組み。
- [final-freeze](2026-10-25-main4-pilot-readiness/final-freeze.md): P0-1 の K4 v2 を取り込んだ R1。
- [pilot-ticket](2026-10-25-main4-pilot-readiness/pilot-ticket.md): pilot 実行票。
- [refreeze](2026-10-25-main4-pilot-readiness/refreeze.md): RELEASE 昇格後の R2、14 pin の解消、実 preflight exit 0（10/26）。
- [nollm-rerun](2026-10-25-main4-pilot-readiness/nollm-rerun.md): R2 で無評価 LLM の 12 段通し、照合、pilot manifest の lock（10/26）。

## 前提（人の操作。task comment と計画 v4 の回答）
- RELEASE `a28184889f56` に昇格済み（本 task の前回分と P0-1 を含む。`RR/revision.json` の health・current・promoted.json）。
- `claude_max_lab` に login 済み、claude OAuth の model catalog 14 件を取得（`RR/env-lock.json`）。
- Sirius ControlMaster: `ssh -O check -S /run/celeris-cap/sirius-ssh.sock sirius` → exit 0（`RR/cap-probe.json`）。
- 承認枠: 利用者の 2026-10-26 の記録（ループせず前進する限り試行数の固定上限なし、同じ原因で 2 回続けて失敗したら停止、期限 2026-11-13）。
- K2 submodule 5 件の bundle: `/local/celeris/data/measure/main-4/k2-submodule-bundles/`（SHA256SUMS 一致）。

## 受入条件 1: pending 14 件の現物照合と解消
- 条件: 14 項目を実測定構成で解消し、推測値で埋めない。
- コマンド: `python3 scripts/campaign/check-main-pins.py --root E/bundle/final-freeze-2 --pins E/bundle/final-freeze-2/pinned/main/pins.json`
- 結果: exit 0、`check-main-pins: ok`（`RR/check-main-pins.stdout`・`.exit`）。`RR/pin-reconcile.json` は 14 key すべて resolved（not_required 0）。

| key | 解消の証拠（RR の記録） |
|---|---|
| RELEASE | pins・launcher.env・B の harness_version = `a28184889f56`。preflight の daemon.release が同値 |
| A_VERSION / A_PROGRAM / A_PROGRAM_SHA | `2.1.303 (Claude Code)`、preflight の a_program.path、`sha256sum` が pin と一致 |
| CAMPAIGN_BIN_SHA | `sha256sum` = pin（`RR/check-main-pins` で照合した全 64 桁、実行票 §1.1）。実 preflight の argv.txt が同じ binary（再固定は §受入条件 7） |
| env-lock.json / cap-env.json | launcher lock exit 0、`env-lock --verify` exit 0、cap-probe 4 本 exit 0 |
| daemon.toml / launcher.env / canaries.json | 測定専用 state/v4・port 7741 で start exit 0、実構成 preflight exit 0 |
| K2 repo-src | submodule 5 件の commit を bundle から入れ verify.sh exit 0、lock の再実行が byte 一致（`RR/repo-src.json`） |
| cap-probe/preflight 証跡 | cap-probe A/B×K2/K4 exit 0・paid_calls 0、`R2/pinned/evidence/cap-{a-k2,a-k4,b}.json` |
| task-template.project_id | 測定 DB で作った project `01M5W9014BJF29SS6VQDFX0Z18` |
| shared-ledger | `ledger-init` exit 0。出典・範囲・期限 2026-11-13・承認 40・消費 0・残 40（`RR/ledger.json`） |

- 判定: 達成。

## 受入条件 2: 一つの測定 ID への結び付けと P0-1 の取り込み
- 測定 ID: `mlsys-main-4-v4`（campaign_sha256 `3756ff02244a5d6c8ee4e1c04d5041487d7ebbba9ba762543bc429dfa2e51c65`）。
  旧 `mlsys-main-4-v2`・`-v2-pilot1`・`-v2-nollm1`・`-v3` とは state dir も分けた（`RR/revision.json` の not_mixed_with）。
- R2 は R1 の写しで、pins・launcher.env・daemon.toml・final.json（B の harness_version を `641f334b6d64` から `a28184889f56` へ）だけを替えた。
  validate・dry-run・stage-grader-inputs・lock・config-check はすべて exit 0（`RR/revision.json` の steps）。
- P0-1（01M5SRJMA9CFNG2C9KCCS5VS4B）の K4 v2 の 6 項目（archive・case_json・materials・kb_snapshot・grade_case・private）を disk から
  hash し直し、adoption.json と全一致（`k4_adoption_match = true`）。
- 版の差（GitHub/main/稼働版）は pin-audit の `versions` に記録済み。稼働版は b7b9c3b0d0ff から a28184889f56 に進んだ。
- 判定: 達成。

## 受入条件 3: 測定専用環境での全段の通し（各段の exit と ID）
実構成（実 daemon・本物の A program・Sirius。fixture なし）、measurement `mlsys-main-4-v4`、state/v4、port 7741:

| 段 | exit | ID・要点 |
|---|---|---|
| cap-probe（A/B × K2/K4） | 0 | `RR/cap-probe.json`、paid_calls 0 |
| lock / env-lock --verify | 0 / 0 | `RR/env-lock.json` |
| prepare・start・preflight・stop | 0 | preflight run `01M5WANNTJSBHTQA364VMBC5YB`、ok=true、missing []、checked 17、daemon release `a28184889f56`（`RR/preflight.json`、config=real） |

無評価 LLM（stub harness・fake model、R2 の写し `E/rerun/nollm`、port 7742。`RR/nollm-pass.json` の stages）:

| 段 | exit | real_daemon | 要点 |
|---|---|---|---|
| prepare | 0 | false | run `01M5WDNMPXZ9X8WVYC7FXYZEBC` |
| start | 0 | true | instance `01M5WE18R8VJ01QSJEQHAMBHB7` |
| cap-probe | 0 | true | 4 本 ok |
| preflight | 0 | true | ok=true、missing [] |
| env-lock | 0 | true | `--verify` verified |
| watch | 0 | true | settled。B/k4 は arm 予算で未実行 |
| collect | 0 | false | not_submitted 2、blocking 0（旧 revision では 1） |
| grade | 0 | false | 3 件採点、B/k4 は not run |
| check | 0 | false | launcher と同じ 40 引数、violations [] |
| export | 0 | false | records 75 件、run `01M5WEETW5JK1N7K2M5A4TRY5Z` |
| human-op-report | 0 | false | 4 trial 行 |
| final-accounting | 0 | false | export の写しと同内容 |

- fixture/stub と実 daemon を分けて記録した。実構成の preflight は exit 0 で、無評価 LLM の通しの値を実構成の ready の根拠にしていない。
- stub の通しは collect 修正入りの binary（sha256 `2dd77ed2a28772999878a65bd122450dc9eb1fb3eea6dc0d4c49cb7d39b18e6d`）で行った。pin した binary（`5132230995128dc745593546132dfb947e537d1913718bea482cbc83f5ff93dd`）とは違う（受入条件 7）。
- paid_calls_made = 0。判定: 達成。

## 受入条件 4: checker・export・report の一致と比較契約
- コマンド: `python3 scripts/campaign/check-out-consistency.py --out E/rerun/nollm/out …`（launcher と同じ check 引数）→ exit 0、`consistent: true`、mismatches []。
  final-accounting は `RR/consistency-work/merge.py` で生記録と突き合わせ、全一致。

| arm | n | ran | 未実行 | final_passed | tokens 欠測 |
|---|---|---|---|---|---|
| A | 2 | 2 | 0 | 0 | 0 |
| B | 2 | 1 | 1 | 0 | 1 |

- checker・export・human-op report・final-accounting・生記録で n・ran・成功/失敗・分母が一致した。欠測（費用・wait 区間・未実行 cell）は null のまま数え、0 にしていない。
- 比較契約は `docs/ops/mlsys-main-4-ab-contract.md`（A0/B0・Jev 無効、token 4 欄、費用 4 区分、CoS/共有消費）。
- 判定: 達成。

## 受入条件 5: pilot 実行票と開始可否
- `docs/ops/mlsys-main-4-pilot-runbook.md` を R2・pilot manifest の値で更新した（構成・hash・measurement_id・出力先・台帳の残枠・§6）。
- pilot manifest（`RR/pilot-manifests.json`）: K4 だけ `mlsys-main-4-v4-pilot-k4`、K2 だけ `mlsys-main-4-v4-pilot-k2`。各 2 試行・approved_trials 2・同じ shared_ledger、lock exit 0。
- 開始可否: C1〜C8 は満たした。C9（collect 修正入り binary の再固定）が残るので、現時点は **開始不可**。
- 主測定 12/20 試行・追加予算・pilot の LLM 試行は実行していない。

## 受入条件 6: done の必須条件 (a)〜(d)
- (a) P0-1 の採用資産・hash の取り込み: 達成（受入条件 2。k4_adoption_match=true）。
- (b) 測定に必須の pending の解消: 達成（14/14 resolved、check-main-pins exit 0）。
- (c) 実測定構成の preflight exit 0: 達成（run `01M5WANNTJSBHTQA364VMBC5YB`）。
- (d) 採点・集計の通し証跡: 達成（R2 の 12 段すべて exit 0、consistency exit 0・consistent=true）。

## 受入条件 7: 残る運用条件（pilot 開始前）
1. C9（worker、本 task の統合後）: R2 の campaign binary を collect 修正入りに再固定する。
   pin した binary には collect の修正が無い。そのため job を出さない cell（not_submitted）で collect が exit 1 になり、実行票の停止条件 7 に当たる。
   修正（commit `collect-stub: collect-observed-out は not_submitted だけなら記録を書いて exit 0`）は task branch にあり、まだ main に無い。
   手順は nollm-rerun.md の「未解決」と実行票 §6 C9 にある。main で build し、CAMPAIGN_BIN_SHA を更新して check-main-pins を通す。
   そのあと env-lock・preflight・pilot manifest 2 件の lock を取り直す。
2. pilot manifest への check-main-pins（任意）: pilot 用の pins は作っていない。lock の照合は watch の preflight が毎回行う。
3. R2 の主 manifest は approved_trials 2 のまま（実効 2 試行）。pilot は manifest 2 件（各 2 試行）を使うので、この値の影響を受けない。
   主測定の前には承認値に合わせた再 freeze が要る（本 task の範囲外）。
4. 台帳の承認は 40 試行・期限 2026-11-13。同じ原因で 2 回続けて失敗したら止めて人に上げる。

## 検査（提出時の HEAD で実行）
final-close WU の検査結果は WU の result に記録する。この WU の変更はこの file と実行票だけ。

## commit（件名）
前回の提出分（main 取り込み済み）:
- `phase 1: reconcile main-4 pending pins and runtime versions`
- `phase 1: main-4 の A0/B0 主比較契約と費用区分を文書化`
- `fix: progress_index_commit_hashes の短縮 hash を検出条件を満たす長さまで伸ばす`
- `campaign: A の cap-probe 失敗に bind できない cap env の元（socket・mount）を添える`
- `campaign: 同じ out で checker・export・human-op report の件数と分母を照合し、未実行の消費と records 欠如を明記する`
- `progress: final-freeze で main-4 の新 revision（mlsys-main-4-v3、K4 v2 取り込み）を固定し、未昇格 release 等の残件を記録する`
- `docs: main-4 pilot 実行票（K4 A0→B0、通過後に K2 A0/B0）を置く`

今回（10/26）:
- `fix(campaign): check-main-pins の env-lock arm 集合を manifest の B/C config に合わせる`
- `campaign: ledger-init で共有台帳に人の承認（出典・範囲・期限・試行数）を記録する`
- `campaign: ledger-init の決定記録を global_ledger の module doc へ移す（WU 範囲外の ADR file を外す）`
- `refreeze: pin-reconcile と進捗を記録する`
- `collect-stub: collect-observed-out は not_submitted だけなら記録を書いて exit 0`
- `nollm-rerun: 無評価 LLM 全段通し・照合・pilot manifest lock の進捗を記録する`
- `final-close: main-4 集約進捗と pilot 実行票の開始可否を refreeze・nollm-rerun の証跡で更新する`（本 WU）
- `final-close: 集約進捗の binary hash を全桁で書く`（本 WU）

## 提案
- check-main-pins に「manifest の approved_trials ≥ expected_trials」と「CAMPAIGN_BIN が main の campaign crate から作られたか」の検査を足す。
- ledger-init の決定を ADR file（`agent-docs/adr/2026-10-26-global-ledger-approval-init.md`）に起こす（今は module doc）。
- cap-probe など setup 段の消費を shared_overhead として 1 回数える規則を final-accounting に足す（nollm-rerun の提案）。
- check-out-consistency.py に final-accounting の照合を入れる（今は merge.py で別に照合している）。
