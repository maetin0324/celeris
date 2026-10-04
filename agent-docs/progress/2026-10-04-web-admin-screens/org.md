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

## skills 葉（2026-10-04 完了）

### 変更点
- `web/features/org/org-skills.tsx`（書き直し）:
  - 一覧: skill 名・出どころ Badge（「独自」/「<上位の担当> から継承」）・「<名前> を表示」・「外す」。360px では名前の行と操作の行を分ける（`flex-col sm:flex-row`）。
  - 外す: `ConfirmDialog` で対象・影響（`<担当名>、… の worker に <skill> が届かなくなります。実行中の run には影響せず、次の run から外れます。`、上位も mount していれば「worker には届き続けます」）・戻し方・確認先を書く。trigger の accessible name は「<名前> を外す」。
  - mount: `<form>` + `label htmlFor`、select に `aria-describedby`（hint・error・403 の理由）と `aria-invalid`。未選択・失敗（422 の detail 等）は select の下に error を出して select へ focus、確定後は一覧（`ul tabIndex=-1`）へ focus。ConfirmDialog の focus 返しの後で移すため、alertdialog が消えてから移す。
  - 403（`ApiError.kind === "forbidden"`、変更 API でも skill 一覧の GET でも）: 外す・mount・select を disabled にし、`role=alert` の理由を一覧の上に出して focus。
  - 入力欄の枠は `border-input`（--color-input）のまま。error は枠の色を変えず文と aria-invalid で示す（e2e で計算色を確認）。
  - 使われていなかった `OrgProfile` を削除（詳細の DataList と重複）。
- `web/features/org/org-tree.ts`: `skillSource`（mount している最も近い上位）と `skillRemovalImpact`（外すと届かなくなる担当）を追加。単体試験 2 件。
- `web/features/org/org-screen.tsx`: `OrgSkills` に `items` を渡すだけ。
- `web/routes/org.secretary.tsx`: `/org/cos` への redirect だけで描画を持たないので変更なし（表示は features/console 側。h1「組織の人 cos」と URL を保つ）。
- `web/e2e/parity/org.spec.ts`: 「外す」の後に確認ダイアログの「review を外す」を押す 1 手を足した（h1・名前・URL は同じ）。
- `web/e2e/admin/org.spec.ts`: 360px の `/org/secretary`・skill 欄と確認ダイアログの横 scroll なし、確認ダイアログの影響文と「戻る」で送らないこと・確定後の一覧 focus、未選択/422 の error が select の accessible description に入り focus が欄へ移ること・枠色が --color-input、403 で disabled と理由（page.route で 403 を返す）。

### 検査結果
| 検査 | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| `corepack pnpm@12.6.0 -C web lint` | exit 0（warning 4 件は既存の skills-screen・styles.css） |
| `corepack pnpm@12.6.0 -C web test` | exit 0（vitest 48 files / 304 tests、server node --test 含む） |
| `build` + `e2e e2e/parity/org.spec.ts e2e/admin/org.spec.ts` | 9 passed、1 skipped（WEB_SHOTS_OUT 無しの screenshot） |
| `e2e e2e/a11y/axe.spec.ts e2e/parity/mobile-gate.spec.ts` | 64 passed |
| `mobile-audit --only /org` / `check:boundaries` | ok / exit 0 |
| 色・任意値 grep（web/features/org web/routes/org.*.tsx） | 0 件 |
| 差分範囲（ac78fad613ad 起点の許可リスト） | 範囲外 0 件 |

### screenshot
- run の artifacts `after-org/`（`screenshots` の 32 画面 × 360/390/412/1440。/org は `_org-*.png`・`_org_cos-*.png`）。
- 選択済みの詳細と skill 欄は `after-org/org-spec/org-selected-fixture-{360,390,412,1440}.png`（parity spec の screenshot 試験を WEB_SHOTS_OUT 付きで実行）。初回の 360px で skill 名が 1 文字ずつ折れたので行を 2 段に分けて直した。

### ui-ux-quality-gate 自己レビュー
- 主作業「担当の skill を足す・外す」で、危険側（外す）は確認と影響の明示、戻し方を書いた。届く範囲（配下）は一覧の説明文でも示す。
- 状態（独自・継承・403）は文字で出し、色だけに頼らない。focus は失敗→欄、確定→一覧、403→理由。
- disabled の button は理由の `aria-describedby` を持つ。

### fixture 要望（web/e2e/support/ は変えていない）
- 403 を返す org/skills の fixture（変更系の 403、`/api/v1/skills` の 403）が無いので、admin spec では page.route で再現した。screens.ts に「権限なし」の /org を足すと screenshot と mobile-audit に写せる。
- `/org?selected=<課>` で skill 欄（独自・継承の両方）が写る fixture と、確認ダイアログを開いた状態の screenshot 経路がほしい。

### 提案
- 外す操作の影響は org の木から計算している。daemon が「この変更で届かなくなる worker / 実行中 run」を返す preview API を持てば、確認文を正本から出せる。
