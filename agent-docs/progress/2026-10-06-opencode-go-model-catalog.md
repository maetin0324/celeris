---
title: opencode go の LLM source 化とモデル catalog の自動更新
tasks: [01M49KDZXXBKP8K2CZDXWTCJ5S]
status: done
updated: 2026-10-06
---
# opencode go の LLM source 化とモデル catalog の自動更新

ADR: [2026-10-06-opencode-go-and-model-catalog](../adr/2026-10-06-opencode-go-and-model-catalog.md)（付記に実装との突き合わせ）。
運用手順: [docs/ops/opencode-go-and-model-catalog.md](../../docs/ops/opencode-go-and-model-catalog.md)。

## 概要

opencode go の subscription を account pool の第 3 の adapter・LLM source `opencode_go` として使えるようにし、アカウント画面に 5 時間・1 週間・1 か月の枠を出す。claude・codex・opencode go・self-host の利用可能モデルを決定的に発見して SQLite の catalog に持ち、`/models` 画面・API・`celerisctl models` で一括管理する。LLM は呼ばない。

## phase 1 の内容（commit 0bbdafa6 / 1be957f5）

- `QuotaWindow::OneMonth` / `RateLimitObservation.one_month`。取得できない窓は `null`（画面は『不明』）で 0 にしない。
- `AccountAdapter::OpencodeGo`（`XDG_DATA_HOME`、`opencode/auth.json`）と `GET /zen/go/v1/usage` による枠確認（`task-worker::opencode_account`）。`[accounts] opencode_dir` / `opencode_go_usage_url`。
- `LlmSourceRef::OpencodeGo`、`AccountPoolSetting`（acp 行から名前付き pool を指す）。
- migration 0052（`model_catalog` / overrides / discovery）、`Event::ModelCatalogChanged`、`celeris::model_discovery`、`[model_catalog]`。
- `GET/PUT/DELETE /api/v1/llm/models…`、`POST /api/v1/llm/models/discover`、`celerisctl models`。
- web: アカウント画面の 3 本目のバー、`/models` 画面、fake-daemon・e2e。
- 1be957f5 は整形のみ。

## 証拠

- `bash scripts/dev/test-parallel.sh`: exit 0。`CELERIS_TEST_SUMMARY {"runner": "nextest", "nextest_version": "0.9.146", "jobs": 8, "binaries": 140, "passed": 4184, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0}`（2026-10-06、tree 1be957f5 + 文書）
- `cargo clippy --workspace -- -D warnings`: exit 0（警告なし。`Finished dev profile`）
- `pnpm -C web typecheck`: exit 0（`tsc -b`）
- `pnpm -C web test`: exit 0（vitest 70 file / 468 件 pass、node --test 59 件 pass、fail 0）
- `pnpm -C web e2e`: exit 0（269 passed、8 skipped。`e2e/admin/models.spec.ts` 5 件を含む）
- `pnpm -C web e2e:nfr`: exit 0（106 passed。`/models` の axe 4 幅を含む）

## 再レビュー対応（attempt 2、2026-10-06）

- `web/api/realtime/invalidation-map.ts` の import を Biome で整形し、`modelKeys` を名前順に配置した。実行時の処理は変更していない。
- `pnpm -C web lint`: exit 0（382 files、error 0。既存の `styles.css` の warning 4 件）。前回の organizeImports / format の error 2 件を解消した。
- 上記以外の検証結果は前回の実行記録。今回の変更は import の整形と本記録のみのため、Rust・web の機能試験は再実行していない。

## 統合時の注意

本ブランチの `0052_model_catalog.sql` は、並行中の CoS ブランチ
`celeris-wu/01M46VVAD0ZAVZ9C4Q0KJM9ESV/live-check` の
`0052_cos_run_credentials.sql` と版数が重なる。同ブランチは `0053_cos_triage.sql` も使用している。
本ブランチの `RESERVED_VERSIONS` は `[38, 39, 40, 50]` で、この重複を回避しない。
後から main に取り込む側で、その時点の未使用番号へ振り直すこと。
SQL ファイル名・migration 登録・`SCHEMA_VERSION`・版数を固定した試験と文書の参照をそろえ、
既存 DB からの migration 試験と workspace 検査を再実行する。

## 未解決事項

- run 中の 429（`GoUsageLimitError`）の窓の特定と `resets_at` の記録が未実装（`crates/task-worker/src/provider.rs` の TODO）。
- 発見は claude の OAuth token を更新しない（期限切れなら claude の発見は失敗し、catalog は変わらない）。
- 発見 hook は起動時の config の写しを使う。設定変更は再起動後に効く。
- catalog の新モデルは routing に自動で入らない（override の `tier` か config で決める）。
- API の provider の作成・更新は名前付き `account_pool` を設定できない。
- 本番での実機確認が未実施。

## 提案

- run 中の 429 の窓の特定（`limitName` → 窓、`retry-after` → `resets_at`）を ADR の追補として別 phase で行う。
- provider API の `account_pool` を bool ではなく `bool | adapter 名の文字列` にそろえる（config の `AccountPoolSetting` と同じ形）。
- システム全体の変更（catalog の変化など）の event は、nil ULID の疑似 task ではなく task に属さない event の置き場を設計する。

## 実機確認の手順（Fable が本番で 1 回）

[docs/ops/opencode-go-and-model-catalog.md](../../docs/ops/opencode-go-and-model-catalog.md) の順に行う。

1. 手順 1・2 で account dir と config を用意し、daemon を配送・再起動する。
2. `celerisctl models discover` と `celerisctl models list` で 4 source の結果を確かめる（失敗した source の error を記録）。
3. `GET /api/v1/accounts` で adapter `opencode-go` の 3 窓を確かめ、`POST /api/v1/accounts/opencode-go/main/check` を 1 回叩く。
4. web の `/accounts` と `/models` を見る。
5. opencode go の provider で task を 1 件流し、run が成功することを確かめる。
6. 結果を `## 証拠` に追記し、status を done にする。
