---
title: docs/guides の現行化
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
status: done
updated: 2026-10-02
---
# docs/guides の現行化

## 処理結果

| 旧パス | 処理 | 理由・照合先 |
|---|---|---|
| `docs/guides/browser-capability.md` | 修正 | `gui/app/celeris/browser-live.server.ts`・`gui/app/components/BrowserRunsPanel.tsx` の本人専用 relay と現行の credential policy を反映。旧 dashboard 手順と旧 Phase 1 の境界を除去 |
| `docs/guides/browser-credentiald.md` | 修正 | `crates/task-worker/src/browser.rs` の H3 注入経路と `crates/celeris-credentiald/src/lib.rs` の直接 resolve 拒否を反映。旧 login 試験記録を除去 |
| `docs/guides/knowledge.md` | 修正 | `crates/task-core/src/knowledge/`・`crates/celeris/src/knowledge_maint.rs`・`crates/task-dispatch/src/dispatcher/dispatch_run.rs` と照合。置き場・候補・cheap fallback は維持し、旧 API/workspace パスを更新 |
| `docs/guides/llm-source.md` | 修正 | `crates/llm-proxy/src/config.rs`・`crates/task-api/src/llm_sources.rs`・`gui/app/routes/accounts.tsx` と照合し、実装済み GUI 表示を反映。旧パスを更新 |
| `docs/guides/mcp.md` | 修正 | `crates/celerisctl/src/commands/mcp.rs`・`crates/celeris-mcp/` と照合。旧パス・過去の未検証メモ・一度きりの実機確認手順を除去 |
| `docs/guides/providers.md` | 修正 | `crates/task-dispatch/src/dispatcher/dispatch_run.rs` と設定例を照合。現行の model/provider 選択と重なる旧 routing 実験の節を短縮し、参照を更新 |
| `docs/guides/repository-documentation-maintenance.md` | 修正 | `crates/celerisctl/src/commands/docs_maintenance.rs` と照合。現行の操作は保持し、ADR の参照先を更新 |
| `docs/guides/workspace.md` | 修正 | `crates/task-core/src/workspace_config.rs`・`crates/task-worker/src/container.rs` と照合。古い worktree/PR の手順と前置きの写しを削り、設定項目と文書の置き場に絞った。docs maintenance の重複説明は専用 guide へリンク |

browser capability と credential broker は公開ページ操作と認証情報の設定で用途が異なる。providers と llm-source は worker の provider 選択と HTTP 互換 proxy の設定で用途が異なるため、それぞれ残した。workspace と repository documentation maintenance の重複は後者へのリンクに寄せた。

## 削除・統合・移動の記録

| 処理 | 旧パス | 新パス or - | 理由 | 最後の commit |
|---|---|---|---|---|

この WorkUnit ではファイル単位の削除・統合・移動は行っていない。

## 検証

- `sh scripts/dev/check-doc-links.sh docs/guides`: exit 0。
- `git diff --check`: exit 0。
- `sh scripts/dev/check-doc-links.sh`: exit 1。残る 36 件は README.md、agent-docs/、docs/api/、docs/ops/、docs/protocol/ の参照であり、この WorkUnit の範囲外。移動前から全体検査は失敗しており、guide 内の違反は解消した。

## 提案

全体のリンク検査で残る参照は担当 WorkUnit と land-verify の統合時に直す。CLAUDE.md・.claude・crates・scripts から参照される guide のパスは変えていない。
