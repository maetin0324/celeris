---
title: 統合の ADR 番号 resolver が許可済み・両側既存の番号重複を統合依頼にしない
tasks: [01M42XXNHYRX6N7VHG893SX7Z7]
status: done
updated: 2026-10-04
---

# PROGRESS — 許可済みの ADR 番号重複の誤検出

正本: [ADR 2026-10-02-parallel-integration-auto-resolve](../adr/2026-10-02-parallel-integration-auto-resolve.md) の「付記（2026-10-04、許可済み・両側既存の ADR 番号重複）」。

## Phase 1（完了 2026-10-04、単一 phase）

- 許可リストの正本を `scripts/dev/adr-allowed-duplicates.txt` に移した。`scripts/dev/check-adr-numbers.sh` と `crates/task-dispatch/src/auto_resolve/classify.rs`（`allowed_adr_duplicates`）の両方がこれを読む。
- classify は「全員が target に既にある（ADR は docs/adr と agent-docs/adr を名前で同一視）」グループと「全員が許可リストにある ADR」グループを拾わない。`resolve_adr` は target・許可リストの ADR を動かさない。
- 試験（一時 git repo）: `allowed_adr_duplicate_brought_in_by_source_requests_nothing`、`duplicate_already_on_both_sides_requests_nothing`、`new_duplicates_are_still_renamed_or_requested`、`repository_allowlist_is_the_single_source`。

### 証拠

| コマンド | 結果 |
|---|---|
| 修正前（commit 167ad5ec）`cargo nextest run -p task-dispatch -E 'test(/allowed_adr_duplicate\|duplicate_already_on_both\|new_duplicates_are_still/)'` | exit 100、3 本中 2 本失敗（「許可済みの重複を番号重複として拾った」「両側に既にある重複を拾った」）、新しい重複の試験は通過 |
| 修正後 `cargo nextest run -p task-dispatch -E 'test(/auto_resolve/)'` | 35 passed |
| `bash scripts/dev/test-parallel.sh` | exit 0、3880 tests run: 3880 passed (1 slow), 12 skipped |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `sh scripts/dev/check-adr-numbers.sh` / `--refs` | ok (136 files) / ok。許可リストの 0078 の 1 行を注釈にすると 0078 の重複を報告することを手で確認 |

### 未解決事項

- 本番は修正を含む release への昇格後に効く（昇格は人）。f24bbed8 として手で統合済みの依頼は、受信箱から「統合した」と答えれば閉じる。

### 提案

- `ALLOWED_OVER_LAST`（0128 超えの許可）も resolver が使うようになれば同じ正本へ移す。今は script だけが使うので script に残した。
