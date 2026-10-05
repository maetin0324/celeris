# browser identity 画面と project からの link

---
tasks: [01M470CJT10MV8XVFEGSMCXHYS]
status: done
completed: 2026-10-05
---

## 変更

- `web/features/browser/identities-screen.tsx`（新設）: `/projects/$id/browser-identities` の画面（ADR
  2026-10-05-browser-department-web-live-view D3.4）。
  - 一覧: origin・id・generation・期限（UTC 固定の `YYYY-MM-DD HH:MM:SSZ`）・状態（有効 / 失効済み の Badge）。
  - active にだけ「失効」「復元」、全件に「削除」（`ConfirmDialog`、対象は `origin（id）`）。
  - 登録 form: id・origin・確認した人（`demand_confirmed_by`）・state JSON。state JSON は伏せ字
    （`-webkit-text-security: disc`）の textarea で受け、`autocomplete="off"`・password manager 無視の属性付き。
    送信の直前に必ず空にする（検証失敗・送信失敗でも残さない）。TTL は gateway が 7 日（604800 秒）を付ける。
  - 本人でない・本人確認を使えない session には一覧も form も出さず `OwnerSessionNotice` を出す
    （identity の query も走らせない）。
  - エラーは gateway の固定コードを和文にした文言だけを出す（応答本文は出さない）。
  - 操作可否・状態の表示・期限の書式・state JSON の検証・エラー文言は純関数にして vitest で確かめる。
- `web/routes/projects.$id.browser-identities.tsx`: 骨組みを `BrowserIdentitiesScreen` に差し替えた。
- `web/features/projects/project-detail-view.tsx`: project 詳細の link 群に「ブラウザの identity」を 1 つ足した（`min-h-11`）。
- `web/features/browser/identities-screen.test.ts`（新設）: vitest 16 件。
- `web/e2e/browser/identities.spec.ts`（新設）: Playwright 3 件。
  - 本人: 一覧（P1 の 2 件のみ、revoked に失効・復元なし）→ 登録（偽 backend に state・`ttl_secs: 604800` が届き、
    textarea は空、`page.content()` に秘密値が無い）→ 復元（project・origin 付き）→ 失効 → 削除（ConfirmDialog）。
  - 本人でない session: notice が出て、identity の要求が backend に 1 件も届かない。
  - project 詳細の link から画面へ遷移できる。
- `web/styles.css` は変えていない。gui/・crates/・`web/server/browser-live.js` も変えていない。

## 実行したコマンドと結果

| コマンド（`web/` で実行） | 結果 |
|---|---|
| `pnpm install --offline --frozen-lockfile` | exit 0 |
| `pnpm typecheck` | exit 0 |
| `pnpm lint` | exit 0（既存の warning 5 件。今回の 5 file は `biome check` で警告 0） |
| `pnpm exec vitest run` | 64 files / 411 tests passed |
| `pnpm test`（vitest + server の node --test） | server 52 pass / 0 fail |
| `pnpm check:boundaries` / `pnpm check:secrets` | exit 0 |
| `pnpm build` | exit 0 |
| `WEB_E2E_SCOPE=functional pnpm exec playwright test e2e/browser/` | 4 passed（fixture-smoke 1 + identities 3） |
| 360/390/412/1440 の `scrollWidth` | 各幅と一致（横 scroll なし） |

スクリーンショット（本人・本人でない・project 詳細の link、4 幅）は run の成果物
`/local/celeris/data/workspaces/01M470CJT10MV8XVFEGSMCXHYS/wu/identities/artifacts/screenshots/` に置いた。
Rust は変えていないので `cargo` の検査はこの葉では回していない（close 葉で全検査）。

## 未解決事項

- 360px の「本人でない」表示で、共通部品 `web/components/ui/notice.tsx` の action（「本人確認のコードを発行」）が
  折り返さず、本文が 1 文字幅近くまで潰れる（`identities-not-owner-360.png`）。本文の div が `flex-1` で basis を
  持たないため。共通部品なのでこの葉では触らず、提案に回す。
- 復元の成功は gateway が `{ ok: true }` しか返さないので「復元しました。」とだけ出す。復元先の session の表示は無い。

## 提案

- `Notice` の本文 div に最小幅（例: `basis-48`）を与え、狭幅では action が下へ折り返すようにする。
  owner-session-notice を使う run 一覧・run 画面・待ち panel にも効く。close 葉か web 共通部品の task で扱う。
