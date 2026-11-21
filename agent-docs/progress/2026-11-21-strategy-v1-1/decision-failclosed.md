---
title: 戦略層 v1 子 1 decision-failclosed — 承認要求の needed_before の取りこぼしの再現と、壊れた要求の fail-closed
tasks: [01M7XSGW4FH258N1K3AMW76Y6F]
status: done
updated: 2026-11-21
completed: 2026-11-21
---
# decision-failclosed — 承認要求の取りこぼしの再現と、壊れた要求の fail-closed

完了日: 2026-11-21

## 結論

- (a) アーキv4 E の子 task 01M7Q5ZABN9NFQCH6QBTTP3EZ8 で、worker の決定の要求 `work-units-size-trim`
  （`needed_before: ["integrate-impl"]`）は進行の 1 行（seq 436）だけを残して捨てられ、決定の行 0 行・受信箱の項目 0 件のまま
  task は done（seq 568）に達した。`reproduce.py verify` が fixture でこの流れと修正前の原因の file:line 12 か所を確かめ、exit 0。
- (b) 11-18 の修正の owner の 2 hunk（`decision.rs`・`tree_units.rs`）を作業ツリーで戻すと `rejected_worker_decision_*` の 2 件が落ち
  （exit=100）、`git checkout -- crates/` で戻すと 2 件とも通った（exit=0）。作業ツリーは元に戻した。
- (c) survey の 14 経路（cluster job の wait を分けて 15 行）のうち 5 経路を「直した」（decisions-not-array・continuable-end・
  failed-end・question-and-decisions・atomic の節点）。cluster job の wait は「未解決」。残りの 9 経路は fail-closed（2）か問題無し（7）。
  直した経路の `decision_fail_closed_*` 14 件は、どれも修正前に assertion で落ちた（4 つの before.log、どれも exit=100）。
- (d) 11-18 の修正を含む最初の昇格（promoted_at 2026-11-19T01:25:29Z）の後、進行の行 0 行・key rejected の決定 0 件・対応しない行 0。
  この task の修正（fix・settle-leaf・settle-failure・settle-atomic）は main にも release にもまだ入っていない。昇格の後に人が走らせる
  再監査の手順を「本番への移行」に書いた。
- この unit（close）は記録だけで、実装・試験・script は変えていない。

## (a) 取りこぼしの events と修正前の原因（詳細は [repro.md](decision-failclosed/repro.md)）

fixture `decision-failclosed/fixtures/e-target-leaf-events.json` は本番 DB を `mode=ro` で読み、events seq 435〜568 の 134 件
（type・seq・ts・状態の欄・run_id だけ）、文脈の seq 419・422、task の decisions の全行、件数を持つ。人の名前・秘密は入れていない。

| seq | ts (UTC) | type | 内容 |
|---|---|---|---|
| 422 | 2026-11-18T14:20:43 | worker_progress | worker の最後の文「so I need your OK on that before integrate-impl.」 |
| 435 | 2026-11-18T14:46:27 | work_unit_transitioned | `target-leaf` running → done（completed） |
| 436 | 2026-11-18T14:46:28 | worker_progress | `worker の決定の要求を記録できませんでした（ADR-0079 D7）: decision work-units-size-trim: needed_before names an unknown unit "integrate-impl" (use self, a unit key of the plan or stage:<key>)` |
| 437 | 2026-11-18T14:46:28 | worker_finished | 報告に「人の確認を decision work-units-size-trim で求めた（integrate-impl の前まで）」 |
| 440 | 2026-11-18T14:46:28 | work_unit_transitioned | `integrate-impl` pending → running（integrate） |
| 462 | 2026-11-18T15:35:01 | work_unit_transitioned | `integrate-impl` running → done（integrated） |
| 562 | 2026-11-18T16:46:12 | transitioned | task running → reviewing（worker_done） |
| 568 | 2026-11-18T17:10:51 | transitioned | task reviewing → done（review_pass） |

- seq 436 の後に作られた decisions の行は 0 行、decision の event は 0 件、この task を指す受信箱の項目は 0 件。task の decisions は
  全部で 1 行（別の正しく受け付けられた要求 `record-run-write-sets-home`、12:47:51Z に作られ回答済み）。

修正前の commit は、commit「adr: 受け付けられない worker の決定の要求を 1 件の決定にまとめて人に出す」（ADR
2026-11-18-rejected-worker-decisions-reach-human を足した commit）の first parent。その tree の file:line:

- `crates/task-dispatch/src/dispatcher/tree_units.rs:390` — `.filter(|u| u.kind != task_core::WorkUnitKind::Integrate)`。
  `needed_before` に書ける unit から Integrate を除くので、`integrate-impl` は unknown unit になる（これ自体は正しい）。
- `crates/task-core/src/decision.rs:628` — `bad = Some(format!("unknown unit {n:?}"));`、`:638`〜`:639` — 拒否の理由の文字列を
  `out.rejected.push(...)` で積むだけ。`:568`〜`:569` — `rejected` の doc「捨てた要素と理由（人に見える進行の 1 行にする）。」
- `crates/task-dispatch/src/dispatcher/tree_units.rs:419`〜`:422` — `for reason in &batch.rejected {` → `Event::worker_progress(` →
  `format!("worker の決定の要求を記録できませんでした（ADR-0079 D7）: {reason}")`。`rejected` の使い道はこの 1 行だけ。
- `crates/task-dispatch/src/dispatcher/tree_units.rs:434`〜`:438` — `if batch.accepted.is_empty() {` のとき `held_rows: Vec::new()`・
  `self_hold: false` で返す。決定の行も止める unit も作らず、run は成功のまま進む。

## (b) 修正の hunk を戻すと落ち、戻すと通る（詳細は [revert.md](decision-failclosed/revert.md)）

- 戻した hunk（`decision-failclosed/revert/revert.patch`、1 行追加・12 行削除）: `task_core::decision::prepare_worker_decisions` の
  `rejected_decision` で決定を組み立てる block（`escalated` が常に `None` になる）と、`Dispatcher::worker_decisions` の記録する列
  `batch.recorded()` を `batch.accepted.iter()` に戻す変更。
- 戻した状態（`revert/reverted.log`）: `2 tests run: 0 passed, 2 failed`、`exit=100`。
  - `rejected_worker_decision_reaches_the_inbox_and_holds_the_leaf` は `tree_decisions.rs:815` の `assert!(blocked_by_decision(&units, "a"))`
    で落ちた。leaf `a` も `integrate-s1` も `Done` になり、本番と同じく黙って完了した。
  - `rejected_worker_decisions_become_one_self_decision` は `decision/tests.rs:369` の `expect("the rejected ones reach a human")` で落ちた。
- 戻した後（`revert/restored.log`）: `2 tests run: 2 passed`、`exit=0`。2 件とも `PASS` の行がある。
- 作業ツリー: `git checkout -- crates/` の後、`git diff -- crates/` と `git status --short --untracked-files=all -- crates/` は 0 行。

## (c) 経路ごとの判定（詳細は [survey.md](decision-failclosed/survey.md) と各 unit の md）

survey.md の全行（14 行）に、調査の後に直した結果を入れた。「直した」の経路の修正は unit fix・settle-leaf・settle-atomic、
試験の追加は unit settle-failure も行った。

| id | 経路 | 判定 | 直した unit・理由 |
|---|---|---|---|
| decisions-not-array | `result.json` の `decisions` が object・文字列・数・真偽値 | 直した | fix。`worker_decisions_from_json` が欄の値を返し、配列でなければ `prepare_worker_decisions` が拒否 1 件にして key `rejected` の決定で止める（[fix.md](decision-failclosed/fix.md)） |
| continuable-end | leaf・atomic の run が yield・予算切れ・limit・continuation_cap・auto-leaf の上限・plan_issue で終わり `decisions` を書いた | 直した | settle-leaf（WU の run）・settle-atomic（atomic の節点）。D1 の規則で読み、self の要求なら決定で止める（[settle-leaf.md](decision-failclosed/settle-leaf.md)・[settle-atomic.md](decision-failclosed/settle-atomic.md)） |
| failed-end | check の不合格・worker の失敗・replan の使い切り・integration repair の失敗で終わった run の `decisions` | 直した | settle-leaf（決定を失敗の処理より先にする）・settle-atomic（失敗を `Continue{advance}` にする）。settle-failure が dispatcher の経路全体で確かめた（[settle-failure.md](decision-failclosed/settle-failure.md)） |
| question-and-decisions | `question` と `decisions` の併記 | 直した | survey の時点では fail-closed（質問が最優先で止まる）と判定したが、`decisions` の中身は人に届かなかった。settle-leaf・settle-atomic で決定が先に止め、質問の文は outcome の末尾（`。止めた質問: …`）に残す。settle-failure が試験した |
| atomic-run | WU の無い run（木の子の atomic の節点）の `decisions` | 直した | 配列の誤りは survey の時点で fail-closed（`self` のまま止める）。配列でない欄は fix、done 以外の終わり方は settle-atomic が直した |
| cluster-job-wait | WU の cluster job の wait（遷移 `cluster_jobs`）と atomic の `Waiting` で終わった run の `decisions` | 未解決 | 読まない。待ちの正本は WU の行の `blocked(cluster_jobs)` と wait の行で、job の終了による再開（`cluster_job_wait.rs`）はその行からだけ動く。`blocked(decision)` に置き換えると待ちの正本が 2 つになり、job が終わっても再開できない。直すには `cluster_job_wait.rs`・`task_ops::decision` の変更が要り、この段の範囲の外（ADR 2026-11-21-worker-decisions-fail-closed の付記） |
| planner-plan | planner の計画の `decisions` の形・key の重複・`needed_before` の誤り | fail-closed | 計画の検証の誤りで計画は採用されず、2 回とも不正なら `plan_invalid`（`needed_before: [self]`）の決定で止める |
| result-json-broken | `result.json` が JSON として壊れている・`summary` などの型が違う | fail-closed | run は `error(retryable=true)` で失敗し、完了しない |
| harness-error | run が harness の誤りか取り消しで終わった | 問題無し | WU は `ready` に戻り run をやり直す。次の run の `result.json` が要求を出し直す |
| non-tree-node | 木の節点でない task の run | 問題無し | 前置きに決定の要求の書き方の節が出ないので、`decisions` は契約の外の欄 |
| tree-disabled | `[execution.limits.tree] enabled = false` | 問題無し | 同上。既存の試験 `tree_disabled_ignores_worker_decisions` が確かめる |
| planner-result-decisions | planner の run の `result.json` の `decisions` | 問題無し | planner の run には worker の書き方の節を出さない。planner は計画の `decisions` に書く |
| empty-or-null | `decisions` が無い・`null`・空の配列 | 問題無し | 要求が 0 件。修正の後も `None` のまま（ADR の I3） |
| reread-same-question | 答えた後の run が同じ問いの要求をもう一度書いた | 問題無し | 人が答えた（または開いている）問いなので出し直さない（11-18 の ADR D3） |
| needed-before-started-unit | `needed_before` が走っている・終わった unit を指した | 問題無し（人に届く） | 決定は記録され受信箱に出る。指した unit が止まらない点は「提案」 |

survey.md は 14 行で、表の continuable-end の行の「cluster job の wait」を cluster-job-wait の行に分けた（計 15 行）。

読む規則と止め方の順: 木の節点の worker の run の `decisions` は、状態だけを戻す run（harness_error・取り消し）と cluster job の
wait（WU の遷移 `cluster_jobs`・atomic の `Waiting`）を除き、終わり方によらず読む（atomic は終わり方が分からない `None` の run も
読まない）。読み取りは `worker_finish.rs` の `finish_worker_result_with` で `settle_work_unit` の直後に置き、`needed_before: self` の
要求があれば、integration repair の打ち切りと WU の失敗の処理（再試行・自動 replan・質問・replan の使い切りの `limit:max_replans`）
より先に `hold_for_worker_decisions` が leaf を `blocked(decision)`・task を `Continue{advance}` にして、`failure`・
`integration_repair_exhaust`・`auto_replan` を空にする。その後の失敗の処理は何もしないので、1 つの leaf を止めるのは決定 1 か所に
なる。atomic の節点では `hold_task_for_worker_decisions` が `WorkerDone`・`WorkerQuestion`・`WorkerError` を `Continue{advance}` に
替え、`Continue{continue}`・`Continue{children_wait}` は残し、次の run は `dispatch_admission` の `decision_self_hold` が答えまで止める。
遷移の数え（continuation・再試行・試行）は止めた run で動かさず、記録済みの数えは下げない。

直した経路の `decision_fail_closed_*` が修正前に落ちた出力:

| log | command の対象 | 結果 | 落ちた試験 |
|---|---|---|---|
| [fix/before.log](decision-failclosed/fix/before.log) | decisions-not-array・atomic-run | `3 tests run: 0 passed, 3 failed`、`exit=100` | `decision_fail_closed_non_array_decisions_become_one_rejected_decision`（task-core）・`..._hold_the_leaf`・`..._hold_the_atomic_node` |
| [settle-leaf/before.log](decision-failclosed/settle-leaf/before.log) | continuable-end・failed-end・question-and-decisions（WU の run） | `4 tests run: 0 passed, 4 failed`、`exit=100` | `decision_fail_closed_reads_every_end_but_reset_and_cluster_jobs`・`decision_fail_closed_hold_clears_failure_side_effects`・`decision_fail_closed_yielded_leaf_records_decisions`・`decision_fail_closed_no_progress_leaf_records_decisions` |
| [settle-failure/before.log](decision-failclosed/settle-failure/before.log) | failed-end・question-and-decisions・plan_issue（dispatcher の経路全体。settle-leaf の production を一時的に戻した） | `4 tests run: 0 passed, 4 failed`、`exit=100` | `decision_fail_closed_check_failed_leaf_records_decisions`・`decision_fail_closed_plan_issue_leaf_records_decisions_before_replan`・`decision_fail_closed_replan_exhausted_leaf_records_one_decision`・`decision_fail_closed_question_leaf_records_decisions` |
| [settle-atomic/before.log](decision-failclosed/settle-atomic/before.log) | atomic の節点の continuable-end・failed-end | `3 tests run: 0 passed, 3 failed`、`exit=100` | `decision_fail_closed_task_reads_every_end_but_reset_and_waiting`・`decision_fail_closed_yielded_atomic_node_records_decisions`・`decision_fail_closed_failed_atomic_node_records_decisions` |

- 4 つの before.log とも、コンパイル error は無く assertion で落ちた。対応する after.log は全部 `exit=0`（3・4・4・3 passed）。
- `decision_fail_closed_auto_leaf_request_recorded_with_and_without_worker_decisions`（`tree_gate.rs`）は修正の前も後も通る守りの試験で、
  before.log には入れていない。

## (d) 本番の再監査（詳細は [repro.md](decision-failclosed/repro.md) と `decision-failclosed/audit.json`）

- 11-18 の修正（commit「worker の決定: 受け付けられない要求を 1 件の決定にまとめて人に出し、その仕事を止める」）を含む最初の昇格は
  promoted_at 2026-11-19T01:25:29Z（release の directory は保持数で消えていたので promote の log で補った）。
- 数え方: その時刻以後の events のうち、type `worker_progress` で msg が `worker の決定の要求を記録できませんでした` で始まる行と、
  その時刻以後に作られた decisions のうち key が `rejected`／`rejected-N`、`raised_by.origin` が worker の行を、run id で対応させる。
- audit.json（2026-11-21T01:10Z 時点の DB）と、提出前に `extract.py audit` を `mode=ro` で走らせ直した結果（events の最新
  2026-11-21T06:04Z）は同じだった。

| 欄 | 値 |
|---|---|
| since | 2026-11-19T01:25:29Z |
| progress_lines | 0 |
| rejected_decisions | 0 |
| matched | 0 |
| task_ids | なし |
| unmatched | なし |

- 文脈: 同じ時刻以後の origin worker の決定は 0 件、`worker_finished` は 190 件（06:04Z 時点）。修正前の進行の行は DB 全体で 14 行・
  11 task（2026-10-01〜2026-11-18）で、全部が key rejected の決定を伴わない。修正後の経路は本番でまだ一度も使われていない。
- この task の修正（下の「変更」の fix・settle-leaf・settle-atomic の commit）は main の祖先でなく、今の release（promoted_at
  2026-11-21T01:12:56Z）にも入っていない。

## 本番への移行

config・DB の移行は無い。この task の修正を含む release が昇格した後に、人が次の再監査を repo の root で走らせる（どれも読むだけ）。

1. 修正の commit を main で引く（件名で引く。rebase で hash が変わるため）:
   ```sh
   FIX=$(git log --format=%H -F --grep='decision-failclosed settle-atomic: atomic の節点の run の decisions を D1 の規則で読み、self の要求なら決定で止める' main)
   ```
   fix の commit「decision-failclosed fix: 配列でない decisions を受け付けなかった要求 1 件にし、要求を出した仕事を止める」と
   settle-leaf の commit「decision-failclosed settle-leaf: WU の run の decisions を D1 の規則で読み、self の要求なら失敗の処理より先に
   決定で止める」も同じ release に入ることを `git merge-base --is-ancestor` で確かめる。
2. 最初の昇格の時刻 `SINCE` を決める: `/local/celeris/state/releases/*/promoted.json` の release のうち
   `git merge-base --is-ancestor "$FIX" <release の directory 名>` が 0 で、`from` の release が修正を含まないものの promoted_at。
   directory が消えていれば `/local/celeris/state/logs/promote-*.log` の `promoted.json: … (mode=live from=…)` の行で補う
   （`extract.py` の `promotions()` と同じ決め方）。
3. 数える（`SINCE` は 2. の値）:
   ```sh
   DB='file:/local/celeris/data/db/celeris.sqlite3?mode=ro'
   sqlite3 "$DB" "select task_id, json_extract(json,'\$.run_id') from events where json_extract(json,'\$.type')='worker_progress' and ts>='$SINCE' and json_extract(json,'\$.msg') like 'worker の決定の要求を記録できませんでした%';"
   sqlite3 "$DB" "select task_id, key, json_extract(json,'\$.raised_by.run_id') from decisions where created_at>='$SINCE' and json_extract(json,'\$.raised_by.origin')='worker' and (key='rejected' or key like 'rejected-%');"
   ```
   1 つ目（進行の行）と 2 つ目（key rejected の決定）を run id で対応させ、件数・task id・対応しない行を書く。進行の行に対応する
   決定が無い run があれば取りこぼしで、その task id を不具合として起票する。配列でない `decisions` の拒否も進行の行と決定の両方を
   残すので、決定だけの行も対応しない行として調べる。
4. 11-18 の修正の昇格以後の全体は、今までどおり `python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/decision-failclosed/extract.py audit`
   で数えられる（`since` は 2026-11-19T01:25:29Z に固定。3. の期間を含む）。
5. fixture の再現は DB を読まない: `python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/decision-failclosed/reproduce.py verify` が exit 0。

## 変更（commit の件名。unit ごと）

- repro: 「decision-failclosed repro: E の取りこぼしの fixture・reproduce.py・本番の再監査を置く」
- revert: 「strategy-v1 1 revert: 修正の hunk を戻すと rejected_worker_decision_* が落ち、戻すと通る出力を記録する」、
  「strategy-v1 1 revert: 2 つの log を NEXTEST_STATUS_LEVEL=pass で取り直し、restored.log に通った試験の名前を出す」
- survey: 「decision-failclosed survey: 決定の要求を取りこぼす経路の表と ADR を書く」
- fix: 「decision-failclosed fix: decision_fail_closed_* の試験を先に書き、修正前に落ちる before.log を取る」、
  「decision-failclosed fix: 配列でない decisions を受け付けなかった要求 1 件にし、要求を出した仕事を止める」、
  「decision-failclosed fix: fix.md に変えた関数・試験・before/after と残る 2 経路の replan の要求を書く」、
  「decision-failclosed fix: fix.md を done にし、残る経路を同じ段の settle-leaf・settle-failure・settle-atomic に渡す」
- settle-leaf: 「decision-failclosed settle-leaf: decision_fail_closed_* の試験を先に書き、修正前に落ちる before.log を取る」、
  「decision-failclosed settle-leaf: WU の run の decisions を D1 の規則で読み、self の要求なら失敗の処理より先に決定で止める」、
  「decision-failclosed settle-leaf: ADR に付記 2026-11-21（読む規則と止め方の順）を足す」、
  「decision-failclosed settle-leaf: 決定の読み取りを Dispatcher::work_unit_decisions に移し、finish_worker_result_with を大きくしない」、
  「decision-failclosed settle-leaf: 決定の読み取りを finish_worker_result_with の中に戻し、行数を base 以下にする」、
  「decision-failclosed settle-leaf: settle-leaf.md に変えた関数・試験・before/after・LoC を書く」
- settle-failure: 「decision-failclosed settle-failure: check の不合格・plan_issue・replan の使い切り・質問で終わる leaf の run の
  decision_fail_closed_* の試験 4 件を足す」、「decision-failclosed settle-failure: settle-failure.md に試験・before/after・一時的な戻し・LoC を書く」
- settle-atomic: 「decision-failclosed settle-atomic: decision_fail_closed_* の試験を先に書き、修正前に落ちる before.log を取る」、
  「decision-failclosed settle-atomic: 修正前の before.log を置く」、
  「decision-failclosed settle-atomic: atomic の節点の run の decisions を D1 の規則で読み、self の要求なら決定で止める」、
  「decision-failclosed settle-atomic: ADR に付記 2026-11-21（atomic の規則の実装）と after.log を足す」、
  「decision-failclosed settle-atomic: settle-atomic.md に変えた関数・試験・before/after・LoC を書く」
- close: 「decision-failclosed close: decision-failclosed.md に (a)〜(d)・経路ごとの判定・証拠・LoC・本番への移行を書く」
- 設計判断: [ADR 2026-11-21-worker-decisions-fail-closed](../../adr/2026-11-21-worker-decisions-fail-closed.md)（D1〜D5 と付記 2 件）

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

この unit は `crates/` を変えていない。cargo の検査は、この md を commit する前の tree（`crates/` は提出時の HEAD と同じ）で走らせた。

- `python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/decision-failclosed/reproduce.py verify` → exit 0
  （seq 436 の進行の行、seq 568 の done、修正前の file:line 12 か所が一致）
- `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/rejected_worker_decision/) | test(/decision_fail_closed/)'`
  → exit 0、`17 tests run: 17 passed, 3290 skipped`（`rejected_worker_decision*` 2 件、`decision_fail_closed_*` 15 件）
- `CELERIS_WU_BASE=$(git merge-base HEAD main) bash scripts/dev/test-affected.sh`（この task の全変更の crate と逆依存の crate。
  14 crate・web・ts-verify）→ exit 0、875 秒。repo-health: verdict=pass（fail_grade=0、new=0、worsened=0）、web の vitest 101 files・
  705 passed、test-ts-verify 20 passed、nextest `8533 tests run: 8533 passed, 22 skipped`、doc test 通過、script bundle の SKIP 0 件。
  末尾に scratch の古い trash dir を消せない `rm: Permission denied` が 6 行出たが、exit code は 0。
- `bash scripts/dev/test-affected.sh`（WU の基準 `$CELERIS_WU_BASE`）→ exit 0、152 秒。この unit の変更は agent-docs だけで `試験対象の変更なし（docs のみ等）`、repo-health: verdict=pass（fail_grade=0、new=0、worsened=0）、script の試験 1・126 件 OK、repo_health_gate: all checks passed
- `cargo clippy --workspace -- -D warnings` → exit 0（外部の crate `deno_core` の deprecated の warning 1 件だけ）
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`、`sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、
  `sh scripts/dev/check-adr-numbers.sh` → `check-adr-numbers: ok (470 files)`（どれも exit 0。この md を書いた後に実行）
- 本番 DB は `extract.py audit`（`sqlite3.connect(…mode=ro, uri=True)`）と `sqlite3 'file:…?mode=ro'` だけで読んだ。書き込みはしていない。

## LoC 差分（`git diff --numstat $(git merge-base HEAD main) -- crates/`、追加/削除）

| 区分 | file | 追加 | 削除 |
|---|---|---|---|
| production | `crates/task-core/src/decision.rs` | 50 | 14 |
| production | `crates/task-dispatch/src/dispatcher/tree_units.rs` | 6 | 5 |
| production | `crates/task-dispatch/src/dispatcher/worker_finish.rs` | 34 | 39 |
| production | `crates/task-dispatch/src/run_settlement/task_run.rs` | 40 | 12 |
| production | `crates/task-dispatch/src/run_settlement/work_unit.rs` | 49 | 14 |
| test | `crates/task-core/src/decision/tests.rs` | 105 | 12 |
| test | `crates/task-dispatch/src/dispatcher/tests/tree_decisions.rs` | 919 | 9 |
| test | `crates/task-dispatch/src/dispatcher/tests/tree_gate.rs` | 88 | 6 |
| test | `crates/task-dispatch/src/run_settlement/task_run/tests.rs` | 89 | 22 |
| test | `crates/task-dispatch/src/run_settlement/work_unit/tests.rs` | 285 | 41 |

- production: 5 file、+179／−84（純増 95 行。説明の comment を含む）
- test（`tests.rs`・`tests/` の下）: 5 file、+1486／−90（純増 1396 行）
- production の 4 file にある `#[cfg(test)]` は `mod tests;` の宣言と試験の hook の既存の行で、差分の行は production に数えた。
- この task は行数の純減を目的にしていない。

## 未確認の事項

- cluster job の wait の run（WU の `cluster_jobs`・atomic の `Waiting`）の `decisions` は読まないままで、人の答えの前に leaf が
  job の終了で進みうる。要求は job の後の run の `result.json` が出し直す前提で、出し直さなければ人に届かない。
- 答えの後の run は continuation にならず最初から走る（WU の run。atomic の continuation を止めた run だけは checkpoint の続きになる）。
  止めた run の途中の作業は前置きの答えと作業ツリーにしか残らない。
- 質問を決定で止めた run の質問の文は、run の outcome の末尾（`。止めた質問: …`）にだけ残る。`QuestionRaised` と承認の行は作らない。
- 止めた run の check の不合格は WU の retries に足されない（遷移が `decision` で、replay は数えを動かさない）。
- atomic の終わり方が分からない run（`None`。infra 再試行を使い切った run など）が `decisions` を書いていた場合は読まない。
- 修正後の経路は本番でまだ一度も使われていない（(d)）。本番で効くことは試験でしか確かめていない。

## 提案

- cluster job の wait の run の `decisions` を読む別 task を起こす。owner は `cluster_job_wait.rs`（job の終了による再開）と
  `task_ops::decision`（答えの解放）で、待ちの理由を 2 つ持てるようにするか、job の終了と答えの両方を待つ再開の条件にするかを
  設計 brief で決める。
- `needed-before-started-unit`: 指した unit がもう走っている・終わっているとき、決定は人に届くが unit は止まらない。止められない
  理由を決定の問いに書き足すかを別の task で決める（survey の提案）。
- release の directory を消すときも promoted.json（または昇格の 1 行）を残し、再監査が promote の log に頼らずに済むようにする（repro の提案）。
