---
title: task 一覧 API の遅さ: ADR（WU adr）
updated: 2026-10-21
tasks: [01M5EDRV4B2K5NYPMF9EZ0YN9A]
status: done
completed: 2026-10-21
---
# task 一覧 API の遅さ: ADR（WU adr）

ADR: `agent-docs/adr/2026-10-21-task-list-opt-in-integrations.md`

## 実施
- 現状の行番号（tasks.rs:28-86,296、view.rs:1624、changes.rs:80-117、task_integration_state.rs:95-114）を読み、4 決定を ADR に書いた。
  - D1 `?with=integrations` の opt-in（旧い常時付加は消す）
  - D2 要求ごと・repo ごと 1 回、`task_integration_list_impl` を 1 本の SELECT
  - D3 `run()` を wait thread + channel（timeout 維持）
  - D4 CPU 負荷なしの計数試験
- コードは触っていない。

## 証拠
- `test -s agent-docs/adr/2026-10-21-task-list-opt-in-integrations.md` と `grep -c '^### D' ` で 4 決定を確認（下記）。

## 本番への移行
config 変更なし。後続 unit の実装は web と同時 release。

## 未解決・提案
なし。
