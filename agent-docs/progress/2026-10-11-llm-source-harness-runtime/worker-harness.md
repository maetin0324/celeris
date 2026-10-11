---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: complete
title: worker-harness: task-worker・celerisctl worker が選ばれた harness・source・model を受ける
updated: 2026-10-11
---
# worker-harness: task-worker・celerisctl worker が選ばれた harness・source・model を受ける

## 変更

- `crates/task-worker/src/route.rs`（新規）: `route_adapter(adapters, &SelectedLlmRoute, resolve_secret)` と `request_adapter(adapters, &RunRequest, …)`。
  - 実行器は `llm_route.harness_id` の adapter（`[harnesses.<id>]` から組んだもの。command/args は宣言から）。表に無い harness は `RouteError::UnknownHarness`、`llm_route` が無ければ `RouteError::Missing`（どちらも fail closed）。
  - model は `llm_route.model_id` をそのまま `with_model` で載せる（model の選択肢を持たない `fake`・`langmem` は自分の設定のまま）。
  - `CredentialReference::AccountDirectory { path, env }` は `env` の環境変数に account ディレクトリを入れる。
  - `CredentialReference::Environment { name }` は `resolve_secret(name)` で値を引き（protocol には秘密を載せない）、`endpoint.base_url` と一緒に `WorkerAdapter::with_llm_endpoint` へ渡す。受けられない harness（`EndpointUnsupported`）・引けない credential（`CredentialUnresolved`）は fail closed。
- `WorkerAdapter::with_llm_endpoint`（新規の trait method、既定 `None`）。`PiAdapter` が実装（`base_url` と `CELERIS_PI_API_KEY`）、`BrowserSpecialistAdapter` は中継。
- `PaperQaAdapter`・`LdrAdapter` に `with_model` を足した（route の model を黙って捨てない）。
- protocol: `CredentialReference::AccountDirectory` に `env`（harness が account ディレクトリを読む環境変数名）を足した。dispatcher の `selected_llm_route`（route_launch.rs）が `AccountAdapter::env_var()` で埋める。`docs/protocol/worker-protocol.schema.json` を `UPDATE_SCHEMA=1` で再生成。
- task-dispatch の公開: `account_adapter_for_source_kind`・`selected_llm_route`・`RouteLaunchError` を crate root から出す（celerisctl が使う）。
- `celerisctl worker run`: `--provider`・`--adapter`・`select_provider`（`StaticPolicy`）・`resolve_account`（provider 行の account_pool）・tier から model を引く処理（`model_for_tier`・`select_tier`・`effective_models`）・`build_adapters` の利用を消した。代わりに `--harness`・`--source`・`--model`・`--account` を受け、`task_dispatch::route_select`（dispatcher と同じ純関数）で route を決め、`selected_llm_route` → `build_harness_adapters` → `task_worker::route::route_adapter` で起動する。`RunRequest.llm_route` に選んだ route を入れる。出力は `worker run: task=… harness=… source=… model=… account=…`。
- `local_deep_research.rs` の doc comment から provider 行の説明を消した（範囲 check の `grep -w ProviderConfig` に当たるため）。

## 試験

- 足した（task-worker、`worker_route_`）: `worker_route_runs_the_given_harness_with_the_given_model`、`worker_route_unknown_harness_fails_closed`、`worker_route_without_a_route_fails_closed`、`worker_route_account_directory_is_put_in_the_named_env`、`worker_route_openai_compatible_endpoint_and_credential`、`worker_route_starts_the_declared_command_with_the_route_model`（実 process: 宣言の command が route の `--model` で起動）。
- 足した（celerisctl、`worker_route_cli_`）: 先頭候補・`--model`・OpenAI 互換 source の endpoint・fail closed（未知 harness・account 無し・route_defaults 無し・未知 source・不正 account）・`--account` で絞る。
- 書き換えた（celerisctl/tests/worker_run.rs）: `provider_selects_the_matching_account_env` → `harness_flag_starts_the_declared_harness_command`、`unknown_provider_id_errors` → `unknown_harness_errors`、`fixed_account_and_quota_routing_reach_the_cli_without_mutating_the_store` → `codex_route_reaches_the_cli_without_mutating_the_store`（実 CLI stub に route の model と CODEX_HOME が届く・store と account 帳簿は不変）。他の試験は fake source・harness・route_defaults の宣言を持つ config に替えた。worker_run_signal.rs も同じ。
- 消した: `resolve_account_*`（7 件）と `build_adapters_resolves_an_instance_for_an_acp_provider_selected_by_worker_run`（provider 行の account_pool・acp provider の試験。account は source 側で `worker_route_cli_*` が見る）。
- dispatcher の `route_launch` 試験の期待に `env` を足した。

## 実行したコマンドと結果

- `cargo test -p task-worker worker_route_` → exit 0、6 passed
- `cargo test -p celerisctl --bin celerisctl worker` → exit 0、14 passed
- `cargo test -p celerisctl --test worker_run --test worker_run_signal` → exit 0、8 passed + 1 passed
- `cargo test -p task-dispatch route_launch` → exit 0、4 passed
- `UPDATE_SCHEMA=1 cargo test -p task-worker schema_matches` → schema 更新、その後 `cargo test -p task-core -p task-api -p task-worker schema_matches` → exit 0（1 + 8 + 1 passed）
- `! grep -rnw ProviderConfig crates/task-worker crates/celerisctl` → 一致なし
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `cargo test -p task-worker`（run の TMPDIR のまま）: browser launcher 系だけ失敗（unix socket の path が長い TMPDIR で上限を超える。変更とは無関係）。短い TMPDIR で再実行 → 1072 passed、0 failed。
- `bash scripts/dev/test-parallel.sh`: 下の「全体試験」節。

## 未解決・次の unit への引き継ぎ

- `TieredAdapter`（tiered.rs）と `WorkerAdapter::{model_for_tier, reasoning_effort_for_tier, with_tier_models}` は**この unit では消していない**。残る呼び出し元は task-dispatch の `provider_select.rs`（`effective_lane_model`・`adapter_with_effective_models`・`legacy_provider_profiles`）、`dispatch_run.rs` の監査記録（`reasoning_effort_for_tier`）、task-dispatch の試験 helper（`tests/mod.rs` の `three_lane_adapter`、`browser_fallback.rs` の `prep_tiered`）、celeris の `build_adapters`（provider 経路）と `crates/celeris/tests/model_role_assignments_consistency.rs`。どれも同じ段で並行する dispatch-prune と後段の config-remove が消す対象なので、ここで消すと並行 unit と衝突する。**dispatch-prune・config-remove は呼び出し元を消すときに tiered.rs と上の 3 method も同じ変更で消すこと**（最後に呼び出し元を消した unit が定義を消す）。
- celeris daemon の通常 run は今も dispatcher の `route_adapter`（route_launch.rs）で adapter に route を当てている。worker 側の `task_worker::route::route_adapter` と同じ規則だが、dispatcher は `Environment` credential の値を引かず endpoint も当てない。dispatcher を worker 側の helper に寄せるのは route_launch.rs の持ち主の仕事（提案）。
- `Environment` credential の秘密は `[secrets] dir/<name>` から読む（celerisctl）。daemon 側の引き方は未配線。

## 提案

- dispatcher の `Dispatcher::route_adapter` を `task_worker::route::route_adapter` に置き換え、OpenAI 互換 source の endpoint・credential も daemon の run で効かせる（daemon の secrets 読み出しを `resolve_secret` に渡す）。

## 本番への移行

- `celerisctl worker run --provider/--adapter` は無くなった。手動 run は `--harness <id>`（省略時は task の harness 欄と `[harness_selection]`）、必要なら `--source`・`--model`・`--account` を使う。config に `[llm_sources.*]`・`[harnesses.*]`・`[route_defaults.*]` が要る。
