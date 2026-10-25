---
title: main-4 pilot readiness — 測定専用環境と実 daemon での launcher 全段（無 LLM）
tasks: [01M5T4W9MVH43ZGWE6V2ZPNXGE]
status: blocked
updated: 2026-10-25
---
# main-4 pilot readiness — 測定専用環境と実 daemon での launcher 全段（無 LLM）

更新日: 2026-10-25（close）。状態: blocked。人の項目が残り、ready=false。

- 段の記録・pin 解消表・人の手順: `/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready/nollm-pass.json`（`stages`・`pins`・`blocked`・`ready_basis`）
- 測定 root: M=`/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready/measure`。DB・port 7741・state・token は本番と別。無 LLM の写しは `M/nollm/`（port 7742）
- 作業記録: `M/notes/{campaign-bin,repo-src,measure-config,daemon-preflight,watch-export}.md`
- paid_calls_made = 0（全段）。pin-reconcile.json と旧 bundle は書き換えていない

## 各段の結果

### 実構成（実 daemon。fixture なし。daemon start 以降は候補 manifest の写しを使う）
候補写し `M/candidate/final.json` は、manifest B の harness_version だけを build 基点の label に替えたもの（下の「RELEASE」を参照）。

| 段 | exit | real_daemon | 主な ID | 要点 |
|---|---|---|---|---|
| repo-src | 1 | false | task 01M5T4W9MVH43ZGWE6V2ZPNXGE | 主 tree は initial.lock と一致し、submodule 5 件の content hash も一致。submodule の git commit が無い（found:false） |
| prepare | 0 | false | run 01M5T5ZB64ZETYJE1JDJKFDVJR、measurement mlsys-main-4-v2-pilot1 | validate・config-check・token 作成・CELERIS_BIN の sha 固定 |
| start | 0 | true | daemon instance 01M5T6B5BQBN120MA2BXNYQE75、project 01M5T6BJ5Y23SXE2F7RT4JJ47N | 正確な env（RELEASE なし）では exit 1。候補写しの env で exit 0 |
| cap-probe | 1 | true | probe task 01M5T6BZP4RF1NJSGGEE75RV29（B/K2）、01M5T6C5Q446CGA7011MP3CDZB（B/K4） | B は sirius socket と sirius-link だけ不足。A は sirius-ssh.sock が dangling で、bwrap の bind に失敗し全項目が落ちる |
| preflight | 1 | true | run 01M5T65F1ZBC8VTRYQXXE60CM2 | missing: capability（A）、a-arm（claude_max_lab が未ログイン） |
| env-lock | 1（--verify も 1） | true | 同上 | model catalog が空（claude OAuth token の期限切れ）。lock を作らない |

### stub harness（無 LLM。daemon は実 binary、構成は `M/nollm/` の写し。measurement mlsys-main-4-v2-nollm1）
| 段 | exit | real_daemon | 主な ID | 要点 |
|---|---|---|---|---|
| watch | 0 | true | B/K2 task 01M5T7T04PHDDEW157CMCFWK3J、project 01M5T7MVYBVDM00JVNJQDD7A12 | settled。B/K2 は claude-b-stub で 15 run。B/K4 は arm_budget_cancelled |
| collect | 1 | true | — | not_submitted。stub は K2 の job を投入しないため |
| grade | 1 | true | — | 3 件を採点（final_pass 0）。最後の check で 1 |
| check | 1 | true | — | manifest の initial_info が lock の materials_sha256 と食い違う |
| export | 1 | true | — | 同上 |
| human-op-report | 0 | true | — | human-ops.jsonl は空 |
| final-accounting | 1 | true | — | 同上の manifest 不整合。decision-log の export は 0 records |

nollm-prepare・nollm-start・nollm-preflight（いずれも exit 0）は `M/nollm/logs/` にある。実構成の段と名前を分けるため、stages には入れていない。

## pin の解消（pin-reconcile.json の 14 件）
- resolved（6）: A_VERSION（2.1.303 (Claude Code)）、A_PROGRAM、A_PROGRAM_SHA、daemon.toml、canaries.json、task-template.project_id
- blocked（5）: RELEASE（人）、shared-ledger（人）、CAMPAIGN_BIN_SHA、launcher.env、repo-src
  - CAMPAIGN_BIN_SHA: 候補 binary の build 基点は、下の campaign 修正より前。昇格した release と同じ source から作り直す
  - launcher.env: RELEASE（人）に依存する
  - repo-src: submodule の commit が無い（人が出所を示す）
- failed（3）: env-lock.json、cap-env.json、cap-probe/preflight evidence。実段が非 0 で、原因は人の項目（OAuth・sirius master・ログイン）
- ready = false。実構成の preflight と env-lock --verify がともに exit 1 のため（`ready_basis`）

## 変えた code（commit 件名）
- `campaign: A の cap-probe 失敗に bind できない cap env の元（socket・mount）を添える` — A probe が落ちたとき、bind できない cap env の元を B/C と同じ id で detail に添える。判定は変えていない。試験 `failed_a_probe_names_the_missing_socket_and_mount_behind_the_exit`（daemon-preflight unit）
- 同じ工程の別 unit の修正: `fix: progress_index_commit_hashes の短縮 hash を検出条件を満たす長さまで伸ばす`（fix-hash-selftest）

## 証拠コマンドと結果
- `sh scripts/campaign/measure-launcher.sh prepare --env M/launcher.env` → exit 0（`M/logs/prepare.*`）
- `sh scripts/campaign/measure-launcher.sh start --env M/candidate/launcher.env` → exit 0。正確な env では exit 1（`M/logs/start*.*`）
- `M/bin/celeris-campaign cap-probe --case {K2,K4} --arm {a,b} --cap-env M/cap-env.json …` → exit 1（`M/cap-probe/`）
- `sh scripts/campaign/measure-launcher.sh preflight --env M/candidate/launcher.env` → exit 1（`M/logs/preflight.*`）
- `LOCK_OUT=M/lock … lock` → exit 1。`env-lock --verify` → exit 1（`M/logs/lock.*`・`env-lock-verify.*`）
- `M/nollm/` の watch〜final-accounting: 上の表のとおり（`M/nollm/logs/*.stdout|stderr`）
- 実構成の不変: `M/nollm/logs/real-config.{before,after}.sha256` を diff して差分なし
- close では sirius-ssh.sock を再確認した: まだ dangling
- close では A_PROGRAM の sha256 を再計算した: preflight の値と一致
- この unit の repo 検査は下の「検査（提出時の HEAD）」に書く

## 未解決（人）
`nollm-pass.json` の `blocked` に、各項目の手順（steps）を書いた。
- release: 統合後の release gate と昇格。そのあと final-freeze で manifest B/C を再固定し、binary を作り直して CAMPAIGN_BIN_SHA を更新する
- shared-ledger-approval: 承認の出所・範囲・期限・消費（過去分を含む）・残数を記録する。approved_trials は 2 で、2 problem × A/B の 4 trial に足りない
- shared-ledger-path: 宣言の path は `/local/celeris/data/measure/main-4/global-ledger` で、まだ作られていない
- a-account-login: claude_max_lab を通常の経路でログインする
- claude-oauth-catalog: 通常の経路で token を更新し、そのあと lock と --verify を再実行する
- sirius-controlmaster: TOTP で master を再確立し、cap-probe と preflight を再実行する
- repo-src-submodule-commits: submodule 5 件の pinned commit を含む repo か bundle
- manifest-initial-info-mismatch: initial_info と materials_sha256 の再固定、MEASUREMENT_ID を lock の派生形に揃える
- 未調査: A/K4 の初期 workspace・KB が snapshot と一致しない。B の tokens_missing。decision-log の export が 0 records

## 提案
- 測定 daemon.toml の [[clusters]] は本番の写しで、pegasus forward 127.0.0.1:18000 が本番と重なる。測定用には forward を外すか、別 port にする
- check・export・final-accounting の initial_info 不一致は、preflight の段で先に検出させる。今は watch の後に初めて分かる
- MEASUREMENT_ID は launcher.env で手書きせず、campaign sha から導く

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0。docs のみの変更なので Rust 試験は 0 件。文書の自己試験は ok
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → ok。`check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok。`check-doc-links.sh` → ok
