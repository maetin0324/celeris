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

### D4 研究対象として注入した障害（採点結果とは独立した軸）
再試行着手時の main は `adc0373e`、先行実装は `9288657f`。先行実装には注入障害と偶発障害の区別が無かった。
注入した障害そのものは成果物の不合格でも採点基盤の不良でもない。回復して合格する試行も、注入と採点環境不良が同居する試行もあるため、
`failure_class` に排他的な第 4 値を加えず、以下の独立した軸を checker/export に保存する。これが human_ops 比較表と共有する追加 schema である。

- manifest の `fault_scenarios: {problem_id: scenario_id}` で、問題ごとの注入シナリオ（版込み）を事前登録する。
  全 arm・rep で共通。同一シナリオで複数の障害を扱う場合はその組合せをシナリオとして版管理する。
  campaign hash に含め、実行後の後付け・変更は既存試行の結合を壊す。省略は未計画で、旧 manifest の hash は変わらない。
- 注入器が campaign 出力直下の `faults.jsonl` に 1 実行試行 1 行を書き、観測ログを出力ディレクトリ内に保存する。
  行は `schema=celeris.campaign-fault/1`、`campaign_sha256`、`config`、`problem_id`、`rep`、`trial_no`、
  `scenario`、`status`（`injected` または `not_injected`）、`evidence`（ログ相対 path）、`evidence_sha256` が必須。
  `injected` は実施確認の記録、`not_injected` は trigger 未到達など未実施確認の記録。予定から実施を推論しない。
  ログは注入器が観測した対象・操作・結果を記録する。checker はログ内容の意味を自動判定せず、結合・存在・非空・hash を検証する。
  model workspace の自己申告から生成してはならない。これは注入器からの記録の受け口であり、新しい汎用障害注入機構ではない。
- 未宣言シナリオ、別 campaign/試行、未実行試行、重複行、不明 status、ログ欠落・改変・出力外参照を checker が拒否する。
  宣言済みだが観測が無い cell は `unobserved`、宣言の無い cell は `not_planned` とする。
  `not_planned` は「障害が一切なかった」の証明ではなく、注入実験として計画していないことを表す。
- checker の totals と export の JSON/CSV に `fault_breakdown[exposure][result]` を追加する。
  exposure は `injected | not_injected | unobserved | not_planned`。
  result は `final_passed` または D2 の failure_breakdown の各キー。
  例: `injected.model_failure` と `injected.grader_env_failure` と `injected.final_passed` は別 cell。
  全 exposure/result の和は n。従来の failure_breakdown と重複する別軸なので、足して二重計上しない。
  注入が失敗の原因だったと自動断定はしない。原因の因果評価は同じシナリオ・非注入対照との比較で行う。
- export の `fault_scenarios` と `fault_observations` は宣言および全試行（再試行込み）の検証済み原記録を保持する。
  集計は D3 と同じく最小 trial_no。再採点は fault 原記録を変更せず、n・消費・注入状態を増減しない。
- `unobserved` または `not_injected` があれば注入実験として `complete=false`、最終表を出さない。
  欠測を 0 や未計画で補わず、まず原ログを収集する。実際に未実施なら、再採点だけでは解決しない。
  その測定は未完了のまま保存し、別 measurement の試行として再実施して実消費も残す。旧試行を消して置き換えない。
  注入済みで executor failed/timeout/cancelled/blocked の結果は通常どおり固定分母の不成功として保持する。
  注入済みでも env_failure/undetermined は D1 の再採点方針を守る。

追加検証: `grade_failure_class` の `injected_fault_is_an_independent_axis_through_real_grader_check_and_export` は
局所 subprocess に SIGTERM を送った観測ログを使い、実 grader→bind→check→export の env_failure→model_failure→pass の各段で
注入状態と原記録の保存を確かめる（grader の command runner は stub、LLM/HPC は使わない）。
未観測・未実施と結合不正・ログ改変の負例も同じ suite で確認する。
