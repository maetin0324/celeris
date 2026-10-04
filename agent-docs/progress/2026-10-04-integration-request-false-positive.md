---
title: 並列取り込みの統合依頼の誤検出（日付 ADR）と回答済み依頼の残留の修正
tasks: [01M425C01VCJ8QZ1YX0V0WSZ23]
status: done
updated: 2026-10-04
---

# PROGRESS — 統合依頼の誤検出と残留の修正（ADR 2026-10-02-parallel-integration-auto-resolve 付記 2026-10-04）

正本: [ADR 2026-10-02-parallel-integration-auto-resolve](../adr/2026-10-02-parallel-integration-auto-resolve.md) の「付記（2026-10-04、日付名 ADR の誤検出と回答済み依頼の残留）」。

## Phase 1（完了 2026-10-04、単一 phase）

本番（release d9cf53ab、task 01M420EMSFS1VP5RWF2FGCV6XR）の観測:

- integrate-close が merge base = target 先端・`git merge` 衝突なしの取り込みで、日付名 ADR 4 本を conflict_files とする統合依頼（reason「日付名 ADR の衝突」）を出した（events 196548）。
- integrate-impl の依頼（docs/ops/cron-jobs.md）は人が `POST /tasks/{id}/answer`（汎用の回答、events 196372 `answered`）で再開したが、`integration_answered` が無く受信箱に残り続けた。

### 原因

1. `crates/task-dispatch/src/auto_resolve/classify.rs` の `adr_number` が日付名 `YYYY-MM-DD-<slug>.md` の年 4 桁を ADR 番号と読み、同じ年の日付名 ADR を「同番号グループ」にしていた。取り込み側が同年の日付名 ADR を足すとグループ全体が `renumber::resolve_adr` に渡り、日付名は振り直せないため `NeedsHuman`。
2. `IntegrationAnswered` を追記するのが受信箱の answer API だけだった。汎用の回答・手で統合した後の再実行・両端 head の変化による新依頼のどれでも古い依頼が閉じない。

### 入れたもの

- `classify::adr_number`: `dated_adr` の path は `None`。
- `task-core`: `TaskStore::integration_request_answer`（id 指定・未回答のときだけ追記）、`TaskStore::integration_requests_close`（origin の未回答を全て閉じる）、`integration_request_record` が同じ origin の古い組を `superseded` で閉じる。定数 `integration_request::{INTEGRATED_ANSWER, SUPERSEDED_ANSWER}`。
- `task-dispatch` `phase_integration::on_integration_finished`: merge が衝突なしで通ったら `phase:<key>` の未回答依頼を `integrated` で閉じる。
- `celeris` `delivery::advance`: `merge_reviewed` 成功時に `delivery` の未回答依頼を `integrated` で閉じる。
- `task-ops` `gate::answer`: `blocked(question)` の統合 WU の `phase:<key>` の未回答依頼を `answered`（note = 回答文）で閉じる。
- `task-api` 受信箱の answer: `integration_request_answer` を使う。段の依頼で統合 WU が既に再開・完了していれば（本番の残留の状態）回答の記録だけ行い 200 で消す（従来は 410）。
- ADR 付記（経路と answer の表）。

### 試験

- `crates/task-dispatch/src/auto_resolve/tests.rs`: `dated_adrs_added_on_both_sides_without_conflict_request_nothing`、`fast_forward_source_with_dated_adrs_requests_nothing`。修正前に実行して両方 FAILED（本番と同じ 4 本の日付 ADR・同じ reason 文で `NeedsHuman`）を確認した。
- `crates/task-core/src/store/tests.rs`: `integration_requests_close_once_and_newer_request_of_same_origin_supersedes`。
- `crates/task-ops/src/inbox/tests.rs`: `answered_integrated_and_superseded_integration_requests_leave_the_inbox`（attention と human_inbox の両方）。
- `crates/task-dispatch/src/dispatcher/tests/work_units.rs`: `an_integration_request_leaves_the_inbox_once_the_human_merged_and_answered`（本番の残留の再現: 依頼 → 人が worktree で手で merge → 汎用の回答 → 依頼が消え、再実行の統合が done）、`a_clean_integration_closes_the_open_request_of_its_origin_as_integrated`。
- `crates/celeris/src/delivery/tests.rs`: `integration_repair_fallback_delivers` に配送依頼の消滅を追加。
- `crates/task-api/tests/inbox_notifications.rs`: `phase_integration_request_of_a_finished_unit_is_answered_without_resuming`。

### 証拠コマンドと結果

- 修正前: `cargo test -p task-dispatch --lib auto_resolve::tests::` → `dated_adrs_added_on_both_sides_without_conflict_request_nothing` と `fast_forward_source_with_dated_adrs_requests_nothing` が FAILED（`NeedsHuman`、conflict_files は日付名 ADR 3〜4 本、reason「日付名 ADR の衝突」）。4 passed; 2 failed。
- 修正後: 同コマンド → 6 passed（renumber を含む `auto_resolve::` 全体で 26 passed）。
- `cargo test -p task-core --lib integration_requests` → 2 passed。`cargo test -p task-ops --lib integration_request` → 3 passed。
- `cargo test -p task-dispatch --lib -- an_integration_request_leaves_the_inbox a_clean_integration_closes phase_integration_request_is_the_only` → 3 passed。
- `cargo test -p celeris --lib delivery::tests::integration_repair_fallback_delivers` → 1 passed。`cargo test -p task-api --test inbox_notifications` → 9 passed。
- `cargo test --workspace` → exit 0。test result 行 128: passed 3858, failed 0, ignored 13, exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（`--all-targets` でも exit 0）。
- `sh scripts/dev/check-doc-links.sh` → ok。`sh scripts/dev/check-adr-numbers.sh` → ok (135 files)。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok。

### 未解決事項

- 本番に残る 1 件（`01M420EMSFS1VP5RWF2FGCV6XR:f865063c…:7fbc8de6…`）は過去の events で、この修正は遡って閉じない。修正を含む release に昇格した後、人が受信箱から「統合した」と答えれば消える（統合 WU は done なので再開は起きない）。昇格前の release では answer が 410 のまま。

### 提案

- 旧 release で記録された残留依頼を昇格直後に一掃したければ、daemon 起動時に「統合 WU が done の `phase:` 依頼」を `integrated` で閉じる一回限りの整理を足せる。今回は人が一度答えれば済むので入れていない。
