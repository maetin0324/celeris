---
tasks: [01M43XZ28V51VGE1Z791MF1MJY]
status: running
updated: 2026-10-04
---

# knowledge・skills・help・login の改修

## knowledge

完了日: 2026-10-04（WU knowledge、基点 ac78fad613ad）

### 変更点

- `/knowledge`（`web/features/knowledge/knowledge-screen.tsx`）
  - 検索結果を md 以上では `Table`（列見出し「タイトル」「出典」「scope」「更新日」、caption に件数）、スマホでは対象名の link → `DataList`（出典・scope・更新日）の行に組み替えた（DESIGN.md「Table と list」）。
  - 開いたページの見出しの下に `DataList` で出典・scope・更新日を出す。本文は `max-w-prose-ja` で読みやすい幅に。
  - 値が無い時は空欄にせず「なし」「未設定」「不明」と書く。結果が切り詰められた時は文で知らせる。
  - 「検索結果」は名前付き region にしない（`getByLabel("検索")` と取り違えるため。h2 は保つ）。
- `/knowledge/inbox`（同 file）
  - 候補ごとに h2（タイトル）・path、`DataList`（出典・scope・更新日・提案された取り込み先と新規／既存の別）、本文、取り込み先の入力。
  - 採用・却下は `ConfirmDialog` を挟む。採用の dialog は「取り込み先・scope・出典」と上書きの有無を示し、確定ボタンは「「<title>」を正本に採用」。失敗時は dialog を閉じず理由を出す。
  - 操作結果は対象の行に残す（一覧から消えた候補の結果だけ上部の「操作の結果」に出す）。
- `/knowledge/skills`（`web/features/knowledge/skills-screen.tsx`）
  - 状態を `Badge`・`StatusBadge` と文字で示す: 「登録済み」（success）、「配送 N 課」（info）／「配送なし」（neutral）、保存・削除が失敗した skill は `StatusBadge status="failed"`（「失敗」）。色だけに頼らない。
  - 一覧は md 以上で `Table`（名前・状態・更新日）、スマホでは名前 → 状態 → 説明の行。詳細は `DataList`（状態・配送先・更新日・付属ファイル）。
  - 削除は `ConfirmDialog`（配送先の課にも届かなくなることを示す）。
- 共通: 入力欄の枠を `border-input`（--color-input）に、label に `text-label font-medium`、link を `text-primary` にした。生の色・任意値 4 件（`border-neutral-400` ×2、`grid-cols-[…]` ×2）を `lg:grid-cols-5` + `col-span` に替えて 0 件。
- `web/e2e/parity/knowledge.spec.ts`: 採否・削除の確認 dialog を挟む流れに合わせ、dialog の内容（取り込み先・scope）と確定前に accept が送られないこと、出典・scope・更新日の見出し、skills の「登録済み」「配送なし」を確かめる。h1・accessible name・URL は従来のまま。
- `web/routes/knowledge.*.tsx` は変更不要だった。

### 検査結果（web/ で実行）

| コマンド | 結果 |
| --- | --- |
| `corepack pnpm@12.6.0 typecheck` | exit 0 |
| `corepack pnpm@12.6.0 lint` | exit 0（既存の warning 4 件は styles.css 等で今回の差分外） |
| `corepack pnpm@12.6.0 test` | exit 0（server node --test 42 pass） |
| `corepack pnpm@12.6.0 check:boundaries` | exit 0 |
| `corepack pnpm@12.6.0 build && corepack pnpm@12.6.0 e2e e2e/parity/knowledge.spec.ts` | exit 0（3 passed、1 skipped = WEB_SHOTS_OUT 無しの screenshot 試験） |
| FRONTEND_CONTRACT §66 の grep を `web/features/knowledge web/routes/knowledge.*.tsx` に | 0 件（grep exit 1） |

### screenshot

WU の artifacts（リポジトリ外）: `/local/celeris/data/workspaces/01M43XZ28V51VGE1Z791MF1MJY/wu/knowledge/artifacts/after-knowledge/`

- `corepack pnpm@12.6.0 -C web screenshots --only <fixture>` で `/knowledge`・`/knowledge?path=projects%2Fdemo.md`・`/knowledge/inbox`・`/knowledge/skills`・`/knowledge/skills?name=demo` を 360/390/412/1440（20 枚）。
- `spec/` に parity spec の screenshot 試験（`WEB_SHOTS_OUT`）で create・edit を含む 6 fixture × 4 幅（24 枚）。

### 要望（fixture）

- `web/e2e/support/fake-daemon.mjs` の knowledge tree・inbox・skills に `sources`・`scope`・`updated`／`created` と、`mounted_by` が空でない skill が無い。今は「なし」「未設定」「不明」「配送なし」の表示しか確かめられない。値のある行と、配送先のある skill を 1 件ずつ足してほしい。
- 確認 dialog を開いた状態の screenshot は screenshots.mjs で撮れない（操作が要る）。dialog の内容は parity spec で確かめている。

### 未解決・提案

- skills の「失敗」は、この画面で行った保存・削除の失敗だけを示す。daemon 側の配送失敗を API が返していない（`SkillSummaryView` に配送の失敗状態が無い）。配送失敗を出すなら API に欄を足す提案。

### ui-ux-quality-gate 自己レビュー（Light gate）

- 破壊的・不可逆な操作（候補の採用・却下、skill の削除）は全て ConfirmDialog を挟み、対象・何が正本に入るか・戻し方・確認先を示す。
- 長い path・出典・名前は `break-all` で折り返し、Table の横溢れは名前付きの枠の中だけ。
- スマホは表を縦に積むだけにせず、対象名 → 状態 → 補助情報の行に組み替えた。
- 状態は色と文字の両方（「登録済み」「配送 N 課」「配送なし」「失敗」）。読み込み・失敗は既存の FetchFrame。
- 残る点: 確定前に何が入るかは dialog の文で示すだけで、本文の差分（既存ページとの比較）は出していない（API に既存ページ本文との差分が無い）。
