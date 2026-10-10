---
tasks: [01M4HRQBN8XHKQRD1DAXHXBSEX]
---

# Live View disabled reason 修正

## 変更

- gateway の `liveAvailability` は upstream 未設定を先に判定し、状態や session の有無によらず `relay_unavailable` を返す。upstream がある場合は `RUNNING` 以外を `not_running` とし、`RUNNING` は同じ origin の live link を返す。session_id が無い `RUNNING` は従来どおり `not_configured`。
- Live View UI は本人確認後に gateway の disabled reason を優先し、browser state による `not_running` 判定を行う。WAITING_FOR_AUTH や認証 wait による独自除外を削除した。
- `/browser/control` の polling を RUNNING 以外すべてで止める。404 停止も維持。
- gateway の状態判定、web model、polling の Vitest を更新。upstream 未設定の全 run 応答、RUNNING 以外の状態、WAITING_FOR_HUMAN 表示を確かめる Playwright spec を追加。

## 検査

- `pnpm run lint` — exit 0。既存 styles.css の `!important` 4 件の warning を表示。
- `pnpm run typecheck` — exit 0。
- `pnpm run test` — exit 0。Vitest 90 files / 655 tests passed、server node:test 全件 passed。
- `TMPDIR=/tmp pnpm run e2e` — exit 0。322 passed、8 skipped。新規 Live View spec 2 件を含む。
- 新規 Playwright spec は既定 TMPDIR では長い Unix socket path のため EINVAL となったが、`TMPDIR=/tmp` で再実行し 2 件とも passed。
- `sh "$CELERIS_WU_SCOPE_PATHS"` — exit 0。差分は Live View 実装・試験と本進捗ファイルの範囲。
