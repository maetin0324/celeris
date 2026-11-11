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
