---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: daemon-wire
status: done
completed: 2026-10-05
---

# Phase 4 daemon-wire: shadow 設定を dispatcher と llm-proxy の shadow queue・予約へ配線

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §7.1・§7.2・§10 Phase 4 の daemon 配線分。

## 実装
- `crates/task-dispatch/src/dispatcher/routing_shadow.rs`: `RoutingShadowListener` trait と
  `Dispatcher::set_routing_shadow` / `routing_shadow_policy` / `add_routing_shadow_listener`。dispatcher が検証済み
  `ShadowPolicy` を持ち、差し替えのたびに listener へ「同じ時点の mode と policy の組」を 1 回で渡す。
  decision shadow 自体は従来どおり `mode = shadow` で動く（dispatch-shadow unit）。既存 pub struct に欄は足していない
  （`Dispatcher` の private 欄 2 つのみ）。
- `crates/celeris/src/daemon/bootstrap.rs` / `admin.rs`: 起動と reload で `set_dispatch_routing` の直後に
  `set_routing_shadow(runtime.shadow)`。`Config::load` を通らない設定は従来どおり旧 snapshot のまま。
- `crates/celeris/src/daemon/routing_shadow.rs`（新規）: `install_proxy_shadow`。
  - 既定（legacy・execute = false）: proxy に shadow を差し込まない。queue・予約 store を作らず、候補列も作らない（追加送信 0）。
  - `mode = shadow`: proxy の decision shadow（`PreferSelfHosted`、追加呼出しなし）。
  - `execute = true` で上限が揃う: `ShadowQueue` + `StoreShadowBudget`（daemon の DB を別接続で開き migration 0049 の予約を数える）。
    handoff 先の新 instance も同じ DB の予約を読むので合計で上限を越えない。
  - `ProxyShadowControl`（listener）: reload で decision 記録の開閉と `ShadowQueue::reconfigure`。
  - 記録は `TaskShadowSink` が `routing_shadow_recorded` として追記（run の所属を runs 索引で照合できたものだけ。decision は mode = shadow の間だけ）。
  - 最悪費用は catalog の deployment（`source_ref`・`upstream_model` 一致）の単価から。単価が揃わなければ未知 → `unknown_cost`。
- `crates/llm-proxy/src/shadow.rs`: `ShadowQueue` の policy・上限・予約先を `RwLock<Arc<QueueConfig>>` にまとめ、
  `reconfigure(policy, Option<budget>)` で 1 回の書き込みで差し替える。実行中の分は開始時の予約先で確定。
  off へ差し替えると以後 `NotAdmitted(Off)`、待ち中の未開始分は `dropped/off`（detail `reconfigured_off`）。
- `services.rs`: `build_llm_proxy_state` が `&mut Dispatcher` を取り、上の配線を `ProxyState::with_shadow` に入れる。

## 試験
`routing_shadow_wiring_defaults_off_and_reloads`（`crates/celeris/src/daemon/routing_shadow_tests.rs`、一時 DB・127.0.0.1 の偽上流・固定時計）:
1. 既定: shadow 未差し込み、queue なし、decision off。reload で有効化 → dispatcher は新 policy・mode shadow、proxy は再起動まで送らない。上流 chat 0。
2. instance A（上限 2）: 1 件送信。reload で off → 同じ queue が `NotAdmitted(Off)`。reload で上限 1 → policy が差し替わり、DB の今日の 1 件で満杯（送信増えず）。
3. handoff: 同じ DB の instance B（上限 2）は A の予約を読み継ぎ、b1 だけ送り b2 は止まる（合計 2）。

## 証拠
- `cargo nextest run -p celeris -E 'test(routing_shadow_wiring_defaults_off_and_reloads) | test(routing_shadow_config_defaults_off_and_requires_caps)'` → 2 passed
- `cargo nextest run --no-fail-fast -p celeris -p llm-proxy -p task-dispatch` → 1184 passed（instance_handoff・config 試験を含む）
- `bash scripts/dev/test-parallel.sh` → exit 0、4015 passed, 12 skipped, test-parallel: ok
- `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- 範囲 check（`CELERIS_WU_BASE` 基準）→ 範囲外 0 件

## 未解決事項
- 既定 off で起動した proxy に reload で shadow を新しく足すことはしない（`ProxyState` は起動時に 1 度だけ組む。
  `[model_routing.retry]` と同じく再起動で効く。warn を出す）。reload で効くのは dispatcher の decision shadow、
  起動時に作った queue の締め・緩め・off、proxy の decision 記録の開閉。
- primary と shadow の実行枠表（`ShadowQueue::with_capacity`）の共有と `candidate_resource_group` は未配線
  （server.rs が primary の `ReservationTable` を外へ出していない。proxy-caps の未解決と同じ）。
- `shadow_candidate_policy` の名前は proxy では使っていない（比較 policy は `PreferSelfHosted` 固定）。

## 提案
- ops-doc 葉の手順に「proxy 側の shadow の新規有効化は再起動が要る、締める・止めるは reload で効く」を書く。
