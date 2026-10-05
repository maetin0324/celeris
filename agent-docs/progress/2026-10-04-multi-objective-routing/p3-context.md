---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: context
status: done-in-branch
completed: 2026-10-05
---

# p3-context: task-core の拡張可能な RoutingContext と context_ref registry

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §3.4・§5・§6・§10 Phase 3 のうち、
この WorkUnit（`context`）が持つ範囲（`task-core::model_router::context` の欄拡張と
`RoutingContextRegistry` interface）を実装した。escalation（軌跡 escalation）と events（feature/
outcome schema）は並行する別 WU（`escalation`・`events`）の範囲で、ここでは触れていない。

## 実装内容

- `crates/task-core/src/model_router/context.rs`: `RoutingContext` に ADR §3.4 の欄を足した
  （`org_node`・`role`・`harness`・`task_kind`・`phase`（`RoutingPhase`、未知値は `#[serde(other)]` で
  `Unknown` に decode）・`acceptance_criteria`（criterion ID 列）・`required_tool_ids`・
  `environment`（`RoutingEnvironment { locality, host, external_network }`）・`attempts`・
  `review_failures`・`check_failures`・`priority`・`field_provenance`（欄ごとの出自）・
  `missing_fields`）。本文・credential・prompt は入れていない。
  `#[derive(Default)]` + 構造体に `#[serde(default)]` を付け、`deny_unknown_fields` は付けない
  （未知の欄は無視して decode する。将来の Phase が足す欄をここで決め打ちしない）。
  既存の `required_tools: bool`・`input_tokens`・`output_reserve`・`safety_margin`・`provenance`
  の型は変えていない（既存の呼び出し元を壊さない）。
  `pub fn standalone_context(provenance: &str) -> RoutingContext` を足した（origin="standalone"、
  task 由来の欄は全部 `None`/空で、その欄名を `missing_fields` に列挙する）。
- `crates/task-core/src/model_router/context_registry.rs`（新規）: `RoutingContextRegistry` trait
  （`register(run_id, ctx, ttl, now) -> context_ref`、`resolve(ref, now) -> Result<ctx, Invalid|Expired>`、
  `release(run_id)`、`prune(now)`）と、時計を注入できる in-memory 実装
  `InMemoryRoutingContextRegistry`（`Mutex<HashMap<...>>`、opaque な ref は `ulid::Ulid` で生成）。
  `release` は run の in-flight 終了時に呼び、その run に結んだ ref を即時 Invalid にする（ttl を
  待たない）。ttl 超過は `Expired` として `Invalid`（未登録・release 済み・prune 済み）と区別する。
- 既存の `RoutingContext { .. }` struct literal 2 か所（`crates/llm-proxy/src/selection_state.rs`
  の `request_context`、`crates/task-core/src/model_router/tests.rs` の `context()`）に
  `..RoutingContext::default()` を足した。`grep -rn "RoutingContext {" --include=*.rs crates` で
  他に literal が無いことを確認済み（workspace 全体を検査）。

## ADR からの逸脱点（1 件、要相談）

ADR §10 Phase 3 の試験表は `register(run_id, ctx, ttl) -> ref` と書いているが、`now` を渡していない。
時計を読まず決定的に試験するには `register` にも `now` を渡す必要があるため、
`register(run_id, ctx, ttl, now)` にした（`resolve` と同様に呼び出し側が時刻を注入する）。
close WU での ADR 付記時にこの署名を正本として書くか、別の形（registry 自体に `Clock` を注入し
`register`/`resolve` は `now` を取らない）に変えるか、人の判断を仰ぎたい。今回は後方互換を壊す
変更ではなく（trait は Phase 3 で新規）、どちらでも Phase 3 の他 WU（dispatch-context・
proxy-context・worker-transport）には影響しない程度の差だと考えている。

## 検査の結果

| 検査 | コマンド | 結果 |
| --- | --- | --- |
| 新規試験（単体） | `cargo test -p task-core model_router` | exit 0、12 tests passed（新規 3 件: `routing_context_ignores_unknown_fields`・`routing_context_decodes_phase2_json`・`routing_context_registry_expires_and_rejects_unknown_refs`） |
| llm-proxy 全試験 | `cargo test -p llm-proxy` | exit 0、53+1+27 tests passed。cheap Qwen 優先 (`cheap_only_celeris_cheap_prefers_the_reachable_relay_when_prefer_free`)・stream 前 fallback (`routing_proxy_fallback_preserves_constraints_and_stream_boundary`) を含む |
| workspace 全試験 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest Summary 原文「`Summary [  77.135s] 3971 tests run: 3971 passed (1 slow), 12 skipped`」。doctest 0 failed |
| clippy（workspace） | `cargo clippy --workspace -- -D warnings` | exit 0 |
| clippy（試験コードを含む、変更した 2 crate） | `cargo clippy -p task-core -p llm-proxy --all-targets -- -D warnings` | exit 0 |
| 整形 | `cargo fmt --all -- --check` | exit 0 |
| 範囲 | `git diff --stat $(git merge-base HEAD celeris/01M4577C9412HCDQEV1AFTT69C)` | `crates/llm-proxy/src/selection_state.rs`・`crates/task-core/src/model_router/{context,mod,tests}.rs` の変更と `context_registry.rs` の新規のみ |

## 受け入れ条件（この WorkUnit）への対応

0. `RoutingContext` が ADR §3.4 の欄を持つ。旧（Phase 2）JSON の decode は
   `routing_context_decodes_phase2_json` で固定（新欄は `None`/空になり、0/false で埋めない）。
1. registry の失効・未知参照・in-flight 保持は `routing_context_registry_expires_and_rejects_unknown_refs`
   （偽時計、`Instant` を明示的に進める）で固定。未知の ref と ttl 超過の ref を区別し、
   release 済みの ref は別 run の ref に影響しない。
2. cheap の Qwen 優先と stream 前 fallback の既存 llm-proxy 試験は上の表のとおり全部通る
   （`RoutingContext` の欄追加は construction を `..Default::default()` にしたことで型として
   そのまま吸収された）。

## 未解決事項・提案

- 上記「ADR からの逸脱点」の `register` 署名（`now` を引数に取る）を人に確認してほしい。
- `RoutingContextRegistry` はまだどこにも配線していない（この WU の範囲外）。`dispatch-context`
  WU が daemon 側で run 登録に使い、`proxy-context` WU が `resolve` を呼ぶ想定（ADR §10 Phase 3）。
- `field_provenance`・`missing_fields` は型だけ用意した。実際にどの欄に何の出自文字列を入れるかは
  `dispatch-context`（task metadata からの抽出元）側の決定が必要。
- `priority` の型を `i32` にしたが、ADR は型を明記していない。スケジューラ側の実際の値域が決まれば
  見直してよい（破壊的変更にならないよう Option のまま）。
