---
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: accepted
---
# ADR 2026-11-14: 段の統合の完了を 1 つの transaction で書く

- 状態: 採用（実装は task 01M6TYVJT4S29ZMP6P0D4831JG の WorkUnit fix-integ。決定 store-scope の答え a）
- 関係: [ADR 2026-11-11 Dispatcher の状態の owner と、判断と副作用の分け方](2026-11-11-dispatcher-state-ownership.md) の
  D4（判断と副作用の分割）・D7（反例の集合）を、`finish_phase_integration` について具体にする。ADR-0074 D1.4 5.（統合の成功）・
  D2.2（途中確認は次の工程の WU を ready にする前に適用する）と ADR-0079 付記「R6-1」D2 の順序を、1 回の書き込みの中で守る。
  ADR 2026-11-11 の本文は変えない。

## 背景

`crates/task-dispatch/src/dispatcher/phase_integration.rs` の `finish_phase_integration` は、統合の成功を次の 3 種類の書き込みに
分けていた。

1. 統合 WU の running → done（`TaskStore::work_unit_transition`）
2. 次の工程の WU の pending → ready（`promote_newly_ready`。WU ごとに `work_unit_transition`）
3. `PhaseIntegrated`・`PhaseReported` などの events と Task の遷移（`apply_transition_with_events`）

1 の後で daemon が落ちると、再起動の後に統合 WU は done のまま残り（統合をやり直す経路は running の統合 WU だけを pending に戻す）、
`PhaseIntegrated` は 1 件も記録されない。途中確認（`review: human`・`pause_after`）の段では、2 を飛ばしたまま 3 の
`PhaseGate` も書かれないので、次の dispatch の `promote_newly_ready` が次の工程の WU を ready にし、人の確認を飛ばして進む
（`agent-docs/progress/2026-11-11-health-v2-r4/cex-review.md` の未解決 1、試験 7）。

## 決定

### D1. store の method を 1 つ足し、3 種類の書き込みを 1 つの Immediate transaction にする

`TaskStore::phase_integration_finish(task_id, work_units, trigger, extra_events) -> Outcome` を足す。`work_units` は
`(書き込む WU の行, その WorkUnitTransitioned)` の列で、統合 WU の done を先頭に、promote する行を続ける。中身は既存の tx 関数
`update_work_unit_tx`・`append_event_tx`・`apply_transition_tx` を順に呼ぶだけで、`execution_plan_replan_tx` と同じ組み方にする。
SQL 文・migration・列は足さない。新しい WU 行を INSERT する `repair_apply`（`review_repair_apply`・`delivery_repair_apply` と共有）
は使わず、変えない。

events の順は以前の 3 回の書き込みと同じ（統合 WU の `WorkUnitTransitioned`、promote の `WorkUnitTransitioned`、`Transitioned`、
`PhaseIntegrated`、`PhaseReported` 等）にする。replay・投影が読む順は変わらない。統合 WU を先に done にするので、
`apply_transition_tx` の `integration_running` の拒否（`Continue{advance}` は running の統合 WU があると通さない）は同じ
transaction の中で以前と同じく通る。

### D2. `finish_phase_integration` は判断を先に組み立て、1 回で書く

`finish_phase_integration` は、統合 WU を done にした行を store から読んだ WU の集合に置いて（書き込みはまだしない）、その集合から
次のものを組み立てる。

- trigger（`WorkerDone`・`Continue{advance}`・`PhaseGate`）と events（`PhaseIntegrated`、停止点なら途中報告の artifact と
  `PhaseReported`）。判定の中身は以前と同じ（`execution_scheduler::settle_phase`、`PausePointsResolved`）。
- promote の行。`task_core::newly_ready` で求め、行と event は `promote_newly_ready` と同じ関数（`ready_promotions`）で作る。
  `PhaseGate` のときは promote しない（ADR-0079 付記「R6-1」D2）。

組み立てたものを `phase_integration_finish` で 1 回だけ書く。WU の worktree と取り込んだ子 task の worktree の削除は、commit の後の
冪等な副作用として残す（削除に失敗しても log を出すだけで、次の統合・Task の終端で消える）。途中報告の artifact の file は
書き込みの前に書く（event に path を載せるため。落ちて書き直しても同じ path に同じ内容を書く）。最終レビューの起動は commit の後。

### D3. Task の遷移が通らなければ何も書かず、統合 WU を pending に戻す

`phase_integration_finish` が `InvalidTransition` を返すのは、統合の結果を読んだ後に人が Task を cancel・interrupt した場合である。
このとき transaction は何も書かない（以前は統合 WU の done と promote だけが残り、`PhaseIntegrated` の無い done の統合 WU ができた）。
`finish_phase_integration` は既存の `reconcile_integration`（running の統合 WU を pending に戻す）を reason
`integration_result_not_applied` で呼ぶ。Task が再び Ready になれば、統合は既存の冪等な手順でやり直す（済んだ merge は飛ばす）。

### D4. 更新前の版の daemon が残した行は扱わない

更新前の版の daemon が 1 と 3 の間で落ちて残した行（`PhaseIntegrated` の無い done の統合 WU）を、新しい版は記録し直さない。理由:

- 窓は 1 つの同期関数の中（store の書き込みと worktree の削除・途中報告の file の書き込みの間）だけで、await を挟まない。
  daemon の process がその間に死んだときだけ起きる。
- 新しい版はこの状態を作らない。記録し直す照合の経路（決定 store-scope の b）を置くと、新しい版では起きない状態のための経路が
  ずっと残る。決定 store-scope はこれを採らなかった。
- 残っているかは read only の SQL 1 本で確かめられる。人は本番に入れる前に次を実行し、0 件であることを確かめる
  （1 件以上なら、その task を人が確認する。手順は進捗 `agent-docs/progress/2026-11-11-health-v2-r4/fix-integ.md` の「本番への移行」）。

```sql
SELECT w.task_id, w.key FROM work_units w
JOIN tasks t ON t.id = w.task_id
WHERE w.kind = 'integrate' AND w.status = 'done' AND w.phase IS NOT NULL
  AND t.status NOT IN ('done', 'failed', 'cancelled')
  AND NOT EXISTS (
    SELECT 1 FROM events e WHERE e.task_id = w.task_id
      AND json_extract(e.json, '$.type') = 'phase_integrated'
      AND json_extract(e.json, '$.work_unit_id') = w.id);
```

### D5. 統合 WU を done にするほかの経路

統合 WU（kind `integrate`）を done にするのは `finish_phase_integration` だけである（`reason: "integrated"` の書き手は 1 か所）。
ほかの WU を done にする経路（子 task の done の `tree_units.rs`、子 task の採用の `task_ops::tree_adopt`、atomic task の暗黙の
WU の実体化の `review_verdict.rs`）は、既に 1 つの transaction で書いているか（`work_unit_transition_with_events`・`repair_apply`）、
Task の遷移を伴わない。寄せる対象は無い。

## 試験

- `crates/task-dispatch/src/dispatcher/tests/cex_review_authority.rs` の試験 7
  （`cex_integration_restart_after_the_integration_unit_is_done_does_not_integrate_again`）に、再起動の後に `PhaseIntegrated` が
  ちょうど 1 件、merge が二重にならないことを足す。準備は「1 回の書き込みの前で落ちた形」（落ちた daemon の統合は merge と検査を
  終えたが、完了は書いていない）に変える。D1 の後は「統合 WU だけが done」の状態は起きない。
- 同じ file に `cex_integration_restart_keeps_the_stage_pause` を足す。途中確認の段の統合の完了の前で落ちて再起動した後も、
  `PhaseGate` で Blocked になり、`PhaseIntegrated` は 1 件、次の段の WU は pending のまま。完了を書いた後でもう一度再起動しても
  同じ。
- `crates/task-core/src/store/tests.rs` に、Task の遷移が通らないとき `phase_integration_finish` が WU の行も events も書かないことを
  足す。
