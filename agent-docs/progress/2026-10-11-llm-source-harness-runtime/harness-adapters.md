---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: harness-adapters: harness 単位の adapter 表と RoutePick から起動入力を作る helper
updated: 2026-10-11
---
# harness-adapters: harness 単位の adapter 表と RoutePick から起動入力を作る helper

## 完了

- `task_core::llm_route::SourceSpec` に `endpoint`・`credential_ref`（秘密の参照 id。値ではない）を追加し、`Config::llm_route_registry` が `[llm_sources.<id>]` から写す。
- `Dispatcher` に `harness_adapters: HashMap<HarnessId, Arc<dyn WorkerAdapter>>` と `set_harness_adapters`・`harness_adapter` を追加。
- `crates/celeris/src/daemon/adapters.rs` に `build_harness_adapters`: `[harnesses.<id>]` の `kind`（`claude_code` と `claude-code` は同じ）で adapter を選び、`[adapters.<kind>]` を基底に宣言の `command`/`args` で上書きする。model と account はここでは付けない。未知の kind は `ConfigError::Invalid`（起動・reload とも失敗）。
- bootstrap（`build_dispatcher`）と admin の reload の両方で `set_harness_adapters` を呼ぶ。
- 新 module `crates/task-dispatch/src/dispatcher/route_launch.rs`:
  - `Dispatcher::route_adapter(&RoutePick)`: harness の adapter に `with_model(model_id)`（model の選択肢が無い adapter は元のまま）、OAuth 型 source（`account_adapter_for_source_kind`）なら `adapter_for_account` で account の env を当てる。harness・source が無い、または account を当てられない場合は `RouteLaunchError`（fail closed）。
  - `selected_llm_route(&Registry, &RoutePick, account_root)` と `Dispatcher::route_launch_target`: `SelectedLlmRoute` を作る。API source は endpoint（base_url・api_format）と `CredentialReference::Environment { name: credential_ref }`、OAuth source は `CredentialReference::AccountDirectory { path: <[accounts] root>/<account> }`。秘密値は載せない。
- `dispatch_run.rs`・`review_spawn.rs` は変えていない（run-switch・review-switch の持ち物）。provider 鍵の `adapters` 表も残している（dispatch-prune が消す）。

## 検証

- `cargo test -p task-dispatch route_select_dispatch_adapter_` — exit 0、4 passed（選ばれた harness の adapter と model、OAuth account env、未知 harness/source/account 不可は Err、SelectedLlmRoute の中身）。
- `cargo test -p celeris route_select_dispatch_adapter_` — exit 0、2 passed（daemon が宣言ごとに adapter を作る、未知 kind は設定エラー）。
- `cargo clippy --workspace -- -D warnings` — exit 0。`cargo clippy --workspace --tests -- -D warnings` — exit 0。
- `bash scripts/dev/test-parallel.sh` — exit 100、5155 run: 5047 passed、108 failed、13 skipped。失敗は全部 browser/launcher/credentiald 系（`celeris::browser::*`・`celeris-credentiald::{broker,injection_ipc,browser_doctor}`・`task-worker::browser_*`・`task-api::browser_*`・`celerisctl commands::browser`）。原因は worker sandbox の環境: Unix socket path が SUN_LEN を超える（`path must be shorter than SUN_LEN`、worktree path が深い）と、sandbox 内で daemon の socket が立たない（60 秒待ちの timeout）。この葉の変更箇所（task-core llm_route・task-dispatch・celeris daemon adapters）に失敗は無い。

## 未解決

- `CredentialReference::Environment { name }` には `credential_ref`（secret id）をそのまま入れた。worker 側がそれをどの環境変数名で harness に渡すかは worker-harness が決める（必要なら source 宣言に env 名の欄を足す）。
- pi harness の `base_url` は `build_harness_adapters` では付けない。選択 route の endpoint から付けるのは run-switch / worker-harness。
- 試験用 config は loader がまだ `[[providers]]` 1 行を要求するため、試験の中で 1 行足している（config-remove で消える）。

## 本番への移行

- `[harnesses.<id>]` の `kind` は claude_code・codex・aider・pi・acp・browser_specialist・paperqa・local_deep_research・langmem・fake のいずれか（`-` 区切りも可）。それ以外の kind があると起動と reload が設定エラーになる。本番 config に `[harnesses.*]` を書くときはこの範囲で書く。
