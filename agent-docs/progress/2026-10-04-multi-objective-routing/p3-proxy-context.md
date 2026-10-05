---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: proxy-context
status: done-in-branch
completed: 2026-10-05
---

# p3-proxy-context: llm-proxy の x-celeris-routing-context の参照解決・偽装拒否・header 除去

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §5・§6・§10 Phase 3 のうち、
llm-proxy の担当分を実装した。

## 実装

- `crates/llm-proxy/src/routing_context.rs`（新設）
  - `ROUTING_CONTEXT_HEADER = "x-celeris-routing-context"`。
  - `resolve(headers, registry, req, now)`: header 無し → `standalone_context("llm-proxy:standalone")`
    （origin=standalone）。header あり → `RoutingContextRegistry::resolve`。`Invalid` は
    `invalid_routing_context`、`Expired` は `expired_routing_context`。registry を持たない proxy に
    header が来たら `Invalid`（fail-closed）。
  - 本文から足すのは測れる量だけ（入力長 chars/4、`max_tokens`、tools の有無、stream）で、登録値より
    厳しい側（大きい token 数・`true`）にしか動かさない。変えた欄は `field_provenance` に
    `llm-proxy:request-fields`。本文の org/priority/privacy/run_id 等は `ChatCompletionRequest` が持たない
    ので読まれず、上流にも出ない。
  - `ProxyEventSink` trait（`parent_decision(run_id) -> Option<String>` 既定 None、`record(ProxyRoutingEvent)`）。
    `ProxyRoutingEvent { task_id, features: RoutingFeaturesRecord(stage=proxy), request: RoutingRequestRecord }`。
    daemon 登録の context で `task_id` があるときだけ組む（standalone は proxy log のみ）。
  - `InFlightContexts` / `InFlightGuard`: run ごとの in-flight 要求数。guard は要求の記録（非 stream は応答時、
    stream は終端時）まで生き、drop で戻る。`ProxyState::in_flight_contexts().count(run_id)`。
- `crates/llm-proxy/src/server.rs`
  - `ProxyState::with_routing_context(registry, sink)`（`with_fallback` と同じ builder。既定は両方 None）。
  - `chat_completions` が `HeaderMap` を受け、最初に context を解決。失敗は 400 と log 行（status error、
    error_kind = 上の code）で返し、上流に送らない。
  - `LogHandle` を `RequestScope`（id・時刻・`ResolvedRouting`・試した source 列・sink・in-flight guard）
    から作る形にまとめ、終わり方ごとに 1 回だけ書く。daemon 登録の要求は `log::insert_routed` で 0048 の
    相関欄（decision_id = 親 decision、run_id、task_id、source_id、model）を書き、standalone は従来の
    `log::insert`。同じ時点で sink に event を渡す。
  - 要求ごとの proxy decision_id は `pdec_<ulid>`。試した source は `RequestSourceAttempt`（失敗して次へ
    倒したものは `fallback_reason` = `auth_failed`/`rate_limit`/`health_down`/`local`/`client`、最後の
    source は None）。
  - header は上流に転送しない（上流への要求は `ChatCompletionRequest` から組み直すので構造的に出ない）。
- `crates/llm-proxy/tests/proxy_routing_context.rs`（新設）
  - `routing_context_ref_propagates_and_rejects_spoofing`: 偽 adapter（reqwest）→ proxy → 偽 relay 上流。
    有効 ref で 200、上流の全 header に `x-celeris*`・ref・run ID が無く、本文に org/priority/privacy/run_id/
    metadata が無い。sink の features は登録値（org_node・priority=1・required_tools=true・input_tokens=120000）
    で本文の自己申告（cos/999）や短い本文で緩まない。parent decision・run_id・request_id が結合し、log の
    0048 欄に run/task/decision が残る。偽造 ref・期限切れ ref・release 済み ref は 400 で上流 0 回・sink 0 件。
    header 無しは standalone で 200、sink に渡さず log の相関欄は NULL。要求後の in-flight 数は 0。
  - `routing_context_header_without_a_registry_is_rejected`: registry 無しの proxy は header 付きを 400。

## 証拠

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -p llm-proxy` | 83 passed（新規 2 本を含む） |
| `cargo nextest run -p llm-proxy routing_context cheap_only claude_429` | 12 passed（cheap の Qwen 優先・fallback・OAuth 429 cooldown の回帰を含む） |
| `cargo clippy -p llm-proxy --all-targets -- -D warnings` | exit 0 |
| `bash scripts/dev/test-parallel.sh` | `test-parallel: ok`、3978 passed / 12 skipped |
| `cargo clippy --workspace -- -D warnings` | exit 0 |

## 未解決

- daemon 側の配線（`with_routing_context` に registry と sink を渡す、run 終了時に
  `in_flight_contexts().count(run_id) == 0` を待って `release`、sink の冪等な task event 追記と
  `parent_decision` の DB 参照）は daemon-wire leaf。配線前は registry が None なので、header 付きの要求は
  400 になる（worker-transport が header を送り始めるのと daemon-wire は同じ Phase の中で揃う必要がある）。
- 偽装拒否は「本文の欄を採らない」ことで担保している。legacy の選択経路は context を候補の制約にまだ使って
  いない（enforce の proxy 選択への context の適用、unsupported 経路の制約付き enforce 拒否は別 leaf）。
- proxy trace（`RoutingRequestRecord.trace`）は legacy 経路では None。

## 提案

- close leaf の ADR 付記に上の実装（400 の code 2 種、registry 無しは fail-closed、本文からは厳しい側だけ、
  in-flight 数の API、log の decision_id は run 側 decision）を書き写す。並行 leaf との衝突を避けるため
  この leaf では ADR 本文を編集していない。
