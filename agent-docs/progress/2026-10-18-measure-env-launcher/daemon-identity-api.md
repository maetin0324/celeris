---
title: "測定環境 launcher: daemon の同一性を返す read-only 項目（daemon-identity-api）"
tasks: [01M56HCNS9SYAS7TAEA7Z3E4CZ]
unit: daemon-identity-api
status: done
updated: 2026-10-18
---
# daemon-identity-api

完了日: 2026-10-18。ADR 2026-10-18-measure-env-launcher D2 に従い、`GET /api/v1/config`（認証必須）に `identity` を追加。

## 変更
- `crates/task-api/src/types.rs`: `ConfigView.identity: DaemonIdentityView`、`EffectiveConfigView`（instance_id・release・pid・started_at・config_sha256・overrides・実効 db/workspace_root/knowledge/memory/selfdeploy/api_listen/llm_proxy_listen）。
- `crates/celeris/src/config/mod.rs`: `Config.source_sha256`（load 時の bytes の sha256）と `applied_overrides`（`apply_overrides` が記録）。
- `crates/celeris/src/daemon/api.rs`: `config_view` が identity を作る。`/health` は変更なし。
- schema 再生成: `UPDATE_SCHEMA=1`（`docs/api/v1/api-v1.schema.json`）、`web/api/generated/` を `node scripts/gen-types.mjs` で更新（gui に ConfigView の生成型は無い）。
- 試験: `task-api/tests/daemon_clusters_config.rs::config_identity_is_authenticated_only_and_has_no_secret`（実 router・token 値が出ない・/health に identity が無い）、`celeris` bin の `config_identity_reports_effective_values_without_secrets`（実 `Config::load`＋上書き→実 router→sha256・overrides・実効値・token 値と token_file 名が出ない）。

## 判断
- ADR D2 の `api_token_file` は出さない（既存 e2e の「token_file の場所は応答に出ない」不変条件を保つ）。ADR に付記済み。

## 証拠
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- 新規 2 試験 ok。`bash scripts/dev/test-parallel.sh`: 1 回目は e2e の token_file 名不変条件で失敗（api_token_file を除いて解消）。2 回目は task-dispatch の work_units 2 件が負荷で失敗（無関係の flaky。`cargo nextest run -p task-dispatch work_units` は 55 passed）。

## 提案
- env-check WU は `identity.effective` の `api_token_file` 比較を行わない（`config_sha256`＋`config_path`＋`overrides` 空で縛る）。
