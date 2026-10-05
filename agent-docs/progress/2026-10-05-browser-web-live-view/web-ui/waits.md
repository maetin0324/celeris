# browser waits の decision・credential form と本人登録の案内

---
tasks: [01M470CJT10MV8XVFEGSMCXHYS]
---

## 変更

- `web/features/browser/owner-session-notice.tsx`: 本人 session の状態（`browser-query.ts` の `OwnerSession`）から
  notice の要否を決める純関数 `ownerNoticeReason`（`owner_unavailable` / `not_owner` / `null`）を定義した。
  - `owner_unavailable`（パスワード認証が無効）はボタンを出さず、利用できない理由だけを Notice で出す。
  - `not_owner` は challenge 発行ボタン（`requestOwnerSession()` を呼ぶ `issueOwnerChallenge()`）と、発行後に
    `celerisctl browser owner-session approve <challenge> --socket <path>`（`ownerApproveCommand()`）を `CodeBlock` で出す。
    `<path>` は web gateway 起動設定（`CELERIS_WEB_OWNER_SOCKET`）の socket path に置き換える旨を本文に書いた。
  - 本人（`isOwner`）なら何も描画しない（waits panel 側が form を出す）。
- `web/features/browser/browser-waits-panel.tsx`:
  - `BrowserWaitsList`（純粋な表示本体。owner を props で受け取る）と `BrowserWaitsPanel`（`ownerSessionQuery()` を
    `useQuery` で取り、`BrowserWaitsList` に渡す薄い wrapper）に分けた。表示ロジックを useQuery から切り離すことで
    vitest で出し分けを直接確かめられるようにした。
  - `reason` ごとに form を出し分け: `waiting_for_approval` は `DecisionForm`（`approve_once` / `deny` ボタンと
    `OperationSummary`＝対象操作・引数 digest・policy revision。サイトは共通の `WaitSummary` に出す）、
    `waiting_for_auth` は `CredentialForm`（username・password、`autoComplete="off"`・`autoCapitalize="none"`・
    `spellCheck={false}`・`data-1p-ignore`・`data-lpignore`、1〜256 / 1〜1024 文字）。
  - `submitCredential()` は成功・失敗のいずれでも `{username: "", password: ""}` を返す純関数にし、
    `CredentialForm` は送信結果をそのまま state へ書き戻すだけで、入力値を残さない。
  - エラーは `BrowserGatewayError.code` を固定の日本語文言表に通す `waitActionMessage()`（`version_conflict`・
    `wait_gone`・`csrf_failed` 等、celeris の応答本文はそのまま出さない）。
  - 二重送信は各 form の `pending` state でボタン・submit を無効化する（`browser-query.ts` の
    `browserActionGate` が全体で直列化するのに加え、form 単位でも早期 return する）。
  - 本人でない・owner_unavailable の間は form の代わりに `OwnerSessionNotice` を出す（open な wait がある時だけ）。
    owner の取得がまだ済んでいない間（`owner === undefined`）は notice も form も出さない（誤った
    owner_unavailable 表示を避ける）。
  - タップ領域は既存の `Button`/`Input` primitive（`min-h-11`）をそのまま使い、新しい寸法は増やしていない。
    生の色・任意値は使わず、`text-label`・`text-muted-foreground`・`bg-warning` 等の既存 token だけを使った。
- `web/features/browser/browser-waits-panel.test.tsx`・`owner-session-notice.test.tsx` を追加した。

## 検証

| コマンド | 結果 |
| --- | --- |
| `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` | 成功。278 package を local store（`/local/.pnpm-store/v11`）から再利用 |
| `corepack pnpm@12.6.0 -C web exec vitest run features/browser` | 成功。4 files / 30 tests |
| `corepack pnpm@12.6.0 -C web exec vitest run`（全体） | 成功。63 files / 391 tests |
| `corepack pnpm@12.6.0 -C web exec tsc -b` | 成功。差分なし |
| `corepack pnpm@12.6.0 -C web exec biome check features/browser` | 成功。警告・エラーなし |
| `corepack pnpm@12.6.0 -C web exec biome check .`（全体、read-only） | 成功（exit 0）。既存の `styles.css` 警告 5 件のみ（この葉の変更ではない） |
| `git status --porcelain` / `git log --format= --name-only $CELERIS_WU_BASE..HEAD` | 新規 untracked は `web/features/browser/{browser-waits-panel,owner-session-notice}.{tsx,test.tsx}` と本記録だけ。`gui/`・`crates/`・`web/server/browser-live.js` は無差分 |

### vitest で確かめた出し分け・password 消去・notice（受け入れ条件 0）

- `BrowserWaitsList` に `waiting_for_approval` の wait を渡すと `browser-decision-form` が出て
  `browser-credential-form` は出ない。対象操作・引数 digest・origin・policy revision の値を含むことを確認。
- `waiting_for_auth` の wait では逆になる。`autoComplete="off"` が username・password 両方の `<input>` に付くこと、
  `type="password"` が出ることを確認（React 19 の `renderToStaticMarkup` はこの属性をキャメルケースのまま出力する
  ため、その表記で一致を取っている）。
- `submitCredential()` を成功・失敗それぞれのモックで直接呼び、どちらの場合も戻り値の `username`・`password` が
  必ず `""` になることを確認（component はこの戻り値をそのまま state に書き戻すだけなので、画面にも値は残らない）。
- owner が `{available: false}` のときは `browser-owner-unavailable` の notice が出て、どちらの form も出ない。
  owner が `{available: true, isOwner: false}` のときは `browser-owner-request`（challenge 発行ボタン）が出て
  form は出ない。owner が `undefined`（取得中）のときは notice も form も出ない。
  resolved 済み（`state !== "pending"`）の wait は owner に関わらず form を出さない。
- `issueOwnerChallenge()` は成功で `{challenge}`、失敗で固定文言の `{error}` を返す（`BrowserGatewayError` の
  code を見る。生の応答本文は出さない）。`ownerApproveCommand()` は
  `celerisctl browser owner-session approve <challenge> --socket <path>` を返す。

## 未解決事項

- `BrowserWaitsPanel`（`useQuery` 込みの wrapper）と `OwnerSessionNotice` の challenge 発行ボタンは、まだ
  どの画面にも配線されていない。配線は `runs-screen`・`run-screen`・`links`（task 詳細の `#browser-waits`）が行う。
- `celerisctl browser owner-session approve` に `--socket` flag が実装済みかは、このWorkUnitでは確認していない
  （gateway 側は `agent-docs/progress/.../gateway.md` に記載があるが、CLI 側の実装確認は範囲外）。
- mobile-audit・axe 等の実機確認は、この葉では画面に配線されていないため実行していない（`close` WorkUnit で
  実画面に対して行う）。
