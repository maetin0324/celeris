---
title: llm-proxy と route 選択の 3 不具合（Claude の名乗り・codex の Responses API・qwen の暗黙アカウント）
tasks: [01M5WMNDA8AM0NTC3NXSX16NVB]
status: done
updated: 2026-10-26
---
# llm-proxy と route 選択の 3 不具合（Claude の名乗り・codex の Responses API・qwen の暗黙アカウント）

完了日: 2026-10-26

判断は [ADR 2026-10-26-ldr-proxy-and-route-fixes](../adr/2026-10-26-ldr-proxy-and-route-fixes.md)。
発端は Web 調査 task 01M5WC8PPWXB22788R17Y10DEB（local-deep-research）が release ca6f52cb91a6 で 3 回とも落ちたこと。

## 変更

- A（Claude の名乗り）: `crates/llm-proxy/src/sources/claude.rs` の `to_anthropic_body` が `system` を block 配列にし、
  先頭 block を `CLAUDE_CODE_IDENTITY` に固定、client の system を後ろに元の順で続ける。`anthropic-ratelimit-unified-status`
  の無い 429 は新しい `SourceError::Refused` → `FailureClass::Refused`（cooldown 無し・breaker 無し・次の候補へ倒す・
  client へ 502 `upstream_refused`）。`server.rs` の `record_failure` は `Unauthorized`/`RateLimited` だけを帳簿に書く。
- B（Chat Completions 固定）: `local_deep_research_run.py` が LDR の公開引数 `llms` に `openai_endpoint` の factory を渡し、
  `ChatOpenAI(use_responses_api=False)` を作る（`llm.provider` の設定はそのまま）。`langmem_run.py` の `build_chat_model` も
  `use_responses_api=False`。PaperQA は litellm で `ChatOpenAI` を作らないので対象外。proxy の router に fallback
  handler `unknown_route`（warn と `llm_proxy_requests` の `not_found`/`unknown_route` 行、404 problem JSON）。
- C（暗黙アカウント）: `task_dispatch::source_account_pool` を新設し、dispatcher の `route_state` と
  `celerisctl worker` の `route_state` の両方がこれでプールを作る（写しを 1 つにまとめた）。account adapter の無い source で
  `accounts` が空なら `[<source id>]`。
- 文書: `docs/guides/llm-source.md` に名乗り・ヘッダの無い 429・一致しない path の節。

## 試験（追加・変更）

- `llm-proxy`: `sources::claude::tests::system_blocks_start_with_the_claude_code_identity_and_keep_client_order`、
  `system_identity_is_sent_even_without_a_client_system_message`、`fallback_tests` の `Refused` 分類、
  `proxy_integration.rs` の `claude_429_without_unified_status_is_refused_without_a_cooldown`（両 account に cooldown 無し・
  502・2 account を試す）、`unknown_route_is_recorded_in_the_request_log_and_returns_404`（認証無しは 401 で記録しない）。
  偽の Anthropic 上流は名乗りの無い要求をヘッダ無しの 429 で拒むようにし、既存の 429 試験は unified-status ヘッダ付きにした。
- `task-worker`: `runner_registers_a_chat_completions_factory_for_openai_endpoint`（codex の model 名で
  `use_responses_api: false`）、`langmem::tests::build_chat_model_pins_chat_completions_for_a_codex_model_name`。
- `task-dispatch`: `route_select_offers_an_undeclared_pool_source_its_implicit_account`、`source_account_pool_rules`。
- `celerisctl`: `worker_tests.rs` の `route_config` から qwen の `accounts` を消し（本番と同じ形）、
  `worker_route_cli_openai_compatible_source_carries_its_endpoint` が暗黙アカウント `qwen` を選ぶことを確かめる。
- 手での確認（本物の LDR venv `/local/celeris/state/tools/ldr/.venv`、langchain_openai 1.6.2、loopback の偽 server）:
  adapter の factory を LDR の `register_llm` に登録し `get_llm(provider="openai_endpoint", model_name="codex_oauth/gpt-5.5-codex")`
  で invoke → 届いた path は `/v1/chat/completions` のみ。素の `ChatOpenAI(model="gpt-5.5-codex")` は
  `_use_responses_api({}) == True`（直す前の挙動の再現）。

## 検査（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

結果は報告本文に書く（`bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、
`cargo fmt --all -- --check`）。

## 本番への移行

- config の変更は無い（`[llm_sources.qwen]` に `accounts` を足さない。LDR の `llm.provider = "openai_endpoint"` もそのまま）。
- release と昇格の後に operator が行う確認:
  1. task 01M5WC8PPWXB22788R17Y10DEB を reopen する（`celerisctl` か web の task 画面）。
  2. 選ばれた route を events で見る: `llm_route_selected` の `source_id`・`model_id`・`account_id`（qwen なら
     `account_id = "qwen"`。`AccountUnavailable` の除外が qwen に付いていないこと）。
  3. proxy の記録を読み取り専用で見る:
     `sqlite3 -readonly <celeris DB> "SELECT ts, source, account, requested_model, status, error_kind FROM llm_proxy_requests ORDER BY ts DESC LIMIT 20"`。
     claude なら `source = claude-oauth`・`status = ok`、codex なら `codex-oauth` で `/v1/chat/completions` 経由（`unknown_route` の行が無い）、
     qwen なら `openai-compatible:qwen`。
  4. claude-oauth の account に偽の `Throttled` cooldown が付いていないこと（アカウント画面か `GET /api/v1/llm/accounts`）。
     既に付いている古い偽 cooldown は期限で消える（手で消す必要は無い）。
  5. task が done になり、どの source で通ったかを task のコメントに残す。

## 未解決事項

- 名乗り付きの要求が本番の Anthropic で 200 になることは operator の最小要求（5h-utilization 0.38）でしか確かめていない。
  昇格後の reopen で確かめる。
- PaperQA（litellm）が codex の model 名で Responses API へ切り替わるかは未確認。切り替わっても proxy の
  `unknown_route` 行で見えるようになった。

## 提案

- codex source の 429 も unified ヘッダに当たる印で throttling と拒否を分けられるかを調べる（今回は対象外）。
