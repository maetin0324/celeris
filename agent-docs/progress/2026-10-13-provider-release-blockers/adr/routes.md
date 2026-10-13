---
tasks: [01M4T98YHKGT945HH1588SC2VR]
status: done
title: routes — 件 3（proxy 経由 harness の <source>/<model>）と件 5（execution class ごとの route 候補）の設計
completed: 2026-10-13
---

# routes: 件 3・件 5 の設計（ADR 2026-10-11「2026-10-13 付記」の材料）

コードは変更していない（この memo だけ。`crates/` に差分なし）。後段の write-adr がここから
`agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の付記と `docs/ops/provider-llm-source-migration.md`
の手順を書く。実装 leaf 名は件 3 = `model-names`、件 5 = `class-routes`。

## 読んで確かめた前提（現在の挙動）

| 事実 | 場所 |
|---|---|
| llm-proxy は要求 model 名を `<source_id>/<model_id>` としてだけ解析する（最初の `/` までが source ID。それ以外は 422 `invalid_model`） | `crates/llm-proxy/src/naming.rs:parse_model`、`crates/llm-proxy/src/server.rs:1089`（422） |
| route が harness に渡す model は `Candidate.model`（接頭辞なし）のまま | `crates/task-dispatch/src/dispatcher/route_launch.rs:105-107`（`with_model`）・`:72`（`SelectedLlmRoute.model_id`）、`crates/task-worker/src/route.rs:44-46` |
| `[[llm_routes]].via` は**適格性判定にしか使われていない**（`via = "llm_proxy"` でも model 名・endpoint は変わらない） | `crates/task-core/src/llm_route.rs:constrain_for_harness`、`crates/celeris/src/config/llm_route.rs:validate_registry` |
| `SelectedLlmRoute.endpoint` は source 自身の `endpoint`（OAuth source は `None`）。proxy の URL ではない | `crates/task-dispatch/src/dispatcher/route_launch.rs:65-68` |
| endpoint を adapter に当てるのは worker 側の `route_adapter` だけ。daemon 内の run（本番経路）は `Dispatcher::route_adapter` を使い、endpoint を当てない | `crates/task-worker/src/route.rs:65-72` と `crates/task-dispatch/src/dispatcher/route_launch.rs:92-117`、`dispatch_run.rs:524` |
| `with_llm_endpoint` を実装している adapter は `pi` だけ（`paperqa`/`local-deep-research`/`langmem`/`claude_code`/`codex`/`acp` は無い） | `crates/task-worker/src/pi.rs:127`、`crates/task-worker/src/adapter.rs:279`（既定 `None`） |
| proxy 経由 harness の接続先は adapter 側の設定にある: PaperQA = `[adapters.paperqa].env OPENAI_BASE_URL`、LDR = `[adapters.local_deep_research].settings "llm.openai_endpoint.url"`、LangMem = `[knowledge.langmem].base_url` | `crates/task-worker/src/paperqa.rs:721`（`env_value`）、`crates/task-worker/src/local_deep_research.rs:463-467`、`crates/celeris/src/daemon/adapters.rs:105-156` |
| LangMem は route の model を**無視**する（`with_model` 未実装 → `[knowledge.langmem].model` のまま） | `crates/task-worker/src/langmem.rs`（`with_model` なし）、`crates/celeris/src/daemon/adapters.rs:147-155` |
| PaperQA の runner は `settings.llm` だけ上書きする（`summary_llm`・`agent.agent_llm` は settings file の値のまま） | `crates/task-worker/src/paperqa_ask.py:792-794` |
| PaperQA の検索語 LLM は model 名の先頭 1 区画を**落とす**（`qwen/qwen3.8-27b` → `qwen3.8-27b`）。proxy には届かない名前になる | `crates/task-worker/src/paperqa.rs:669`（`strip_provider_prefix`）・`:705-718`（`query_llm_model`） |
| `route_defaults` は harness 単位だけ。execution class 別の候補欄は無い | `crates/task-core/src/llm_route.rs:111`（`Registry.route_defaults`）、`crates/celeris/src/config/llm_route.rs:58-63`（`RouteDefaultConfig`、`deny_unknown_fields`） |
| `route_select` は harness を確定させた後に **execution class を捨てている** | `crates/task-dispatch/src/dispatcher/route_select.rs:158`（`one.harness.execution_class = None;`） |
| execution class を作る箇所は 3 つ（`coding` / `planner` / `review`） | `route_select.rs:343`、`dispatch_run.rs:513`、`review_spawn.rs:1082` |
| `base_url` が同じ celeris の llm-proxy を指すかの判定は既にある | `crates/celeris/src/config/knowledge.rs:targets_llm_proxy`（private）・`langmem_targets_llm_proxy`、bearer は `[api] token_file`（`langmem_api_key`） |

---

## 件 3: proxy 経由 harness へ渡す model 名

### D3-1 規則と変換の 1 か所

**決定**: harness に渡す model 名（wire name）は

- `[[llm_routes]]` に `(harness, source, model)` の行があり `via = "llm_proxy"` なら `<source_id>/<model_id>`
- それ以外（直接接続）なら `<model_id>` のまま

とする。変換は**純粋関数 1 つ・呼び出し 1 か所**に置く:

```rust
// crates/task-core/src/llm_route.rs
pub const VIA_LLM_PROXY: &str = "llm_proxy";
/// (harness, candidate) の登録経路の `via`（無ければ None）。
pub fn route_via(registry: &Registry, harness: &HarnessId, key: &CandidateKey) -> Option<&str>;
/// harness に渡す model 名。via = llm_proxy のときだけ `<source_id>/<model_id>` にする。
pub fn wire_model_id(registry: &Registry, harness: &HarnessId, candidate: &Candidate) -> ModelId;
```

計算は `route_select_one`（`crates/task-dispatch/src/dispatcher/route_select.rs`）で**一度だけ**行い、
`RoutePick` に新欄 `wire_model: ModelId` として載せる。消費側は 2 つともこの欄を読むだけで、再び変換しない:

- `Dispatcher::route_adapter`（`route_launch.rs:105-107`）: `with_model(pick.wire_model.as_ref())`
- `selected_llm_route`（`route_launch.rs:72`）: `model_id: pick.wire_model.to_string()`

`SelectedLlmRoute.model_id` の意味は「**harness を起動するときの名前**」に変わる（protocol の型は同じ、doc comment
を更新）。worker 側 `crates/task-worker/src/route.rs:route_adapter` は**変更しない**（受け取った名前をそのまま
`with_model` に渡すだけ。registry を持たないので変換しようがない）。event・run 行の model は従来どおり
`Candidate.model`（接頭辞なしの正本）を使い、`RouteTrace::metadata` の `decision` に `"wire_model"` を 1 欄足して
証拠を残す。

**理由**: `via` は registry（daemon 側）にしか無く、worker には registry が渡らない。worker で変換するには
protocol に `via` か source 接頭辞を足すことになり、変換箇所が 2 か所（daemon の in-process 経路と worker 経路）に
増える。ADR 2026-10-11「model ID はその source の model」も保てる（接頭辞は記録でなく転送の名前）。
`Candidate.model` を正本のまま残すので event・run 行・API 表示・`model_hint` の照合は変わらない。

**変更箇所**:
- `crates/task-core/src/llm_route.rs`: `VIA_LLM_PROXY`、`route_via`、`wire_model_id`
- `crates/task-dispatch/src/dispatcher/route_select.rs`: `RoutePick.wire_model`、`route_select_one`（決定時に計算）、`RouteTrace::metadata`（`decision.wire_model`）
- `crates/task-dispatch/src/dispatcher/route_launch.rs`: `Dispatcher::route_adapter`、`selected_llm_route`
- `crates/task-worker/src/route.rs`: doc comment のみ（変換ロジックは足さない）
- `RoutePick` を組む試験の追従: `crates/task-dispatch/src/dispatcher/tests/route_launch.rs:112`、`tests/route_select.rs`、`tests/review_route.rs`

**実装 leaf（model-names）が足す試験名**:
- `task_core::llm_route::tests::wire_model_id_qualifies_only_routes_declared_via_llm_proxy`
- `task_core::llm_route::tests::wire_model_id_keeps_a_model_that_already_contains_a_slash`（`opencode-go/grok-4.6` は最初の `/` までが source ID なのでそのまま使える）
- `task_dispatch::dispatcher::tests::route_select::proxy_route_pick_carries_the_qualified_wire_model`
- `task_dispatch::dispatcher::tests::route_select::direct_route_pick_keeps_the_bare_model`
- `task_dispatch::dispatcher::tests::route_launch::route_adapter_launches_the_wire_model`
- `task_worker::route::tests::route_adapter_hands_the_route_model_to_the_adapter_unchanged`（worker が二度接頭辞を付けないことの固定）

### D3-2 proxy 経由 route の endpoint

**決定**: `via = "llm_proxy"` の route では `SelectedLlmRoute.endpoint` を **`None`** にする（source 自身の
`endpoint` を載せない）。proxy の URL は harness 側の宣言（`[adapters.paperqa].env OPENAI_BASE_URL`、
`[adapters.local_deep_research].settings "llm.openai_endpoint.url"`、`[knowledge.langmem].base_url`、pi は
`[harnesses.pi] env`〈件 2 の leaf が宣言欄を作る〉）に置く。bearer は `[api] token_file` の値
（`Config::langmem_api_key` と同じ規則を paperqa/ldr の secret にも適用する）。

`route_launch.rs:65-68` の式は「直接接続の route だけ endpoint を載せる」に変わる:

```rust
let via_proxy = route_via(registry, &pick.harness, &pick.candidate.key()).is_some_and(|v| v == VIA_LLM_PROXY);
let endpoint = (!via_proxy && (source.endpoint.is_some() || !oauth)).then(|| LlmEndpointReference { … });
```

**理由**: (1) source の endpoint（例: Qwen の直 URL）を proxy 経由 route に載せると、`with_llm_endpoint` を持つ
adapter（pi）が proxy を迂回して直接接続し、anthropic→openai_compatible の変換が効かなくなる。(2)
`with_llm_endpoint` を持たない adapter（paperqa/ldr/langmem）は endpoint が `Some` だと worker 経路で
`RouteError::EndpointUnsupported` になり fail closed してしまう（daemon 経路では無視されるので経路ごとに挙動が
食い違う）。(3) 人の決定 2026-10-13「LDR は proxy 経由のまま」＝接続先は harness の宣言に残す、と一致する。

**変更箇所**: `crates/task-dispatch/src/dispatcher/route_launch.rs:selected_llm_route`

**試験名**:
- `task_dispatch::dispatcher::tests::route_launch::proxy_route_carries_no_source_endpoint`
- `task_dispatch::dispatcher::tests::route_launch::direct_route_still_carries_the_source_endpoint`

### D3-3 設定検証（両方向・fail closed）

**決定**: `Config::load`（`crates/celeris/src/config/llm_route.rs`）で次を検査し、違反は読み込みエラーにする。
警告だけ出して無視する経路は作らない。

1. `via = "llm_proxy"` の行が 1 つでもあれば `[llm_proxy] enabled`（`effective_enabled()`）を要求する。
2. `via = "llm_proxy"` の行の source ID は proxy 側にも宣言があること（`claude_oauth` / `codex_oauth` /
   `[[llm_proxy.sources.openai_compatible]] id`）。proxy が知らない source ID を接頭辞にすると 422 になる。
3. harness ごとの**実効の接続先**を 1 つの関数で求める:
   `Config::declared_llm_base_url(harness_id) -> Option<String>`（kind 別: `paperqa` → `[adapters.paperqa].env
   OPENAI_BASE_URL`、`local-deep-research` → settings `llm.openai_endpoint.url`、`langmem` →
   `[knowledge.langmem].base_url`、`pi`/`acp` → `[harnesses.<id>] env` の `OPENAI_BASE_URL`、それ以外 → `None`）。
   `targets_llm_proxy`（`config/knowledge.rs` の private fn）を `config` crate 内の共有 helper に上げて再利用する。
4. **両方向の一致**: その harness の実効の接続先が llm-proxy を指す ⇔ その harness の候補（`route_defaults`・
   class 別の候補〈件 5〉・`[cos] route_candidates`）が**すべて** `via = "llm_proxy"` の行を持つ。
   - proxy を指すのに `via` の行が無い候補 → エラー（＝今回の不具合そのもの。LangMem×qwen がこれに当たる）
   - `via = "llm_proxy"` の行があるのに実効の接続先が proxy でない（または宣言が無い）harness → エラー
     （`claude_code`・`codex`・`aider`・`fake` は接続先を宣言できないので proxy 経由にできない）
5. `via = "llm_proxy"` の候補の model は `<source_id>/` で始まっていてはならない（二重接頭辞の防止）。

**理由**: wire name の規則（D3-1）は `via` の宣言だけを根拠にするので、宣言と実接続先が食い違うと黙って 422 に
なる。起動前に決定的に弾くのが fail closed。4 の「両方向」が無ければ LangMem のように `via` 行の無い候補が
素通りする（本番で起きた形）。

**変更箇所**: `crates/celeris/src/config/llm_route.rs`（`Config::declared_llm_base_url`、`validate_registry` の
呼び出し側で `&Config` を見る新しい `validate_proxy_routes(&self, &Registry)`）、`crates/celeris/src/config/knowledge.rs`
（`targets_llm_proxy` を `pub(super)` の共有 helper へ）

**試験名**:
- `celeris::config::tests::proxy_route_requires_an_enabled_llm_proxy`
- `celeris::config::tests::proxy_route_requires_the_source_declared_in_llm_proxy`
- `celeris::config::tests::proxy_bound_harness_candidates_all_need_a_via_llm_proxy_route`（LangMem×qwen の形を再現）
- `celeris::config::tests::via_llm_proxy_route_needs_a_harness_endpoint_that_targets_the_proxy`（`claude_code` で弾かれる）
- `celeris::config::tests::proxy_route_model_must_not_carry_its_own_source_prefix`

### D3-4 PaperQA の `settings.llm`（と `summary_llm` 等）の上書き

**決定**: route が model を選んだときは PaperQA の**3 つの LLM 欄をすべて**その wire name で上書きする。
`paperqa_ask.py` の `settings.llm = model` の隣に

```python
settings.summary_llm = model
settings.agent.agent_llm = model
```

を足す（payload の `model` が無い＝route が無いときは settings file の値のまま）。あわせて
`crates/task-worker/src/paperqa.rs` の `strip_provider_prefix`（`:669`）と `query_llm_model` 内の 2 か所の呼び出し
（`:710`・`:717`）を**削除**し、検索語を立てる LLM にも wire name をそのまま渡す。
`[adapters.paperqa].acquire.query_model` を書くときは接続先が受け取る名前（proxy なら `<source_id>/<model_id>`）で
書く（旧来の LiteLLM `provider/` 接頭辞の読み替えは廃止）。

**理由**: `summary_llm`・`agent.agent_llm` が settings file の値（`qwen3.8-27b`）のままだと、fallback で
`claude_oauth/claude-opus-5-5` になった run でも要約・agent の呼び出しだけが古い名前＝接頭辞なしで proxy に当たり
422 になる（1 run の中で LLM 先が割れる）。`strip_provider_prefix` は先頭 1 区画を落とすので、wire name
（`qwen/qwen3.8-27b`・`claude_oauth/claude-sonnet-5-5`）を必ず壊す。名前の変換は D3-1 の 1 か所に寄せ、worker 側の
書き換えは消す（破壊的変更を是とする方針: 旧い仕組みを残さない）。

**変更箇所**: `crates/task-worker/src/paperqa_ask.py:792-794`、`crates/task-worker/src/paperqa.rs`
（`strip_provider_prefix` 削除、`query_llm_model`、doc comment）

**試験名**:
- `task_worker::paperqa::tests::route_model_overrides_llm_summary_llm_and_agent_llm`
- `task_worker::paperqa::tests::query_llm_model_keeps_the_source_qualified_name`
- `task_worker::paperqa::tests::query_model_config_is_used_as_written`（`acquire.query_model` をそのまま渡す）

### D3-5 LangMem は route の model を使う

**決定**: `LangMemAdapter` に `with_model` を実装し（`config.model = Some(model)`）、route が選んだ wire name を
優先する。`[knowledge.langmem].model` は route が無い経路（dispatch 前の `GET <base_url>/models` probe、
`knowledge_fallback`）の値として残り、proxy を指す設定では接頭辞付き（`qwen/qwen3.8-27b`）で書く。

**理由**: 現状は `[route_defaults.langmem]`・`llm_route_selected` event が選ばれた model を記録するのに、実際の
呼び出しは `[knowledge.langmem].model` 固定で、記録と実体が食い違う（fallback しても qwen のまま）。
`route.rs` の契約「model は route の model そのまま」に合わせる。

**変更箇所**: `crates/task-worker/src/langmem.rs`（`impl WorkerAdapter for LangMemAdapter` に `with_model`）、
`crates/celeris/src/daemon/adapters.rs:147-155`（`model: llm.model.clone()` は既定値のまま残る）

**試験名**:
- `task_worker::langmem::tests::with_model_replaces_the_configured_model`
- `task_worker::langmem::tests::run_payload_carries_the_route_model`

### D3-6 LDR は変更不要（設定だけ）

**決定**: `LdrAdapter` は `config.model` を settings `llm.model` に上書きする既存の経路
（`local_deep_research.rs:463-467`）でそのまま wire name を受け取る。構造化 synthesis も同じ `llm.model` と
`llm.openai_endpoint.url` を見る（`local_deep_research_run.py:1234-1252`）ので追加の変更は無い。

**試験名**（回帰固定）: `task_worker::local_deep_research::tests::route_model_overrides_settings_llm_model`

### D3-7 fallback 時の model 名（人の決定 2026-10-13 の前提）

**決定**: LDR・PaperQA・LangMem は proxy 経由のまま、source が Claude/Codex に倒れても wire name を替えるだけで
動く。本番の候補順と wire name は次のとおり（`[route_defaults.*]` の順は変えない）:

| harness | 1 番目 | 2 番目 | 3 番目 |
|---|---|---|---|
| `local-deep-research` | `qwen/qwen3.8-27b` | `claude_oauth/claude-sonnet-5-5` | `codex_oauth/gpt-6-luna` |
| `paperqa` | `qwen/qwen3.8-27b` | `claude_oauth/claude-opus-5-5` | `codex_oauth/gpt-6.1-sol` |
| `langmem` | `qwen/qwen3.8-27b` | `claude_oauth/claude-sonnet-5-5` | `codex_oauth/gpt-6-luna` |

いずれも `[[llm_routes]]` に `via = "llm_proxy"` の行を置き、`to_api = "openai_compatible"`（anthropic 由来は
`from_api = "anthropic"`）を宣言する。proxy が source 側の認証・quota・変換を持つので、harness 側の設定は
fallback ごとに変わらない。

### D3-8 本番 config の書き換え（人が実行する runbook 手順）

`docs/ops/provider-llm-source-migration.md`「移行後の TOML」に次を反映する（書き換えは人が行う。daemon の
reload 手順・backup は既存の runbook のまま）:

1. `[[llm_routes]] harness = "langmem" source = "qwen" model = "qwen3.8-27b" via = "llm_proxy"
   from_api = "openai_compatible" to_api = "openai_compatible"` を**追加**（今の runbook に無く、D3-3 の 4 で
   エラーになる形）。
2. `[adapters.local_deep_research].settings` を proxy 向けにする:
   `"llm.openai_endpoint.url" = "<[llm_proxy] listen の URL>/v1"`、`"llm.openai_endpoint.api_key" = "<[api] token
   を入れた secret の参照>"`、`"llm.model" = "qwen/qwen3.8-27b"`（route が無い run の既定値も接頭辞付きにする）。
3. `[adapters.paperqa].env` に `OPENAI_BASE_URL = "<proxy URL>/v1"`、`env_from_secrets` の `OPENAI_API_KEY` に
   `[api] token` を持つ secret を入れる。PaperQA の settings file の `llm`・`summary_llm`・`agent.agent_llm` は
   `qwen/qwen3.8-27b` にする（route がある run では D3-4 で上書きされるが、route の無い run のため）。
4. `[knowledge.langmem]` は `base_url = "<proxy URL>/v1"`、`model = "qwen/qwen3.8-27b"`（現行 runbook どおり）。
5. `[[llm_routes]] harness = "claude_code" source = "codex_oauth" model = "gpt-6-astra" via = "llm_proxy"` と
   `[cos] route_candidates` の `{ source = "codex_oauth", model = "gpt-6-astra" }` を**外す**（D3-3 の 4 で
   エラーになる。`claude_code` は接続先を宣言できず、proxy に向けられない）。CoS の候補は `claude_oauth` の
   model だけになる。
6. 確認: `celerisctl routing show`（D5-4 で class 別候補も出す）で候補と `via` を突き合わせ、
   `curl -H "Authorization: Bearer <[api] token>" <proxy URL>/v1/chat/completions -d '{"model":"qwen/qwen3.8-27b",…}'`
   が 200、`"model":"qwen3.8-27b"` が 422 `invalid_model` になることを確かめてから reload する。

### D3-9 未解決（別 task にする提案）

- **`claude_code`・`codex` を proxy 経由にできない**: ADR 2026-10-11 §CoS は codex_oauth への fallback を
  維持するとしているが、`ClaudeCodeAdapter` に `with_llm_endpoint`（`ANTHROPIC_BASE_URL` /
  `ANTHROPIC_AUTH_TOKEN`）が無く、daemon 経路（`Dispatcher::route_adapter`）は endpoint を当てないため、
  その fallback は「claude に存在しない model 名を渡す」形でしか動かない。D3-8 の 5 で一旦外す。
  戻すには (a) `ClaudeCodeAdapter::with_llm_endpoint`、(b) daemon 経路でも endpoint を当てる
  （`Dispatcher::route_adapter` を `task_worker::route::route_adapter` に寄せるか、同じ `with_llm_endpoint` を呼ぶ）、
  (c) proxy の bearer を secret 参照で worker に渡す経路、が要る。→ followup task を提案する。
- **daemon 経路が endpoint を当てない**（`dispatch_run.rs:524` → `route_launch.rs:92-117`）: 直接接続の route
  （pi×qwen など）で worker 経路と挙動が違う。件 2 の `[harnesses.pi] env` 宣言で実害は消える見込みだが、
  経路の一本化は上の followup と同じ作業。

---

## 件 5: execution class ごとに route の候補を変える

lane・tier は使わない。後続 task 01M4M8D4AQ（評価に基づく policy 差し替え）の `RoutePolicy` 境界は触らない
（`DefaultRoutePolicy` は「宣言順の最初」のまま）。ADR 2026-10-11 §source と model の選択の
「必要なら execution class ごとに同じ形式を定義できる」を実現する。

### D5-1 config の形

**決定**: `[route_defaults.<harness_id>]` に class 別の表を足す（同じ `candidates` の形を class ごとに繰り返す）:

```toml
[route_defaults.claude_code]
# 既定（coding 以外の execution class と、class を持たない task）
candidates = [
  { source = "claude_oauth", model = "claude-sonnet-5-5" },
  { source = "claude_oauth", model = "claude-fable-5-1" },
]

[route_defaults.claude_code.classes.coding]
candidates = [
  { source = "claude_oauth", model = "claude-opus-5-5" },
  { source = "claude_oauth", model = "claude-sonnet-5-5" },
  { source = "claude_oauth", model = "claude-fable-5-1" },
]
```

型（`crates/celeris/src/config/llm_route.rs`、どちらも `deny_unknown_fields`）:

```rust
pub struct RouteDefaultConfig {
    #[serde(default)] pub candidates: Vec<RouteCandidateConfig>,
    #[serde(default)] pub classes: BTreeMap<String, RouteClassDefaultConfig>,
}
pub struct RouteClassDefaultConfig {
    pub candidates: Vec<RouteCandidateConfig>,
}
```

`candidates` は省略可（class 別だけを持つ harness も書ける）だが、`classes.<c>` を書いたのに `candidates` が空なら
読み込みエラー（黙って harness 既定に倒さない）。

**理由**: 既存の `[route_defaults.<harness>]` の形を壊さず（`candidates` の意味は同じ）、TOML で 1 階層足すだけ。
`RouteDefaultConfig` は `deny_unknown_fields` なので、綴り違い（`class`・`by_class`）は黙って無視されずエラーになる。

### D5-2 registry と優先順の引き当て

**決定**: `Registry` に class 別の表を足し、`route_priority` に execution class を渡す:

```rust
// crates/task-core/src/llm_route.rs
pub struct Registry {
    …
    pub route_defaults: BTreeMap<HarnessId, Vec<CandidateKey>>,
    /// execution class 別の候補（宣言があれば harness 既定に**優先**し、無ければ既定を使う）。
    pub route_class_defaults: BTreeMap<HarnessId, BTreeMap<ExecutionClass, Vec<CandidateKey>>>,
    …
}

pub fn route_priority<'a>(
    registry: &'a Registry,
    harness: &HarnessId,
    execution_class: Option<&ExecutionClass>,
    cos: bool,
) -> &'a [CandidateKey];
```

優先順は `cos` → `cos_route_candidates`、次に class 別の宣言（**空でなければ**）、最後に harness 既定。
class 別の宣言があるときは既定に**倒さない**（全部不適格なら `RouteFailure::NoEligible` で fail closed）。

`route_select` は harness を確定させた後に execution class を捨てている行
（`route_select.rs:158` の `one.harness.execution_class = None;`）を**削除**し、`route_select_one` の
`route_priority(registry, &harness, input.harness.execution_class.as_ref(), input.cos)` に class を渡す。
harness は `one.harness.harness = Some(h)` で明示済みなので `select_harness` は class を見ない（＝class 既定の
harness 順と候補順は別々に効く）。

**理由**: 候補の優先順を決める情報は宣言順だけ（`default_route_policy` は変えない）。`RoutePolicy` の境界
（01M4M8D4AQ）を先取りしない。class 別を「上乗せ」でなく「置き換え」にするのは、coding run が opus を使い切った
ときに黙って sonnet へ倒れると、人が書いた候補順が効いているか event から分からなくなるため（倒すなら class 別に
sonnet を書き足す＝D5-1 の例のとおり）。

**変更箇所**:
- `crates/task-core/src/llm_route.rs`: `Registry.route_class_defaults`、`route_priority`（引数追加）
- `crates/task-dispatch/src/dispatcher/route_select.rs:158`（削除）・`:206`（class を渡す）
- `crates/celeris/src/config/llm_route.rs`: `RouteDefaultConfig.classes` → registry 構築（`:119-130`）
- `crates/celeris/src/config/harness.rs:apply_verify_smoke`（`:242-246`）: `defaults.candidates` だけでなく
  `defaults.classes` の中の候補からも `smoke` を外す
- `crates/celerisctl/src/commands/routing.rs:475-478`: `=== route_defaults ===` に class 別の行
  （`claude_code[coding]: claude_oauth/claude-opus-5-5, …`）を出す

### D5-3 検証と class 名の正本

**決定**:
1. class 別候補も harness 既定と同じ検証を通す（source 宣言あり・model 宣言あり・harness と api_format/`llm_routes`
   の整合・class 内での重複なし）。`validate_registry`（`config/llm_route.rs:215-242`）のループを
   `route_defaults` と `route_class_defaults` の両方に掛ける。
2. execution class 名の正本を task-core に置く:
   `pub const EXECUTION_CLASSES: &[&str] = &["coding", "planner", "review"];`。
   class を作る 3 箇所（`route_select.rs:343`、`dispatch_run.rs:513`、`review_spawn.rs:1082`）はこの定数を使い、
   設定の `[route_defaults.<h>.classes.<c>]` と `[harness_selection.execution_class_defaults]` の key は
   この集合に無ければ読み込みエラーにする（綴り違いは黙って無視されると候補が効かない）。

**理由**: class 名はコードが作る値との一致だけが意味を持つ。`Coding`・`code` のような綴り違いは今の形だと
「宣言したのに一度も引かれない」静かな不具合になる（provider 廃止リリースで起きた種類の不具合と同じ）。

**試験名**:
- `task_core::llm_route::tests::route_priority_prefers_the_execution_class_list`
- `task_core::llm_route::tests::route_priority_ignores_the_class_for_cos`
- `task_dispatch::dispatcher::tests::class_route::coding_class_picks_the_class_candidate_first`
- `task_dispatch::dispatcher::tests::class_route::class_list_does_not_fall_back_to_the_harness_defaults`（fail closed）
- `task_dispatch::dispatcher::tests::route_select::class_candidates_survive_the_harness_loop`（`route_select.rs:158` の削除の固定: 複数 harness を跨いでも class が残る）
- `celeris::config::tests::route_defaults_classes_build_the_registry`
- `celeris::config::tests::class_candidates_are_validated_like_harness_candidates`
- `celeris::config::tests::empty_class_candidates_are_a_config_error`
- `celeris::config::tests::unknown_execution_class_is_a_config_error`
- `celeris::config::tests::apply_verify_smoke_drops_smoke_from_class_candidates`
- `celerisctl::commands::routing::tests::show_prints_class_candidates`

### D5-4 本番 config の書き換え（人が実行する runbook 手順）

1. `[route_defaults.claude_code] candidates` を
   `[{ source = "claude_oauth", model = "claude-sonnet-5-5" }, { source = "claude_oauth", model = "claude-fable-5-1" }]`
   にする（既定 = sonnet。今の runbook の opus 先頭をやめる）。
2. `[route_defaults.claude_code.classes.coding] candidates` を
   `[{ source = "claude_oauth", model = "claude-opus-5-5" }, { source = "claude_oauth", model = "claude-sonnet-5-5" }, { source = "claude_oauth", model = "claude-fable-5-1" }]`
   にする（coding だけ opus 先頭）。
3. `[route_defaults.codex]`・`planner`・`review` は class 別を宣言しない（既定のまま）。`planner`・`review` run は
   sonnet 先頭になる。review だけ opus にしたい場合は `[route_defaults.claude_code.classes.review]` を同じ形で足す
   （人の判断。今回は足さない）。
4. `[cos] route_candidates` は別表（CoS は class を持たない）なので変えない。
5. 確認: `celerisctl routing show` の `route_defaults` に `claude_code` と `claude_code[coding]` の両方が出ること。
   dispatch 後の run の `llm_route_selected.metadata.decision.model` が coding run で `claude-opus-5-5`、
   それ以外で `claude-sonnet-5-5` になっていること。

### D5-5 API・web への出し方（今回は範囲外）

`GET /api/v1/harnesses` の `HarnessView.route_defaults`（`crates/celeris/src/daemon/api.rs:229-235`）は harness 既定
の候補だけを返す。class 別候補を API・web に出すには生成 schema の再生成（`UPDATE_SCHEMA=1` + gui/web の型）が要る
ので、この不具合修正では**変更しない**。`celerisctl routing show`（D5-2）で確認できることを runbook に書く。
出すと決めたときは別 task（web の harness 画面の表示も含む）。

---

## 並行 leaf・後段との接点

- **harness leaf（件 2・4）**: `crates/celeris/src/config/llm_route.rs` の `HarnessDeclarationConfig`（`[harnesses.<id>]`
  の `extensions`/`tools`/`env` 宣言）と、この memo の `RouteDefaultConfig.classes`・`validate_proxy_routes` は
  同じ file・同じ `validate_registry` の近くを触る。`[harnesses.pi] env` に `OPENAI_BASE_URL` が入るなら、
  D3-3 の 3 の `declared_llm_base_url` はその欄を読む（pi×qwen を proxy 経由にするか直接にするかは
  `[[llm_routes]]` の `via` と一致させる）。件 4（opencode の model ID）で `qwen-local/qwen3.8-27b` のような
  宣言にする場合、proxy 経由なら wire name は `qwen/qwen-local/qwen3.8-27b` になる（`parse_model` は最初の `/`
  までを source ID とするので整合する）が、**opencode の model ID の正本は件 4 の決定に従う**。
- **events leaf（件 1）**: 接点なし（`routing_decided` の読み取り専用表現は別 file）。
- **write-adr leaf**: ADR 付記には D3-1〜D3-9・D5-1〜D5-5 を「決定／理由／変更箇所／試験名」の形で写し、
  D3-8・D5-4 を runbook（`docs/ops/provider-llm-source-migration.md`）の手順として書く。D3-9 は付記の
  「残る別工程」節に置く。
- **実装 leaf の範囲**: `model-names` = D3-1〜D3-6（task-core `wire_model_id` / route_select `RoutePick.wire_model` /
  route_launch / config 検証 / paperqa 3 欄＋`strip_provider_prefix` 削除 / langmem `with_model`）。
  `class-routes` = D5-1〜D5-3（config `classes` / registry `route_class_defaults` / `route_priority` 引数 /
  `route_select.rs:158` 削除 / `apply_verify_smoke` / `celerisctl routing show`）。両方とも
  `crates/celeris/src/config/llm_route.rs` を触るので、統合後の段で `cargo test -p celeris --lib` を流す。
