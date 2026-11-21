---
tasks: [01M7XQ9Q4P4ZQ38Y509J9XYAQ0]
---
# ADR 2026-11-21: root の最終 review の ArtifactExists は同じ task の WorkUnit の run の成果物も名前で見つける

- 日付: 2026-11-21
- 状態: Accepted
- 関係: [ADR-0036](0036-per-task-artifacts.md)（成果物の dir と `ArtifactExists` の既定の path）、
  [ADR-0074](0074-parallel-work-units-checkpoints-milestones-quota.md) D1.2（WorkUnit の run は `wu/<key>/artifacts` に書く）、
  [ADR-0003](0003-worker-protocol.md) D5（成果物の path はワークスペース相対。`task_worker::artifact::resolve`）。
  この ADR は `review_task` の `produced`（`ArtifactExists` の名前の照合に使う成果物）の組み立てだけを変える。
  `ArtifactExists` の既定の path（`<artifacts_rel>/<name>`）、ディレクトリの判定、他の check は変えない。
- task: 01M7XQ9Q4P4ZQ38Y509J9XYAQ0（戦略層 v1 子 1、WorkUnit `root-artifacts`）

## 事象

task 01M7MZTKBPZPQE4XN451F8X84F（アーキ v4 H）の受け入れ条件 3 は `ArtifactExists { name: "final-report.md" }` だった。

- WorkUnit `finish` の 1 本目の run 01M7WBMVFEKRS1VK86X2D82T74 が `final-report.md` を
  `wu/finish/artifacts/final-report.md` に出し（events seq 1974 の `artifact_produced`）、question で止まった（seq 1975）。
- 人の答えの後、同じ WorkUnit の 2 本目の run 01M7WF5JD6NAV1R4EATPZ8P4BT が成果物を出し直さずに done になった（seq 2043）。
- 最終 review は 2 本目の run を対象にし、seq 2075（2026-11-20T13:09:39Z）で
  `artifact "final-report.md" not found at artifacts/final-report.md` と判定して落ちた。
- その後 replan で足した WorkUnit `publish` の run 01M7WHTMZZJVMQBKHHR3KQ8EEF が成果物を `wu/publish/artifacts/` に出し直し、
  その run を対象にした review（seq 2478）が合格した。成果物は 1 本目から存在していたので、出し直しの run は不要だった。

## 原因

- `crates/task-dispatch/src/dispatcher/review_spawn.rs:823`（基準 `$CELERIS_WU_BASE` の行）:
  `let produced = artifacts_for_run(&events, &run_id);` は review する run の `ArtifactProduced` だけを候補にしていた。
- `crates/task-dispatch/src/review.rs:686`〜`692`（同じ基準）: `Check::ArtifactExists` は候補に名前が無いと
  `<artifacts_rel>/<name>`（root の `artifacts/`）を探す。計画を持つ task の成果物は WorkUnit の run の dir
  （`wu/<key>/artifacts`）にあるので、review する run 以外の WorkUnit の run が出した成果物は見つからない。

## 決定

- D1: 候補の組み立ては `crates/task-dispatch/src/review.rs:638` の `artifact_candidates`（純粋関数）にし、最終 review の
  `spawn_review`（`crates/task-dispatch/src/dispatcher/review_spawn.rs:823`）が対象 task の `events_for(task_id)` と
  `work_units_for(task_id)` と review する run の id を渡して `produced` にする。`produced` の意味（`ArtifactExists` の名前の
  照合と優先順）は `review_task` の契約なので、同じ module に置く。`review_spawn.rs` に置くと repo health の
  `size.responsibility-growth` の既存の 2 件（module `crates/task-dispatch/src/dispatcher` の本体の行 32350 > 8000、
  `Dispatcher::spawn_review` の関数の行 727 > 200）が悪化して fail になった（実測）ので置かない。
- D2: 候補は「同じ task の WorkUnit の run の `ArtifactProduced`（events の順）→ review する run の `ArtifactProduced`」の並び。
  `review_task` は同じ名前なら後ろを使うので、名前が重なれば review する run の分 → WorkUnit の新しい run の分の順に選ばれる。
- D3: WorkUnit の run は `WorkUnitTransitioned { to: running, run_id: Some(r), work_unit_id }` で、`work_unit_id` が対象 task の
  `work_units` の行にあるものだけ（段の途中報告の成果物のリンク〈`phase_integration.rs` の `build_phase_report`〉と同じ見分け方）。
  子 task に任せた unit の遷移は `run_id` を持たず、子 task の成果物は子の events にあるので入らない。他の task・他の案件の
  events は読まない。
- D4: path が workspace の外を指す候補（空・絶対 path・`..`・root・prefix の要素）は除く。規則は成果物を出す側の
  `task_worker::artifact::resolve` の字面の検査を `check_relative`（`crates/task-worker/src/artifact.rs:23`）に切り出し、
  `resolve` と候補の組み立ての両方が使う（同じ規則を 2 か所に書かない）。review する run の候補にも当てる。成果物を出す側は
  この path を `ArtifactProduced` に書かないので、計画の無い task の到達しうる判定は変わらない。
- D5: 計画の無い task（`work_units` が空）の候補は review する run の分だけで、従来の `artifacts_for_run` と同じ。前の attempt
  の run の成果物は今までどおり候補にしない。
- D6: 新しい状態・DB 表・event・設定の欄・fallback は足さない。成果物を root の `artifacts/` に写す仕組みも置かない。

## 却下した案

- WorkUnit の成果物を root の `artifacts/` に写す: 同じ成果物の正本が 2 か所になり、sha256 と path の食い違いを生む（二重の正本）。
- 同じ task の全 run の `ArtifactProduced` を候補にする: 計画の無い task で前の attempt の成果物を拾い、従来の判定が変わる。
  daemon の途中報告（`daemon:phase-gate:<phase>`）も混ざる。
- `runs` 索引（`RunRow.work_unit_id`）で WorkUnit の run を引く: 索引の書き込みは失敗しても警告だけで run を止めない
  （`run_index_start` の呼び出し元）。events は追記専用の正本なので events から引く。
- 子 task の events も読む: 子 task の成果物は子 task 自身の review が判定する。親の review が子の成果物で合格すると、
  子の受け入れ条件の外の file を親の合格の根拠にできてしまう。

## 撤去

- `review_spawn.rs` の `artifacts_for_run(&events, &run_id)` の 1 行は `artifact_candidates` の呼び出しに置き換え、
  使われなくなった `dispatcher.rs` の `artifacts_for_run` の import を消した。`task_ops::derive::artifacts_for_run` は
  `artifact_candidates` が review する run の分を取るのに使い続ける（同じ規則を書き直さない）。
- `review.rs` の `review_task` の doc（「`produced` はその run の `ArtifactProduced`」）を新しい意味に書き換えた。

## 不変条件と試験

`crates/task-dispatch/src/dispatcher/tests/root_review_artifacts.rs`（store は `SqliteStore`、WorkUnit の行は本物の
`adopt_plan` で作る）:

- `root_review_artifact_finds_work_unit_run_artifact`: 本番と同じ形（WU `finish` の 1 本目の run が成果物を出し、review する
  2 本目の run は出さない）で、`ArtifactExists` が `path=wu/finish/artifacts/final-report.md` で合格する。修正前の本体
  （`artifacts_for_run` だけ）では `artifact "final-report.md" not found at artifacts/final-report.md` で落ちる。
- `root_review_artifact_prefers_reviewed_run_then_newest_work_unit`: 名前が重なれば WU の新しい run の分、review する run
  の分があればそれを選ぶ。
- `root_review_artifact_ignores_other_tasks_and_projects`: 同じ workspace を継ぐ子 task、同じ案件の兄弟、他の案件の task の
  WU が同じ名前の成果物を出しても拾わない。対象 task の events に他の案件の WorkUnit の id を指す遷移が紛れても拾わない。
- `root_review_artifact_ignores_paths_outside_workspace`: 絶対 path・`..`・空の path は WU の run の分も review する run の分も
  候補にしない。修正前は review する run の絶対 path が候補に入り、workspace の外の file で合格していた。
- `root_review_artifact_unplanned_task_keeps_reviewed_run_only`: 計画の無い task の候補は `artifacts_for_run` と等しく、
  前の attempt の成果物は従来どおり拾わない。
- 既存の `crates/task-worker/src/artifact.rs` の `resolve_rejects_absolute_parent_missing_and_symlink_escape` が
  `check_relative` を切り出した後の `resolve` の拒否を守る。

## 本番での確認（release の昇格は人が行う）

この修正を含む release が本番に昇格した後、WorkUnit を持つ task の最終 review の `review_verdict` で、`ArtifactExists` の
reason が `path=wu/<key>/artifacts/<name>` になることを確かめる:

```sh
sqlite3 "file:/local/celeris/data/db/celeris.sqlite3?mode=ro" \
  "select task_id, seq, ts, json_extract(json,'$.reason') from events
   where json_extract(json,'$.type')='review_verdict' and json like '%path=wu/%' order by ts desc limit 5;"
```

昇格の後に `not found at artifacts/<name>` で落ちた review があれば、同じ task の events に同じ名前の `artifact_produced` が
あるか（あれば、その run の `work_unit_transitioned{to: running}` の有無）を見る。
