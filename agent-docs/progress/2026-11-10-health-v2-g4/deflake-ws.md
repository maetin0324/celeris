---
title: 健全性v2 G4 deflake-ws — git_workspace の review の試験を tick の回数でなく状態で待つ
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: done
updated: 2026-11-11
---
# 健全性v2 G4 deflake-ws — git_workspace の review の試験を tick の回数でなく状態で待つ

完了日: 2026-11-11

`crates/task-dispatch/src/dispatcher/tests/git_workspace.rs` の review の結果を読む 9 試験の待ちを、tick の回数の上限から ADR-0125 (b) の出来事待ちに直した。本体のコード・検査器・policy・baseline は変えていない。基準は `$CELERIS_WU_BASE`。

## 症状
段 hardening の統合（2 回目）の test-affected で `dispatcher::tests::git_workspace::review_records_each_repositories_own_definition_and_worktree_results` が 11.3 秒で落ちた。panic は `git_workspace.rs` の `find_map(...).expect("each repository must have its own evidence")`。log は task の workspace の `integration-checks/integrate-hardening/` の 7 番目の check の log。

## 実装前 brief
1. 根本原因: 試験は `run_until_idle(&mut d, 60)` で待つ。この関数は 60 tick（各 tick の後に 20 ms の sleep）で idle にならなくても黙って戻る。`Dispatcher::is_idle` は `reviewing` が空でない間は false なので、idle が早く来たのではなく、tick の回数が尽きて待ちが終わる。review の必須検査（repo ごとの `workspace.toml` の check と repo health の項目）は tick の外の task で走るので、負荷で遅いと `RequiredChecksEvaluated` が書かれる前に試験が event を読む。症状は `find_map` の `expect` に出る。
2. 責務 owner: 試験の待ち（`git_workspace.rs`）。本体の review の経路は正しく動いている（待てば evidence は揃う）。
3. 既存の再利用先: `dispatcher/tests/mod.rs` の `run_until_state` と `STATE_WAIT_GUARD`（60 秒の壁時計の保険）、`Status::is_terminal`。
4. 選択肢: (a) tick の上限を増やす。負荷次第でまた足りなくなるので採らない（ADR-0125 の共通規則）。(b) `mod.rs` の `run_until_task_terminal` を使う。終端で `Done` を assert するので、`second_passes` が false の回（Done にならない）と `the_repository_check_commands_are_the_reviewers_default` の落ちる task に使えない。(c) `run_until_state` で「task が終端の状態」と追加の条件を待つ helper を試験の file に置く。(c) を採った。
5. 新しい状態・分岐・fallback・port: 無し。試験の file に helper `run_until_terminal_and`（終端の状態と追加の条件）と `run_until_terminal`（終端の状態だけ）を足した。保険を超えたら最後の状態と event を出して落とす。
6. 撤去対象: 下の 9 試験の tick の回数の上限の待ち。`run_until_idle` 自体は他の試験が使うので残す。
7. 不変条件: 試験の期待（evidence の件数・cmd・pass・指紋・Done の真偽、verdict の件数と内容、Done・artifact・progress の行）はそのまま。diff で消えた `assert` の行は 0。

## 負荷を掛けない再現と結果
- 方法: 対象の試験の `test -f first-only`（`workspace.toml` の check と、期待の cmd の 2 か所）を一時的に `sleep 3 && test -f first-only` にした。60 tick の待ちは約 1.2 秒と tick の時間で終わるので、3 秒の check の結果が出る前に戻る。CPU の負荷は掛けていない（host の load average は他の job で 16〜62）。
- 修正前: `cargo nextest run -p task-dispatch -E 'test(review_records_each_repositories)'` は exit 100。1.785 秒で落ち、panic は同じ行の `each repository must have its own evidence`。
- 修正後（`sleep 3` のまま）: 同じコマンドが exit 0。1 passed、7.299 秒（`second_passes` が true と false の 2 回とも）。
- 同じ形に直した試験: `test -f in-tree` と `test -f left-behind` を一時的に `sleep 3 && …` にして `cargo nextest run -p task-dispatch -E 'test(git_workspace)'` を実行し、exit 0、24 passed。修正前の版でこの 2 つの check に sleep を入れた実行はしていない。
- 一時的な sleep は戻した（`grep -c 'sleep 3' crates/task-dispatch/src/dispatcher/tests/git_workspace.rs` は 0）。

## 変更
- `phase 1: wait for terminal state and evidence in git_workspace review tests`
  - `review_records_each_repositories_own_definition_and_worktree_results`: task が終端の状態になり、両方の repo の `RequiredChecksEvaluated` が store にあるまで待つ。
  - 終端の状態を待つ形に直した 8 試験（review の必須検査の結果の後の `Done`・verdict・片付けを読む）: `a_local_git_workspace_runs_in_a_per_task_worktree_on_its_own_branch`、`git_worktree_task_registers_report_md_as_an_artifact`（終端の後に `run_until_idle` で idle を確かめる assert は残した）、`a_dirty_worktree_is_kept_and_a_progress_line_is_appended`、`a_compaction_task_does_not_get_a_worktree_even_when_its_project_has_a_primary_repo`、`a_host_task_still_runs_when_no_container_runtime_is_available`、`the_repository_check_commands_are_the_reviewers_default`（2 つの task の両方）、`a_parallel_unit_runs_in_its_own_worktree_with_its_own_artifacts`、`a_v1_plan_stays_serial_without_work_unit_worktrees`。
- 変えていない tick の上限の待ち: `the_worktree_branches_from_the_current_release_when_it_is_ahead_of_main`（branch だけを読む）、`cancelling_a_task_removes_its_worktree_and_branch`（worker の質問で Blocked）、`a_container_task_is_blocked_with_a_question_when_no_runtime_works`（run の前に Blocked）、`shared_mode_keeps_the_repository_itself_as_the_working_directory`・`a_local_path_that_is_not_a_git_repository_is_unchanged`（worker の呼び出しの記録だけを読む）。どれも review の必須検査の結果を待たないので、この unit の範囲の外。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh`: exit 0（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。nextest 3737 tests run、3737 passed、11 skipped。Python の unittest 94 件 OK・expected failures=2、check-health の台本 7 件 OK）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更 path は `crates/task-dispatch/src/dispatcher/tests/git_workspace.rs` とこの進捗 file。

## 未解決
- 上の「変えていない tick の上限の待ち」の 5 試験は、worker の run の結果を tick の回数で待つ。今回の失敗の経路ではないので変えていない。
- この run の `CARGO_TARGET_DIR` は seed から作られていて、v8 の build script の出力が消えた seed の世代の path を `-L` に指していた（`could not find native static library rusty_v8`）。run の target の中で `cargo clean -p v8` をして build し直した。
