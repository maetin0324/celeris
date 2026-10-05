---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: proxy-shadow-est
status: done
completed: 2026-10-05
---

# Phase 5 proxy-shadow-est: llm-proxy の estimator shadow

## 変更

- `crates/llm-proxy/src/estimator_shadow.rs` を新設。`EstimatorShadow::submit` は同期で返り、sidecar 呼び出しは `tokio::spawn` の中（primary は完了を待たない）。返った `SidecarEstimateSnapshot` を `QualityEstimator` として shadow kernel（`kernel_choice` = task-core `optimize` を `Shadow` mode で回す）に渡し、選んだ候補を primary と比べる。
- 記録は Phase 4 の `routing_shadow_recorded`（ADR §6 の `routing_shadow_evaluated`）を再利用し `kind = estimator`。`policy_version = estimator:<id>/<version>`、status completed / failed / dropped、reason、`detail`（completed は `<primary との比較>;heuristic:<heuristic 選択との比較>`、失敗は sidecar の理由語 `timeout` / `prompt_required` / `circuit_open` など）、`latency_ms` = 推論 overhead（sidecar 往復）、`reservation_id`。
- 決定権なし: `shadow_only = false` は `EstimatorShadow::new` が `ShadowOnlyRequired` で拒否。hard constraint・privacy の除外は estimate より前に当たり、sidecar には除外後の model id だけを送る（除外候補の id も出さない）。
- 上限: `ShadowPolicy::admit`（allowlist・標本化・日次上限が揃った opt-in のみ）→ `resource_group` の枠を共有 `ReservationTable` から待たずに取る（埋まっていれば `concurrency_limit` で dropped、予約・送信 0）→ `ShadowBudget::reserve`（最悪費用 `worst_call_effective_usd` が未知なら `unknown_cost` で dropped）。完了時は計測できない推論費用を予約した最悪値で確定（ゼロにしない）、失敗は `Failed`、timeout は `TimedOut` で確定。
- 失敗の写像: timeout → failed/timeout、circuit_open・max_inflight → dropped/concurrency_limit、prompt_required・dependencies_not_allowed → dropped/privacy、payload_too_large → dropped/cap_exceeded、その他 → failed/upstream_error。
- `server.rs`: `ProxyState::with_estimator_shadow` を追加（既定 None）。primary 成功後の `shadow_after_primary` から `estimator_after_primary` を呼ぶ。明示 model の要求（lane = explicit）は対象外。prompt は `prompt_allowlist`（既定空）に一致したときだけ最後の user message を渡す（client の `send_prompt=false` でさらに落ちる）。`ProxyShadow` の pub 欄は変えていない（daemon 側は無変更）。

## 証拠

- `cargo nextest run -p llm-proxy -E 'test(routing_sidecar_cannot_override_constraints_or_primary) | test(routing_decision_shadow_never_calls_upstream_or_changes_primary) | test(routing_shadow_opt_in_caps_survive_restart_and_handoff) | test(routing_shadow_backpressure_timeout_and_no_tool_execution)'` → exit 0、4 tests run: 4 passed。
- `cargo nextest run -p llm-proxy` → 96 passed（新規 `tests/estimator_shadow.rs` 2 件 + unit 1 件を含む）。
- `bash scripts/dev/test-parallel.sh` → 4027 tests run: 4027 passed、12 skipped、doctest ok、`test-parallel: ok`。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。

試験 `routing_sidecar_cannot_override_constraints_or_primary`（偽 sidecar・偽上流とも 127.0.0.1 の in-process）: (1) `shadow_only=false` 拒否、(2) privacy で除外した model-x を sidecar が 1.0 と評価しても kernel は model-b を選ぶ、(3) sidecar の応答を止めたまま primary が返り legacy と同じ、完了後の要求も同じ primary、sidecar には model-a/b だけが届き prompt は送られない、記録は completed・`estimator:route-test/1`・overhead・予約あり、確定費用は最悪値 0.002（ゼロでない）、sidecar の資源枠は往復中だけ 1。

## 未解決事項

- daemon 配線（`[model_routing.estimator.sidecar]` → `EstimatorShadowConfig`・catalog・`StoreShadowBudget`）は daemon-wire 葉。config の `SidecarEntry` には sample_rate・tokens/effective の日次上限・`worst_call_effective_usd` が無く、daemon-wire で `ShadowPolicy` を組むときに既定値か config 追加が要る。
- legacy catalog の model は context 上限が unknown のため、要求に tokens があると kernel が `context` で全候補を除外し `no_eligible_candidate`（dropped/not_allowlisted）になる。実運用の比較には profile の context 上限が要る。

## 提案

- `ShadowRecord` に estimator_id/version を別欄で持たせるかは audit-api 葉で判断（現状は `policy_version` に `estimator:<id>/<version>` で入れている）。
