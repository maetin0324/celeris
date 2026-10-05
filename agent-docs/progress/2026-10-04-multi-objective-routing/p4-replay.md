---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: replay
status: done
completed: 2026-10-05
---

# Phase 4 replay: 過去の routing event の読み取り専用評価

`task_ops::routing_replay` は SQLite の `SQLITE_OPEN_READ_ONLY` で events を task 単位に抽出する。schema v1 の JSONL と manifest に policy/catalog/estimator hash、期間、抽出条件、masking、seed、task ID の SHA-256 による train/calibration/test、欠測率を記録する。event の JSON 本文は出さず、routing の監査欄だけを allowlist で写す。retry と review は同一 task の split に属する。

`evaluate` は frozen dataset の候補 trace と shadow 記録から legacy / heuristic / shadow_recorded の判断を比較し、report v1 に合否、失敗 criterion、cash/effective cost（実測と候補の推定を別欄）、API と task の時間、retry/escalation、quota、制約違反、source 比率、unknown/timeout/drop、counterfactual coverage を分けて記録する。未選択モデルに task の合否を代入しない。APGR/AIQ/IBC は同一 task の weak/strong/routed モデルに観測済みの合否・cost がある場合だけ task-core の純粋関数を呼び、通常は未定義理由を出す。任意の RouterBench 型 baseline は `external_benchmark_baseline` に分離する。

## 証拠

- `cargo test -p task-ops routing_offline_replay_is_deterministic_and_split_by_task -- --nocapture` → 1 passed。一時 DB で同一 task の retry を同じ split にし、JSONL/report のバイト一致、秘密を含む task 本文の不出力、DB ファイルのバイト不変を確認。
- `cargo test -p task-ops -q` → 503 passed、1 ignored（手動計測）。paired 指標は同じ task の両モデルに観測済み合否・cost がある場合だけ算出する試験を含む。
- `cargo clippy -p task-ops -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 境界

API latency は相関の取れた `llm_proxy_requests.latency_ms` のみで集計し、task 完了時間や shadow 呼出し時間と混ぜない。相関のない過去イベントは欠測として残す。候補の score は当時の trace を再評価する frozen snapshot であり、未保存の catalog や prompt を復元しない。期間フィルタは RFC3339 を時刻として比較する。
