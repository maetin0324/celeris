---
title: モデル役割の割り当てで self-host（openai-compatible）の ACP・Pi 行に渡すモデル名の接頭辞（qwen-local/）が落ちる問題を直す
tasks: [01M4A9F8VVESKBSK7ZQEDWZ9YS]
status: done
updated: 2026-10-07
completed: 2026-10-07
---

# モデル役割の割り当てで self-host（openai-compatible）の ACP・Pi 行に渡すモデル名の接頭辞（qwen-local/）が落ちる問題を直す

設計は [ADR 2026-10-06 model-role-assignments](../adr/2026-10-06-model-role-assignments.md) の付記「実行用モデル名の接頭辞（2026-10-07 wire-prefix）」。Pi 行側は [ADR 2026-10-07 coding-harness-default-pi-hashline](../adr/2026-10-07-coding-harness-default-pi-hashline.md) の付記。

## 何を直したか

- **規則の一般化**: `task_core::model_catalog::assignments::WireRule`（`new(source, adapter, row_model)` / `proxy(source)`、`prefix` / `model` / `catalog_model_id`、`configured_prefix` / `configured_wire` / `default_wire_prefix` / `adapter_takes_provider_prefix`）。行の config の wire（lane binding → 行の `model`）の `<prefix>/` を引き継ぎ、無ければ source × adapter の表（`opencode-go` × acp|pi → `opencode-go/`）、proxy は接頭辞なし。旧 `wire_prefix_for`（opencode-go × acp だけ）は廃止。
- **3 か所 + API**: dispatcher `provider_select.rs`（`effective_tier_models`・役割メンバー候補）、routing catalog `celeris::config::model_catalog`（`assigned_upstream` が元の `upstream_model` の接頭辞を引き継ぐ。seed は表。`apply_model_catalog` の不在・無効の照合も `<prefix>/` を見通す）、llm-proxy `normalize_legacy_config_with`（`WireRule::proxy`）、task-api `model_assignments.rs`（`Participant.adapter`、preview の before / after を実行用の名前に。`docs/api/v1/gui-api.md` §3.127.4 を更新）。
- **人のコメント（Pi 行）への対応**: `pi` を `provider/<id>` を要する adapter に含め、qwen の cheap 割り当て（ACP・Pi 行とも `qwen-local/qwen3.8-27b`）と opencode go の Pi 行（`opencode-go/grok-4.6` 行に `glm-5` → `opencode-go/glm-5`）の両方を dispatcher の実起動（`WorkerStarted.model`）・routing catalog・preview で固定した。

## 証拠

| 条件 | コマンド | 結果 |
| --- | --- | --- |
| 0 | `cargo nextest run -p task-core -p task-dispatch -p celeris -p llm-proxy -p task-api -E 'test(/wire\|prefix\|assign\|legacy_catalog\|model_catalog\|role_members/)'` | 93 tests: 93 passed（初回は新試験 2 件が「割り当ての無い lane は `assignment:none`」の既存規則を見落とした期待違いで落ち、期待を直して再実行） |
| 0, 1 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest 4299 passed / 0 failed / 14 ignored（132 binaries）、doc-test exit 0 |
| 1 | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 1 | `cargo fmt --all -- --check` 相当（`cargo fmt --all` 後に差分なし）、`sh scripts/dev/progress-index.sh --check`、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | すべて ok |

実行ログは `/local/celeris/data/workspaces/01M4A9F8VVESKBSK7ZQEDWZ9YS/artifacts/{clippy,test-parallel}.log`。

## 未解決事項

- self-host の行で config の `model` も `tier_models` も無い（seed だけ）場合は接頭辞を知る手段が無く接頭辞なしになる。config に `model = "<provider>/<id>"` を書く運用（ADR 付記に記載）。
- 本番の cheap 役割の割り当て（と opencode-go の Pi 行の追加）は配送後に運用セッションが `/models` 画面で行う。本 run は本番 config / daemon / DB に触れていない。

## 提案

- `ProviderLive` に行ごとの「実行用の接頭辞」を載せて API に出せば、web の preview で接頭辞の由来（config / 表）を表示できる。
