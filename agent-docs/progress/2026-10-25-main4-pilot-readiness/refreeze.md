---
title: main-4 pilot readiness — RELEASE 昇格後の再 freeze（final-freeze-2、mlsys-main-4-v4）
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
status: done
updated: 2026-10-26
---
# main-4 pilot readiness — RELEASE 昇格後の再 freeze（final-freeze-2、mlsys-main-4-v4）

完了日: 2026-10-26。14 の pin はすべて resolved、`check-main-pins` は exit 0、実構成の preflight は exit 0。
ただし pilot の 4 試行を流すには manifest の `approved_trials`（今は 2）を直す必要がある（「未解決」）。

以下のパス表記:
- N = `/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready`
- R1 = `N/bundle/final-freeze-1`（読むだけ。前後の全 file の sha256 一覧が一致、71 file）
- R2 = `N/bundle/final-freeze-2`（新 revision）
- RR = `N/rerun`（unit ごとの機械向け記録 `RR/*.json` と作業記録 `RR/notes/<unit>.md`）

## 前提の確認（現物）
- release: 本番 health（読むだけ）が release=a28184889f56、`/local/celeris/state/current` → `releases/a28184889f56`（`RR/revision.json` の release）。
- claude_max_lab の OAuth: 測定 daemon の lock で claude-oauth の model catalog 14 件（claude-sonnet-5-5 を含む）を取得（`RR/env-lock.json` の models_evidence）。
- Sirius ControlMaster: `ssh -O check -S /run/celeris-cap/sirius-ssh.sock sirius` → exit 0（`RR/cap-probe.json`）。
- 承認記録: 利用者の 2026-10-26 の d-approval 回答（出典・範囲・期限 2026-11-13）を台帳に記録（`RR/ledger.json`）。
- K2 submodule 5 件: `/local/celeris/data/measure/main-4/k2-submodule-bundles/` の 5 bundle（SHA256SUMS 一致、bundle verify ok。`RR/repo-src.json`）。

## 受け入れ条件ごとの結果
### 新 revision の pins が全て埋まり check-main-pins が通る
- R2 は R1 の写し。pins.json（CAMPAIGN_ID=mlsys-main-4-v4、RELEASE=a28184889f56、CAMPAIGN_SHA）、launcher.env、daemon.toml（state/v4、port 7741）、
  final.json（campaign_id、B の harness_version を 641f334b6d64 から a28184889f56 へ）を書き換えた。
  pinned の celeris-campaign で validate / dry-run / stage-grader-inputs / lock（--measurement-id mlsys-main-4-v4）/ config-check → すべて exit 0（`RR/revision.json` の steps）。
  新 campaign_sha256 = `3756ff02244a5d6c8ee4e1c04d5041487d7ebbba9ba762543bc429dfa2e51c65`。
- P0-1 の K4 v2 の 6 項目（archive・case_json・materials_task_md・kb_snapshot_md・grade_case・private）を disk から hash し直し、adoption.json と全一致（`RR/revision.json` の k4_adoption_match=true）。
- record unit で、cap-probe の実構成の証跡（`R2/pinned/evidence/cap-probe-real/` の A-k2・B-k2・A-k4）を check-main-pins が読む名前
  `R2/pinned/evidence/cap-a-k2.json`・`cap-b.json`・`cap-a-k4.json` に写した（内容は byte 一致）。
- `python3 scripts/campaign/check-main-pins.py --root R2 --pins R2/pinned/main/pins.json` → exit 0、`check-main-pins: ok`
  （`RR/check-main-pins.stdout`・`.stderr`・`.exit`）。

### 14 の pending の解消
`RR/pin-reconcile.json` の items（14 key の dict、各値に status と evidence）。resolved 14、not_required 0。

| key | 状態 | 主な証拠 |
|---|---|---|
| RELEASE | resolved | revision.json（health・current）、pins/launcher.env、preflight の daemon.release |
| A_VERSION | resolved | `A_PROGRAM --version` → 2.1.303、preflight の a_program.version |
| A_PROGRAM | resolved | preflight の a_program.path、env-lock --verify exit 0 |
| A_PROGRAM_SHA | resolved | `sha256sum A_PROGRAM` が pin と一致 |
| CAMPAIGN_BIN_SHA | resolved | `sha256sum` が pin と一致、実 preflight の argv.txt が同じ binary |
| env-lock.json | resolved | launcher lock exit 0、env-lock --verify exit 0（verified）、preflight checked に env-lock |
| cap-env.json | resolved | host の path から作り直し（前の pin と同じ内容）、cap-probe 4 件 exit 0 |
| daemon.toml | resolved | state/v4 の測定専用 DB・port 7741 で start exit 0 |
| launcher.env | resolved | 実構成の launcher preflight exit 0 |
| canaries.json | resolved | R1 から不変、B の cap-probe と preflight が読む |
| repo-src | resolved | submodule 5 件の commit を入れ verify.sh exit 0、lock 再実行が byte 一致 |
| cap-probe/preflight evidence | resolved | cap-probe A/B×K2/K4 exit 0・paid_calls 0、k2-preflight.json、argv.txt |
| task-template.project_id | resolved | 測定 DB で作った project 01M5W9014BJF29SS6VQDFX0Z18 |
| shared-ledger | resolved | ledger-init exit 0、出典・範囲・期限・承認 40・消費 0・残り 40（manifest 側の上限は「未解決」） |

### 実測定構成の preflight が exit 0
- `sh scripts/campaign/measure-launcher.sh prepare/start/preflight/stop`（env は R2 の launcher.env、測定専用 state/v4・port 7741）→ preflight exit 0、
  ok=true、missing=[]、paid_calls_made 0、checked 17 段（env-lock・trial-state・lock・budget を含む）。
  run_id 01M5WANNTJSBHTQA364VMBC5YB、measurement_id mlsys-main-4-v4（`RR/preflight.json`、config=real）。daemon は stop 済み。
- cap-probe が実 LLM を呼ばない宣言ガードは維持（B の probe task は fake harness・fake model、cap_probe.rs は変更なし）。

### 進捗 file と Rust の検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 本 unit はコードを変えていない。Rust の検査結果は下の「検査」に書く。

## 検査（提出時の HEAD で実行）
- `sh scripts/dev/check-doc-links.sh` → exit 0
- `sh scripts/dev/check-adr-numbers.sh` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` → 結果は報告本文

## 変更（この task branch の commit、件名）
- `fix(campaign): check-main-pins の env-lock arm 集合を manifest の B/C config に合わせる` — main-4 の manifest は A/B だけなので、固定の B/C を manifest の config に合わせた。試験を 1 件追加。
- `campaign: ledger-init で共有台帳に人の承認（出典・範囲・期限・試行数）を記録する` — campaign CLI に ledger-init を追加。
- `campaign: ledger-init の決定記録を global_ledger の module doc へ移す（WU 範囲外の ADR file を外す）`
- `refreeze: pin-reconcile と進捗を記録する`（本 unit）

## 安全
- 本番の systemd・`~/.config/celeris`・本番 DB・release dir は読むだけ。測定は `/local/celeris/data/measure/main-4/state/v4` の専用 DB・port 7741。
- token・秘密値は file に書いていない（測定 token の値で RR の記録を grep -F → 0 件）。LLM の有料呼び出し 0。
- 旧い measurement（mlsys-main-4-v2・v2-pilot1・v2-nollm1・v3）の記録とは別の id・state dir。

## 未解決
- manifest（R2 の final.json）の `experiment_sets.main.fixed.approved_trials` が 2。台帳の上限は manifest との小さい方なので、実効の残りは 2 試行。
  pilot は 4 試行（K4-A0・K4-B0・K2-A0・K2-B0、expected_trials 4）なので、watch の admit_start が 3 試行目以降を拒否する。
  直すには runbook C5 のとおり approved_trials を承認値（40）に替え、validate → lock → env-lock → preflight をやり直し、pins.json の CAMPAIGN_SHA を更新する。
- ledger-init の設計判断は ADR file ではなく `crates/campaign/src/global_ledger.rs` の module doc にある（WU 範囲の都合）。

## 提案
- approved_trials を 40 に替えた final-freeze-3 を作る葉を pilot 前に置く。今回の unit 構成（revision → env → preflight → record）をそのまま使える。
- ledger-init の決定を `agent-docs/adr/2026-10-26-global-ledger-approval-init.md` として起こす（module doc から移す）。
- check-main-pins に「manifest の approved_trials ≥ expected_trials」を足すと、今回の食い違いを freeze の時点で検出できる。
