---
title: dispatcher の reviewer 必須の条件を repo の欄にする
tasks: [01M7CFRYZH0AW00M3WAHXX8B57]
status: done
updated: 2026-11-14
---
# dispatcher の reviewer 必須の条件を repo の欄にする

完了日: 2026-11-14

基準は `$CELERIS_WU_BASE`。開始時点で、`review_spawn.rs` の reviewer 必須の判定は
`task_ops::delivery::auto_integrates`（判定が真、かつ support task でも木の子でもない）を使っており、
案件単位の判定は task-dispatch に残っていなかった（`git grep` で案件の auto_integrate の参照 0 件）。
dispatcher/tests 以下の試験も `ProjectRepo.auto_integrate`（repo の欄）で組まれていた。
この WorkUnit では確認に加え、足りなかった試験を足した。

## 追加した試験（`dispatcher/tests/target_sync.rs`）
- `reviewer_run_is_required_only_for_an_enabled_repo`: 1 件の repo が無効なら reviewer run が起きず、有効なら起きる。
  守る不変条件は「reviewer 必須は task の repo の欄で決まる」。既存の試験は `DeliverySkipped` の理由しか見ず、
  reviewer run の有無（`WorkerStarted{role: Reviewer}`）を確かめていなかった。
- `partially_enabled_multi_repo_task_does_not_require_a_reviewer_run` に reviewer run が起きない断言を追加（複数 repo の一部が無効）。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-dispatch target_sync` → 33 passed
- `bash scripts/dev/test-affected.sh` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## check の不合格への対応（2 回目の run）
- `cargo clippy -p task-dispatch --all-targets -- -D warnings` が既存の試験コード 7 件で落ちていた。
  `gate.rs`（`OnceLock` を `cfg(not(test))`、不要な deref）、`disk_forecast.rs`、`dispatch_ready_phases.rs`、
  `health_enforce.rs`・`cos_chat_root_done.rs`（type 別名）、`cos_slot_wait.rs` を直した。挙動は変えていない。
- `cargo test -p task-dispatch` は 1 回目に 2 件落ちた（`build_cache`・`cron_jobs`）が、単独では通り、再実行の全体でも 1762 passed で通った。負荷下の揺れとみる（原因は未調査）。

## 証拠（提出時の HEAD で実行）
- `cargo clippy -p task-dispatch --all-targets -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `cargo test -p task-dispatch` → 1762 passed, 0 failed

## 未解決事項
- 上記 2 件の試験の負荷下の揺れ（未調査）。
