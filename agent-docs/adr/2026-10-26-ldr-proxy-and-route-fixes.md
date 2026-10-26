# llm-proxy の Claude 名乗り・未知 path の記録・Chat Completions 固定と、プールの無い source の暗黙アカウント

- 日付: 2026-10-26
- 状態: 採用・実装済み
- task: 01M5WMNDA8AM0NTC3NXSX16NVB
- 関連: [ADR-0053](0053-llm-source-proxy.md)（LLM source proxy）、[ADR-0029](0029-web-research-harness.md)（LDR）、
  [ADR 2026-10-08 account-concurrency-limits](2026-10-08-account-concurrency-limits.md) §(e)（暗黙アカウント）、
  [ADR 2026-10-11 llm-source-harness-selection](2026-10-11-llm-source-harness-selection.md)、
  [ADR 2026-10-26 claude-source-drops-sampling-params](2026-10-26-claude-source-drops-sampling-params.md)、
  [破壊的変更の方針](2026-10-11-breaking-changes-by-default.md)

## 背景

2026-10-26、Web 調査 task 01M5WC8PPWXB22788R17Y10DEB（harness local-deep-research）が release ca6f52cb91a6 で
3 回とも落ちた。route_defaults の候補 qwen・claude・codex のどれも動かなかった。operator の調査で原因が 3 つ分かった。

- A. claude source（`crates/llm-proxy/src/sources/claude.rs`）の上流要求に Claude Code の名乗りが無い。Anthropic は
  名乗りの無い OAuth 要求を `429 rate_limit_error "Error"`（`anthropic-ratelimit-unified-*` ヘッダ無し）で拒否する。
  名乗りを system に入れた最小要求は 200。proxy はこの 429 を throttling（`RateLimited`）と見なし、Claude Code
  ワーカーと共有の account 帳簿に偽の `Throttled` cooldown を書いていた。
- B. LDR の `langchain_openai` 1.6.2 は model 名に `codex` を含むと Responses API（`POST /v1/responses`）を使う。
  proxy の router に `/v1/responses` も fallback も無く、axum が本文の空の 404 を返し、journal にも
  `llm_proxy_requests` にも何も残らなかった。
- C. `[llm_sources.qwen]` に `accounts` が無い。dispatcher の route 選択（`route_select.rs`）は account プールの無い
  source の候補を作らず、qwen は常に `AccountUnavailable` で外れていた。§(e)（プールの無い source は暗黙の 1 アカウント）
  の route 選択側が未実装だった。`celerisctl worker` の写しも同じ。

## 決定

### A. claude source の名乗りと、ヘッダの無い 429

- A1. `to_anthropic_body` は `system` を常に text block の配列にする。先頭 block は固定の
  `You are Claude Code, Anthropic's official CLI for Claude.`、client の system message は 2 番目以降の block に
  元の順で続ける（1 message = 1 block。空の message は block にしない）。client が system を送らなくても先頭 block は付く。
  文字列の `system` を送る経路は消す。
- A2. 上流の 429 は `anthropic-ratelimit-unified-status` ヘッダがあるときだけ `SourceError::RateLimited`（throttling）。
  ヘッダが無い 429 は新しい `SourceError::Refused`（要求そのものを上流が拒んだ。本文の要約を持つ）にする。
- A3. `Refused` の分類は `FailureClass::Refused`。account の cooldown を付けない（`record_failure` は
  `Unauthorized` と `RateLimited` だけを帳簿に書く）、breaker に数えない、次の候補へは倒す（他の source なら通る
  見込みがある。上限は `rate_limited` と同じ 3）。client への応答は `502 upstream_refused`（429 を返して client の
  再試行を誘わない）。request log の `error_kind` は `refused`、fallback reason は `refused`。
- A4. codex source の 429 は今回の対象外（codex の上流は unified ヘッダを持たない）。

### B. Chat Completions への固定と、未知の path の記録

- B1. LDR adapter（`crates/task-worker/src/local_deep_research_run.py`）は LDR の公開引数 `llms` で
  `openai_endpoint` という名前の factory を登録し、LDR の組み込み `openai_endpoint` provider より先に使わせる
  （LDR の `get_llm` は登録済みの名前を組み込みより先に引く）。factory は `ChatOpenAI` に公開引数
  `use_responses_api=False` を渡し、model 名に依らず `POST /v1/chat/completions` を使う。endpoint・api key・model は
  adapter が受け取った設定（`llm.openai_endpoint.url`・`llm.openai_endpoint.api_key`・`llm.model`）から取り、
  `llm.max_tokens`・`llm.max_retries`・`llm.request_timeout` があれば写す。本番 config の `llm.provider =
  "openai_endpoint"` はそのまま。private API の差し替え（`_use_responses_api` の上書き等）はしない。
- B2. langmem adapter（`langmem_run.py` の `build_chat_model`）も `ChatOpenAI` を直接作る同じ形なので
  `use_responses_api=False` を渡す。PaperQA adapter は litellm を使い `ChatOpenAI` を作らないので同じ形ではなく、変えない。
- B3. proxy の router に fallback handler を置く。どの route にも一致しない method/path は `warn` を出し、
  `llm_proxy_requests` に `status = "not_found"`・`error_kind = "unknown_route"`・`model = "<METHOD> <path>"` の行を
  書いて `404 not_found`（problem JSON）を返す。guard（bearer・standby）の後に動くので、認証の無い要求は記録しない。

### C. プールの無い source の暗黙アカウント（route 選択）

- C1. account adapter を持たない source（`account_adapter_for_source_kind` が `None`）で `accounts` が空なら、
  その source の account プールは `[<source id>]`（暗黙の 1 アカウント。§(e) の `implicit/<source id>` と同じ id）とする。
  `accounts` を書いた source はこれまでどおりその値を使う。
- C2. dispatcher（`route_select.rs` の `route_state`）と `celerisctl worker`（`commands/worker.rs` の `route_state`）は
  同じ関数 `task_dispatch::source_account_pool` でプールを作る（OAuth source の「宣言を logged-in で絞る」規則も
  同じ関数に入れる）。写しを 2 つ持たない。
- C3. 本番 config は変えない（`[llm_sources.qwen]` に `accounts` を足さずに直る）。

## 影響

- Claude の OAuth 要求は名乗りの分だけ入力 token が増える（数十 token）。
- 名乗りの無い 429 で Claude Code ワーカーの account が冷えることは無くなる。真の throttling（unified ヘッダ付き）は従来どおり。
- LDR・langmem は codex の model 名でも Chat Completions を使う。proxy は `/v1/responses` を実装しない。
- qwen が route 選択の候補に戻る。暗黙アカウントの同時実行の上限は §(e) のまま。
- 本番への反映は release が要る。確認手順は進捗
  [2026-10-26-ldr-proxy-and-route-fixes](../progress/2026-10-26-ldr-proxy-and-route-fixes.md) に書く。
