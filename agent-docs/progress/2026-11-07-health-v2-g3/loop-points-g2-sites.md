---
title: 健全性v2 G3 loop-points g2-sites — G2 の「base に検査器が無ければ当てない」3 か所を適用判定に付け替える
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 loop-points g2-sites — G2 の「base に検査器が無ければ当てない」3 か所を適用判定に付け替える

WorkUnit `g2-sites`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1、unit loop-points の子）。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D1.1・D1.3 と、[evidence の「loop-points への引き継ぎ」](evidence.md) の撤去を行った。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

結論: 付け替えは終わり、この unit の試験と `bash scripts/dev/test-affected.sh` が通る（4 回目の run）。前回まで落ちていた範囲外の試験 83 件は、試験の土台と dagbench の driver に合格の stub の同梱の検査器を設定して直した（計画の replan で範囲に `dispatcher/tests/`・`dagbench/celeris_side.rs` が入った）。`Unavailable` を合格にしない本番の規則は変えていない。

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
- 件名「G3 loop-points g2-sites: 試験の土台と dagbench の driver に合格の stub の同梱の検査器を設定する」の commit（4 回目の run）
  - `crates/task-dispatch/src/health_review/stub_checker.rs`（新規）: `configure_passing_stub_checker() -> Result<&'static BundledChecker, String>`。stub の検査器を書く関数はここだけ。`std::env::temp_dir()` の下の、stub の本文の sha256 で名前を決めた dir に `CHECKER_TREE`・`scripts/dev/repo-health/repo_health.py`・返す JSON 2 つを書き（process で 1 回。file は一時 file から rename）、`gate::configure_bundled_checker` で process に設定する。stub の `repo_health.py` は `--print-review-policy` で `source = absent` の方針を、report の mode で合格（verdict pass・finding 0・`policy.changed = false`）の report を書いて exit 0。`health_review.rs` に `pub mod stub_checker` と説明を足した。sha256 は `gate::sha256_hex` を `pub(super)` にして使う（二重に書かない）。
  - `crates/task-dispatch/src/dispatcher/tests/mod.rs`: `use_passing_stub_checker()` を足し、`dispatcher_with_test_clock`（`dispatcher`・`worktree_dispatcher` もここを通る）と `pool_dispatcher_with_requeues` の `Dispatcher::new` の前で呼ぶ。本体は `new_test_dispatcher` に分けた。同梱の検査器を設定しない土台 `dispatcher_without_bundled_checker`（process に同梱の検査器が設定済みなら panic）を足した。
  - `crates/task-dispatch/src/dispatcher/tests/review.rs`: 3 か所の `Dispatcher::new` の前で `use_passing_stub_checker()` を呼ぶ。
  - `crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs`: `case_for` は `dispatcher_without_bundled_checker` を使う（`health_enforce_g2_unavailable_is_not_pass` と `no_reviewer_tests.rs` の検査器の無い場合が、stub なしのまま `Unavailable` を確かめる）。
  - `crates/campaign/src/dagbench/celeris_side.rs`: `CelerisSide::with_backend` が `build_dispatcher`（`Dispatcher::new`）の前で `configure_passing_stub_checker()?` を呼ぶ。理由（dagbench は health を測らない。測るのは DAG の実行・待ち・障害復旧で、一時 repo の base に検査器が無いと計画が設計 review に、review が policy の承認待ちに回り、点を測れない）を module の doc と `with_backend` の doc comment に書いた。dagbench の module は `size.responsibility-growth` の閾値（8000 行）を超えているので、1 回だけ使う別名 `max_wait` を消してコード行の純増を 0 にした。
  - 試験 `crates/task-dispatch/src/health_review/stub_checker/tests.rs`（下の節）。

## 試験（守る不変条件と、既存の試験で検出できない理由）
- 足した試験（`health_enforce_g2_`、3 件）
  - `health_enforce_g2_review_policy_without_base_checker`（`policy/tests.rs`）: base に検査器の無い repo では、同梱の検査器で review 方針を読む（作業ツリーの検査器は起こさない）。同梱も無ければ `Unavailable("checker_missing")`。前は同梱を見ずに `Absent` を返していた。
  - `health_enforce_g2_bundled_checker_builds_material_and_policy_state`（`input/tests.rs`）: base に検査器の無い repo で、G2 の材料（reviewer あり）と policy の状態（reviewer なし）を同梱の検査器で作る。同じ candidate で検査器の起動は 1 回（stub の起動の log が 1 行）で、2 回目は記録を使う。同梱も無ければ材料を作らず、policy の状態を `unavailable`（理由 `検査器を当てられない: checker_missing`）で記録する。前は何も作らず何も記録しなかったので、既存の試験はこの経路を通らない。
  - `health_enforce_g2_unavailable_is_not_pass`（`policy_review/unknown_tests.rs`）: base にも同梱にも検査器の無い repo は、reviewer の有無に依らず承認の子を作って待つ（`assert_waits`。終端でない子がある間は合格しない）。前の「検査器の無い repo は記録せず合格」の退行を検出する。
- 足した試験（`health_enforce_g2_passing_stub_`、2 件。4 回目の run）
  - `health_enforce_g2_passing_stub_passes_a_repo_without_checker`: base に検査器の無い一時 repo で、stub を同梱の検査器として渡すと入口（`health_gate`）は同梱の版の証跡の Pass（判定 `Pass`）、review 方針は `Absent`。渡さなければ入口は `Unavailable(checker_missing)`、方針は `Unavailable`（stub は適用判定を変えない）。同じ本文は同じ dir に書く。既存の試験では、stub の report・方針の形が gate の読む形とずれても「設計 review に回った」「承認待ちで止まった」としか見えないので、ここで直接確かめる。
  - `health_enforce_g2_passing_stub_configures_once_per_process`: process への設定は何度呼んでも同じ値で、`gate::bundled_checker()` がそれを返す（試験の土台は 1 つの試験で `Dispatcher` を複数組むことがあり、dagbench も 1 process で複数の点を流す）。
- 消した契約と変えた試験
  - `health_review_policy_script_missing_on_base_is_absent` → `health_enforce_g2_review_policy_without_base_checker` に置き換えた（「base に無ければ Absent」の契約を廃止した。解決できない target の確認は残した）。
  - `health_review_policy_no_reviewer_no_checker_and_normal_change_pass_without_approval`・`health_review_policy_unknown_no_checker_and_normal_change_pass_without_approval`: 「検査器の無い repo は合格」の場合（`None`）を外し、名前を `…_normal_change_passes_without_approval` にした。外した場合は `health_enforce_g2_unavailable_is_not_pass` が逆の契約で確かめる。
  - `scenario_tests.rs` の比較用の repo（`with_policy = false`）は、検査器を持たない repo から、`[review]` の無い（`source = absent`）検査器を持つ repo にした。検査器の無い repo は合格にしないので、「従来の流れ」の比較には使えない。
  - `input/tests.rs` の `repo_id` は RepoId の文字列にした（G2 の材料が証跡のために repo の id を読むため）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
4 回目の run（2026-11-09）、件名「G3 loop-points g2-sites: 試験の土台と dagbench の driver に合格の stub の同梱の検査器を設定する」の commit の後で実行した。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path が `health_review`・`dispatcher/tests/`・`dagbench/celeris_side.rs`・この file に収まる）→ exit 0
- `! grep -n 'CheckerSite::Base' crates/task-dispatch/src/health_review/input.rs crates/task-dispatch/src/health_review/policy.rs` → exit 0（0 件）
- `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/health_enforce_g2_/) | test(/health_review/)'` → exit 0、`101 tests run: 101 passed`（stub の試験 2 件と、stub なしの `health_enforce_g2_unavailable_is_not_pass` を含む）
- `cargo nextest run -p task-dispatch -p celeris-campaign --no-tests=fail -E 'test(/dispatcher::tests::(target_sync|tree_branches|git_workspace|work_units|stale_priority|stage_reopen|stalled_replan|tree_replan|write_set_record|phase_effect_ab)/) | binary(/dagbench/)'` → exit 0、`276 tests run: 276 passed`
- `cargo fmt --all -- --check` → exit 0、`cargo clippy --workspace -- -D warnings` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0、`3619 tests run: 3619 passed, 11 skipped`、`repo-health: verdict=pass exit=0 … fail_grade=0 new=1 worsened=0`（new の 1 件は既存の warning `dup.contract`（`UnavailableReason` の 2 か所の定義）で fail 級でない）、`repo_health_gate: all checks passed`
  - 1 回目は exit 1: `size.responsibility-growth` が `crates/campaign/src/dagbench`（13185 行、閾値 8000）のコード行の純増 1 を worsened にした。別名 `max_wait` を消して純増 0 にし、2 回目で通った。

これまでの run の経緯
- 1〜3 回目の run: `bash scripts/dev/test-affected.sh` は exit 100（`3617 tests run: 3534 passed, 83 failed`。task-dispatch の `dispatcher/tests/` 32 件、celeris-campaign の `dagbench_*` 51 件）。原因は 1 つで、試験の process と dagbench の driver が同梱の検査器を持たず、検査器の無い一時 repo の適用判定が `Unavailable(checker_missing)` になり、計画は設計 review（`design_review_failed:2`）に、review は policy の承認待ちに回った。2 回目の run で範囲外の 2 file に stub を一時的に入れて 83 件が通ることを確かめ、`yield.plan_issue` で範囲の不足を申告した。3 回目の run は計画が変わっていなかったので、範囲の扱いを人の決定として出した。4 回目の run は replan 後の範囲（上の範囲の check）で直した。

## 未解決
- G2 の材料が作った証跡（`CheckedRun.run`）は今は捨てている。review-path が review の health の項目に同じ評価を使えば、同じ candidate で検査器を 2 回起こさずに済む（`CandidateRepo.repo_id` は文字列なので、RepoId を持たせるか、review-path が材料作りの結果を受け取る形にする）。
- 同梱の検査器は process の global（`OnceLock`）なので、同じ process の試験の間で設定が残る。nextest は試験ごとに process を分けるので今は問題ない（既存の `health_evidence_gate_bundled_checker` も同じ前提）。`cargo test` で 1 process に試験をまとめて流すと、`dispatcher_without_bundled_checker` の assert で落ちる（黙って合格はしない）。

## 提案
- 同梱の検査器を process の global ではなく値として dispatcher に渡す（`DispatchConfig` に `bundled_checker: Option<BundledChecker>` を持たせ、daemon の構築が `configure_bundled_checker` の代わりに値を渡す）。試験の土台と dagbench の driver は stub の値を config に入れるだけになり、同じ process で stub あり・なしの試験を混ぜられる。範囲は `crates/task-dispatch/src/dispatcher.rs`・daemon の構築（`crates/celeris/src/daemon*.rs`）と `health_review` の `bundled_checker()` の呼び出し 3 か所（`input.rs`・`design_gate.rs`・`worker_profile.rs`）。
