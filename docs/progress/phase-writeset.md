---
tasks: [01M3YE0JTQEYBFDTV3HCHR4G9J]
---
# Phase 5 検証記録 — expected/actual write-set と target behind 指標

完了日 2026-10-02（verify WorkUnit、ADR-0130）。実装済みの Phase 5 を workspace 全体で検証し、architecture map に参照を追加した。コード機能の変更はない。

- 実装案内: `task-core::write_set` は hint の正規化・重なり判定、`task-core::store::write_sets` は実績永続化を担う。Dispatcher は `write_set_gate` で同一 repo の強い重なりを待機させ、`write_set_record` で Git 差分を記録する。`behind_target` が target との差分と age を観測し、`stale_priority` が長期 stale task の review 前 sync を優先する。API の TaskDetail / execution metrics と GUI task 詳細にも公開している。詳細は [ADR-0130](../adr/0130-write-set-parallelism-and-behind.md) D1–D5。
- 着手時の ref: `main` と `origin/main` はともに `faa20195521f6edae6f508d7523b4ec97837cf12`、HEAD は `0ca05217640c44d3bd625c660ebabe2a27d779cd`。fetch は行わず、登録済み ref を使った。
- 衝突見積もり: `git merge-tree --write-tree --name-only HEAD main` → exit 1。衝突対象は `crates/task-api/src/query.rs`, `crates/task-api/src/query/tests.rs`, `crates/task-core/src/cluster_job/tests.rs`, `crates/task-core/src/store/migrations.rs`, `crates/task-core/src/store/tests.rs`, `crates/task-ops/src/delivery.rs`, `crates/task-ops/src/delivery/tests.rs`, `crates/task-worker/tests/browser_shared_cdp.rs`, `docs/PROGRESS.md`, `gui/app/routes/inbox.tsx`。自動 merge 候補も複数あり、最終 merge 時に確認が必要。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（warning なし）。
- `cargo test --workspace` → exit 101。通常 fail-fast 実行では `crates/celeris/tests/instance_handoff.rs` が 8 件中 3 passed / 5 failed。失敗名: `starting_the_same_release_twice_exits_three`, `normal_mode_does_not_inject_the_smoke_builtins`, `verify_mode_never_dispatches_and_never_touches_daemon_instances`, `a_newer_release_takes_over_while_the_old_one_finishes_its_run`, `a_stale_heartbeat_promotes_the_standby`。最初の3件は worker DB guard の user namespace probe が `Operation not permitted` で起動を拒否、残り2件は sandbox で handoff dispatch / standby 待機 assertion に失敗。
- 全範囲を採るための `cargo test --workspace --no-fail-fast` も exit 101。24 targets が失敗: `celeris/instance_handoff`; e2e の `account_pool_scenarios`, `api_scenarios`, `cluster_scenarios`, `codex_account_pool_scenarios`, `delegation_scenarios`, `multi_account_scenarios`, `phase7_scenarios`, `plan_scenarios`, `provider_admin_scenarios`, `scenarios`, `worker_db_read_only`; task-api の `browser_h3_injection`, `browser_restore_deliver`, `browser_restore_live_session`; task-worker の `--lib`, `browser_cdp_sink`, `browser_egress_relay`, `browser_h3_wire`, `browser_injection_attacks`, `browser_injection_wire`, `browser_restore_deliver`, `browser_runtime_isolated`, `browser_runtime_supervisor`。明示ログ上、instance handoff / e2e は user namespace probe の `Operation not permitted`、browser runtime 系は `unshare: Operation not permitted` または `NoChildPid` により失敗した。既知の sandbox 制約として扱い、試験・実装は変更していない。
- 実装に近い試験は no-fail-fast run で通過: task-core 657/657、task-dispatch 557/557（`write_set_record` を含む）、task-ops 398/398（TaskDetail snapshot 含む）、task-api lib 76 passed / 2 ignored（`write_set_can_be_created_updated_and_read_through_task_api` を含む）。dispatcher test output に write-set record 専用試験の pass を確認した。clippy/fmt は独立に exit 0。
- 未解決事項: workspace 全体の test 合格は sandbox の namespace 制限と handoff 待機失敗のため確認できていない。通常権限の stage 統合または final review で再実行する。
- 提案: 最新 main との統合時に、上記の query/store/delivery/browser/GUI/PROGRESS 衝突を各担当変更を保って解消した後、fmt/clippy/test を再実行する。
