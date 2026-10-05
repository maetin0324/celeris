---
tasks: [01M43690B86J9GHP87CARYM8RS]
unit: inbox
status: done
completed: 2026-10-04
---
# inbox: 受信箱画面（判断を画面から直接返す）と /approvals の受信箱への寄せ

設計は [ADR 2026-10-04-web-inbox-notifications-screens](../../adr/2026-10-04-web-inbox-notifications-screens.md) の D4。差分の基点は 171c8e02（WU 起点 11016d1f）。

## 変えたもの

- `web/features/inbox/inbox-screen.tsx`（作り直し）: `GET /inbox/items` だけを 1 行 1 項目の list で出す。各行に種類・何を決めるか・推奨・期限（期限切れは badge）・止めている範囲（summary・root/task への link・葉）・先に答える項目（`blocked_by` を同じ画面の項目へ anchor）・案件 link・待ち時間・関連 link・detail。右（スマホは下）に理由・note と選択肢（label・effect、推奨は primary）。
- 答えは `POST /inbox/items/{id}/answer`（`option`・`note`）。`removed` なら `['inbox','items']` の cache から即座に外して件数も減らし（nav の件数も同じ cache）、`inboxKeys.all` を invalidate。答えた項目は上の status 欄に残す。二重送信しない。
- 破壊的な選択（`withdraw`・`cancel`・`reject`・`deny` 等、`inbox-model.ts`）は `ConfirmDialog`。`needs_note` の選択は note が空なら送らずに欄の横で止める。
- 409 `native_action_required`、および専用操作の種類（`cluster_login`・`delivery_skipped`・`integration_request`・`browser_wait`、選択肢が無い項目）は専用画面（links → `/clusters` → task）への誘導。他の 409・404・`invalid_option` は再取得して「状態が変わりました」等。
- 絞り込みは URL の search param（`/inbox?project=&kind=`、`web/routes/inbox.tsx` の validateSearch）。
- `web/features/approvals/approvals-screen.tsx`: 判定の操作を外し、「認可待ち」は受信箱の `kind=authorization` の件数と `/inbox?kind=authorization` への誘導だけ。決めた記録と常設ルールは残し、常設ルールの削除は ConfirmDialog を経る。h1「承認」・h2「認可待ち」は保つ。
- `web/e2e/parity/inbox.spec.ts`: 新しい受信箱に合わせた（答える・消える・409 native・確認・URL 絞り込み）。既存の題名 3 本は保ち、`parity: /inbox 破壊的な選択は確認を経て消える`・`parity: /inbox 案件・種類の絞り込みは URL の search param` を足した。
- `web/features/inbox/inbox-model.test.ts`（新規）: 破壊的判定・409 の振り分け・専用画面の行き先。

## 証拠

| 検査 | 結果 |
|---|---|
| `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| `corepack pnpm@12.6.0 -C web lint` | exit 0（既存の warning 4 件のみ） |
| `corepack pnpm@12.6.0 -C web test` | exit 0（vitest 45 files / 288 tests） |
| `check:parity`・`check:boundaries` | exit 0 |
| `e2e --grep-invert 'S1 /'`（全体） | 177 passed / 8 skipped |
| `e2e e2e/latency/transition.spec.ts` | 30 passed |
| `e2e e2e/parity/inbox.spec.ts e2e/a11y`（最終） | 38 passed |
| `mobile-audit` | 30 path × 4 幅 ok |
| 生の色・任意値 grep（FRONTEND_CONTRACT §66）を変更 file に | 0 件 |
| screenshots `after-inbox`（31 画面 × 4 幅）＋ fixture の inbox/approvals × 4 幅 | run の artifacts |

## 未解決事項

- nav の「承認」入口と `approvals_pending` badge は notifications 葉が扱う（ADR D4）。parity/inbox.spec.ts の approvals 試験から nav badge の確認を外した（nav の変更で落ちないように）。
- feature-parity.md の R02・R19 の記述（区画ごとの approve/reject）は verify 葉で新しい受信箱に合わせる。

## 提案

- 各行に note 欄が常に出るのでスマホでは縦に長い。選択肢を押して初めて note 欄を開く（needs_note の選択だけ）形にすると密度が上がる。
- `/inbox/items` の項目に案件の題名が無く、案件一覧（`GET /projects`）を別に引いて名前を出している。API に `project_title` があると 1 往復減る。
