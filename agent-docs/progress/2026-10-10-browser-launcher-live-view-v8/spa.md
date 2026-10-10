# SPA Live View frames

tasks: [01M4JHECPYZBC6YVMBX162E57S]

## 完了

- gateway の `/browser/live/{task}/{run}/frames` WebSocket から binary frame を受け、最新画像だけを表示する viewer を実装。blob URL は frame 差し替えと unmount 時に revoke する。
- launcher frame が使える run は読み取り専用表示にし、ControlBar（pause/takeover 等）を出さない。
- credential 待ちを含む run で「本人だけに表示」を明示。`launcher_protocol_no_live_frames` の理由表示を追加。
- fake daemon の frame available fixture、Vitest、Playwright の owner frame / 未認証 viewer 拒否 spec を追加。

## 検証

- `cd web && pnpm lint` — 合格（既存 styles.css の `!important` 警告 4 件）
- `cd web && pnpm typecheck` — 合格
- `cd web && pnpm vitest run` — 91 files / 657 tests 合格
- `cd web && TMPDIR=/tmp WEB_E2E_SCOPE=functional pnpm exec playwright test e2e/browser/live-view-frames.spec.ts` — 1 test 合格
- `sh "$CELERIS_WU_SCOPE_PATHS"` — 対象範囲内
