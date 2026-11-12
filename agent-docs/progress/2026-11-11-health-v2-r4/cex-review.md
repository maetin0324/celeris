---
title: "健全性v2 R4 cex-review: review・統合・権限・予算の反例試験"
tasks: [01M75475JER9MAVHY9BTHCRXH9]
status: running
updated: 2026-11-11
---
# 健全性v2 R4 cex-review: review・統合・権限・予算の反例試験

規約（`cex_lease_restart.rs` と同じ）: 試験は `crates/task-dispatch/src/dispatcher/tests/cex_review_authority.rs` に置く。Dispatcher の private field に触らず、store の行（task・lease・WU・`runs` 索引・delivery・credential_wait 等）と event と `tick()` の戻り値（`TickReport`）だけを見る。待ちは `tests/mod.rs` の `run_until_state` と出来事待ちで、保険は壁時計（`STATE_WAIT_GUARD`）。tick の回数の上限で待たない。file の中に tick を回数で回して待つ helper と、thread を止める sleep を書かない。run の終わりは fake adapter の gate（semaphore・`Notify`）で試験が決め、時刻は注入した時計か store の時刻で作る。CPU 負荷は使わない。他の試験 file は変えない。各試験の直前の doc comment に「不変条件:」と「既存の試験で検出できない理由:」を書く。今の実装で成り立たない反例は本体を直さず、assert を入れずに「反例」に再現手順と観測した event を書く。

## 節: review

基準は `$CELERIS_WU_BASE`。本体（dispatcher・store）は変えていない。変えたのは `tests/cex_review_authority.rs`（新規）と `tests/mod.rs`（`mod cex_lease_restart;` の後に doc comment 付きの `mod cex_review_authority;`）とこの記録だけ。

### 作業前 brief

1. 根本原因: R4 の分割の前に、review の権限（検査した候補の同一性・自己申告で完了しないこと・再同期の上限）を `tick()` の経路で固定する試験が無い。既存の試験は `spawn_review`・`on_review_finished` を直接呼び、private field（`reviewing`・`review_resync_halted`）を見る。
2. 責務 owner: 試験は `dispatcher/tests/`。対象の判断は `review_spawn.rs`（検査後の snapshot の照合・reviewer の合格の再利用・再同期の上限）、`review_verdict.rs`（`on_review_finished` の判定の適用）、`review.rs`（`review_task` の検査と reviewer の順序）。
3. 再利用: `tests/mod.rs` の `run_until_state`・`worktree_dispatcher`・`git_task`・`init_test_repo`・`project_with_repos_integrating`・`events_of`・`done_outcome`、`target_sync.rs` の `commit_file`（`pub(super)`）。`target_sync.rs` の組織（cos・engineering）と再同期の検査の command は写した。
4. 選択肢: review を `spawn_review` で直接起こさず、`ready` の task を `tick()` で走らせて worker run → review → reviewer run に進めた。reviewer run の終わりは semaphore の gate で試験が決める。
5. 新しい状態・分岐・fallback・port: 無し（試験だけ）。
6. 撤去対象: 無し。
7. 不変条件: 下の表の各行。

### 試験（4 本、全部合格）

| 試験 | 守る不変条件 |
|---|---|
| `cex_review_late_pass_on_the_old_candidate_does_not_pass_the_rebased_candidate` | 配送の始まる root task で、reviewer run A が候補 X を検査している間に target が進み、候補が rebase で Y に変わったら、A の合格が遅れて届いても Y を合格・`MergeQueued`・Done にしない。A の run は `Cancelled` で閉じ、`ReviewTargetAdvanced`（X、新しい target）を残し、`ReviewerPassed` は X にだけ束縛される。Y では reviewer をやり直し（`ReviewerPassReused` 無し）、Y の不合格で `failed` になる（ADR-0118 D3〜D5） |
| `cex_review_worker_claim_does_not_override_a_failing_check` | worker の result.json と `Terminal::Done` が全条件の合格（command の exit 0 を含む）を申告しても、決定的な検査が実際に落ちれば Done にならない（`failed`、criterion 0 の `ReviewVerdict` は不合格） |
| `cex_review_worker_claim_is_not_done_before_or_against_the_reviewer` | worker が成功を申告し、決定的な検査も合格していても、reviewer run を止めている間は reviewing のまま（`TickReport.in_flight >= 1`、reviewer 条件の `ReviewVerdict` 無し）。reviewer が不合格なら `failed` |
| `cex_review_resync_cap_halts_and_starts_no_more_reviewer_runs` | target が動き続けると再同期は `MAX_TARGET_RESYNCS + 1` 回目で止まり、`ReviewResyncHalted` を 1 件だけ残す。止めた後に target がさらに進んでも、`tick()` は review を起こさない（`(in_flight, reviewed) == (0, 0)`、reviewer の `WorkerStarted`・`ReviewTargetSynced`・検査の実行の数が増えない、attempts 不変、reviewing のまま） |

fake の reviewer は `review.json` に criterion 0〜7 の判定を書く。配送の始まる root task には reviewer が判定する暗黙の条件（task の条件の後ろの index）が足されるため。最初の版は reviewer 条件（index 1）だけに書き、暗黙の条件が「判定なし」で不合格になって、遅れて届く A の結果が合格になっていなかった（guard の確認の 1 回目で気付いて直した）。

### guard を一時的に外すと落ちることの確認

本体の 1〜3 行を一時的に書き換えて `cargo nextest run -p task-dispatch -E 'test(/cex_review_authority::/)' --no-fail-fast` を実行した。一時変更は試験の後に `git checkout --` で戻し、commit していない（戻した後に `grep -rl GUARD-OFF crates/` は 0 件、`git status` は試験 file と `tests/mod.rs` だけ）。`crates/task-core/src/store/` と `crates/task-core/migrations/` は触っていない。build は 4 回（B: G1、C: G2・G5・G7、D: G3・G6、E: G4）。同じ build の guard は、それぞれ別の試験だけを落とす組み合わせにした（G3 と G6、G2 と G5 と G7）。

| guard（一時変更） | 落ちた試験（assert と観測） |
|---|---|
| G1 `review_spawn.rs` `spawn_review` の review task: 検査後の snapshot の照合（HEAD・branch・target と `reviewed_sha`・`target_sha`）を `false && (…)` で無効にする | `cex_review_late_pass_…`（`assert!(!merge_queued(..))`）。配送の行が `MergeQueued` になった（X の遅い合格が適用された）。`cex_review_resync_cap_…`（`status == Reviewing` が Done）。検査中の target の前進を stale と見ず合格を適用した |
| G2 `review_spawn.rs` `reviewer_reuse`: `if !overlap.is_empty()` を `if false && …` にする | `cex_review_late_pass_…`（`adapter.reviews().len() == 2` が 1）。遷移は dispatch → worker_done → review_pass で、X の合格を Y に再利用して Done になった |
| G3 `review_verdict.rs` `on_review_finished`: `let all_pass = outcome.all_pass();` を常に true にする | `cex_review_late_pass_…`（最後の `status == Failed` が Done）、`cex_review_worker_claim_does_not_override_a_failing_check`（`status == Failed` が Done）、`cex_review_worker_claim_is_not_done_before_or_against_the_reviewer`（最後の `status == Failed` が Done） |
| G4 `review.rs` `review_task`: `Check::Reviewer` の条件を reviewer を起こさずに合格にする | `cex_review_worker_claim_is_not_done_before_…`（`adapter.reviews().len() == 1` が 0。reviewer の判定なしで Done）。`cex_review_late_pass_…`（`adapter.reviews()[0]` が index 範囲外で panic。reviewer run を 1 本も起こさず Done）。`cex_review_resync_cap_…`（`adapter.reviews().len() == reviewers` が 0 対 1。reviewer の `WorkerStarted` は 1 件記録されたが adapter は呼ばれなかった） |
| G5 `review_spawn.rs` `spawn_review`: `target_resync_exhausted` が真でも `return Ok(false)` しない | `cex_review_resync_cap_…`（止めた後の tick の `(report.in_flight, report.reviewed) == (0, 0)` が `(1, 0)`）。上限の後も review を起こした |
| G6 `review_spawn.rs` `target_resync_exhausted`: 上限を `MAX_TARGET_RESYNCS + 1000` にする | `cex_review_resync_cap_…`（`run_until_state(.. halted == 1 ..)` が保険の 60 秒で false） |
| G7 `review.rs` `review_task`: `Check::Command` の結果を常に合格にする | `cex_review_worker_claim_does_not_override_a_failing_check`（`status == Failed` が Done） |

各 build の結果: B は 4 本中 2 本が失敗、C は 3 本、D は 4 本、E は 3 本。guard を戻した後は 4 本すべて合格。

### 確かめられなかった guard

- `on_review_finished` の `task.status != Status::Reviewing` による破棄（reopen・人の割り込みで reviewing を抜けた task に遅れて届いた判定）。今回の試験は target が進む変種だけで、割り込み（`Trigger::Interrupt`）の変種は書いていない。割り込みの後は次の tick が review を止め（`abort_stale_runs` が `stop_review` で handle を abort し、reviewer run を `Cancelled` で閉じる）、遅れた判定がそれでも届けば検査後の snapshot の照合（G1）が候補の変化で stale にする。guard が 3 重なので、1 つだけ外して落ちる試験は作れていない。
- `bound_reviewer_pass` の同期前 HEAD への束縛。`cex_review_late_pass_…` では同期前の HEAD が X なので束縛は満たされ、再利用を止めるのは重なりの判定（G2）だけになる。束縛を外す一時変更は実行していない（この試験の形では落ちない見込み）。
- 配送の merge 直前の候補照合（`crates/celeris` の delivery）は dispatcher の外で、この節の範囲外。

### 反例

今の実装で成り立たない反例は見つからなかった。

### 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-dispatch -E 'test(/cex_review_authority::/)' --no-fail-fast`: 4 本すべて合格、exit 0（1670 本は skip）。
- guard の確認は上の表のとおり（一時変更は commit していない）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。`sh scripts/dev/progress-index.sh --check`: exit 0。
- 参考: `cargo clippy -p task-dispatch --tests -- -D warnings` は exit 101。4 件はどれも他の試験 file（`dispatch_ready_phases.rs`・`health_enforce.rs`・`cos_chat_root_done.rs`・`cos_slot_wait.rs`）の既存の指摘で、`cex_review_authority.rs` には指摘が無い。他の試験 file は変えない規約なので直していない。
- 規約の確認: `cex_review_authority.rs` に tick を回数で回す helper の名前と thread の sleep は 0 件。`/// 不変条件:` と `/// 既存の試験で検出できない理由:` は各 4 行（試験 4 本）。
- `bash scripts/dev/test-affected.sh` は run の壁時計が足りず実行していない（`--list` の対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）。変更は task-dispatch の `#[cfg(test)]` の module と文書だけで、逆依存の crate の build には入らない。統合の check で走らせる。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path はこの記録・`cex_review_authority.rs`・`tests/mod.rs` の 3 件。

### 提案

- reopen（人の割り込み）の変種は guard が 3 重で、1 つだけ外して落ちる試験が作れていない。R4 の分割で review の中断（`abort_stale_runs`）と判定の破棄（`on_review_finished`）を同じ owner に寄せるなら、そのときに割り込みの変種の試験を足し、どちらか 1 つを外して落ちることを確かめる。
- `cargo clippy -p task-dispatch --tests -- -D warnings` の既存の 4 件は、release gate が `--tests` を見ないため残っている。別 task で直すか、検査に `--tests` を足すかを決める。

## 節: integration

基準は `$CELERIS_WU_BASE`。本体（dispatcher・store・task-ops）は変えていない。変えたのは `tests/cex_review_authority.rs`（試験 3 本と、その helper・fake adapter・統合の跡を作る fixture）とこの記録だけ。`tests/mod.rs` と他の試験 file は変えていない（`orphan_takeover.rs` の `instance`・`SELF_ID` と `target_sync.rs` の `commit_file` は `pub(super)` のものを use した）。

### 作業前 brief

1. 根本原因: R4 の分割の前に、統合の途中の再起動と、review 前の同期の衝突を `tick()` の経路で固定する試験が無い。既存の試験（`work_units.rs` の `an_interrupted_integration_is_redone_idempotently_after_restart`、`target_sync.rs` の `integration_repair_*`）は Dispatcher の private field（`integrating`・`infra_backoff`・`reviewing`）に触り、`spawn_review` を直接呼び、tick の回数の上限で待つ。
2. 責務 owner: 段の統合は `dispatcher/phase_integration.rs`（`start_integration`・`on_integration_finished`・`finish_phase_integration`）と `integration.rs`（`integrate` の merge）、再起動の照合は `dispatcher/work_units.rs` の `reconcile_parallel_tasks`。review 前の同期の衝突は `dispatcher/review_spawn.rs`（`spawn_review` の同期）・`dispatcher/review_verdict.rs`（`try_integration_repair`）・`task-ops` の `changes::sync_onto_target`。
3. 再利用: `tests/mod.rs` の `run_until_state`・`parallel_dispatcher`・`parallel_task`・`adopt_v2_plan`・`v2_wu`・`ParallelWuAdapter`・`worktree_dispatcher`・`init_test_repo`・`git_out`・`git_ok`、`orphan_takeover.rs` の `instance`・`SELF_ID`、`target_sync.rs` の `commit_file`、この file の `git_review_task`・`deliverable`。
4. 選択肢: 再起動の止め所は private な hook（`before_integration_start`）を使わず、統合の検査の command が試験の置く file を待つ形にした（統合の作業ツリーで最初に走ったときだけ止まる）。落ちた daemon の dispatcher は drop し、作り直す dispatcher は自分の instance の行を登録して孤児の回収を有効にした（lease を失効させず、`infra_backoff` に触らずに済む）。統合 WU の done と Task の遷移の間で落ちた状態は、store に統合 WU の遷移を直接書いて作った（`cex_lease_restart.rs` の再起動の試験と同じ）。衝突は配送の始まる root task で、worker run が作業ツリーと target の同じ行を変えて作り、repair の run は semaphore で止めた。
5. 新しい状態・分岐・fallback・port: 無し（試験だけ）。
6. 撤去対象: 無し。
7. 不変条件: 下の表の各行。

### 試験（3 本、全部合格）

| 試験 | 守る不変条件 |
|---|---|
| `cex_integration_restart_mid_phase_integration_does_not_merge_twice` | 段の統合が WU `a`・`b` のブランチを Task のブランチに merge し終え、統合の検査の途中で daemon が落ちた（統合 WU は running、`PhaseIntegrated` 無し）後に、同じ store・同じ Git repo で作り直した dispatcher（新しい active）が統合をやり直しても、Task のブランチの merge commit は WU のブランチごとに 1 つのまま。統合 WU は `orphan_takeover` で pending に戻り、やり直しは済んだ merge を飛ばし（`PhaseIntegrated.merged` の `skipped` が両方 true）、`PhaseIntegrated`・統合 WU の done・Task の reviewing・done・最終レビューの判定は各 1 回 |
| `cex_integration_restart_after_the_integration_unit_is_done_does_not_integrate_again` | `finish_phase_integration` が統合 WU を done にした後、Task の遷移の前で落ちた状態から作り直しても、統合をやり直さない（統合 WU の遷移は running → done の 2 件のまま、`IntegrationCheckStarted` は 1 件、merge commit は WU ごとに 1 つ）。Task は Ready を経て最終レビューに 1 回だけ進み Done になる |
| `cex_integration_sync_conflict_schedules_one_repair_and_never_passes_the_conflict` | 配送の始まる root task の review 前の同期（target への rebase）が衝突したら、rebase を戻し（作業ツリーは worker の commit のまま clean、`REBASE_HEAD` 無し、target は動かず merge commit 無し）、IntegrationRepair の repair WU を 1 つだけ作る（`IntegrationRepairScheduled` 1 件、attempt 1、before は worker の commit）。repair の run を止めている間は reviewer run・`ReviewVerdict`・`ReviewTargetSynced` が無く、Done・`MergeQueued` にならず、attempts は不変で、tick を重ねても 2 つ目の repair を作らない。repair が衝突を解いた後は `IntegrationRepairResolved` 1 件、reviewer run 1 本で合格し、配送の行の merge candidate は target を含む HEAD |

### guard を一時的に外すと落ちることの確認

本体の 1〜2 行を一時的に書き換えて `cargo nextest run -p task-dispatch -E 'test(/cex_review_authority::/)' --no-fail-fast` を実行した。一時変更は試験の後に `git checkout --` で戻し、commit していない（戻した後に `grep -rl GUARD-OFF crates/` は 0 件、`git status` は試験 file だけ）。`crates/task-core/src/store/` と `crates/task-core/migrations/` は触っていない。build は 4 回（B: GI1・GC1、C: GI2・GC2、D: GC3、E: GI3）。同じ build の guard は、それぞれ別の試験だけを落とす組み合わせにした。B〜D の時点では `cex_integration_restart_after_…` はまだ無く、E で足した後に確かめた。

| guard（一時変更） | 落ちた試験（assert と観測） |
|---|---|
| GI1 `integration.rs` `integrate`: Task の HEAD の祖先になった WU のブランチを飛ばす `is_ancestor` を `false && …` にする | `cex_integration_restart_mid_…`（`phase_integrated(..) == [[("a", true), ("b", true)]]` が `[[("a", false), ("b", false)]]`）。merge commit の数の assert は合格した（git の `merge --no-ff` は取り込み済みの commit に新しい commit を作らない）。二重の merge commit は git 自身も防ぐので、この guard は `skipped` の記録で見る |
| GC1 `review_spawn.rs` `spawn_review`: `try_integration_repair` が repair を作った後の `return Ok(true)` を `continue` にする | `cex_integration_sync_conflict_…`（`reviewer_starts(..) == 0` が 1）。repair の run の間に、同期していない HEAD で reviewer run を起こした |
| GI2 `work_units.rs` `reconcile_parallel_tasks`: 持ち主の居ない running の統合 WU を pending に戻す条件を `false && …` にする | `cex_integration_restart_mid_…`（再起動後の `run_until_state(.. terminal ..)` が保険の 60 秒で false）。統合 WU が running のまま残り、Task が進まない |
| GC2 `review_verdict.rs` `try_integration_repair`: 上限の判定を常に「上限に達した」にする | `cex_integration_sync_conflict_…`（`repairs.len() == 1` が 0）。repair を作らず、同期していない HEAD の review に回った |
| GC3 `task-ops` `changes.rs`: `rebase_onto` で `rebase --abort` をせず、`sync_onto_target` の「元の HEAD・clean に戻った」照合を外す | `cex_integration_sync_conflict_…`（`repairs_scheduled(..)` の before が worker の commit ではなく target）。作業ツリーが rebase の途中のまま残り、その HEAD を before として記録した |
| GI3 `work_units.rs` `reconcile_parallel_tasks`: 何も走っていない running の v2 の Task を `Continue{advance}` で Ready に戻す処理を飛ばす | `cex_integration_restart_mid_…` と `cex_integration_restart_after_…`（どちらも再起動後の `run_until_state(.. terminal ..)` が保険の 60 秒で false） |

各 build の結果: B は 6 本中 2 本が失敗、C は 6 本中 2 本、D は 6 本中 1 本、E は 7 本中 2 本。どの build でも `cex_review_` の 4 本は合格した。guard を戻した後は 7 本すべて合格。

### 確かめられなかった guard

- `on_integration_finished` の古い結果の破棄（`task.status != Running || integ.status != Running`）。試験の「再起動」は落ちた daemon の dispatcher を drop するので、旧い統合の完了は channel ごと捨てられ、作り直した dispatcher に届かない。旧い daemon の完了が届く順序（draining の引き継ぎ）は cex-lease の範囲で、ここでは作っていない。
- `start_integration` の状態の判定（pending・ready 以外は始めない）と `try_start_work_unit_integration` の比較更新。どちらの再起動の試験でも、dispatch の gate が done の統合 WU に統合を始めさせないので、この判定まで届かない。
- 段の統合の merge の衝突（`schedule_merge_repair` の repair WU `merge-<phase>-<key>`）。衝突の試験は review 前の同期（ADR-0120 の IntegrationRepair）だけで書いた。
- 配送の Merging の途中の再起動（`crates/celeris` の delivery）。dispatcher の外で、書いてよい範囲（`crates/task-dispatch/src/dispatcher/tests/`）の外。

### 反例

統合 WU を done にした後、Task の遷移の前で落ちると、`PhaseIntegrated` が 1 件も記録されない。

- 再現手順: `cex_integration_restart_after_the_integration_unit_is_done_does_not_integrate_again` と同じ。段 `build` の WU `a`・`b` の統合が merge を 2 件済ませ、統合の検査の途中で daemon が落ちた跡を作る。store に統合 WU の running → done（reason `integrated`、`integrated_commit` は Task のブランチの HEAD）を直接書く（`finish_phase_integration` の最初の書き込みだけが残った状態）。同じ store で新しい active の dispatcher を作り、Task が終端になるまで tick する。
- 観測した event（Task の分、再起動の後）: `transitioned` running → ready（`advance`）、ready → reviewing（`plan_complete`）、`delivery_skipped`（`no_project`）、reviewing → done（`review_pass`）、`review_verdict`（criterion 0 合格）。統合 WU の遷移は pending → running（`integrate`）と running → done（`integrated`）の 2 件だけ。`integration_check_started` は落ちる前の 1 件で、`integration_check_finished` と `phase_integrated` は 0 件。Task のブランチの merge commit は 2 つ（WU ごとに 1 つ）。
- 影響: merge は二重にならず Task は 1 回で Done になるが、統合の完了の記録（取り込んだ WU と commit、統合後の HEAD、検査の結果）が event に残らない。途中確認のある段（`PausePointsResolved` の段）なら `PhaseReported` も出ず、停止点で止まらない見込み（未確認）。
- 原因の見込み: `finish_phase_integration` は統合 WU の done（`work_unit_transition`）と、Task の遷移・`PhaseIntegrated`（`apply_transition_with_events`）を別の書き込みにしている。再起動後の `reconcile_parallel_tasks` は Task を Ready に戻し、全 WU が done なので dispatch の gate が最終レビューに進む。
- 試験ではこの点を assert していない（本体は直していない）。

### 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-dispatch -E 'test(/cex_review_authority::/)' --no-fail-fast`: 7 本すべて合格（`cex_integration_` 3 本、`cex_review_` 4 本）、exit 0。
- guard の確認は上の表のとおり（一時変更は commit していない）。
- 規約の確認: `cex_review_authority.rs` に tick を回数で回す helper の名前と thread の sleep は 0 件。`/// 不変条件:` と `/// 既存の試験で検出できない理由:` は各 7 行（試験 7 本）。

### 提案

- `finish_phase_integration` の統合 WU の done と、Task の遷移・`PhaseIntegrated` を 1 つの store の書き込みにする（または、再起動の照合が `integrated_commit` を持つ done の統合 WU に `PhaseIntegrated` が無ければ記録し直す）。R4 で統合の完了の owner を分けるときに、上の反例の assert（`PhaseIntegrated` が 1 件）を足す。
