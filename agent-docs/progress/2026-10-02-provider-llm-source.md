---
title: provider の LLM source / adapter 分離と cheap 専用 Qwen（ADR-0132）
tasks: [01M3YF3NSR46FM314VHG14T3N6]
status: done
updated: 2026-10-02
---
# provider の LLM source / adapter 分離と cheap 専用 Qwen（ADR-0132）

> 旧 `docs/PROGRESS.md`（現 `agent-docs/PROGRESS.md`）に main が足した節を ADR-0128 D6 に従い sync-main-2 migrate-docs（task 01M3Z8CXYG1J6BZCQ87YS3FC67）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

棚卸し（旧 `docs/progress/provider-llm-source-inventory.md`）: [inventory](2026-10-02-provider-llm-source/inventory.md)。

## provider / LLM source 分離の設定例（2026-10-02、config-docs）

ADR-0132 D2・D7 に合わせ、設定例の provider ID を source 非依存の名前にし、
PaperQA の設定 JSON を `config/paperqa.proxy.example.json` に改名した。
PaperQA は `celeris/standard`、LDR・LangMem・opencode は `celeris/cheap` を参照する。
Qwen の tier 写像は cheap のみ。人が実行する本番移行・確認・戻し方は
`docs/ops/provider-llm-source-migration.md` に記した。

`cargo test -p celeris --lib example` の既存 `loads_research_example_config` は
`[adapters.paperqa].settings` の旧ローカルパスを文字列一致で固定しているため、
設定例を `settings/proxy` にした後はそのアサーションだけ失敗する。
設定の `Config::load` と `validate` は成功し、他の example 試験は成功した。
この WorkUnit の指示に従って試験コードは変更していない。
`celerisctl config to-harnesses --config <file>` による全 10 件の
`config/celeris*.example.toml` の読み込みと設定検査は全件 exit 0。

## provider の LLM source / adapter 分離と cheap 専用 Qwen — 全体検証（2026-10-02、verify）

完了日 2026-10-02。ADR-0132（`agent-docs/adr/0132-provider-llm-source-split-and-cheap-qwen.md`）の D1〜D7
（provider を LLM source と adapter / harness に分ける設定・API・schema、llm-proxy の Qwen tier 写像を
cheap だけにする fallback、paperqa・langmem・ldr・opencode の Qwen 前提除去、gui/ と web/ の providers 画面、
設定例と本番移行手順）の統合後 HEAD（`ecafb5a4` 「integrate wu/worker-tools (phase tools)」）に対して
全体の fmt・clippy・ビルド・対象試験を実行した。ADR-0132 の状態欄を「実装済み」に更新した。実装そのものの
修正はこの WorkUnit では行っていない（指示どおり）。

- 証拠コマンドと結果:
  - `cargo fmt --all -- --check` → exit 0（差分なし）。
  - `cargo clippy --workspace -- -D warnings` → exit 0（warning ゼロ）。
  - `cargo test --workspace --no-run` → exit 0（全クレート・全試験バイナリのビルドに成功。実行はしていない
    — userns が要る browser 系試験が worker sandbox で落ちるため、全体の実行は daemon 側の workspace check
    に任せる）。
  - `cargo test -p llm-proxy` → exit 0。unit 39 passed、`tests/proxy_integration.rs` 27 passed、doctest 0
    （cheap のみの Qwen 優先・`celeris/frontier`・`celeris/standard` が Qwen に倒れないこと・Qwen 生存時は
    Qwen 優先・不達時は Claude/GPT cheap へ倒れることを固定する `cheap_only_*` 試験を含む）。
  - `cargo test -p celeris config::` → exit 0、86 passed（設定の読み込み・検証・互換 provider 行・
    knowledge fallback 設定を含む）。続けて `cargo test -p celeris --lib` 全体も exit 0、217 passed
    （`config::tests::loads_research_example_config` を含め、settings/proxy 化後の設定例試験もすべて pass。
    config-docs 段で期待パスの更新が済んでいるため、以前この節に記録していた既知の失敗は解消済み）。
  - `cargo test -p task-api --test providers_admin` → exit 0、13 passed（`provider_kind_*` を含む provider
    の kind・llm_source の作成・更新・互換検証）。
  - `cargo test -p task-api --test llm_sources` → exit 0、3 passed（`GET /api/v1/llm/sources` の読者ビュー）。
  - `cargo test -p task-dispatch --lib knowledge` → exit 0、9 passed（knowledge_fallback: cheap-only の
    Qwen 生存時・不達時の両経路、langmem 到達可否、probe キャッシュを含む）。
  - `cargo test -p task-dispatch --lib`（全体）→ exit 0、506 passed / 0 failed（最終レビュー受け入れ条件 0
    の `cargo test -p task-dispatch --lib` に一致）。
- 未解決事項:
  - 本番の `~/.config/celeris/config.toml` の移行はこの WorkUnit では実行していない。人が
    `docs/ops/provider-llm-source-migration.md` の手順（控え・ID 対応表・`models.qwen` の非 cheap キー削除・
    `celeris/<tier>` への切替・opencode の cheap 制限・検証・画面確認・戻し方）に従って行う。
  - browser 系など userns を要る試験（`crates/celeris/tests/browser_*` ほか）はこの worker sandbox では
    実行できない。daemon 側の workspace check（本番相当の権限を持つ環境）の `cargo test --workspace` に
    全体実行を委ねる。
  - `cargo test --workspace` のフル実行はこの run では行っていない（上記の userns 制約のため、対象クレート・
    対象試験に絞って実行した）。
- 提案: 次に本番 config.toml を移行する際は、移行前後で `GET /api/v1/providers` と `GET /api/v1/llm/sources`
  の応答を保存し比較すると、`kind` / `llm_source` の推定結果が意図どおりか目視確認しやすい。
- ACP の Qwen 直指定（`OPENCODE_CONFIG` 無し・`openai_compatible` の Qwen source）も cheap だけに絞る修正（acp-cheap）。
