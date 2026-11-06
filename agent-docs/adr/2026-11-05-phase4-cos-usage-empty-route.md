# 案件 CoS の chat run の usage が null になる原因は pilot の [cos].route_candidates が空だったこと

日付: 2026-11-05 / task 01M6PYNMP4EFRX9C5SGT6XB97E

## 背景
P0-F の再実行（C06・C08 の new 腕）で、案件 CoS thread の chat_runs.usage_json が 14 件とも null だった（`cos_usage_null:6`・`:8`）。本番の chat_runs 413 件は全件に usage が入っている。

## 判断
- D1: daemon は直さない。usage を記録する経路は案件 CoS も統括 CoS も同じで、`cos_chat/rollover.rs` の `run_attempts` → `record_attempt` → `ChatRunSink::record_usage` → `finish`（`sink.rs`）→ `chat_run_finish_with_telemetry` に集約されている。案件別の分岐はない。
- D2: usage が残らないのは、run が harness を起動する前に `routes.rs` の `launch_routes` が「CoS route unavailable」で `fail_routes` し、`chat_run_finish`（usage 引数なし）で終わる場合である。`driver.py` の実 LLM 側は `route = a.cos_route`（既定 `"[]"`）を `[cos].route_candidates` に書く。空だと `select_cos_route_from`（`route_select.rs`）が候補を選べない。
- D3: 直し方は pilot 側。`driver.py` は実 LLM の tier で `--cos-route` が空なら起動せずに止める。operator は `--cos-route '[{ source = "claude_oauth", model = "claude-sonnet-5-5" }]'` を渡して再実行する。
- D4: project_guard は変えない。`project_run_usage_between`（`project_guard.rs`）は chat_runs.usage_json を和に入れており、route が通った run の usage は入る。

## 未確認
再実行の一時 DB（state2）は残っておらず、14 件の run の state・reason は直接見ていない。operator が実際に渡した引数も log に無い。D2 は code と config からの推定。

## 付記: 試験による確認
案件 CoS thread の chat run が usage を記録し、`project_run_usage_between` の和に入ることを `harness_tests.rs` の `project_cos_chat_run_records_usage_and_counts_in_project_budget`（fake adapter、実 LLM なし）で固定した。daemon 側の修正は不要と確定した。

## 付記（2026-11-06）: 「daemon 側の修正は不要」の訂正
G7 の 1 回目（approval `n1-g7-20261106-1`）で、route 候補と skill があっても案件 CoS の chat run 5 本のうち 3 本の usage が null だった。
起動前の route 失敗の行が費用不明に数えられて案件を止める経路と、試行の終わりで CoS run の usage が切れる経路が daemon と driver にある。
上の D1 と付記の「daemon 側の修正は不要と確定した」は誤りで、[ADR 2026-11-06-cos-run-launch-accounting-and-drain](2026-11-06-cos-run-launch-accounting-and-drain.md) D0 で訂正した。D2・D3（route 候補が空の原因と driver の検査）は維持する。
