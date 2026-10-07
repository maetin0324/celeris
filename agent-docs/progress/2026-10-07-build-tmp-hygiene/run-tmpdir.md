---
title: 葉 run-tmpdir — worker run ごとの TMPDIR と終了時の片付け、前置きの指示（ADR 2026-10-07-build-tmp-hygiene D2）
tasks: [01M4B60FFMGV5F888BXAJ0841M]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 run-tmpdir

## 実装（D2）

- `crates/task-worker/src/run_tmpdir.rs`（新規）: `RunTmpDir::create(workspace, run_id)` が
  `<workspace>/runs/<run_id>/tmp/` を `0700` で作る（前の attempt の残りは空にしてから）。`env()` は
  `TMPDIR`・`TMP`・`TEMP` をその path に。`remove()` は `tmp/` だけ消す（`runs/<run_id>/` のログ・`result.json` は残る）。
  `Drop` でも消すので、`?` の早期 return・future ごとの中断（cancel・takeover）でも残らない。
  `sweep_stale(workspace, is_live)` は `runs/*/tmp` のうち live でない run のものを消す補助。
- `crates/task-dispatch/src/dispatcher/worker_task.rs`: `run_worker` がローカルの host run（Remote・コンテナは範囲外、D2.1）で
  `RunTmpDir` を作り、`RunAdapterPrep.run_tmp_env` で adapter に重ねる（WU の env の後、コンテナの前）。
  run の終端（成功・失敗・timeout・auto-leaf の停止を含む adapter の戻りの後、remote push の後）で `remove`。
  作れない・消せないときは `tracing::warn` だけで run は止めない。
- D2.2 の取りこぼし回収: `task_worker::workspace_prune::prunable_paths` に終端タスクの `runs/*/tmp` を足した
  （既存の `prune_one_workspace` の tick が `workspace_prune_after_secs` 経過後に消す。ログ・`result.json` は残す）。
- D2.3 前置き: `preamble::run_tmpdir_note()`（常に出る）「リポジトリの写し・pnpm store の写し・ビルド出力・大きな一時 file は
  `/tmp` に置かず、`$TMPDIR`（run 終了で消える）か作業場所の下に置く。残したい物は `artifacts/` に置く」。
- 既存試験の更新: 空 context の前置きのバイト比較（`preamble/tests.rs` 7 か所・`claude_code/tests.rs` 1 か所）に新しい節を足した。
  `build_cache::scratch_runs_get_target_and_cargo_tuning_but_no_sccache` の「cargo env の後は `CELERIS_*` だけ」に
  `TMPDIR`・`TMP`・`TEMP` を許した。`browser_fallback` の `RunAdapterPrep` 直組み 3 か所に `run_tmp_env: None`。

## 試験（run_tmpdir_）

- task-dispatch: `run_tmpdir_env_is_passed_to_a_local_run`・`run_tmpdir_removed_when_the_run_succeeds`・
  `run_tmpdir_removed_when_the_run_fails`（dispatcher を通した run。adapter が TMPDIR の下に写しを作り、終了後に消えている）。
- task-worker: `run_tmpdir_env_points_to_run_dir`（0700・3 変数）・`run_tmpdir_remove_keeps_run_records`・
  `run_tmpdir_removed_on_drop_when_the_run_is_abandoned`・`run_tmpdir_create_clears_a_previous_attempt`・
  `run_tmpdir_sweep_stale_removes_only_terminal_runs`・`run_tmpdir_preamble_forbids_tmp_copies`・
  `workspace_prune::tests::run_tmpdir_leftovers_of_a_terminal_task_are_prunable`。

## 証拠

- `cargo test --workspace run_tmpdir_` → exit 0、run_tmpdir_ の ok 10 件（task-dispatch 3・task-worker 7）。
- `bash scripts/dev/test-parallel.sh` → exit 0、passed 4671 / failed 0 / ignored 14。
- `cargo clippy --workspace -- -D warnings` → exit 0。

## 未解決事項

- D2.2 の警告 event `RunTmpCleanupFailed` は入れていない（Event の variant 追加は schema 再生成と web の
  `event-kinds.ts`・`invalidation-map.ts` の更新が要り、この葉の範囲を超える）。今は `tracing::warn` と、終端タスクの
  workspace prune での回収。prune は `workspace_prune_after_secs = 0` だと走らない。
- ADR は「scratch_gc の tick が拾い直す」と書くが、実装は `prune_one_workspace`（終端タスクの作業場所の刈り込み）に載せた。
- D2.4（harness が TMPDIR を `/tmp` に戻さないことの adapter ごとの試験）は未着手。grep では claude-code・codex・acp・pi の
  起動は `TMPDIR` を設定・除去しない（`env_clear` は browser の runtime/credential だけ）。
- D2.1 の「sandbox（bwrap 等）で bind」は不要だった: db_guard の namespace は workspace を覆わない。

## 提案

- `RunTmpCleanupFailed` を D4（disk 系の event / 通知）を足す葉でまとめて入れる。
- ADR の D2.2 に「回収は workspace prune（終端タスク）に載せた」付記を close-out 葉で書く。
