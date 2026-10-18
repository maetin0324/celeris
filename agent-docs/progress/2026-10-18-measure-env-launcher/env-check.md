---
title: "測定環境 launcher: env-check（接続先 daemon の同一性・実効構成照合を preflight に結合）"
tasks: [01M56HCNS9SYAS7TAEA7Z3E4CZ]
unit: env-check
status: done
updated: 2026-10-18
---
# env-check

完了日: 2026-10-18。ADR `2026-10-18-measure-env-launcher.md` D2 の 1〜4 相当（API の答えと測定 config の照合）を実装。

## 実装
- `crates/campaign/src/env_check.rs`
  - `derive_api_url`: 測定 config の `[api] listen` から origin を導く（`0.0.0.0`/`::` は loopback）。
  - `check_identity`: api-url が導出値と違えば**接続も token 送出もせず**拒否。一致時は `GET /api/v1/config` を 1 回だけ読み、
    `config_path`、`identity.config_sha256`（起動時の config bytes）、`overrides` 空、`release`（`dev` 不可・manifest B/C `harness_version` 一致）、
    `effective.*`（db・workspace_root・knowledge_root・default_mounts・memory_dir・langmem/gc・cron seed 数・selfdeploy・api_listen）を
    config から導いた値と項目ごとに照合。`identity` を返さない daemon は拒否。
  - `run`（単独 `env-check` の本体）: `check_config` + canary 定義 + `check_identity`。
- CLI: `celeris-campaign env-check --manifest <m> --measure-config <toml> --normal-config <toml> --canaries <json> --api-url <origin> --token-env <NAME>`（不足があれば exit 1・JSON）。
- preflight に `env-check` 項目として結合（`run`/`watch` は `preflight::gate` を通るので同じ検査が有料実行の前に走る）。
- 試験 `crates/campaign/tests/env_check.rs`: 応答は実 task-api router（正式 `ConfigView`、`EnvOptions.config_view` を追加）の本文。
  一致で preflight 通過、別 daemon・実効 config 各項目の相違・起動後 config 書換・版不一致・identity 無しは preflight 不通過かつ
  `run_gated` の transport 呼び出し 0 件・out 未作成。api-url 食い違いは `GET /config` 0 回。CLI も試験。
- 既存の preflight 系試験（`preflight`・`preflight_contract`・`lock_*`・`celeris/tests/campaign_lock_e2e`）は api-url を測定 config の listen と揃え、
  daemon 応答（`daemon_identity` 共有部品、e2e は daemon 自身の `celeris::config_view`）を返すよう更新。

## 証拠
- `cargo test -p celeris-campaign --test env_check`: 8 passed。`--test preflight`: 18 passed。`-p celeris --test campaign_lock_e2e`: 4 passed。
- 全体のゲートは下記（コミット前に実行）。

## 未解決事項・提案（ADR 付記の差分）
- **`--env <measure-env.toml>` への統合・旧 CLI 欄の削除は未実施**（本 WU の Objective は既存引数のままの env-check。launcher WU で扱う）。
- 未実装: D2-5（`/proc/<pid>` の starttime・exe sha256・cmdline・PATH）と D2-6（ポート所有: SO_REUSEPORT 相乗りの検出）。
  `identity` は同一 daemon の自己申告なので、同 port に別 daemon が相乗りして要求が振り分けられる場合は単発 GET では見分けられない
  （instance_id が毎回同じことの確認は `/proc` 照合と合わせて launcher WU）。`Host` に `/proc` 読みを足す設計のまま。
- `llm_proxy_listen` は照合していない（config に無いと daemon の既定で起動し得る）。`check_config` への ADR D1 末尾の追加検査
  （llm_proxy・selfdeploy・push・notify）も未。selfdeploy は既定値（本番 releases_dir・push=true）と比べるだけで拒否はしない。
- 実装済み / 無 LLM 検証済み。実 daemon を使った実経路確認は launcher WU の e2e。

## 追記（再実行）
- task-api/tests/common への変更は戻し、campaign 試験側で ApiSettings.config_view を差し替える形にした。
- crates/celeris/tests/campaign_lock_e2e.rs の mock Host に /api/v1/config（正式 DTO）と測定 config listen に合わせた port を追加済み。4 試験 ok。
- 検査: fmt exit 0 / clippy exit 0 / test-parallel: 5734 passed, 0 failed。
