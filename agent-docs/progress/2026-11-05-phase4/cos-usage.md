---
task: 01M6PYNMP4EFRX9C5SGT6XB97E
status: done
completed: 2026-11-05
---
# 案件 CoS の chat run の usage が null（cos_usage_null 14 件）

結論: daemon の不具合ではなく、pilot が `[cos].route_candidates = []` で起動していた可能性が高い（推定。下の未解決を参照）。daemon は変えず、driver に空 route を拒む guard を足した。ADR は `agent-docs/adr/2026-11-05-phase4-cos-usage-empty-route.md`（実装より先に書いた）。

## 原因の根拠
- usage の記録経路は案件 CoS と統括 CoS で共通: `crates/task-dispatch/src/dispatcher/cos_chat/rollover.rs` の `run_attempts` → `record_attempt`（`sink.record_usage(outcome_usage(..))`）→ `sink.rs` の `ChatRunSink::finish` が `chat_run_finish_with_telemetry(.., state.usage.as_ref(), ..)` を呼ぶ。
- harness を起動できなかった run は `routes.rs` の `launch_routes`（"CoS route unavailable"）→ `fail_routes` → `chat_run_finish`（usage なし）で終わり、usage_json は null のまま。
- `scripts/dev/long-horizon-pilot/driver.py` の実 LLM 側は `route = a.cos_route`（`--cos-route` 既定 `"[]"`）を config の `[cos] route_candidates` に書く。空だと CoS の route を選べない。
- 本番の 413 件が全件 usage 入りなのは、本番 config に CoS の route があるため（整合する。未確認）。
- `driver.py` の `collect_usage` は started_at のある全 chat_runs を数えるため、起動前に失敗した run も `cos_usage_null` になる。

## 変更
- commit 件名: 「phase4 cos-usage: 実 LLM pilot の --cos-route が空なら起動を止め、再実行の raw と ADR・進捗を足す」
- `driver.py`: 空の `--cos-route` を `SystemExit` で拒む。daemon・project_guard は変更なし。

## operator の次の手
`--cos-route '[{ source = "claude_oauth", model = "claude-sonnet-5-5" }]'` を付けて再実行する。成功の目安は facts の usage.missing に `cos_usage_null` が出ないこと。

## raw の要点（llm-rerun/ に cp -a、diff -r exit 0）
- C06: worker+reviewer の input 40・output 5,465・cache_read 356,951・cache_creation 113,796、wall 122 秒。CoS 6 件は usage null。
- C08: input 54・output 8,210・cache_read 502,106・cache_creation 131,579、wall 165 秒。CoS 8 件は usage null。
- 2 試行とも verdict pass・achieved_oracle_mismatch 0・double_effect 0。
- 実行 log: /local/celeris/data/scratch/p0f-pilot/llm2-run.log（再実行の一時 DB は残っていない）。

## 試験
- fake harness で usage を出す案件 CoS run の再現試験は、この run の turn 予算内で書けていない（未実施）。原因が config 側のため daemon のコードは変えていない。project_guard の和は `project_run_usage_between` が chat_runs.usage_json を含むことをコードで確認しただけ。

## 未解決
- 14 件の run の state・reason が未確認。operator が再実行時に chat_runs の state と reason を記録すれば D2 を確定できる。
- 上の再現試験（fake harness → usage_json 非 null → project_guard の和）は後続で足す余地がある。
