---
title: provider 廃止リリース不具合: Pi 設定と OpenCode model ID の設計
tasks: [01M4T98YHKGT945HH1588SC2VR]
status: done
updated: 2026-10-13
---
# provider 廃止リリース不具合: Pi 設定と OpenCode model ID の設計

## 件2: pi-harness — Pi の extensions・tools・env

### 決定

`[harnesses.pi]` に `extensions: Vec<PathBuf>`、`tools: Vec<String>`、`env: HashMap<String, String>` を追加する。`env` は既存の adapter env と同じ secret reference 解決・優先順位を通し、Pi 起動時に子 process へ渡す。Pi の route で `with_env` が作る account directory や credential、endpoint は宣言 env を保持した上で重ねる。extensions は設定ファイル位置からの相対パスとして解決する。空の extensions/tools は設定検証で拒否する。

対応表:

| `[harnesses.pi]` 欄 | 型 | `PiConfig` / 起動で使う欄 | 旧 `[[providers]]` pi-* から移す値 |
|---|---|---|---|
| `extensions` | path 文字列の配列 (`Vec<PathBuf>`) | `PiConfig.extensions` → `-e <path>` | 各 pi-* 行の extensions をそのまま移す。Hashline extension の実パスは既存 config/backup を参照する |
| `tools` | 文字列配列 (`Vec<String>`) | `PiConfig.tools` → `--tools` の comma-separated allowlist | 各 pi-* 行の tools をそのまま移す。extension の read/edit 名を含む完全な許可リスト |
| `env` | TOML string table (`HashMap<String,String>`) | `PiConfig.env` → child process env | 各 pi-* 行の `env` と `env_from_secrets` を harness 共通設定へ移す。secret 値そのものは書かず secret reference を維持 |

旧 provider 型は `crates/celeris/src/config/providers.rs` の `extensions/tools/env/env_from_secrets` を持ち、旧 `build_adapters` は PiConfig にそれらを写していた。現 runbook の移行 TOML は pi-* の個別 extensions/tools/env 値を掲載せず、Pi の旧設定例もチェックアウト内に残っていない。よって値を捏造せず、本番 backup の各 pi-* 宣言を移行元として対応を指定する。共有 harness へ統合する際、差がある extension/tool allowlist/env は和集合でなく、共通 Pi が必要とする完全な値として本番運用者の既存値を照合して確定する。

### 理由

`HarnessConfig` は `deny_unknown_fields` で、現在 Pi 固有欄を受けない。`build_harness_adapters` は空の extensions/tools/env で `PiConfig` を作る一方、`PiConfig::validate` は extensions と tools を必須とし、Pi 起動は `-e`、`--tools`、`envs` を使用する。設定宣言から値を渡すのが provider 行別設定を廃止した後の同等な実行契約となる。環境値を route より先に組み、route 由来の追加 env を保つ。

### 変更箇所（file:関数）

- `crates/celeris/src/config/harness.rs`: `HarnessConfig`、設定検証および relative path 解決。
- `crates/celeris/src/daemon/adapters.rs`: `build_harness_adapters` の PiConfig 構築。
- `crates/task-worker/src/pi.rs`: `PiConfig` env を既に child command に渡す起動処理。契約変更が必要なら同じ箇所。
- `crates/task-worker/src/route.rs`: `route_adapter` の `with_env` / `with_llm_endpoint` 合成で宣言 env を保持。

### 実装 leaf が足す試験名（案）

- `celeris::config::harness::tests::pi_harness_deserializes_extensions_tools_and_env`
- `celeris::config::harness::tests::pi_harness_resolves_relative_extension_paths`
- `celeris::daemon::adapters::tests::pi_harness_declaration_reaches_pi_adapter`
- `task_worker::route::tests::route_endpoint_preserves_pi_declared_environment`
- `task_worker::pi::tests::pi_adapter_passes_declared_environment`

## 件4: model-names — OpenCode の model ID

### 決定

`[harnesses.opencode]` に `model_namespace: Option<String>` を宣言し、OpenCode ACP に限り `route_adapter` が `model_namespace/model_id` を `with_model()` へ渡す。route の source ID は namespace と同一とは限らないため `<source_id>/<model_id>` の推測変換は採用しない。今回の設定値は `qwen-local`、route model は `qwen3.8-27b` とし、OpenCode が要求する `qwen-local/qwen3.8-27b` を生成する。未宣言時は route model をそのまま使い、他 harness には設定を適用しない。空 namespace と `/` を含む値は拒否する。

### 理由

`route_adapter` は現在 `route.model_id` をそのまま `with_model()` に渡し、ACP adapter は選択 model を session model option に渡す。runbook は OpenCode の既存 `OPENCODE_CONFIG` を保持しつつ model を `qwen3.8-27b` と記しているが、本番 OpenCode config の provider ID は `qwen-local` で、要求される ID は `qwen-local/qwen3.8-27b`。runbook の LLM source `qwen` と provider namespace は異なるため、source prefix 付与では一致しない。設定で namespace を明示し、配線層で一意に組み立てる。

### 変更箇所（file:関数）

- `crates/celeris/src/config/harness.rs`: `HarnessConfig` の `model_namespace` 設定と検証。
- `crates/celeris/src/daemon/adapters.rs`: OpenCode ACP adapter に namespace を設定する組み立て。
- `crates/task-worker/src/route.rs`: `route_adapter` で OpenCode 用 model ID を作る。
- `crates/task-worker/src/acp.rs`: `with_model` から model option へ渡す既存経路の確認。

### 実装 leaf が足す試験名（案）

- `celeris::config::harness::tests::opencode_model_namespace_deserializes_and_validates`
- `celeris::daemon::adapters::tests::opencode_harness_passes_model_namespace`
- `task_worker::route::tests::opencode_model_id_uses_declared_namespace`
- `task_worker::route::tests::non_opencode_model_id_remains_route_model`
- `task_worker::acp::tests::opencode_receives_qualified_route_model_id`
