---
title: "健全性v2 R4 cex-late2: 段の repair・replan の後に届く旧い統合・旧い review の合格の反例試験と guard の確認"
tasks: [01M7DYMCVS1PMDK2N0WW3ME9T8]
status: running
updated: 2026-11-14
---
# 健全性v2 R4 cex-late2

`cex-late.md` の「残り」1〜3 を終える task。WU ごとに節を分ける。この節は WU `late-integ`（段の repair の後に届く旧い統合の
完了）の結果。基準は `$CELERIS_WU_BASE`。本体（`cfg(test)` の外）は変えていない。

## late-integ: 段の repair の後に届く旧い統合の完了

### 試験

| 試験 | 守る不変条件（要約） | 結果 |
|---|---|---|
| `cex_late_integration_after_stage_repair_is_dropped` | 段 `build` の統合の検査が走っている間に段の repair（planner の replan が足す `kind = repair` の unit `repair-build-1`。`RepairScheduled{origin: planner}`）が先に確定したら、旧い統合（repair の前の HEAD の候補）の完了は捨てる。repair の done の前は `PhaseIntegrated` を付けず、統合 WU を done にせず、Task を Reviewing・Done に進めない。`PhaseIntegrated` は repair の done の後に、repair の commit（`r.txt`）を含む HEAD にだけ付く。Task は repair を含む候補で Done | 合格（4 本の file 全体も合格） |

順序の作り方（`crates/task-dispatch/src/dispatcher/tests/cex_late_completion.rs`）:

1. `adopt_blocking_plan` で段 `build`（WU `a`・`b`）を採用する。`a` の検査は統合の作業ツリーで最初に走ったときだけ、
   試験が `<marks>/open` を置くまで止まる。
2. `run_until_state` で `<marks>/first` と `IntegrationCheckStarted` を待つ（統合 WU は running）。
3. `task_ops::execution::replan`（`PlanOrigin::Planner`）で段 `build` に `kind = repair` の unit を足す。
   `RepairScheduled{key: repair-build-1, origin: planner}` が記録されたことを assert する。
4. `<marks>/open` を置いて旧い統合の完了を届ける。repair の run は fake adapter の gate（`holding`）で止める。
5. `run_until_state` で repair の run の開始を待つ。統合の process が残る間は Task の WU を起こさない（`dispatch_parallel_work_units`
   と D1.7 の Ready 戻しが `is_integrating` を見る）ので、repair の run の開始は旧い統合の完了の処理の後。ここで「repair の
   done の前」の assert をする。
6. gate を開け（`notify_one`。permit は待ちの前に出しても残る）、Task が終端になるまで待ち、event の順と HEAD を assert する。

既存の試験で検出できない理由: replan の変種（`cex_late_review_and_integration_after_replan_are_dropped`）は葉の WU を足し、
`PhaseIntegrated.merged` の key で新旧の統合を見分けられる。repair の unit は Task の worktree で走り merge の item に
ならないので、key では旧い統合と新しい統合を見分けられない。この試験は HEAD に repair の commit があるかで見分ける。

### guard の一時変更と結果

| 一時変更（本体） | 結果 |
|---|---|
| `phase_integration.rs` の `on_integration_finished` の古い結果の破棄 `if task.status != Status::Running \|\| integ.status != task_core::WorkUnitStatus::Running` から `\|\| integ.status != …Running` を外す | `cex_late_integration_after_stage_repair_is_dropped` が落ちた: 「旧い統合の完了が repair の done の前に PhaseIntegrated を付けた: repair_done=None integrated=[["a", "b"]] integ_done=Some(32) reviewing=None done=None status=Running」。同じ一時変更で既存の `cex_late_review_and_integration_after_replan_are_dropped` も落ちた（integrated=[(33, ["a", "b"])]、c_done=Some(39)）。他の 2 本は合格 |

一時変更は `git checkout -- crates/task-dispatch/src/dispatcher/phase_integration.rs` で戻した（commit していない）。戻した後、
4 本すべて合格。

### 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-dispatch -E 'test(/cex_late_completion::/)'`: 4 本すべて合格、exit 0。
- 上の一時変更の下で同じコマンド（`--no-fail-fast`）: 4 本中 2 本が失敗（上の表）、`error: test run failed`。戻した後は 4 本合格。
- `cargo nextest run -p task-dispatch -E 'test(=dispatcher::tests::cex_late_completion::cex_late_integration_after_stage_repair_is_dropped)'` を 3 回: 3 回とも合格（各 約 1.1 秒）。
- `bash scripts/dev/test-affected.sh`（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）: exit 0、3944 本合格・11 本 skip（nextest の Summary）、doc test も合格。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0（差分無し）。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: `crates/task-dispatch/src/dispatcher/tests/cex_late_completion.rs` と、この file だけ。

### 変更（commit の件名）

- `late-integ: 段の repair の後に届く旧い統合の完了の反例試験を足し、guard を外すと落ちることを確かめる`

### 未解決事項

- 無し（反例は今の実装で成り立った）。

## late-review: replan・repair の後に届く旧い最終 review の合格

結論（計画 v2 の後）: 反例は今の実装で成り立った（下の観測）。計画 v2 で late-review が本体の修正も受け持ち、
[ADR 2026-11-14-review-verdict-after-plan-reopen](../../adr/2026-11-14-review-verdict-after-plan-reopen.md) を足してから
`on_review_finished` を直した。試験は変えていない。提出時の HEAD で `cex_late_completion.rs` の 6 本すべてが合格する
（下の「修正（計画 v2）」）。この節の「修正（計画 v2）」より前の記述は、計画 v1（試験だけ）の run の記録として残す。

### 試験

| 試験 | 守る不変条件（要約） | 結果 |
|---|---|---|
| `cex_late_review_pass_after_replan_is_dropped` | 最終 review の reviewer run が候補（`a`・`b` だけ）を検査している間に人の replan（段 `build` に WU `c`）が確定したら、旧い合格は Task を Done にせず、`MergeQueued` にせず、`ReviewerPassed`・`ReviewerPassReused` で `c` を含む候補に付かない。Done にできるのは `c` の done の後の reviewer run の、`c.txt` を含む候補の合格だけで、Done の時点で有効な計画の unit はすべて done | **失敗**（下の観測） |
| `cex_late_review_pass_after_repair_is_dropped` | 同じ順序で、planner の replan が段の repair（`kind = repair` の `repair-build-1`、`RepairScheduled{origin: planner}`）を確定する。旧い合格は Task を Done にしない（以下同じ。新しい候補は `r.txt` を含む） | **失敗**（下の観測） |

既存の試験で検出できない理由（doc comment の要約）: 既存の replan・repair の変種（`cex_late_review_and_integration_after_replan_are_dropped`・
`cex_late_integration_after_stage_repair_is_dropped`）は、統合の検査が走っている間（Task が Reviewing に入る前）に計画を変える。
`cex_review_late_pass_on_the_old_candidate_does_not_pass_the_rebased_candidate` は reviewer を止めている間に target を進め、
候補の commit が変わる順序で、計画は変えない。`stage_reopen.rs` は統合済みの段に unit を足す replan を、Task が Running の間に確定する。

### 再現手順（`crates/task-dispatch/src/dispatcher/tests/cex_late_completion.rs` の `late_review_pass_after`）

1. 案件の repo を持つ自動取り込みの root task（`git_review_task`・`deliverable`。条件 0 `test -f a.txt && test -f b.txt`、
   条件 1 reviewer）に、`adopt_v2_plan` で段 `build`（WU `a`・`b`）を採用する。
2. adapter `ReviewGateWuAdapter`: WU の run は `ParallelWuAdapter` に任せる。reviewer run は Task のブランチの commit を記録してから
   semaphore の gate で止まり、開いたら全条件の合格を `write_review_json`（`cex_review_authority.rs` と共有）で書く。
3. `run_until_state` で reviewer run の開始を待つ。Task は Reviewing、`integrate-build` は done、候補に新しい unit の file は無い。
4. `LateChange::apply` で `task_ops::execution::replan` を確定する（replan 版: `PlanOrigin::Human` で WU `c`。repair 版:
   `PlanOrigin::Planner` で `kind = repair` の `repair-build-1`、`RepairScheduled{origin: planner}` を assert）。
5. gate を 1 本だけ開け（`add_permits(1)`）、旧い reviewer run の `WorkerFinished{role: reviewer}` を `run_until_state` で待つ。
6. assert: Task は Done でない・`review_pass` が無い・`MergeQueued` でない・旧い合格は変更の前の候補にだけ付く。
7. gate を閉じて以後の reviewer run を止めず（`Semaphore::close`）、Task の終端まで進めて、新しい候補の review だけが Done にしたことを assert する。

### 観測（提出時の HEAD、2 本とも手順 6 の最初の assert で落ちる）

```
旧い reviewer の合格が計画の変更の後の Task を Done にした: unit_done=None status=Done
transitions=[(Running, "dispatch"), (Reviewing, "worker_done"), (Done, "review_pass")]
old_end=[Some(Completed)] reviewer_passed=[(<旧い reviewer run>, <変更の前の候補>)] delivery=None
delivery_skipped=[(NoMarker, ".../<task>/worktree.json が読めない")]
units=[("a", Done), ("b", Done), ("c", Ready), ("integrate-build", Pending)]
```

repair 版は `("repair-build-1", Ready)` で、ほかは同じ。

- replan（`task_ops::execution::plan_replan`）は Task の状態を変えない。統合済みの段に unit が増えると `integrate-build` を
  `pending` に戻し（`STAGE_REOPENED_REASON`）、新しい unit を `ready` にする。Task は Reviewing のまま残る。
- 旧い reviewer の合格は `on_review_finished`（`review_verdict.rs`）でそのまま適用され、Task は `review_pass` で Done に進む。
  有効な計画には未完の unit（`c` か `repair-build-1` が ready、`integrate-build` が pending）が残る。
- 配送の行は作られなかった（`DeliverySkipped{NoMarker}`）。fixture は Task の worker run を経ない（`adopt_v2_plan`）ので
  `worktree.json` が無い。本番では planner run が目印を書くので、同じ順序で `MergeQueued` まで進むかもしれないが、試していない（未確認）。

### guard の一時変更と結果

今の HEAD で 2 本とも落ちるので、「guard を外すと落ちる」ことは確かめられない。代わりに、この順序を止めそうな guard 2 つを
外して、落ち方が変わらないこと（どちらの guard もこの順序では効いていないこと）を確かめた。

| 一時変更（本体、同時に 2 つ） | 結果 |
|---|---|
| `review_verdict.rs` の `on_review_finished` の判定の破棄 `if task.status != Status::Reviewing {`（「review result discarded」）を `if false && …` にする。`review_spawn.rs` の検査後の snapshot の照合（`sync_guards` の HEAD・branch・target の比較）の 3 項をすべて `false && …` にする | 新しい 2 本は同じ観測で落ちた（`status=Done`、`c`/`repair-build-1` は Ready）。既存の 4 本は合格（`cex_late_completion::` の 6 本中 4 本合格・2 本失敗） |

一時変更は `git checkout -- crates/task-dispatch/src/dispatcher/review_verdict.rs crates/task-dispatch/src/dispatcher/review_spawn.rs`
で戻した（commit していない）。戻した後、`git status` は試験 file 2 つだけを変更として出す。

### 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-dispatch -E 'test(/cex_late_completion::/)' --no-fail-fast`: 6 本中 4 本合格、2 本（新しい 2 本）失敗、exit 100。
- `bash scripts/dev/test-affected.sh`（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）: exit 100、
  3946 本中 3944 本合格・2 本失敗（新しい 2 本だけ）・11 本 skip。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0（差分無し）。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: `cex_late_completion.rs`・`cex_review_authority.rs` と、この file だけ。

### 試験の helper の共有

- `cex_review_authority.rs`: reviewer の `review.json` の組み立てを `write_review_json` に切り出し（`ReviewGateAdapter` もこれを
  使う。書く内容は同じ）、`git_review_task`・`deliverable`・`transitions`・`reached_done`・`reviewer_finishes`・`merge_queued` を
  `pub(super)` にした。
- `cex_late_completion.rs`: 段 `build` に unit を足す replan を `LateChange::apply` にまとめ、late-integ の 2 か所
  （`cex_late_integration_after_stage_repair_is_dropped`・`late_integration_after_human_replan`）もこれを使う（replan の理由と origin は同じ）。

### 変更（commit の件名）

- `late-review: replan・repair の後に届く旧い最終 review の合格の反例試験を足す（今の実装で落ちる）`
- `late-review: 再実行で反例の再現を確かめ、本体の修正が要るため plan_issue を申告したことを進捗に書く`
- `late-review: 一時修正で試験の後半の合格を確かめ、plan_issue が届かなかった理由を進捗に書く`

### 未解決事項

（計画 v1 の時点の記録。最初の 1 件と後の 2 件は、計画 v2 の「修正（計画 v2）」と「後半の確認」で解決した。）

- 反例が成り立つ（上の観測）。人の `PUT /tasks/{id}/execution-plan`（`task-api` の `put_execution_plan`）と CoS の `put_plan_audited` も
  同じ `task_ops::execution::replan` を Task の状態を見ずに呼ぶ。そのため、本番でも最終 review 中の replan は、未完の unit を残したまま
  Task を Done にしうる（本番での再現は試していない。未確認）。
- 新しい 2 本の後半（新しい unit の done の後の review だけが Done にする）の assert は、今の HEAD では前半で落ちるため実行されない。
  3 回目の run で本体に一時修正を当てて後半を実行し、合格を確かめた（下の「後半の確認」）。
- この fixture では配送の行ができない（`NoMarker`）。`MergeQueued` の assert は今は行が無いので効いていない。
- この unit の再実行（check の不合格の後）で同じ check を再実行した: 6 本中 4 本合格・新しい 2 本が同じ観測（`status=Done`、`c`／`repair-build-1` が Ready、`integrate-build` が Pending）で失敗、exit 100。原因は試験ではなく本体にある。`review_verdict.rs` と `review_spawn.rs` には有効な計画の版と照合する処理が無い（`execution_plan_active` を読むのは最終 review の主題の組み立てと repair の追加だけ）。`task-api` の `put_execution_plan` も Task の状態を見ずに `replan` を呼ぶ。この unit は本体を変えられないので、check は通せない。celeris に `plan_issue` を申告し、本体を直す leaf（上の提案 (a) か (b)）を計画に足すよう求めた。
- 2 回目の run の `plan_issue` の申告は届かなかった。その run の `result.json` は `yield` と `summary` を両方持っていた。
  `task-worker` の `interpret_result_text`（`result_report.rs`）は `summary` を `yield` より先に見るので、run は done として扱われた。
  celeris は check を走らせ、WU を retry（2/2）に戻した。3 回目の run（この run）は `summary` を書かず `{"yield": {"plan_issue": …}}`
  だけを書く。

### 後半の確認（3 回目の run、一時変更は commit していない）

試験の後半が本体の修正の受け入れの check として働くかを確かめるため、本体に次の一時修正を当てた。

| 一時変更（本体） | 結果 |
|---|---|
| `review_verdict.rs` の `on_review_finished` で、`task.status != Status::Reviewing` の破棄の直後に「`work_units_for(task_id)` に done でない active な unit があれば、判定を捨て（`WorkerFinished.end = Cancelled`、run index も Cancelled）、`Trigger::Interrupt` で Task を Reviewing → Ready に戻して return」を足す | `cex_late_completion::` の 6 本すべて合格（`Summary 6 tests run: 6 passed`、exit 0）。新しい 2 本は後半（新しい unit の done の後の reviewer run が `c.txt`／`r.txt` を含む候補で Done にする、Done の時点で全 unit が done）まで通った。既存の 4 本も合格 |

一時変更は `git checkout -- crates/task-dispatch/src/dispatcher/review_verdict.rs` で戻した。戻した後の同じ check は
6 本中 4 本合格・新しい 2 本が手順 6 の最初の assert で失敗（exit 100）。

この一時修正は試験の確認のためのもので、修正の案ではない（`Trigger::Interrupt` は人のコメントの遷移で、reason が `comment` になる）。
本体を直す leaf では、上の提案 (a)（計画の版の束縛）か (b)（replan で Task を Running に戻す）を ADR に書いてから実装する。

### 提案

（計画 v1 の時点の提案。計画 v2 では案 (a)・(b) のどちらでもなく、SQLite の unit から破棄を導く形にした。理由は ADR の D4。）

- 根本の owner は「Reviewing の Task の計画が変わったこと」を判定に入れていない review の権限の側。案は 2 つ。
  - (a) 推奨: `spawn_review` が review の subject に有効な計画の id（版）を束縛し、`on_review_finished` が判定の適用の前に
    `execution_plan_active` の id と比べる。違えば stale と同じく判定を捨て（`WorkerFinished.end = Cancelled`）、Task を Running に戻して
    新しい版の unit を走らせる。target の stale の判定（ADR-0118 D4 付記）と同じ場所・同じ扱いになる。
  - (b) `plan_replan` が Reviewing の Task の段を開き直したとき、同じ transaction で Task を Running に戻す
    （進行中の review は既存の `task.status != Reviewing` の破棄で捨てられる）。replan の経路が 3 つ（API・CoS・planner）あるので
    store の transaction に置く必要がある。
- どちらも本体（`cfg(test)` の外）の変更で、ADR 2026-11-11-dispatcher-state-ownership D7 の「試験だけ」の範囲の外。別の repair か
  replan の leaf に回す。

### 修正（計画 v2）

ADR: [2026-11-14-review-verdict-after-plan-reopen](../../adr/2026-11-14-review-verdict-after-plan-reopen.md)（status: accepted）。
ADR 2026-11-11-dispatcher-state-ownership D7（成り立たない反例は owner で直す）による修正で、前例は
ADR 2026-11-14-dependency-promotion-every-plan-version。

- `crates/task-dispatch/src/dispatcher/review_verdict.rs` の `on_review_finished`: Task が Reviewing のとき、`ReviewerPassed`・
  `RequiredChecksEvaluated`・判定の適用より前に、`store.work_units_for` を既存の `task_core::next_work_unit` で見る。
  `RunWorkUnit`（ready か needs_continuation の unit が残る）なら判定を合格・不合格とも捨てる。判定を適用しない 3 つの場合
  （Task が Reviewing でない・計画が開き直された・`target_stale`）を 1 つの分岐にまとめ、`WorkerFinished.end` と run index を
  `Cancelled` にする処理を共有した（以前は前の 2 つの場合で同じ 11 行を 2 回書いていた）。計画が開き直されたときは、続けて Task を
  `Trigger::ReviewPlanReopened` で Reviewing → Ready に戻す。
- `crates/task-core/src/transition.rs`: `Trigger::ReviewPlanReopened`（`reason = "review_plan_reopened"`、Reviewing → Ready、
  attempts 不変）。遷移は `ReviewRepair` と同じ腕を共有する。`retry_policy::attempt_history` と `plan_awaits_final_review` の表には
  入れない（判定ではない）。
- 試験: `crates/task-core/src/transition/tests.rs` の直積に `ReviewPlanReopened` を足し（4 × 8 × 36）、
  `review_plan_reopened_keeps_attempts_and_returns_to_ready`（attempts 3・`max_retries` 3 でも failed にならない。直積は
  attempts 0 だけを見るので `retry_or_fail` と取り違えても検出できない）を足した。`crates/task-core/src/retry_policy/tests.rs`
  の `history_is_derived_from_events` に、`review_plan_reopened` の遷移が試行に数えられないことを足した。

条件を `plan_work_finished`（done でない active な unit が 1 つでも残る）から `next_work_unit` の `RunWorkUnit` に絞った理由:
最初の版では `dispatcher::tests::target_sync::integration_repair_fallback_*` の 3 本が落ちた（Task が Done でなく Blocked）。
ADR-0120 の integration repair の打ち切り（fallback）では、repair の WU が `blocked(plan_issue)` のまま Task が同期していない
HEAD で最終 review に進む。この review の判定は捨ててはいけない。blocked・failed だけが残る計画は `Stuck` で、条件に当たらない。

repo-health の `size.responsibility-growth` への配慮: `crates/task-dispatch/src/dispatcher` の module の本体の行（base で
閾値 8000 を超える既存の負債）と `Dispatcher::on_review_finished`・`transition::transition` の関数の行は、増えると worsened で
落ちる。共有の関数を足す最初の版は module を 32 行・`fn transition` を 1 行増やした。3 つの場合を 1 つの分岐にまとめる形に
変え、`python3 scripts/dev/repo-health/repo_health.py --repo . --base "$CELERIS_WU_BASE" --head HEAD --profile celeris
--policy-version 1 --rules size.responsibility-growth,source-size` が pass（worsened 0）になることを確かめた。

### guard の一時変更と結果（計画 v2）

| 一時変更（本体） | 結果 |
|---|---|
| `review_verdict.rs` の `let plan_reopened = task.status == Status::Reviewing` を `let plan_reopened = false && task.status == Status::Reviewing` にする（足した破棄を外す） | `cex_late_review_pass_after_replan_is_dropped`・`cex_late_review_pass_after_repair_is_dropped` が前の観測と同じ「旧い reviewer の合格が計画の変更の後の Task を Done にした: unit_done=None status=Done transitions=[(Running, "dispatch"), (Reviewing, "worker_done"), (Done, "review_pass")] …」で落ちた。既存の 4 本は合格（6 本中 4 本合格・2 本失敗、exit 100） |

一時変更は `git checkout -- crates/task-dispatch/src/dispatcher/review_verdict.rs` で戻した（commit していない）。共有の関数の版でも
同じ一時変更で同じ 2 本が落ちることを確かめている。

### 変更（commit の件名、計画 v2）

- `late-review: 最終 review の間に計画が開き直されたら旧い判定を捨て、Task を ReviewPlanReopened で ready に戻す`
- `late-review: 判定を適用しない 3 つの場合（reviewing でない・計画が開き直された・stale）を 1 つの破棄にまとめ、module の行を増やさない`
- `late-review: 破棄の条件を走らせる unit の有無に絞り（integration repair の fallback を捨てない）、ADR と進捗に修正と guard の確認を書く`

### 証拠（計画 v2、提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）


- WU の check 0（ADR・試験 2 本・進捗の ADR の参照の grep）: exit 0。
- `cargo nextest run -p task-dispatch -E 'test(/cex_late_completion::/)'`: 6 本すべて合格、exit 0。
- `cargo nextest run -p task-dispatch -p task-core`: 3264 本すべて合格、exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh`: exit 0（`check-adr-numbers: ok (446 files)`）。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` を許す path で絞る）: 範囲外の path は無し。変更 path は ADR・この file・
  `crates/task-core/src/{transition.rs,transition/tests.rs,retry_policy/tests.rs}`・`crates/task-dispatch/src/dispatcher/review_verdict.rs`・
  `crates/task-dispatch/src/dispatcher/tests/{cex_late_completion.rs,cex_review_authority.rs}`。
- `python3 scripts/dev/repo-health/repo_health.py --repo . --base "$CELERIS_WU_BASE" --head HEAD --profile celeris --policy-version 1`:
  verdict=pass、worsened 0・new 0。`sh scripts/dev/progress-index.sh --check`: exit 0。
- `bash scripts/dev/test-affected.sh`（task-core の変更で逆依存の crate・web の型検査と unit・ts-verify も対象）: exit 0。nextest: `Summary [ 566.464s] 8517 tests run: 8517 passed (9 slow), 23 skipped`。
- commit（追加）: `late-review: 計画 v2 の修正の証拠を進捗に書く`、`late-review: test-affected の結果を進捗に書く`
