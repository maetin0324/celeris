---
title: "健全性v2 R4 cex-tick: 同じ tick の Done と lease の期限、WU と Task の遷移の間の人の操作の反例試験"
tasks: [01M7EQJXQB3S4Q8FS54JEYZFVW]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 R4 cex-tick

完了日: 2026-11-15

この file は WU tick-expiry が作った。WU tick-wu-human が (ii) の試験の節を足し、file を締めた（status done）。

結論: cex-tick の 3 試験（(i) 1 本、(ii) cancel・comment の 2 本）はすべて合格し、どの順序でも反例は今の実装で
成り立たない。本体（`cfg(test)` の外）と `tests/hooks.rs` は変えていない。

## tick-expiry: cancel の確定の後、同じ tick に Done と lease の期限が来る

結論: 反例は今の実装で成り立たない（不変条件は保たれる）。試験 `cex_done_and_lease_expiry_in_one_tick_close_the_run_once`
は合格する。本体（`cfg(test)` の外）と `tests/hooks.rs` は変えていない。

基準は `$CELERIS_WU_BASE`。

### 変更（commit の件名）

- `tick-expiry: pin a human cancel before a run's Done and lease expiry processed in one tick (cex_done_and_lease_expiry_in_one_tick_close_the_run_once)`
  — 新しい file `crates/task-dispatch/src/dispatcher/tests/cex_same_tick.rs`（adapter `DoneGateAdapter`、helper
  `active_with_clock`・`release_until_completion_sent` ほか、試験 1 本）と、`tests/mod.rs` の cex_ の mod の並びの
  doc comment 付きの `mod cex_same_tick;` の 1 行。
- `tick-expiry: record the same-tick cancel, Done and lease expiry test and the guard checks in cex-tick`
  — この進捗ファイル。
- `tick-expiry: make the guard check a level-2 section in cex-tick (## guard を外した確認, per-WU subsections)`
  — guard の節を `##` に上げた（attempt 1 の check の不合格の修正）。

### 作業前 brief

1. 根本原因: ADR 2026-11-11-dispatcher-state-ownership D7 の表の同時完了の行のうち、「人の cancel の確定 → run A の Done
   が完了の channel に溜まる → A の lease の期限切れ → 1 回の tick」の順序に試験が無い。
2. 責務 owner: 試験は `dispatcher/tests/`。対象の判断は `dispatcher.rs` の `tick`（`drain_completions` の後に
   `reclaim_expired_leases`）、`dispatcher/worker_finish.rs` の `on_worker_finished`（`run_holds_lease` で古い結果を捨てる）、
   `dispatcher/leases.rs` の `reclaim_expired_leases`・`abort_stale_runs`・`reconcile_terminal_records`、
   `dispatcher/ownerless_runs.rs`、store の `apply_transition`（Running を出る遷移で lease を外し、終端なら同じ
   トランザクションで `runs` 行を閉じる。ADR-0079 付記「R6-1」D4）。
3. 既存の再利用先: `tests/mod.rs` の `dispatcher_with_test_clock`・`new_task`・`run_until_state`・`events_of`・
   `done_outcome`・`STATE_WAIT_GUARD`、`orphan_takeover.rs` の `instance`・`SELF_ID`（`new_active` と同じ構成）。
   `cex_review_authority.rs` の `HumanGateAdapter` は file の private なので使えず、同じ形（semaphore の gate と
   `Notify`）の `DoneGateAdapter` を新しい file に置いた。
4. 選択肢: lease の期限は (a) 注入時計、(b) `store.renew_lease(id, run, Duration::ZERO)` の 2 つ。
   `reclaim_expired_leases` は `OffsetDateTime::now_utc()` を読み注入時計を見ないので (b) を採った。Done が channel に
   届いたことの確かめ方は (a) adapter の通知だけ、(b) 通知の後に `run_worker` が送信の直前に書く event を待つ、の 2 つ。
   adapter が返った後の `run_worker` は result.json の誤配置の確認（`result_location::misplaced_report` の `tokio::fs`）で
   await するので、通知だけでは送信済みと言えない。(b) を採った。
5. 新しい状態・分岐・fallback・port: 無し（試験だけ）。
6. 撤去対象: 無し。
7. 不変条件: A は 1 回だけ閉じる（`WorkerFinished` は cancel の遷移が積んだ `end: cancelled` の 1 件、`runs` 行は
   cancelled のまま `finished_at` も変わらない）。Done は Task を Reviewing・Done に進めない。回収は
   `WorkerFinished{lease_expired}` も `orphan_takeover` の記録も書かない。Task は Cancelled のまま lease を持たず、
   その後の tick でも run・reviewer を起こさない。確かめる試験は `cex_done_and_lease_expiry_in_one_tick_close_the_run_once`。

### 試験の手順

`cex_done_and_lease_expiry_in_one_tick_close_the_run_once`（Dispatcher の private field に触らない。`test_hooks` は
既存の注入時計 `now` だけを使う。待ちは出来事待ちで、保険は壁時計の `STATE_WAIT_GUARD`。CPU 負荷なし）:

1. 自分の instance の行を登録し、孤児の回収を有効にし、注入時計を持つ dispatcher（`active_with_clock`）で Task を
   dispatch する。run A は `DoneGateAdapter` の gate で止まる（`run_until_state` で A の起動と lease を待つ）。
2. `store.renew_lease(id, A, Duration::ZERO)` で A の lease の期限を今にする（true を確かめる）。期限は cancel の前に作る。
   cancel は同じトランザクションで lease を外すので、cancel の後の `renew_lease` は false を返す（試験はこれも確かめる）。
3. 人の cancel（`store.apply_transition(id, Trigger::Cancel, None)`）を確定する。2 と 3 の間に await は無く、worker の
   lease の更新は割り込まない。確かめること: Task は Cancelled、lease は無い。A の `WorkerFinished` は
   `end: cancelled` の 1 件（R6-1 の閉じ）。`runs` 行は cancelled で、`finished_at` を控える。
4. gate を開ける（`release_until_completion_sent`）。adapter は `artifacts_dir` に `same-tick-note.md` を書いて `Done` を
   返し、返る直前に `returned` を鳴らす。試験はこの通知を待ち、続けて `run_worker` が返る前に書く
   `ArtifactProduced{run_id: A}` を store で待つ。この append から `Completion::Worker` の送信まで await は無いので、
   current-thread の runtime の試験の task がこの event を見た時点で、A の Done は完了の channel に入っている。
   この間 dispatcher は tick しない。
5. tick を 1 回だけ呼ぶ。`TickReport` は `finished == 1`（Done を `drain_completions` が受けた）、`reclaimed == 0`、
   `dispatched == 0`、`in_flight == 0`。
6. 確かめること（`assert_closed_once`）: A の `WorkerFinished` は 3 と同じ 1 件。`runs` 行は cancelled、`finished_at` は
   3 で控えた値。`orphan_takeover` の記録は 0 件。Task は Cancelled、lease は無い。`Transitioned` は 3 の時点から
   増えない。`WorkerStarted` は A の 1 件だけ（reviewer 0）。delivery は無い。`runs_running` は空。
7. 注入時計を `RUNS_RECONCILE_INTERVAL_SECS + 1` 秒進め、周期の照合（終端の task の `runs` 行・持ち主の居ない run）も
   回る tick を 3 回呼ぶ。各 tick で `dispatched`・`finished`・`reclaimed`・`reviewed` は 0、6 の確かめはすべて成り立つ
   （Done 無し・reviewer 0・lease 無し）。

doc comment に、守る不変条件と、`cex_human_cancel_precedes_successful_worker_completion`（cancel の後の完了を
`run_until_state` の tick の繰り返しで受けるので、完了がどの tick に届くかを決めず、lease の期限も作らない。
`WorkerFinished` の件数・`runs` 行の閉じの回数・`orphan_takeover` の記録も見ない）と `cex_lease_restart.rs` の試験
（人の cancel を挟まず、回収が先に lease を取る順序だけを作る）がこの順序を作らない理由を書いた。

### 観測

- 3 の cancel の時点で、store が A の `runs` 行を `WorkerFinished{end: cancelled, outcome: "interrupted: the task reached
  cancelled while this run was still open (runs index closed, ADR-0079 R6-1)"}` で閉じ、lease を外す。
- 5 の tick では、`drain_completions` が A の Done を受け、`on_worker_finished` の `run_holds_lease`（Task が Running で
  なく lease も無い）で捨てる。event も `runs` 行も書かない。続く `reclaim_expired_leases` は Running の Task だけを
  見るので、期限切れの lease は cancel で既に消えており、何もしない（`reclaimed == 0`）。
- この順序で lease の期限が効くのは、cancel が lease を外さなかった場合だけになる（下の G2 の行）。

### 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

attempt 1 の run の後の check は exit 1 だった。原因は見出しの段で、`### guard を外した確認` が check の
`grep -q '^## guard を外した確認'` に当たらなかった。attempt 2 で節を `## guard を外した確認` に上げ、WU ごとの小節
（`### tick-expiry`）に分けた。試験と本体は変えていない。下の検査は attempt 2 の commit
`tick-expiry: make the guard check a level-2 section in cex-tick (## guard を外した確認, per-WU subsections)` の上で
実行した（その後の変更はこの進捗ファイルの文面だけで、同じ commit に入れた）。

- 計画の check（`grep -q '^## guard を外した確認' … && grep -q 'cex_done_and_lease_expiry_in_one_tick_close_the_run_once' …
  && grep -q '^mod cex_same_tick;' crates/task-dispatch/src/dispatcher/tests/mod.rs`）→ exit 0。
- `cargo nextest run -p task-dispatch -E 'test(/cex_/)'` → exit 0（35 passed、`cex_same_tick::` の 1 本を含む）。
- `bash scripts/dev/test-affected.sh` → exit 0。3947 本すべて合格（11 skipped、7 slow、9 分 4 秒）。対象 crate は
  celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。script bundle の SKIP は 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → この進捗ファイル・`cex_same_tick.rs`・`tests/mod.rs` の 3 つだけ。
- `git diff --stat $CELERIS_WU_BASE -- ':!crates/task-dispatch/src/dispatcher/tests' ':!agent-docs'` と
  `git diff --stat $CELERIS_WU_BASE -- crates/task-dispatch/src/dispatcher/tests/hooks.rs` → どちらも差分なし（本体と
  `tests/hooks.rs` は変えていない）。
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）。

## tick-wu-human: WU の done と Task の遷移の間に確定した人の cancel・comment と再起動

結論: 反例は今の実装で成り立たない（不変条件は保たれる）。試験 `cex_human_cancel_between_wu_and_task_transitions_is_kept`
と `cex_human_comment_between_wu_and_task_transitions_is_kept` は合格する。本体と `tests/hooks.rs` は変えていない。

基準は `$CELERIS_WU_BASE`。

### 変更（commit の件名）

- `tick-wu-human: pin a human cancel and comment between the WU done and the Task transition across a restart (cex_human_cancel_between_wu_and_task_transitions_is_kept, cex_human_comment_between_wu_and_task_transitions_is_kept)`
  — `cex_same_tick.rs` に helper `halt_after_wu_a_done`・`assert_wu_a_kept_done` と試験 2 本を足した。
  `cex_lease_restart.rs` の helper `wu_transitions`・`wu_row`・`dependency_ready_count` を `pub(super)` にした（中身は
  変えていない）。
- `tick-wu-human: record the WU/Task-transition human cancel and comment tests and the guard checks, close cex-tick`
  — この進捗ファイル（status done）。

### 作業前 brief

1. 根本原因: ADR 2026-11-11-dispatcher-state-ownership D7 の表の同時完了の行のうち、「finish が WU の done を書いた
   直後（Task の遷移・`runs` の閉じ・`WorkerFinished` の前）に落ちる → 人の cancel か comment が確定する → 再起動」の
   順序に試験が無い。
2. 責務 owner: 試験は `dispatcher/tests/`。対象の判断は `dispatcher/worker_finish.rs` の `on_worker_finished`（WU の
   遷移の後の `halt_after_wu_transition`）、再起動の照合（`leases.rs` の `reclaim_expired_leases`・`close_aborted_run`・
   `reconcile_terminal_records`、`ownerless_runs.rs`、`work_units.rs` の `reconcile_parallel_tasks`・`wu_dispatch_gate`）、
   dispatch の選別（store の `ready_tasks`・`acquire_lease`、task-core の `next_work_unit`）、人の操作（`task_ops::gate::cancel`、
   `task_ops::comment::post_human_comment_as`）と store の `apply_transition`（Running を出る遷移で lease を外し、終端なら
   `runs` 行を閉じる。R6-1）・`WorkerFinished` の append（`runs` 行を同じトランザクションで閉じる。Phase F5-fix3）。
3. 既存の再利用先: `cex_restart_after_wu_done_promotes_dependents_once` と同じ作り（`adopt_three_step_plan`・
   `dispatcher`・`done_adapter`・`halt_after_wu_transition`・`orphan_takeover::new_active`）、`cex_lease_restart.rs` の
   `wu_row`・`wu_transitions`・`dependency_ready_count`（`pub(super)` にした）、`cex_same_tick.rs` の `finishes`・
   `transitions`・`starts`・`orphan_records`。
4. 選択肢: 計画の版は (a) v2（工程つき、objective の文面）、(b) v1（参照の試験と同じ）の 2 つ。v2 を一時的な probe
   試験で確かめた（commit していない）: `halt_after_wu_transition` は finish の残りだけを止め、tick の残りは止めない。
   同じ tick の `reconcile_parallel_tasks` が「走っている WU も統合も無い Running の v2 Task」を `advance` で Ready に戻し、
   続く `dispatch_ready` が b を起こした（probe の観測: Task は `Running`、`Transitioned` は `dispatch`・`advance`・
   `dispatch`、b は `dependency_ready`・`dispatch`、`WorkerStarted` は 2 件）。v2 では停止点が残らないので (b) を採った
   （v1 の Task の lease は run A のままで、`reconcile_parallel_tasks` は工程の保持者でない lease を飛ばす）。
   comment の変種は Running の Interrupt を使い、質問の回答待ちの Blocked の変種は置いていない。
5. 新しい状態・分岐・fallback・port: 無し（試験だけ）。
6. 撤去対象: 無し。
7. 不変条件と試験: 下の「試験の手順」の assert。確かめる試験は上の 2 本。

### 試験の手順

共通（`halt_after_wu_a_done`）: v1 の計画（a → b → c）の Task を `daemon_instances` に行を持たない旧 dispatcher
（`halt_after_wu_transition = true`、`done_adapter`）で dispatch し、`run_until_state` で WU a の done を待つ。停止点で
確かめること: Task は Running、lease は run A、A の `WorkerFinished` は無く `runs` 行は running、b・c は pending、
`WorkerStarted` は A の 1 件。人の操作を確定してから旧 dispatcher を落とし、同じ store に `new_active` を起こす。
Dispatcher の private field に触らず、待ちは `run_until_state`（出来事待ち、保険は `STATE_WAIT_GUARD`）。CPU 負荷なし。

`cex_human_cancel_between_wu_and_task_transitions_is_kept`:

1. 停止点で `task_ops::gate::cancel` を確定する。確かめること: Task は Cancelled、lease は無い。A の `WorkerFinished` は
   `end: cancelled` の 1 件（R6-1 の閉じ）で、`runs` 行の `finished_at` を控える。`Transitioned` と b・c の行・遷移を控える。
2. 旧 dispatcher を落とし、新しい active の最初の tick（起動時の照合）を呼ぶ。`TickReport` は `reclaimed`・`dispatched`・
   `finished`・`in_flight` がすべて 0。
3. 確かめること（`assert_kept`。最初の tick と、その後の 3 回の tick の各回）: Task は Cancelled で lease 無し。WU a の
   遷移は dispatch・completed の 2 つだけで done のまま。A の `WorkerFinished` は 1 と同じ 1 件、`runs` 行は cancelled で
   `finished_at` は変わらない。`orphan_takeover` の記録は 0 件。`WorkerStarted` は A の 1 件だけ。b・c の行と遷移は 1 と
   同じ（`dependency_ready` も無い）。`Transitioned` は 1 から増えない。`ReviewVerdict`・delivery は無い。`runs_running` は空。

`cex_human_comment_between_wu_and_task_transitions_is_kept`:

1. 停止点で `task_ops::comment::post_human_comment_as`（author `human:web`）を確定する。確かめること: effect は
   `Interrupted`、遷移は Running → Ready（理由 `comment`）、lease 無し、attempts は停止点と同じ。A の `WorkerFinished` は
   `interrupted: comment`（end 無し）の 1 件。
2. 旧 dispatcher を落とし、新しい active を `run_until_state` で Task の Done まで回す。
3. 確かめること: comment は 1 件残る。`Transitioned{Ready, comment}` は 1 件のまま残る。A の `WorkerFinished` は 1 と同じ
   1 件で、`runs` 行は running でない。WU a の遷移は dispatch・completed の 2 つだけ。`WorkerStarted` は A・b の run・
   c の run の 3 件（再起動の後に起こした run は done でない WU b・c だけ）。b・c は done で、`dependency_ready` は各 1 回。
   `runs_running` は空。

各試験の doc comment に、守る不変条件と、`cex_restart_after_crash_between_wu_and_task_transitions_is_idempotent`・
`cex_restart_after_wu_done_promotes_dependents_once` が停止点と再起動の間に人の操作を挟まない（Task が Running のまま
再起動し、回収が A を閉じて Done まで進めることだけを固定する）ので検出できない理由を書いた。

### 観測

- cancel: 停止点の後の cancel が、同じトランザクションで lease を外し、A の `runs` 行を `WorkerFinished{end: cancelled}`
  で閉じる。再起動の照合はどれも A を見ない（`reclaim_expired_leases` は Running の Task だけ、持ち主の居ない run と
  終端の task の照合は running の `runs` 行だけを見る）。dispatch の選別（store の `ready_tasks` と `acquire_lease` の
  状態の照合）は Cancelled の Task を拾わない。
- comment: comment の `WorkerFinished{interrupted: comment}` の append が A の `runs` 行を同じトランザクションで閉じる
  （Phase F5-fix3）。そのため再起動の `reconcile_ownerless_runs` は A を閉じ直さず、`close_aborted_run` の WU の回収
  （`reconcile_work_unit_run`）にも届かない。再起動の後の dispatch では `wu_dispatch_gate` の `promote_newly_ready` が b を
  上げ、`next_work_unit` が ready の b（続いて c）を選ぶ。

### 証拠（tick-wu-human。提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

検査は試験の commit（`tick-wu-human: pin a human cancel and comment between the WU done and the Task transition across a restart …`）
の上で実行した。その後の変更はこの進捗ファイルの文面だけ。

- `cargo nextest run -p task-dispatch -E 'test(/cex_same_tick::/)'` → exit 0（3 passed）。
- `bash scripts/dev/test-affected.sh` → exit 0。3949 本すべて合格（11 skipped、9 slow、418.6 秒）。script bundle の SKIP は 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）。
- `bash scripts/dev/check-doc-links.sh`・`bash scripts/dev/check-adr-numbers.sh` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → `cex_same_tick.rs`・`cex_lease_restart.rs`・この進捗ファイルの 3 つだけ。
- `git diff --stat $CELERIS_WU_BASE -- ':!crates/task-dispatch/src/dispatcher/tests' ':!agent-docs'` と
  `git diff --stat $CELERIS_WU_BASE -- crates/task-dispatch/src/dispatcher/tests/hooks.rs` → どちらも差分なし。

## guard を外した確認

WU ごとに節を分ける。tick-wu-human は (ii) の試験の行を自分の節に書く。

### tick-expiry（`cex_done_and_lease_expiry_in_one_tick_close_the_run_once`）

一時変更は `git checkout --` で戻し、commit していない（戻した後に `grep -rn "GUARD-OFF" crates/` は 0 件、
`git status --short` に本体の file は無い）。command はどれも
`cargo nextest run -p task-dispatch -E 'test(/cex_same_tick::/)'`。

| 変えた行 | command | exit | 落ちた assert | 戻した後の結果 |
|---|---|---|---|---|
| G1（完了処理の照合）: `crates/task-dispatch/src/dispatcher/worker_finish.rs` の `on_worker_finished` の `if !lease_matches {` を `if false && !lease_matches {` にする（cancel 済み・lease の保持者でない run の Done を捨てない） | 上の command | 100 | `cex_same_tick.rs` の `assert_closed_once` の `runs` 行の status: `A's runs row in the same tick`、left `Completed`、right `Cancelled`。cancel で閉じた A の `runs` 行を、捨てるべき Done が completed に書き換えた | `git checkout -- crates/task-dispatch/src/dispatcher/worker_finish.rs` の後、上の command は exit 0（1 passed） |
| G2（回収の照合）: `crates/task-dispatch/src/dispatcher/leases.rs` の `reclaim_expired_leases` の `self.store.list(Some(Status::Running))?` を `self.store.list(None)?` にする（Cancelled の Task も回収の対象に入れる） | 上の command | 0 | 無し（合格）。cancel が同じトランザクションで lease を外すので、回収は lease の無い Task を飛ばす。この順序で回収側を守るのは store の lease の解放（`apply_transition` の `leaving_running`）で、試験は 3 でそれを assert する | `git checkout -- crates/task-dispatch/src/dispatcher/leases.rs` の後、`cargo nextest run -p task-dispatch -E 'test(/cex_/)'` は exit 0（35 passed） |

### tick-wu-human（`cex_human_cancel_between_wu_and_task_transitions_is_kept`・`cex_human_comment_between_wu_and_task_transitions_is_kept`）

一時変更は `git checkout --` で戻し、commit していない（戻した後に `grep -rn "GUARD-OFF" crates/` は 0 件、
`git status --short` に本体の file は無い）。command はどれも
`cargo nextest run -p task-dispatch -E 'test(/cex_same_tick::/)'`。

| 変えた行 | command | exit | 落ちた assert | 戻した後の結果 |
|---|---|---|---|---|
| G3（dispatch の選別で Cancelled を照合する箇所）: `crates/task-core/src/store/tasks.rs` の `ready_tasks_impl` の `WHERE status = ?1` を `WHERE (status = ?1 OR status = 'cancelled')` に、`acquire_lease_impl` の状態の照合 `status_col != status_str(Status::Ready)` を `(status_col != status_str(Status::Ready) && status_col != "cancelled")` に、同じ関数の `UPDATE … WHERE id = ?6 AND status = ?7` を `… AND (status = ?7 OR status = 'cancelled')` にする（3 か所を一緒に変える。どれか 1 つが残れば Cancelled の Task は lease を取れない） | 上の command | 100 | cancel の試験の再起動の最初の tick の `report.dispatched == 0`: left 1、right 0（`TickReport{dispatched: 1, in_flight: 1}`。新しい active が Cancelled の Task を dispatch した）。tick-expiry の試験も手順 5 の 1 回の tick の `report.dispatched == 0` で落ちた（left 1、`TickReport{finished: 1, dispatched: 1}`）。comment の試験は合格 | `git checkout -- crates/task-core/src/store/tasks.rs` の後、`bash scripts/dev/test-affected.sh` で cex_same_tick の 3 本を含め全部合格（下の証拠） |
| G4（dispatch の選別で done の WU を除く箇所）: `crates/task-core/src/execution_plan/scheduling.rs` の `next_work_unit` の `.filter(\|u\| u.status == WorkUnitStatus::Ready)` を `.filter(\|u\| u.status == WorkUnitStatus::Ready \|\| u.status == WorkUnitStatus::Done)` にする（done の WU a を「次の WU」に選ぶ） | 上の command | 100 | comment の試験の `reached`（Task の Done 待ち）: 60 秒（`STATE_WAIT_GUARD`）で false。`status=Running`、`Transitioned` は `comment` の後に `dispatch`・`advance` の繰り返し（`dispatch` は 462 件）、`WorkerStarted` も同じく積み上がった（WU a を走らせ直し続けた）。cancel と tick-expiry の試験は合格 | `git checkout -- crates/task-core/src/execution_plan/scheduling.rs` の後、同上 |
| G5（再起動の照合で done の WU を戻さない箇所）: `crates/task-dispatch/src/dispatcher/phase_integration.rs` の `reconcile_work_unit_run` の `u.status == task_core::WorkUnitStatus::Running` を `(u.status == … Running \|\| u.status == … Done)` にする（持ち主の居ない run を閉じるとき done の WU も回収する） | 上の command | 0 | 無し（3 本とも合格）。comment の `WorkerFinished` の append が A の `runs` 行を同じトランザクションで閉じる（Phase F5-fix3）ので、再起動の `reconcile_ownerless_runs` は A を見ず、`close_aborted_run` からこの関数に届かない。この順序で A を 1 回だけ閉じ WU a を戻さないのを守るのは store の閉じで、試験は 1 の `WorkerFinished` の 1 件と、再起動の後の `runs` 行・WU a の遷移でそれを assert する | `git checkout -- crates/task-dispatch/src/dispatcher/phase_integration.rs` の後、同上 |

## 未解決事項

- 無し。comment の変種は Running の Interrupt だけを置いた。質問の回答待ちの Blocked（`Answer`）の変種は置いていない
  （objective では任意）。
- objective は「v2 の計画の Task」と書くが、v2 では `halt_after_wu_transition` の停止点が同じ tick の
  `reconcile_parallel_tasks` で進んでしまう（作業前 brief の 4）。v2 の停止点を試験で作るには tick の残りも止める hook が
  要り、`tests/hooks.rs` を変えることになるので、この WU では置かない（下の提案）。

## 提案

- v2 の Task で「WU の done の直後に落ちる」を作る試験には、`halt_after_wu_transition` が finish だけでなくその tick の
  残り（`reconcile_parallel_tasks`・`dispatch_ready`）も止める hook が要る。要るなら `tests/hooks.rs` を変える別の unit に
  する。
