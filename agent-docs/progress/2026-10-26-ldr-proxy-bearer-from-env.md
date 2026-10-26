---
title: LDR runner が proxy の bearer を env から読まず全 run が 401 になる回帰の修正
tasks: [01M5WTKE9M2VSDBBR4F3MZF0T4]
status: done
updated: 2026-10-26
---
# LDR runner が proxy の bearer を env から読まず全 run が 401 になる回帰の修正

完了日: 2026-10-26

判断は [ADR 2026-10-26-ldr-proxy-bearer-from-env](../adr/2026-10-26-ldr-proxy-bearer-from-env.md)。発端は task
01M5WC8PPWXB22788R17Y10DEB の LDR run が release a9ff7a2bc294 で全部 llm-proxy の 401 になったこと。

## 変更

- `crates/task-worker/src/local_deep_research_run.py`: `openai_endpoint_api_key(settings)`（settings → env
  `LDR_LLM_OPENAI_ENDPOINT_API_KEY`、URL があって key が無ければ `MissingApiKeyError`）を追加し、`chat_completions_llms` の
  factory と構造化合成の `synth_api_key` の両方で使う。`"not-needed"` の fallback を削除。`main()` は LDR を呼ぶ前に検査し、
  key が無ければ exit 2。
- `crates/task-worker/src/local_deep_research/tests.rs`: 試験 2 本を追加。既存の
  `main_applies_structured_synthesis_when_targets_are_present` は URL があって key が無い入力だったので settings に偽の key を足した。
- `docs/guides/llm-source.md`: key の読み順の 1 段落。

## 他の python runner の確認（受け入れ条件 1）

| runner | key の読み方 | 同種の欠陥 |
|---|---|---|
| `paperqa_acquire.py` | `config.api_key`（`<env:...>` 解決）→ env `OPENAI_API_KEY` | 無し（env を見る） |
| `paperqa_ask.py` | key を自分で扱わない。litellm が env `OPENAI_API_KEY` を読む | 無し |
| `langmem_run.py` | `llm.api_key` → env `OPENAI_API_KEY`/`ANTHROPIC_API_KEY` | 無し |

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `cargo test -p task-worker --lib local_deep_research`: 51 passed。
- 回帰の確認: runner を基準の版に戻すと `env_only_proxy_bearer_reaches_a_bearer_checking_endpoint` が落ちる（偽サーバの 401 で
  adapter が失敗）。戻すと通る。
- `bash scripts/dev/test-affected.sh` / `cargo clippy --workspace -- -D warnings` / `cargo fmt --all -- --check`: 結果は報告本文。

## 本番への移行

不要。`[adapters.local_deep_research] env_from_secrets` の `LDR_LLM_OPENAI_ENDPOINT_API_KEY` がそのまま使われる。

## 未解決事項

- 無し。release 後に operator が 01M5WC8PPWXB22788R17Y10DEB を開き直して確かめる。

## 提案

- 無し。
