---
tasks: [01M3Z08A0T81ZQ60XVR62XJMPD]
---

# /local hot データ移行: 工程 follow 統合後の再検査（follow-recheck）

対象 task: 01M3Z08A0T81ZQ60XVR62XJMPD（ADR-0136 local-hot-data-layout）。検査日: 2026-10-03。
対象 HEAD: e2162ecc（integrate wu/runbook、phase follow。guard-paths・migrate-script・runbook 統合済み）。
本 WorkUnit のブランチはこの HEAD まで fast-forward 済み（merge-base(e2162ecc, 開始時 HEAD d2ccae4d) = d2ccae4d で祖先関係、衝突なし）。
各コマンドは 1 回だけ流した（流し直しなし・CPU を焼く負荷なし・本番 host 操作なし・外部ネットワークなし）。コード変更はしていない（全検査が初回で pass したため）。

## follow 段の葉の checks

- `cargo test -p task-worker`: pass（exit 0、714 passed / 0 failed / 4 ignored、統合テスト含め全 binary pass）
  - `claude_code::tests::planner_prompt_declares_production_host_changes_as_a_human_procedure` ... ok
  - `preamble::tests::production_host_changes_are_declared_as_a_human_procedure` ... ok
  - `db_guard` 系 16 件 ... 全 ok
  - `browser_launcher_ptrace::launcher_chrome_denies_daemon_uid_ptrace` ... ok（host launcher は launcher-skew 対応済みで版ずれなし）
- `cargo clippy --workspace -- -D warnings`: pass（exit 0、警告なし）
- `cargo fmt --all -- --check`: pass（exit 0）
- `for t in scripts/selfdeploy/tests/*.sh; do bash "$t"; done`（全 12 本）: 全て pass（exit 0）
  - install_units_hot_dir.sh / local_state_layout.sh / migrate_to_local_test.sh / pid_resolution_test.sh / prepare_timeout_test.sh / promote_authorization_marker.sh / promote_web_follows_release.sh / release_gui_skip_and_shared_tree.sh / release_parallel_test_gate.sh / release_uses_scratch_lease.sh / release_web_stage_nonblocking.sh / verify_durations_and_parallel.sh
- 手順書の grep（`docs/ops/local-hot-data-migration.md` に `ADR-0136` がある）: pass

## cargo test --workspace --no-fail-fast（全体）

- 結果: pass（exit 相当 — ログに `test result: FAILED` 0 件、全 125 の test result ブロックが `ok`）
- 件数: 3337 passed / 0 failed（ignored 分は内訳に含めず集計、個別 crate ログに記載）
- 落ちた試験: なし（今回の 1 回の実行では decisive failed も flaky も再現しなかった）

## integrate-follow の integration_failed（2026-10-03 00:51〜01:00、理由未記録）について

今回の再検査では、follow 段の変更（`crates/task-worker` の `db_guard.rs`・`db_guard_tests.rs`・`preamble.rs`・`preamble/tests.rs`・`claude_code/prompt.rs`・`claude_code/tests.rs`、`crates/celeris/src/daemon/bootstrap.rs`、`scripts/selfdeploy`、`docs/ops`）に起因する決定的な失敗は 1 件も見つからなかった。過去に記録されている負荷時 flaky（`tests/e2e` の `api_scenarios` 2 件、`browser_launcher_ptrace` の host launcher 版ずれ）もこの実行では発生せず、すでに core 段の `e2e-stable`・`launcher-skew` で対処済みの状態のまま安定している。

integration_failed の理由ログが残っていないため確証はできないが、run sandbox 内の一過性の負荷（並行 cargo・userns 制約など）による環境要因であって、コードの不具合ではないと判断する。範囲内の修正は不要だった。

## 結論

follow 段の checks（0 項目）はすべて通り、`cargo test --workspace --no-fail-fast` も全 pass だった。コード変更は行っていない。
