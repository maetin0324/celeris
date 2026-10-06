---
tasks: [01M47M7QCGRA9V72WKP07Z471B]
---

# Live View upgrade の RST 耐性

final review の R2 を修正した。HTTP upgrade で切り離された client socket には、host 検査より前に `error` listener を付ける。browser live の認可・origin 検査より前にも同じ保護を行い、upstream 接続 socket と upgrade 済み upstream socket も保護する。拒否応答は `end()` 後、1 秒の期限で `destroy()` する。期限は接続が閉じれば解除する。

`browser-live.test.mjs` は未認証と origin 不一致、`app.test.mjs` は host 不一致の upgrade を送る。応答前と応答受信後の client RST を使い、socket の close を待った後で `/healthz` が応答し、`uncaughtExceptionMonitor` に例外が届かないことを確認する。固定 sleep は使わない。

## 検証

| コマンド | 結果 |
| --- | --- |
| `node --test web/server/browser-live.test.mjs web/server/app.test.mjs` | 18 件成功、exit 0 |
| `pnpm -C web test` | Vitest 458 件、Node 59 件成功、exit 0 |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web lint` | exit 0。既存の CSS warning 4 件 |
| `git diff --check` | exit 0 |

`crates/` と `gui/` は変更していない。
