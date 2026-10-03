---
title: docs 再構成 — 壊れた文書参照の修正（cleanup-api / links）
tasks: [01M3YNFVFZG5AFFM0RS05X22ZT]
status: done
updated: 2026-10-02
---
# docs 再構成 — 壊れた文書参照の修正（cleanup-api / links）

前回の全体リンク検査で残った参照を、実在する移動先へ直すか、廃止された参照を文言に置き換えた。`gui/docs/celeris-api-v1.md` は変更禁止のため、参照先 `docs/api/v1/overview.md` を `gui-api.md` への短い案内ページとして再作成した。API の説明本文は `gui-api.md` に集約したまま、コピー元の旧パスを壊さない。

## 修正一覧

| ファイル:行 | 旧参照 | 修正後 |
|---|---|---|
| README.md:46,170 | `docs/knowledge.md` | 削除（知識ベースの運用資料への説明に変更） |
| README.md:88,168 | `docs/PROGRESS.md` | 削除（agent-docs の進捗記録への説明に変更） |
| README.md:131 | `docs/selfdeploy.md` | 削除（selfdeploy の運用資料という文言に変更） |
| README.md:165 | `docs/DESIGN.md` | 削除（現行の SPEC へ案内） |
| README.md:166-167 | `docs/adr/0046*`〜`0048*` | `agent-docs/adr/` |
| README.md:169 | `docs/providers.md` | `docs/guides/providers.md` |
| README.md:170 | `docs/workspace.md` | `docs/guides/workspace.md` |
| README.md:170 | `docs/gui/api.md` | `docs/api/v1/gui-api.md` |
| agent-docs/adr/0081-web-spa-frontend.md:9 | `../../gui/docs/adr/0002-frontend-stack.md` | `../gui/adr/0002-frontend-stack.md`（既存 agent-docs 内の ADR） |
| agent-docs/adr/0081-web-spa-frontend.md:206 | `../api/v1/api-v1.schema.json` | `../../docs/api/v1/api-v1.schema.json` |
| agent-docs/gui/design.md:6,229,431 | `api.md` | `../../docs/api/v1/gui-api.md` |
| agent-docs/gui/design.md:7,433 | `bootstrap/README.md` | 削除（bootstrap 資料が現行ツリーに無い旨を記載） |
| agent-docs/gui/design.md:7 | `docs/adr/0013-celeris-api-and-gui-foundations.md` | `../adr/0013-taskd-api-and-gui-foundations.md` |
| agent-docs/progress/phase-R.md:832 | `../ops/adr-0079-r5b-runbook.md` | 削除（当時の手順参照を記録文に置換） |
| agent-docs/progress/phase-browser.md:8 | `../browser-capability.md` | `../../docs/guides/browser-capability.md` |
| agent-docs/progress/phase-browser.md:110,142,177 | `../testing.md` | 削除（削除した stress 台本の理由を文言で維持） |
| agent-docs/progress/phase-web.md:7,79 | `../web/implementation-plan.md`, `feature-parity.md`, `gates/*` | 保持。実ファイルあり（対象外の参照を含む） |
| agent-docs/progress/phase-web.md:90,113 | `../web/dogfood.md`, `../PROGRESS.md` | 削除（dogfood の記録先の説明に置換） |
| agent-docs/reports/model-routing-2026-09-20.md:28 | `../config/celeris.model-tiers.example.toml` | `../../config/celeris.model-tiers.example.toml` |
| agent-docs/reports/model-routing-2026-09-20.md:65 | `gui/model-routing/*.png` | `../gui/model-routing/*.png` |
| docs/guides/browser-capability.md:8,127 | `adr/0078-browser-execution-capability.md` | `../../agent-docs/adr/0078-browser-execution-capability.md` |
| docs/guides/browser-capability.md:15 | `../config/celeris.acp-opencode.example.toml` | `../../config/celeris.acp-opencode.example.toml` |
| docs/guides/browser-capability.md:50 | `browser-live-relay.md` | 削除（参照先資料なしのため機能説明だけ残す） |
| docs/guides/browser-credentiald.md:7 | ADR-0080, ADR-0103 の docs/adr 相対参照 | `../../agent-docs/adr/` 内の実在 ADR |
| docs/guides/browser-credentiald.md:17 | `../deploy/systemd/celeris-credentiald@.service` | 削除（service unit の参照文字列に変更） |
| docs/guides/providers.md:123 | `selfdeploy.md` | `../ops/selfdeploy.md` |
| docs/guides/providers.md:126 | ADR-0049 の docs/adr 相対参照 | `../../agent-docs/adr/0049-portable-providers-and-codex-usage.md` |
| docs/guides/repository-documentation-maintenance.md:7 | ADR-0068 の docs/adr 相対参照 | `../../agent-docs/adr/0068-knowledge-gc-and-repository-docs-maintenance.md` |
| docs/ops/web-parallel-operation.md:15 | `adr/web-0003-parallel-operation.md` | `../../agent-docs/web/adr/web-0003-parallel-operation.md` |
| docs/ops/web-parallel-operation.md:34 | `../selfdeploy.md` | 同ディレクトリの `selfdeploy.md` |
| docs/ops/web-parallel-operation.md:105 | `../progress/phase-web.md` | `../../agent-docs/progress/phase-web.md` |
| gui/docs/celeris-api-v1.md:3 | `../../docs/api/v1/overview.md` | gui/ は変更禁止のため据え置き。移動先の転送ページを復元 |
| docs/api/v1/overview.md | 削除済みだった旧 API overview | `gui-api.md` へ誘導する短い転送ページとして再作成 |

旧ファイルの最終 commit は README.md が `4f0e1ab9`、overview.md が統合前 `a42f9a54`、その他の修正対象 Markdown は `a42f9a54`。gui の参照元は `01fd1f91`。`overview.md` は新規作成のため旧ファイルの記録では `a42f9a54` を使用。

## 検証

- `sh scripts/dev/check-doc-links.sh` — exit 0、`check-doc-links: ok`。
- `docs/api/v1/overview.md` は `gui-api.md` へのリンクのみを持つ短い案内ページ。
- `gui/`、`crates/`、`web/`、`scripts/`、schema JSON は変更していない。
