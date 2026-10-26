# LLM source のローカル OpenAI 互換プロキシ

celeris は `[llm_proxy]` を有効にすると、`127.0.0.1:18100`（既定）で OpenAI 互換の
`POST /v1/chat/completions` を提供する。source/model の候補は現行の `[llm_sources.*]` と
route 設定で明示する。ログイン済みの Claude Code / Codex の資格情報
（`[accounts] claude_dir` / `codex_dir`）と、既存の OpenAI 互換エンドポイント（Qwen 等）を
「供給元（LLM source）」として束ね、opencode・PaperQA・Local Deep Research・LangMem のような
ハーネスに対して、設定済みの具体的な source/model へ要求を中継する。

実装は `crates/llm-proxy`（独立クレート）。celeris（daemon）はこれを起動・停止するだけで、
選択やトークン更新の判断ロジックは持たない。

## 1. モデル名

proxy の旧 tier alias と `[llm_proxy.models]` は廃止された。具体的な `<source_id>/<model_id>` を書く。旧設定を現行の source/model route に移す場合は
[provider 設定の移行手順](../ops/provider-llm-source-migration.md)を参照する。

### 参考: 旧単価表

`task_core::pricing` の USD / 100 万トークン。Claude の値はこのタスクの計画時に
オフラインの `claude-api` skill（`SKILL.md` の「Current Models (cached 2026-06-24)」、
`shared/models.md`、`shared/prompt-caching.md` の「Economics」）から確認した値を転記した。
`*` は明示単価でなく、同 skill の一般則（cache read = 入力の 0.1 倍、
5 分 TTL の cache write = 入力の 1.25 倍）からの導出値。fable と opus の
cache read はモデル別の明示値を優先する。cache write は 5 分 TTL の概算であり、
異なる TTL や契約単価には適用できない。

| モデル | 入力 | 出力 | cache read | cache write | 出典 | 取得日 |
|---|---:|---:|---:|---:|---|---|
| `claude-fable-5-1` | 10.0 | 50.0 | 0.25 | 12.5* | claude-api: Current Models / models / prompt-caching | 2026-06-24 キャッシュ、2026-09-25 転記 |
| `claude-opus-5-5` | 4.0 | 20.0 | 0.20 | 5.0* | 同上 | 同上 |
| `claude-sonnet-5` | 2.0 | 10.0 | 0.2* | 2.5* | 同上 | 同上 |
| `claude-haiku-4-5-20251001`（表の prefix は `claude-haiku-4-5`） | 1.0 | 5.0 | 0.1* | 1.25* | 同上 | 同上 |
| `gpt-6-astra` | 不明 (`None`) | 不明 (`None`) | 不明 (`None`) | 不明 (`None`) | 一次情報が手元にない | 2026-09-25 確認 |
| `gpt-6-sol` | 不明 (`None`) | 不明 (`None`) | 不明 (`None`) | 不明 (`None`) | 同上 | 同上 |
| `gpt-6-luna` | 不明 (`None`) | 不明 (`None`) | 不明 (`None`) | 不明 (`None`) | 同上 | 同上 |

GPT-6 の ID は `docs/` と `config/` にあるが、単価の根拠はない。このタスクでは外部ネットワーク・
道具を使えないため推測値を入れず、全欄を `None` にした。`None` の単価に 0 より大きい
トークンがある場合、推定額全体を `None` とする。旧 `gpt-5` 行の cache write も根拠のない
0 USD をやめ、`None` とした。

## 2. 供給元（source）

| source | 種類 | 資格情報 | 上流 |
|---|---|---|---|
| `claude-oauth` | Claude アカウントプール | `<claude_dir>/<id>/.credentials.json`（Claude Code と同じ形） | `https://api.anthropic.com/v1/messages`（Messages API） |
| `codex-oauth` | Codex（ChatGPT）アカウントプール | `<codex_dir>/<id>/auth.json`（Codex CLI と同じ形） | `https://chatgpt.com/backend-api/codex/responses`（Responses API） |
| `openai-compatible` | 中継（例 Qwen） | 無し（`api_key` は平文の bearer。無ければ付けない） | 設定した `base_url` そのまま |

`claude-oauth` / `codex-oauth` は **ADR-0024/0025 のアカウントプールを再利用する**（`.celeris-usage.json`
帳簿を CLI ワーカーの dispatch と共有。cooldown・観測値は同じ表に書く。別の写しは作らない）。

### codex-oauth の要求形（Phase 65b 追記）

本番で `gpt/*`（codex-oauth）経由の要求が全て 0.4 秒以内に `400 {"error":{"message":"upstream error","type":"upstream_error"}}`
で落ちる事故があった（2026-09-21。`agent-docs/adr/0053-llm-source-proxy.md` の Phase 65b 追記）。原因は
ChatGPT の Codex backend（`https://chatgpt.com/backend-api/codex/responses`）が Codex CLI
（`codex-rs`）と違う形の要求を拒否すること。この層は Codex CLI と同じ形で送る:

- **`store: false`、`stream: true` を常に送る**（クライアントの `stream` の値に関わらず。Codex
  backend は非 stream の応答を受け付けない）。クライアントが `stream: false` を要求したときは、
  この層が上流の SSE を集約して 1 つの `chat.completion` に組み立てる（`response.output_text.delta`
  でテキスト、`response.output_item.done` の `function_call` で tool call、`response.completed`
  （`incomplete`/`failed` も同様）で usage / finish_reason を集める）。
- **`instructions` は常に入れる**（クライアントに system メッセージが無ければ既定の一文。Codex CLI
  は空にしない）。
- **`temperature` / `max_output_tokens` は既定では送らない**（Codex CLI が送らないフィールドで、
  送ると拒否されることがある）。`[llm_proxy.sources.codex_oauth] send_sampling_params = true` で
  明示的に opt-in したときだけクライアントの値を転送する。
- `reasoning_effort`（`[llm_proxy.sources.codex_oauth]`）を設定したときだけ
  `"reasoning": {"effort": ..., "summary": "auto"}` と `"include": ["reasoning.encrypted_content"]`
  を付ける（既定では付けない）。
- `parallel_tool_calls: true` を常に付け、`tools` があって `tool_choice` の指定が無ければ
  `tool_choice: "auto"` を付ける。
- ヘッダに `User-Agent: codex_cli_rs/<version>`（`[llm_proxy.sources.codex_oauth] user_agent`。
  **既定値のバージョン番号は未確認**。`codex --version` 等の実機で確認して上書きすること）を追加した。
  他のヘッダ（`Authorization`、`chatgpt-account-id`、`OpenAI-Beta: responses=experimental`、
  `originator: codex_cli_rs`、`session_id`、`accept: text/event-stream`）は Phase 65 のまま。

### claude-oauth の sampling 引数（2026-10-26）

claude-oauth は OpenAI 互換要求の `temperature` / `top_p` を Messages API へ写さず、黙って捨てる。
Claude 5 系はこれらを `400 "`temperature` is deprecated for this model."` で拒否し、LDR（LangChain の
`ChatOpenAI`）などの client は自前の既定値を付けて来るため（task 01M5WC8PPWXB22788R17Y10DEB の 3 連続失敗）。
送る経路を残す設定は無い（`agent-docs/adr/2026-10-26-claude-source-drops-sampling-params.md`）。
`max_tokens` と `stop`（→ `stop_sequences`）はこれまでどおり写す。

### claude-oauth の名乗りと、ヘッダの無い 429（2026-10-26）

claude-oauth は上流要求の `system` を常に text block の配列にし、先頭 block を
`You are Claude Code, Anthropic's official CLI for Claude.` に固定する。client の system メッセージは
2 番目以降の block に元の順で続く。Anthropic は名乗りの無い OAuth 要求を `429 rate_limit_error "Error"`
（`anthropic-ratelimit-unified-*` ヘッダ無し）で拒むため。`anthropic-ratelimit-unified-status` の無い 429 は
throttling ではなく拒否（`refused`）として扱い、account に cooldown を付けず、次の候補へ倒し、client には
502 `upstream_refused` を返す（`agent-docs/adr/2026-10-26-ldr-proxy-and-route-fixes.md` A）。

### 一致しない path（2026-10-26）

`/v1/chat/completions`・`/v1/models`・`/v1/embeddings`・`/healthz` 以外の method/path は `warn` を出し、
`llm_proxy_requests` に `status = not_found`・`error_kind = unknown_route`・`requested_model = "<METHOD> <path>"`
の行を書いて 404（problem JSON）を返す。Responses API（`/v1/responses`）は実装しない。LangChain の
`ChatOpenAI` は model 名に `codex` を含むと Responses API を使うので、LDR・langmem の adapter は
`use_responses_api=False` で Chat Completions に固定する（同 ADR B）。
LDR の factory と構造化合成は proxy の bearer を `llm.openai_endpoint.api_key`（settings）→ 環境変数
`LDR_LLM_OPENAI_ENDPOINT_API_KEY`（`env_from_secrets`）の順に読み、`llm.openai_endpoint.url` があるのに key が
どちらにも無ければ LDR を呼ぶ前に exit 2 で止まる（[ADR 2026-10-26-ldr-proxy-bearer-from-env](../../agent-docs/adr/2026-10-26-ldr-proxy-bearer-from-env.md)）。

### 上流エラーの読み方（Phase 65b 追記）

上流が非 2xx を返したとき、この層は本文から `error.message`（OpenAI 互換の形）と、ChatGPT backend
がよく返す上位の `detail` / `message` フィールドの両方を見て、300 文字までの要約を作る
（`sources::codex::extract_error_summary`。トークン・ヘッダは絶対に含めない）。この要約は:

1. **WARN ログ**に出る（`llm-proxy: codex-oauth upstream returned a non-success status`。
   `status` と `summary` のフィールド）。本番でログを見れば、以前のように
   `candidate failed before any bytes were sent` だけでなく、実際に上流が何と言って拒否したかが
   分かる（例: `{"detail":"Store must be set to false"}` → 要約は `Store must be set to false`）。
2. **プロキシの応答本文**（`error.message`）にそのまま出る。`llm_proxy_requests.error_kind` は
   `upstream` のまま変えていない（本文を書く列は増やしていない。CLAUDE.md「migration は今回の
   Phase だけ」に沿って、要約はログと応答にだけ出す）。

### トークン更新

- **claude-oauth**: `.credentials.json` の `expiresAt`（unix ms）を見て、送信前に残り 60 秒を切っていれば
  先に更新する。401 を受けたら事後に 1 回だけ更新して再試行する。どちらも `refresh_token` の
  grant で `token_url`（既定 `https://console.anthropic.com/v1/oauth/token`。**この URL と
  client_id `9d1c250a-e61b-44d9-88ed-5944d1962f5e` は Claude Code CLI の実装から知られている値だが、
  このセッションでは実機で確認していない**）を叩き、同じファイルへ atomic に（`.tmp` + rename、
  mode 0600）書き戻す。
- **codex-oauth**: `auth.json` に有効期限に相当するフィールドが無い（ADR-0053 のフィクスチャどおり）ので、
  **事後（401 を受けてから）だけ**更新する（claude-oauth と違い、事前更新はしない）。
  `token_url = https://auth.openai.com/oauth/token`、`client_id = app_EMoamEEZ73f0CkXaXp7hrann`
  （ADR-0053 の指示にあった値）。
- 資格情報の値は**どのログ・応答にも出さない**（`ClaudeTokens`/`CodexTokens` は値を隠す `Debug` を持つ。
  自動テストで captured tracing subscriber を使い確認済み）。

## 3. 選択（決定的。ADR-0053 D1 / ADR-0049 の再利用）

要求の `model` は具体的な `<source_id>/<model_id>` だけを受ける（最初の `/` までが source ID。
`crates/llm-proxy/src/naming.rs`）。source ID は `claude_oauth`・`codex_oauth`・
`[[llm_proxy.sources.openai_compatible]]` の `id`。どの source/model を使うかは要求側（harness の
route 設定）が決め、proxy は source を跨いで選び直さない。

- `claude_oauth` / `codex_oauth`: その source のアカウントを残量スコアで順位付けする（ADR-0024 D3 の
  `evaluate`/スコア式。除外: 未ログイン・cooldown・5 時間枠/週次枠の枯渇）。同点は使用中の少ない方 → id 昇順。
- `openai_compatible`（例 `qwen`）: その id の中継先だけを使う。
- 使えるアカウント・中継先が無ければ 503 `no_source_available`。

**429/401 を受けたら**、そのアカウントに cooldown を付け（既存の帳簘。401 は `AuthFailed`、429 は
`Throttled`。claude-oauth の unified-status ヘッダの無い 429 は cooldown を付けない）、**まだバイトを送っていなければ**同じ要求を次の候補でやり直す。stream 開始後
（upstream から 200 を受けて bytes を転送し始めた後）に何か起きたら、やり直さずに切断するだけ
（OpenAI 形式のエラーフレームを送らない。ADR-0053: 「開始後は切断を返す」）。

選ばれた供給元・アカウントは応答ヘッダ `x-celeris-source` / `x-celeris-account` に出る。

## 4. 要求の記録（`llm_proxy_requests`。migration 0022）

`id` / `ts` / `source` / `account` / `requested_model` / `upstream_model` / `prompt_tokens` /
`completion_tokens` / `latency_ms` / `status`（`ok`/`error`/`unavailable`）/ `error_kind`。
**本文（プロンプト・応答）は書かない**。stream の要求は、ストリームが終わった時点でまとめて 1 行を書く
（先頭で「選んだ」ことは記録されるが、行そのものは完了後）。

`GET /llm/sources`（主 API、`/api/v1/llm/sources`。ADR-0053 D4）が、供給元ごとの到達性・アカウントの
残量・cooldown・直近 1 時間の要求/token 数を返し、GUI の「アカウント」画面の LLM source 節にも表示する。

## 5. 設定

`config/celeris.example.toml` の `[llm_proxy]` に worked example がある。要点:

```toml
[llm_proxy]
# enabled は省略可（source が 1 つでもあれば自動で有効）
listen = "127.0.0.1:18100"
prefer_free = true

[llm_proxy.sources.claude_oauth]
# accounts_dir を省略すると [accounts] claude_dir を使う

[llm_proxy.sources.codex_oauth]
# 同上（[accounts] codex_dir）

[[llm_proxy.sources.openai_compatible]]
id = "qwen"
base_url = "http://127.0.0.1:18000/v1"

```

## 6. 廃止済みの tier alias 設定

旧版の tier alias を使う proxy 設定例（旧 §§6–7）は、LLM source と model の
選択を分離した構成では使えない。tier alias は廃止済みで、具体的な `<source_id>/<model_id>` を書く。`[llm_proxy.models]` の tier alias と `[[providers]]` は新しい binary
で読み込みエラーになる。現行 config への移行は[移行手順](../ops/provider-llm-source-migration.md)を参照する。

## 8. 本番の運用手順

1. `[llm_proxy]` を有効化（`[llm_proxy.sources.claude_oauth]` / `codex_oauth` を追加するか、
   既存の Qwen 中継を `[[llm_proxy.sources.openai_compatible]]` として登録）。`POST /reload` では
   反映されない設定なので celeris を再起動する。
2. `curl -s -H "Authorization: Bearer $(cat ~/.config/celeris/api.token)" http://127.0.0.1:18100/healthz`
   → `{"status":"ok"}`。
3. `curl -s -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
     -d '{"model":"qwen/qwen3.8-27b","messages":[{"role":"user","content":"hi"}]}' \
     http://127.0.0.1:18100/v1/chat/completions` → 200 と応答本文。`x-celeris-source` ヘッダが
   `openai-compatible:qwen` であることを確認する（`GET /v1/models` は空の一覧を返す。model の正本は
   celeris の `[llm_sources.*].models`）。
4. **codex_oauth** を明示的に確認する:
   `curl -s -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
     -d '{"model":"codex_oauth/gpt-6-luna","messages":[{"role":"user","content":"hi"}]}' \
     http://127.0.0.1:18100/v1/chat/completions` → 200 と応答本文、`x-celeris-source: codex-oauth`。
   400 等が返ったら、celeris のログの `llm-proxy: codex-oauth upstream returned a non-success status`
   の `summary` フィールドに上流が言っている理由が出る（§「上流エラーの読み方」参照）。
5. proxy を使う harness（PaperQA・Local Deep Research・LangMem・opencode）の設定を**1 つずつ**
   proxy と具体的な `<source_id>/<model_id>` へ向け、そのつど 1 タスクを流して結果を確認する
   （同時に全部変えない）。LangMem を proxy へ向けるときは、`[knowledge.langmem].api_key_secret` を
   必ず設定すること（到達性 probe の `GET /v1/models` がこのトークンで `Authorization: Bearer` を送る。
   無いと 401 が返るが、401/403 は「落ちている」と誤認せず `Unknown`（＝そのまま `langmem` で走らせる）
   として扱うので、フォールバックし続けることはない）。

## 9. 明示した既知の制約（Phase 65 の範囲）

- マルチモーダル（画像等）は扱わない（テキストのみ）。
- streaming の tool call は Claude/Codex とも 1 つの引数チャンクをそのまま転送する簡略化（OpenAI の
  実際の増分ぶんまで細切れにする厳密な互換ではないが、最終的な組み立て結果は正しい）。
- `openai-compatible` の stream は生バイトの中継で、`model` フィールドだけ書き換える（usage の
  取り出しはしない。ログの `prompt_tokens`/`completion_tokens` は relay の stream では `null`）。
- アカウントの同時実行カウント（`IN_USE_PENALTY` の分母）は、このプロキシ内の同時要求だけで数える
  （CLI ワーカーの `in_use` とは別枠。cooldown・観測値は共有するが、公平性の計算は共有していない）。
- Qwen トンネルの操作は GUI のクラスタ画面、供給元の状態はアカウント画面で確認する。
