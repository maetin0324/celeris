# Pi harness 宣言の移行

tasks: [01M4T94XWN31GW9SENK41GAP8F]

`[harnesses.pi]` に `extensions`、`tools`、`env`、`env_from_secrets` を追加し、daemon が `PiConfig` へ値を渡すようにした。extension の相対 path は `Config::load` 時に TOML ファイルのディレクトリ基準で解決する。Pi 宣言では extensions/tools を必須にし、他 harness のこれらの欄は拒否する。env は secret reference 解決を含む共通合成関数を通す。

Runbook TOML と 2026-10-11 progress の複製には、Pi extension/tools/env 欄を加えた。checkout に実稼働 backup の旧 pi-* 値は存在しなかったため、値は推測せず backup から転記する場所を明示した。運用者は稼働 config backup と Hashline README で path/tool 名を照合する必要がある。

確認: `cargo test -p celeris pi_harness_declaration_deserializes_extensions_tools_and_env --lib` と `cargo test -p celeris migration_runbook_toml_loads --lib` は成功。
