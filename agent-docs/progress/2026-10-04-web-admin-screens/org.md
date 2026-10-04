---
tasks: [01M43ZC8G0MR4EGQAQDA7SHKM9]
status: running
updated: 2026-10-04
---
# 組織画面（/org）: org tree・課の詳細・skills・secretary

## tree-detail 葉（2026-10-04 完了）

### 変更点
- `web/features/org/org-tree.ts`: `flattenOrgTree`（深さ付きの平らな行）と `orgSettingState`（自分の profile に値があれば「独自設定」、無ければ「継承」、根なら「既定」）を足した。単体試験 `org-tree.test.ts`。
- `web/features/org/org-screen.tsx`:
  - 木: 1 行に「種類 · 名前」・ID・skill 数（実効の skills_mounts）・状態 Badge を並べる高密度の一覧。字下げは段ごとの固定 class（`pl-0`〜`pl-12`、4 段目以降は同じ）で 360px でも溢れない。選択中は `bg-accent` と `aria-current="page"`。
  - 課の詳細: h3 に名前、操作「話す」を Section の actions に。設定の現在値（種類・ID・親・分野・状態・継承・能力タグ・mount する skill・実行環境・モデル階層・上限の段・既定/使える harness・レビューの段・最大試行回数・外部/禁止の道具）を `DataList`、方針を Markdown、配下の担当を `Table`（名前・種類・skill・状態）で出す。値の無い項目は「既定」「なし」と書く。
  - `lg:grid-cols-[minmax…]` → `lg:grid-cols-5`（2:3）。入力欄は `border-input`・focus ring の token class。保存は primary、削除は destructive。
  - `OrgProfile`（org-skills.tsx）は詳細の DataList と重複するので呼ばなくした（export は skills 葉の file なので残置）。
- `web/e2e/admin/org.spec.ts`（新規）: width 360 で `/org`（長い名前・長い ID・5 段の木）、選択後、長い名前の課の選択、`/org/cos` の `document.documentElement.scrollWidth <= clientWidth`。
- routes（`org.index.tsx`・`org.$id.tsx`）は変更不要だった（h1・URL を保つため触らない）。

### 検査結果
| 検査 | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| `corepack pnpm@12.6.0 -C web lint` | exit 0（warning 4 件は既存の skills-screen・styles.css） |
| `corepack pnpm@12.6.0 -C web test` | vitest 48 files / 302 tests pass、node --test 42 pass |
| `build` + `e2e e2e/parity/org.spec.ts e2e/admin/org.spec.ts` | 4 passed、1 skipped（WEB_SHOTS_OUT 無しの screenshot） |
| `e2e e2e/a11y/axe.spec.ts e2e/parity/mobile-gate.spec.ts` | 64 passed |
| `mobile-audit --only /org` | 1 path × 4 widths ok |
| FRONTEND_CONTRACT の色・任意値 grep（web/features/org web/routes/org.*.tsx） | 0 件（grep exit 1） |
| screenshots | run の artifacts `after-tree-detail/`（360/390/412/1440、/org と /org?selected=cos） |

### ui-ux-quality-gate 自己レビュー
- 面の種類: ops workbench（組織設定の閲覧と編集）。主な作業は「課を選んで現在の設定を読む → 必要なら変える」。
- 長い名前・ID は折り返し（`break-words`/`break-all`）、360px 試験で固定。
- 状態は色だけに頼らず文字（独自設定・継承・既定）で出す。
- 残る課題は下の「提案」。

### fixture 要望（web/e2e/support/ は変えていない）
- `screens.ts` の `/org` fixture は選択なし・2 件だけで、詳細と深い木が screenshot / mobile-audit に写らない。`/org?selected=<課>` と、長い名前・3 段以上の木・effective_profiles（chain・harnesses_allowed・tier）のある fixture を足してほしい。

### 提案
- 担当の削除は `window.confirm` のまま（parity spec が dialog event を受ける）。影響（配下の担当・mount の扱い）を書く ConfirmDialog にするなら parity spec の削除手順も合わせて変える。
- スマホ幅では「担当を追加」の form が木と詳細の間に入る。追加を Drawer か折り畳みに移すと、選んだ詳細までの scroll が減る（DOM 順と見た目の順を揃えたまま直すため、order での並べ替えはしなかった）。
- API に担当の稼働状態（実行中の task 数など）が無いので、一覧の「状態」は設定の出どころを示している。StatusBadge は Celeris の状態語専用なので Badge を使った。
