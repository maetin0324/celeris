---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: proxy-client
status: done
completed: 2026-10-05
---

# Phase 5 proxy-client: llm-proxy の非同期 estimator sidecar client

## 変更

- `crates/llm-proxy/src/estimator_sidecar.rs` を新設。`SidecarEstimatorClient::estimate(EstimateRequestV1)` が既存の reqwest で `POST {endpoint}/estimate` を呼び、task-core の `SidecarEstimateSnapshot::from_response`（検証込み）で snapshot を返す。最後の成功は `cached_snapshot()` に残す。
- 上限: `timeout`（tokio::time::timeout）、`max_inflight`（Semaphore の try_acquire。待たずに評価不能）、`max_payload_bytes`（request は送る前、response は Content-Length と chunk 累積で打ち切り）。
- circuit: 連続失敗 `circuit_failure_threshold` 回で `circuit_open_for` の間開く。過ぎたら 1 件試し、成功で閉じ失敗で再び開く。時計は `MonotonicClock`（既定 `TokioClock` = tokio Instant）で注入。
- privacy / 依存 gate（送信 0）: `send_prompt=false` なら `optional_prompt` を落とす。descriptor が `needs_prompt` で prompt を送れなければ `prompt_required`。descriptor が network / 外部 embeddings 依存を申告し `allow_external_dependencies=false` なら `dependencies_not_allowed`。応答の依存申告が descriptor と違えば `dependency_mismatch`。
- 失敗はすべて `SidecarUnavailable`（`reason()` は circuit_open / max_inflight / timeout / transport / http_status / payload_too_large / decode / version_mismatch / prompt_required / dependencies_not_allowed / dependency_mismatch / invalid_estimate）。heuristic 側は変えていない。
- reqwest client は `no_proxy()`・redirect 無効で、送信先は endpoint だけ。

## 証拠

- `cargo test -p llm-proxy --test estimator_sidecar` → 2 passed（routing_sidecar_protocol_validates_identity_range_and_size / routing_sidecar_privacy_and_dependencies_gate_prompt。偽 sidecar は 127.0.0.1:0 の in-process axum）。
- `cargo nextest run -p llm-proxy` → 93 passed（Qwen 優先・fallback breaker・pool 順位の既存試験を含む）。
- `bash scripts/dev/test-parallel.sh` → exit 0、4024 passed、12 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p llm-proxy --all-targets -- -D warnings` → exit 0。

## 未解決事項

- celeris config（`SidecarEntry`）→ `SidecarClientConfig` の写像（`estimator_version`、descriptor の needs_prompt/dependencies の出所、`network_allowlist` と `allow_external_dependencies` の関係、circuit 値）は daemon-wire unit で決める。
- 試験の timeout は実時間 300ms（Hang mode は応答しないので結果は決定的）。circuit の時間経過は `tokio::time::pause/advance` で進める。
