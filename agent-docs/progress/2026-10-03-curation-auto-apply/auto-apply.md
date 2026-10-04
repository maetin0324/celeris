---
task: 01M421KVXKGEE8PWER7HAJJFZY
work_unit: auto-apply
status: done
completed: 2026-10-03
base: 91bc486dc202
---

# 日次整理の apply を承認なしで適用し KB に commit・push する（celeris 終端処理）

## やったこと

- `crates/celeris/src/knowledge_curation.rs`（ADR-0131 付記 2026-10-04）:
  - `finish`: `mode = apply` で検証に通った計画は `curation-apply`（`approve-<hash>`）の決定を出さず、
    `apply_validated` でその場で本番 KB を再検証（`check_plan` を再実行し hash 一致を確認）→
    `task_ops::knowledge_curation::apply` → 変更 path（統合先・`_curation/<date>.md`・`index.json`・`README.md`）を
    `task_ops::knowledge::commit_curation` で 1 commit → `push_remote`。再検証・apply 失敗は適用せず `Stale`。
  - push: `Pushed` / `NoRemote`（報告に「remote 無し」）/ `Failed`（apply は失敗にしない。報告と event に残す）。
    commit できなければ push は `skipped`。
  - 報告（daily-summary 1 件）に「KB の git」節（commit sha・push の結果・revert の案内）。状態ファイルの
    `Entry` に `commit_sha`・`push` を追加。
  - 状態ファイルに残る `AwaitingApproval` 行は `apply_pending` で同じ再検証を通れば適用、通らなければ `Stale`。
    開いたままの `curation-apply` 決定は `DecisionWithdrawn` で取り下げる。
  - `curation-human` は従来どおり出し、自動承認しない。`request_approval`・`APPROVE_PREFIX`・`REJECT_OPTION` を撤去。
- `Event::KnowledgeCurationApplied { date, merged, new, deleted, fixed, commit_sha, commit_error, push, push_detail }`
  を task-core に追加（task-api の種類名 `knowledge_curation_applied`）。`UPDATE_SCHEMA=1` で docs/api/v1 を再生成、
  gui `pnpm gen:types`、web `node web/scripts/gen-types.mjs`、web の realtime `EVENT_KINDS`・invalidation map に追加。
- 試験（tempdir の bare repo を KB の origin）: 承認なしで適用・commit（題に日付と件数、本文に task id）・push、
  remote 無し、push 失敗（commit は残り task は done のまま）、元ページ変更で不適用、旧 `AwaitingApproval` 行の
  自動適用 / Stale。既存の承認前提の試験（unit・`tests/daily_curation.rs`）を新方針に書き換えた。
- `docs/ops/cron-jobs.md` §6 に承認なしの挙動・remote 無し / push 失敗・救出手順（revert / checkout）を追記。

## 証拠

- `cargo test -p celeris knowledge_curation` → lib 17 passed、daily_curation 3 passed（受け入れ条件 0 の command は exit 0）
- `cargo test --workspace` → exit 0、3839 passed / 0 failed
- `cargo clippy --workspace -- -D warnings` → exit 0
- web: `pnpm exec vitest run api/realtime` → 56 passed、`pnpm exec tsc --noEmit` → 差分・エラーなし。gui `tsc --noEmit` → エラーなし

## 未解決事項

- 本番 KB に private remote を足す・daemon UID で非対話 push できる認証を置くのは人の作業（docs/ops/cron-jobs.md §6）。

## 提案

- なし
