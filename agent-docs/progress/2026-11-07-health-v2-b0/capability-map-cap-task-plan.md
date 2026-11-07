---
title: 健全性v2 B0 capability-map cap-task-plan — task lifecycle・ExecutionPlan・WorkUnit/段の統合・決定/plan gate・再帰 task 木の棚卸し
tasks: [01M6V1TGEG5QT5P6PM0Q0D0QMK]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 B0 capability-map cap-task-plan

## 結論
- 断片 `capability-map/cap-task-plan.json`（schema `celeris.health-v2-b0.capability-map-fragment/1`）に 5 capability を書いた: task-lifecycle、execution-plan-adoption、workunit-scheduling-stage-integration、decisions-plan-gate、recursive-task-tree。各 capability に owner・不変条件・public port・実在する試験 path・試験コストがある。
- 共有状態は 29 件（SQLite 表 10・event kind 18・file 1）。SQLite 表の書き手は、`routing-eval` の fixture 生成（`crates/routing-eval/src/manifest.rs`、`tasks`・`work_units` へ直接 INSERT）を除くと全部 task-core::store にある。event kind は 18 件すべてが複数 crate（task-core・task-dispatch・task-ops・task-api のうち 2〜4 crate）から作られる。`Transitioned` は 36 file、`WorkUnitTransitioned` は 20 file。
- docs/architecture-map.md との食い違いを 10 件書いた。主なもの: `task-dispatch::execution_scheduler` と `dispatcher::auto_leaf` の行が無い、task-ops 節に作成・cancel・reopen・retry・pause の行が無い、ExecutionPlan と決定の行の owner が 1 crate だけ、関連試験の抜け（`decision/tests.rs`・`tree_decisions.rs`・`execution/tests.rs` など）。地図そのものは直していない。

## 基準
- 着手時の `git merge-base HEAD main`（件名「repair: main 取り込みの衝突解消の import を整形する」）。SHA は JSON の `base_sha`。

## 方法（読み取りだけ。build・試験は実行していない）
- SQLite 表の書き手: `grep -rlE '(INSERT( OR X)? INTO|UPDATE|DELETE FROM) +<table>' crates`（試験 file を除く）。
- event kind の書き手: task-core・task-dispatch・task-ops・task-api の `src` での `Event::<Kind>` の grep。試験 file と、読み手の投影（`task-api/src/query.rs`・`task-ops/src/{view,replay}.rs`・`task-core/src/{execution_metrics,routing_audit}.rs`・`store/{decision_log,request_ledger}.rs`）は除いた。match の腕も拾う近似で、構築している場所との区別はしていない（未確認）。
- 試験コスト: `test-targets.json` の `build_no_run[<crate>].secs`。段1 で測っていない task-ops の試験は `null`（cost_source「not measured in stage 1」）。
- 検証: 生成時に全 path（owner・evidence・port・test・writer）の実在を確かめた。

## 未解決事項
- event の書き手の数は近似（上記）。統合の段（deps-shared-state）で構築位置だけに絞ると減る可能性がある。
- `routing-eval` の `manifest.rs` の直接 INSERT が本番経路か fixture 生成だけかは未確認（`expect` を使っているので fixture 生成とみられる）。

## 提案
- architecture-map の task-dispatch 節に execution_scheduler と auto_leaf の行、task-ops 節に task lifecycle 操作の行を足す（別 task で。ここでは直さない）。
