---
title: main-4 の未確定 pin と測定環境を解消し、実 pilot 開始条件を揃える（IEEE Access 準備 P0-2）
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
status: done
updated: 2026-10-26
---
# main-4 の未確定 pin と測定環境を解消し、実 pilot 開始条件を揃える（IEEE Access 準備 P0-2）

完了日: 2026-10-26（final-close WU で集約し、pin-close WU で repin・nollm-pinned の証跡に差し替えた）。done の必須条件 (a)〜(d) はすべて満たした。
campaign binary は collect 修正入り（sha256 `2344d2c47b001e66560a778e964d9e5c2f2b9f5108c552016b727f5105b2b402`）に再固定済みで、
実行票 §6 は C1〜C9 すべて済、**判定: 開始可**（運用条件: 承認枠・同じ原因で 2 回連続失敗したら停止・期限 2026-11-13）。
WorkUnit ごとの記録は [2026-10-25-main4-pilot-readiness/](2026-10-25-main4-pilot-readiness/) にある。この file はその集約。
基準は `git merge-base HEAD main`（WU では `$CELERIS_WU_BASE`）。新しい実装はこの WU ではしていない。

証跡の置き場（以下 E=`/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready`）:
- 新 revision R2 = `E/bundle/final-freeze-2`（measurement_id = campaign_id = `mlsys-main-4-v4`）。旧 R1 = `E/bundle/final-freeze-1`（`mlsys-main-4-v3`）は読むだけ。
- 機械向けの記録 RR = `E/rerun`: `revision.json`・`env-lock.json`・`cap-probe.json`・`ledger.json`・`repo-src.json`・`preflight.json`・
  `pin-reconcile.json`・`check-main-pins.{stdout,exit}`・`nollm-pass.json`・`consistency.json`・`pilot-manifests.json`、作業記録 `RR/notes/<unit>.md`。
- repin・nollm-pinned の記録 RP = `E/repin`: `repin.json`（root・measurement_id・新旧 binary sha256・check-main-pins・cap-probe・env-lock・preflight・pilot lock）、
  `revision.json`・`lock-probe.json`・`live.json`・`pilot.json`・`nollm-pass.json`・`consistency.json`、作業記録 `RP/notes/`。判定はこちらを使う。
- RR のうち binary に依る記録（`preflight.json`・`nollm-pass.json`・`consistency.json`・`pilot-manifests.json`・`check-main-pins.*`）は旧 binary（sha256 `5132230995128dc745593546132dfb947e537d1913718bea482cbc83f5ff93dd`）や未 pin の binary の時点の物で、参考として残す。
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
- [nollm-rerun](2026-10-25-main4-pilot-readiness/nollm-rerun.md): R2 で無評価 LLM の 12 段通し、照合、pilot manifest の lock（10/26、未 pin の binary。参考）。
- [repin](2026-10-25-main4-pilot-readiness/repin.md): collect 修正入り campaign binary を R2 に pin し、check-main-pins・cap-probe・env-lock・実 preflight・pilot lock を取り直し（10/26）。
- [nollm-pinned](2026-10-25-main4-pilot-readiness/nollm-pinned.md): pin した binary そのもので無評価 LLM の 12 段通しと 4 者照合（10/26）。

## 前提（人の操作。task comment と計画 v4 の回答）
- RELEASE `a28184889f56` に昇格済み（本 task の前回分と P0-1 を含む。`RR/revision.json` の health・current・promoted.json）。
- `claude_max_lab` に login 済み、claude OAuth の model catalog 14 件を取得（`RR/env-lock.json`）。
- Sirius ControlMaster: `ssh -O check -S /run/celeris-cap/sirius-ssh.sock sirius` → exit 0（`RR/cap-probe.json`）。
- 承認枠: 利用者の 2026-10-26 の記録（ループせず前進する限り試行数の固定上限なし、同じ原因で 2 回続けて失敗したら停止、期限 2026-11-13）。
- K2 submodule 5 件の bundle: `/local/celeris/data/measure/main-4/k2-submodule-bundles/`（SHA256SUMS 一致）。

## 受入条件 1: pending 14 件の現物照合と解消
- 条件: 14 項目を実測定構成で解消し、推測値で埋めない。
- コマンド: `python3 scripts/campaign/check-main-pins.py --root E/bundle/final-freeze-2 --pins E/bundle/final-freeze-2/pinned/main/pins.json`
- 結果: 新 binary の pin 後に exit 0、`check-main-pins: ok`（`RP/repin.json` の check_main_pins_exit、`RP/notes/record-check-main-pins.out`）。旧 binary 時点の `RR/check-main-pins.*` は参考。`RR/pin-reconcile.json` は 14 key すべて resolved（not_required 0）。

| key | 解消の証拠（RR の記録） |
|---|---|
| RELEASE | pins・launcher.env・B の harness_version = `a28184889f56`。preflight の daemon.release が同値 |
| A_VERSION / A_PROGRAM / A_PROGRAM_SHA | `2.1.303 (Claude Code)`、preflight の a_program.path、`sha256sum` が pin と一致 |
| CAMPAIGN_BIN_SHA | collect 修正入りに再固定: `2344d2c47b001e66560a778e964d9e5c2f2b9f5108c552016b727f5105b2b402`（`sha256sum` = pins.json、check-main-pins exit 0、`RP/repin.json`）。旧 `5132230995128dc745593546132dfb947e537d1913718bea482cbc83f5ff93dd` は `.pre-repin` に退避 |
| (再固定後の取り直し) | 新 binary で cap-probe 4 本 exit 0、lock・env-lock --verify exit 0、lock.json・env-lock.json は pin と byte 一致で新 measurement_id 不要（`RP/lock-probe.json`・`RP/live.json`） |
| env-lock.json / cap-env.json | launcher lock exit 0、`env-lock --verify` exit 0、cap-probe 4 本 exit 0 |
| daemon.toml / launcher.env / canaries.json | 測定専用 state/v4・port 7741 で start exit 0、実構成 preflight exit 0 |
| K2 repo-src | submodule 5 件の commit を bundle から入れ verify.sh exit 0、lock の再実行が byte 一致（`RR/repo-src.json`） |
| cap-probe/preflight 証跡 | cap-probe A/B×K2/K4 exit 0・paid_calls 0、`R2/pinned/evidence/cap-{a-k2,a-k4,b}.json` |
| task-template.project_id | 測定 DB で作った project `01M5W9014BJF29SS6VQDFX0Z18` |
| shared-ledger | `ledger-init` exit 0。出典・範囲・期限 2026-11-13・承認 40・消費 0・残 40（`RR/ledger.json`） |

- 判定: 達成。

## 受入条件 2: 一つの測定 ID への結び付けと P0-1 の取り込み
- 測定 ID: `mlsys-main-4-v4`（repin 後も同じ。`RP/repin.json` の needs_new_mid=false。campaign_sha256 `3756ff02244a5d6c8ee4e1c04d5041487d7ebbba9ba762543bc429dfa2e51c65`）。
  旧 `mlsys-main-4-v2`・`-v2-pilot1`・`-v2-nollm1`・`-v3` とは state dir も分けた（`RR/revision.json` の not_mixed_with）。
- R2 は R1 の写しで、pins・launcher.env・daemon.toml・final.json（B の harness_version を `641f334b6d64` から `a28184889f56` へ）だけを替えた。
  validate・dry-run・stage-grader-inputs・lock・config-check はすべて exit 0（`RR/revision.json` の steps）。
- P0-1（01M5SRJMA9CFNG2C9KCCS5VS4B）の K4 v2 の 6 項目（archive・case_json・materials・kb_snapshot・grade_case・private）を disk から
  hash し直し、adoption.json と全一致（`k4_adoption_match = true`）。
- 版の差（GitHub/main/稼働版）は pin-audit の `versions` に記録済み。稼働版は b7b9c3b0d0ff から a28184889f56 に進んだ。
- 判定: 達成。

## 受入条件 3: 測定専用環境での全段の通し（各段の exit と ID）
実構成（実 daemon・本物の A program・Sirius。fixture なし）、pin した binary `2344d2c47b001e66560a778e964d9e5c2f2b9f5108c552016b727f5105b2b402`、
measurement `mlsys-main-4-v4`、state/v4、port 7741（`RP/live.json`・`RP/repin.json`）:

| 段 | exit | ID・要点 |
|---|---|---|
| prepare / start | 0 / 0 | project `01M5W9014BJF29SS6VQDFX0Z18` |
| cap-probe（A/B × K2/K4） | 0 | paid_calls 0 |
| lock / env-lock --verify | 0 / 0 | lock.json・env-lock.json は pin と byte 一致 |
| preflight | 0 | run `01M5WHQT0TVY435VY470C2KNYB`、ok=true、missing なし、checked 17、daemon release `a28184889f56`、config=real |
| stop | 0 | port 7741 listen なし |

無評価 LLM（stub harness・fake model、pin した binary そのもの、R2 の写し `E/repin/nollm`、port 7743。`RP/nollm-pass.json` の stages）。
共通 ID: daemon 段 run `01M5WJZMYGDNW5SEPA7R4J8AQV`、post 段 run `01M5WKNVY44TE0Y48SRMMJESKH`、instance `01M5WK3KF0N0KRMPW70MFQGT8Z`:

| 段 | exit | real_daemon | 要点 |
|---|---|---|---|
| prepare | 0 | false | — |
| start | 0 | true | project `01M5WK3V96X5EJW82DHCF3C4ZP` |
| cap-probe | 0 | true | 4 本 ok |
| preflight | 0 | true | ok=true、missing なし |
| env-lock | 0 | true | `--verify` verified |
| watch | 0 | true | 最後の run `01M5WK8MVZ7XAWF884XGE3ZECN`。B/k4 は arm 予算で未実行 |
| collect | 0 | false | not_submitted 2、blocking 0（collect 修正が pin した binary で効いている） |
| grade | 0 | false | B/k4 は not run |
| check | 0 | false | violations なし、ungraded 0 |
| export | 0 | false | lock_verified true |
| human-op-report | 0 | false | trials 4、共有 setup 1 回計上 |
| final-accounting | 0 | false | export の final-accounting.json と一致 |

- fixture/stub と実 daemon を分けて記録した。実構成の preflight は exit 0 で、無評価 LLM の通しの値を実構成の ready の根拠にしていない。
- 参考: 旧 binary 時点の実 preflight（run `01M5WANNTJSBHTQA364VMBC5YB`、`RR/preflight.json`）と、未 pin の binary（sha256 `2dd77ed2a28772999878a65bd122450dc9eb1fb3eea6dc0d4c49cb7d39b18e6d`）による通し（`RR/nollm-pass.json`）は判定に使わない。
- paid_calls_made = 0。判定: 達成。

## 受入条件 4: checker・export・report の一致と比較契約
- コマンド: `python3 scripts/campaign/check-out-consistency.py --out E/repin/nollm/out --campaign-bin <pin した binary> --final-accounting … --check-arg=…`（launcher と同じ check 引数。全文は `RP/consistency.json` の cmd）→ exit 0、`consistent: true`、mismatches []、campaign_bin_matches_pin true。
  checker・export・human-op report・final-accounting の 4 つとも produced で、final-accounting も同じ script で照合した。旧 binary 時点の `RR/consistency.json` は参考。

| arm | n | ran | 未実行 | final_passed | tokens 欠測 |
|---|---|---|---|---|---|
| A | 2 | 2 | 0 | 0 | 0 |
| B | 2 | 1 | 1 | 0 | 1 |

- checker・export・human-op report・final-accounting・生記録で n・ran・成功/失敗・分母が一致した。欠測（費用・wait 区間・未実行 cell）は null のまま数え、0 にしていない。
- 比較契約は `docs/ops/mlsys-main-4-ab-contract.md`（A0/B0・Jev 無効、token 4 欄、費用 4 区分、CoS/共有消費）。
- 判定: 達成。

## 受入条件 5: pilot 実行票と開始可否
- `docs/ops/mlsys-main-4-pilot-runbook.md` を R2・pilot manifest の値で更新した（構成・hash・measurement_id・出力先・台帳の残枠・§6）。
- 実行票 §1.1 の campaign binary を新 sha256 `2344d2c47b001e66560a778e964d9e5c2f2b9f5108c552016b727f5105b2b402` に、root・measurement_id・pilot manifest の lock 値を `RP/repin.json`・`RP/pilot.json` の値にした（pilot の `$K4`・`$K2` は `E/repin/pilot/k{4,2}`）。
- pilot manifest（`RP/pilot.json`）: K4 だけ `mlsys-main-4-v4-pilot-k4`（sha256 `472594e3ceca7fc79a44d455347a41a755398f4f7a0f0ee67c668d99c2c505de`）、K2 だけ `mlsys-main-4-v4-pilot-k2`（sha256 `0bd0db33718536c32f0e2665e6bbf4bb20953793b88002e550718952221654c7`）。各 2 試行・approved_trials 2・同じ shared_ledger、新 binary で lock exit 0。
- 開始可否: C1〜C9 すべて済、**判定: 開始可**（運用条件: 承認枠、同じ原因で 2 回連続失敗で停止、期限 2026-11-13）。
- 主測定 12/20 試行・追加予算・pilot の LLM 試行は実行していない。

## 受入条件 6: done の必須条件 (a)〜(d)
- (a) P0-1 の採用資産・hash の取り込み: 達成（受入条件 2。k4_adoption_match=true）。
- (b) 測定に必須の pending の解消: 達成（14/14 resolved、CAMPAIGN_BIN_SHA は collect 修正入りに再固定し check-main-pins exit 0）。
- (c) 実測定構成の preflight exit 0: 達成（pin した binary で run `01M5WHQT0TVY435VY470C2KNYB`）。
- (d) 採点・集計の通し証跡: 達成（pin した binary そのもので 12 段すべて exit 0、consistency exit 0・consistent=true・mismatches []）。

## 受入条件 7: 残る運用条件（pilot 開始前）
1. C9: 解消済み（repin・nollm-pinned）。R2 の campaign binary を collect 修正入り（sha256 `2344d2c47b001e66560a778e964d9e5c2f2b9f5108c552016b727f5105b2b402`）に再固定し、
   check-main-pins・cap-probe・env-lock --verify・実 preflight（run `01M5WHQT0TVY435VY470C2KNYB`）・pilot lock 2 件を exit 0 で取り直した。新 measurement_id は不要。
   同じ binary の無評価 LLM 通しで collect は exit 0（not_submitted 2・blocking 0）。
   残る注意: この binary は task branch の HEAD（件名 `collect-stub: collect-observed-out は not_submitted だけなら記録を書いて exit 0` の commit を含む）から build した物で、
   collect 修正はまだ main・release gate を通っていない。別 build の binary に差し替えるときだけ repin と無評価 LLM 通しをやり直す。
2. pilot manifest への check-main-pins（任意）: pilot 用の pins は作っていない。lock の照合は watch の preflight が毎回行う。
3. R2 の主 manifest は approved_trials 2 のまま（実効 2 試行）。pilot は manifest 2 件（各 2 試行）を使うので、この値の影響を受けない。
   主測定の前には承認値に合わせた再 freeze が要る（本 task の範囲外）。
4. 台帳の承認は 40 試行・期限 2026-11-13。同じ原因で 2 回続けて失敗したら止めて人に上げる。

## 検査（提出時の HEAD で実行）
pin-close WU の検査結果は WU の result に記録する。この WU の変更はこの file と実行票だけ（新しい実装なし）。

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
- `final-close: main-4 集約進捗と pilot 実行票の開始可否を refreeze・nollm-rerun の証跡で更新する`
- `final-close: 集約進捗の binary hash を全桁で書く`
- `repin: repin.json と進捗を記録する`
- `consistency: check-out-consistency.py に final-accounting の照合を足し、共有 human-op を identity で判定する`
- `phase 1: restore consistency scope and record pinned recheck`
- `nollm-pinned: 12 段通しと照合の進捗を記録する`
- `pin-close: 集約進捗と pilot 実行票を repin・nollm-pinned の証跡で更新し開始可にする`（本 WU）

## 提案
- check-main-pins に「manifest の approved_trials ≥ expected_trials」と「CAMPAIGN_BIN が main の campaign crate から作られたか」の検査を足す。
- ledger-init の決定を ADR file（`agent-docs/adr/2026-10-26-global-ledger-approval-init.md`）に起こす（今は module doc）。
- cap-probe など setup 段の消費を shared_overhead として 1 回数える規則を final-accounting に足す（nollm-rerun の提案）。
- check-main-pins に「argv.txt の binary の sha256 = CAMPAIGN_BIN_SHA」の照合を足す（repin の提案）。
- cap-probe・preflight の probe task に campaign tag を付け、final-accounting の shared 行に計上する（nollm-pinned の提案）。
