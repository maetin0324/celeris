---
tasks: [01M44NN2DCZXV0TZ5FXX5TMN58]
completed: 2026-10-05
---
# Phase 1: 移行手順の初版（migration-doc）

[docs/ops/model-routing-migration.md](../../../docs/ops/model-routing-migration.md) を作った。ADR §9 の順に、config の控え、
旧キー → `[model_routing]` 対応表、起動時の警告・エラーの読み方、検証、`mode = legacy → shadow`、enforce 不可、指標確認、rollback を書いた。

実装との突き合わせ（`crates/celeris/src/config/model_routing.rs`、`crates/llm-proxy/src/legacy_catalog.rs`、`crates/task-core/src/model_router/policy.rs`）:

- キー名・mode 値（`legacy`・`shadow`・`enforce`）・objective・billing の値、旧形由来の model/deployment id（`legacy:<family>:<wire>`、`legacy:<source>:<Lane>`、`provider:<id>[/<lane>]`）、警告とエラーの文言はコードから写した。
- `[model_routing]` 専用の validate コマンドは無い。検証は `Config::load` を通る既存の `celerisctl config to-harnesses --config …`（exit code）と daemon 起動ログ（警告）とした。`celerisctl` は tracing を初期化しないので警告は出ない、と明記した。
- Phase 1 の `shadow` は catalog/policy の `mode` に載るだけで、dispatcher（`provider_select.rs` は `RoutingMode::Legacy` 固定）と proxy の選択は変わらない、と明記した。
- `Config` は `deny_unknown_fields` なので、旧 binary は `[model_routing]` のある設定で起動しない。binary の rollback は旧 config と組にする根拠とした。
- ADR §9 の `x-celeris-decision-id` はまだ実装に無いので書いていない。catalog API は並行の catalog-api 葉の担当なので「版にあれば」とした。

検証: `sh scripts/dev/check-doc-links.sh docs/ops/model-routing-migration.md` と `sh scripts/dev/check-doc-links.sh`（全体）はどちらも `check-doc-links: ok`、exit 0。コードは変更していない。

## 未解決事項

- catalog API の応答の欄名は catalog-api 葉の統合後に §6 へ具体的に書き足す（close 葉で確認）。

## 提案

- `[model_routing]` の警告も出せる設定検査コマンド（例: `celerisctl config check`）があると、daemon を再起動する前に警告を確かめられる。
