---
tasks: [01M7DYMCZ4T7C5RNQ3JZQAF4G7]
status: accepted
---
# ADR 2026-11-15: lease の回収は snapshot の lease と DB の lease を同じ transaction で照合してから書く

- 状態: 採用（実装は task 01M7DYMCZ4T7C5RNQ3JZQAF4G7 の WorkUnit fix-store〈store の口〉と fix-reclaim〈回収の経路〉）
- 関係: [ADR 2026-10-03 持ち主の居ない running の run](2026-10-03-ownerless-running-runs.md) の不変条件 `lease-single-holder`、
  [ADR-0070](0070-task-failure-visibility-and-handoff-safe-runs.md) D3・D5（lease 切れの回収と、生きている run の延長）、
  [ADR 2026-11-11 Dispatcher の状態の owner](2026-11-11-dispatcher-state-ownership.md) D7 の表の lease の行
  （回収側が期限切れの snapshot を取った直後に、保持者が renew する順序）。これらの本文は変えない。

## 背景

`crates/task-dispatch/src/dispatcher/leases.rs` の `reclaim_expired_leases` は、`TaskStore::list(Some(Running))` の snapshot で
lease が切れた Task を選び、`RunSupervisor::release_lease` → `TaskStore::apply_transition_with_events(task_id, trigger, finished)`
で遷移（`InfraRequeue` など）と終了記録（`WorkerFinished`）を書く。書き込みの transaction の中の照合は `transition()` の状態
（running か）だけで、snapshot の lease（run id・期限）とは照合しない。

同じ DB を使う別の instance（保持者）が、snapshot の後・書き込みの前に `renew_lease` で lease を延ばすと、回収側は古い snapshot の
判断のまま回収を書く。保持者の run は生きたまま lease を失い、`runs` 行は `harness_error` に閉じられ、Task は ready に戻る
（`agent-docs/progress/2026-11-11-health-v2-r4/cex-race.md` の renew-race の節「観測」。試験
`cex_lease_renew_after_expired_snapshot_keeps_the_holder`）。

store には、期待する lease を引数に取り writer transaction の中で照合する Task の口が無い（WU の行の `recover_work_unit`・
`try_start_work_unit_integration` だけ）。`TaskStore::release_lease`（run id が一致すれば lease だけを消し、status を変えない）は
本番の呼び出しが無く、試験 `release_lease_then_reacquire_succeeds` だけが使っている。

## 決定

### D1. 回収の書き込みは、snapshot の lease と DB の lease を同じ writer transaction で照合してから書く

`TaskStore::apply_transition_if_lease(task_id, expected: &Lease, trigger, extra_events) -> Result<Option<Outcome>, StoreError>` を足す。
1 つの Immediate transaction で Task を読み（`get_locked`）、次の 2 つが両方成り立つときだけ、`apply_transition_tx` で遷移・
`Transitioned`・`extra_events` を書いて commit し `Some(outcome)` を返す。

- `status` が `Running`。
- `task.lease` が `expected` と等しい（`Lease` の `PartialEq`: `worker_run_id` と `expires_at` の両方）。

期限も比べるので、保持者が同じ run id のまま `renew_lease` で延ばした lease（v2 の WU の lease の延長が Task の lease を延ばした
場合を含む）は「違う」になる。比較は Task の `json` から読んだ lease どうしで行う（snapshot の `list` も `json` を読む）。

遷移が `transition()` で無効なとき（`StoreError::InvalidTransition`）など、`apply_transition_tx` の誤りはそのまま返し、何も書かない
（transaction は commit しない）。

lease の回収（`reclaim_expired_leases` の `RunSupervisor::release_lease`）は、`apply_transition_with_events` の代わりにこの口で
snapshot の lease を渡す（WorkUnit fix-reclaim）。

### D2. 照合が合わなければ何も書かない

Task が無い・`status` が `Running` でない・lease が `expected` と違う、のどれかなら、`tasks` 行・events・`runs` 行・承認・wait の
どれも書かずに `Ok(None)` を返す。回収側は `None` を「この Task は回収しない」と読み、終了記録・WU の照合・統合の照合・
decision log・backoff の期限を作らない。次の tick の snapshot が新しい lease で判断し直す。

### D3. 本番の呼び出しの無い `TaskStore::release_lease` を消す

`TaskStore::release_lease`・`SqliteStore::release_lease_impl`・試験 `release_lease_then_reacquire_succeeds` を消す。lease を
手放すのは running から出る遷移（`apply_transition_tx` が lease を消す。ADR-0002 D1）だけにする。消した試験の
「解放の後に取り直せる」は、`apply_transition_if_lease` で回収した後に `acquire_lease` が通る試験で置き換える。

`dispatcher` の `RunSupervisor::release_lease`（lease の解放と終了記録の副作用）は別の関数で、名は変えない。

### D4. `orphan_takeover` の event は回収が書けたときだけ残す

孤児の回収（`ReclaimReason::Orphaned`）の `worker_progress`（`orphan_takeover: …`）は、今は回収の書き込みの前に追記している。
D2 で回収が書かれない（保持者が lease を延ばした）ときに、回収しなかった run の takeover の記録が残る。`orphan_takeover` の event は
D1 の書き込みが `Some` を返したときだけ残す（`extra_events` に入れて同じ transaction で書くか、`Some` の後に追記する。
どちらにするかは fix-reclaim が決め、その節に書く）。

## 範囲外

- `finalise_from_result_json` の経路（終端の result.json を残して消えた run を、その内容で確定させる）。lease の照合は
  この ADR では足さない。
- reviewer run の判定の比較更新（`on_review_finished` が、reviewer run の `runs` 行が `running` であることを同じ writer
  transaction で照合してから判定を書く）。`cex-race.md` の fix-verdict の節の提案のまま残す。

## 守る不変条件と試験

- 照合が合ったときだけ書く・合わなければ何も書かない: `crates/task-core/src/store/lease_reclaim_tests.rs`
  （一致・別の接続の renew の後・run id が違う・running でない・回収の後の取り直し）。
- 回収側の古い snapshot が保持者の延ばした lease を消さない: `crates/task-dispatch/src/dispatcher/tests/cex_lease_restart.rs` の
  `cex_lease_renew_after_expired_snapshot_keeps_the_holder`（fix-reclaim の後に合格する）。
