# ADR: 評価試行の RunRequest の置き場（result.json・run_dir）と終了理由・runs 状態

- Date: 2026-10-14
- Status: Implemented（実装済み）
- Related: [ADR 2026-10-11-llm-source-model-routing-evaluation](2026-10-11-llm-source-model-routing-evaluation.md)（付記 2026-10-12 eval-dispatch §4・§5）、ADR-0041 D1（work_dir）、ADR-0036 D3（artifacts_rel）

## Context

2026-10-14T02:17Z の codex pilot 10 試行（batch 01M4W35QEMQGVV5ZSD0JJ61ZDD）が全部 `eval_trials.status='error'`、`error='run ended as error'` になった。codex は 10 件とも作業を終えていた。原因は評価の仕組みにある。

1. `eval_run_request` が `workspace = <data>/eval/<trial>/repo`、`artifacts_dir = <data>/eval/<trial>/artifacts`（repo の兄弟）、`work_dir = None` で RunRequest を作っていた。`RunRequest::artifacts_rel()` は `work_dir` が無いと `task_core::artifacts::rel_from` を使い、`artifacts_dir` が `workspace` 配下でないので既定の `artifacts` に落ちる。prompt は「`artifacts/result.json` を書け」と相対で指示し、worker（cwd = repo）は `repo/artifacts/result.json` に書く。adapter は `artifacts_dir/result.json` を読むので `Terminal::Error`（"codex exited without artifacts/result.json"）になった。`work_dir` が無いので `adopt_result_json_written_under_work_dir` も働かない。adapter の `run_dir`（`<workspace>/runs/<run_id>`）も repo の中にでき、採点の作業ツリーを汚していた。
2. `on_eval_worker_finished` が `Terminal::Done` 以外を `run ended as <kind>` にし、`Terminal::Error` の message を捨てていた。adapter を `NullSink` で呼ぶので event にも残らない。
3. 同じ関数が Done・Yielded・BudgetExhausted 以外の終端と adapter の `Err` を `runs.status = 'completed'` にしていた。

## Decision

### D1. 評価試行の RunRequest は通常の worker run と同じ形（workspace = 試行の dir、work_dir = repo）

`eval_run_request` は次の形で RunRequest を作る。harness ごとの分岐は作らない。

- `workspace = <data>/eval/<trial_id>`（試行の dir。`Task.workspace` の `Local.path` も同じ）
- `work_dir = Some(<data>/eval/<trial_id>/repo)`（`git worktree add --detach` した作業ツリー。worker の cwd）
- `artifacts_dir = <data>/eval/<trial_id>/artifacts`

これで `artifacts_rel()` は `work_dir` がある経路（ADR-0041 D1）に入り、`artifacts_dir` の絶対パスを返す。prompt が示す result.json の場所と adapter が読む `artifacts_dir/result.json` が一致する。prompt には `work_dir_note`（cwd と成果物 dir の注意）も入る。worker が相対 `artifacts/result.json` に書いても `adopt_result_json_written_under_work_dir` が `artifacts_dir` へ移す。

adapter の `run_dir`（`<workspace>/runs/<run_id>`）は `<data>/eval/<trial_id>/runs/<run_id>` になり、作業ツリー（repo）の外に出る。採点（hidden check）は従来どおり `eval_trials.worktree_path`（= repo）で走る。

codex との整合: cwd は `req.cwd()` = repo。fresh `exec`（評価 run は常に新 session）は `--add-dir <artifacts_dir>` を足すので、作業ツリーの外の `artifacts_dir` に書ける。`git_writable_roots` は cwd（repo）の gitdir を足す。claude-code も `req.cwd()` で起動し、同じ `artifacts_rel()` を prompt に使う。

旧い置き場（`workspace = repo`、`work_dir = None`）の経路は残さない。

### D2. 終了理由の本文を `eval_trials.error` と daemon log に残す

`on_eval_worker_finished` は Done 以外の終端の本文を `eval_trials.error` に書く。形は `run ended as <kind>: <本文>`（`<kind>` は `terminal_kind`）。本文は Error・BudgetExhausted は `message`、Question は `text`、Yielded は checkpoint の JSON、Waiting は summary（空なら job id の列）。adapter の `Err` は従来どおり `adapter: <e>`。

どの終わり方も daemon log に `trial_id`・`run_id`・`kind`・`reason` つきで warn を出す（`run ended as error` だけの行にしない）。

### D3. runs 行の状態は終端どおり。usage は取れれば残す

`runs.status` は終端から決める: Done → completed、Yielded → yielded、BudgetExhausted → budget_exhausted、Question → question、Waiting → waiting、Error → failed、adapter の `Err` → harness_error、dispatcher が止めた run（`abort_eval_runs`）→ cancelled。Error・adapter の失敗は completed にしない。

`runs.usage` は終端が usage を持てば（Done・Yielded・BudgetExhausted・Waiting）書く。Error・Question は protocol 上 usage を持たないので空のまま。

## Consequences

- 2026-10-14 の pilot 10 試行は記録を消さずに残る。やり直しは別 unit（trial_no をずらす正規手順）で行う。
- 集計から error・cancelled の試行を除くのは aggregate 側の変更（同じ task の別 WorkUnit）。
- 試験: `crates/task-dispatch/src/dispatcher/tests/eval_trial_dispatch.rs` の `eval_trial_result_at_artifacts_rel`（`eval_run_request` が作った RunRequest をそのまま使い、`artifacts_rel()` の位置へ result.json を書く scripted worker で trial が scoring を経て passed まで進む。run_dir が repo の外）と `eval_trial_error_keeps_reason`（`Terminal::Error` の message が `eval_trials.error` に残り、`runs.status` が failed）。
