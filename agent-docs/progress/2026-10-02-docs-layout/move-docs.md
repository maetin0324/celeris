---
title: docs/ の人向け/agent 向け再配置（move-docs）
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
status: done
updated: 2026-10-02
---

# move-docs: tsv どおりの git mv / git rm と移行期間の形

ADR-0128（`agent-docs/adr/0128-docs-layout.md`）と `scripts/dev/docs-layout.tsv`（amend 後）どおりに、
`docs/` 196 ファイルのうち 172 件を `git mv`、10 件を `git rm`、15 件は旧=新で変更なし。

## 実施したこと

- `scripts/dev/docs-layout.tsv` の全行を機械的に適用（`git mv` は内容を変えない、`git rm` は delete 行）。
- `docs/PROGRESS.md` を `git mv` で `agent-docs/PROGRESS.md` へ移し、1 行目より前に追記終了の案内を追加（既存本文は変更なし）。
- `docs/progress/README.md`・`docs/adr/README.md` を新設（「agent-docs/… へ移った、新しいファイルを置かない」）。
- `docs/README.md`（人向け入口、冒頭に `agent-docs/README.md` への案内）、`agent-docs/README.md`（読む順）を新設。
- `scripts/dev/check-doc-layout.sh` を新設（POSIX sh/dash。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` で exit 0 を確認）。
- `docs/ops/adr-0079-r5b-runbook.md` の削除で壊れる `crates/task-api/tests/tree_adopt.rs` の `include_str!` を修正:
  手順書が持っていた 4 つの JSON（`browser-root.json`, `browser-plan.json`, `benchfs-root.json`, `benchfs-plan.json`）を
  `crates/task-api/tests/fixtures/` へ実ファイルとして切り出し、`runbook_json()` を `std::fs::read_to_string` でそれを読むように変更
  （試験の意味は変えない。6 試験 `cargo test -p task-api --test tree_adopt` は変更前と同じ 6 件 pass）。

## 証拠

- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → `check-doc-layout: ok`、exit 0。
- `cargo test -p task-api --test tree_adopt` → 6 passed; 0 failed（`the_r5b_runbook_plans_are_accepted_as_written` を含む）。
- `git status --short` の全エントリが `R` （rename、172 件）または `D`（delete、10 件）、新規は `crates/task-api/tests/fixtures/*.json` と
  この記録・`docs/README.md`・`agent-docs/README.md`・`docs/progress/README.md`・`docs/adr/README.md`・`scripts/dev/check-doc-layout.sh` のみ。
- `git log --follow -- agent-docs/PROGRESS.md` で旧 `docs/PROGRESS.md` までの履歴が辿れる（git mv のため rename として記録）。

## 未解決・次の WorkUnit への申し送り

- `refs-crates` / `refs-repo` / `refs-worker`: この WU は docs のパス参照の追従（CLAUDE.md、prompt、他 crate、scripts、architecture-map 等）を
  行っていない。`docs/api/v1/api-v1.schema.json` を読む `crates/task-api/src/schema.rs` の `include_str!` はパス自体が旧=新で
  変わっていないため今回の修正は不要（確認済み）。
- `scripts/dev/check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh` は doc-tools WU の担当のため本 WU では書いていない。
- 移行期間の終了処理（`docs/progress/README.md`・`docs/adr/README.md` の削除、`agent-docs/PROGRESS.md` のリンク修復）は ADR-0128 D6 どおり
  cleanup の後続 task が行う。

## delete 行（D9: 削除の記録）
| 処理 | 旧パス | 新パス | 理由 | 最後の commit |
|---|---|---|---|---|
| 削除 | `docs/DESIGN.md` | - | 現行の仕様の入口は docs/SPEC.md とし、CLAUDE.md の参照もそちらへ変える（ADR-0079 D7 人の決定、ADR-0128 D8）。人の確認で削除。 | `ea69c29a` |
| 削除 | `docs/gui/bootstrap/CLAUDE.md` | - | 初期配布用の作業規則だが、現行の作業規則は gui/CLAUDE.md に配置済みの重複。人の確認で削除。 | `926e19c0` |
| 削除 | `docs/gui/bootstrap/GOAL_TEMPLATE.md` | - | 初期配布用テンプレートだが、現行の同名成果は gui/docs/GOAL_TEMPLATE.md に配置済みの重複。人の確認で削除。 | `926e19c0` |
| 削除 | `docs/gui/bootstrap/PROGRESS.md` | - | G0〜G5 を全て未着手とする初期配布状態だが、現行の進捗は gui/docs/PROGRESS.md に記録済み。人の確認で削除。 | `926e19c0` |
| 削除 | `docs/gui/bootstrap/README.md` | - | run-gphases.sh による gui/ 作成と git init を指示するが、gui/ は既に追跡・実装済みで run-gphases.sh は存在しない。人の確認で削除。 | `926e19c0` |
| 削除 | `docs/gui/bootstrap/agents/auditor.md` | - | 初期コピー元だが、現行の agent 定義は gui/.claude/agents/auditor.md に配置済みの重複。人の確認で削除。 | `926e19c0` |
| 削除 | `docs/gui/bootstrap/agents/implementer.md` | - | 初期コピー元だが、現行の agent 定義は gui/.claude/agents/implementer.md に配置済みの重複。人の確認で削除。 | `926e19c0` |
| 削除 | `docs/ops/adr-0079-r5b-runbook.md` | - | ADR-0079 R5b の一度きりの作業手順であり、現行の運用文書ではない。人の確認で削除（JSON フィクスチャは crates/task-api/tests/fixtures/ へ切り出し）。 | `243009c3` |
| 削除 | `docs/ops/home-nfs-migration-2026-09-25.md` | - | 2026-09-25 の一度きりの移行手順であり、現行の運用文書ではない。人の確認で削除。 | `d0e78511` |
| 削除 | `docs/web/dogfood.md` | - | 起動・確認手順と H6/H9 の未決定・問題記録が混在する一度きりの作業文書で、現行の運用文書は docs/ops/web-parallel-operation.md にある。人の確認で削除。 | `c63d53c2` |

## move 行（172 件、git mv で rename として記録）
| 処理 | 旧パス | 新パス | 理由 | 最後の commit（移動前） |
|---|---|---|---|---|
| 移動 | `docs/GOAL_TEMPLATE.md` | `agent-docs/GOAL_TEMPLATE.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `8aa8115c` |
| 移動 | `docs/PROGRESS.md` | `agent-docs/PROGRESS.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `0229a78c` |
| 移動 | `docs/adr/0001-scope-and-principles.md` | `agent-docs/adr/0001-scope-and-principles.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `ce983f8c` |
| 移動 | `docs/adr/0002-state-machine.md` | `agent-docs/adr/0002-state-machine.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `ce983f8c` |
| 移動 | `docs/adr/0003-worker-protocol.md` | `agent-docs/adr/0003-worker-protocol.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `ce983f8c` |
| 移動 | `docs/adr/0004-taskctl-cli.md` | `agent-docs/adr/0004-taskctl-cli.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `0b2339b4` |
| 移動 | `docs/adr/0005-phase3-dispatch-and-worker.md` | `agent-docs/adr/0005-phase3-dispatch-and-worker.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `8aa8115c` |
| 移動 | `docs/adr/0006-phase4-claude-code-adapter.md` | `agent-docs/adr/0006-phase4-claude-code-adapter.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `65efe9a1` |
| 移動 | `docs/adr/0007-phase5-planner-and-reviewer.md` | `agent-docs/adr/0007-phase5-planner-and-reviewer.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `db363bdd` |
| 移動 | `docs/adr/0008-phase6-approval-gate-and-codex-adapter.md` | `agent-docs/adr/0008-phase6-approval-gate-and-codex-adapter.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `3b74cbf5` |
| 移動 | `docs/adr/0009-proposal-adoption-and-phase-closure.md` | `agent-docs/adr/0009-proposal-adoption-and-phase-closure.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `80d761fa` |
| 移動 | `docs/adr/0010-phase7-hardening.md` | `agent-docs/adr/0010-phase7-hardening.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `55294885` |
| 移動 | `docs/adr/0011-requeue-limit.md` | `agent-docs/adr/0011-requeue-limit.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `aa91dd44` |
| 移動 | `docs/adr/0012-multi-account-and-worker-run.md` | `agent-docs/adr/0012-multi-account-and-worker-run.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `aa91dd44` |
| 移動 | `docs/adr/0013-taskd-api-and-gui-foundations.md` | `agent-docs/adr/0013-taskd-api-and-gui-foundations.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `072042b8` |
| 移動 | `docs/adr/0014-reviewer-run-events-objective-search-create-validation.md` | `agent-docs/adr/0014-reviewer-run-events-objective-search-create-validation.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `61799010` |
| 移動 | `docs/adr/0015-observability-and-taskref-actions.md` | `agent-docs/adr/0015-observability-and-taskref-actions.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `ae1297b7` |
| 移動 | `docs/adr/0016-roles-and-delegation.md` | `agent-docs/adr/0016-roles-and-delegation.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `2212f8c5` |
| 移動 | `docs/adr/0017-account-management-from-gui.md` | `agent-docs/adr/0017-account-management-from-gui.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `2d6d5ba1` |
| 移動 | `docs/adr/0018-remote-clusters-over-ssh.md` | `agent-docs/adr/0018-remote-clusters-over-ssh.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `0ed4a1d1` |
| 移動 | `docs/adr/0019-worktree-sync-for-large-repositories.md` | `agent-docs/adr/0019-worktree-sync-for-large-repositories.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `ae1f1f8c` |
| 移動 | `docs/adr/0020-single-repository-with-the-gui.md` | `agent-docs/adr/0020-single-repository-with-the-gui.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `99062878` |
| 移動 | `docs/adr/0021-parent-retries-when-a-delegated-child-fails.md` | `agent-docs/adr/0021-parent-retries-when-a-delegated-child-fails.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `b4c5a175` |
| 移動 | `docs/adr/0022-provider-check-results-and-no-directory-watching.md` | `agent-docs/adr/0022-provider-check-results-and-no-directory-watching.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `661485d1` |
| 移動 | `docs/adr/0023-liveness-interval-run-request-and-awaiting-children.md` | `agent-docs/adr/0023-liveness-interval-run-request-and-awaiting-children.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `661485d1` |
| 移動 | `docs/adr/0024-claude-account-pool-and-budget-balancing.md` | `agent-docs/adr/0024-claude-account-pool-and-budget-balancing.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `97f43ac2` |
| 移動 | `docs/adr/0025-codex-accounts-in-the-pool.md` | `agent-docs/adr/0025-codex-accounts-in-the-pool.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e0f12dde` |
| 移動 | `docs/adr/0026-acp-worker-adapter.md` | `agent-docs/adr/0026-acp-worker-adapter.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `827908ac` |
| 移動 | `docs/adr/0027-task-genres-and-research-harness.md` | `agent-docs/adr/0027-task-genres-and-research-harness.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `9b5a9282` |
| 移動 | `docs/adr/0028-genre-manifest.md` | `agent-docs/adr/0028-genre-manifest.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `31192d74` |
| 移動 | `docs/adr/0029-web-research-harness.md` | `agent-docs/adr/0029-web-research-harness.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `2772896d` |
| 移動 | `docs/adr/0030-api-keys-from-the-gui.md` | `agent-docs/adr/0030-api-keys-from-the-gui.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `553f6377` |
| 移動 | `docs/adr/0031-web-research-evidence-gate.md` | `agent-docs/adr/0031-web-research-evidence-gate.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `4e617b39` |
| 移動 | `docs/adr/0032-cluster-connect-from-the-gui.md` | `agent-docs/adr/0032-cluster-connect-from-the-gui.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `0ed4a1d1` |
| 移動 | `docs/adr/0033-organization-projects-and-reports.md` | `agent-docs/adr/0033-organization-projects-and-reports.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `cf05d9f0` |
| 移動 | `docs/adr/0034-report-generation-and-compaction.md` | `agent-docs/adr/0034-report-generation-and-compaction.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `7528b18a` |
| 移動 | `docs/adr/0035-literature-acquisition.md` | `agent-docs/adr/0035-literature-acquisition.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `4e617b39` |
| 移動 | `docs/adr/0036-per-task-artifacts.md` | `agent-docs/adr/0036-per-task-artifacts.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `834cce66` |
| 移動 | `docs/adr/0037-discord-notifications.md` | `agent-docs/adr/0037-discord-notifications.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `26d6eada` |
| 移動 | `docs/adr/0038-milestone-review-dialogue.md` | `agent-docs/adr/0038-milestone-review-dialogue.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `31192d74` |
| 移動 | `docs/adr/0039-project-workspace.md` | `agent-docs/adr/0039-project-workspace.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `72959ccb` |
| 移動 | `docs/adr/0040-self-improvement-deploy.md` | `agent-docs/adr/0040-self-improvement-deploy.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `c4a2ec41` |
| 移動 | `docs/adr/0041-self-improvement-loop-hardening.md` | `agent-docs/adr/0041-self-improvement-loop-hardening.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `248d7888` |
| 移動 | `docs/adr/0042-celeris-naming-and-layout.md` | `agent-docs/adr/0042-celeris-naming-and-layout.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `4152b0a5` |
| 移動 | `docs/adr/0043-workspaces.md` | `agent-docs/adr/0043-workspaces.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `d10a7d25` |
| 移動 | `docs/adr/0044-task-management.md` | `agent-docs/adr/0044-task-management.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `31192d74` |
| 移動 | `docs/adr/0045-rename-to-celeris.md` | `agent-docs/adr/0045-rename-to-celeris.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `51bafff4` |
| 移動 | `docs/adr/0046-organization-as-agent-profiles.md` | `agent-docs/adr/0046-organization-as-agent-profiles.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `0ed4a1d1` |
| 移動 | `docs/adr/0047-knowledge-base.md` | `agent-docs/adr/0047-knowledge-base.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `408e2cfe` |
| 移動 | `docs/adr/0048-console.md` | `agent-docs/adr/0048-console.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `31192d74` |
| 移動 | `docs/adr/0049-portable-providers-and-codex-usage.md` | `agent-docs/adr/0049-portable-providers-and-codex-usage.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `4e9fba43` |
| 移動 | `docs/adr/0050-request-completion-and-notifications.md` | `agent-docs/adr/0050-request-completion-and-notifications.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `c6a9977d` |
| 移動 | `docs/adr/0051-supervised-delivery.md` | `agent-docs/adr/0051-supervised-delivery.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `c300b4a5` |
| 移動 | `docs/adr/0052-knowledge-run-fallback.md` | `agent-docs/adr/0052-knowledge-run-fallback.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `4e617b39` |
| 移動 | `docs/adr/0053-llm-source-proxy.md` | `agent-docs/adr/0053-llm-source-proxy.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `d46675f6` |
| 移動 | `docs/adr/0054-stateful-sessions-and-streaming-chat.md` | `agent-docs/adr/0054-stateful-sessions-and-streaming-chat.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `c6995fc1` |
| 移動 | `docs/adr/0055-mobile-ux.md` | `agent-docs/adr/0055-mobile-ux.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `14809897` |
| 移動 | `docs/adr/0056-mcp-server.md` | `agent-docs/adr/0056-mcp-server.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `d8ab32e9` |
| 移動 | `docs/adr/0057-console-composer-layout-level.md` | `agent-docs/adr/0057-console-composer-layout-level.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `ad345981` |
| 移動 | `docs/adr/0058-release-verify-breakdown.md` | `agent-docs/adr/0058-release-verify-breakdown.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `cccc3041` |
| 移動 | `docs/adr/0059-command-only-remote-workspace.md` | `agent-docs/adr/0059-command-only-remote-workspace.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `45e07523` |
| 移動 | `docs/adr/0060-ssh-master-outside-the-daemon-cgroup.md` | `agent-docs/adr/0060-ssh-master-outside-the-daemon-cgroup.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `367387aa` |
| 移動 | `docs/adr/0061-coding-harness-routing-foundation.md` | `agent-docs/adr/0061-coding-harness-routing-foundation.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `ac974ede` |
| 移動 | `docs/adr/0062-ssh-master-keepalive-and-cluster-tool-routing.md` | `agent-docs/adr/0062-ssh-master-keepalive-and-cluster-tool-routing.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a4b05d92` |
| 移動 | `docs/adr/0063-research-tasks-resilience.md` | `agent-docs/adr/0063-research-tasks-resilience.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e342626a` |
| 移動 | `docs/adr/0064-db-local-disk-and-store-resilience.md` | `agent-docs/adr/0064-db-local-disk-and-store-resilience.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `4aab35ad` |
| 移動 | `docs/adr/0066-worker-build-cache-and-tunnel-probe-backoff.md` | `agent-docs/adr/0066-worker-build-cache-and-tunnel-probe-backoff.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `9dc0431a` |
| 移動 | `docs/adr/0067-human-deliverables-policy-and-approval-visibility.md` | `agent-docs/adr/0067-human-deliverables-policy-and-approval-visibility.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e9468709` |
| 移動 | `docs/adr/0068-knowledge-gc-and-repository-docs-maintenance.md` | `agent-docs/adr/0068-knowledge-gc-and-repository-docs-maintenance.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `5d80fa15` |
| 移動 | `docs/adr/0069-routing-four-layers.md` | `agent-docs/adr/0069-routing-four-layers.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `0d0b4d9a` |
| 移動 | `docs/adr/0070-task-failure-visibility-and-handoff-safe-runs.md` | `agent-docs/adr/0070-task-failure-visibility-and-handoff-safe-runs.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `3a3d747a` |
| 移動 | `docs/adr/0071-gui-resume-recovery.md` | `agent-docs/adr/0071-gui-resume-recovery.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `68535ccc` |
| 移動 | `docs/adr/0072-task-execution-decomposition.md` | `agent-docs/adr/0072-task-execution-decomposition.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `f08d4439` |
| 移動 | `docs/adr/0073-ui-ux-section.md` | `agent-docs/adr/0073-ui-ux-section.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `f7338ad1` |
| 移動 | `docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md` | `agent-docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `f8e008ce` |
| 移動 | `docs/adr/0075-tiered-build-cache.md` | `agent-docs/adr/0075-tiered-build-cache.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `30987223` |
| 移動 | `docs/adr/0076-planner-reviewer-quota-roles.md` | `agent-docs/adr/0076-planner-reviewer-quota-roles.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `288b20f6` |
| 移動 | `docs/adr/0077-milestone-in-progress-auto-reach.md` | `agent-docs/adr/0077-milestone-in-progress-auto-reach.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `31192d74` |
| 移動 | `docs/adr/0078-browser-execution-capability.md` | `agent-docs/adr/0078-browser-execution-capability.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `2d4ff3ac` |
| 移動 | `docs/adr/0078-ssh-master-persist-independent-of-daemon.md` | `agent-docs/adr/0078-ssh-master-persist-independent-of-daemon.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `6a571bd4` |
| 移動 | `docs/adr/0079-recursive-task-decomposition.md` | `agent-docs/adr/0079-recursive-task-decomposition.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `5eb666f6` |
| 移動 | `docs/adr/0080-browser-phase2-policy-broker-approval.md` | `agent-docs/adr/0080-browser-phase2-policy-broker-approval.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `ae0d5503` |
| 移動 | `docs/adr/0081-web-spa-frontend.md` | `agent-docs/adr/0081-web-spa-frontend.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `6f0dcf7c` |
| 移動 | `docs/adr/0082-dispatcher-module-split.md` | `agent-docs/adr/0082-dispatcher-module-split.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `31e36802` |
| 移動 | `docs/adr/0083-source-size-guardrail.md` | `agent-docs/adr/0083-source-size-guardrail.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a3bbfe08` |
| 移動 | `docs/adr/0089-cos-runs-bypass-concurrency.md` | `agent-docs/adr/0089-cos-runs-bypass-concurrency.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `0a822692` |
| 移動 | `docs/adr/0090-durable-wait-for-cluster-jobs.md` | `agent-docs/adr/0090-durable-wait-for-cluster-jobs.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `85993e73` |
| 移動 | `docs/adr/0095-worker-runs-see-the-db-read-only.md` | `agent-docs/adr/0095-worker-runs-see-the-db-read-only.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `0e5870ce` |
| 移動 | `docs/adr/0098-worker-created-tasks-inherit-the-origin-project.md` | `agent-docs/adr/0098-worker-created-tasks-inherit-the-origin-project.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `66db9ae9` |
| 移動 | `docs/adr/0099-browser-phase3-control-lease.md` | `agent-docs/adr/0099-browser-phase3-control-lease.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0100-browser-phase3-live-proxy-acl.md` | `agent-docs/adr/0100-browser-phase3-live-proxy-acl.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0101-browser-phase3-identity-contract.md` | `agent-docs/adr/0101-browser-phase3-identity-contract.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0102-browser-phase4-isolation-injection-routing.md` | `agent-docs/adr/0102-browser-phase4-isolation-injection-routing.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0103-browser-phase4-runtime-selection.md` | `agent-docs/adr/0103-browser-phase4-runtime-selection.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0104-browser-egress-transport.md` | `agent-docs/adr/0104-browser-egress-transport.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0105-browser-p4a-same-uid-bwrap-runtime.md` | `agent-docs/adr/0105-browser-p4a-same-uid-bwrap-runtime.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0106-browser-phase4-conformance-dispatch.md` | `agent-docs/adr/0106-browser-phase4-conformance-dispatch.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0107-browser-fallback-candidate-preparation.md` | `agent-docs/adr/0107-browser-fallback-candidate-preparation.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0108-browser-p4a-relay-supervisor-launch-restore.md` | `agent-docs/adr/0108-browser-p4a-relay-supervisor-launch-restore.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0109-browser-p4b-injection-ipc-cdp-sink.md` | `agent-docs/adr/0109-browser-p4b-injection-ipc-cdp-sink.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0110-browser-p4b-h3-shared-cdp-trusted-selector.md` | `agent-docs/adr/0110-browser-p4b-h3-shared-cdp-trusted-selector.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0111-browser-p4b-redisplay-guard-wiring.md` | `agent-docs/adr/0111-browser-p4b-redisplay-guard-wiring.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0112-browser-p4b-conformance-evidence-unlock.md` | `agent-docs/adr/0112-browser-p4b-conformance-evidence-unlock.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0113-browser-p3c-control-gate-action-server.md` | `agent-docs/adr/0113-browser-p3c-control-gate-action-server.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0114-browser-p4a-restore-deliver-state.md` | `agent-docs/adr/0114-browser-p4a-restore-deliver-state.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/adr/0115-browser-ptrace-owner-ns-launcher.md` | `agent-docs/adr/0115-browser-ptrace-owner-ns-launcher.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `bbccc23d` |
| 移動 | `docs/adr/0117-review-human-decisions-and-check-results.md` | `agent-docs/adr/0117-review-human-decisions-and-check-results.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `85486f09` |
| 移動 | `docs/adr/0121-root-delivery-without-assignee.md` | `agent-docs/adr/0121-root-delivery-without-assignee.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `c300b4a5` |
| 移動 | `docs/adr/0122-ui-ux-external-skills.md` | `agent-docs/adr/0122-ui-ux-external-skills.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `fea49183` |
| 移動 | `docs/adr/0128-docs-layout.md` | `agent-docs/adr/0128-docs-layout.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `21abef99` |
| 移動 | `docs/browser-capability.md` | `docs/guides/browser-capability.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `e9b174d3` |
| 移動 | `docs/browser-credentiald.md` | `docs/guides/browser-credentiald.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `a71bb2ce` |
| 移動 | `docs/browser-live-relay.md` | `agent-docs/reports/browser-live-relay.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `4cedc73f` |
| 移動 | `docs/celeris-api-v1.md` | `docs/api/v1/overview.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `85993e73` |
| 移動 | `docs/execution-architecture-2026-09-24.md` | `agent-docs/reports/execution-architecture-2026-09-24.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `573e9a51` |
| 移動 | `docs/execution-decomposition-report-2026-09-25.md` | `agent-docs/reports/execution-decomposition-report-2026-09-25.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `1dcd760a` |
| 移動 | `docs/execution-parallel-report-2026-09-28.md` | `agent-docs/reports/execution-parallel-report-2026-09-28.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `c4aa38e8` |
| 移動 | `docs/gui/DESIGN-GUI.md` | `agent-docs/gui/design.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `926e19c0` |
| 移動 | `docs/gui/adr/0001-architecture-boundary.md` | `agent-docs/gui/adr/0001-architecture-boundary.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `926e19c0` |
| 移動 | `docs/gui/adr/0002-frontend-stack.md` | `agent-docs/gui/adr/0002-frontend-stack.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `cf5bdf1c` |
| 移動 | `docs/gui/api.md` | `docs/api/v1/gui-api.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `0a822692` |
| 移動 | `docs/gui/celeris-proposals.md` | `agent-docs/gui/celeris-proposals.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `926e19c0` |
| 移動 | `docs/gui/model-routing/accounts-1440.png` | `agent-docs/gui/model-routing/accounts-1440.png` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e97f8072` |
| 移動 | `docs/gui/model-routing/accounts-360.png` | `agent-docs/gui/model-routing/accounts-360.png` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e97f8072` |
| 移動 | `docs/gui/model-routing/accounts-393.png` | `agent-docs/gui/model-routing/accounts-393.png` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e97f8072` |
| 移動 | `docs/gui/model-routing/accounts-412.png` | `agent-docs/gui/model-routing/accounts-412.png` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e97f8072` |
| 移動 | `docs/gui/model-routing/providers-1440.png` | `agent-docs/gui/model-routing/providers-1440.png` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e97f8072` |
| 移動 | `docs/gui/model-routing/providers-360.png` | `agent-docs/gui/model-routing/providers-360.png` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e97f8072` |
| 移動 | `docs/gui/model-routing/providers-393.png` | `agent-docs/gui/model-routing/providers-393.png` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e97f8072` |
| 移動 | `docs/gui/model-routing/providers-412.png` | `agent-docs/gui/model-routing/providers-412.png` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e97f8072` |
| 移動 | `docs/knowledge.md` | `docs/guides/knowledge.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `408e2cfe` |
| 移動 | `docs/llm-source.md` | `docs/guides/llm-source.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `379627b3` |
| 移動 | `docs/mcp.md` | `docs/guides/mcp.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `1bb30c90` |
| 移動 | `docs/model-routing-2026-09-20.md` | `agent-docs/reports/model-routing-2026-09-20.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `baa2929f` |
| 移動 | `docs/notes/build-cache-tiering-input-2026-09-28.md` | `agent-docs/notes/build-cache-tiering-input-2026-09-28.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `1b050a00` |
| 移動 | `docs/ops/browser-isolated-runtime-subuid.md` | `agent-docs/ops/browser-isolated-runtime-subuid.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/ops/dev-builds-local-target-dir.md` | `agent-docs/ops/dev-builds-local-target-dir.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `c542e5a5` |
| 移動 | `docs/ops/ui-ux-external-skills.md` | `agent-docs/ops/ui-ux-external-skills.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `99a529c6` |
| 移動 | `docs/progress/browser-followups.md` | `agent-docs/progress/browser-followups.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `4e3b1208` |
| 移動 | `docs/progress/phase-001-050.md` | `agent-docs/progress/phase-001-050.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `1ba256f3` |
| 移動 | `docs/progress/phase-051-100.md` | `agent-docs/progress/phase-051-100.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `1ba256f3` |
| 移動 | `docs/progress/phase-101-150.md` | `agent-docs/progress/phase-101-150.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `1ba256f3` |
| 移動 | `docs/progress/phase-E.md` | `agent-docs/progress/phase-E.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `1ba256f3` |
| 移動 | `docs/progress/phase-F.md` | `agent-docs/progress/phase-F.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `f08d4439` |
| 移動 | `docs/progress/phase-G.md` | `agent-docs/progress/phase-G.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `48dc00a6` |
| 移動 | `docs/progress/phase-K.md` | `agent-docs/progress/phase-K.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `081695c2` |
| 移動 | `docs/progress/phase-P0-dispatcher.md` | `agent-docs/progress/phase-P0-dispatcher.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `9f266ffa` |
| 移動 | `docs/progress/phase-R.md` | `agent-docs/progress/phase-R.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `7f3482a3` |
| 移動 | `docs/progress/phase-browser-2.md` | `agent-docs/progress/phase-browser-2.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `d5ec0cd7` |
| 移動 | `docs/progress/phase-browser-3.md` | `agent-docs/progress/phase-browser-3.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/progress/phase-browser-4.md` | `agent-docs/progress/phase-browser-4.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/progress/phase-browser-acceptance.md` | `agent-docs/progress/phase-browser-acceptance.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a71bb2ce` |
| 移動 | `docs/progress/phase-browser-main-merge.md` | `agent-docs/progress/phase-browser-main-merge.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `76b5b5bb` |
| 移動 | `docs/progress/phase-browser.md` | `agent-docs/progress/phase-browser.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `9d022095` |
| 移動 | `docs/progress/phase-guardrail.md` | `agent-docs/progress/phase-guardrail.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `a3bbfe08` |
| 移動 | `docs/progress/phase-structure-refactor.md` | `agent-docs/progress/phase-structure-refactor.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `2494ce0b` |
| 移動 | `docs/progress/phase-web.md` | `agent-docs/progress/phase-web.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `80e4129f` |
| 移動 | `docs/progress/ui-ux-skills.md` | `agent-docs/progress/ui-ux-skills.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `6d95a306` |
| 移動 | `docs/providers.md` | `docs/guides/providers.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `0a822692` |
| 移動 | `docs/repository-documentation-maintenance.md` | `docs/guides/repository-documentation-maintenance.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `5d80fa15` |
| 移動 | `docs/selfdeploy.md` | `docs/ops/selfdeploy.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `8bb6a99b` |
| 移動 | `docs/testing.md` | `agent-docs/guides/testing.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `9d022095` |
| 移動 | `docs/web/adr/web-0001-sse-invalidate-unlisted-kinds.md` | `agent-docs/web/adr/web-0001-sse-invalidate-unlisted-kinds.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `c63d53c2` |
| 移動 | `docs/web/adr/web-0002-project-plan-milestone-successors.md` | `agent-docs/web/adr/web-0002-project-plan-milestone-successors.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `c63d53c2` |
| 移動 | `docs/web/adr/web-0003-parallel-operation.md` | `agent-docs/web/adr/web-0003-parallel-operation.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `6f0dcf7c` |
| 移動 | `docs/web/feature-parity.md` | `agent-docs/web/feature-parity.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `59db6e70` |
| 移動 | `docs/web/gates/p5-01-latency.md` | `agent-docs/web/gates/p5-01-latency.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `46b3275f` |
| 移動 | `docs/web/gates/p5-02-security.md` | `agent-docs/web/gates/p5-02-security.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `e263ec4e` |
| 移動 | `docs/web/gates/p5-03-mobile-a11y.md` | `agent-docs/web/gates/p5-03-mobile-a11y.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `ba629384` |
| 移動 | `docs/web/implementation-plan.md` | `agent-docs/web/implementation-plan.md` | ADR-0128 D1 分類: agent（進捗・ADR・経緯の記録として agent-docs/ へ） | `3de38913` |
| 移動 | `docs/web/parallel-operation.md` | `docs/ops/web-parallel-operation.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `8bb6a99b` |
| 移動 | `docs/workspace.md` | `docs/guides/workspace.md` | ADR-0128 D1 分類: human（現行の仕様・手順として docs/ に残す） | `5d80fa15` |
