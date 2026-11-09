---
title: 健全性v2 G3 loop-points review-path — atomic/direct の review に health の項目を足し、Fail を review の repair へ
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: done
updated: 2026-11-09
completed: 2026-11-09
---
# 健全性v2 G3 loop-points review-path — atomic/direct の review に health の項目を足し、Fail を review の repair へ

WorkUnit `review-path`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1、unit loop-points の子）。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D1.2（atomic/direct の review の経路）・D2.4・D3.1〜D3.4 を `spawn_review` → `review::review_task` の必須検査の列に配線する。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。

## 変更
commit（件名）:
- 「G3 loop-points review-path: review の必須検査に health の項目（途中。既存の試験 14 件が未対応）」
- 「G3 loop-points review-path: 必須検査の件数の試験を health の項目込みにする」
- 「G3 loop-points review-path: health の項目の型を review に移して循環を解き、G2 の試験を G3 の規則に合わせる」
- 「G3 loop-points review-path: 試験の整形」

流れ:
1. `dispatcher/review_spawn.rs`（`spawn_review`）: repo ごとの `RequiredRepoChecks` に `health: Some(RepoHealth::At(<候補の worktree、無ければ repo の dir>))` を置く。`binding` は `RequiredCheckBinding::from(&ReviewSync)` で作る（関数の行は base より 1 行少ない）。
2. `health_review/input.rs`（`HealthInputJob::attach`。review の非同期 task の中）: G2 の材料を作った後、`review_item::evaluate_all(checks, runs, remote, gate::bundled_checker())` で全 repo の health を評価する。`HealthInputJob` は `{ work: Option<HealthInputWork>, remote }` にし、材料の仕事が無い review（reviewer の合格の再利用・WU の review など）でも評価する。
3. `health_review/review_item.rs`（`evaluate_all`）: 候補（`(target_sha, reviewed_sha)`）のある repo は経路共通の入口 `gate::health_gate` で 1 回評価する。同じ束縛（repo・base・head）の G2 の評価（`gate::check_with` の `HealthRun`）があればそれを使い、検査器を起こさない。候補の無い repo は `gate::applicable` が NotApplicable ならその理由（`not_a_git_repo`・`remote_workspace`）、それ以外は `NOT_SYNCED` を `note` に残し、項目を足さない。`item(outcome)` は Pass・PolicyOnly を合格、Fail を不合格（`repair_hint = { scope: HEALTH_CHECK_ID, class: "health:<rule_id>"（複数は `,`） }`）、Unavailable・Error を `health_check_error: <理由>` の不合格にする。
4. `review/required.rs`（`evaluate`）: `RepoHealth::Evaluated` の項目を cmd の列の後に足し、指紋を `definition_digest(cmds, health.checker)`（Applies のときだけ検査器）にする。評価されないまま（`RepoHealth::At`）は `health_check_error: not evaluated` の不合格（fail closed）。`RequiredRepoResult.{health_repair_hint, health_note}` を持ち、`review.rs` は必須検査の verdict に `repair_hint` を付ける。
5. `dispatcher/review_verdict.rs`（`try_review_repair`）: acceptance の外の verdict の検査を `Verdict::implicit_check()` にした。health の Fail は `Check::Command { cmd: HEALTH_CHECK_ID, expect_exit: 0 }` として `classify_review_failure` に渡り、ほかは従来どおり `Check::Human`。

module の依存: health の項目のデータ型（`RepoHealth`・`HealthItem`・`HEALTH_CHECK_ERROR`）と `Verdict::implicit_check`・`From<&ReviewSync> for RequiredCheckBinding` は `review/required.rs` に置いた。`review` と `dispatcher` から `health_review` への辺を作らない（`health_review` は既に `review`・`dispatcher` を使うので、逆向きの辺は deps.cycle の循環を 3 単位から 4 単位に増やす。1 回目の run の check はこれで落ちた）。

## 試験（守る不変条件と、既存の試験で検出できない理由）
`health_review/review_item_tests.rs`（`gate_tests` の子 module。同じ stub の検査器を base に commit し、`evaluate_all` で評価してから `review_task` に渡す）。既存の `health_enforce_entry_*` は入口の分類だけを見て、review の必須検査の列・定義指紋・repair の材料を通らないので、配線の抜けを検出できない。
- `health_enforce_direct_review_pass_records_item_and_checker_digest`: 候補のある review は health の項目（証跡つき）を足し、指紋に適用判定の検査器が入る。
- `health_enforce_direct_review_policy_only_goes_to_policy_gate`: PolicyOnly は合格で repair の材料を持たない。
- `health_enforce_direct_review_fail_routes_to_repair`: Fail は不合格で hint（scope `HEALTH_CHECK_ID`・class `health:deps.violation`）を持ち、`implicit_check` が `Check::Command { cmd: HEALTH_CHECK_ID }` を返す。
- `health_enforce_direct_review_error_is_not_pass`: Unavailable（`checker_missing`）・exit 2・評価されないままの health は `health_check_error:` の不合格で、`Check::Human`（修復しない）。Unavailable の指紋に検査器を入れない。
- `health_enforce_direct_review_non_git_repo_adds_no_item`: Git 管理でない作業場所と remote workspace は項目を足さず、理由を `health_note` に残す。
- `health_enforce_direct_review_uses_the_g2_evaluation_of_the_same_candidate`: 同じ候補の G2 の評価を使い、検査器を起こさない。
- `health_enforce_integration_repair_review_reevaluates_the_new_head`: 統合の修復の後の新しい head では、修復の前の head の評価（束縛が違う）を使わず評価し直す。

既存の試験の変更（契約の変更に合わせたもの）:
- `dispatcher/tests/git_workspace.rs` の 2 件: 必須検査の結果に `health: pass` が 1 件増える。期待を health の項目込みにし、指紋に検査器が入ることも確かめる。
- G2 の試験の土台（`dispatcher/tests/health_review_input.rs`・`health_review/scenario_tests.rs`）: 偽の検査器に `health/engine.py`（`gate_tests::write_engine`。本物と同じ `is_fail_grade`・`policy_review_files`）を足し、report の finding を warning（exit 0）にした。G3 では fail 級の report は review を止めるので、Done を期待する G2 の試験（材料・policy の関門の試験）は fail 級でない report を使う。G3 の強制は上の `health_enforce_direct_review_*` が見る。
- `health_review_policy_unknown_flow_missing_report_waits_for_a_human` → `..._is_not_a_pass`: report を書かない検査器は、G2 の材料が policy の状態を不明と記録したうえで、review の health の項目が `health_check_error: report_missing` の不合格になり task は Failed（承認の子には進まない）。検査器の実行は 1 回（項目は G2 の評価を使う）。
- 「repo health の実行は 1 回」の 4 件（reviewer の再試行・reviewer の合格の再利用・WU・policy の承認の後の再 review）: G2 の材料の実行（report を task dir の `runs/health/` に書く行）だけを数える（`material_runs`）。これらの review では G2 が記録済みの材料を使い評価をしないので、review の health の項目が自分の scratch に report を書いて 1 回評価する。

## 証拠（提出時の HEAD で実行。最後の commit「G3 loop-points review-path: 試験の整形」の後）
- `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/health_enforce_|health_review_(input|flow|scenario|policy)|git_workspace|policy_review/)'` → 99 passed（`health_enforce_direct_review_` 6 件・`health_enforce_integration_repair_review_` 1 件を含む）
- `bash scripts/dev/check-health.sh` → exit 0（fail 級 0。new は他の unit の warning `dup.contract`（task-core の `UnavailableReason`）1 件だけ）
- `bash scripts/dev/test-affected.sh` → exit 0（check-health は verdict=pass、nextest 3633 passed・11 skipped、script bundle SKIP 0 件。対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は `crates/task-dispatch/src/{review.rs,review/,health_review.rs,health_review/,dispatcher/review_spawn.rs,dispatcher/review_verdict.rs,dispatcher/tests/}` とこの進捗 file だけ

## 未解決
- `classify_review_failure` は `Check::Command { cmd: HEALTH_CHECK_ID }` を分類しない（`classify_command` が `None` を返し Substantive）。health の bucket（`health:<rule_id>`）の分類は `crates/task-core/src/execution.rs` で、この unit の範囲の外（integration-path の範囲）。統合されるまで、review の health の Fail は repair にならず ReviewFail（不合格）になる。
- D3.4 の「task を Blocked にし受信箱に `health_check_error: <reason>` を 1 件」: review の経路には reviewing → blocked の Trigger が無い（`ChildFailed` だけ）。今は Error を不合格（修復しない）にしており、task は Failed になる。Blocked にするには task-core の Trigger と `review_verdict` の分岐が要る。範囲の外なので flow・record で扱うか replan で unit を足す。
- 記録済みの G2 の材料を使う review（reviewer の再試行など）では、review の health の項目が検査器を 1 回起こす（Celeris の repo では 1 回 約 22〜95 秒）。束縛の同じ Pass の証跡の再利用は reuse-sync（`required_checks::reusable_health`）の範囲。
- 定義指紋に検査器が入ったので、`reusable_passes`（指紋を `definition_digest(required, None)` で比べる）は health の当たる候補の必須検査を再利用しなくなる。reuse-sync が health の要求を渡すまで、再 review は必須検査をやり直す。
- `health_note`（NotApplicable の理由など）は `RequiredRepoResult` と tracing の info に残るが、event には書かない。

## 提案
- 無し。
