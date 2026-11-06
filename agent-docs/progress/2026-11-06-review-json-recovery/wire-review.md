---
title: wire-review — reviewer の review.json に誤配置回収を配線し、reviewer prompt に絶対パスの注意を出す
tasks: [01M6S325NYYJS9YWNHDH757E08]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# wire-review — reviewer の review.json に誤配置回収を配線し、reviewer prompt に絶対パスの注意を出す

完了日: 2026-11-06

## 結論
`crates/task-dispatch/src/review.rs` の `run_reviewer_inner` が、review.json を読む直前に
`task_worker::result_location::misplaced_report(REVIEW_FILE_NAME, artifacts_dir, workspace_dir, run_started)` を呼ぶ。
`run_started` は reviewer の adapter を起動する直前の時刻。誤配置 1 件（run 中に書かれたもの）は期待 path へ copy され、
従来どおり parse・verdict の照合にかかる。複数件は `review.json misplaced: …; reports were not adopted` で全基準不合格。
0 件・run 開始より古い 1 件は期待 path が空のままなので、従来どおり `… review.json not found or unreadable` で不合格。
review.json 用の個別の回収経路は作っていない（result.json と同じ関数。ADR D1・D3）。

reviewer prompt を読んで確かめた結果、注意は**出ていなかった**ので直した（ADR D5）。
- reviewer run は `work_dir: None` で、cwd は対象タスクの作業場所。`claude_code::work_dir_note` は `work_dir != workspace`
  の run だけに注意を出すので、reviewer には何も出ていなかった。
- 成果物ディレクトリが workspace の中にあると `RunRequest::artifacts_rel` は相対 `artifacts` を返し、Result 節は
  「`artifacts/review.json` に書け」と相対で指示していた。`repos/<name>` へ cd した後に書くと repo 内に落ちる（01M6QZ031R の形）。
- 直し: `artifacts_rel` は `TaskKind::Review` でも絶対パスを返す（全 harness の Result 節・result.json 指示が絶対パスになる）。
  `work_dir_note(&RunRequest)` は reviewer run にも result.json 側と同じ文面の注意（cwd・成果物ディレクトリ・
  「相対 `artifacts/` はリポジトリの中を指すので使わない」・review.json と result.json の絶対パス・「親ディレクトリを推測した
  相対パスも使わない」）を出す。claude-code と codex が同じ関数を使う。

## 変更
commit（件名。基準は `$CELERIS_WU_BASE`）:
- adr: wire review.json recovery and absolute paths in the reviewer prompt（`agent-docs/adr/2026-11-06-misplaced-report-recovery-generalized.md` に D5 と検証を追記。実装より先）
- wire misplaced review.json recovery into the reviewer and state absolute report paths in its prompt
- progress: review-json-recovery wire-review（clippy の needless_borrow の直しと、この進捗ファイル）

変えたファイル: `crates/task-dispatch/src/review.rs`、`crates/task-dispatch/src/review/tests.rs`、`crates/task-worker/src/protocol.rs`、
`crates/task-worker/src/claude_code/prompt.rs`、`crates/task-worker/src/claude_code.rs`、`crates/task-worker/src/codex.rs`、
`crates/task-worker/src/claude_code/tests.rs`、`crates/task-worker/src/codex/tests.rs`。

## 試験
- `review::tests::a_review_json_misplaced_in_the_repo_is_recovered_and_judged` — reviewer が `repos/agent-platform/artifacts/review.json`
  に書いた判定（c0 pass・c1 fail）がそのまま criteria に採られ、期待 path に copy され、誤配置元は残る。
- `review::tests::several_misplaced_review_jsons_fail_every_criterion` — 2 件は全基準 `review.json misplaced:` で不合格、期待 path は空。
- `review::tests::zero_or_a_stale_misplaced_review_json_keeps_the_not_found_failure` — 0 件と 1 時間前の mtime の 1 件は `not found or unreadable`。
- `claude_code::tests::reviewer_prompt_states_absolute_review_and_result_paths`・`codex::tests::reviewer_prompt_states_absolute_review_and_result_paths`
  — reviewer prompt の冒頭の注意と、Result 節の review.json が絶対パスであること。

修正前に落ちることの確認: review.rs の配線だけを外して（他の変更は残して）`cargo nextest run -p task-dispatch -E 'test(/misplaced|stale_misplaced/)'`
を実行し、4 件中 2 件（1 件回収・複数件）が落ちた（回収なしでは `not found` になる）。配線を戻すと 4 件とも通った。

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` — exit 0、5564 tests run: 5564 passed, 22 skipped
- `cargo clippy --workspace -- -D warnings` — exit 0
- `cargo fmt --all -- --check` — exit 0
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は上の 8 ファイル・ADR・この進捗ファイルだけ

途中で `cargo nextest run -p task-worker` 全体を回したとき、browser の launcher_login / post_login 試験 5 件が並列負荷で落ちた。
同じ試験だけを再実行すると 12 件とも通った。今回の変更は browser のコードに触れていない。

## 本番への移行
本番の config 変更は要らない。新しい config 欄は無く、既存の欄も変えていない。release を promote すれば効く。

## 未解決事項
- pi・acp 等の harness は `work_dir_note` を使わない（冒頭の注意は出ない）。ただし `artifacts_rel` の変更で Result 節の
  review.json・result.json の指示は全 harness で絶対パスになる。

## 提案
なし。
