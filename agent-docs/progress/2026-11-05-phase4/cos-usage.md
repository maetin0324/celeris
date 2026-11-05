---
task: 01M6PYNMP4EFRX9C5SGT6XB97E
status: done
completed: 2026-11-05
---
# 案件 CoS の chat run の usage が null（cos_usage_null 14 件）

結論: daemon の不具合ではない（fake harness の試験で案件 CoS の usage_json 記録と予算の和への算入を確認）。pilot が `[cos].route_candidates = []` で起動していたことが原因と見る（再実行の一時 DB が残っておらず、14 件の chat_runs の state・reason は未確認）。daemon は変えず、driver に空 route を拒む guard を足した。ADR は `agent-docs/adr/2026-11-05-phase4-cos-usage-empty-route.md`（実装より先に書いた）。

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
- 再現試験を足した: `crates/task-dispatch/src/dispatcher/cos_chat/harness_tests.rs` の `project_cos_chat_run_records_usage_and_counts_in_project_budget`。usage（input 100・output 20）を返す fake adapter で案件 CoS thread の chat run を回し、`ChatRun.usage`（chat_runs.usage_json）が null でないこと、`store.project_run_usage_between`（project_guard の予算の和の入力）が 1 件・input 100 を返すことを確かめる。実 LLM は使わない。
- command: `RUSTY_V8_ARCHIVE=/local/celeris/data/scratch/rusty-v8/v150.4.0/librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz cargo nextest run -p task-dispatch project_cos_chat_run_records_usage` → 1 passed（v8 の build が空の out dir で落ちたため `cargo clean -p v8` してから再実行した）。
- これで「案件 CoS の finish 経路は usage を記録し、予算の和に入る」は daemon の試験で確定した。pilot で null になったのは、harness が起動せず usage が出なかった run（空 route で起動できない run を含む）と整合する。daemon のコードは変えていない。

## 未解決
- 14 件の run の state・reason が未確認。operator が再実行時に chat_runs の state と reason を記録すれば D2 を確定できる。
