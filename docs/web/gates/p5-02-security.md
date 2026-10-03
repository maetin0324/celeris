---
tasks: [01M3TG9K4VV5Z4GZ0WY4DCVBZS]
---
# P5-02 security gate

2026-10-01。偽 daemon と gateway を loopback の空き port で起動し、`web/e2e/parity/gateway.spec.ts` の `P5-02 全経路 security gate` と `web/e2e/parity/shell.spec.ts` の `P5-02 全画面 storage gate` で再確認した。外部ネットワーク、`:7700`、`:7710`、staging は使わない。

| 経路 | Host | CSRF | session | header | token |
| --- | --- | --- | --- | --- | --- |
| fingerprint 付き `/assets/*` | 400 | 外部 Origin と same-site POST は 403 | 公開、GET 200 | nosniff、no-referrer、frame 拒否、CSP、cache | 本文に無し |
| `/`、`/login` | 400 | 同上 | 公開、GET 200 | 同上、no-store | 本文に無し |
| 未定義 HTML `/no-such-page` | 400 | 同上 | 公開、GET 404 | 同上、no-store | 本文に無し |
| `/api/health`、`/api/console/stream` | 400 | 同上 | 未認証・不正 cookie は 401 | 同上、no-store | 未認証本文・認証後本文に無し |
| `/files/tasks/T1/artifacts/0` | 400 | 同上 | 未認証・不正 cookie は 401 | 同上、no-store | 未認証本文・認証後本文に無し |
| `/events`、`/console/stream` | 400 | 同上 | 未認証・不正 cookie は 401 | 同上、no-store | 未認証本文・認証後の応答 header と最初の frame に無し |

ここで `/console/stream` は専用 SSE relay、`/api/console/stream` は一般 `/api/*` relay として扱う。両方を検査対象に含める。X1〜X6 の既存 parity テストも cookie 属性、`next`、非 loopback bind、file path、header 許可リスト、HTML attachment、daemon への token 付与と browser への秘匿を検査する。偽 daemon を停止した後も `/login` と `/` の HTML が 200 を返すことを新しい経路テストで確認する。

X8 は `web/e2e/support/screens.ts` の全 31 行を fixture URL で開き、**各画面の直後**に localStorage、sessionStorage、IndexedDB、Cache Storage、Service Worker を調べる。localStorage は表示設定の `celeris.web.display` の theme/timeZone だけを許し、daemon token と fixture の API 応答本文が保存されないことを確認する。`/login` と同じ fixture を持つ台帳行も省略しない。

再現コマンド（`web/` にインストール済みの依存が必要）:

```sh
corepack pnpm@12.6.0 -C web typecheck
corepack pnpm@12.6.0 -C web e2e parity/gateway.spec.ts parity/gateway-auth.spec.ts parity/gateway-relay.spec.ts
corepack pnpm@12.6.0 -C web e2e parity/shell.spec.ts -g 'parity-x: storage'
corepack pnpm@12.6.0 -C web e2e parity/gateway.spec.ts -g 'parity-x: security 全経路'
```

この worktree には当初 `web/node_modules` が無く、固定版 pnpm が registry から依存を取得しようとして DNS 制限で止まった。この run では既存 worktree の同版依存をこの worktree に複製し、対応する `web/node_modules/.bin` のコマンドで再現した。複製した依存は commit 対象に含めない。

実行結果: `tsc -b` と変更した 2 spec の Biome check は成功。`playwright test parity/gateway.spec.ts parity/gateway-auth.spec.ts parity/gateway-relay.spec.ts parity/shell.spec.ts` は 20/20 成功（1.4 分）。最終変更後の横断テスト 2/2 も成功（6.4 秒）。

再検証: X8 のテスト題を parity matrix の `parity-x: storage に機密が無い` に合わせた。固定版 pnpm の `install --frozen-lockfile --prefer-offline`、`typecheck`、`lint`、`test`、`build`、`gen:types --check`、`check:boundaries`、`check:secrets`、`check:parity` は連続実行で成功。横断 e2e 2/2 も再度成功した。
