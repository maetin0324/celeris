---
task: multi-objective-routing
unit: p2-proxy-select
phase: 2
status: done-in-branch
date: 2026-10-05
---

# p2-proxy-select: llm-proxy の state 選択・制約・予約（競合は 1 回だけ再選択）

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` Phase 2 の proxy 側の選択を足した。
変更は `crates/llm-proxy/` の中だけ。mode=legacy の既存経路（`select_across_pools`・`rank_*`・`legacy_deployments`）と `server.rs` は変えていない（配線は integrate-runtime / config の後段）。

## 実装内容

- `src/selection_state.rs`（`selection::state` として公開。`selection.rs` に `#[path]` の 1 行）
  - `request_context(&ChatCompletionRequest)`: 要求の最小 context（input = 本文文字数/4 の切り上げ、output = `max_tokens`、tools・stream）。詳細搬送は Phase 3。
  - `select_state(input, candidates, now, occupancy)`: 手順は (1) 供給の除外（`cost::exclusion_reasons` の cooldown / rate_limit / quota_exhausted、予約表で枠が埋まった候補は `concurrency`）、(2) 残りに `cost::estimate_cost` を付けて kernel `optimize` へ（lane・privacy・capability（tools）・context floor・min_quality は kernel が score より前に適用）、(3) 落ちた候補を全部 `CandidateTrace` に残す（`excluded_reasons`・`excluded_reason`、score は持たない。費用成分は `with_cost`）。mode=legacy は Err。
  - OAuth account は `<deployment>@<account>` の候補に分ける（kernel は deployment id で候補を区別するため）。
  - `slots_for`: account 枠（`source_ref`+account）と共有 GPU 枠（`resource_group_id`）を別の枠として返す。
  - `snapshot_id`（候補 state の sha256）、`routing_trace`（stage `proxy` の `RoutingTraceV1`、最終 source/model/account）、`correlation`（log の相関欄 `RoutingCorrelation`）。
  - 時計は `now: OffsetDateTime` の引数で注入（I/O・時計の読み出しはしない）。
- `src/reservation.rs`（新規）
  - `ReservationTable`（in-process `Mutex`）: 全枠をまとめて取るか何も取らない。1 要求内の重複枠は除いてから数える（共有 GPU を二重計上しない）。`held`/`peak` で上限の確かめ。`Reservation` は drop で返す。
  - `reserve_with_reselect`: 競合したら `select(1, table)` で 1 回だけ選び直す。2 回目の競合・候補なしはそれ以上選び直さず Err。
  - `Clock`/`SystemClock`/`FixedClock`（時計の注入口）。
- `src/log.rs`: `insert_routed(conn, row, &RoutingCorrelation)` — 0048 の相関欄（decision・snapshot・run・task・source・model）を 1 文で書く。task events とは request id（`llm_proxy_requests.id`）と decision id で結ぶ。既存 `insert` は不変。
- `Cargo.toml`: dev-dependencies に `time`（macros、試験の固定時刻用）。

## 試験

- `routing_reservation_conflict_reselects_once`: `Barrier` で 2 要求に同じ候補 a（gpu0 上限 1）を選ばせてから予約を取り合わせる。結果は (a, 再選択 0) と (b, 再選択 1)、select 呼び出しは計 3 回、gpu0/gpu1 の peak は 1。候補が a だけなら負けた側は再選択で `NoCandidate{attempt:1}`（trace に `concurrency`、outcome `defer`）、呼び出し計 3 回。
- `routing_shared_gpu_capacity_not_double_counted`: account 枠は GPU 枠に数えない、同 group の別 deployment は同じ枠を共有、account+GPU の両方に載る要求は各 1（重複 gpu0 は 1 回だけ）、GPU 満杯は account を止めず account 満杯は GPU を止めない。
- `constraints_exclude_before_score_and_reasons_are_traced`: capability / cooldown / privacy / quality_below_min が score なしで trace に残り、同じ snapshot・時刻から同じ選択・同じ trace JSON。
- `reserve_is_all_or_nothing_and_released_on_drop`、`reselect_happens_at_most_once`、`insert_routed_writes_the_correlation_columns`、`request_context_takes_the_floor_from_request_fields`、`legacy_mode_does_not_use_state_selection`。

## 証拠

| コマンド | 結果 |
|---|---|
| `cargo test -p llm-proxy routing_reservation_conflict_reselects_once` | exit 0、1 passed（5 回連続で通過） |
| `cargo test -p llm-proxy routing_shared_gpu_capacity_not_double_counted` | exit 0、1 passed |
| `cargo test -p llm-proxy` | exit 0、lib 48 passed・結合 27 passed（既存試験含む） |
| `cargo clippy -p llm-proxy --all-targets -- -D warnings` | exit 0 |
| `bash scripts/dev/test-parallel.sh` | exit 0、Summary「3949 tests run: 3949 passed (1 slow), 12 skipped」 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| 範囲 check（merge-base HEAD celeris/01M44ZK8GADYD9PVAYBY73F6YC、crates は llm-proxy/task-dispatch のみ） | exit 0 |

## 未解決事項

- `server.rs` への配線（mode≠legacy で `select_state` → `reserve_with_reselect` → `insert_routed`）はしていない。proxy-fallback と同じ file を触るため integrate-runtime / config の後で行う。
- `CapacityLimits` の値（account 上限 = `max_concurrent_per_account`、resource group 上限）の config 配線は config unit。
- input_tokens の見積もり（文字数/4）は下限の目安。正確な context の搬送は Phase 3。

## 提案

- 配線時、予約は stream を含む上流応答の終わりまで保持し（`Reservation` を応答 body の drop に結ぶ）、fallback で次候補へ移るときは前の予約を drop してから取り直す。
