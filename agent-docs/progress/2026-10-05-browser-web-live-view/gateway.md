# ブラウザ Live View gateway の実装記録

---
tasks: [01M46W97H391DSFW1XJ745W0G9]
---

- `web/server/browser-live.js` に owner challenge と private Unix socket、署名済み assertion、task/run/session を照合する HTTP・WebSocket 中継を追加した。宛先は起動時指定の loopback に固定し、応答では upstream の Content-Type 以外を採用しない。
- Live View の entry で grant を取得し、後続要求と WebSocket message を再照合する。grant は失効前に更新する。入力は有効な本人 lease・`human_control`・認証区間外のときだけ転送し、拒否理由を本人に返してログにも固定コードで残す。
- `/browser/runs`、`/browser/control/*`、`/browser/waits/*`、`/browser/identities*` を本人専用経路として追加した。generic `/api` relay は browser の変更経路、live、control、identity を拒否する。JSON と SSE から `live_view_url` の値を除く。
- owner socket は `CELERIS_WEB_OWNER_SOCKET` を使う。`celerisctl browser owner-session approve <challenge> --socket <path>` で web の challenge を確定できる。`CELERIS_WEB_LIVE_VIEW_UPSTREAM` は loopback `host:port` のみ、`CELERIS_WEB_ATTESTATION_KEY_FILE` は非公開の Ed25519 PKCS#8 PEM とする。
- 人の追加要望（2026-10-06）は ADR の D2.0 に決定として追記した。org の web 編集、task 作成時の `requirements.browser.allowed_domains`、broker/egress の交差と親子制限は、この WorkUnit の `web/server` 専用範囲外の実装であり、親 task の計画へ追加が要る。

## 検証

| コマンド | 結果 |
|---|---|
| `node --test web/server/browser-live.test.mjs` | 10/10 成功。拒否・許可・lease 取得/返却・WS 入力の毎回判定・socket challenge・raw URL 除去を確認 |
| `pnpm -C web test` | Vitest 59 file / 361 test、Node server 52 test 成功 |
| `pnpm -C web typecheck` | 成功 |
| `pnpm -C web lint` | 成功。既存の 5 warning が残る |
| `git diff --check` | 成功 |
