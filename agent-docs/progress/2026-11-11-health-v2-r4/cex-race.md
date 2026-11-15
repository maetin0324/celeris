---
title: "健全性v2 R4 cex-race: lease の renew の競合と、ownerless で閉じた旧 reviewer の遅れた合格の反例試験"
tasks: [01M7DYMCZ4T7C5RNQ3JZQAF4G7]
status: running
updated: 2026-11-15
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

### replan v2: old-review（2026-11-14）

計画 v2 で old-review は「反例の試験を残し、同じ file の不安定な既存試験を出来事待ちに直す」になった。本体の修正は次の unit
fix-verdict（`review_verdict.rs` の `on_review_finished`）が行う。この run の変更は試験と進捗だけで、本体（`cfg(test)` の外）は
変えていない。

変更（commit の件名）:

- `old-review: wait for the WU check's last WorkUnitCheckFinished before the one tick (cex_run_finish_and_work_unit_check_finish_in_one_tick)`
  - `cex_run_finish_and_work_unit_check_finish_in_one_tick`: flag を置き、run `b` を返した後、WU `a` の検査の列の最後の項目の
    `WorkUnitCheckFinished`（`key == "a"`、`index + 1 == total`）が store に載るまで待ってから 1 回 tick する（保険は
    `STATE_WAIT_GUARD`、20 ms の sleep で待つ。tick の回数では待たない）。検査の task は最後の項目の `record_finish` から
    `Completion::WorkUnitChecks` の送信まで await を挟まないので、current-thread の runtime でこの event が見えれば完了は
    channel に入っている。assert（`report.finished == 2`、各 WU done、`WorkerFinished` 各 1 件、`runs` 行 completed、統合 1 回）は
    変えていない。doc comment に待ち方と変えた理由を書いた。
  - `cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored`: 手順と assert は変えていない（足した commit の版と関数の本体
    142 行が一致）。doc comment から「今の実装では」の語を外した（不変条件と T1a〜T1c の理由は残した。観測はこのファイルに置く）。
- この進捗ファイル（この節、front matter の status を running に）。

出来事待ちの確認（試験専用の遅延。一時変更は `git checkout --` で戻し commit していない。戻した後 `git status --short` は空）:

| 一時変更 | 結果 |
|---|---|
| WU `a` の検査の command の `test -f a.txt` の前に `sleep 1` を足す（新しい待ち方のまま） | 合格（exit 0、2.8 秒） |
| 同じ `sleep 1` のまま、足した待ち（`a_checks_done` の loop と assert）を外す（以前の待ち方） | 落ちる（exit 100）。`cex_lease_restart.rs:1099` の `assert_eq!(report.finished, 2)` が left 1・right 2。load 19.6 で観測した失敗と同じ |

証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）:

- `cargo nextest run -p task-dispatch --no-fail-fast --no-tests=fail -E 'test(/cex_lease_restart::/)'` → exit 100。11 本中 10 本合格、
  落ちたのは反例の試験 `cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored` の 1 本だけ（`cex_lease_restart.rs:615`、
  上の「観測」と同じ）。
- `cex_run_finish_and_work_unit_check_finish_in_one_tick` を単独で 5 回 → 5 回とも合格（load average 4.9）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/check-health.sh` → exit 0（repo-health の verdict pass、fail_grade 0、new 1・worsened 0、検査器の試験 99 本と gate の試験 7 件も合格）。
- `bash scripts/dev/test-affected.sh` はこの unit では走らせない。反例の試験が本体の修正まで落ちるので exit 0 にならない。
  fix-verdict が本体を直した後に走らせる。

## fix-verdict: 閉じた reviewer run の遅れた判定を捨てる（2026-11-14）

結論: `review_verdict.rs` の `on_review_finished` を直した。reviewer run を伴う結果を受けたら、遷移・event・`runs` 行の更新より
前に `run_index_get` でその run の行を読み、行があって `running` でなければ結果を捨てる。反例の試験
`cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored` は試験 file を変えずに合格する。基準は `$CELERIS_WU_BASE`。

### 変更（commit の件名）

- `fix-verdict: ADR ownerless-running-runs addendum (discard a closed reviewer run's late verdict in on_review_finished)`
  — `agent-docs/adr/2026-10-03-ownerless-running-runs.md` に付記 2026-11-14 を足し、D3 の「持ち主が後から書く
  `WorkerFinished` はそのまま追記される」の一文に置き換えの注を付けた。
- `fix-verdict: on_review_finished discards a reviewer run's late result once its runs row is closed`
  — `on_review_finished` の部署の lead session の usage の記録の後、`task.status != Reviewing` の判定の前に照合を 1 つ足した。
  捨てるとき: Task の遷移・`ReviewVerdict`・`ReviewerPassed`・`RequiredChecksEvaluated`・delivery の判定に使わない。
  2 件目の `WorkerFinished` を書かず、`runs` 行を書き換えない。take した `ReviewEntry` を放す（review lock が解ける）。
  `defer_review` の持ち越しは置かない。tracing の warn に run id・review run id・行の status を出す。行が無いとき・
  `running` のときは従来どおり。
- `fix-verdict: keep on_review_finished and the dispatcher module from growing (merge the two report records, flatten the lead-session usage, trim the guard)`
  — 前の run の check（repo-health）の不合格を直した（下の「前の run の check の不合格」）。挙動は変えていない。
- この節。試験 file（`crates/task-dispatch/src/dispatcher/tests/`）は変えていない。

### 前の run の check の不合格（2026-11-15）

前の run は done を返したが、celeris の check `celeris:repo-health/1` が `size.responsibility-growth` の 2 件で落ちた。
`check-health.sh` の既定（base auto = main）では 2 件とも existing で exit 0 になり、前の run はこれで合格と読んでいた。
基準を `$CELERIS_WU_BASE` にすると再現した:
`python3 scripts/dev/repo-health/repo_health.py --repo scripts/dev --base "$CELERIS_WU_BASE" --head worktree --policy-version 1`
→ exit 1、worsened 2。

| finding | WU base | 前の run の HEAD | この run の HEAD |
|---|---|---|---|
| `on_review_finished` の関数の行（閾値 200、注釈と空行も数える） | 563 | 581（worsened） | 563（existing） |
| module `crates/task-dispatch/src/dispatcher` の本体の行（閾値 8000、注釈・空行・use を除く） | 31588 | 31602（worsened） | 31585（existing） |

直し方（`review_verdict.rs` の中だけ。照合の条件・捨てるときの動作・quota の扱いは変えていない）:

- 部署の lead session の usage の記録: `org_list` の `match` の入れ子を `let` の連鎖にした（`org_list` の失敗の warn は
  `inspect_err` で同じ文のまま残した）。
- 報告の記録: `Status::Done`（`result`）と `Status::Failed`（bad_news）で 2 回書いていた `record_run_report` の呼び出しを、
  報告の種類を `match` で選んでから 1 回呼ぶ形にした。宣言された `report.kind` は従来どおり `Done` のときだけ読む。
- 足した照合: tracing の warn を 1 行にし、`drop(entry)` を消した（return で entry は落ちる。`take_review` が `reviewing`
  の表から外した時点で review lock は解けている）。quota の event の追記は `if let` にした。注釈を 3 行にした。

`#[path]` で file を dispatcher の module の外に移す方法（`review_limits` の形）は採らなかった。数え方の上で減るだけで、
責務は減らないため。

### 作業前 brief

1. 根本原因: `on_review_finished` が判定を捨てる条件は `task.status != Reviewing` と `target_stale` だけで、reviewer run の
   `runs` 行（lease を持たない reviewer run の生存の記録）が別の instance の ownerless の照合で閉じたかを見ない。
2. 責務 owner: `crates/task-dispatch/src/dispatcher/review_verdict.rs` の `on_review_finished`（判定を適用する所）。
3. 既存の再利用先: `TaskStore::run_index_get`（`ownerless_runs.rs` の `reconcile_ownerless_work_units` と同じ読み方）、
   この関数の先頭の `reviewer_quota`（`settle_quota` の 1 回の精算）。試験は old-review の反例の試験をそのまま使う。
4. 選択肢: (a) `on_review_finished` で行を読んで捨てる（採用。owner で直り、D3 の判定は変えない）。(b) D3 が review lock を
   取れない reviewer run を閉じない（D3 の判定の材料と反例の試験の前提を変えるので採らない）。(c) store の比較更新で判定の
   書き込みと照合を 1 つの transaction にする（store の高 risk の class。提案に回す）。
5. 新しい状態・分岐・fallback・port: 分岐 1 つ（閉じた行の結果を捨てる）。状態・fallback・port は無し。
6. 撤去対象: ADR D3 の「持ち主が後から書く `WorkerFinished` はそのまま追記される（行は終端のまま）」の決定（付記で
   置き換え）。コード・試験の撤去は無し。
7. 不変条件: 閉じた reviewer run の結果は Task の遷移・判定・`WorkerFinished`・`runs` 行に使われない。確かめる試験は
   `cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored`。

### quota の扱いとその理由

捨てる場合も、結果の usage で `settle_quota` を 1 回だけ通し（関数の先頭の `reviewer_quota`。変えていない）、A の
`QuotaEstimated` を書く。

- 旧 instance の `QuotaActivity` に A が開いたまま残ると、同じ account の重なりの判定が狂う（ADR-0074 D4）。閉じるのは
  `quota_begin` を呼んだ旧 instance だけができる。
- ownerless の照合（新 active の `reconcile_ownerless_runs` → `close_aborted_run`）は quota を精算しない。閉じた側の
  `WorkerFinished` は usage を持たず、`QuotaEstimated` も書かない。新 active の `QuotaActivity` に A は無い。したがって
  旧 instance の 1 回が A の唯一の精算で、二重に数えない（`QuotaEstimated` は同じ run id の最後の event が有効なので、
  仮に重なっても畳まれる）。
- usage 無しの解放（`release_quota_if_tracked`）は採らない。A の重みが 0 になり、同じ account の group が閉じるときに
  他の run が A の消費を按分で負う。
- `runs` 行は書き換えないので、A の `runs.usage_json` は閉じた側の値（usage 無し）のまま残る。案件の USD 上限は `runs` 行
  を読む（`project_run_usage_between`）ので、adapter・model の入った A の行は費用不明として数えられる（fail-closed。試験では
  確かめていない。未解決に書いた）。

### 照合を一時的に外した確認

一時変更は `git checkout --` で戻し、commit していない（戻した後に `git status --short` は空、`grep -rn "GUARD-OFF" crates/` は 0 件）。

| 一時変更 | 結果 |
|---|---|
| 足した照合の条件に `&& false` を足す（照合を無効にする） | `cargo nextest run -p task-dispatch --no-fail-fast --no-tests=fail -E 'test(/cex_lease_restart::/)'` → exit 100。11 本中 10 本合格、`cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored` が `cex_lease_restart.rs:615` の `A's late pass was used after A was closed as an ownerless run`（A の `runs` 行 `Some(Completed)`、A の `WorkerFinished` 2 件、`Transitioned{Reviewing → Done, review_pass}`）で落ちた。old-review の節の観測と同じ |
| 戻した後（照合あり） | 同じ command で 11 本すべて合格 |
| この run（2026-11-15）の形に直した後、同じく照合の条件に `&& false // GUARD-OFF` を足す | 同じ command で exit 100。11 本中 10 本合格、`cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored` が `cex_lease_restart.rs:615` で落ちた |
| `git checkout --` で戻した後 | 同じ command で exit 0、11 本すべて合格。`git status --short` は空、`grep -rn "GUARD-OFF" crates/` は 0 件 |

### 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-dispatch --no-fail-fast --no-tests=fail -E 'test(/cex_/) | test(/ownerless/) | test(/review/)'`
  → exit 0、363 本すべて合格。
- `bash scripts/dev/test-affected.sh` → exit 0、3944 本すべて合格（11 skipped）。対象 crate は celeris・celeris-campaign・
  celerisctl・llm-proxy・task-dispatch。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `git diff --stat $CELERIS_WU_BASE HEAD -- crates/task-dispatch/src/dispatcher/tests` → 差分なし（試験 file は変えていない）。
- `python3 scripts/dev/repo-health/repo_health.py --repo scripts/dev --base "$CELERIS_WU_BASE" --head worktree --policy-version 1`
  → exit 0、verdict pass、fail_grade 0、new 0・worsened 0。
- `bash scripts/dev/check-health.sh`（base auto）→ exit 0。

### 未解決

- 反例（旧 reviewer A の遅れた合格）は fix-verdict が `on_review_finished` で直した（fix-verdict の節）。
  `cex_run_finish_and_work_unit_check_finish_in_one_tick` の負荷依存は replan v2 の old-review の節で直した。
- 読みと書きの間の窓: fix-verdict の照合は `run_index_get` で行を読んでから判定を書く。その間に別の instance の
  ownerless の照合が行を閉じると、遅れた判定は今までどおり適用される（worker run の `run_holds_lease` の照合と同じ水準）。
  試験は無い。
- 捨てた reviewer run の usage は `runs` 行に載らない（閉じた側の usage 無しのまま）。案件の USD 上限はこの run を費用不明として
  数えるはず（fail-closed。`project_run_usage_between` の述語を読んだだけで、試験では未確認）。`QuotaEstimated` には載る。
  ownerless の照合で閉じた run の usage が `runs` 行に残らないのは、この変更の前から同じ（D3 の閉じは usage を持たない）。

### 提案

- store の比較更新（fix-verdict の読みと書きの間の窓を閉じる）: 判定の適用（`apply_transition_with_events`・`complete_plan`
  と reviewer run の `WorkerFinished`・`run_index_finish`）を、reviewer run の `runs` 行が `running` であることを同じ writer
  transaction の中で照合してから書く store の API にする。行が閉じていれば何も書かずに知らせる。`crates/task-core/src/store/`
  の高 risk の class に当たるので、設計 brief と設計 review を通す別の変更で行う。
- 別の選択肢（採らなかった）: ownerless の照合（`reconcile_ownerless_runs`）が review lock を取れない reviewer run を閉じない
  （lock を持つ process が生きている印として使う）。D3 の判定の材料と、反例の試験の手順 2 の前提（A を閉じる）が変わる。

## renew-race: 回収側の期限切れ snapshot の直後の renew（2026-11-15）

結論: 反例は今の実装で成り立つ（不変条件が破れる）。試験 `cex_lease_renew_after_expired_snapshot_keeps_the_holder` は
assert を残したまま落ちる。本体（`cfg(test)` の外）は変えていない。この unit の check は落ち、repair か replan に回す（D7 の規則）。
基準は `$CELERIS_WU_BASE`。

### 変更（commit の件名）

- `renew-race: pin the holder's renew after the reclaimer's expired snapshot (cex_lease_renew_after_expired_snapshot_keeps_the_holder, TestHooks::after_reclaim_decision)`
  - `crates/task-dispatch/src/dispatcher/tests/hooks.rs`: `TestHooks` に一回フック `after_reclaim_decision: Option<Box<dyn FnOnce(TaskId) + Send>>` を足した。
  - `crates/task-dispatch/src/dispatcher/leases.rs`: `reclaim_expired_leases` で `reclaim_reason` が回収の理由を返した直後に、
    `#[cfg(test)] if let Some(hook) = self.test_hooks.after_reclaim_decision.take() { hook(task.id); }` の 4 行だけを足した
    （`halt_after_wu_transition`・`before_integration_start` と同じ形。既存の行は変えていない。`cfg(test)` の外の挙動は同じ）。
  - `crates/task-dispatch/src/dispatcher/tests/cex_lease_restart.rs` の末尾に試験 1 本を足した。
- この節と front matter の status（blocked）。

### 作業前 brief

1. 根本原因: D7 の表の lease の行（回収側が期限切れの snapshot を取った直後に、保持者が renew する）に試験が無い。
2. 責務 owner: 試験は `dispatcher/tests/`。対象の判断は `dispatcher/leases.rs` の `reclaim_expired_leases` と、
   `run_supervisor.rs` の `RunSupervisor::release_lease`（→ store の `apply_transition_with_events`）。
3. 既存の再利用先: `cex_lease_restart.rs` の `GatedAdapter`・`true_task`・`lease_run`・`status_of`・`finishes`・`worker_starts`・
   `index_status`・`took_over`、`orphan_takeover.rs` の `slow_adapter`、`tests/mod.rs` の `dispatcher`・`run_until_state`・`events_of`。
4. 選択肢: (a) 既存の継ぎ目（期待する行を引数に取り writer transaction の中で照合する store の API）で順序を作る、
   (b) 試験で `TaskStore` を包む store を作り書き込みの前に renew を挟む、(c) `TestHooks` に一回フックを足す。(a) は継ぎ目が
   無い（下）。(b) は `TaskStore` の 181 個の method をすべて転送する包みが要り、試験 1 本には大きすぎる。(c) を採った。
5. 新しい状態・分岐・fallback・port: `TestHooks` の一回フック 1 つ（`cfg(test)` の中だけ）。本体の状態・分岐は無し。
6. 撤去対象: 無し。
7. 不変条件: 下の「試験」。

### 既存の継ぎ目の有無（hook を足した理由）

- `reclaim_expired_leases` は同期で await を持たない。snapshot（`self.store.list(Some(Status::Running))`）から回収の書き込みまでの
  間に、試験の task も別の接続の書き込みも挟めない。
- 回収の書き込み `RunSupervisor::release_lease` → `TaskStore::apply_transition_with_events(task_id, trigger, finished)` は
  期待する lease（run id・期限）を引数に取らない。transaction の中の照合は `transition()` の状態（running か）だけ。
- 期待する行を引数に取り writer transaction の中で照合する store の API は `recover_work_unit`・`try_start_work_unit_integration`
  （WU の行）だけで、Task の lease の回収はこれを通らない。
- `dispatcher.rs` に試験から差し込める store の包み・遅延の口は無い（`TestHooks` の既存の口は `worker_start_delay`・
  `halt_after_wu_transition`・`before_integration_start` で、どれも回収の経路に無い）。

### 試験

`cex_lease_renew_after_expired_snapshot_keeps_the_holder`（Dispatcher の private field は `test_hooks` の設定だけで、既存の試験と
同じ形。待ちは `run_until_state` の出来事待ちと壁時計の保険 `STATE_WAIT_GUARD`。run の終わりは `GatedAdapter` の semaphore で試験が
決める。lease の失効は `renew_lease(.., 0)` で store の lease 時刻を今にする。CPU 負荷なし）:

1. tempdir の DB file を `SqliteStore::open` で 2 回開き、保持者の store と回収側の store にする（同じ DB の別の接続）。
2. 保持者の dispatcher が run A を起こす（gate で止まる）。保持者の store で `renew_lease(id, A, 0)` として lease を切らす。
3. 回収側の dispatcher（A を手元に持たない）に `after_reclaim_decision` を置く。フックは保持者の store で
   `renew_lease(id, A, 3600 秒)` を呼び、結果と延ばした後の lease を記録する。回収側を 1 回 tick する。
4. assert: フックが呼ばれ renew が true を返した。`report.reclaimed == 0`、Task は running、lease は延ばした lease（run id A・
   延ばした期限）と等しい、attempts は変わらない、A の `runs` 行は running、A の `WorkerFinished` が無い、`orphan_takeover` の
   event が無い、失効の後に `Transitioned` が無い。
5. A の gate を開け、保持者が A の通常の完了で Task を Done にする。A の `WorkerFinished` は 1 件、`WorkerStarted` は A だけ。

doc comment に、守る不変条件と、既存の試験（`cex_stale_holder_late_finish_does_not_overwrite_the_reassigned_run`・`orphan_takeover.rs`・
store の `renew_lease_extends_only_the_matching_running_lease`・ADR-0070 D5 の手元の run の延長）がこの順序を作らない理由を書いた。

### 観測（今の実装で反例が成り立つ）

手順 3 のフックは呼ばれ、保持者の renew は成功した（true、lease は A のまま期限が 1 時間後）。その後、回収側は古い snapshot の
判断のまま回収を書いた。手順 4 の最初の assert で落ちる:

```
panicked at crates/task-dispatch/src/dispatcher/tests/cex_lease_restart.rs:1204:5:
assertion `left == right` failed: report TickReport { reclaimed: 1, dispatched: 0, finished: 0, ... };
status Ready; attempts 0 (before 0); lease None (extended Lease { worker_run_id: "<A>", expires_at: <renew の 1 時間後> });
A's runs row Some(HarnessError);
events after the expiry [Transitioned { from: Running, to: Ready, reason: "infra_requeue" },
  WorkerFinished { run_id: "<A>", outcome: "infra_requeue: lease expired (run_id=<A>)", role: None, end: Some(HarnessError { class: LeaseExpired }) }]
  left: 1
 right: 0
```

- 保持者が延ばした直後の lease が回収側に消された（Task は ready、lease は None）。保持者の A は gate で生きたまま。
- 回収側が A の `WorkerFinished{lease_expired}` を書き、A の `runs` 行を `harness_error` に閉じた。attempts は変わらない（0）。
- `orphan_takeover` の event は無い（回収側は孤児の回収を設定していない。理由は Expired）。
- 原因: `release_lease` → `apply_transition_with_events` が snapshot の lease を照合しない（上の「既存の継ぎ目の有無」）。
- 再現: `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(/cex_lease_renew/)'`（exit 100、0.6 秒前後で落ちる）。

### guard を一時的に外した確認

回収の書き込みに lease を照合する guard は今の実装に無い（上の観測）。試験は外す前から手順 4 で落ちる。代わりに次の 2 つを
一時変更で確かめた。一時変更は `git checkout --` で戻し、commit していない（戻した後に `git status --short` は空、
`grep -rn "GUARD-OFF\|GUARD-PROTO" crates/` は 0 件）。

| 一時変更 | 結果 |
|---|---|
| G-hook-off: `leases.rs` の足した呼び出しの `hook(task.id)` を呼ばない形にする（renew を挟まない） | 落ちる（exit 100）。`the reclaimer did not decide to reclaim the expired lease, or the lease vanished: None`。フックが無いと試験の順序は作れない |
| G-proto（逆向きの確認）: `release_lease` の直前に「store の Task の lease が snapshot の lease と違えば回収しない」読み直しを足す | `cargo nextest run -p task-dispatch --no-fail-fast --no-tests=fail -E 'test(/cex_lease_restart::/)'` → exit 0、12 本すべて合格（新しい試験は手順 1〜5 すべて）。G-proto は確認のための一時変更で、修正の案ではない（読み直しと書き込みの間の窓が残る） |

### 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(/cex_/) | test(/ownerless/)'` → exit 100。49 本中 48 本合格、
  落ちたのは `cex_lease_renew_after_expired_snapshot_keeps_the_holder` の 1 本だけ（上の観測）。old-review の反例の試験
  `cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored` を含む既存の試験は合格。
- `bash scripts/dev/test-affected.sh` → exit 100。3945 本中 3944 本合格（11 skipped）、失敗は同じ 1 本だけ。対象 crate は
  celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/check-health.sh` → exit 0。
- `python3 scripts/dev/repo-health/repo_health.py --repo . --base "$CELERIS_WU_BASE" --head worktree --policy-version 1 --out <file>`
  → exit 0、verdict pass、fail_grade 0、new 0・worsened 0。
- `git diff $CELERIS_WU_BASE HEAD -- crates/task-dispatch/src/dispatcher/leases.rs` → 追加 4 行（`#[cfg(test)]` の呼び出し）だけ。

### 未解決

- 反例（回収側の古い snapshot の回収が、保持者の延ばした lease を消す）は直っていない。この unit は本体を変えないので、
  check は exit 0 にならない。修正の unit（repair）か replan に回す。

### 提案

- 回収の書き込みを比較更新にする: `RunSupervisor::release_lease` が snapshot の lease（run id と期限）を store に渡し、store が
  writer transaction の中で「Task が running で lease が期待どおり（同じ run id、期限が延びていない）」を照合してから遷移と
  `WorkerFinished` を書く。違えば何も書かずに知らせ、回収側はその Task を飛ばす。`crates/task-core/src/store/` と
  `run_supervisor.rs`（lease の高 risk の class）に当たるので、設計 brief と設計 review を通す修正の unit で行う。
  fix-verdict の節の提案（reviewer run の判定の比較更新）と同じ形の store の API にまとめられるかも、その設計で決める。

### replan v3: renew-race（2026-11-15）

計画 v3 で renew-race は「反例の試験を残し、修正の unit に渡す」になった。反例 (i)（上の「観測」）は、次の 2 つの unit が直す:

- fix-store: store に、期待する lease を同じ writer transaction の中で照合してから遷移を書く口を足し、使われなくなる
  `release_lease` を消す。
- fix-reclaim: `reclaim_expired_leases` の回収の書き込みを、snapshot の lease との照合付きの口（fix-store が足す）に替える。
  `cex_lease_renew_after_expired_snapshot_keeps_the_holder` は試験 file を変えずに合格する見込み（上の G-proto の一時変更で
  12 本すべて合格を確かめた。照合の窓の無い本当の修正では未確認）。

この run の変更（commit の件名）:

- `renew-race: keep only the invariant and the detection reason in the renew-after-snapshot test's doc comment`
  - doc comment から、今の実装の書き込みの経路を説明する 2 文（`release_lease` → `apply_transition_with_events` が期待する
    lease を取らないこと、期待する行を取る store の API が WU の行のものだけであること）を外した。fix-store・fix-reclaim の後は
    事実でなくなるため。継ぎ目が無い理由は上の「既存の継ぎ目の有無」に残っている。doc comment には不変条件と既存の試験で
    検出できない理由だけが残る。「今の実装では落ちる」の類の記述は前の版から無い。
  - 試験の手順と assert は変えていない（関数の本体を前の commit と比べて sha256 が一致）。hook（`TestHooks::after_reclaim_decision`）と
    `leases.rs` の `#[cfg(test)]` の 4 行も変えていない。
- この節と front matter の status（running）。

この unit では `bash scripts/dev/test-affected.sh` を走らせない。反例 (i) の試験が fix-reclaim まで落ちるので exit 0 にならない
（前の run の結果: exit 100、3945 本中 3944 本合格、失敗は反例 (i) の 1 本だけ）。fix-reclaim の後に走らせる。

証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）:

- `cargo nextest run -p task-dispatch --no-fail-fast --no-tests=fail -E '(test(/cex_/) | test(/ownerless/)) & not test(/cex_lease_renew_after_expired_snapshot_keeps_the_holder/)'`
  → exit 0、48 本すべて合格（old-review の反例 `cex_restart_old_reviewer_pass_after_ownerless_close_is_ignored` を含む）。
- `cargo nextest run -p task-dispatch --no-fail-fast --no-tests=fail -E 'test(/cex_lease_restart::/) & not test(/cex_lease_renew_after_expired_snapshot_keeps_the_holder/)'`
  → exit 0、11 本すべて合格。
- `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(/cex_/) | test(/ownerless/)'` → exit 100、49 本中 48 本合格。
  落ちたのは反例 (i) の 1 本だけ（上の「観測」と同じ assert、`report.reclaimed` が 1）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/check-health.sh` → exit 0（repo-health の verdict pass、fail_grade 0、worsened 0、gate の試験も合格）。
- `python3 scripts/dev/repo-health/repo_health.py --repo . --base "$CELERIS_WU_BASE" --head worktree --policy-version 1 --out <file>`
  → exit 0、verdict pass、new 0・worsened 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → `cex-race.md`・`leases.rs`・`cex_lease_restart.rs`・`hooks.rs` の 4 file だけ。
- `git diff $CELERIS_WU_BASE HEAD -- crates ':!crates/task-dispatch/src/dispatcher/tests'` → `leases.rs` の `#[cfg(test)]` の追加 4 行だけ
  （`cfg(test)` の外の差分は無い）。
