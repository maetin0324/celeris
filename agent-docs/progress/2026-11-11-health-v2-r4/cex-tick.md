---
title: "健全性v2 R4 cex-tick: 同じ tick の Done と lease の期限、WU と Task の遷移の間の人の操作の反例試験"
tasks: [01M7EQJXQB3S4Q8FS54JEYZFVW]
status: running
updated: 2026-11-15
---
# 健全性v2 R4 cex-tick

この file は WU tick-expiry が作った。WU tick-wu-human が (ii) の試験の節を足し、file を締める（status done）。

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

## 未解決事項

- (ii) `cex_human_cancel_between_wu_and_task_transitions_is_kept` と comment の変種は WU tick-wu-human の担当。

## 提案

- 無し。
