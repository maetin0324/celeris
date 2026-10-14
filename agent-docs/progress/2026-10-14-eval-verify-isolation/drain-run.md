---
title: "handoff-eval / drain-run: drain が評価 run を待つ・取り残し eval runs 行の照合"
tasks: [01M4WYKAQTQSKBNB2P341MMHCY]
status: done
updated: 2026-10-14
---
# drain-run（ADR eval-verify-isolation D3.2・D3.3・D3.5 後半・D3.6）

- `Dispatcher::in_flight` に `eval_runs.len()` を足した。旧 instance は評価 run の完了まで draining に留まる。
- `on_eval_worker_finished` は draining（`accepting_new_work = false`）なら `runs` 行を閉じて試行を `scoring` に
  するだけで採点しない。採点は新 active の `reconcile_eval_trials`。
- `reconcile_ownerless_runs` の role `eval` 除外を `reconcile_ownerless_eval_run` に置換: 手元に無い running の
  eval 行は、試行が無い・running でない、または持ち主が生きていない（`instance_alive` / NULL は `holder_gone`）
  とき `cancelled` で閉じる。
- `abort_all_runs`（DrainTimedOut）は `abort_eval_runs` を呼ぶ。`interrupt_runs_on_shutdown` は不変。

## 証拠
- `cargo test -p task-dispatch live_handoff_keeps_running_eval_trial` / `eval_run_row_not_left_running_after_handoff`: 各 1 passed
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0
- `bash scripts/dev/test-parallel.sh`: exit 0、5103 passed / 0 failed

## 未解決事項
- なし
