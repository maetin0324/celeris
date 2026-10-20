---
title: "ADR: 独立採点の失敗原因を checker・export まで保ち、完遂率の分母と再採点を定める"
status: accepted
date: 2026-10-20
tasks: [01M5BT8CNN9AYC6NN036DVR6F9]
---
# 独立採点の失敗原因（failure_class）の保持と分母・再採点方針

## 文脈
着手時の main（cfae4a9a）では、grader が `failure_class`（model_failure / env_failure）を出していたが、`grades::bind` が
`final_pass` の bool だけを取り込み、checker の `grader_failed` に採点基盤の不良も混ざっていた。初期 tree を渡されない採点は
`env_failure` と書かれ、「初期資産の不良」と「原因未確定」を区別できなかった。

## 決定
### D1 失敗原因の schema（4 human_ops 比較表と共有する語彙）
grade 行（`celeris.campaign-grade/1`）の `grade.failure_class`（と行頭の `failure_class`）を不合格のとき必須にした。値は次の 3 つ。

| 値 | 意味 | 成功 | 完遂率の分母 | complete |
|---|---|---|---|---|
| `model_failure` | 初期 tree が健全で、モデルの変更が成果物を壊した | 数えない | 残す | 影響なし |
| `env_failure` | 採点基盤・初期資産の不良（toolchain 欠落、hidden test 改変、初期 tree が baseline で build できない） | 数えない | 残す | **false**（再採点まで未確定） |
| `undetermined` | 原因未確定（初期 tree 未指定で compile error の出どころを決められない） | 数えない | 残す | **false**（初期 tree を渡して再採点） |

control case は `verdict=control_only`（`grader_control_only`）で、成功にも model_failure にも数えない。
不合格で `failure_class` が無い・未知の値の行は bind が拒否する（`model_failure` に既定しない）。
bind の結果は bool ではなく `GradeOutcome`（Passed / ModelFailure / EnvFailure / Undetermined / ControlOnly）。

### D2 分母と集計
- 分母 n は計画試行総数（問題数 × repeats）で固定。failed / error / timeout / cancelled / blocked / missing / 未採点 / env_failure / undetermined のどれも除かない。
- 総合値は `pass_rate = final_passed / n`（固定分母）。原因別の内訳は export の各行 `failure_breakdown`
  （model_failure・grader_env_failure・grader_undetermined・grader_control_only・ungraded・executor_*・missing）と
  `grader_env_failure` / `grader_undetermined` / `grader_control_only` 列に出す。n = final_passed + 内訳の和。
- env_failure / undetermined / 未採点が 1 件でもあれば `complete=false`。最終 `table.json` は出さず `table.incomplete.*` に書く。
  つまり固定分母の総合値は再採点が済むまで「未確定」で、環境不良を黙ってモデル失敗に混ぜない。欠測の token・費用は 0 で補わない（従来どおり）。

### D3 再採点・再試行
- 再採点（`grade-out` の再実行）は試行を走らせない。`trials.jsonl`（消費・wall・outcome）は変わらず、分母・費用は動かない。
- `grades.jsonl` は最新の 1 試行 1 行だけを持つ。置き換える前の行は `grades.history.jsonl` に追記する（履歴を失わない）。
  結合・集計は最新の `grades.jsonl` だけを読み、履歴は数えない。
- 再試行（新しい trial_no）の分母・消費の扱いは従来どおり（各 cell は trial_no 最小の記録で判定、消費は全試行の和）。

## 検証
`crates/campaign/tests/grade_failure_class.rs`: 3 種の class が bind → checker → export まで保たれること、class 欠落の拒否、
実 grader（grade_out）で env_failure → 再採点 model_failure → 再採点 pass の間に分母・token・費用・trials.jsonl が不変で履歴が残ること。
`tests/real_asset_grade.rs`: 初期 tree 未指定は `undetermined`、初期 tree が build できなければ `env_failure`。
