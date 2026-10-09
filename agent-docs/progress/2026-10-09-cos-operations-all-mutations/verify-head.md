---
title: CoS 変更操作の全登録 — HEAD 検証（verify-head WorkUnit）
tasks: [01M4F24B0GAEVZQPP35830PA0F]
status: done
updated: 2026-10-09
---
# verify-head WorkUnit

完了日: 2026-10-09。コードは変えていない。対象は task branch の HEAD `5e130339`（clippy 修正を含む）。

## 結果の要点

| 検査 | コマンド | 結果 |
|---|---|---|
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 試験の compile | `cargo check --workspace --tests --keep-going` | exit 0、error 0 件 |
| 試験 | `cargo nextest run -p task-api -p task-core` | 1505 tests: 1500 passed, 5 failed（exit 100） |
| 試験（短い TMPDIR） | `TMPDIR=<短い /tmp dir> cargo nextest run -p task-api -p task-core` | 1505 tests run: 1505 passed, 2 skipped（exit 0） |
| 文書 | `sh scripts/dev/check-doc-links.sh` | exit 0（ok） |
| 文書 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（ok、170 files） |
| 進捗索引 | `sh scripts/dev/progress-index.sh --check` | exit 0（ok） |

## 1 回目の試験の失敗と原因

- 1 回目（`TMPDIR` が 93 文字の run 専用 dir）は 5 件が失敗した。
  - `task-api::browser_e2e` の 4 件（`approved_credential_requires_conformance_and_does_not_consume_approval`、`denied_approval_fails_the_task_without_a_lease`、`auth_section_store_denies_takeover_until_closed`、`registered_credential_does_not_open_approval_for_uncertified_backend`）
  - `task-api::browser_waits` の 1 件（`real_broker_registration_keeps_sentinel_out_of_db_events_artifacts_and_worker_output`）
- 失敗の表示は `timed out waiting for fixture readiness`（`browser_e2e.rs:107`、`browser_waits.rs:86`、60 秒で打ち切り）。
- 原因は fixture の unix socket path が長すぎる（SUN_LEN 上限）こと。bind が失敗して socket が現れず、readiness 待ちが時間切れになる。コード上の退行ではない。
- 同じ既知の制約は `TMPDIR` を長くした run で既に記録されている（進捗・memory 参照）。
- 短い `TMPDIR` で同じ 2 crate を流し直し、全件合格した（上の表の 2 行目）。試験用の一時 dir は走らせた後に削除した。

## 未解決事項

- run の `TMPDIR`（93 文字）のままでは `task-api` の browser 試験 5 件が落ちる。全体検査（`scripts/dev/test-parallel.sh`）の扱いは別途、TMPDIR の長さを前提にしない修正が要る。本 WorkUnit では直さない。

## 提案

- 全体検査の前に `TMPDIR` を短く取る（または試験の socket dir を短くする）ことを test-parallel 側の既定にする。別 WorkUnit で扱う。
