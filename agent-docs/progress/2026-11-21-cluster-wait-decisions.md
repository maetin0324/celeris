---
title: クラスタ job の wait で終わった run の決定の要求を読み、job の終了と答えの両方を待って再開する
tasks: [01M7YWAC1JMG1Y87EA9XM1QSJ8]
status: done
updated: 2026-11-21
completed: 2026-11-21
---
# クラスタ job の wait で終わった run の決定の要求を読む

完了日: 2026-11-21

## 結論

- クラスタ job の wait で終わった木の節点の run（WU の遷移 `cluster_jobs`、atomic の trigger `ClusterJobWait`）の
  `decisions` を、既存の `Dispatcher::worker_decisions` で記録するようにした。決定の行と受信箱の項目ができる。
- 待ちの理由は常に 1 つにした（ADR [2026-11-21-cluster-job-wait-worker-decisions](../adr/2026-11-21-cluster-job-wait-worker-decisions.md)）。
  run の終わりでは WU は `blocked(cluster_jobs)` のまま。job が終わったとき決定がまだ unit を止めていれば
  `blocked(decision)` にし（`task_ops::decision::job_end_target`）、答えの解放は最後の run が wait で終わった unit を
  `needs_continuation` に戻す（`release_rows`）。答えが先なら unit は job の終了まで `blocked(cluster_jobs)` のまま。
  atomic は止めの正本が決定の行で、job の終了の後は `decision_self_hold` が答えまで run を起こさない。
- 子 1 の settlement の読む規則（`reads_worker_decisions`・`task_reads_worker_decisions`）とその試験の期待値は変えていない。

## 変更（commit の件名）

- `cluster-wait decisions: ADR を足す（wait を開くときに決定を記録し、job の終了と答えの両方を待って再開する）` — 実装の前の ADR。
- `cluster-wait decisions: ADR の ADR-0090 への link を直す`
- `cluster-wait decisions: wait で終わった run の決定の要求を記録し、job の終了と答えの両方を待って再開する` — 実装と試験 4 件。
- `cluster-wait decisions: 読み取りと再開の行き先を owner（worker_decisions・task_ops::decision・run_settlement）に寄せ、dispatcher の行を増やさない`
- `cluster-wait decisions: worker_decisions が作業場所を 1 か所で引き、dispatcher の行を base より増やさない`
- `cluster-wait decisions: 進捗と ADR の名前を実装に合わせる`

変えた production の file: `crates/task-ops/src/decision.rs`（`held_by_decisions`・`holds_unit`・`job_end_target`・
`waited_units`、`release_rows` の `needs_continuation`）、`crates/task-dispatch/src/dispatcher/cluster_job_wait.rs`
（`cluster_wait_unit_row` が行き先を `job_end_target` に聞く）、`dispatcher/tree_units.rs`（`worker_decisions` が
`result.json` の場所を自分で引く）、`dispatcher/worker_finish.rs`（読むかの判定を `records_worker_decisions`・
`task_records_decisions` に替えた）、`run_settlement/work_unit.rs`・`run_settlement/task_run.rs`・`run_settlement.rs`。

## 条件ごとの証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- 条件 0（実装の前の commit で ADR）: ADR の commit は実装の commit より前。事象・選択肢と採用の理由・順序の入れ替わり
  （D2 job が先、D4 答えが先、D5 atomic）・試験・ADR 2026-11-21-worker-decisions-fail-closed と ADR-0079 D7・ADR-0090 D2
  との関係を書いた。
- 条件 1（決定の行・受信箱・答えの前に進まない。両方の順序）: `crates/task-dispatch/src/dispatcher/tests/cluster_job_wait.rs` に
  `cluster_wait_decisions_job_end_first_holds_the_leaf_until_the_answer`・`cluster_wait_decisions_answer_first_resumes_on_the_job_end`・
  `cluster_wait_decisions_atomic_node_job_end_first`・`cluster_wait_decisions_atomic_node_answer_first` を足した。どれも決定の行
  （open・origin worker・`needed_before`）と `task_ops::decision::open_rows` の項目を確かめ、30 回の tick で run が起きない
  （`TickReport.dispatched == 0`）ことを確かめ、両方が揃った後の run が wait の続き（`cluster_jobs.state == "satisfied"`）で
  あることを確かめる。WU の 2 件は `assert_replay_is_clean` も通る。子 1 の `decision_fail_closed_*` の試験は 1 行も変えていない
  （`git diff --stat $(git merge-base HEAD main) --` に `run_settlement/work_unit/tests.rs`・`run_settlement/task_run/tests.rs`・`dispatcher/tests/tree_decisions.rs`・`dispatcher/tests/tree_gate.rs`・`task-core/src/decision/tests.rs` を渡すと出力が空）。
- 条件 2（決定的な試験と失敗注入）: job の状態は `apply_cluster_job_notice` で渡し、poll の hook は挿さない。sleep の待ちと負荷は
  使わない（`tick_until` の既存の 20 ms の間隔は worker の完了を待つ出来事待ちの保険で、判定は状態で行う）。失敗注入
  （ログは run の成果物 `inject/`）:
  - production の 6 file を base に戻す → `4 tests run: 0 passed, 4 failed`、exit=100。4 件とも `assert_open_worker_decision` で
    `left: []`／`right: ["vault"]`（決定の行が 0 件）。
  - `cluster_wait_unit_row` の止め（`job_end_target` の結果）を外す → `4 tests run: 3 passed, 1 failed`、exit=100。job が先の WU の
    試験が `left: (NeedsContinuation, None)` で落ちた（job の終了で答え無しに再開する）。
  - `release_rows` の `needs_continuation` への解放を外す → `4 tests run: 3 passed, 1 failed`、exit=100。job が先の WU の試験が
    `left: (Ready, None)`／`right: (NeedsContinuation, None)` で落ちた。
  - どの注入も `git checkout HEAD -- <file>` で戻し、作業ツリーは clean。
- 条件 3（検査）:
  - `bash scripts/dev/test-affected.sh` → exit 0。repo-health `verdict=pass`（`worsened=0`、`new=0`）、nextest
    `6922 tests run: 6922 passed (11 slow), 29 skipped`、web `Tests 705 passed (705)`。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
  - `cargo fmt --all -- --check` → exit 0。
  - `cargo nextest run -p task-dispatch -p task-ops --no-fail-fast -E 'test(/decision|cluster_job|cluster_wait|tree_decisions|run_settlement/)'`
    → exit 0、`124 tests run: 124 passed, 2490 skipped`。
  - `cargo nextest run -p task-dispatch -E 'test(cluster_wait_decisions_)'` → `4 tests run: 4 passed`。
  - 1 回目の `test-affected.sh` は repo-health の `size.responsibility-growth` が worsened=3（`finish_worker_result_with` の行・
    `Dispatcher` の method の数・`dispatcher` の module の行）で落ちた。新しい method を消し、読み取りと行き先の判定を
    `worker_decisions`・`task_ops::decision`・`run_settlement` に寄せて、dispatcher の本体の行を base より 1 行減らした。

## 未確認の事項

- v1 の WU（段階の無い計画）の上限切れ（`cluster_jobs_timed_out`）で決定がまだ止めているときは、unit を `blocked(question)` で
  なく `blocked(decision)` にする（`job_end_target` が同じ判定をする）。task の質問（`QuestionRaised`）は今までどおり出る。この
  経路の試験は足していない。
- unit が `blocked(cluster_jobs)` の間に replan で `ready` に戻され、走る前に別の決定で `blocked(decision)` になったとき、
  答えの解放は最後の run が wait で終わったことから `needs_continuation` にする。この組み合わせは試験で確かめていない。
- 修正後の経路は本番でまだ使われていない。本番で効くことは試験でしか確かめていない。

## 提案

- 無し（ADR の範囲で閉じた）。
