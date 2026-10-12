---
tasks: [01M4R2JBJSJTZRKVH5JV3P0Y7N]
work_unit: web-purge
---
# web: provider 前提の残りを消す（web-purge）

ADR 2026-10-11-llm-source-harness-selection「API と画面」節に合わせ、web/ から provider 画面・provider API の呼び出し・
assignment 画面を消した。crates/ と gui/ は変えていない。

## 完了

- 完了日: 2026-10-12
- 削除: `web/api/model-assignments.ts`、`web/features/ops/model-assignments.tsx`・`.test.tsx`、`web/features/ops/model-role-editor.tsx`
- `models-screen.tsx`: `ModelAssignmentsSection` を外し、「割り当て」表示（`assigned_tiers`）を消す。routing の tier/deployment 表示は残す（API が返すため）
- `models-catalog.ts`: `assigned_tiers` を消す（daemon の API に無い欄）
- query key: `providerKeys` と `queryKeys.providers` を消す。`stale-time.ts` の `providers` を消す
- realtime: `invalidation-map.ts` の `providers` token を消し、`provider_throttled`・`quota_estimated`・`model_catalog_changed`・`model_role_assignment_changed` から `providers` を外す。`event-kinds.ts` は消していない（下の「未解決」）
- e2e: `admin/ops-config.spec.ts` の providers 3 件（`/providers` 画面）を消す。`parity/ops.spec.ts` の `/providers` 試験を消す（describe 名は `P4-12 daemon`）。`work/narrow-r6.spec.ts` の (1)(7)（`/providers` 幅試験）を消す。`admin/models.spec.ts` の役割割り当て試験と「provider を追加」試験を消す
- `e2e/support/fake-daemon.mjs`: `/api/v1/providers`・`/api/v1/reload` の偽応答、`modelAssignmentsFixture`・`assignments` 経路を消す。`/api/v1/replay` は残す
- `agent-docs/web/feature-parity.md` R30（`/providers`）は直していない。この unit の範囲（web/ と本進捗ファイル）の外のため、変更は revert した。そのため `check:parity` は R30 の title 欠落で exit 1 のまま（下の「未解決事項」）

## 証拠

| 条件 | 実行したコマンド | 結果 |
|---|---|---|
| typecheck | `corepack pnpm@12.6.0 typecheck` | exit 0（`tsc -b`） |
| lint | `corepack pnpm@12.6.0 lint` | exit 0。warning 4 件は `styles.css` の既存 `noImportantStyles`（未変更） |
| test | `corepack pnpm@12.6.0 test`（TMPDIR=/local/x/wp） | exit 0。vitest 90 files / 650 tests passed、node --test fail 0 |
| build | `corepack pnpm@12.6.0 build` | exit 0 |
| parity 検査 | `corepack pnpm@12.6.0 check:parity` | exit 1: `R30: missing parity test title`（feature-parity.md の R30 が `/providers` の試験名を指したまま） |
| boundaries / secrets | `check:boundaries`、`check:secrets` | 未実行 |
| e2e（変更 spec） | `WEB_E2E_SCOPE=functional playwright test e2e/admin/models.spec.ts e2e/admin/ops-config.spec.ts e2e/parity/ops.spec.ts e2e/work/narrow-r6.spec.ts --workers=2` | 26 passed, 0 failed |
| 残存 grep | `grep -rn "api/providers\|providerKeys\|model-assignments\|api/v1/providers\|/providers\|assigned_tiers" web` | 一致なし |
| crates・gui 差分 | `git diff HEAD -- crates gui` | 空 |

全 e2e suite は流していない（変更した 4 spec のみ）。

## 未解決事項

- `event-kinds.ts` に残る `provider_throttled`・`model_role_assignment_changed` は、生成 schema（`Event` enum）にまだ在るので残した。`invalidation-map.test.ts` は schema の全 kind を map と突き合わせるため、schema から消えるまで消せない。Rust 側で variant を消す（追記専用 events の扱いを決める）作業の後に消す。
- `models-screen` の `tier` 表示・override の `tier` 欄・`celeris_tiers`（fake daemon）は、ADR の決定 2（tier alias 撤去）の対象。crates の変更を待つ。
- `agent-docs/web/feature-parity.md` の R30 行（`/providers`）の更新が要る。`check:parity` が exit 1 のまま。closeout 葉（文書）で直す。
- 範囲 check の 2 つ目（`git diff --quiet $(git merge-base HEAD main) -- crates/`）は、この branch の HEAD と merge-base（4ef38473）の間に crates の変更が 341 file ある（main 側の統合分）ため、web の変更とは無関係に失敗する。base（`CELERIS_WU_BASE`）基準の crates・gui の差分はゼロ。
- 本番の migration・config の書き換えは無し（web の変更のみ）。

## 提案

- R30 の `廃止` 行は、parity 台帳の「全行完了で gui 切替」の gate と噛み合わない。gate 側が廃止行を数えないように直すか、人が判断する。
