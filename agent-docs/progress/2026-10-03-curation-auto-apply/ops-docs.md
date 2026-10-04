---
title: 日次整理の apply 自動適用（ops-docs WU）: cron-jobs 運用文書 §6 の書き直しと knowledge-curation budget 120
status: done
updated: 2026-10-03
---
# ops-docs: cron-jobs 運用文書と knowledge-curation budget

親の task（日次整理の apply を承認なしで自動適用し、KB に commit・push する）のうち、この WU が担当した部分の記録。
設計判断は ADR-0131 の付記（kb-commit-push WU が書く）。ここでは運用文書と設定だけを変えた。

## 変更

- `config/celeris.example.toml`: `[[harnesses]] id = "knowledge-curation"` の `budget = { max_turns = 120 }`（旧 30）。
  1 回 40 件の上限は変えていない。本番 `config.toml` は変更していない（人が §1 の手順で反映する）。
- `docs/ops/cron-jobs.md`:
  - §1: harness 例の budget 行（120）を追加。
  - §2.1: 「承認時」の再検証を「適用時」に直した（承認 decision は出なくなったため）。
  - §6: 「dry_run から apply へ切り替える」を書き直し。6.1 承認なしの自動適用・1 commit・push の流れ（push 失敗は apply を失敗にしない）、
    6.2 切り替え、6.3 救出（`git revert <sha>`、`git checkout <sha>^ -- <path>` と commit、`git push`、確認は `git show --stat` と `celerisctl knowledge get`）。

## 受け入れ条件の証拠

- 条件 0（budget 120）: `grep -n max_turns config/celeris.example.toml docs/ops/cron-jobs.md` → example の 374 行目と docs の 41 行目に `budget = { max_turns = 120 }`。
- 条件 1（§6 が自動適用・commit・push と救出手順を説明）: §6.1〜§6.3 に記載。文書内の相対 link（`../../agent-docs/adr/0131-cron-jobs.md`、`../api/cron-jobs.md`）は存在を確認済み。
- 条件 2（crates/ に差分が無い）: `git diff --name-only b742ec75 -- crates | wc -l` → 0。

## 未解決事項・提案

- commit の題の書式（`日次整理 YYYY-MM-DD: 統合 n・新規 n・削除 n・修正 n 件`、本文の `task: <task id>`）は §6.1 に書いた。
  kb-commit-push WU の実装がこの書式と一致するか、integrate-impl で確かめる。違えば §6.1 を実装に合わせる。
- cargo test / clippy は docs と設定だけの変更のため、この WU では走らせていない。task 全体の検査は close-sync で実行する。
- 本番への反映（daemon 昇格、`config.toml` への harness 追記、`mode = apply` への切り替え）は人の手順。
