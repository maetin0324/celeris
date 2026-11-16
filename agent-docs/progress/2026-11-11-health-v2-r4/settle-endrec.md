---
title: "健全性v2 R4 settle-endrec: run-close の終了記録の組み立てを run_end_record の 1 つにし、閉じる経路は分けたまま残す"
tasks: [01M7B2KBNJ72C5F75RAEWSFECG]
status: done
updated: 2026-11-13
completed: 2026-11-13
---
# 健全性v2 R4 settle-endrec

## 概要

- ADR 2026-11-11 dispatcher-state-ownership D4 の run-close の行（担当 unit settle）の共有の関数 `run_settlement::run_end_record` を、
  `crates/task-dispatch/src/run_settlement.rs` に 1 つだけ定義した。
  - 入力: run id・outcome の文・usage・role（`None` はワーカー run）・metrics・終端（`Option<RunEnd>`。呼ぶ経路が明示し、関数は補わない）。
  - 返す値: 終了記録 `Event::WorkerFinished` だけ。追記・lease の検証・二重の終了記録の確認・状態遷移・transaction の境界は持たない
    （run を閉じる関数にしない）。
- 次の組み立て（`Event::WorkerFinished { .. }` の literal）を `run_end_record` の呼び出しに置き換えた。どの経路も、作った event を渡す
  store の呼び出し（`append_event`・`apply_transition_with_events`）と、その前後の lease の検証・二重の終了記録の確認・WU の照合は変えていない。
  | 経路 | 関数 | 渡す store の呼び出し（変えていない） |
  |---|---|---|
  | worker 完了の通常の終わり | `worker_finish.rs` `finish_worker_result_with` | 遷移と同じ transaction（既存の events の列の先頭） |
  | 確定の失敗 | `worker_finish.rs` `close_run_after_finalisation_failure`（closure） | 工程の lease は `append_event`、task の lease は `apply_transition_with_events` |
  | lease の回収 | `run_supervisor.rs` `release_lease` | `apply_transition_with_events`（lease を持っていた run ごとに 1 件） |
  | daemon の停止での割り込み | `leases.rs` `record_shutdown_interrupt`（closure） | task の lease は `apply_transition_with_events`、他は `append_event` |
  | 止めた run を閉じる | `leases.rs` `close_aborted_run` | `append_event`（既に記録があれば書かない） |
  | 工程の lease の孤児 WU run | `leases.rs` `requeue_orphaned_work_unit_run` | `append_event`（既に記録があれば書かない） |
  | planner run の終わり | `planner_flow.rs`（計画の採用・不正な計画の 2 か所） | 採用・やり直しの各経路（変えていない） |
  | reviewer run の終わり | `review_verdict.rs` `on_review_finished` | 終端は今と同じく `None` で作り、後で `set_worker_finished_end` が書き戻す |
  | 設計 review の reviewer run の終わり | `health_review/design_gate.rs` | `append_event` |
- 3 つの閉じる経路 `close_aborted_run`・`close_run_after_finalisation_failure`・`reconcile_ownerless_runs` は別々の関数のまま残した。
  一律の close(run) は作っていない。transaction の範囲は変えていない（D3）。
- `record_shutdown_interrupt` の metrics（`wall_ms_since(since)`）は今と同じく closure の中で、呼ばれたときに測る。
- `dispatcher.rs` の module map の run_settlement の行（同じ 1 行の中で）と、`docs/architecture-map.md` の run_settlement の行に
  run-close と呼び元を足した。ADR は変えていない。

## 同じ意味か確かめた組み立てと、変えなかったもの

- `reconcile_ownerless_runs`（`ownerless_runs.rs`）: 自分では組み立てず、`close_aborted_run` を呼ぶ（reviewer run は `reap_lost_reviews` も同じ）。
  `close_aborted_run` が `run_end_record` を使うので、この経路も同じ組み立てを通る。関数は分けたまま。
- 同じ意味なので置き換えた: `planner_flow.rs` 2 か所・`review_verdict.rs`・`health_review/design_gate.rs`・`leases.rs` の
  `requeue_orphaned_work_unit_run`。どれも run id・outcome・usage・role・metrics・終端から run の終了記録を作るだけで、Objective の 5 経路と同じ意味。
- 組み立てでない（読むだけ）ので変えなかった: `auto_leaf.rs`・`child_tasks.rs`・`work_units.rs`・`run_context.rs`・`dispatcher.rs`・
  `decision_log_wire.rs`・`continuation_session.rs`・`health_review/design.rs`、`worker_finish.rs` の `worker_finished_usage`・
  `set_worker_finished_end`・`build_continuation_context`。いずれも既にある `WorkerFinished` を match で読むか、終端を書き戻すだけ。
- task-core の store の中で組むので変えなかった: `crates/task-core/src/store/events.rs` の終端 task の run を閉じる処理
  （`close_runs_of_terminal_tasks` の `WorkerFinished{end: Cancelled}`）。task-core は task-dispatch に依存できず、組み立ては store の
  transaction の中にある。
- 依存の向きで呼べないので変えなかった: `crates/task-ops/src/comment.rs` の人のコメントの割り込み（`interrupted: comment`、`end: None`）。
  task-ops は task-dispatch の下の crate で、`run_settlement` を呼べない。
- 試験の fixture（`dispatcher/tests` などで events を作る literal）は入力の data なので変えていない。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

- `bash scripts/dev/test-affected.sh`: exit 0（707 秒）。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
  3879 tests run: 3879 passed、11 skipped。repo-health verdict=pass fail_grade=0 worsened=0。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: exit 0。verdict=pass、fail_grade=0、worsened=0、new=1。new の 1 件は `migration.remnant`（warning）で
  `crates/task-dispatch/src/host_maintenance.rs` の `legacy` の行。この unit は host_maintenance.rs を変えていない（main との差分の先行 commit 由来）。
- `cargo nextest run -p task-dispatch -E 'test(cex_) | test(structure_guard_)'`: exit 0、36 passed。cex_ の試験は書き換えていない
  （`git diff --quiet "$CELERIS_WU_BASE" -- …cex_lease_restart.rs …cex_review_authority.rs …2026-11-11-dispatcher-state-ownership.md` は exit 0）。
- `cargo nextest run -p task-dispatch -E 'test(run_settlement)'`: exit 0、4 passed（足した 1 件を含む）。
- 計画の check（run_end_record）: 定義は `run_settlement.rs` の 1 つ、呼び出しは 10 か所、`worker_finish.rs`・`leases.rs`・`run_supervisor.rs` に
  `= Event::WorkerFinished {`・`| Event::WorkerFinished {` は無い、`close_aborted_run`・`close_run_after_finalisation_failure`・`reconcile_ownerless_runs`
  は残り、`run_settlement*` に `fn close…` は無い。exit 0。
- 純粋さの grep（`run_settlement.rs`・`run_settlement/tests.rs` に範囲の check が grep する字面と名前に decide を含む関数が無い）: exit 0。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path が許可の正規表現に収まる）: exit 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0。

## 足した試験

`crates/task-dispatch/src/run_settlement/tests.rs` の `run_end_record_carries_every_input_as_given`。守る不変条件と、既存の試験で検出できない
理由は doc comment に書いた。

- 不変条件: 終了記録は渡された run id・outcome・usage・role・metrics・終端をそのまま運び、欄を落とさず、取り違えず、終端も role も補わない。
- 既存の試験で検出できない理由: 既存の試験は経路ごとに dispatcher を通して events を見るので、共有の組み立ての欄の取り落としは、その欄を
  その経路で assert する試験があるときだけ検出される。全欄を経路によらず 1 か所で固定する試験は無かった。

## 未解決事項

- 無し。

## 提案

- 無し。
