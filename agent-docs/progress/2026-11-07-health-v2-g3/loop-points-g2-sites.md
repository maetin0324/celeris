---
title: 健全性v2 G3 loop-points g2-sites — G2 の「base に検査器が無ければ当てない」3 か所を適用判定に付け替える
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: blocked
updated: 2026-11-09
---
# 健全性v2 G3 loop-points g2-sites — G2 の「base に検査器が無ければ当てない」3 か所を適用判定に付け替える

WorkUnit `g2-sites`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1、unit loop-points の子）。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D1.1・D1.3 と、[evidence の「loop-points への引き継ぎ」](evidence.md) の撤去を行った。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

結論: 付け替えは終わり、この unit の試験は通る。ただし、同梱の検査器を持たない試験の process で、検査器の無い一時 repo を使う範囲外の試験 83 件が落ちる（`Unavailable` を合格にしなくなったため）。直すには範囲外の path が要るので、作業を止めて plan_issue で replan を申告した（「提案」）。

## 変更
- 件名「G3 loop-points g2-sites: G2 の 3 か所を適用判定に付け替え、G2 の材料は経路共通の評価の report を使う」の commit
  - `crates/task-dispatch/src/health_review/gate.rs`
    - `with_applicable_checker(worktree, base, integrated_by_celeris, bundled, deadline, run) -> CheckerChoice<T>` を足した。`repo_facts` → `applicability` を呼び、`Applies` なら選んだ検査器を `run(&BaseChecker, &CheckerId)` に渡す。結果は `NotApplicable(reason)`・`Unavailable(reason)`・`Ran(Result<T, String>)`。
    - `CheckerSource` → `CheckerSite` の写しを `checker_site` 1 か所にした（同梱の版が適用判定と違えば `checker_mismatch`、渡されなければ `checker_missing`）。`health_gate` と `run_health_check` も同じ写しを使う。
    - 評価の本体を `check_with(c, checker, repo_id, repo, head, report_out, deadline) -> CheckedRun { run, report }` にした。証跡（`HealthRun`）と、評価で読んだ report を一緒に返す。`run_health_check` と G2 の材料がこれを共有する。検査器の起動（`run_report`）は 1 回。report を読めた後の判定（policy の判定など）が失敗しても report は返す。report の理由の文は先頭の語（`report_missing:` など）を付けない（G2 の理由の文は前と同じ）。
  - `crates/task-dispatch/src/health_review/policy.rs`: `load_review_policy(repo, target, bundled, timeout)` は `with_applicable_checker` を通る。`NotApplicable` は `Absent`、`Unavailable` は `ReviewPolicyState::Unavailable("checker_missing" など)`（risk は unknown で合格にしない）。module と `with_base_checker`・`CheckerSite` の doc comment を書き換えた。
  - `crates/task-dispatch/src/health_review/input.rs`
    - G2 の材料（`repo_input`）と policy の状態（`record_policy_only`・`absent_policy`）は `with_applicable_checker` を通り、report は `gate::check_with` が読んだもの（daemon の task dir の `runs/health/` に書く）を使う。
    - `NotApplicable` は理由 `repo health を当てない: <reason>` で記録し、policy の関門は無い。`Unavailable` は理由 `検査器を当てられない: <reason>` と policy の状態 `unavailable`（不明）で記録し、関門は承認を求める（合格にしない）。
    - 定数 `NO_CHECKER` を消し、`NOT_APPLICABLE`・`CHECKER_UNAVAILABLE` を足した。
    - `build_health_input(work)` は `gate::bundled_checker()` を読んで `build_health_input_with(work, bundled)` に渡す。
  - 呼び出し側 `health_review/design_gate.rs`・`health_review/worker_profile.rs` は `gate::bundled_checker()` を `load_review_policy` に渡す。
  - 試験（下の節）: `health_review/{input/tests.rs, policy/tests.rs, policy_review/no_reviewer_tests.rs, policy_review/unknown_tests.rs, scenario_tests.rs}`。
- 件名「G3 loop-points g2-sites: 進捗」の commit: この file。

## 試験（守る不変条件と、既存の試験で検出できない理由）
- 足した試験（`health_enforce_g2_`、3 件）
  - `health_enforce_g2_review_policy_without_base_checker`（`policy/tests.rs`）: base に検査器の無い repo では、同梱の検査器で review 方針を読む（作業ツリーの検査器は起こさない）。同梱も無ければ `Unavailable("checker_missing")`。前は同梱を見ずに `Absent` を返していた。
  - `health_enforce_g2_bundled_checker_builds_material_and_policy_state`（`input/tests.rs`）: base に検査器の無い repo で、G2 の材料（reviewer あり）と policy の状態（reviewer なし）を同梱の検査器で作る。同じ candidate で検査器の起動は 1 回（stub の起動の log が 1 行）で、2 回目は記録を使う。同梱も無ければ材料を作らず、policy の状態を `unavailable`（理由 `検査器を当てられない: checker_missing`）で記録する。前は何も作らず何も記録しなかったので、既存の試験はこの経路を通らない。
  - `health_enforce_g2_unavailable_is_not_pass`（`policy_review/unknown_tests.rs`）: base にも同梱にも検査器の無い repo は、reviewer の有無に依らず承認の子を作って待つ（`assert_waits`。終端でない子がある間は合格しない）。前の「検査器の無い repo は記録せず合格」の退行を検出する。
- 消した契約と変えた試験
  - `health_review_policy_script_missing_on_base_is_absent` → `health_enforce_g2_review_policy_without_base_checker` に置き換えた（「base に無ければ Absent」の契約を廃止した。解決できない target の確認は残した）。
  - `health_review_policy_no_reviewer_no_checker_and_normal_change_pass_without_approval`・`health_review_policy_unknown_no_checker_and_normal_change_pass_without_approval`: 「検査器の無い repo は合格」の場合（`None`）を外し、名前を `…_normal_change_passes_without_approval` にした。外した場合は `health_enforce_g2_unavailable_is_not_pass` が逆の契約で確かめる。
  - `scenario_tests.rs` の比較用の repo（`with_policy = false`）は、検査器を持たない repo から、`[review]` の無い（`source = absent`）検査器を持つ repo にした。検査器の無い repo は合格にしないので、「従来の流れ」の比較には使えない。
  - `input/tests.rs` の `repo_id` は RepoId の文字列にした（G2 の材料が証跡のために repo の id を読むため）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `! grep -n 'CheckerSite::Base' crates/task-dispatch/src/health_review/input.rs crates/task-dispatch/src/health_review/policy.rs` → exit 0（0 件。grep は exit 1）
- `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/health_enforce_g2_/) | test(/health_review/)'` → exit 0、`99 tests run: 99 passed`
- `cargo nextest run -p task-dispatch -E 'test(/health_review/) | test(/health_enforce/) | test(/health_evidence/) | test(/policy/)'` → 126 件すべて pass（entry の `health_enforce_entry_` 8 件、`health_evidence_gate_` を含む）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 100、`3617 tests run: 3534 passed, 83 failed, 11 skipped`。落ちたのは全部範囲外の試験。
  - task-dispatch 32 件: `dispatcher/tests/` の `target_sync`（11）・`tree_branches`（5）・`git_workspace`（4）・`work_units`・`stale_priority`・`stage_reopen`・`phase_effect_ab::review_sync`（各 2）など。
  - celeris-campaign 51 件: `crates/campaign/tests/dagbench_*`（harness 19・overhead 8・accept 5・fault 5・process 4 など）。
  - 原因は 1 つ。試験の process には同梱の検査器が無く（`configure_bundled_checker` を呼ばない）、一時 repo の base にも検査器が無い。そのため適用判定が `Unavailable(checker_missing)` になり、planner の計画は risk unknown で設計 review（`design_review_failed:2` → `awaiting_plan_approval` の Blocked）に、review は policy の状態が不明で承認の子の待ちになる。
- 原因の切り分け（2026-11-09、2 回目の run。範囲外の試験の土台に一時的に stub の同梱の検査器を入れて確かめ、確認後に戻した。branch には残していない）
  - stub: `configure_bundled_checker` に渡す dir に `CHECKER_TREE`（`repo-health <40 桁>`）と `repo_health.py`（`--print-review-policy` は `source = absent`、report の mode は合格の report を書いて exit 0）を置く。
  - `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(/dispatcher::tests::(target_sync|tree_branches|git_workspace|work_units|stale_priority|stage_reopen|phase_effect_ab)/)'`: stub なしは `126 tests run: 97 passed, 29 failed`、`dispatcher/tests/mod.rs` の `dispatcher_with_test_clock` と 2 つ目の `Dispatcher::new` の前で stub を設定すると `126 passed`。
  - `cargo nextest run -p celeris-campaign --no-fail-fast -E 'test(/dagbench/) | binary(/dagbench/)'`: `crates/campaign/src/dagbench/celeris_side.rs` の `Dispatcher::new` の前で stub を設定すると `139 tests run: 139 passed`。
  - したがって 83 件は「試験の土台・dagbench の driver が同梱の検査器を持たない」ことだけで落ちる。直す場所は範囲外の上の 2 file（と、値で渡すなら `dispatcher.rs`・daemon の構築）。
- 前回の run は `result.json` に `yield.plan_issue` を書かずに done を返したため、replan の申告が届かず、check の `test-affected.sh` が exit 100 で落ちた。2 回目の run は `yield.plan_issue` で申告し直した。

## 未解決
- 上の 83 件（task-dispatch 32・celeris-campaign 51）。直すには範囲外の path（下の「提案」）が要るので、この unit では直さない（範囲を広げない）。`crates/campaign/src/dagbench/celeris_side.rs` は試験だけでなく dagbench の計測の driver でもあるので、計測でも同梱の検査器が要る。
- G2 の材料が作った証跡（`CheckedRun.run`）は今は捨てている。review-path が review の health の項目に同じ評価を使えば、同じ candidate で検査器を 2 回起こさずに済む（`CandidateRepo.repo_id` は文字列なので、RepoId を持たせるか、review-path が材料作りの結果を受け取る形にする）。

## 提案
- replan（plan_issue）: 同梱の検査器を、process の global ではなく値として dispatcher に渡し、試験の土台も同梱の検査器を持つようにする unit を、g2-sites の前（または g2-sites の範囲の拡大）に置く。
  - 範囲に要る path: `crates/task-dispatch/src/dispatcher.rs`（`Dispatcher` か `DispatchConfig` に `bundled_checker: Option<BundledChecker>`）、daemon の構築（`crates/celeris/src/daemon*.rs`。`configure_bundled_checker` の値を渡す）、試験の土台 `crates/task-dispatch/src/dispatcher/tests/mod.rs`（`dispatcher`・`worktree_dispatcher`）と `crates/campaign/src/dagbench/celeris_side.rs`。
  - 試験の土台の同梱の検査器は、`source = absent` の方針を出し、合格（exit 0、`policy.changed = false`）の report を書く stub にする。そうすれば今の試験の意味（`[review]` の無い通常の repo）が保たれ、`Unavailable` を合格にしない本番の規則も変えずに済む。
  - review-path・integration-path も同じ問題に当たる（検査器の無い一時 repo の review・WU の check がどれも `Unavailable` → Error → Blocked になる）ので、この unit は後続の unit より前に置く。
