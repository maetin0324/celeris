---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: complete
title: llm_route snapshot から provider 表を除去
updated: 2026-10-11
---
# llm_route snapshot から provider 表を除去

## 変更

- `DaemonSnapshot.providers` / `ProviderLive` と provider 表を構成する `SnapshotPublisher`、dispatcher setter、in-use 集計を削除した。snapshot publisher を作る通常 API・account limit API の配線と各試験 fixture も更新した。
- celeris の provider 鍵 adapter 構築、provider ごとの実効モデル表、provider snapshot 構築を削除し、harness ごとの adapter 表だけを残した。secret usage は adapter 設定の secret と `llm_sources.*.credential_ref` から組み立てる。
- implicit account の行参照を `models` に改め、選択 registry の route default / explicit route / CoS 候補から source ごとの model id を並べた。API 型・schema・GUI 型・API 文書も合わせた。
- account check の表示にも利用する値は `AccountCheckView` として残した。
- 旧 provider adapter の期待値に依存する試験を削除し、新しい harness adapter 試験を保持した。

## 検証

- `cargo check -p task-ops -p task-dispatch -p task-api -p celeris --tests` — pass。
- `cargo clippy -p task-ops -p task-dispatch -p task-api -p celeris --tests -- -D warnings` — pass。
- `cargo test -p task-dispatch tick_publishes_daemon_snapshot_to_watch` — pass。
- `cargo test -p celeris route_select_dispatch_adapter_daemon_builds_one_adapter_per_declared_harness` — pass。
- `cargo test -p task-api --test accounts_admin implicit_accounts_are_listed_and_patchable_without_the_accounts_section` — pass。
- `UPDATE_SCHEMA=1 cargo test -p task-core -p task-api -p task-worker schema_matches` — pass。schema の `DaemonSnapshot.properties` に `providers` がなく、`ImplicitAccountLive` が `models` を持つことも JSON で確認した。
- `git diff --check` と `sh "$CELERIS_WU_SCOPE_PATHS"` — pass。変更パスは許可範囲内。
- `cargo fmt --all -- --check` — fail。今回触れていない `celerisctl` / `llm-proxy` 等を含む formatter 差分が既存状態にあり、全 workspace の check は通らない。全体 formatter は走らせず、他 unit のファイルは変更していない。
- GUI type generator `pnpm -C gui gen:types` は packageManager 指定 11.27.0 に対し起動環境が pnpm 12.6.0 のため拒否された。GUI generated type の該当2欄は schema に合わせて手動更新した。

## 再試行（attempt 2）

- 前回の否定 grep check（`ProviderLive|provider_lives|effective_models|build_adapters|set_snapshot_providers`）が comment 4 箇所で落ちた。`crates/celeris/src/daemon.rs` の module 表、`config/adapters.rs`・`config/dispatch.rs`・`task-worker/src/langmem.rs` の comment を `build_harness_adapters` に直した。check は exit 0。
- `tests/e2e/tests/api_scenarios.rs` の `api_enforces_token_host_and_workspace_boundaries_without_leaking_env_values` が、api-unprovider で消えた `GET /providers`（404）と `/config` の provider env key 表示を期待していたので、その 2 点を消した（秘密値が応答に出ない検査は残す）。`cargo nextest run -p e2e --test api_scenarios` 12 passed。
- `cargo clippy --workspace --all-targets -- -D warnings` exit 0。
- `bash scripts/dev/test-parallel.sh`: run の既定 TMPDIR（93 文字）では unix socket path が長すぎ browser・credentiald 系が 109 件落ちる（環境要因）。短い TMPDIR で再実行して passed 4916 / failed 4。4 件は `task-api::browser_e2e` の fixture 準備待ち 60 秒 timeout（全体並列の負荷）で、単独実行では 4 passed。

## 未解決

- `web/api/generated/{schema.json,types.ts}` は再生成していない。再生成すると `web/features/ops/daemon-screen.tsx`（`s.providers.length`）と `web/api/model-assignments.ts` が型で壊れ、どちらもこの unit の範囲外。web の provider 表示の撤去と生成型の同期は後続（hint-drop 以降か web の葉）で行う。

## 本番への移行

本番 host や DB は変更していない。新 API schema/type と一緒にアプリを通常リリースする。運用者による追加 migration 操作は不要。
