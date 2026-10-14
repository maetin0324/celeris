---
tasks: [01M4W3PTCHJD4XQPZGT81WBTRM]
status: done
completed: 2026-10-14
---
# 評価試行の result.json 置き場・終了理由・runs 状態の修正（WorkUnit fix-run）

設計判断は [ADR 2026-10-14-eval-trial-result-placement](../../adr/2026-10-14-eval-trial-result-placement.md)。
既存 ADR 2026-10-11-llm-source-model-routing-evaluation の付記 eval-dispatch §4・§5 も同じ内容に書き換えた。

## 変更

- `crates/task-dispatch/src/dispatcher/eval_trials.rs`
  - `eval_run_request(trial, trial_dir, …)`: workspace = `<data>/eval/<trial_id>`、work_dir = `Some(<…>/repo)`、
    artifacts_dir = `<…>/artifacts`（`Task.workspace` も試行の dir）。`artifacts_rel()` は絶対パスになり、prompt の
    result.json と adapter が読む場所が一致する。run_dir（`<workspace>/runs/<run_id>`）は作業ツリーの外。旧い形
    （workspace = repo、work_dir = None）は残していない。harness ごとの分岐は無い。
  - `on_eval_worker_finished`: Done 以外は `run ended as <kind>: <本文>` を `eval_trials.error` に残し、daemon log に
    `trial_id`・`run_id`・`kind`・`reason` つきで warn を出す。adapter の `Err` も同様に log する。
  - runs 行: Error → failed、adapter `Err` → harness_error、Question → question、Waiting → waiting、
    abort → cancelled。usage は終端が持てば（Done・Yielded・BudgetExhausted・Waiting）残す。
- codex との整合: cwd = `req.cwd()` = repo、fresh exec が `--add-dir <artifacts_dir>` を足す（codex.rs）、
  `git_writable_roots` は cwd の gitdir を足す。claude-code も `req.cwd()`・`artifacts_rel()` を使う。
- 試験（`crates/task-dispatch/src/dispatcher/tests/eval_trial_dispatch.rs`）:
  - `eval_trial_result_at_artifacts_rel`: `eval_run_request` が作った RunRequest をそのまま受け取る scripted worker が、
    実際の worker prompt（`build_prompt`）が示す `artifacts_rel()/result.json` に cwd から書き、adapter と同じく
    `artifacts_dir/result.json` を読む。trial は scoring を経て passed、runs は completed、run_dir と artifacts_dir は作業ツリーの外。
  - `eval_trial_error_keeps_reason`: `Terminal::Error` の message が `eval_trials.error`（`run ended as error: <message>`）に残り、
    runs.status は failed（completed でない）。

## 証拠

- `cargo test -p task-dispatch --lib eval_trial` → exit 0、6 passed（新しい 2 本を含む）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、`5082 tests run: 5082 passed, 12 skipped`、
  CELERIS_TEST_SUMMARY passed 5085 / failed 0（userns: null）

## 未解決事項

- 2026-10-14 の pilot 10 試行の記録はそのまま残る。やり直しは WorkUnit redo（trial_no をずらす手順）。
- 集計から error・cancelled を除くのは WorkUnit aggregate。

## 提案

- なし
