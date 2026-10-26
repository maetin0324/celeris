---
title: main-4 pilot readiness — P0-1 の採用版を取り込んだ最終 freeze（新 revision と測定 ID）
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
status: blocked
updated: 2026-10-25
---
# main-4 pilot readiness — P0-1 の採用版を取り込んだ最終 freeze（新 revision と測定 ID）

更新日: 2026-10-25。状態: blocked（measurement_ready=false）。新 revision は作り、P0-1 の K4 v2 は取り込み済み。
`check-main-pins` は exit 1。原因は RELEASE が pending（d-release=post-merge の昇格がまだ）。ほかの未解消項目は下の「未解決事項」。

- 新 revision: `/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready/bundle/final-freeze-1`（以下 R）。
  旧 `.../01M5KVHA4222TK6VEKW599SNMA/artifacts/mlsys-main-4/revisions/judged-scope-v2` は写しただけで変えていない（旧 lock の sha256 は freeze-report に記録）。
- 報告: `/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready/freeze-report.json`（measurement_id・bindings・check_main_pins・measurement_ready・blocked）。
  中間物・検査出力: 同じ dir の `final-freeze/`。
- 基準: `$CELERIS_WU_BASE`（main を祖先に持つ）。研究 source は提出時の HEAD。main は P0-1 の commit（件名「campaign: K4 case を公開入力 v2（main-v3-n2-43706-public2.tgz）に揃え、数値誤りの独立負例を足す」）を含む。

## 測定 ID
- campaign_id = measurement_id = `mlsys-main-4-v3`。check-main-pins の規則（lock.json の measurement_id = campaign_id）に従う。
  旧 `mlsys-main-4-v2` の記録（pilot1・nollm1・P0-1 の nollm lock）と混ざらないよう、新しい id にした。P0-3 の形式どおり campaign_id + measurement_id に、
  source・binary・case・grader・input・env の完全な sha256 を結んでいる。
- campaign_sha256 `be513f522ec2e6441f31b843cfda25ae626095d40ffc2f9f23df5bae98c5f3a2`（validate の値。RELEASE 昇格後に B harness_version を書き換えると変わる）。

## 旧 revision からの変更
- K4: case.json・materials/task.md・grade-case・private.json は repo（P0-1 の採用版）から、tgz は P0-1 の `k4-inputs-v2/main-v3-n2-43706-public2.tgz` から取った。
  sha256 が adoption.json と全て一致した（archive・case_json・materials_task_md・kb_snapshot_md・grade_case・private）。v1 の tgz は R から外した。
- manifest: `snapshot.materials_sha256` を新しい materials の木の hash 810b9562a7b28e736dbe0ce7a7ee9079a6f79df245f2c9f9092a747bc7b14593 にした。旧 ae9007f5f32c1ea4230001ca30c7e47fc6ea1cf51e142ba2e921dbb613af32d4 は v1 の値。
  `experiment_sets.main.fixed.initial_info` も 810b9562a7b28e736dbe0ce7a7ee9079a6f79df245f2c9f9092a747bc7b14593 にした。旧 dab3ca05d01adf1da268c8ec35e24f33249ca3cc02083e0a2e6b487d0c5a800d は main-1 から残っていた値で、grade-integrate では check・export・final-accounting がこれで拒否していた。
  木の hash は `final-freeze/hash_tree.py`（`snapshot.rs::hash_tree` の写し）で計算した。旧 materials に掛けると ae9007f5f32c1ea4230001ca30c7e47fc6ea1cf51e142ba2e921dbb613af32d4 が出るので、写しが正しいことも確かめた。
- binary: celeris-campaign（sha256 5132230995128dc745593546132dfb947e537d1913718bea482cbc83f5ff93dd）と celeris（sha256 99af23117cff6ec4fd5851ad13629c867dee66faac96604c50d5dad8ab471640）を HEAD から `cargo build --release -p celeris-campaign -p celeris` で作った。
  celeris-campaign は grade-integrate の修正を含む。celeris は env-nollm の build と同じ sha256 になった。
- pins.json: A_VERSION 2.1.303・A_PROGRAM・A_PROGRAM_SHA（375988f457756524234695cf2d8f64ace99c5f68682e47ea24862a843c191450、実 file と一致）・CAMPAIGN_BIN_SHA・CAMPAIGN_SHA・MATERIALS_SHA・CAMPAIGN_ID を記入した。
  RELEASE は空のまま（推測で埋めない）。
- 設定は env-nollm の measure/ から R/pinned/main へ写し、checker の規則に合わせた:
  - daemon.toml: [jev] 節を消した（既定で無効）。db・token・kb・scratch を `/local/celeris/data/measure/main-4/state/v3` 配下へ移した。cap-probe の command は R の binary にした。
  - launcher.env: 行末の注記を別の行へ移した。GLOBAL_LEDGER は manifest の shared_ledger にした。
  - task-template の project_id は、新しい測定 DB で作り直すまで仮置きにしてある。

## 実行した検査（提出時の HEAD で実行）
- `celeris-campaign validate` exit 0、`dry-run` exit 0、`stage-grader-inputs` exit 0、`lock --measurement-id mlsys-main-4-v3` exit 0（seal 545761e94988915358168038a2cf3ab26d48a69a8850c682bee412e338a9f9bf）。
- `celeris-campaign config-check --manifest R/pinned/main/final.json --measure-config … --normal-config … --canaries … --kb-src …` exit 0（ok: true）。
- `python3 scripts/campaign/check-main-pins.py --root R --pins R/pinned/main/pins.json` exit 1（`NG pins.json: RELEASE: pending: 人が記入`、1 件）→ measurement_ready=false。
- 診断のための追加の 1 回: RELEASE に仮の値 000000000000 を入れた別の pins で走らせた。exit 1、9 件。ready の判定には使わない。
  残りは RELEASE 由来 3 件、env-lock.json 無し、cap-probe 証跡 3 件、実 K2 preflight の証跡 2 件。

## 未解決事項（freeze-report.json の blocked）
1. release（人）: 本 task を統合した後に release gate と昇格を行う。その sha12 を pins・launcher.env・manifest B に書き、lock を作り直す。
   d817adbbd092 は P0-1 を含むが本 task を含まないので、pin しない。
2. a-account-login（人）: claude_max_lab に login する。
3. claude-oauth-catalog（人）: OAuth token を更新し、env-lock を作れるようにする。
4. sirius-controlmaster（人）: TOTP で ControlMaster を張り直し、cap-probe（A/B）を採り直す。
5. real-preflight（1〜4 の後に worker）: 実構成の preflight で exit 0 を取り、新しい DB で project_id を作り直す。
6. shared-ledger（人）: 承認の出典・残枠を記録し、ledger を初期化する。
7. repo-src-submodule-commits（人）: K2 の submodule の commit object を用意する。
8. consistency-rerun（5 の後）: 新しい revision の out で checker・export・report の照合をやり直す。

## 提案
- check-main-pins の lock 規則（measurement_id = campaign_id）と P0-1 の `<campaign_id>-<sha12>` 形式がずれている。どちらかに統一すると、P0-3 の記録形式と突き合わせやすくなる。
