---
task: 01M4M8D4AP4YY4F9TV09HG30SJ
run: 01M4SRZZ8GM9KE31K5X9Z8YCVP
work_unit: worker-fixtures
status: partial
---

# crates-residue / worker-fixtures

## 変更（担当 file のみ）

- 試験 fixture の `celeris/{frontier,standard,cheap}` を具体的な source/model に置き換えた:
  - `celeris/standard`・`celeris/cheap` → `qwen/qwen3.8-27b`（`openai/` 前置きは `openai/qwen/qwen3.8-27b`、`celeris-proxy/` 前置きは `celeris-proxy/qwen/qwen3.8-27b`）
  - `celeris/frontier` → `opencode/opencode-go/grok-4.6`
- 対象: `acp/tests.rs`・`aider/tests.rs`・`langmem/tests.rs`・`local_deep_research/tests.rs`・`paperqa/tests.rs`・`paperqa_acquire.py`（例）・`paperqa_ask.py`（例）
- `paperqa.rs` L667 付近の doc 注釈を `qwen/qwen3.8-27b` の例に直した
- 試験名の tier 語を外した: `provider_kind_langmem_passes_proxy_model_to_stub`・`provider_kind_acp_sends_proxy_model_to_stub`・`with_model_sends_the_replacement_model_instead_of_the_configured_one`・`provider_kind_paperqa_query_model_uses_model_from_settings`
- `paperqa.rs` の `strip_provider_prefix` から `prefix != "celeris"` の例外と doc 行を削除（tier alias 専用の dead code。実在の model 名は変わらない）
- `paperqa/tests.rs` の `strip_provider_prefix("celeris/cheap")` の行を削除（上と対）

## 証拠

- `grep -rnE 'celeris/(frontier|standard|cheap)'` を担当 file（acp・aider・langmem・local_deep_research・paperqa 全体・paperqa*.py）に掛けた: 0 件
- `cargo test -p task-worker --lib paperqa`（TMPDIR=/local/celeris/data/scratch/cr-worker-fixtures）: `58 passed; 0 failed`
- `rustfmt --check --edition 2024` で変更 Rust file: exit 0
- `cargo test -p task-worker --no-fail-fast`（同 TMPDIR）: lib `1064 passed; 4 failed; 4 ignored`。その他の test binary は全て ok。
  - 失敗 3 件（担当の試験。原因は下の未解決事項 1）: `acp::tests::routing_context_ref_propagates_and_rejects_spoofing`・`acp::tests::the_task_tool_override_composes_with_the_routing_context_overlay`・`aider::tests::routing_context_ref_propagates_to_aider_proxy_settings`
  - 失敗 1 件（担当外）: `process_group::tests::a_container_is_stopped_even_when_the_process_group_is_already_gone`。単独実行では `1 passed`。並列負荷下の待ち時間（25ms × 200 の polling）が足りない flake。

## 未解決事項

1. **acceptance 1（`cargo test -p task-worker` が通る）はこの WU 単独では未達**。上の 3 件は `routing_context_transport.rs:81`（`celeris/{frontier,standard,cheap}` のときだけ routing context header を付ける）と `aider` の同種判定に依る。model 名を新形にした以上、判定が tier 名のままだと header が付かず落ちる。この判定の撤去（接続先か `routing_context_ref` の有無で決める）は兄弟 WU（worker-route）の担当。worker-route と統合された後に `cargo test -p task-worker` を再実行して確かめること。3 件を tier 名に戻すのは、tier 判定を消すという目的に反するため行っていない。
2. **`paperqa.rs` の `strip_provider_prefix` の危険（要判断・挙動は変えていない）**: 小文字の最初の区画を LiteLLM の接頭辞とみなして剥がすため、`<source_id>/<model_id>` 形（例 `qwen/qwen3.8-27b`）を `config.model` や settings の `llm` に接頭辞なしで書くと `qwen3.8-27b` に剥がされる。`query_llm_model` は `config.model` にも剥がしを掛けるので、run の model が新形のとき検索語の LLM 先を誤るおそれがある。これは旧 tier 例外とは別の、もともとの規則。paperqa の判断（剥がしを `openai/` 等の既知の接頭辞に限るか）が要る。
3. `pi/tests.rs`・`tool_policy.rs`・`pi.rs`・`routing_context_transport.rs` は兄弟 WU（worker-route）の担当なので触っていない。`pi/tests.rs` と `tool_policy.rs` には `celeris/<tier>` が残っている（worker-route が直す）。
4. `process_group` の flake（上の担当外の失敗）は別の run で扱う。
5. 本番の `[llm_proxy.models]` 等の旧 tier 設定は今回の範囲外。
