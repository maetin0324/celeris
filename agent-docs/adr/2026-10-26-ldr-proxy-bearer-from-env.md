# LDR runner は proxy の bearer を settings → env の順に読み、無ければ起動時に止まる

- 日付: 2026-10-26
- 状態: 採用・実装済み
- task: 01M5WTKE9M2VSDBBR4F3MZF0T4
- 関連: [ADR 2026-10-26 ldr-proxy-and-route-fixes](2026-10-26-ldr-proxy-and-route-fixes.md) B1（回帰の元）、
  [ADR-0063](0063-research-tasks-resilience.md) D4（秘密の env 渡し）・Phase 109c B3（構造化合成）、
  [ADR-0029](0029-web-research-harness.md)（LDR）

## 背景

ADR 2026-10-26-ldr-proxy-and-route-fixes B1 で `local_deep_research_run.py` が LDR 組み込みの `openai_endpoint` provider を
自前の factory（`chat_completions_llms`）に置き換えた。factory は key を `settings["llm.openai_endpoint.api_key"]` からだけ読み、
無ければ `"not-needed"` を送った。本番は proxy の bearer を settings ではなく `[adapters.local_deep_research] env_from_secrets`
の `LDR_LLM_OPENAI_ENDPOINT_API_KEY` で渡す（組み込み provider は LDR の settings snapshot が `LDR_` 接頭の env 上書きを
読むので届いていた）。このため release a9ff7a2bc294 で LDR の run が source に関係なく全部 llm-proxy の 401
（missing or invalid bearer token）で落ちた。構造化合成の `synth_api_key` も settings しか見ていなかった（以前からの欠陥。
合成は best-effort なので 401 でも黙って省かれていた）。

## 決定

- D1. key の解決を `openai_endpoint_api_key(settings)` 1 つにまとめ、factory と構造化合成の両方が使う。順は
  `settings["llm.openai_endpoint.api_key"]`（`"<env:...>"` の解決後）→ 環境変数 `LDR_LLM_OPENAI_ENDPOINT_API_KEY`。
- D2. `"not-needed"` への黙った fallback を消す。`llm.openai_endpoint.url` があり key がどちらにも無ければ
  `MissingApiKeyError`。`main()` は LDR を import・呼び出す前に解決を試し、失敗なら `llm: ...`（env 名を含み、値は出さない）
  を stderr に出して exit 2。celeris では URL は llm-proxy を向くので「proxy を向いているとき」を「URL があるとき」で判定する
  （鍵の要らない中継を使うなら `api_key = "unused"` のように明示する。ADR-0029 当時の設定例と同じ）。URL が無ければ `None`。
- D3. 他の python runner は同じ形の欠陥が無いので変えない: `paperqa_acquire.py` は placeholder → `OPENAI_API_KEY` の順、
  `paperqa_ask.py` は litellm が `OPENAI_API_KEY` を env から読む、`langmem_run.py` は `api_key` → `OPENAI_API_KEY`/`ANTHROPIC_API_KEY`。

## 結果

- 本番 config の変更は要らない（env_from_secrets の渡し方のまま届く）。
- 試験: `runner_reads_the_proxy_bearer_from_env_for_the_factory_and_the_synthesis`（python 側）、
  `env_only_proxy_bearer_reaches_a_bearer_checking_endpoint`（adapter を python3 と偽 LDR で実行し、bearer を要求する偽サーバへ）。
