---
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
---
# ADR 2026-10-30: ops-r1 の消費を 6 区分の名前で出し、宣言 model と観測 model・実行経路・比較範囲を集計に載せる

状態: 採用。

## 背景

K4 追試の報告第 2 版（task 01M6617H9V の `artifacts/ops-r1/report.md`）には、集計の出力だけでは誤読される点が残った。

- 消費の区分に名前が無い。final-accounting は CoS を `shared_unattributable.cos` に `added_to_total=false` で出すが、
  task 実行系・案件 CoS・統括 CoS・外部 operator・共有・評価器の区分は報告の本文にしか無い。B-t002 の試行の tokens 0 は
  「task 実行系が 0」で、運用全体の 0 ではない。
- trials.jsonl の B の `model` は manifest の宣言（claude-sonnet-5-5）の写しで、実際に作業した CoS の model（claude-opus-5-5）が
  試行行に無い（報告 §8 の 8）。
- B-t002 は案件 CoS が起票せず自分で済ませた（`cos_self_completed`）。task 経路（planner/worker/reviewer）は動いていない。
  `reduction_confirmable` は欠測の有無だけで決まり、条件の境界の外の試行や model の違う組でも true になりうる。

## 決定

- D1 試行行（`TrialRecord`）に任意欄 `execution_path` と `observed_model` を足す（ops-r1-assemble だけが書く。runner の試行は持たず、
  欄も出ない）。`execution_path` は `direct_harness`（A）・`task_pipeline`（B、起票して task が作業）・`cos_self_completed`・`stopped`。
  `observed_model` は試行の作業をした run の実際の model（A は transcript の models、B の task 経路は task run の model、
  `cos_self_completed` は受付の CoS run の model、停止は task run の model）。観測 0 件は null で、宣言名で埋めない。
  既存の `model` 欄は宣言の写しのまま残す（runner・checker・既存 raw の schema と共通の欄で、名前を変えると全 campaign の記録の
  読み書きが変わる）。宣言は `model`、観測は `observed_model` と doc comment に明記する。
- D2 records.jsonl の usage 行に `model`（attribution の観測値）と `consumption_lane` を足す。`runs` は `task_execution`、
  CoS の `cos_project` は `project_cos`、`cos_general` は `general_cos`、それ以外の共有は `shared`。
- D3 final-accounting に `consumption_lanes` を足す。鍵は `task_execution`・`project_cos`・`general_cos`・`external_operator`・
  `shared`・`evaluator` の 6 つで、`added_to_total` が true なのは `task_execution` だけ（`total_covers: ["task_execution"]`）。
  CoS の区分は行の state（`observed`・`missing`・`no_rows`）・models・rows を出す。外部 operator は records に無いので
  `outside_records`（operator transcript から別に数える）、評価器は決定的な program なので `not_called`。lane も cos_role も無い CoS 行は
  `cos_unclassified` に出し、案件・統括のどちらとも推測しない。既存の `shared_unattributable`（`added_to_total=false`）は変えない。
  recheck は 6 区分の `added_to_total` と `task_execution.trial_outer == trials.outer` を検査する。
- D4 final-accounting に `comparison_scope` を足し、`reduction_confirmable[metric] = total.complete[metric] && comparison_scope.comparable` とする。
  ops-r1 の manifest（`contract.conditions`）では、実行した試行ごとに次の理由を集め、理由が 1 つでもあれば comparable=false。
  - 実行経路が条件の境界（A=`direct_harness`、B=`task_pipeline`）の外、または欠測。
  - 観測 model が欠測、または宣言 model と違う。
  - 腕の間で観測 model の集合が違う。
  runner の campaign（ops-r1 でない）は腕ごとに model を変える設計（routing の C など）があるので、経路と model の規則を当てず、
  `reduction_confirmable` は従来どおり欠測だけで決まる。どちらの campaign でも `cross_measurement` は `not_comparable` で、
  別の測定系列（旧 A0/B0 と ops-r1 など）の数値を削減として比べない。cell 行に `declared_model`・`observed_model`・`execution_path` を出す。
- D5 export の表の `consumption` に `comparison_scope`（comparable・reasons）と `lanes_added_to_total` を足す。records が無い export では
  `comparison_scope` は null。
- D6 report の消費の主張検査（`check_consumption_claims`）に 2 種の行を足す。`comparison scope=<comparable|not_comparable>` は
  `comparison_scope` と一致しなければ違反。`lane name=<区分> added_to_total=<bool>` は区分の計上と一致しなければ違反。
  比較範囲の外で `reduction ... confirmed=true` と書くと、理由（reasons）付きの違反になる。

## 既存の出力が変わる点（`--b-cos-self-completed` を渡さない入力も含む）

- ops-r1-assemble の出力は、`--b-cos-self-completed` の有無にかかわらず変わる。trials.jsonl の A・B の行に `execution_path`・
  `observed_model` が、records.jsonl の usage 行に `model`・`consumption_lane` が入る。理由: 宣言と観測の model と実行経路は
  どの B（task 経路・停止・CoS が自分で済ませた）でも報告に要り、入力ごとに欄の有無を変えると下流が欄の欠落を区別できない。
  したがって親 task（01M62S8X）と追試 task（01M6617H9V）の既存の out・records.jsonl とは byte 一致しない。採点（grade-out の
  final_pass）・outcome・reason・wall・tokens の値は変えない。2 回の再生成の一致は新 binary 同士で確かめる（WU regrade）。
- final-accounting は ops-r1 でも runner の campaign でも `consumption_lanes`・`comparison_scope` と cell の 3 欄が増える。
  ops-r1 で境界の外の試行（B-t001 の `stopped`、B-t002 の `cos_self_completed`）か model の違う組を含むと、欠測が無くても
  `reduction_confirmable` は false になる。runner の campaign の `reduction_confirmable` は変わらない。
- 旧欄を残す互換分岐は作らない。旧 binary の final-accounting を新 recheck に通すと `consumption_lanes`・`comparison_scope` の欠落で違反になる。

## 検討して採らなかった案

- `TrialRecord.model` を `declared_model` に改名する: runner・checker・export・既存の raw の trials.jsonl すべての読み書きが変わり、
  この WU の範囲（ops-r1 の集計）を超える。宣言欄の意味は doc comment と本 ADR で固定する。
- CoS の消費を試行の tokens に足して「B の総消費」を出す: 人の判断 2（CoS は試行に足さず別掲）に反する。
- 外部 operator の消費を final-accounting の入力に取る: operator transcript の集計は `operator-usage` の別の出力で、
  この WU では区分の名前と `outside_records` だけを出す。
