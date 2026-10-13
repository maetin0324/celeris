---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: "config-drop: 旧 provider 設定（[[providers]]・providers.d）と SourceOrigin を消す"
updated: 2026-10-11
---
# config-drop: 旧 provider 設定（[[providers]]・providers.d）と SourceOrigin を消す

ADR 2026-10-11 llm-source-harness-selection と breaking-changes-by-default に従い、source+harness の固定対だった provider 設定を削除した。経路はすでに `[llm_sources.<id>]`・`[harnesses.<id>]`・`[route_defaults.<harness>]` で決まっており、この unit は旧い読み込み経路と試験 fixture を消しただけ。

## 消したもの

- `crates/celeris/src/config/providers.rs`（ファイルごと）: provider 行の struct、`providers_include` の glob 解決と `providers.d/*.toml` の読み込み、行ごとの設定解決、provider 種別の互換警告、provider 検証、`account_pool_providers`・`account_pool_adapters`・`provider_llm_sources`・`provider_lanes`・`provider_llm_source`・`provider_kind`。
- `crates/celeris/src/config/mod.rs`: `mod providers` と re-export、`Config.providers`・`Config.providers_include`・`Config.providers_dir`、`Config::load` の include 読み込み・設定解決・警告ループ、`validate` の provider 検証。
- `crates/celeris/src/config/fixtures/provider_kind_legacy_production.toml`（旧 provider 推論試験の fixture）。
- `crates/task-core/src/provider_source.rs`: `enum SourceOrigin`、`struct ResolvedLlmSource`（`origin` を消すと使う所が残らないので struct ごと）、`enum ProviderKind`（使う所が残らない）。`crates/task-core/src/lib.rs` の re-export は `LlmSourceRef` だけにした。
- lane ごとのモデル束縛欄への言及: `task-core/src/model_catalog/assignments.rs`（2 か所）と `task-dispatch/src/dispatcher.rs`（1 か所）の comment を、その識別子を使わない文言にした。`crates/celeris/src/daemon/cos_reload_tests.rs` の `[providers.<lane のモデル束縛>.*]` 表を消した。
- 試験 config 文字列の `[[providers]]` block: script（`[[providers]]` から続くキー行までを消す）で一括に消した。crates/ と tests/e2e の試験。残っているのは `crates/celeris/src/config/tests.rs` の legacy 拒否試験だけ。
- 読み込みに provider 行が要った名残: `crates/celerisctl/tests/worker_run.rs` の `PROVIDER_ROW` 定数と `write_config` の `extra_toml` 引数、`crates/celeris/tests/instance_handoff.rs` の「`[[providers]]` は読み込みがまだ 1 行を要求する」comment と `verify.providers` の assert、`crates/celerisctl/src/commands/worker_tests.rs` の `Config` struct literal の 3 欄、`tests/e2e/tests/multi_account_scenarios.rs` の sentinel 行と comment、`tests/e2e/tests/api_scenarios.rs` の `write_config` の `provider_env` 引数（provider 行の `env` 専用）と、その秘密値の漏れ検査 1 項目。
- `[[providers]]` を指していた doc comment（`task-worker` の acp.rs・paperqa.rs・paperqa/tests.rs、`task-ops/src/daemon.rs`、`celeris/examples/seed_hello_crate_task.rs`、`celeris/src/config/harness.rs`）を新しい形の言い方に直した。`task-ops` の `ImplicitAccountLive` の doc comment が schema の description に出るので、`UPDATE_SCHEMA=1` で `docs/api/v1/api-v1.schema.json` を更新した（その 1 行だけの差分）。

## 残したもの

- `Config::parse_with_delivery` が TOML の `providers`（`[[providers]]`・`[providers.d]`）と `providers_include` を読み込みエラーにする。エラーは `[llm_sources.<id>]`・`[harnesses.<id>]`・`[route_defaults.<harness>]` の書き方を示す。試験 `config::tests::legacy_providers_rejected_with_new_config_guidance`。
- `crates/celeris-credentiald` の `providers`（credential provider。別の概念）には触れていない。

## 消した試験（旧 provider 設定だけを試していたもの）

`crates/celeris/src/config/tests.rs`:
- `provider_kind_legacy_production_inference_warnings_and_cheap_tier`
- `provider_kind_explicit_source_rejects_adapter_model_and_missing_reference`
- `provider_kind_qwen_id_without_model_does_not_guess_source`
- `provider_kind_qwen_direct_acp_without_opencode_config_is_cheap_only`
- `provider_kind_qwen_direct_acp_source_reference_is_cheap_only`
- `provider_kind_qwen_direct_acp_proxy_model_keeps_all_tiers`
- `rejects_unknown_adapter_and_missing_providers`
- `command_and_args_are_only_allowed_on_acp_providers_and_override_per_row`
- `rejects_row_level_settings_field_for_local_deep_research_provider`
- `settings_is_only_allowed_on_paperqa_providers_and_overrides_per_row`
- `multi_account_providers_parse_and_duplicate_ids_are_rejected`
- `providers_include_merges_files_in_filename_order_and_still_rejects_duplicate_ids`
- `providers_include_rejects_patterns_not_ending_in_glob_toml`
- `providers_include_with_missing_directory_is_empty_not_an_error`
- `account_pool_requires_claude_code_adapter_and_accounts_section`
- `account_pool_for_codex_requires_codex_dir_specifically`
- `opencode_go_pool_row_requires_opencode_dir`
- `legacy_provider_concurrency_is_optional_and_warns_including_zero_and_include`
- `pi_adapter_config_rejects_missing_hashline_and_tools_on_other_adapters`（provider 行の `extensions`/`tools` 欄の検証）

## 書き直した試験

- `crates/celeris/src/lib/tests.rs` `unknown_adapter_message_lists_langmem`: 未知 adapter を provider 行でなく `[[harnesses]] adapter = "bogus"` で与える。
- `crates/celeris/src/config/tests.rs` の部分削除: `accepts_acp_adapter_with_default_config`・`accepts_paperqa_adapter_with_default_config`・`paperqa_paths_are_resolved_relative_to_the_config_file` から provider 行の assert を、`env_from_secrets_is_parsed_on_adapters_and_providers` から行側の assert を消し、名前を `env_from_secrets_is_parsed_on_adapters` にした。`let providers = ...` を足して `format!` していた試験は空文字列の直書きにした（clippy の `useless_format`・`unnecessary_to_owned` を `cargo clippy --fix` で直した）。
- `crates/celerisctl/tests/worker_run.rs`・`tests/e2e/tests/api_scenarios.rs`: 上記の引数削除。
- そのほかの試験は `[[providers]]` block を消すだけで通った（新形への書き換えが要った試験は無い）。

## 実行したコマンドと結果

すべて `CARGO_TARGET_DIR=/local/celeris/data/scratch/opctl-configdrop/target`（ローカル）。

- `cargo test -p celeris legacy_providers_rejected_` → `test result: ok. 1 passed; 0 failed; ... 380 filtered out`（exit 0）
- `! test -e crates/celeris/src/config/providers.rs && ! grep -rnwE 'ProviderConfig|SourceOrigin|tier_models' crates --include=*.rs` → exit 0
- `bash scripts/dev/test-parallel.sh -E 'package(celeris) or package(celerisctl) or package(task-core) or package(task-dispatch) or package(task-worker)'` → exit 0、`3515 tests run: 3515 passed, 10 skipped`、summary `passed 3518 failed 0`（doc-test 込み）
- `CELERIS_TEST_SKIP_DOC=1 bash scripts/dev/test-parallel.sh -E 'package(e2e)'` → exit 0、`45 tests run: 45 passed`
- `CELERIS_TEST_SKIP_DOC=1 bash scripts/dev/test-parallel.sh -E 'package(task-api) or package(task-ops)'` → exit 0、`1134 tests run: 1134 passed, 3 skipped`
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `UPDATE_SCHEMA=1 cargo test -p task-api schema` の後 `cargo test -p task-core -p task-api -p task-worker schema_matches` → 1 + 8 + 1 passed（exit 0）
- 変更 path は crates/・tests/e2e/・docs/api/v1/・agent-docs/progress/ の中だけ（`git status --short` で確認）。

## config-drop WorkUnit での再確認（2026-10-11）

- `cargo test -p celeris legacy_providers_rejected_` → 1 passed（exit 0）。
- `! test -e crates/celeris/src/config/providers.rs && ! grep -rnwE 'ProviderConfig|SourceOrigin|tier_models' crates --include='*.rs' && sh "$CELERIS_WU_SCOPE_PATHS" && git diff --check` → exit 0。
- 初回の package suite は browser doctor / celerisctl browser socket 試験が長い `TMPDIR` の UNIX socket path で `SUN_LEN` 超過した。`TMPDIR=/tmp CELERIS_TEST_SKIP_DOC=1 bash scripts/dev/test-parallel.sh -E 'package(celeris) or package(celerisctl) or package(task-core) or package(task-dispatch) or package(task-worker)'` の再実行は 3515 passed、10 skipped（exit 0）。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。

## 未解決

- `web/api/generated/schema.json`・`types.ts` は HEAD の時点で `docs/api/v1/api-v1.schema.json` と既にずれている（`ProviderView`・`ResolvedLlmSource` などが残る。前の unit の API 削除分）。web の生成と `web/features/ops/providers-llm-source.ts` は scope 外なので触っていない。closeout で `npm run gen:types` と web 側の削除が要る。
- `task-api/src/admin.rs` などに `providers.d` への言及（管理 API の旧説明）が残る。config 外で、この unit の範囲外。
- README・旧 ADR の `[[providers]]` 記述は closeout の担当。

## 本番への移行

- 本番の `~/.config/celeris/config.toml` に `[[providers]]`・`[providers.*]`・`providers_include` が 1 つでも残っていると、この版の celeris は設定の読み込みで止まる（エラーは `[llm_sources.<id>]`・`[harnesses.<id>]`・`[route_defaults.<harness>]` を示す）。deploy の前にこれらを消し、経路を `[llm_sources.<id>]`・`[harnesses.<id>]`・`[route_defaults.<harness>]` で宣言しておくこと。
- `providers.d/` ディレクトリはもう読まれない。中身（アカウントごとの env・credential の場所）は消さずに残し、必要な値は `[llm_sources.<id>]` 側へ移す（credential は消さない）。
