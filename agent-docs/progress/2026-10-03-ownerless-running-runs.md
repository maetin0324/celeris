---
title: 持ち主の居ない running の run 行の回収（ADR 2026-10-03-ownerless-running-runs）
tasks: [01M41M32MWB3AEQ0QP7C23Q3CN]
status: done
updated: 2026-10-03
---

# PROGRESS — 持ち主の居ない running の run 行の回収（ADR 2026-10-03-ownerless-running-runs）

正本: [ADR 2026-10-03 lease を持たない run の行を running のまま残さない](../adr/2026-10-03-ownerless-running-runs.md)。

## Phase 1（完了 2026-10-03、単一 phase）

本番 2026-10-03 の reviewer run `01M4129NX8R264QXMQVEZRDKHM`（14:22 開始、旧 release `32b19f45d8a9`、16:01 の `0225c752ef0c` へのライブ引き継ぎ後も `runs.status='running'` のまま）と、人の割り込みで追加された 2 例目 `01M41EKM2QGD4T65F8ZDS4RFF7`（17:57 開始、同じ instance の中で残った）を直した。

### 分かったこと

- 2 例目は本番 DB（読み取りのみ）の events で確認: `[1274] worker_started{01M41EKM…, role: reviewer}` の後、`[1275] transitioned{reviewing → ready, review_fail}` と `review_verdict` 5 件（criterion 4 は「not evaluated: a deterministic check failed」）。判定の `run_id` はレビュー対象の最後の WU の worker run `01M417R6…` で、reviewer run は起動されていない。つまり引き継ぎと無関係な **経路 A**（決定的 check が落ちて reviewer を起こさず、用意した run 行を `on_review_finished` が閉じない）。
- 残る経路は 3 つ（A: 同じ instance、B: SIGTERM/SIGINT の停止がレビューを閉じない、C: crash 等で誰も照合しない）。どれも lease を持たない reviewer run で起きる。

### 直したこと（ADR D1〜D5）

- D1 `review_verdict.rs`: 用意だけして起動しなかった reviewer run を review の完了で閉じる（quota 解放 + `WorkerFinished{role: reviewer, end: cancelled}`）。
- D2 `leases.rs`: `interrupt_runs_on_shutdown` が手元のレビューも止め、reviewer run を閉じる（task は `reviewing` のまま、次の active が `recover_reviews` でやり直す）。
- D3 `dispatcher/ownerless_runs.rs`: `reconcile_ownerless_runs` が active の最初の tick と 600 秒ごとに、手元に無く lease も持たない running 行を閉じる（他の instance が全て居なければ即、居れば `started_at + ownerless_ttl` で）。判定は `runs`・lease・`daemon_instances`（heartbeat・pid）だけ。LLM は呼ばない。
- D5 同 module `reap_lost_reviews`: 手元のレビューの取りこぼし（tokio task が判定を届けずに終わった = 2 tick 続けて `is_finished`、または `since + ownerless_ttl` の期限越え）を毎 tick、active・draining を問わず閉じる。同じ instance の中でも回収が働く（人の割り込みへの回答）。
- `task-core`: `TaskStore::runs_running`（running 行の読み出し）。`orphan.rs`: `ownerless_run_decision` と理由の定数。e2e の BrowserFixture は持ち主つき（draining の instance 行）の run に変更。
- 文書: `docs/architecture-map.md` に 1 行追加。

### 証拠

- 条件 0（旧 instance が reviewer run を抱えたまま引き継ぎ・停止）: `cargo test -p task-dispatch ownerless` → exit 0、13 passed。`handoff_shutdown_closes_the_old_reviewer_run_and_the_new_active_rereviews`（経路 B）、`a_crashed_old_daemons_reviewer_run_is_closed_and_the_task_rereviewed`（経路 C）、`a_reviewer_run_not_launched_because_a_check_failed_is_closed`（経路 A）。修正前の挙動は前回 run の `artifacts/prefix-fail.log`（修正前の試験が running のまま残ることで落ちる）に記録。
- 条件 1（生きたプロセスの無い running を起動時と定期に回収、task が再試行可能）: `startup_closes_an_ownerless_reviewer_run_and_rereviews`、`ownerless_runs_are_reconciled_again_after_the_interval`（時計の差し替え）、`an_ownerless_run_is_kept_while_another_instance_lives_until_its_deadline`、`a_leased_worker_run_of_a_gone_daemon_is_requeued_by_the_lease_path`、同じ instance 内の `a_review_that_ended_without_a_verdict_is_closed_and_redone_in_the_same_instance`・`a_review_that_outlives_its_deadline_is_stopped_and_redone_in_the_same_instance`。
- 条件 2: `cargo test --workspace --no-fail-fast` → exit 0（127 suites、3677 passed、0 failed、13 ignored）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。文書検査: `check-adr-numbers.sh`・`check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-doc-links.sh`・`check-architecture-map.py` → 各 exit 0。
- 途中の失敗 1 件: D5 の期限判定を差し替え時計で測ったため、時計を 2 時間進める既存試験 `provider_failure_requeues_without_consuming_attempts` が落ちた。`ReviewEntry.since` と同じ実時計で測る形に直し、期限の試験は `since` を差し替えて決定的に再現する。

### 未解決

- 本番に残っている 2 行（`01M4129N…`、`01M41EKM…`）は、この変更を含む release が active になった最初の tick の D3 で閉じる。本番 DB は触っていない。昇格は人が行う。

### 提案

- `runs` 行に持ち主 instance を書く案は採らなかった（ADR「採らない」）。将来 reviewer run に lease を持たせるなら D3/D5 は不要になるが、`acquire_lease` の経路が 2 つになる。
