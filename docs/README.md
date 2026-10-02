# docs/ — 人向け文書

agent（Celeris のタスク）がまず読むべき文書は `docs/` ではなく [`agent-docs/README.md`](../agent-docs/README.md) にある
（進捗・ADR・設計判断の経緯はすべてそちら）。

`docs/` は、人が今のシステムを理解するのに必要十分な最小限の文書だけを置く（ADR-0128 D2a）。

- [`SPEC.md`](SPEC.md) — 仕様・概要の入口
- [`architecture-map.md`](architecture-map.md) — subsystem → crate/module → entry point → ADR の索引
- `api/` — API の説明と schema（`api/v1/overview.md`, `api/v1/gui-api.md`, `api/v1/*.schema.json`）
- `protocol/` — worker protocol の説明と schema
- `guides/` — 機能・画面ごとの使い方（knowledge、mcp、providers、llm-source、workspace、browser-capability、browser-credentiald、
  repository-documentation-maintenance）
- `ops/` — 運用手順（selfdeploy、nextest、sccache-l1、web-parallel-operation）

一度きりの作業手順・経緯・日付付きの調査報告・ADR・進捗記録は `agent-docs/` にある。
