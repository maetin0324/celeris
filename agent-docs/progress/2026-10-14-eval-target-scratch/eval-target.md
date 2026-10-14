---
title: 葉 eval-target — 評価試行に試行ごとの scratch CARGO_TARGET_DIR を渡し、終端で消す
tasks: [01M4WHYHVZTYNBRGM5C3PWHMZE]
status: done
updated: 2026-10-14
completed: 2026-10-14
---
# 葉 eval-target

## 確かめたこと
- 変更前の `eval_run_request`（`crates/task-dispatch/src/dispatcher/eval_trials.rs`）は `cargo_target_dir: None` で、
  `start_eval_trial` も adapter に scratch の env を重ねていなかった（評価試行は `run_worker` を通らないため）。

## やったこと（ADR 2026-10-14-eval-trial-scratch-target D1 に実装の具体を書いた）
- owner は `agent-eval-<trial_id>`（試行ごと。tasks 行が無いので task owner にすると GC が走行中に回収する）。
  lease の TTL = 1800 + check_timeout + 600 秒（daemon が落ちたときの回収）。
- `start_eval_trial`: scratch が無効なら作業ツリーも作らず `error`。作業ツリーの後に lease を作り（adopt なし、seed は通常どおり）、
  `scratch_cargo_env` を adapter の `with_env` で重ねる（`with_env` 不可の adapter は `error`）。`RunRequest.cargo_target_dir` に同じ path。
  起動途中の失敗は作業ツリーと target を消す。
- `run_eval_check` は env を受け、hidden check は同じ `CARGO_TARGET_DIR` で走る（前に lease を touch）。
- 後始末 `remove_eval_trial_files`（作業ツリー + target 削除 + lease の `released_at`）を採点の後・`fail_and_cleanup_eval_trial`
  （adapter 失敗・done 以外の終端・回復）で呼ぶ。`abort_eval_runs`（停止）も target を消す。
- 試験（`crates/task-dispatch/src/dispatcher/tests/eval_trial_dispatch.rs`）:
  `eval_run_request_sets_per_trial_cargo_target_dir`・`eval_trial_target_removed_after_scoring`・
  `eval_trial_check_uses_trial_target_dir`・`eval_trial_target_removed_on_recovery`・`eval_trial_fails_without_scratch`、
  `eval_trial_error_keeps_reason` に異常終了後の削除の確認を追加。既存の stub に `with_env` を足し、scratch を有効にした。
- daemon e2e `crates/celeris/tests/routing_eval_run.rs` に `[scratch] dir = "scratch"` を足し（既定の scratch は NFS の home 配下で無効になる）、
  試行の target が消え lease が released であることを確かめる。

## 証拠
- `cargo test -p task-dispatch --lib -- --exact dispatcher::tests::eval_trial_dispatch::<名前>`（上の 5 本 + error_keeps_reason）: 各 `1 passed`
- `cargo nextest run -p task-dispatch eval_`: 14 passed
- `cargo nextest run -p celeris --test routing_eval_run`: 1 passed
- `bash scripts/dev/test-parallel.sh`: exit 0、5089 passed / 12 skipped
- `cargo clippy --workspace -- -D warnings`: exit 0、`cargo fmt --all -- --check`: exit 0

## 本番への移行
- 本番の `[scratch]` が有効（`/local/celeris/data/scratch`、ADR-0136）であること。無効・NFS 上だと評価試行はすべて `error`
  （"scratch is disabled; …"）になる。config の書き換えは不要。

## 未解決事項
- なし（`/var/tmp/agent-platform-build` の残り経路は葉 var-tmp-audit）。

## 提案
- なし。

## 2026-10-14 retry: 範囲 check の不合格（plan_issue）
- 範囲 check が `crates/celeris/tests/routing_eval_run.rs` を範囲外として落とした。
- この試験の config は `[scratch]` を書かず、既定の scratch dir が `~/.local/celeris/scratch`（NFS）になるため起動時に scratch が無効化される。目的どおり「scratch 無効なら試行を失敗にする」ので、試験に `[scratch] dir = "scratch"` を足さないと `daemon_runs_two_stub_trials_with_hidden_checks_and_eval_records` が落ちる（変更を戻して実行し確認: `cargo test -p celeris --test routing_eval_run` → FAILED、error "cargo target: scratch is disabled; …"）。
- 計画の範囲に `crates/celeris/tests/routing_eval_run.rs` を加える必要がある（plan_issue で申告）。
