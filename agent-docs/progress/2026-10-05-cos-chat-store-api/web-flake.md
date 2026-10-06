# web overlay fixture の起動待ち

tasks: [01M478CMKRNQGTV7TBR9JAE527]

## 原因

gallery、confirm-dialog、drawer、artifact-preview の各試験が独立した Vite dev server と Chromium を並行起動する。初回変換を含む起動が重なると、fixture helper 内の listen・browser launch・navigation・h1 待ちが Vitest の既定 30 秒 hook timeout を超える場合があった。

## 修正

共通 helper `overlay-browser-test.ts` に 120 秒の `OVERLAY_FIXTURE_TIMEOUT` を定義した。Chromium launch、`page.goto`（`domcontentloaded` まで）、h1 待ちに明示 timeout を設定し、4 試験の `beforeAll` にも同じ上限を適用した。Vite の `server.listen()` は引数をポートとして解釈するため、引数なしで呼び出す。

## 実行結果

- `corepack pnpm@12.6.0 -C web test`: exit 0、59 files / 356 tests passed。CPU 負荷を追加する再現は行っていない。
- この WorkUnit では `crates/` を変更していない。
