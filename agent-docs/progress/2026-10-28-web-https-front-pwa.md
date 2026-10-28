---
title: web gateway を Tailscale HTTPS 前段の後ろで動かし、ホーム画面に置ける PWA にする
tasks: [01M61VEA35DVNBN9GJ2ACWQAAR]
status: done
updated: 2026-10-28
---
# web gateway を Tailscale HTTPS 前段の後ろで動かし、ホーム画面に置ける PWA にする

完了日: 2026-10-28

## 変更
- `web: HTTPS 前段の公開 origin と PWA の manifest・icon` — `web/server/origin.js` に自 origin の判定を 1 か所にまとめた。
  `CELERIS_WEB_PUBLIC_ORIGINS` に挙げた https origin だけ、接続元 loopback・Host 一致・`X-Forwarded-Proto` が無いか https の
  とき https とみなす。app.js の CSRF、auth.js の session cookie、terminal.js の upgrade、browser-live.js の Origin 比較と
  端末 cookie がこれを使う。公開 origin の host は Allowed Host に入る。公開 origin を設定したら password を必須にした。
  `web/public/manifest.webmanifest`・`web/public/icons/*.png`・index.html の link と meta、gateway の配信経路、
  HTML CSP の `manifest-src 'self'` を足した。service worker は無い。
- ADR `agent-docs/adr/2026-10-28-web-https-front-pwa.md`、`docs/ops/web-operation.md` の「HTTPS 前段（tailscale serve）とホーム画面」、
  `docs/architecture-map.md` の行を足した。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cd web && pnpm install --frozen-lockfile && pnpm typecheck && pnpm test` → exit 0（vitest 697 passed、node --test 113 passed）
- `node --test web/server/origin.test.mjs` → 7 passed、`node --test web/server/terminal.test.mjs` → 7 passed
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` → 報告本文に記載

## 本番への移行
- 人が行う: `tailscale serve --bg --https=443 http://127.0.0.1:7720`、`~/.config/celeris/web.env` に
  `CELERIS_WEB_PUBLIC_ORIGINS=https://home-dev.astrapia-algieba.ts.net`、`celeris-web@<sha12>` の再起動。手順と確認は
  `docs/ops/web-operation.md`。

## 未解決
- tailscale serve が `X-Forwarded-Proto` に http 以外を送ること・Host を保つことは実機で未確認（Tailscale の実装上は Host を保ち
  https を送る見込み）。送らない場合も「無い」なら通る。Host が書き換わると公開 origin とみなされず POST は 403 になる。
- スマホでのホーム画面追加・standalone 表示は実機で未確認（人の確認手順に入れた）。

## 提案
