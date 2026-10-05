---
task: multi-objective-routing
unit: p2-dispatch-enforce
phase: 2
status: done-in-branch
date: 2026-10-05
---

# p2-dispatch-enforce: task-dispatch の state 合成・候補 allowlist・quota は defer（lane 降格なし）

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §2・§5・§7 と Phase 2 の試験契約
`routing_enforce_quota_defers_without_lane_downgrade` を task-dispatch に実装した。

## 実装内容

- `crates/task-dispatch/src/dispatcher/routing_enforce.rs`（新規）
  - `source_state_from_account`: account 帳簿（`AccountState`）の 5h / 7d 窓（`window_remaining`）・
    総合残量（`measured_remaining`）・reset 時刻・cooldown・`rejected` の reset（→ `rate_limited_until`）・in-use から
    `SourceState` を組む。帳簿に無い量（latency・rpm/tpm・queue・GPU）は None。古い観測の残量も None（0 にしない）。
  - `source_state_for_provider`: 非 pool の行。provider の cooldown 終了時刻・in-use・self-host load の
    取り込み口（`SelfHostLoad`、`Dispatcher::set_self_host_load`。揮発で保存しない）。
  - `DispatchRoutingSettings { mode, constraints, freshness }`（既定 legacy）と `Dispatcher::set_dispatch_routing`。
    設定ファイルからの配線は後段の config WU。
  - `constraint_exclusions`: task/組織固有の制約がある時、proxy 経由（`llm_source` を持つ）の行は
    `context_transport_unsupported`、ほかに deployment/source/privacy/locality/cost/latency。未知は通さない。
  - `enforce_quota_verdict`: `select_tier` が lane を保てない残量なら Err（lane を下げない）。
  - trace は `RoutingTraceV1{mode: enforce}`。候補は設定順で全て残し、除外は `excluded_reasons`（コード）と
    `excluded_reason`（型。quota → `QuotaExhausted`、proxy に渡せない制約 → `Constraint{name: context_transport}`）。
    `source_id`・`model`・`account_id`・`observed_at`・`fallback_order`（除外されなかった候補）を埋める。
- `provider_select.rs`: `legacy_provider_profiles`（設定行 → profile と `proxy_routed`）を切り出し、
  `select_provider_excluding`（除外集合つき）を追加。`select_provider_for` は空集合で委譲するので legacy・
  cheap-local-first の挙動は不変。除外集合は tick 共有の満杯集合 `full` に混ぜない。
- `dispatch_run.rs`: mode=enforce の時だけ、allowlist → 選択 → `SourceState` → `exclusion_reasons`（cost.rs）+
  quota verdict のループ。外れた source を除外集合に入れて同じ lane で選び直し、無ければ `Ok(false)`（defer、
  tracing に除外理由の要約）。legacy は従来の `select_tier` のまま。

## 証拠

| 条件 | コマンド | 結果 |
| --- | --- | --- |
| 新規試験 | `cargo nextest run -p task-dispatch routing_enforce` | 6 passed（統合 2・単体 4） |
| task-dispatch 既存試験（cheap_local_first 等） | `cargo nextest run -p task-dispatch` | 685 passed, 0 failed |
| 試験コードの clippy | `cargo clippy -p task-dispatch --all-targets -- -D warnings` | exit 0 |
| workspace 全体 | `bash scripts/dev/test-parallel.sh` | exit 0、Summary「3947 tests run: 3947 passed (1 slow), 12 skipped」 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |

`routing_enforce_quota_defers_without_lane_downgrade` は残量 20%（5h/7d）の frontier で:
legacy → p1 の `standard-id`（従来結果）、enforce + 別 source → p2 の `p2-frontier-id`（lane frontier、p1 は
`quota_low_for_lane`）、enforce + 別 source 無し → run を始めず Ready（defer）、残量十分 → p1 `frontier-id` を 2 回同じ結果。
`routing_enforce_keeps_exclusion_reasons_for_constrained_routes` は proxy 経由の p2 が trace に残り
`context_transport_unsupported` と `source` を持つことを固定。

## 未解決事項

- pool 内の別アカウントは「別 source」として個別に試さない（provider 単位で除外）。`pick_account` は既に
  最大スコアのアカウントを選ぶので、そのアカウントで lane を保てなければ provider ごと外す。
- defer は event を残さない（毎 tick の event 増殖を避けるため tracing のみ）。人に見せるなら api WU で
  routing audit から投影するか、変化した時だけ記録する event を検討する。
- `DispatchRoutingSettings` の設定ファイル配線と enforce の検証緩和（heuristic のみ）は config WU。

## 提案

- config WU は `[model_routing].mode = "enforce"` の時だけ `Dispatcher::set_dispatch_routing` を呼ぶ。
  reload でも同じ。
