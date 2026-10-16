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
| 実装済み | `crates/campaign/src/grader/`（`mod.rs` 判定と CLI、`case.rs` case schema `celeris.grade-case/1`、`implementation.rs` 実装テスト、`evidence.rs` 実行証拠、`recompute.rs` 独立再計算、`review.rs` 欠陥検出）。`lib.rs` に `pub mod grader;`、`main.rs` に `grade` 分岐（fmt で 3 行）。`celeris-campaign grade --case <file> --artifact <dir> [--work <dir>]` は判定 JSON（`celeris.grade/1`）を出し、合格 exit 0／不合格 exit 1 |
| mock 検証済み | `crates/campaign/tests/grader_independent.rs` 16 件（stub runner と合成成果物）。下表 |
| 実経路検証済み | CLI を実 process runner（`sh -c`）で起動し判定 JSON と exit code を確かめた（`cli_grade_outputs_json_and_exit_code`）。**実 BenchFS の cargo test・実 Sirius 原票での採点は未実施** |
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

## 証拠コマンドと結果

- `cargo test -p celeris-campaign --test grader_independent` → 16 passed, 0 failed
- `cargo nextest run -p celeris-campaign` → 63 passed
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo clippy -p celeris-campaign --all-targets -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-parallel.sh` → `test-parallel: ok`（passed 5399, failed 0, ignored 13, nextest_exit 0, doctest_exit 0）

## 既存試験の変更
`tests/runner_checker.rs::run_refuses_split_mix_and_unknown_executor_path_has_no_network_code` は `src/` 直下の file だけを
読んでいて、`src/grader/` を file として読もうとして落ちた。走査を再帰にし（走査した file 数の下限も検査）、
`process::Command` は `src/grader/implementation.rs`（評価 command 実行器、ADR D3）に限って許す。network・LLM API の語は
全 file で禁止のまま（弱めていない）。

## 未解決事項
- `cargo clippy --workspace --all-targets` は `crates/celeris/src/daemon/jev_verifier.rs:54`（items after a test module）で落ちる。
  この unit の範囲外で base から存在する。gate の `cargo clippy --workspace -- -D warnings` は通る。
- 実 case（K1〜K4）の case file は WU `cases` が作る。grader の schema（`celeris.grade-case/1`）に合わせるか、変換が要る。

## 提案
- WU `cases` は K1/K2 の pristine テスト（086c4618 の追加試験 3 件など）を `implementation.tests` に、K3/K4 の比・有効セル数を
  `recompute.metrics`（`ratio`・`count`）に、A4/A6 の既知欠陥を `review.known_defects` に書く。報告側には
  `metric <name> = <value>` 行を出すよう problem 文に書く。
- 実行証拠の `evidence/jobs.json` の形（job_id・scheduler_record・log・raw[path,sha256]・conditions・evidence_kind）を
  試行の作業指示に入れる（WU `isolation`／e2e）。
