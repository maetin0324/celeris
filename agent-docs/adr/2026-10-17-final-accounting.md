---
title: "最終報告の消費集計: 外側 trial と decision-log を加算せず照合する"
status: accepted
date: 2026-10-17
---
# 最終報告の消費集計（final accounting）

親: `2026-10-15-decision-log-export.md`（D5〜D8）、`2026-10-16-campaign-budget-reservation.md`。P1-6 record-contract の正式入力（`records.jsonl` の usage / run / trial record）を `crates/campaign/src/final_accounting.rs` が読む。writer・DB・budget 制御は変えない。

## 決定
- D1. 外側 trial（`trials.jsonl`、checker 検査済み）の合計と、同じ run の decision-log 消費は**加算しない**。cell（arm, case, rep）ごとに照合し、`match` / `mismatch` / `incomplete` / `no_breakdown`（旧記録）/ `not_executed` を出す。内訳の無い cell は外側の値を `undecomposed` に置き、推測で分解しない。
- D2. 重複排除は `dedup_key`（run は `run_id:usage_source`、Jev/CoS は call）。同内容の重複は 1 件、内容が食い違う鍵は**先着で選ばず**数えない（入力順に依らない）。別 campaign・別 manifest hash の行は採らず件数だけ残す。run に複数の usage 行があるときは二重の恐れがあるので数えない。
- D3. retry は `by_retry`（`additive:false`）の別の切り口。`by_role` に 1 回入った費用を再加算しない。
- D4. 共有（CoS など run・case に割れない行）は `shared_unattributable` で総和に足さない。Jev は cell に写せるときは cell の coverage に、総和には call 1 回で入れる。
- D5. 呼ばれなかった 0 は根拠（`basis`: arm が Celeris 外 / fixed routing / 分類器が rule / cell 未実行 / `--cos-not-called` で渡した根拠）があるときだけ `not_called`。根拠が無い・行が無い・全欄が欠測は `unretrieved` / `unknown` / `missing` / `not_retrieved` として gap にする。
- D6. token は manifest の `token_basis`（既定の論文用は cache_read・cache_creation を含む All）で数え、内訳も別に出す。定価換算・実支出・executor 報告費用・quota は別項目で互いに代用しない。
- D7. 完全でない和は `observed_sum`、`total` は指標（tokens / list_cost_usd / cash_cost_usd）ごとに gap が無いときだけ値を持つ。gap があれば `reduction_confirmable` は false。`recheck` が出力だけから再計算して矛盾（完全の偽装・retry の再加算）を落とし、`report::check_consumption_claims` が報告の `consumption … scope=complete` / `reduction … confirmed=true` を落とす。
- D8. `budget` 節: 実行中の見え方（欠測を `per_case.max_tokens` で見積もる課金、in-flight の予約、snapshot）と最終 cutoff（観測 tokens のみ、欠測は欠測のまま、未解放の予約は gap）の差を出す。budget の制御自体は変えない。

## CLI
`celeris-campaign final-accounting|export|check-report --records <records.jsonl> [--cos-not-called <根拠>] [--quota <usage.json>]`。`export` は `final-accounting.json` を書き、表に `consumption`（範囲と削減可否）を足す。`--records` が無ければ従来どおり。
