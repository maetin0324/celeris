---
title: 工程 proxy wire 名と opencode model ID の実装 — 統合後検証と記録
tasks: [01M4TCTMB0YX58E5WE06EC1W4Q]
status: complete
completed: 2026-10-13
---

# 工程 proxy wire 名と opencode model ID の実装 — 統合後検証と記録

対象 ADR `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の付記 (3)(4)。統合後 HEAD `76e9c927` で、名前に `proxy_model_name` / `opencode_model_id` を含む試験、`migration_runbook_toml_loads`、`cargo clippy --workspace -- -D warnings` を流した。実装の修正はしていない。全試験が通った。

## 各 leaf の要点（`model-names/` のメモ）

- `wire-core`: daemon の route 選択で、`via = "llm_proxy"` の route に限り model を `<source>/<model>` の wire 名にする（1 回だけ作る）。`/` を含む model はそのまま。`local_deep_research` / `paperqa` / `langmem` の default 候補はすべて対応する `llm_proxy` route を要する（config 検証）。
- `worker-proxy`: PaperQA は route の wire 名を `settings.llm` / `summary_llm` / `agent.agent_llm` に入れ、LangMem は `with_model` で route の model を使う。LDR は既存の model 上書きを維持。
- `opencode-ns`: `[harnesses.opencode].model_namespace` を opencode の ACP にだけ適用。空値・`/` を含む値は検証で拒否。namespace は route の source からは導かない。
- `pi-fixture`: `celerisctl` の `worker_tests.rs` の `[harnesses.pi]` fixture に `extensions` / `tools` を追加（Pi の必須化に合わせた）。
- `integrate-impl`: 上の 4 leaf を統合済み（`76e9c927`）。

## 証拠コマンドと結果

- `cargo test -p task-worker proxy_model_name` — exit 0。6 passed, 0 failed（LDR・PaperQA・LangMem の qwen / claude source）。
- `cargo test -p task-worker opencode_model_id` — exit 0。2 passed, 0 failed（`opencode_model_id_uses_declared_namespace`、`non_opencode_model_id_remains_route_model`）。
- `cargo test -p task-dispatch proxy_model_name` — exit 0。6 passed, 0 failed（`route_launch_proxy_model_name_{ldr,paperqa,langmem}_{qwen,claude}`）。
- `cargo test -p celeris proxy_model_name` — exit 0。ただし **0 件実行**（celeris crate に該当名の試験が無い）。exit 0 は空の一致による。下の未解決事項を参照。
- `cargo test -p celeris migration_runbook_toml_loads` — exit 0。1 passed, 0 failed。
- `cargo test -p celeris --lib config::` — exit 0。137 passed, 0 failed（Pi 宣言 `pi_harness_declaration_deserializes_extensions_tools_and_env`・`model_namespace` の検証を含む）。`wire-core` メモが挙げる「非 proxy route の拒否」試験は、名前では見つけられなかったため、この実行で確認したとは言えない。
- `cargo clippy --workspace -- -D warnings` — exit 0。`Finished dev profile` で警告・エラーなし。
- `git status --short` — 空（記録以外に差分なし。この記録ファイル自体は本ステップで追加）。
- runbook の一致: `agent-docs/progress/2026-10-11-provider-layer-redesign.md:209` の `model_namespace = "qwen-local"` と、`crates/celeris/src/config/tests.rs` の試験が同じ宣言を読む。

本ステップでは `scripts/dev/test-parallel.sh` は流していない（依頼の試験名の指定に従った）。ワークスペース全体の試験は別途必要。

## 未解決事項

1. **celeris crate の `proxy_model_name` 試験が 0 件。** 最終レビューの受け入れ条件 `cargo test -p celeris proxy_model_name` は、空の一致で exit 0 になる。提案: celeris crate 側に proxy route の wire 名を確かめる試験を 1 件足すか、この受け入れ条件を `-p task-dispatch` に寄せる（ADR 付記の試験場所と合わせる）。実装の変更は本 WU の範囲外。
2. **本番 config の反映は人の手順。** 本番 `~/.config/celeris/config.toml` の `[harnesses.opencode]` に `model_namespace = "qwen-local"` を入れる。`pi-*` 行には `extensions` と `tools`、`ldr-qwen` / `paperqa-qwen` / `langmem-main` には対応する `llm_proxy` route が要る。手順と旧値の対応は `agent-docs/progress/2026-10-11-provider-layer-redesign.md` の「本番への移行」節を参照。エージェントは本番 host を変更していない。
3. ワークスペース全体の `scripts/dev/test-parallel.sh` は未実行（上記）。
