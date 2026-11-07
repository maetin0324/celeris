---
title: generalize — 誤配置した report の回収をファイル名引数の 1 つの仕組みにする
tasks: [01M6S325NYYJS9YWNHDH757E08]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# generalize — 誤配置した report の回収をファイル名引数の 1 つの仕組みにする

完了日: 2026-11-06

## 結論
`crates/task-worker/src/result_location.rs` の result.json 専用の `misplaced_result` を消し、回収する file 名を引数に取る
`misplaced_report(file_name, artifacts_dir, cwd, since)` 1 つにした。規則（期待 path 優先、探索範囲と除外、run 開始以後の
ちょうど 1 件だけ copy、0 件・古い 1 件は `None`、新しい複数件は permanent error）は file 名に依らず同じ。result.json の挙動と
message（`result.json misplaced:` で始まる。`MISPLACED_RESULT_MARKER` は不変）は変えていない。review.json の判定（`crates/task-dispatch/src/review.rs`）への配線は
WU wire-review が行う（この WU では review.rs に触れていない）。

## 変更
commit（件名。基準は `$CELERIS_WU_BASE`）:
- adr: generalize misplaced report recovery to review.json（ADR を実装より先に commit）
- generalize misplaced report recovery to a file-name argument
- progress: review-json-recovery generalize

ファイル:
- `agent-docs/adr/2026-11-06-misplaced-report-recovery-generalized.md`（新規）: review.json が回収されない理由（候補が file 名 `result.json` 固定・review.rs が回収を呼ばない）と、1 つの規則にする設計。
- `crates/task-worker/src/result_location.rs`: `misplaced_report` と `RESULT_FILE_NAME` 定数。旧 `misplaced_result` は削除。
- 呼び出し元を新シグネチャへ: `crates/task-worker/src/claude_code.rs`・`crates/task-worker/src/codex.rs`・`crates/task-dispatch/src/dispatcher/worker_task.rs`（いずれも `RESULT_FILE_NAME` を渡す）。

## 試験（`result_location` の単体試験）
- 既存 2 件（他 WU の正規 result.json は候補外、探索範囲・除外・古い 1 件・複数件・1 件の回収）: 呼び出しだけ新シグネチャに変え、期待値は不変で通る。
- 追加 4 件:
  - `a_review_json_written_in_the_repo_is_recovered`: 01M6QZ031R の再現（result.json は期待 path、review.json は repo 内 `artifacts/`）。期待 path へ copy され、誤配置元は残る。旧実装は file 名 `result.json` しか候補にしないので、この試験の review.json は回収されず失敗する（旧関数には file 名を渡す口が無い）。
  - `review_json_zero_stale_and_multiple_strays_are_not_adopted`: 0 件と run 開始より古い 1 件は `None` で何も copy しない。新しい 2 件は `review.json misplaced:` の permanent error（retryable=false）。
  - `the_expected_review_json_wins_over_strays`: 期待 path にあれば stray 2 件があっても上書きしない。
  - `candidates_are_matched_by_the_requested_file_name`: result.json の stray を review.json として拾わない。result.json の message は `MISPLACED_RESULT_MARKER` で始まる。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-worker result_location misplaced reviewer_result`: 10 tests run: 10 passed
- `bash scripts/dev/test-affected.sh`: exit 0、5559 tests run: 5559 passed, 22 skipped（対象 crate: task-worker・task-dispatch・task-api・celerisctl・llm-proxy ほか）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

## 本番への移行
本番の config 変更は要らない（config 欄を足しも消しもしていない）。release に入れば効く。

## 未解決事項
- review.rs が review.json を読む前に `misplaced_report(REVIEW_FILE_NAME, …)` を reviewer run の開始時刻で呼ぶ配線と、reviewer prompt の work_dir の注意の確認は WU wire-review の担当。

## 提案
なし。
