# result.json の誤配置を決定的に回収する（worker と reviewer 共通）

- 日付: 2026-10-18
- 状態: 実装済み
- tasks: [01M56VQ7ME3DSXXM34DFSBRNBV]

## 背景
01M54Y3MRR は誤配置を「permanent error」で検出したが、回収はしなかった。実例: Pairs PRA-30 の reviewer run
（repo 内 `artifacts/result.json`、中身は全基準 pass）と PRA-31c の worker run（`repos/artifacts/result.json`）が
`result.json misplaced … reports were not adopted` で failed になった。reviewer も worker も同じ
`misplaced_result`（`crates/task-worker/src/result_location.rs`）を通る。

## 決定
- D1: `misplaced_result(artifacts_dir, cwd, since)` は、期待 path に無く、探索範囲（artifacts の親と cwd）に
  `result.json` がちょうど 1 件あり、その mtime が run 開始（2 秒の slack。mtime は粗いため）以後なら、
  それを期待 path へ copy して `None`（誤配置なし）を返す。以降は通常の検証が読む。誤配置元は残す。
- D2: 0 件は従来どおり。複数件・古い 1 件は曖昧なので従来どおり permanent error（推測しない）。
- D3: 呼び出し元（claude_code・codex・dispatcher の run_worker）は run 開始時刻を渡す。
- D4: `artifacts_rel()` は artifacts が workspace の外にあるとき絶対 path を prompt に出す（相対 `artifacts/` が
  cwd 直下を指して誤配置になるのを避ける）。

## 検証
`result_location` の単体試験（1 件・複数件・古い 1 件）、`a_reviewer_result_written_in_the_repo_is_recovered`、
既存の誤配置拒否試験は 2 件の stray（曖昧）に直した。

## 付記（WU worktree）
WU の worktree（`wu/<key>/repos/artifacts/result.json`）の誤配置も同じ探索範囲（artifacts の親以下を再帰）に入るので、D1 で回収される。
