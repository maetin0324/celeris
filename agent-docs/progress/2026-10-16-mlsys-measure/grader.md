---
title: "MLSys 本測定: 独立 grader（実装テスト・実行証拠・独立再計算・欠陥検出）"
tasks: [01M51QRH7TD3FHER7HVF9W3TAH]
unit: grader
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# grader（WU grader）

ADR: `agent-docs/adr/2026-10-16-mlsys-independent-grader.md`（実装より先に書いた）。

## 状態の区分

| 区分 | 内容 |
|---|---|
| 実装済み | `crates/campaign/src/grader.rs`（単一 file。判定と CLI、inline mod `case` case schema `celeris.grade-case/1`、`implementation` 実装テスト、`evidence` 実行証拠、`recompute` 独立再計算、`review` 欠陥検出）。`lib.rs` に `pub mod grader;`、`main.rs` に `grade` 分岐（fmt で 3 行）。`celeris-campaign grade --case <file> --artifact <dir> [--work <dir>]` は判定 JSON（`celeris.grade/1`）を出し、合格 exit 0／不合格 exit 1 |
| mock 検証済み | `crates/campaign/tests/grader_independent.rs` 16 件（stub runner と合成成果物）。下表 |
| 実経路検証済み | CLI binary を起動し判定 JSON と exit code を確かめた（`cli_grade_outputs_json_and_exit_code`）。CLI は評価 command を走らせない（`NoRunner`、fail-closed）ので、正直な成果物でも実装テストは「未実行」で fail・exit 1、他 3 検査は pass になることを確認。**評価 command を実際に走らせる実行器・実 BenchFS の cargo test・実 Sirius 原票での採点は未実施** |
| 未実施 | K1〜K4 の実 case file（pristine テスト・参照値・欠陥一覧）での採点（WU `cases` の成果を待つ）。BenchFS の受入 command の実行（UCX が要るので login node、inventory R4）。K3/K4 の実原票（`main-43223.tgz` 等）を `metrics` で再計算する設定 |

## 試験（mock 検証）と不合格になる事例

| 事例 | 試験 | 落ちる検査 |
|---|---|---|
| 正直な成果物は 4 検査とも pass、評価 command は pristine を重ねた作業 dir で 1 回 | `honest_artifact_passes_all_four_checks` | なし（対照） |
| 成功宣言だけ（report.md と result.json の passed） | `success_claim_only_fails`、CLI 版 | 4 検査すべて fail、command は走らない |
| もっともらしい `metric` 行つき報告だが生データなし | `plausible_report_with_metric_lines_but_no_raw_data_fails` | evidence・recompute |
| 評価テスト削除 | `deleted_evaluation_test_disqualifies_without_running` | implementation（失格、command 不実行） |
| 評価テスト弱体化（`#[ignore]` 追加／assert 削除／`#[cfg(any())]`） | `weakened_evaluation_test_is_detected` | implementation（sha256 不一致＋weakened の内訳） |
| 空白だけの変更 | `whitespace_only_change_is_still_a_mismatch` | implementation |
| grader 側 pristine の改変 | `altered_grader_pristine_is_refused` | implementation |
| 試験失敗・0 件実行で exit 0 | `failing_or_empty_test_run_fails` | implementation |
| mock 証拠＋mock trial 記録の Passed | `mock_evidence_with_passed_record_fails_outside_control` | evidence |
| control case は全 pass でも `control_only`・`final_pass=false`、検査 0 件は fail | `control_case_never_final_passes` | — |
| 生データ改竄 | `tampered_raw_data_fails_evidence_and_recompute` | evidence（sha256）・recompute（値） |
| 固定入力を書き換え報告も辻褄合わせ | `consistently_forged_pinned_input_is_caught_by_the_pin` | recompute（pinned sha256） |
| 数値捏造・分母（count）捏造・未報告・非数値 | `fabricated_or_missing_reported_numbers_fail` | recompute |
| 条件（nodes）不一致・成果物外 path・`Unknown Job Id`・job 不足 | `experiment_condition_and_scheduler_mismatch_fail` | evidence |
| 誤指摘（list にも実在もしない）・場所だけ合う指摘・defect id の自己申告 | `false_findings_and_missed_defects_fail_review` | review |

## 証拠コマンドと結果（2026-10-16、grader.rs 単一 file 化後）

- `cargo test -p celeris-campaign` → 全 binary ok（grader_independent 16 passed、runner_checker 16 passed＝base のまま）
- `bash scripts/dev/test-parallel.sh` → `test-parallel: ok`（passed 5399, failed 0, ignored 13, nextest_exit 0, doctest_exit 0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 変更 path（`sh $CELERIS_WU_SCOPE_PATHS`）: ADR・進捗 grader.md・`src/grader.rs`・`src/lib.rs`・`src/main.rs`・`tests/grader_independent.rs` のみ

## 試行間の分離と漏洩検査（追記、WU grader-notes）

実装: `crates/campaign/src/isolation.rs`。

**分離の仕組み**
- `prepare_trial` は試行（A/B/C）ごとに workspace・materials・kb を同じ初期 snapshot から**新規に複製**する。試行は他試行の KB を共有せず、書き込みも元 snapshot へ戻らない。
- `withheld_names` に挙げた file／dir（final の期待値、既知欠陥 list、grader の pristine テスト）は複製時に除く。試行の workspace に解答側の情報を置かない。
- `inputs.json` に repo／materials／kb／budget の hash を記録し、`check_same_inputs` が config 間の hash 一致（同じ初期情報・資源）を検査する。
- `finish_trial` は KB の最終 hash を返して試行の KB を破棄する（次の試行に持ち越さない）。
- 公開部（`cases/`）と grader 私有部（`fixtures/grader-private/`）は別の木。ローダ（`cases.rs`）が、dev 側の file に final の case id・欠陥 id・private hash・初期 commit が現れたら拒否する。

**漏洩を検出する検査**: `verify_isolation` が各試行の dir を走査し、他試行の path・他試行固有の hash・全試行の final 値を含む file を `Leak { in_trial, file, what }` として報告する（試行自身の `inputs.json` は対象外）。

| 試験 | 確かめること |
|---|---|
| `tests/isolation.rs::three_configs_get_identical_input_hashes` | A/B/C の入力 hash が同一 |
| `tests/isolation.rs::kb_write_in_trial1_is_invisible_to_trial2_and_discarded` | 試行 1 の KB 書き込みが試行 2・元 snapshot に出ず、finish で破棄される |
| `tests/isolation.rs::final_expectations_and_pristine_tests_are_not_in_workspace` | final 期待値・欠陥 list・pristine テストが workspace にない |
| `tests/isolation.rs::verify_isolation_detects_intentional_leaks` | 意図的に仕込んだ path／hash／final 値の漏洩を検出 |
| `tests/measure_e2e.rs::dev_case_expands_into_three_identical_isolated_workspaces` | dev case が 3 つの同一入力の隔離 workspace に展開される（case×isolation） |
| `tests/cases_load.rs::final_expectations_are_not_reachable_from_dev_layout`、`rejects_dev_file_that_names_a_final_value`、`rejects_grader_root_inside_cases_root` | dev 側から final の値・grader 私有部に届かない |

## grader の 4 検査・テスト改変検出・dev/final 分離の区分

grader（`crates/campaign/src/grader.rs`）が検査する 4 項目: (1) 実装テスト、(2) 実験条件と実行証拠（job ID・ログ・生データ sha256）、(3) 生データからの独立再計算、(4) 既知欠陥の検出と誤指摘。

| 項目 | 実装済み | mock 検証済み | 実経路検証済み | 未実施 |
|---|---|---|---|---|
| 1 実装テスト（pristine を重ねて評価 command を実行、0 件・失敗は fail） | `implementation` | `honest_artifact_passes_all_four_checks`、`failing_or_empty_test_run_fails`（stub runner） | CLI は `NoRunner`（fail-closed）で「未実行」fail になることのみ（`cli_grade_outputs_json_and_exit_code`） | 実 BenchFS の cargo test の実行（UCX が要り login node） |
| 2 実験条件と実行証拠 | `evidence` | `experiment_condition_and_scheduler_mismatch_fail`、`mock_evidence_with_passed_record_fails_outside_control` | なし | 実クラスタ job の scheduler 記録での採点 |
| 3 独立再計算 | `recompute`（生データと固定入力 sha256 から再計算） | `tampered_raw_data_fails_evidence_and_recompute`、`fabricated_or_missing_reported_numbers_fail`、`consistently_forged_pinned_input_is_caught_by_the_pin` | なし | K3/K4 実原票（`main-43223.tgz` 等）での再計算 |
| 4 既知欠陥の検出と誤指摘 | `review` | `false_findings_and_missed_defects_fail_review` | なし | 実 case の A4/A6 欠陥 list での採点 |
| 評価側テストの削除・弱体化の検出 | `implementation`（sha256 不一致、`#[ignore]`／assert 削除／`#[cfg(any())]` の内訳） | `deleted_evaluation_test_disqualifies_without_running`、`weakened_evaluation_test_is_detected`、`whitespace_only_change_is_still_a_mismatch` | なし | 実 case の pristine テストでの確認 |
| dev／final 分離 | `CaseSplit`、ローダの split 検査、final 私有部の別配置 | `final_case_is_graded_separately_and_its_expectations_are_private`、`real_cases_pin_distinct_inputs_between_splits`、`control_cannot_enter_measurement_or_final`、`control_case_never_final_passes` | なし | 実 A/B/C 試行での final 採点（final は 1 度だけ使う運用） |

総括: 実経路検証済みは CLI の判定 JSON と exit code のみ。4 検査の実 case・実 job での採点は**未実施**で、mock の Passed や成功宣言は最終合格にならない。

## 証拠コマンドと結果（追記、WU grader-notes、2026-10-16）
- `cargo test -p celeris-campaign` → exit 0。合計 **82 passed, 0 failed, 1 ignored**（unit 11、cases_load 11（1 ignored）、cli_dryrun 2、dependency_wait 7、grader_independent 16、isolation 4、measure_e2e 4、pilot_fixtures 8、routing_eval_identity 3、runner_checker 16）。

## 既存試験との関係
`tests/runner_checker.rs::run_refuses_split_mix_and_unknown_executor_path_has_no_network_code` は `src/` 直下の entry を全て file として
読み、`process::Command` 等を禁じる。前回の run は grader を `src/grader/` に置いて同試験を書き換えたため範囲 check で落ちた
（計画を replan）。今回は grader を単一 file `src/grader.rs` にし、子 process を起こす実行器を src から除いた（`NoRunner`）。
`runner_checker.rs` は base のまま変えていない。

## 未解決事項
- `cargo clippy --workspace --all-targets` は `crates/celeris/src/daemon/jev_verifier.rs:54`（items after a test module）で落ちる。
  この unit の範囲外で base から存在する。gate の `cargo clippy --workspace -- -D warnings` は通る。
- 実 case（K1〜K4）の case file は WU `cases` が作る。grader の schema（`celeris.grade-case/1`）に合わせるか、変換が要る。

## 提案
- 評価 command を実際に走らせる実行器（`CommandRunner` の実装）をどこに置くかを決める。src の guard を保つなら e2e／campaign 計測
  側の crate か試験 harness に置き、`grader::grade` に注入する。
- WU `cases` は K1/K2 の pristine テスト（086c4618 の追加試験 3 件など）を `implementation.tests` に、K3/K4 の比・有効セル数を
  `recompute.metrics`（`ratio`・`count`）に、A4/A6 の既知欠陥を `review.known_defects` に書く。報告側には
  `metric <name> = <value>` 行を出すよう problem 文に書く。
- 実行証拠の `evidence/jobs.json` の形（job_id・scheduler_record・log・raw[path,sha256]・conditions・evidence_kind）を
  試行の作業指示に入れる（WU `isolation`／e2e）。
