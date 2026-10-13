---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: complete
title: config の provider 由来 deployment・検証を除去（catalog-unprovider）
updated: 2026-10-11
---
# config の provider 由来 deployment・検証を除去（catalog-unprovider）

## 変更

- `config/model_routing.rs`: `routing_catalog` が provider 行の `model`・lane ごとの model 割り当てから deployment と model profile を作る経路と、それに伴う legacy 警告（`providers.<id>.… (legacy)`・`… overrides …`・`legacy identity conflicts`）を削除した。catalog は `[[model_routing.models]]`・`[[model_routing.deployments]]` だけから組む。`source_ref = "opencode_go"` の既知判定は `kind = "opencode_go"` の `[llm_sources.<id>]` があるかで決める（`Config::declares_source_kind`、`config/llm_route.rs`）。`provider:<id>` という source_ref は受け付けない。provider 行から割り当て seed を作る `Config` の関数も削除した。
- `config/model_catalog.rs`: `apply_role_assignments` から seed 引数と provider 行の deployment の分割・追加を削除し、対象を `legacy:*`（llm-proxy 形）の 1 lane deployment だけにした。seed の型も削除した。試験は provider 前提の 3 本を消し、接頭辞の引き継ぎ試験を `legacy:*` だけで書き直した（`legacy_lanes_inherit_the_configured_prefix`）。
- `config/dispatch.rs`: `[reviewer]` の検証を provider 行に対してではなく `llm_route_registry` の harness 宣言に対して行う。`adapter` を書いたなら `[harnesses.<id>]` の id か kind（`-`/`_` を同一視）を指す必要がある。`tier` は lane のラベルで source×model の適格性に関わらないので検査しない。`config/mod.rs` の `validate` は registry を 1 回組んで渡す。
- `model_discovery.rs`: opencode go の発見対象の判定を provider 行から `[llm_sources.<id>] kind = "opencode_go"` に付け替えた。role assignment の適用は seed なしの新しい形を呼ぶ。
- `celerisctl routing show`（`commands/routing.rs`）: provider 行の lane 表をやめ、`llm_sources`・`harnesses`・`llm_routes`・`route_defaults`・`cos.route_candidates` の宣言を出す。試験も新形に書き直した（`shows_llm_sources_harnesses_and_routes`・`a_source_without_harnesses_or_routes_says_so`）。
- `config/adapters.rs`: acp・paperqa・LDR の doc comment から `[[providers]]` の行ごとの上書きの記述を外し、`[harnesses.<id>]` の command/args と route の model で説明した。
- `config/tests.rs`: reviewer 試験を harness 宣言で書き直した（`reviewer_adapter_must_name_a_declared_harness_and_unknown_reviewer_keys_fail`、`backoff_and_reviewer_settings_map_to_dispatch_config`）。provider 行から deployment を作ることを確かめていた `routing_legacy_equivalence_qwen_is_cheap_only` を削除し、`routing_legacy_config_normalizes_with_warnings` は provider fixture を使わない `routing_catalog_takes_models_and_policies_only_from_model_routing` に書き直した。

この葉の後、`Config.providers` を読む本番 code は `config/providers.rs` と `config/mod.rs`（読み込み・検証）だけ。

```
$ grep -rn "\.providers\b\|provider_llm_source\|provider_lanes\|tier_models" crates/celeris/src crates/celerisctl/src --include=*.rs | grep -v "config/providers.rs\|config/tests.rs"
crates/celeris/src/daemon/cos_reload_tests.rs:48/51  （試験の TOML。config-drop で扱う）
crates/celeris/src/config/mod.rs:487, 504           （読み込み）
```

## 実行したコマンドと結果

- `cargo build -p celeris -p celerisctl --tests` → exit 0
- `cargo nextest run -p celeris -p celerisctl --no-fail-fast` → 648 件中 645 passed、3 failed。3 件（`browser_doctor` 2 件・`commands::browser::tests::approve_talks_one_line_to_a_private_socket`）は `bind: path must be shorter than SUN_LEN`。worker の `$TMPDIR` が 93 文字と長いためで、この変更とは関係ない。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 100。`passed 4809, failed 108, ignored 14`。失敗 108 件はすべて browser・credentiald の unix socket 試験（`browser_launcher`・`launcher_run`・`injection_ipc`・`broker`・`browser_doctor`・`task-api::browser_e2e`/`browser_live_cross`・`approve_talks…`）で、原因は同じ SUN_LEN。
- 確認として `TMPDIR=/tmp cargo nextest run -p celeris-credentiald -p celeris -p celerisctl -E 'binary(injection_ipc) | binary(broker) | test(/browser_doctor|approve_talks|launcher_run/)'` → 46 passed, 0 failed。
- `cargo fmt --all` は他の葉が残した未整形の file（llm-proxy・task-api・task-dispatch など）も書き換えたので、この葉の範囲外の file は戻した。

## 未解決

- test-parallel が worker の長い `$TMPDIR` で socket 試験を落とす。統合の検査は通常の短い TMPDIR で回すこと（この葉では直さない）。
- `[reviewer] adapter` の値は `WorkerHint.adapter` を通って reviewer run に渡る。route 選択はこの値を読まない。欄と経路は hint-drop で扱う。
- `apply_role_assignments` は `legacy:*` の deployment だけを書き換える。celeris の catalog には `legacy:*` 行が無いので、現状 celeris 側では効かない（llm-proxy 側の legacy catalog は proxy-concrete の範囲）。DB の割り当てを route の候補に重ねるかどうかは新しい API の子 task で決める。

## 本番への移行

- `[reviewer] adapter` を書いている本番 config は、その値が `[harnesses.<id>]` の id か kind と一致する必要がある（一致しなければ読み込みエラー）。
- `[[model_routing.deployments]] source_ref = "provider:<id>"` の行は読み込みエラーになる。具体的な source（`claude_oauth`・`openai_compatible:<id>`・`opencode_go` など）に書き換える。
- `source_ref = "opencode_go"` の deployment を使うなら `kind = "opencode_go"` の `[llm_sources.<id>]` を宣言する。
