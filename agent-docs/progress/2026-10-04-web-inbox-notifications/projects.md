---
tasks: [01M43690B86J9GHP87CARYM8RS]
unit: projects
status: done
completed: 2026-10-04
---
# projects: 案件一覧・案件詳細を milestone と task の木の table に

差分の基点は 171c8e02（task 起点）。触った file は web/features/projects の board 以外（project-*.ts(x)）と web/e2e/parity/projects.spec.ts だけ。routes/projects.*.tsx・API 型・gateway・crates/・gui/ は変えていない。

## 変えたもの
- `/projects`（R09）: card 状の `ul` を `Table`（案件・状態・途中目標・判断待ち・最終更新）へ。状態は token の Badge、途中目標は「達成 / 全数」（中止・再設計を除く。詳細の query と cache 共有）、判断待ちは `GET /inbox/items` の `project_id` を数えて受信箱へ link。入力欄は `--color-input`（生の `border-neutral-400 bg-white` を除去）。
- `/projects/:id`（R10）: 「途中目標と仕事」を 1 つの表に。途中目標ごとの `tbody`（区切り行 = 途中目標の題・MilestoneBadge・子の進み）、task 行は字下げ（token の `ps-*`、深さは sr-only でも出す）と StatusBadge、判断待ちのある task は「判断待ち N 件」で `/inbox` へ（`GET /inbox/items?project=` の task と blocking.tasks）。途中目標の無い仕事は最後の束。計画の DAG は木の後ろ、操作はその後ろへ。
- 破壊的操作: `project_cancel`・`project_archive`・`milestone_status`（段階を取り下げる）・`milestone_cancel`・`repo_delete` を `ConfirmDialog`（対象・結果・戻し方・確認先）経由に。旧 `<dialog>` 自前実装は削除。ボタンは `Button` primitive。
- 文書画面の入力欄の生の色も token に。
- 純関数 `flattenTree`・`milestoneGroups`・`milestoneProgress`・`pendingByProject`・`pendingByTask`（project-structure.ts）と単体試験。

R09/R10 の intent（feature-parity の列挙 22 個 + 作成・絞り込み）は全部残る。parity spec の題名・h1・URL は不変。spec の変更は dialog の role（alertdialog）と確認ボタン名、表の列・途中目標の束・深さ・受信箱 link の検査を足したこと。

## 証拠
- `corepack pnpm@12.6.0 -C web e2e e2e/parity/projects.spec.ts` → 9 passed
- `… e2e --grep-invert 'S1 /' e2e/parity/latency-gate.spec.ts e2e/shell e2e/parity/runs-files.spec.ts e2e/a11y` → 46 passed
- `… e2e e2e/latency/transition.spec.ts --grep projects` → 4 passed
- `corepack pnpm@12.6.0 -C web mobile-audit` → 30 path(s) x 4 widths ok
- `typecheck` exit 0、`lint` exit 0（既存の warning 4 件は skills-screen と styles.css で範囲外）、`vitest run features/projects` 9 passed
- FRONTEND_CONTRACT の色・任意値 grep: 触った file で残りは project-docs-screen.tsx の既存 `lg:grid-cols-[…]` 1 件のみ（layout、今回足していない）
- `git diff --stat 171c8e02 -- crates gui web/api/generated web/server docs/api` → 差分なし
- screenshot: run artifacts の `after-projects/`（台帳 31 画面 × 4 幅。既定 fixture に案件が無いため /projects は取得失敗の表示）と `after-projects/with-data/`（案件 fixture で list/detail × 360/390/412/1440 + 確認 dialog 390/1440）

## 未解決事項
- 一覧の途中目標の進みは案件ごとに詳細を 1 本ずつ取る（N 本）。案件数が増えたら一覧 API に milestone の集計を足すのが筋（API 変更なので提案に回す）。
- 偽 daemon の既定 fixture に案件が無く、台帳の screenshot・mobile-audit の /projects は空・失敗状態でしか見ていない（fixture は inbox/notifications 以外足せない範囲）。

## 提案
- `GET /projects` の各項目に `milestones_reached`・`milestones_total` を足す（一覧の N+1 をなくす）。
- 案件詳細の操作（案件・計画・リポジトリの form）が長い。読む構造（概要→木→DAG）の後ろに置いたが、編集は Drawer に畳む改修を次の段で検討。
- fake-daemon の既定 fixture に `/api/v1/projects`・`/api/v1/projects/P1` を足し、台帳の screenshot を中身ありにする。
