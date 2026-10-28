# ADR 2026-10-28: web gateway を HTTPS 前段（tailscale serve）の後ろで動かし、ホーム画面に置ける PWA にする

---
tasks: [01M61VEA35DVNBN9GJ2ACWQAAR]
---

- 状態: 採用（実装済み）
- 関係: ADR-0081（web SPA。機密の永続化と service worker による API/file cache の禁止）、
  [2026-10-27-web-terminal](2026-10-27-web-terminal.md)（端末の Origin 検査）、
  [2026-10-07-browser-trusted-devices](2026-10-07-browser-trusted-devices.md)（browser-live の Origin・cookie）

## 背景

利用者（2026-10-28、統括 CoS チャット）は web をスマホから https で開き、ホーム画面に置きたい。前段は
`tailscale serve --https=443 → http://127.0.0.1:7720`（TLS を前段が終端し、gateway には平文で渡る）。
gateway は自 origin を `req.socket.encrypted` だけで決めていたため、前段経由では自 origin が `http://…` になり、
ブラウザの `Origin: https://…` と一致せず、POST の CSRF 検査・端末と browser-live の Origin 検査が 403 になる。
session cookie にも `Secure` が付かない。

## 決定

### D1 公開 origin を設定で列挙する（`CELERIS_WEB_PUBLIC_ORIGINS`）

- `CELERIS_WEB_PUBLIC_ORIGINS` に https の origin（`https://host[:port]`、カンマ区切り）を書く。https 以外・
  path 付きは起動エラー。設定したら `CELERIS_WEB_PASSWORD_FILE` を必須にする（公開入口に login を要する）。
- 要求を「前段経由の https」と認める条件は 3 つ全部: 接続元が loopback（前段は同じ host）、`Host` が列挙した
  origin の host と一致、`X-Forwarded-Proto` が無いか `https`。`X-Forwarded-Proto` だけでは https にしない。
  認めた要求の自 origin はその https origin になり、CSRF・端末・browser-live の Origin 検査がそれで比べる。
  cookie（session・信頼端末）には `Secure` が付く。判定は `web/server/origin.js` の 1 か所に置く。
- 列挙した origin の host は Allowed Host にも入る。`CELERIS_WEB_ALLOWED_HOSTS` は Host 検査（DNS rebinding
  対策）だけを広げ、自 origin の scheme は変えない。端末の入口（`CELERIS_WEB_TERMINAL_HOSTS`）は従来どおり別に
  opt-in する。
- 直接 `http://127.0.0.1:7720` の要求は条件に当たらないので従来どおり。未設定の origin は従来どおり拒否する。

### D2 PWA は manifest と icon だけ（service worker は入れない）

- `web/public/manifest.webmanifest`（name・short_name・icons・start_url・display standalone・theme_color）と
  `web/public/icons/*.png` を置き、`index.html` から link する。iOS 向けに `apple-touch-icon` と
  `apple-mobile-web-app-*` の meta を入れる。gateway は `/manifest.webmanifest` と `/icons/*` だけを dist から
  配る。HTML の CSP に `manifest-src 'self'` を足す。
- service worker は入れない。ADR-0081 の禁止（API/file cache）と e2e `parity/shell.spec.ts` の「Cache Storage 空・
  SW 0」の検査をそのまま守る。ホーム画面への追加は iOS Safari の「ホーム画面に追加」、Android Chrome の
  「ホーム画面に追加」で行う（icon・名前・standalone 表示は manifest と apple-touch-icon から取る）。オフライン表示・Web Push は範囲外。
