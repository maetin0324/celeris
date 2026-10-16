---
title: "MLSys pilot で見つかった campaign 配線の欠けと runbook の誤りを直す"
tasks: [01M52KQ1FV24YAATBA5ZM0E48K]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# MLSys pilot の配線修正

ADR: `agent-docs/adr/2026-10-16-mlsys-pilot-wiring-fixes.md`（D1〜D9）。実 LLM・本番 API は使っていない（mock executor・stub 実行ファイル・temp dir・temp DB）。
本番 host の操作・pilot の再実行・本測定はしていない（運用セッションが行う）。

## 所見ごとの対応（変更 → 試験）
| # | 所見 | 変更 | 試験 |
|---|---|---|---|
| 1 | `--api-url …/api/v1` が二重、相対 `--out` で A の exit-code 経路が壊れる | `CurlTransport::new` が path 付き URL を拒否（origin だけ）。`main.rs` が `--out` を `std::path::absolute` | `curl_transport::tests::rejects_bad_names_and_urls_without_leaking`、`pilot_wiring::cli_rejects_api_url_with_path_and_nested_template_before_any_request` |
| 2 | template の acceptance 形が 400、HTTP 本文が捨てられ、workspace 不在エラーが原因を隠す | `celeris_exec::check_template`（入れ子 `check` と type 無しを拒否、実行器が埋める欄も拒否）。試行記録に `error`（2 KiB まで）。executor 失敗時に workspace が無ければ初期木から作る | `pilot_wiring::task_template_accepts_only_the_api_acceptance_shape`、`…executor_error_body_is_recorded_and_a_missing_workspace_does_not_hide_it` |
| 3 | max_parallel が効かず全試行を同時投入 | runner の `at_parallel_cap`: 非同期 executor の in-flight（未完了の台帳行）が上限なら投入せず `waiting`（`parallel_deferred`）。`CelerisExecutor::usage` を実装（実行中の per_case/per_arm 判定） | `pilot_wiring::max_parallel_caps_in_flight_trials_and_later_runs_submit_the_rest`（各 run 後に in-flight ≤ 1、1 試行 1 task） |
| 4 | A が permission mode なしで起動、無変更なのに passed | `ClaudeCodeBuilder` に `--permission-mode bypassPermissions`。A が Passed でも workspace hash が初期木と同じなら `failed`/`no_artifact`。checker も A の無変更 passed を違反にする | `pilot_wiring::a_exit_zero_without_any_artifact_is_failed_not_passed`、`…a_that_writes_its_artifact_passes`、`…claude_code_a_is_started_with_a_permission_mode_and_cache_usage_is_parsed` |
| 5 | materials が試行に配られない | `snapshot/workspace-seed` = repo + `materials/` を試行の初期木に（repo に `materials/` があれば拒否）。snapshot.json に `workspace_seed_sha256` | `pilot_wiring::materials_are_distributed_into_every_trial_workspace` |
| 6 | review case が含まれない case の `out/report.md` に依存 | `check_self_contained`（validate・run が拒否）。fixture の review case を `materials/pluvio-report-draft.md` を読む自己完結の case に直し、input-manifest / pilot.json を作り直した | `manifest::tests::case_reading_another_cases_output_is_rejected`、`pilot_wiring::pilot_fixture_cases_are_self_contained` |
| 7 | cache tokens を除外（約 100 倍の差） | `TokenUsage{input,output,cache_read,cache_creation}` を試行記録 `token_usage`・summary `cells.<arm>.tokens`・export 7 列に。`budget.token_basis` 必須（既定なし、`input_output` / `input_output_cache_read` / `all`）。`tokens` は基準で数えた量、基準に要る量が欠ければ欠測 | `pilot_wiring::all_token_kinds_are_kept_in_records_summary_and_export_and_basis_decides_the_count`、`…checker_rejects_tokens_that_do_not_follow_the_basis`、`manifest::tests::missing_budget_field_is_rejected_at_parse_and_zero_at_validate` |
| 8 | `db backup` が稼働中の本番 DB で終わらない | `task_core::backup_database` を読み取り専用接続の `VACUUM INTO`（`<dest>.partial-<pid>` → rename）に | `store::tests::backup_database_finishes_while_another_connection_is_writing`（別接続が書き込み transaction を保持したまま完了・未 commit は含まない・partial を残さない） |
| 9 | grade が NoRunner、grade-case が無い | `NoRunner` を消し `grade_runner::ProcessRunner`（作業 dir で実行・timeout で process group kill）を CLI に注入。`grade-out` で out の全試行を採点し `grades.jsonl`。pilot の grade-case 2 件を `crates/campaign/fixtures/grade-cases/pilot/` に。fixture repo を単独の cargo package に | `pilot_wiring::pilot_impl_grade_case_really_runs_cargo_test_in_the_grade_work_dir`（実 `cargo test --offline`）、`…pilot_review_grade_case_detects_the_known_defects`、`…grade_out_grades_every_run_trial_and_reports_missing_cases`、`grade_runner::tests::*`、`grader_independent::cli_grade_outputs_json_and_exit_code` |

runbook（`../2026-10-16-mlsys-final-connect/runbook.md`）・`../2026-10-16-mlsys-final/run-final.md`・`final.json`・`task-template.json` を実際の CLI に合わせた
（origin だけの `--api-url`、`{"type","text"}` の acceptance、`token_basis` の欄、`waiting=0` までの再実行、`grade --artifact …/rep1/workspace`、`grade-out`、VACUUM INTO の `db backup`）。

## Jev smoke の所見（直さない）
C は jev_ok・lane standard・basis fixed_rule だが、`real_api_verified=false`、Jev の費用・版が記録されない、route decision は shadow / route_defaults。
これらは campaign の配線ではなく Jev 側（routing の decision 記録）の欠けで、この task の対象（crates/campaign・db backup・runbook）の外。
本測定の C の結果を引用するときは「Jev の実 API 呼び出しは未検証、費用・版は未記録」と注記する（人向け手順）。直すなら別 task（下の提案）。

## 証拠コマンドと結果
- `cargo test -p celeris-campaign`（追加 14 件の pilot_wiring を含む）: 全 pass。
- `celeris-campaign run --executor mock --manifest crates/campaign/fixtures/pilot.json --out $TMPDIR/campaign/mock`: executed 12、tokens_total 113000、`check` ok。
- 全体試験・clippy・fmt の結果は下の「最終検査」。

## 最終検査
（下に追記）

## 本番への移行（人が行う）
- 新しい release を昇格するまで、運用セッションは旧い `celeris-campaign` を使わない（manifest に `budget.token_basis` が要り、`--api-url` は origin だけ）。
- `budget.token_basis` の値は人の決定（decision で運用セッションが答える）。決まるまで `final.json` の `<人の決定: …>` が残り、manifest は読めない（fail-closed）。
- `final.json` の sha256 は変わった（`freeze-final.md` の置換前 hash は旧い）。run-final.md 手順 2 で取り直す。
- pilot はこの release で新しい `--out` に回し直す（旧い trials.jsonl は新しい欄を持たない）。旧い pilot manifest（`../2026-10-16-mlsys-pilot/pilot-live.json`）は `token_basis` が無いので、そのままでは読めない。

## 未解決事項
- `budget.token_basis` の人の決定（予算の判定にどの量を使うか）。
- K1〜K4（本測定の case）の grade-case は未作成（run-final.md 手順 5 の `$P/grade-case/`）。

## 提案
- Jev の route decision に `real_api_verified`・費用・Jev の版を記録する別 task（Jev wire 側）。
- `campaign run` に `--until-done`（`waiting=0` まで poll 間隔を空けて繰り返す）を足すと、max_parallel 下の手での再実行が要らなくなる。
