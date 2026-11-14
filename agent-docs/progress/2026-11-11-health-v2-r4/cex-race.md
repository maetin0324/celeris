---
title: "健全性v2 R4 cex-race: lease の renew の競合と、ownerless で閉じた旧 reviewer の遅れた合格の反例試験"
tasks: [01M7DYMCZ4T7C5RNQ3JZQAF4G7]
status: blocked
updated: 2026-11-14
---
# 健全性v2 R4 cex-race

## old-review: ownerless で閉じた旧 reviewer A の遅れた合格

結論: 反例は今の実装で成り立つ（不変条件が破れる）。試験 `cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored`
は assert を残したまま落ちる。本体は変えていない。この unit の check は落ち、repair か replan に回す（D7 の規則）。

基準は `$CELERIS_WU_BASE`。本体（`cfg(test)` の外）の差分は無い。

### 変更

- `old-review: pin the old reviewer's late pass after the new active closed it as ownerless (cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored)`
  — `crates/task-dispatch/src/dispatcher/tests/cex_lease_restart.rs` の
  `cex_draining_instance_reclaims_its_expired_lease_without_redispatching` の直後に、adapter `VerdictGate`、helper
  `reviewer_runs`・`finish_outcomes`・`tick_until_review_delivered` と試験 1 本を足した。`use` は試験の関数の中に書いた。
- この進捗ファイル。

### 作業前 brief

1. 根本原因: D7 の restart の行（新 active が reviewer A を ownerless として閉じ、B を始めた後に、旧 draining instance から
   A の合格が届く）に試験が無い（ADR 付記 2026-11-14 の「試験の無い部分」）。
2. 責務 owner: 試験は `dispatcher/tests/`。対象の判断は `dispatcher/ownerless_runs.rs`（`reconcile_ownerless_runs` が A を閉じる）、
   `dispatcher/review_spawn.rs`（`spawn_review` の task ごとの review lock）、`dispatcher/review_verdict.rs`（`on_review_finished` が
   判定を適用する）、`dispatcher.rs`（`drain_completions`）。
3. 既存の再利用先: `orphan_takeover.rs` の `instance`・`new_active`、`tests/mod.rs` の `dispatcher`・`new_task`・`run_until_state`・
   `events_of`・`done_outcome`・`STATE_WAIT_GUARD`、`cex_lease_restart.rs` の `finishes`・`index_status`・`status_of`。
4. 選択肢: A を閉じる方法は (a) 注入時計で T1c の期限を越える、(b) 旧 instance の行を消す、の 2 つ。(a) は時計を進めるのに
   Dispatcher の private field（`test_hooks.now`）か `config.review_timeout` に触る必要があるので、(b) を採った。
5. 新しい状態・分岐・fallback・port: 無し（試験だけ）。
6. 撤去対象: 無し。
7. 不変条件: A の `runs` 行は Running に戻らない。A の合格は `ReviewVerdict`・Task の遷移に使われない。Task は B の結果だけで
   決まる（A 合格・B 不合格で区別し、Done にならない）。旧 instance が A を手元に持つ間、新 active は B を起こさない。

### 試験

`cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored`（Dispatcher の private field に触らない。待ちは
`run_until_state` と `TickReport.reviewed` の出来事待ちで、保険は壁時計の `STATE_WAIT_GUARD`。reviewer の終わりは
`VerdictGate` の semaphore で試験が決める。CPU 負荷なし）:

1. 同じ in-memory DB に旧 instance（行を登録、active）を作る。旧が reviewing の Task の reviewer A を起こし、A は gate で止まる。
   旧を draining にする（`set_accepting_new_work(false)`、行の role も draining）。
2. 旧 instance の行を消す（旧の dispatcher は A を手元に持ったまま）。新 active（`new_active`）の照合が A を
   `WorkerFinished{end: cancelled, outcome: "interrupted: orphan_takeover: no live daemon instance holds this run"}` で閉じる。
3. assert: 新 active は B を起こさない（旧が A の review lock を持つ）。Task は reviewing。
4. A の gate を開け、旧の tick で A の合格を `drain_completions` に通す。
5. assert: A の `runs` 行は Running でない。旧の drain の後に `ReviewVerdict` と `Transitioned` が無い。Task は reviewing。
6. 新 active が B を起こし、B を不合格で終える。assert: `ReviewVerdict` は不合格 1 件だけ、最初の遷移は
   `ready / review_fail`、Done・`review_pass` が無い、B の `WorkerFinished` は completed 1 件。

doc comment に、守る不変条件と、`tests/ownerless_runs.rs` の T1a〜T1c がこの順序を作らない理由を書いた（T1a・T1b は
`seed_reviewer_run` で行を直接作り、合格を届けうる持ち主が居ない。T1c は draining の他の instance の行だけを登録し、
その instance は reviewer run を走らせない。Task も blocked）。

### 観測（今の実装で反例が成り立つ）

手順 1〜3 は assert どおりに進み、手順 5 で落ちる。

```
panicked at crates/task-dispatch/src/dispatcher/tests/cex_lease_restart.rs:615:5:
A's late pass was used after A was closed as an ownerless run (A's runs row Some(Completed),
A's WorkerFinished ["interrupted: orphan_takeover: no live daemon instance holds this run", "done: ok"]):
[WorkerProgress { msg: "reviewer run <A>: done: ok" }, Transitioned { from: Reviewing, to: Done, reason: "review_pass" },
 WorkerFinished { run_id: <A>, outcome: "done: ok", role: Some(Reviewer), end: Some(Completed) }, QuotaEstimated { run_id: <A> },
 ReviewVerdict { criterion_idx: 0, pass: true, reason: "reviewer(<A>): reviewer: looks complete" }]
```

- A の合格が Task を Done にした（`review_pass`）。`on_review_finished` は Task が reviewing かだけを見て、reviewer run の行が
  別の instance に閉じられたかを見ない。
- A の `WorkerFinished` が 2 件になった（ownerless の閉じと、旧の通常の終わり）。`runs` 行は cancelled から completed に
  書き換わった（`run_index_finish` は status を問わず上書きする）。Running には戻っていない。
- 再現: `cargo nextest run -p task-dispatch -E 'test(/cex_restart_old_reviewer/)'`（exit 100、0.35 秒前後で落ちる）。

D7 の表の順序そのもの（B を始めた後に A の合格が届く）は、今の実装では作れない。旧 instance が A の `ReviewEntry` と一緒に
task ごとの review lock（`runs/.review-<id>.lock`、`spawn_review` の `try_lock`）を判定の保存まで持つので、新 active は A を
閉じた後も B を起こせない。試験はこの lock が B を止めることも assert し、lock の下で作れる順序（A を閉じる → B は lock で
起きない → A の合格が届く → B）で不変条件を確かめた。

### guard を一時的に外した確認

一時変更は `git checkout --` で戻し、commit していない（戻した後に `grep -rn "GUARD-OFF\|GUARD-PROTO" crates/` は 0 件、
`git status --short` は空）。

| 一時変更 | 結果 |
|---|---|
| G-lock: `review_spawn.rs` の `spawn_review` で `TryLockError::WouldBlock` を `return Ok(false)` から素通しに変える（review lock を無視） | 落ちる（手順 3）。`assertion left == right failed: the new active started a reviewer while the old instance still holds review A`（left は reviewer run 2 件、right は A の 1 件）。lock が無いと D7 の表の順序（A を閉じた後、旧が A を持つ間に B が起きる）が作れる |
| A の合格を捨てる照合 | 今の実装には無い（上の観測）。試験は外す前から手順 5 で落ちる |
| G-proto（逆向きの確認）: `review_verdict.rs` の `on_review_finished` の `take_review` の直後に「`outcome.reviewer_run` の `runs` 行が Running でなければ何も書かずに返る」照合を一時的に足す | 試験は合格（手順 1〜6 すべて）。B が lock の解放後に起き、不合格で `review_fail` → ready、Done にならない。同じ build で `-E 'test(/cex_/) \| test(/ownerless/) \| test(/review/)'` は 363 本すべて合格 |

G-proto は確認のための一時変更で、修正の案ではない（A の quota の精算・`WorkerFinished` の扱いを決めていない）。

### 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-dispatch -E 'test(/cex_/) | test(/ownerless/)' --no-fail-fast` → exit 100。48 本中 47 本合格、
  `cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored` の 1 本が上の観測で失敗。
- `bash scripts/dev/test-affected.sh` → exit 100。3944 本中 3943 本合格（11 skipped）、失敗は同じ 1 本だけ。対象 crate は
  celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `git diff --stat $CELERIS_WU_BASE HEAD -- ':!crates/task-dispatch/src/dispatcher/tests' ':!agent-docs'` → 差分なし（本体は変えていない）。

### 2 回目の run: check の不合格の確認（2026-11-14）

前回の run の後、celeris の check 2 本（`cargo nextest run -p task-dispatch --no-fail-fast --no-tests=fail -E 'test(/cex_lease_restart::/)'`
と `bash scripts/dev/test-affected.sh`）が exit 100 で落ちた。原因は上の観測と同じで、新しい試験が A の合格による
`Reviewing → Done (review_pass)` で落ちる。

- 提出時の HEAD で同じ check を走らせ直した: `cargo nextest run -p task-dispatch --no-fail-fast --no-tests=fail -E 'test(/cex_lease_restart::/)'`
  → exit 100。`cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored` が同じ assert（`A's late pass was used after A
  was closed as an ownerless run`、A の `runs` 行 `Some(Completed)`、A の `WorkerFinished` 2 件）で落ちた。
- `review_verdict.rs` の `on_review_finished` を読み直した。判定を捨てる条件は `task.status != Status::Reviewing` と
  `target_stale` だけで、reviewer run の `runs` 行が閉じたかは見ていない。`crates/celeris/src/instance.rs` の `step` は
  自分の行が消えると `register_again` で登録し直すだけで、手元の完了を捨てない。反例は本番の構成でも成り立つ。
- この unit は本体（`cfg(test)` の外）を変えないので、試験をどう直しても check は exit 0 にならない。done を返して
  再試行しても同じ結果になるため、この run は計画の問題（`plan_issue`）として申告し、修正の unit（repair）か replan に回す。
- 同じ実行で、base からある `cex_run_finish_and_work_unit_check_finish_in_one_tick` も 1 回落ちた（`cex_lease_restart.rs`
  の `assert_eq!(report.finished, 2)`、left 1・right 2、`drain_completions` に 477 ms、host の load average 19.6）。
  単独で 3 回走らせると 3 回とも合格した（exit 0）。この試験は flag を置いた後に run `b` の完了だけを待って tick するので、
  WU `a` の検査（`sleep 0.05` の loop）の完了がその tick に間に合わないと落ちる。この unit の範囲外で、変えていない。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0、
  `sh "$CELERIS_WU_SCOPE_PATHS"` → この 2 file だけ、本体の差分なし。

### 3 回目の run: plan_issue が届かなかった理由（2026-11-14）

- 2 回目の run の `result.json` は `summary` と `yield` の両方を持っていた。`crates/task-worker/src/result_report.rs` は
  `question` > `wait` > `summary` > `yield` の順に読むので、celeris はこれを done と読み、check を走らせて retry（2/2）にした。
  `plan_issue` は計画に届いていない。
- 提出時の HEAD で `cargo nextest run -p task-dispatch --no-fail-fast --no-tests=fail -E 'test(/cex_lease_restart::/)'` を
  走らせ直した → exit 100。11 本中 10 本合格、`cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored` が同じ assert
  （`cex_lease_restart.rs:615`、A の `runs` 行 `Some(Completed)`、A の `WorkerFinished` 2 件）で落ちた。試験・本体とも変えていない。
- この run は `result.json` に `yield`（`plan_issue`）だけを書き、`summary` を書かない。

### 未解決

- `cex_run_finish_and_work_unit_check_finish_in_one_tick` は負荷の下で落ちうる（上の 2 回目の run の節）。修正の unit の
  check（`-E 'test(/cex_lease_restart::/)'`）を巻き込むので、検査の完了を出来事で待つ形に直す unit を別に置くとよい。
- 反例（今の実装で成り立つ）: 新 active が reviewer run A を ownerless として閉じた後に、A を持っていた旧 draining instance に
  A の合格が届くと、`on_review_finished` がその合格で Task を Done にし、A の `WorkerFinished` を 2 件目として書き、`runs` 行を
  cancelled から completed に書き換える。この unit の範囲（本体を変えない）では直さない。試験は assert を残して落ちる。

### 提案

- owner は `dispatcher/review_verdict.rs` の `on_review_finished`（判定を適用する所）。A の `runs` 行が既に閉じていれば
  （別の instance が ownerless として閉じた）、判定・遷移・`WorkerFinished` を書かずに捨て、lock を放して Task を reviewing の
  まま残す（次の review が決める）。行の照合は判定の適用と同じ書き込みの中で行う方が、照合と書き込みの間に他の instance が
  閉じる競合を残さない（store の比較更新が要るなら store の高 risk の class に当たる）。quota の精算を捨てた run でも行うかは
  修正の unit で決める。
- 別の選択肢は、ownerless の照合（`reconcile_ownerless_runs`）が review lock を取れない reviewer run を閉じないこと（lock を持つ
  process が生きている印として使う）。この場合は試験の手順 2 の assert（A を閉じる）を変える必要があり、D7 の表の行の前提も
  変わるので、replan で決める。
