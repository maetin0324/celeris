---
title: main-4 pilot readiness — pending pin の現物照合
tasks: [01M5SRJMB1CSGBYT028X0B67N2]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# main-4 pilot readiness — pending pin の現物照合

完了日: 2026-10-25（pin-audit WU のみ）。実測定開始不可。基準は `$CELERIS_WU_BASE`。
コード・旧 main-3 raw/grades・旧 main-4 bundle・本番設定/DB は変更していない。

## 成果

機械向け正本:
`/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/artifacts/main4-ready/pin-reconcile.json`。
取得ログ・再現用 audit.py・JSON の写し・health は
`/local/celeris/data/workspaces/01M5SRJMB1CSGBYT028X0B67N2/wu/pin-audit/artifacts/`。

freeze-status の現物から pending の key 集合を取り出し、14 件すべてに status・evidence（実在 file の path/sha256、欠落は exists=false）・next を記録した。
分類は resolved 0、worker-resolvable 12、human 2。分類の完了は pin の解消ではない。

| 分類 | key | 次の担当工程 |
|---|---|---|
| human | RELEASE | 決定済み post-merge。統合後の release gate・本番昇格は人が実行。現稼働版を測定採用版と同一視しない |
| worker-resolvable | A_VERSION / A_PROGRAM / A_PROGRAM_SHA | env-nollm: 実行せずに調べた Claude symlink の実体候補と digest を JSON に記録。version 候補は path のみ。実 version・両 arm の一致・アカウント分離は launcher で確認 |
| worker-resolvable | CAMPAIGN_BIN_SHA | 採用 source から専用 binary を build・hash。現 release/bin に campaign binary は無い |
| worker-resolvable | env-lock.json / cap-env.json | 同一測定環境の実 cap-probe と model catalog を採取、offline-lock/launcher lock と verify |
| worker-resolvable | daemon.toml / launcher.env / canaries.json | 本番から隔離した専用設定・KB・state・DB・port を生成し config-check/env-check |
| worker-resolvable | repo-src | K2 initial.lock.json の commit/tree/content と入れ子を含む 5 submodule を照合。旧 bundle の source は欠落、main-3 snapshot/repo は空。source の取得先が見つからなければ具体的な取得条件を上げる |
| worker-resolvable | cap-probe/preflight evidence | 実 daemon/launcher で取得し exit 0 を必要条件とする。旧 preflight は exit 1、fake Host 正例は代用不可 |
| worker-resolvable | task-template.project_id | 専用 daemon 起動後、その API で専用 project を作り placeholder を実 ID に固定。本番 DB へ書かない |
| human | shared-ledger | 初期化は後段で可能。継承 manifest の approved_trials=2 だけでは承認出典・有効期限・消費/残枠の証明にならない。pilot 実行前に人が確認。本 task は実 pilot をしない |

## 版差と先行・並行資産

`celerisctl health` は未提供（exit 2）。代わりに本番の読み取り専用
`GET http://127.0.0.1:7710/api/v1/health` を取得し、release manifest の source SHA と照合した。
health は release b7b9c3b0d0ff、schema 93、role active。着手時 HEAD はその source と一致するが、
監査時 main は先行しており P0-1 公開入力 v2 を含む。
10/24 の GitHub main はユーザー提供の過去観測として扱い、ローカル git object の存在と件名だけ確認。
GitHub を再照会したとは主張しない。3 版の完全 SHA、件名、GitHub→release の 95 件・release→監査時 main の 20 件の差は JSON の versions に全件保存した。
`/proc` の読み取りから実行像を確認できなかったため、launcher が後段で process 実体照合を行う必要がある。

P0-1 の adoption.json、archive、manifest、exclude、audit、lock、case/grade-case/private 等を現物と監査時 main の git blob で照合。
path を持つ参照 11 件のうち 10 件一致。**nollm_summary の現物 hash が adoption の記録値と不一致**。
期待値と実値は JSON に残した。最終 freeze 前に現 summary の生成経緯を確認し、採用記録と証拠の整合を取り直す。
archive v2 の hash と採用 case/grade/private は一致。main-4 bundle は v1 のままであり、後段 final-freeze で新 revision に取り込む。
採用記録が示す判断要 26 件の扱いも、その採用範囲を変える場合は人の決定に従う。

P0-3 の親 workspace に progress と recovery scheduler ADR が存在した（path/hash は JSON）。
合わせるのは campaign/measurement ID、完全 hash、collector の campaign_sha256/config/problem_id/rep/trial_no、
harness marker と trial_key の対応、完全な `pbs:sirius:<job_id>`、UTC/monotonic epoch・seq・追記 JSONL、欠測 null、共有作業一度計上。
recovery の baseline/celeris・実 scheduler raw は A0/B0 主比較と dataset/out/分母を分ける。
progress は contract 段のみの完了・実 scheduler 受入未了と明記しており、親 result は計画のみ。P0-2 の最終 ID/pin を後で渡す。

## 証拠（提出時の HEAD で実行）

- 条件0/1/2: `python3 <WU artifacts>/verify.py` → exit 0。14 key の完全一致、status/evidence/next、3版の SHA/件名と差のリスト、進捗の存在を確認。P0-1 の既知 summary 不一致を明示、ready=false を確認。
- `bash scripts/dev/test-affected.sh` → exit 0。docs のみで Rust/web 試験対象なし。health 内の Python 38+1 試験、progress hash 自己試験 5 項目、doc-links 自己試験は成功。source-size 失敗0（既存 debt 警告3、理由付き例外1）、architecture-map 726 path、doc-links/ADR/progress 検査成功。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0、差分なし。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、変更はこの進捗ファイルのみ。`git diff --check` → exit 0。
- 記録を含む commit 後の提出時 HEAD でも上記を再実行し、成功を確認した。ログは WU artifacts の final-*.log。

全 workspace 試験・実 browser/userns・release gate は実行していない。

## 未解決・本番への移行

この WU は監査のみ。12 worker 項目は env-nollm/final-freeze、2 human 項目は上記の条件で解消する。
P0-1 summary digest 不一致も最終 freeze 前に解消する。sonnet と post-merge は再決定を求めない。
測定 readiness は false のまま。本番への変更は人が行う release 手順だけで、本 run は実施しない。

## 提案

最終 freeze 時、採用記録の全参照を再 hash し、summary の再実行による更新と adoption の更新を同じ採用版にそろえる。

## commit（件名）

- `phase 1: reconcile main-4 pending pins and runtime versions`
