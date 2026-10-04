---
title: 主画面 1（task・run・console・changes・files・artifacts・graph）の改修と統合検証
tasks: [01M43DVA9VA6YFH9X2G34NZ4SB]
status: done
updated: 2026-10-04
completed: 2026-10-04
---

# 主画面 1 の統合検証・screenshot・記録

build 段 4 葉の統合結果（HEAD 01cdbf90）に gates 葉の修正（93e7cf06）を足した状態を検証し、ui-ux-quality-gate で 4 画面群を自己レビューした。完了日 2026-10-04。

子の記録:

- [task 一覧・作成・依存グラフ](2026-10-04-screens-ops/task-list-graph.md)
- [task 詳細・木](2026-10-04-screens-ops/task-detail.md)
- [run ログ・Console](2026-10-04-screens-ops/run-console.md)
- [changes・files・artifacts](2026-10-04-screens-ops/changes-files.md)（[changes](2026-10-04-screens-ops/changes-files/changes.md)・[files-artifacts](2026-10-04-screens-ops/changes-files/files-artifacts.md)）
- [統合後の全検査・full e2e](2026-10-04-screens-ops/gates.md)

## 検査コマンドと結果

詳細は [gates.md](2026-10-04-screens-ops/gates.md)。すべて `corepack pnpm@12.6.0 -C web <script>`。

| 検査 | exit | 要点 |
|---|---|---|
| install --frozen-lockfile・typecheck・lint・test・build・check:secrets・check:boundaries・check:parity | 0 | vitest 45 files / 284 tests、server node --test 42 tests。lint は既存 warning のみ |
| mobile-audit | 0 | 30 path × 360/390/412/1440 |
| e2e（full、`--retries=0`、S1 含む） | 0 | 213 passed・8 skipped。load 1 分値 < 16 を待ってから実行 |

gates 葉で 1 件（S1 `/tasks/$id/runs/$runId` のデータ待ち）が落ち、`web/features/runs/run-log-view.tsx` の page header に description を足して直した（commit 32a4205a）。

この record 葉では build と screenshot を再実行した:

- `corepack pnpm@12.6.0 -C web build` → exit 0
- `corepack pnpm@12.6.0 -C web screenshots --out …/after-screens-ops` → exit 0（`31 screen(s) x 4 widths`）

## screenshot

- 置き場所: `/local/celeris/data/workspaces/01M43DVA9VA6YFH9X2G34NZ4SB/wu/record/artifacts/after-screens-ops`
- 枚数: 120 枚（screenshots.mjs の 30 画面 × 360/390/412/1440。出力の「31 screen(s)」は台帳行の数で、同じ fixture の行は 1 枚にまとまる）
- 主画面 1 の分: `_tasks`・`_tasks_new`・`_graph`・`_tasks_T1`・`_tasks_T1_changes`・`_tasks_T1_files`・`_tasks_T1_runs_R1`・`_artifacts`・`_`（Console）の 9 画面 × 4 幅 = 36 枚

## 品質 gate の所見

surface は ops workbench（task を作り・見張り・直す人の道具）。主な task は「状態を見て次の操作を決める」。360〜1440 の screenshot と子の記録を見た。

良い点:

- 4 画面群とも primitive（Button・StatusBadge・DataList・CodeBlock・EmptyState・ErrorNotice）と token に揃い、枠・余白・ボタンの階層がまとまった。360px で横溢れは無い（mobile-audit と 360px の長 path / 長い行の試験で確認）。
- 取得失敗は「何が起きたか + 再試行」の形で、task 一覧・詳細・changes・graph で同じ見た目になった。
- task 作成は「内容」「受け入れ条件」の 2 節に分かれ、条件の種類ごとに説明が付く。主操作「タスクを作成」が 1 つだけ塗りで明確。
- task 詳細は 360px で区画切り替え（概要・判断・実行・木）があり、「次の操作」の hash 移動でその区画を開く。

直すべき点（コードは変えていない。未解決事項へ）:

1. **共通 screenshot fixture で主画面 1 の多くが取得失敗の状態しか写らない**（`/tasks`・`/graph`・`/tasks/T1`・`/tasks/T1/changes`・`/tasks/T1/files`・`/tasks/T1/runs/R1`・`/artifacts`）。通常データ・長文・多数行の見た目は parity 試験と各葉の個別台本でしか確かめられていない。
2. **run ログの取得失敗だけが素の文**で、他画面の ErrorNotice（赤枠 + 再試行）と揃っていない。再試行の操作も無く「再読み込み」を促すだけ。
3. **task 一覧の 360px で状態の絞り込みボタン 8 つが 3 段を占め**、一覧本体が画面下半分に押し出される。スマホでは折りたたむ（「状態: 全部」の 1 行 + Drawer）方が主作業に近い。
4. **task 詳細の tab は 360px で「成果物」が切れ**、横 scroll できることが見えない（端の影や fade が無い）。
5. **英語の内部語が主 UI に残る**: tab「timeline」、graph の入力ラベル「root」「depth」、作成の条件種類「human」。日本語の主ラベル + 補助に原語、の順にしたい。
6. task 一覧で遷移直後に h1 へ focus が移り、太い focus 枠が見出しに出る（keyboard 利用者には有用だが、pointer 遷移でも出る）。`:focus-visible` 相当の扱いを shell 側で揃えたい。

## 未解決事項

- 上の所見 1〜6。いずれも範囲（build 段の葉の path）外の共有部品（`web/e2e/support/`・`web/components/`）に触るか、parity 試験の期待語を変える必要があり、この葉ではコードを変えていない。2〜5 は features 側で直せるので次の改修 task の候補。
- gates.md の未解決: `ScreenFrame` の header slot で description を省くと S1 の「h1 の兄弟」待ちが成り立たない。
- task 詳細: 判断 panel・概要の「状態」は parity 試験（`decision-status` の生の語）のため生の語のまま。

## primitive・fixture の要望

子の記録の要望をまとめた。

primitive（`web/components/ui`）:

- **ShortId / CopyableId**（等幅・省略・title・copy）— task 詳細・run・changes・inbox で共通（task-detail）。
- **TreeList**（枝線・展開/折りたたみ・キーボード操作）（task-detail）。
- **Disclosure**（chevron + 44px の開閉）— run ログと Console が別々に組んでいる（run-console）。
- **追従 scroll と「最新へ」を持つ log 面**、要素を並べる会話 log の面、Markdown 表示（run-console）。
- **CodeBlock の行ごとの装飾**（diff の add/remove）と **path の中間省略**（changes-files）。
- **Alert / Notice**（danger・warning + 本文 + 任意の操作。ErrorNotice は再試行固定）— run ログの取得失敗（所見 2）にも使える（files-artifacts）。
- **Select / Input**（`border-input`・focus ring・44px）（files-artifacts）。
- **横 scroll の tab に端の fade**（所見 4）。
- token: `text-code` が色（`--color-code`）と寸法（`--text-code`）の両方に当たる名前衝突の解消（task-detail）。

fixture（`web/e2e/support/`）:

- 共通 screenshot fixture で `/tasks`・`/graph`・task 詳細・changes・files・run ログ・`/artifacts` が取得成功し、長文・多数行・複数節点・長い path の例を返すようにする（所見 1。task-list-graph・changes-files）。
- Console の block を返す fixture（mobile-audit が会話の tap 領域も検査できる）（run-console）。

試験:

- S1 のデータ待ちを ScreenFrame の構造（page header slot）に合わせる（gates）。
- `decision-status` を StatusBadge にできるよう parity 試験の期待（生の語）を見直す（task-detail）。
