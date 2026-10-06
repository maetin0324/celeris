---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: daemon-wire
status: done
completed: 2026-10-05
---

# Phase 5 daemon-wire: `[model_routing.estimator.sidecar]` を llm-proxy の estimator shadow へ配線

## 変更

- `crates/celeris/src/daemon/routing_sidecar.rs` を新設。`EstimatorSidecarControl::apply` が設定から `EstimatorShadow`（proxy-shadow-est）を組み、proxy と共有する差し替え口に入れる。
  - 既定（`enabled = false`）では client も shadow も作らない（送信 0）。`enabled` かつ `shadow_only = true`（config 検証で必須）のときだけ組む。組めなければ warn を出して off（fail-closed）。
  - descriptor: `needs_prompt = send_prompt`、依存は local のみと宣言（`allow_external_dependencies = false`）。`prompt_allowlist` の対象外は client の gate で送る前に止まる（dropped/privacy・`prompt_required`）。宣言と違う依存を返した応答は `dependency_mismatch` で評価不能。
  - 日次上限は daemon 内の `SidecarDailyBudget`（UTC 日、reload を跨いで保持）。1 回 = 名目 `SIDECAR_NOMINAL_CALL_USD`・1 token で数える。
- `crates/llm-proxy/src/estimator_shadow.rs`: `EstimatorShadowSlot`（`RwLock<Option<Arc<EstimatorShadow>>>`）を追加。`server.rs`: `ProxyState` の estimator shadow を差し替え口にし、`with_estimator_shadow_slot` を追加（`with_estimator_shadow` は互換のまま）。
- 配線: `services.rs::build_llm_proxy_state`（`&mut Config` に変更）が control を作り、`config.model_routing.estimator_sidecar_control` に置く。`admin.rs::reload_providers` は新しい `model_routing` を入れた後に control を引き継いで `reload_estimator_sidecar` で再適用する（締める・off へ戻す・off から有効にするのも再起動なし。同じ設定なら作り直さず circuit を保つ）。
- 評価は proxy の要求の後に proxy 内で spawn（daemon の評価キュー）。CLI 直結 run も同じ proxy を通る。dispatcher の tick は sidecar を呼ばない（試験で確認）。
- `routing_shadow.rs`: estimator 用 sink の口 `task_shadow_sink` を追加（記録は既存の `routing_shadow_recorded`）。`SidecarEntry` に `PartialEq`。

## 証拠

- `cargo nextest run -p celeris --no-fail-fast -E 'test(routing_sidecar_wiring_defaults_off_and_reloads) | test(routing_sidecar_privacy_and_dependencies_gate_prompt) | test(routing_shadow_wiring_defaults_off_and_reloads)'` → exit 0、4 tests run: 4 passed
- `cargo nextest run -p celeris -p task-dispatch -p llm-proxy --no-fail-fast -E 'test(/cheap_local_first_/) | …'` → exit 0、13 passed
- `cargo clippy -p celeris --all-targets -- -D warnings && cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、4031 tests run: 4031 passed、12 skipped
- `cargo clippy --workspace -- -D warnings` → exit 0
- 範囲 check（scope）→ 範囲外 path なし

試験の内容: 偽上流・偽 sidecar とも 127.0.0.1:0 の in-process server。
- `routing_sidecar_wiring_defaults_off_and_reloads`: 既定で `build_llm_proxy_state` が client を作らない・送信 0 → reload で有効化し completed を記録 → 同じ設定の reload は保持、上限を 1 に締めると cap_exceeded で送信なし → off へ戻すと送信・記録とも増えない → tick は sidecar を呼ばない。
- `routing_sidecar_privacy_and_dependencies_gate_prompt`: send_prompt=false では本文に prompt なし／allowlist 外は送信 0・記録 0／prompt_allowlist 外は送る前に privacy で dropped／対象内は prompt を渡す／外部依存を申告する sidecar は failed・`dependency_mismatch`・候補なし。

## 未解決事項

- sidecar の日次上限は daemon 内で数える。handoff で新旧 instance が同時に動く日は最大 2 倍、再起動で当日の数が 0 に戻る。共有 DB の予約（migration 0049）は種類を区別せず数えるので、使うと estimator が実行 shadow の上限を食う。
- 試験の catalog は model の context 上限を宣言している（未宣言の legacy model は kernel が `context` で除外し、`no_eligible_candidate` で送信 0 になる）。本番で使うときは対象 model の `context_limits` を `[[model_routing.models]]` に書く必要がある。runbook に書くこと。

## 提案

- `routing_shadow_reservations` に種類（execution / estimator）の列を足し、estimator の上限も共有 DB で数える（task-core store の変更。別葉）。
