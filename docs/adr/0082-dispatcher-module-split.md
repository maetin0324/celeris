# ADR-0082: dispatcher.rs を責務別の子モジュールへ分け、Dispatcher を facade にする

---
tasks: [01M3QEQPP31ZB29RH6YGFGTAPF]
---

- Date: 2026-09-29
- Status: Accepted
- 適用先: `crates/task-dispatch/src/dispatcher.rs` と `crates/task-dispatch/src/dispatcher/`

## 文脈

`dispatcher.rs` は production だけで約 17,000 行あり、ほぼ全体が 1 つの `impl Dispatcher` である。
エージェントが局所変更をするとき、関係の無い塊まで読み込むことになり、並行する作業の衝突も起きやすい。
test は既に `dispatcher/tests/` に外出し済みである。

## 決定

### D1. facade と子モジュール

- `dispatcher.rs` は facade とする。残すもの: `Dispatcher` struct と private な補助型（`RunEntry`、`ReviewEntry`、`Completion` など）、`new` と公開の setter/getter、`now_utc` / `monotonic_now`、`tick`、`drain_completions`、`dispatch_ready`、`is_idle`、private な `mod` 宣言、明示した `pub use`。
- 子モジュールは `dispatcher/<name>.rs` に置き、それぞれ `impl Dispatcher` ブロックか free fn を持つ。

### D2. 層（依存の向き）

L0（facade の型と free helper）← L1 ← L2 ← L3 ← L4 ← facade の `tick`。

| 層 | 子モジュール |
|---|---|
| L1 | `cluster`（cluster/tunnel 接続・probe）、`housekeeping`（disk guard・scratch・後片付け）、`snapshot`、`quota_book`（quota の見積り・release）、`provider_select`、`workspaces`、`run_context`、`worker_task` |
| L2 | `work_units`、`tree_units`、`child_tasks`、`review_spawn` |
| L3 | `worker_finish`、`planner_flow`、`review_verdict`、`phase_integration`、`leases` |
| L4 | `dispatch_run` |
| 後回し | `sinks`（StoreSink / ReviewerSink） |

- 横の呼び出しは L3 内の `worker_finish` → `phase_integration::start_integration` の 1 本だけ許す。
- `worker_task` は `Dispatcher` に依存しない。

### D3. 規則

- 移動だけ。関数の本体は変えない。移動の commit と中身を変える commit を分ける。
- 可視性は `pub(super)` か既存の `pub(crate)` まで。新しい `pub`・trait・crate は足さない。
- 公開パス（`task_dispatch::dispatcher::ClusterConnector` など、lib.rs の再公開、`crate::dispatcher::provider_failure_outcome`）は facade で明示的に `pub use` し直す。`*` での再公開はしない。
- モジュール名は crate 直下（`review`、`integration`、`scratch_gc` など）とも `task_core` のモジュール名（`tree`、`quota` など）とも重ねない。test が `use super::*` と `use task_core::*` を併用するため。
- エラー・イベントの文字列、schema、設定は 1 byte も変えない。
- 各段で `cargo test -p task-dispatch` の passed 件数が変わらないことを確かめる。

### D4. 守る invariant

1. dispatcher から LLM を呼ばない。
2. `tick` の段階の順序を facade にそのまま残す（`disk_ready` の判定は drain より前）。
3. 同じ transaction に入れる組（`CheckpointSaved`+`WorkerFinished`、`QuotaEstimated`+`WorkerFinished`、provider cooldown+遷移、`DecisionRequested`+blocked、Plan の子の挿入+ReviewPass、子の作成+Created/ApprovalRequested、木の子の一括作成、`ExecutionGated`+routing）を helper に分けても割らない。
4. `release_quota_if_tracked` を早期 return するすべての経路で呼ぶ。
5. lease と lock の順（`run_holds_lease` の確認後に適用、`spawn_review` の per-task lock は verdict 保存まで保持、lease 取得と `running.insert` は同じ流れ、`stop_run` は kill_tree → abort）を保つ。
6. `RunEntry.handle` と `ReviewEntry.handle` の名前と型を変えない（test が直接 await する）。
7. 本番コードの直接の `OffsetDateTime::now_utc()` を `self.now_utc()` に置き換えない（test の時計の意味が変わる）。
8. cluster の hook は `run_cluster_hooks_off_async` で包んで呼ぶ形を保つ。

## 結果

- 局所変更の読む範囲が子モジュール 1 つに縮む。巨大関数（`finish_worker_result`、`dispatch_one` など）の中身の分割は移動の完了後に別の段で行う。
- 子モジュールは facade の private な型に `super::` 経由で触れるため、型の field の可視性は原則変えずに済む。
