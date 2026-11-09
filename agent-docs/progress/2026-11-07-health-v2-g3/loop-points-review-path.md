---
title: 健全性v2 G3 loop-points review-path — atomic/direct の review に health の項目を足し、Fail を review の repair へ
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: running
updated: 2026-11-09
---
# 健全性v2 G3 loop-points review-path — atomic/direct の review に health の項目を足し、Fail を review の repair へ

WorkUnit `review-path`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1、unit loop-points の子）。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D1.2（atomic/direct の review の経路）・D2.4・D3.1〜D3.4 を `review/required.rs` の `evaluate` に配線する。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

## 現在地（1 回目の run の終わり、yield）
- 実装は入った。既存の試験 14 件が落ち、うち件数の 2 件は直した。残る 12 件（G2 の試験の土台）は次の run が直してから検査を回す（下の「未解決」）。

## 変更（件名「G3 loop-points review-path: review の必須検査に health の項目（途中。既存の試験 14 件が未対応）」の commit）
- `crates/task-dispatch/src/health_review/review_item.rs`（新規）: review の health の項目。
  - `RequiredHealth { dir, remote, bundled, prior }`: `spawn_review` が repo ごとに作る評価の場所（同梱の検査器は tick の中で `gate::bundled_checker()` を読んで渡す）。
  - `evaluate(health, binding)`: 候補（`RequiredCheckBinding`）があれば base=`target_sha`・head=`reviewed_sha` で経路共通の入口 `gate::health_gate` を `spawn_blocking` で 1 回呼ぶ。同じ束縛（repo・base・head）の G2 の評価（`prior`）があればそれを使い、検査器を起こさない。候補の無い repo は `gate::applicable` の NotApplicable の理由（`not_a_git_repo`・`remote_workspace`）か `NOT_SYNCED` を `note` に残し、項目を足さない。
  - `item(outcome)`: Pass は合格（`health: pass`）、PolicyOnly は合格（`HEALTH_POLICY_ONLY`。人の承認は G2 の `policy_review_gate`）、Fail は不合格で reason は `HealthRepair.reason` を `; ` で連ね、`repair_hint = ReviewRepairHint { scope: HEALTH_CHECK_ID, class: "health:<rule_id>"（複数の rule は `,` で連ねる） }`。Unavailable・Error は `health_check_error: <理由>` の不合格で repair の材料を持たない。項目は `PhaseCheckResult { cmd: HEALTH_CHECK_ID, health: Some(証跡) }`。
  - `adopt(checks, runs)`: G2 の材料が評価した `HealthRun` を束縛の同じ repo の `prior` に入れる。
  - `implicit_check(verdict)`: acceptance の外の verdict のうち health の Fail（hint の scope が `HEALTH_CHECK_ID`）は `Check::Command { cmd: HEALTH_CHECK_ID, expect_exit: 0 }`、ほかは従来どおり `Check::Human`。
- `crates/task-dispatch/src/review/required.rs`: `RequiredRepoChecks.health`、`RequiredRepoResult.{health_repair_hint, health_note}` と `repair_hint(result)` を足した。`evaluate` は定義が読めた repo で health を評価し、項目を cmd の列の後に足し、指紋を `definition_digest(cmds, health.checker)`（Applies のときだけ検査器）にした。
- `crates/task-dispatch/src/review.rs`: 必須検査の verdict に `repo.repair_hint(result)` を付ける。`mod tests` を `pub(crate)` にした（試験の task の土台 `task_with` を health の試験から使う）。
- `crates/task-dispatch/src/dispatcher/review_spawn.rs`: `RequiredRepoChecks.health` を作り（候補があれば同期した worktree、remote workspace は `remote`）、`health.attach(reviewer_run, &mut repo_checks)` で G2 の評価を渡す。
- `crates/task-dispatch/src/dispatcher/review_verdict.rs`: `try_review_repair` の暗黙の条件を `review_item::implicit_check` にした（1 行）。
- `crates/task-dispatch/src/health_review/input.rs`: `HealthInputWork.evaluated`（`Mutex<Vec<HealthRun>>`）に `run_report` の `gate::check_with` の評価を残し、`attach` が `review_item::adopt` で review の項目に渡す。
- `crates/task-dispatch/src/health_review/gate.rs`: `applicable` を `pub(crate)` にした（候補の無い repo の NotApplicable の判定に同じ関数を使う）。
- 試験の土台: `HealthInputWork`・`RequiredRepoChecks`・`RequiredRepoResult` の literal に新しい欄を足した（`dispatcher/tests/health_review_input.rs`・`health_review/input/tests.rs`・`health_review/policy_review/{tests,unknown_tests}.rs`・`review/tests.rs`）。

## 試験（守る不変条件と、既存の試験で検出できない理由）
`health_review/review_item_tests.rs`（`gate_tests` の子 module。同じ stub の検査器を base に commit する）。既存の `health_enforce_entry_*` は入口の分類だけを見て、review の必須検査の列・定義指紋・repair の材料を通らないので、配線の抜けを検出できない。
- `health_enforce_direct_review_pass_records_item_and_checker_digest`: 候補のある review は health の項目（証跡つき）を足し、指紋に適用判定の検査器が入る。
- `health_enforce_direct_review_policy_only_goes_to_policy_gate`: PolicyOnly は合格で repair の材料を持たない。
- `health_enforce_direct_review_fail_routes_to_repair`: Fail は不合格で、hint（scope `HEALTH_CHECK_ID`・class `health:deps.violation`）を持ち、`implicit_check` が `Check::Command { cmd: HEALTH_CHECK_ID }` を返す。
- `health_enforce_direct_review_error_is_not_pass`: Unavailable（`checker_missing`）と exit 2 は `health_check_error:` の不合格で、`Check::Human`（修復しない）。Unavailable の指紋に検査器を入れない。
- `health_enforce_direct_review_non_git_repo_adds_no_item`: Git 管理でない作業場所と remote workspace は項目を足さず、理由を `health_note` に残す。
- `health_enforce_direct_review_uses_the_g2_evaluation_of_the_same_candidate`: 同じ候補の G2 の評価を使い、検査器を起こさない。
- `health_enforce_integration_repair_review_reevaluates_the_new_head`: 統合の修復の後の新しい head では、修復の前の head の評価（束縛が違う）を使わず評価し直す。

## 証拠（1 回目の run の作業ツリーで実行）
- `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/health_enforce_(direct_review|integration_repair_review)_/)'` → 7 passed
- `cargo nextest run -p task-dispatch` → 1585 件中 1571 passed、14 failed（下の「未解決」）。その後 `git_workspace` の 2 件を直し、その 2 件は passed
- `cargo clippy -p task-dispatch -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 範囲の検査（`sh "$CELERIS_WU_SCOPE_PATHS"` と unit の範囲の正規表現）→ 範囲外の path なし
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` → 未実行（次の run）

## 未解決
- 既存の試験 12 件が落ちる（`cargo nextest run -p task-dispatch`）。
  - 直した 2 件: `dispatcher::tests::git_workspace::{the_repository_check_commands_are_the_reviewers_default, review_records_each_repositories_own_definition_and_worktree_results}` は必須検査の結果が 1 件増えた（`health: pass`）ので、期待を health の項目込みにし、指紋に検査器が入ることも確かめるようにした（件名「G3 loop-points review-path: 必須検査の件数の試験を health の項目込みにする」の commit）。
  - G2 の土台の偽の検査器: `dispatcher/tests/health_review_input.rs` の `health_repo_with` は `health/engine.py` を commit せず、report は fail 級の finding を持つ（exit 1）。review の項目は `health_check_error: policy_review_failed`（engine が無い）になり task が Failed になる。`health_review_input::*` 8 件、`health_review_flow::*` 2 件、`health_review_scenario::*` 2 件（`planner_runs` が 3。review の不合格で replan した）。G3 では fail 級の report は review を止めるので、これらの試験の期待（Done）と土台（fail の report）を G3 の規則に合わせて直す必要がある。直し方の案: 土台に `health/engine.py` を足し、report を「G2 の材料に出す finding（warning・existing など fail 級でないもの）」と「fail 級」に分け、Done を期待する試験は fail 級でない report にする。
  - 検査器の起動の回数: G2 の材料が記録済みの材料を再利用した回（reviewer の再試行。`find_recorded`）は `check_with` を呼ばないので、review の項目が入口で検査器を起こす。`health_review_input_reused_on_reviewer_retry` などの「report の実行は 1 回」の期待と食い違う。束縛の同じ health の証跡の再利用は reuse-sync（`reusable_health`）の範囲なので、次の run で reuse-sync の進み具合を見て、試験の期待をどちらに合わせるか決める。
- `classify_review_failure` は `Check::Command { cmd: HEALTH_CHECK_ID }` を分類しない（`classify_command` が `None` を返し Substantive）。`RepairClass` に health の bucket（`health:<rule_id>`）を足すのは `crates/task-core/src/execution.rs` で、この unit の範囲の外（範囲に入っているのは integration-path）。この unit は FailedCheck の形（cmd・reason・hint）までを作る。task-core の分類が無いと review の Fail は repair にならず ReviewFail（不合格）になる。
- D3.4 の「task を Blocked にし受信箱に 1 件」: review の経路には reviewing → blocked の Trigger が無い（`ChildFailed` だけ）。今は Error を不合格（`health_check_error: <理由>`、修復しない）にし、root task の delivery は既存の `DeliveryState::Blocked` になる。Trigger を足すのは task-core の範囲。
- 定義指紋に検査器が入ったので、`reusable_passes`（指紋を `definition_digest(required, None)` で比べる）は health の当たる候補の必須検査を再利用しなくなる。reuse-sync が `reusable_passes` に health の要求を渡すまで、再 review は必須検査をやり直す。

## 提案
- 無し。
