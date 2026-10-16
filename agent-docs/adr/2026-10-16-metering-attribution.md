---
status: accepted
date: 2026-10-16
---
# 全消費量の計測: 帰属規則・共有オーバーヘッド・欠測・elapsed（migration 0085）

決定ログ（ADR 2026-10-15-decision-log-export、campaign 識別は 2026-10-16-campaign-identity-propagation）の消費行 `decision_log_usage` を、worker 以外の消費まで埋める。

## 決定
- D1 role は cos / planner / worker / reviewer / jev。planner・worker・reviewer は `worker_finished` の投影（既存）、再試行は `is_retry`。Jev は dispatcher の `apply_lane_assist` が board の分類結果（`JevOk`）から `decision:jev:<subject>:1` を置換で書く。費用は `ClassificationEntry.cost_usd`（Jev 応答の `usage.cost`）。返らなければ NULL。
- D2 帰属規則: `attribution_rule`（NULL = campaign に直接帰属。規則を使った行は名前を残す）。`jev_classification_subject`（分類した WU/task の campaign へ）、`cos_turn_shared_overhead`。
- D3 共有オーバーヘッド: `shared_overhead=1`。CoS の対話のように run・case に割れない処理。`CampaignMetering` は `direct` と `shared_overhead` を別に集計し、混ぜない。
- D4 欠測: 数値の NULL は欠測、0 は 0 と観測した値。`MeteredSum` は `sum=None`（1 件も観測なし）・`n_observed`・`n_missing` を持つ。欠測を 0 として足さない。
- D5 二重計上防止: 行の主キーは `dedup_key`（置換）。再処理は同じキーを置換するだけで合算しない。
- D6 区分: token・定価換算（list）・実支出（cash）・クラスタ資源は別欄。cash は取れなければ NULL で、list で代用しない。quota は消費行に載せない。
- D7 elapsed と run 時間総和: elapsed = `decision_log_campaign_activity.last_activity_at`（MAX で畳む。決定ログ書込みの注入時計）− `task_campaigns.created_at` の最小。run 時間総和 = run 由来の消費行 `wall_ms` の合計（並列 run では elapsed を超える）。別欄 `elapsed_ms` / `run_wall_sum_ms`。
