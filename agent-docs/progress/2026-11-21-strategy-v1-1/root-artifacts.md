---
title: 戦略層 v1 子 1 WorkUnit root-artifacts — root の最終 review の ArtifactExists が WorkUnit の成果物を見つける
tasks: [01M7XQ9Q4P4ZQ38Y509J9XYAQ0]
status: done
updated: 2026-11-21
---
# 戦略層 v1 子 1 WorkUnit root-artifacts

完了日: 2026-11-21

結論: root の最終 review の `ArtifactExists` は、同じ task の WorkUnit の run が出した成果物を名前で見つけるようになった。
他の task・他の案件の成果物と、workspace の外を指す path は拾わない。計画の無い task の候補と判定は変わらない。
この修正は release が本番に昇格するまで本番の review に効かない（昇格は人が行う）。

## 原因（確認済み）

- `crates/task-dispatch/src/dispatcher/review_spawn.rs:823`（基準 `$CELERIS_WU_BASE` の行）の
  `let produced = artifacts_for_run(&events, &run_id);` が review する run の `ArtifactProduced` だけを候補にしていた。
- `crates/task-dispatch/src/review.rs:686`〜`692`（同じ基準）の `Check::ArtifactExists` は、候補に名前が無いと
  `artifacts/<name>`（root の成果物 dir）を探す。WorkUnit の成果物は `wu/<key>/artifacts/` にある。
- 本番の例（本番 DB を `mode=ro` で読んだ）: task 01M7MZTKBPZPQE4XN451F8X84F（アーキ v4 H）。WU `finish` の 1 本目の run
  01M7WBMVFEKRS1VK86X2D82T74 が `wu/finish/artifacts/final-report.md` を出し（seq 1974）、question で止まった。2 本目の run
  01M7WF5JD6NAV1R4EATPZ8P4BT は成果物を出し直さずに done になり、その review が seq 2075（2026-11-20T13:09:39Z）で
  `artifact "final-report.md" not found at artifacts/final-report.md` で落ちた。replan で足した WU `publish` の run
  01M7WHTMZZJVMQBKHHR3KQ8EEF が成果物を出し直し、seq 2478 の review が合格した。

## 変更

- `strategy-v1 1 root-artifacts: root の最終 review の ArtifactExists が同じ task の WorkUnit の run の成果物を候補にする`
  - 最終 review の `produced`（`ArtifactExists` の名前の照合に使う候補）を、同じ task の WorkUnit の run の成果物まで広げた。
    並びは「同じ task の WorkUnit の run の `ArtifactProduced`（古い順）→ review する run の分」。
    `review_task` は同じ名前なら後ろを使うので、review する run の分 → WU の新しい run の分の順に選ぶ。
  - WorkUnit の run は `WorkUnitTransitioned{to: running, run_id}` の `work_unit_id` が `work_units_for(task)` にあるものだけ。
  - workspace の外を指す path は除く。規則は `task_worker::artifact::resolve` の字面の検査を `check_relative` に切り出して
    共有した（`resolve` も `check_relative` を呼ぶ）。
  - `review.rs` の `review_task` の doc の `produced` の説明を書き換えた。
  - 試験 `crates/task-dispatch/src/dispatcher/tests/root_review_artifacts.rs`（5 件）を足した。
- `strategy-v1 1 root-artifacts: 試験 (1) を本番と同じ形（review する run も同じ WU の run）にする`
- `strategy-v1 1 root-artifacts: ADR 2026-11-21-root-review-work-unit-artifacts を足す`
- `strategy-v1 1 root-artifacts: 進捗 file を書く`（この file）
- `strategy-v1 1 root-artifacts: 候補の組み立てを review.rs の artifact_candidates に移し、dispatcher の行を増やさない`
  - 組み立ての純粋関数を `crates/task-dispatch/src/review.rs` の `artifact_candidates` に置き、`review_spawn.rs` の
    `spawn_review` からは 1 行で呼ぶ。最初は `review_spawn.rs` に置いたが、test-affected の repo health が
    `size.responsibility-growth` の既存の 2 件の悪化（module `crates/task-dispatch/src/dispatcher` の本体の行 32350 > 8000、
    `Dispatcher::spawn_review` の関数の行 727 > 200）で exit 1 になったので移した。移した後の `check-health.sh` は
    `verdict=pass worsened=0`。使われなくなった `dispatcher.rs` の `artifacts_for_run` の import を消した。
- `strategy-v1 1 root-artifacts: ADR と進捗 file を関数の移動と検査の結果に合わせる`

新しい状態・DB 表・event・設定の欄・Manager・trait・fallback は足していない。成果物を root の dir に写す仕組みも置いていない。

## 試験（守る不変条件は試験 file の doc comment と ADR に書いた）

| 試験 | 確かめること | 修正前 | 修正後 |
| --- | --- | --- | --- |
| `root_review_artifact_finds_work_unit_run_artifact` | (1) 本番と同じ形で WU の 1 本目の run の成果物を見つける | FAIL | PASS |
| `root_review_artifact_prefers_reviewed_run_then_newest_work_unit` | 名前が重なれば review する run → WU の新しい順 | FAIL | PASS |
| `root_review_artifact_ignores_other_tasks_and_projects` | (2) 子 task・同じ案件の兄弟・他の案件の task の同名の成果物を拾わない | PASS | PASS |
| `root_review_artifact_ignores_paths_outside_workspace` | (3) 絶対 path・`..`・空の path を拾わない（review する run の分も） | FAIL | PASS |
| `root_review_artifact_unplanned_task_keeps_reviewed_run_only` | (4) 計画の無い task の候補は `artifacts_for_run` と同じ | PASS | PASS |

(2) と (4) が修正前に通るのは、修正前の候補が review する run の分だけだったから（(2) は修正が他の task の成果物まで
広げないこと、(4) は計画の無い task の挙動を変えないことを守る試験）。

### 修正前の出力（候補の組み立ての本体を base と同じ `artifacts_for_run(events, run_id)` に戻して実行）

```
$ cargo nextest run -p task-dispatch --lib -E 'test(root_review_artifact_)' --no-fail-fast
    FAIL ... root_review_artifact_ignores_paths_outside_workspace
    FAIL ... root_review_artifact_prefers_reviewed_run_then_newest_work_unit
    FAIL ... root_review_artifact_finds_work_unit_run_artifact
panicked at crates/task-dispatch/src/dispatcher/tests/root_review_artifacts.rs:173:5:
    artifact "final-report.md" not found at artifacts/final-report.md
panicked at crates/task-dispatch/src/dispatcher/tests/root_review_artifacts.rs:214:5:
    artifact "report.md" not found at artifacts/report.md
panicked at crates/task-dispatch/src/dispatcher/tests/root_review_artifacts.rs:353:5:
    [ArtifactRef { name: "final-report.md", path: "<tempdir>/outside/final-report.md", sha256: "", kind: "md", declared: false }]
     Summary 5 tests run: 2 passed, 3 failed
exit=100
```

(1) の落ち方は本番の seq 2075 と同じ文面。(3) の修正前の失敗は、review する run が記録した workspace の外の絶対 path が候補に
入っていたことを示す。成果物を出す側の `resolve` はこの path を記録しない（試験 `resolve_rejects_absolute_parent_missing_and_symlink_escape`
が守る）ので、通常の経路でこの記録はできない。本番 DB で同じ記録があったかは調べていない（未確認）。

修正後: 同じ command で `Summary 5 tests run: 5 passed`、exit=0。

## 証拠

- 修正前・修正後の試験: 上の節。完全な出力は run の成果物 dir の `logs/before.log`・`logs/after.log`。
- 検査（下の結果は提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）: 下の「検査」節。

## LoC 差分（`scripts/dev/source-size-report.py --exclusive-classes --rev <rev> --format json`。script と toml は `$CELERIS_WU_BASE` から `git show` で取り出し、base と HEAD の両方に当てた）

| 区分 | base | HEAD | 差 |
| --- | --- | --- | --- |
| production 全言語（product_only） | 376018 | 376071 | +53 |
| production Rust | 331327 | 331380 | +53 |
| own_total_with_tests | 1034966 | 1035442 | +476 |
| external_test | 463385 | 463808 | +423 |
| inline_test | 21408 | 21408 | 0 |

`git diff --numstat $CELERIS_WU_BASE HEAD` の内訳（Rust）: `review.rs` +45/−1、`artifact.rs` +18/−9、`review_spawn.rs` +2/−2、
`dispatcher.rs` +4/−4（import の折り返し）、`dispatcher/tests/mod.rs` +1、`dispatcher/tests/root_review_artifacts.rs` +422。
docs は ADR と進捗 file。

本体の純増 53 行の理由: 候補の組み立て（WU の run の見分けと path の除外、doc comment 7 行を含む）と、`check_relative` の
切り出し（doc comment と関数の枠）。この task は行数の純減を目的にしていない。

## 検査

（提出時の HEAD で実行した結果を書く）

## 回帰の条件

- 計画を持つ task の最終 review の `ArtifactExists` が `not found at artifacts/<name>` で落ち、同じ task の events に同じ名前の
  `artifact_produced` があり、その run に `work_unit_transitioned{to: running}` があれば、この修正の回帰。
- 計画の無い task の `ArtifactExists` の reason が変われば回帰（試験 (4) が守る）。

## 本番での確認（昇格の後に人か operator が行う）

ADR の「本番での確認」の節の `sqlite3 ... mode=ro` の query で、WorkUnit を持つ task の `review_verdict` の reason に
`path=wu/<key>/artifacts/<name>` が出ることを確かめる。本番の current release（7f69057907ad）はこの修正を含まない。

## 親の目的への寄与

- 確認済み: 計画を持つ task の最終 review が、WorkUnit の成果物の有無を正しく判定できる（試験 (1)）。アーキ v4 H では、
  この判定の誤りのために不要な replan と WU `publish` の run が 1 本増えた（events seq 2075〜2478）。
- 推定: 子 2 以降で、戦略・仮説・証拠の成果物を WorkUnit が出しても root の review がそれを根拠として認める前提ができる
  （改善 B と observability の土台）。本番での効果は昇格後まで未確認。
- 未確認: 本番で昇格後に同じ形の review が合格するか（上の手順で確かめる）。

## 未解決

- 子 task（tree の子）の成果物は親の review の候補にしない（ADR の却下した案）。親の受け入れ条件が子の成果物を名前で求める
  計画は、今までどおり子の review で判定するか、親の WorkUnit が成果物を出す必要がある。
- 本番昇格は人が行う。昇格までは、成果物を root task の `artifacts/` にも置く運用（依頼の「失敗からの教訓」）が要る。

## 提案

無し。
