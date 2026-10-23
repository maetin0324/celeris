---
title: 進捗ファイルと完了報告で commit を hash でなく件名で書く
tasks: [01M5MQ6SXVA1WQYJNDG7VJRW8T]
status: done
updated: 2026-10-23
---
# 進捗ファイルと完了報告で commit を hash でなく件名で書く

完了日: 2026-10-23

## 変更
- `phase 1: commit を件名で書く規約と進捗の hash 検査` — 次をまとめて入れた。
  - ADR `agent-docs/adr/2026-10-23-commit-refs-by-subject.md`（D1〜D6）。機械検査は「main の祖先でない hash で失敗」を選んだ（全面禁止はしない。理由は D4）。
  - `CLAUDE.md`「各Phase完了時」と「完了報告の書き方」、ADR-0128 D3・D4 の付記、`agent-docs/README.md`、
    新しいガイド兼テンプレート `agent-docs/guides/progress-files.md`。
  - reviewer の prompt（`build_review_prompt`）に「hash が無いことを不備にしない、件名で `git log $(git merge-base HEAD main)..HEAD` と照合する」節、
    planner の check の書き方に「検証 SHA の記録を条件にしない」を足し、`claude_code/tests.rs` で固定した。
  - `scripts/dev/progress-index.sh --check` に、この branch が変えた進捗ファイルの 7〜40 桁の hex が main の祖先の commit でなければ失敗する検査を足した。
    試験 `scripts/dev/tests/progress_index_commit_hashes.sh`（一時 repo、5 場合）。`scripts/dev/check-health.sh` から `--check` と試験を走らせる。
  - `--check` が main 上で落ちていた `2026-10-23-web-event-kind-required-checks.md` の `tasks: []` を、原因の task の id にした。

- `fix: 進捗 hash 検査の試験で fixture の path を変数で組む` — attempt 1 は試験台本の `agent-docs/progress/...` の literal path を `check-doc-links.sh` が実在検査して test-affected が落ちた（3 件）。path を `d=agent-docs/progress` から組むようにした。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `sh scripts/dev/tests/progress_index_commit_hashes.sh` → exit 0（5 場合 OK）
- `sh scripts/dev/progress-index.sh --check` → exit 0
- `cargo test -p task-worker --lib claude_code::tests` → 125 passed
- `bash scripts/dev/check-doc-links.sh` → exit 0（check-doc-links: ok）
- `bash scripts/dev/test-affected.sh` → exit 0（check-health・check-doc-links 込み。nextest 4527 passed・18 skipped）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 既存の進捗ファイル・KB ページの hash は履歴として書き換えていない。進行中の branch で自分の commit の hash を進捗に書いているものは、
  main 取り込み後の test-affected（check-health）で落ちる。件名に直す。

## 提案
- なし
