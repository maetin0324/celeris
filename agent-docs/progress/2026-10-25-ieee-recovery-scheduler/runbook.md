---
title: IEEE recovery scheduler — 復旧比較 smoke 実行票と P0-2 identity 最終照合（runbook）
tasks: [01M5TCGEVJF1QW03CDWPJRRKZ5]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# 復旧比較 smoke 実行票と P0-2 identity 最終照合（runbook）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。コードは変えていない。
実 job・ssh 接続・qsub・LLM は使っていない（`ssh -O check` によるローカル socket の確認だけ）。

## やったこと

- 新規 [実行票](../../../docs/experiments/recovery-scheduler-smoke.md)。題名・front matter（tasks）と 8 節
  （対象と資源／上限と停止条件／所有権と取消対象／実行手順／観測・回収・再集計／認可の出典／P0-2 identity 照合／判定）。
  コマンドは実在の入口（`run_from_env`・`close_from_env`・`regen_from_env`、`selftest.sh`、qstat/scp/ssh の読み取り・配置）だけ。
- P0-2 identity 照合: `celerisctl show 01M5SRJMB1CSGBYT028X0B67N2`（exit 0、Blocked）と同 task の
  `artifacts/main4-ready/` の `pin-reconcile.json`・`consistency.json`・`freeze-report.json` を読み、ADR D3 の 6 行を照合した。
  一致 4（研究資産の形式・collector・復旧比較・結合）、部分一致 1（harness marker の account_id）、照合対象なし 1（外部操作）。
  未確定 U1〜U4（account_id の扱い、recovery 専用 manifest 未作成、P0-2 最終 freeze 未了、Sirius ControlMaster）。
- 判定: `hpc_ready=false`。実 job 0 本・新規試行 0。不足は d-smoke 未回答、Sirius の BatchMode 接続なし
  （`ssh -O check -o BatchMode=yes sirius` exit 255）、I15 (4) の `RECOVERY_SMOKE_DIR` の qsub `-v` 未実装、
  課金単位の換算未照合、marker 検索未実装。

## 検査（提出時の HEAD で実行）

- `sh scripts/dev/check-doc-links.sh` → exit 0（ok）
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（ok、293 files）
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0（ok）
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の変更なし。docs のみ）
- `cargo clippy --workspace -- -D warnings` → exit 0 / `cargo fmt --all -- --check` → exit 0
- commit: 「runbook: recovery smoke runbook and P0-2 identity reconciliation」

## 未解決事項

- I15 (4) の `QsubRequest::argv` に `RECOVERY_SMOKE_DIR=dirname(script)` を足す実装が無い（ssh-entry の未解決から継続）。
  無いまま live-smoke を流すと job.sh が即 exit 2 で `out.bin` を作らない。
- 人の決定 d-smoke が未回答（未承認）。Sirius ControlMaster の張り直しは人の TOTP が要る。
- 10/29 JST は目標日で、成立条件（承認・接続・argv 実装）を優先する。

## 本番への移行

なし（本 unit は文書だけ。本番 config・daemon・DB は変えない）。live-smoke の前に人が行うこと:
d-smoke への回答、`ssh sirius` の ControlMaster を TOTP で張り直して `ssh -O check -o BatchMode=yes sirius` が exit 0 になること。

## 提案

- live-smoke の前段に、I15 (4) の argv 追加（file/ssh 両方の `Variable_List` 期待値と `FilePbs` の原票も合わせる）を修正葉として置く。
- recovery 専用 manifest（ADR D3「結合」行）を作るかどうかを本測定前に決め、実行票の U2 の暫定対応を置き換える。
