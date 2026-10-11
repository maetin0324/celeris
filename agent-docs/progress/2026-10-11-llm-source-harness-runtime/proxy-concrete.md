---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: proxy-concrete — llm-proxy の tier alias を消し具体 source/model にする
updated: 2026-10-11
---

# proxy-concrete — llm-proxy の tier alias を消し具体 source/model にする

## 変更
- `crates/llm-proxy/src/config.rs`: `ModelsConfig`（`[llm_proxy.models]` の tier → model 写像と既定値）を削除。欄が残れば `MODELS_REMOVED_MESSAGE`（`<source_id>/<model_id>` の例を示す）で読み込みエラー。
- `crates/llm-proxy/src/naming.rs`: 要求は `<source_id>/<model_id>` だけ（source ID = `claude_oauth`・`codex_oauth`・openai_compatible の `id`、最初の `/` まで）。`celeris/<tier>`・`claude/<tier>`・`gpt/<tier>`・`qwen/cheap`・`claude:<model>` は受けない。
- `server.rs`: 候補は名指しの source だけ（OAuth はそのプールのアカウント順、relay はその id）。設定に無い source は 422 `unknown_source`。tier 解決（lane deployment・cross-pool 比較）、role assignment reader（`with_role_assignments`）、`/v1/models` の tier 一覧（空の一覧を返す）を削除。
- `selection.rs`: lane deployment 選択・cross-pool 選択・Qwen tier 写像を削除。`sources_view.rs`: tier 解決先 view を削除（`celeris` daemon api は `celeris_tiers: []`）。
- `legacy_catalog.rs`: proxy 設定由来の tier deployment を削除（routing catalog の lane deployment は DB の role assignment だけから作る）。`celeris` の `model_routing.rs` の proxy 由来 deployment の取り込みを削除。
- `celeris` config: `providers.rs` の proxy 行のローカル判定（`[llm_proxy.models.qwen].cheap` 前提）を削除、`knowledge.rs` の例を `qwen/qwen3.8-27b` に、fixture から `[llm_proxy.models.qwen]` を削除。
- 試験: 追加 `proxy_tier_alias_rejected_*`（llm-proxy config・celeris config）、`proxy_route_*`（naming 2・proxy_integration 3: tier alias/未知 source の 422、relay 名指しで別 source に倒れない、具体 model がそのまま上流へ、/v1/models に tier alias が無い）。既存の要求名を具体形へ書き換え。削除（tier 候補集合が前提）: `llm-proxy/tests/{proxy_fallback,estimator_shadow,proxy_shadow}.rs`、`celeris/tests/{model_role_assignments_consistency,routing_estimator_shadow}.rs`、`celeris/src/daemon/routing_sidecar_tests.rs`、proxy_integration の cheap_only_* 5 件、selection の cross-pool/legacy 等価試験。

## 実行したコマンドと結果
- `cargo test -p llm-proxy`: lib 56 passed、proxy_integration 24 passed、estimator_sidecar 2、proxy_routing_context 2 — 全部 ok。
- `cargo nextest run -p celeris -p llm-proxy --no-fail-fast`: 599 run, 597 passed, 2 failed。失敗 2 件は `browser_doctor::tests::*`（`path must be shorter than SUN_LEN`。run の TMPDIR が長いことによる環境要因で、この変更と無関係）。
- workspace の test-parallel・clippy は統合（integrate-edges）で見る。

## 未解決
- `task-api` の `LlmCelerisTierView`・`celeris_tiers` 欄と web/gui の表示は残っている（daemon は空配列を返す）。schema と web/gui の providers 画面を触るので api-remove で消すこと。
- proxy の decision shadow / estimator shadow は lane を tier 名から得ていた。具体要求では lane が "explicit" で候補は 1 つになり、estimator shadow は proxy から呼ばれない。routing policy の続き（01M4M8D4AQ）で消すか lane の入力を決める。
- 要求内の fallback は同じ OAuth source のアカウント間だけになった。fallback budget/breaker の結合試験（旧 proxy_fallback.rs は複数 relay 前提）は単体試験 `fallback_tests.rs` だけが残る。OAuth 複数アカウントでの結合試験を足す余地あり。
- task-worker の `routing_context_transport.rs`・`pi.rs` は `celeris/<tier>` の名前で proxy 経由を判定している（並行 WU worker-harness の範囲なので触っていない）。hint-remove か closeout で具体名の判定に替えること。
- role assignment（tier → model の DB 割り当て）は dispatcher 側にまだある（dispatch-prune の範囲）。

## 本番への移行
- 本番 config から `[llm_proxy.models]`（`.claude`・`.gpt`・`.qwen`）を消す（残すと起動エラー）。
- `[knowledge.langmem] model`、LDR の provider 行の `model`/`settings."llm.model"`、paperqa の settings の `llm` を具体名に: 例 `qwen/qwen3.8-27b`（openai_compatible の id が `qwen` の場合）、`claude_oauth/claude-sonnet-5`、`codex_oauth/gpt-6-luna`。paperqa（litellm）は `openai/qwen/qwen3.8-27b` の形。
- opencode 等の proxy 利用設定の `celeris/<tier>` も同じく具体名へ。

## 提案
- `[[llm_proxy.sources.openai_compatible]]` の `id` に `claude_oauth`・`codex_oauth` を禁じる検証を celeris の `config/proxy.rs` に足す（今は名前が衝突すると OAuth 側が優先される）。
