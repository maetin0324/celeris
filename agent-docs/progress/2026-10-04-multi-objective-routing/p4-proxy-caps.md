---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: proxy-caps
status: done
completed: 2026-10-05
---

# Phase 4 proxy-caps: llm-proxy の shadow 日次上限を共有 DB の予約へ接続

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §7.1・§7.2・§10 Phase 4 の llm-proxy 部分（日次上限）。

## 実装
- `crates/llm-proxy/src/shadow_budget.rs`（新規）: `StoreShadowBudget` が `ShadowBudget` を task-core の `SqliteStore::routing_shadow_reserve` / `routing_shadow_settle`（migration 0049 `routing_shadow_reservations`、BEGIN IMMEDIATE）で実装する。`ShadowPolicy::daily_caps()` が `None`（off・不完全）なら作れない。時計は注入（`reservation::Clock`）。DB の誤りは fail-closed（`dropped/cap_exceeded`、送らない）、確定できない予約は reserved のまま最悪消費で数え続ける。
- `crates/llm-proxy/src/shadow.rs`: `ShadowQueue::with_capacity(ReservationTable)`。shadow は開始時に候補の resource group の枠を `try_reserve`（待たない）で取り、primary が先に取って埋まっていれば日次予約の前に `dropped/concurrency_limit`（detail `resource_group_full:<group>`、予約・送信 0）。枠は実行が終わるまで持つ。差し込まない既定は従来どおり。
- 既存の pub struct（`RoutingRecord`・`ShadowJob` 等）に欄は足していない。daemon からの配線（store・ReservationTable の受け渡し）は daemon-wire unit。

## 試験
- `routing_shadow_opt_in_caps_survive_restart_and_handoff`（`crates/llm-proxy/tests/proxy_shadow.rs`）: 一時 DB・偽上流（127.0.0.1:0）・偽時計で実 HTTP を叩く。
  - off（既定 policy）と allowlist 外: 候補上流への chat 0・予約行 0・記録 0
  - 未知の費用: `dropped/unknown_cost`、送信 0・行 0
  - request 上限 2: 3 件目 `cap_exceeded`。store と proxy を作り直した再起動後も同じ UTC 日は送らない
  - UTC 日界: `2026-10-06 08:59 +09:00` はまだ 10-05 で拒否、`2026-10-06 00:00:01Z` で新しい日として送る
  - token 上限 70（最悪 33 で予約、完了で実測 2 に確定）: 19 件送り 20 件目で停止
  - handoff: 同じ DB を別に開いた 2 instance へ 8 件同時。effective 上限 0.35（1 件 0.1）で合計 3 件だけ送信、5 件 `cap_exceeded`。request/token/effective の和は上限内、予約行数 = 送信数
- `routing_shadow_uses_only_capacity_left_by_primary`（`shadow_tests.rs`、paused clock）: primary が枠を持つ間は shadow が `concurrency_limit` で落ち（executor 呼出し 0・予約 0）、返った後は残り枠で完了、peak 1。
- task-core 側の同名試験（shadow-model unit 作）も通ることを確認。

## 証拠
- `cargo test -p llm-proxy shadow` → exit 0（unit 4 passed、proxy_shadow 3 passed）
- `cargo test -q -p llm-proxy --test proxy_shadow routing_shadow_opt_in` を 5 回 → 5 回とも 1 passed（1.75〜2.37 s）
- `cargo test -q -p task-core routing_shadow_opt_in_caps_survive_restart_and_handoff` → 1 passed
- `bash scripts/dev/test-parallel.sh` → exit 0、nextest 4009 tests run: 4009 passed, 12 skipped
- `cargo clippy --workspace -- -D warnings` → exit 0

## 未解決事項
- primary の経路（server.rs）はまだ `ReservationTable` を持たない（Phase 2 の selection_state だけが使う）。primary と shadow で同じ表を共有する配線と、`ShadowJob.candidate_resource_group` の設定は daemon-wire unit で行う。

## 提案
- なし
