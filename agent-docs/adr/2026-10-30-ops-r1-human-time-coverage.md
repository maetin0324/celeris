---
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
---
# ADR 2026-10-30: 人の時間を欠測・下限と coverage で出し、断定できない外部 operator の支援を unknown で受ける

状態: 採用。

## 背景

K4 追試（task 01M669QAZE）の報告では、人の能動時間を記録しなかった試行が多い。`human-op report` は既に次を持っていた。

- 評価者の op 記録が無い試行の `human_time` は `source=missing`・値 null（0 にしない）。arm には `human_time_missing_trials` を数える。
- 長さを持てない区間（`open`・`missing_start`・`inverted`）と窓で切った区間は `lower_bound: true`。arm には `human_time_lower_bound_trials`。
- 主体不明（`actor.kind=unknown`）の op は人の時間に入れず `unknown_actor` に別掲する。

足りなかった点は次の 3 つ。

1. arm の比較表（report.md）は `human_union_secs` が無い arm（全試行が欠測）を `0` と書いていた（`cell()` の既定値）。
2. 一部の試行だけ記録がある arm・全体の合計が、下限なのか確定値なのかを 1 つの欄で示していなかった。記録のある試行・区間の割合（coverage）も無かった。
3. 外部 operator の session-map（`operator_usage`）の区分は prep・observe・intervene・shared だけで、結果に影響したか断定できない支援を
   書く区分が無かった。評価者は shared か observe に寄せるか、対応行を書かず shared_unattributable にするしかなかった。

## 決定

- D1 `human-op report` の試行・arm・全体に `human_time_coverage` を足す。欄は `bound`（`exact`・`lower_bound`・`missing`）、
  `union_secs`（`missing` なら null）、`trials`・`observed_trials`・`missing_trials`・`lower_bound_trials`・`trial_ratio`、
  `human_ops`・`ops_with_interval`・`ops_without_interval`・`op_interval_ratio`（分母 0 なら null）。
  `exact` は全試行に op 記録があり、長さを持てない区間も窓で切った区間も無いときだけ。観測済みの試行が 1 つでもあり、欠測の試行か
  長さの無い区間があれば `lower_bound`。試行の欄は記録が無ければ `source=missing` と理由を付ける。
  既存の `lower_bound`・`human_time_missing_trials`・`human_time_lower_bound_trials` は変えず、同じ判定の上に足す。
- D2 report.md の arm 表の人の時間（union・単純和）は、欄が無ければ「欠測」と書く。被覆の節（全体と arm ごと）を足す。
- D3 `operator_usage::Category` に `unknown` を足す。session-map の `unknown` は `trial_key` を持っても持たなくてもよい（空文字は拒否）。
  `unknown` を shared・shared_unattributable に読み替えない。prep・observe・intervene は従来どおり試行鍵が必須、shared は null が必須。
- D4 時刻や主体が記録に無い人の発言は推測で埋めない。始点の無い op は `missing_start`（長さ 0 として足さない、coverage の
  `ops_without_interval` に入る）、主体の分からない op は `actor.kind=unknown` で `unknown_actor` に残す。記録 CLI は既に主体の種類の
  既定値を持たない（ADR 2026-10-27-human-ops-actor-roles-request-ref D3）。この ADR では挙動を変えず、回帰試験で固定する。

## 不採用

- 欠測の試行に他の試行の平均を入れる補完。人の時間は記録された操作の範囲の下限という定義（定義書 §6）と矛盾する。
- `unknown` を `shared_unattributable` に統合する。shared_unattributable は「区間が分からず対応できなかった応答」で、
  評価者が区間を確かめた上で影響を断定できない支援とは別の事実である。
- 既存の `human_union_secs` を arm で null にする。既存の試験と読み手は「欄が無い = 欠測」で読んでおり、`human_time_coverage.union_secs`
  を足せば足りる。

## 既存の出力が変わる点

- report.json に `human_time_coverage`（試行・arm・全体）が増える。report.md の注意に 1 行、被覆の節が増え、全欠測の arm の人の時間が
  `0` から「欠測」に変わる。同じ入力からの再生成は従来どおり同一（時計を読まない）。
- operator-usage.jsonl の `category` に `unknown` が現れうる。既存の session-map・ledger はそのまま読める。

## 試験

- `crates/campaign/tests/human_ops_report.rs` の `unrecorded_human_time_is_missing_with_coverage_not_zero`
- `crates/campaign/tests/operator_usage.rs` の `unknown_category_is_kept_apart_from_shared`
