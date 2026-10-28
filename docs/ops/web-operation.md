# web の運用

---
tasks: [01M4NAKDCESYK36PNY7TTCTJ5E]
---

画面は `web/` の SPA と Node gateway が提供する。daemon の API は loopback 7710、gateway は
既定 `127.0.0.1:7720`。LAN 中継 socket は host に合わせた address の7721番を使う。
release・verify・promote は [selfdeploy](selfdeploy.md) に従う。本番操作は運用セッションが行う。

## 設定と起動

`~/.config/celeris/web.env` に `CELERIS_WEB_BIND`、`CELERIS_API_URL`、必要に応じて
`CELERIS_WEB_ALLOWED_HOSTS` を設定する。API の向け先は daemon。
`CELERIS_WEB_LIVE_VIEW_UPSTREAM` を指定する場合は Browser dashboard の正しい upstream に向ける。
`CELERIS_WEB_PASSWORD_FILE` と `CELERIS_WEB_SESSION_SECRET_FILE` は既存の秘密ファイルを使う。
loopback 以外の bind には password が必須。鍵やパスワードをログへ出さない。

```sh
scripts/selfdeploy/install-units.sh
systemctl --user start celeris-web@<sha12>.service
systemctl --user status celeris-web@<sha12>.service --no-pager
curl -fsS http://127.0.0.1:7720/healthz
```

`<sha12>` は gate/verify が成功した release。`/healthz` の release が一致し、login 後にホーム・受信箱・
タスク・定期実行・Browser が使えることを確認する。unit は daemon に `Wants=` を持たず、web の再起動だけで
handoff を発生させない。LAN 経由で公開する際は socket の `ListenStream` を確認してから enable/start する。

## HTTPS 前段（tailscale serve）とホーム画面

スマホから `https://home-dev.astrapia-algieba.ts.net` で開き、ホーム画面に置く構成。tailscale serve が TLS を終端し、
gateway（`127.0.0.1:7720`）には平文で渡す。gateway は `CELERIS_WEB_PUBLIC_ORIGINS` に挙げた origin だけを
「前段経由の https」とみなす（接続元が loopback、Host が一致、`X-Forwarded-Proto` が無いか `https`）。そのとき
操作（POST）の CSRF 検査・端末と Browser の Origin 検査はその https origin で比べ、cookie に `Secure` を付ける。
直接 `http://127.0.0.1:7720` で開く動作は変わらない。決定は
[ADR 2026-10-28-web-https-front-pwa](../../agent-docs/adr/2026-10-28-web-https-front-pwa.md)。

環境変数の関係:

| 変数 | 役目 |
|---|---|
| `CELERIS_WEB_PUBLIC_ORIGINS` | 前段の https origin（カンマ区切り、`https://host[:port]` だけ）。host は Allowed Host にも入る。設定すると `CELERIS_WEB_PASSWORD_FILE` が必須 |
| `CELERIS_WEB_ALLOWED_HOSTS` | 受け付ける Host を足すだけ（DNS rebinding 対策）。scheme の判定には使わない。LAN の `:7721` 等はこちら |
| `CELERIS_WEB_TERMINAL_HOSTS` | 端末を開ける Host（[web 端末](web-terminal.md)）。公開 origin を足しても端末は別に opt-in |

人が行う手順（worker は本番の設定変更・再起動をしない）:

1. tailscale serve を設定する。`sudo tailscale serve --bg --https=443 http://127.0.0.1:7720`。
   確認: `tailscale serve status` に `https://home-dev.astrapia-algieba.ts.net` → `http://127.0.0.1:7720` が出る。
   以前の `http:80` の serve を残すかは人が決める（残すと平文の入口が残る。消すなら
   `sudo tailscale serve --http=80 off`）。
2. `~/.config/celeris/web.env` に次の 1 行を足す（`CELERIS_WEB_PASSWORD_FILE` が設定済みであることも確かめる）。

   ```sh
   CELERIS_WEB_PUBLIC_ORIGINS=https://home-dev.astrapia-algieba.ts.net
   ```

   端末もスマホから使うなら `CELERIS_WEB_TERMINAL_HOSTS=home-dev.astrapia-algieba.ts.net` も入れる。
3. gateway を再起動する（daemon は止めない）。`<sha12>` は動作中の release。

   ```sh
   systemctl --user restart celeris-web@<sha12>.service
   systemctl --user status celeris-web@<sha12>.service --no-pager
   curl -fsS http://127.0.0.1:7720/healthz
   ```

   設定値が誤り（http の origin・path 付き）なら gateway は起動せず、journal に `CELERIS_WEB_PUBLIC_ORIGINS` の
   エラーが出る。
4. 確認する。
   - host で: `curl -sI https://home-dev.astrapia-algieba.ts.net/manifest.webmanifest` が 200・
     `content-type: application/manifest+json`。`curl -sI https://home-dev.astrapia-algieba.ts.net/` の
     `content-security-policy` に `manifest-src 'self'` がある。
   - スマホで `https://home-dev.astrapia-algieba.ts.net` を開き、ログインする。ホームで操作（例: CoS チャットへの
     送信、受信箱の項目の解決）が 403 にならずに通る。
   - ホーム画面に追加する。iPhone は Safari の共有ボタン →「ホーム画面に追加」、Android は Chrome のメニュー →
     「ホーム画面に追加」。追加した icon から開くとアドレスバーの無い表示（standalone）になる。
   - 直接の入口も確かめる: このマシンで `http://127.0.0.1:7720` にログインし、操作が通る。

戻すときは `web.env` の `CELERIS_WEB_PUBLIC_ORIGINS` の行を消して gateway を再起動する。PWA は manifest と icon だけで、
service worker は無い（ADR-0081 の cache 禁止を守る）。オフライン表示と通知（Web Push）は無い。

## 配布と確認

`release.sh` は web の install・typecheck・test・release と offline production dependency install を必須にする。
`gate.json.web` の `blocking` は true。失敗した release は公開しない。`verify.sh` は展開された app を一時的な
loopback port で起動し、`/healthz` と release の一致を必須検査する。

読み取り専用の staging e2e を追加する場合:

```sh
SD_REPO="$PWD" SD_VERIFY_WEB_HOOK="$PWD/web/scripts/staging-readonly-parity.sh" \
  WEB_STAGING_LOG_DIR=<ログの絶対パス> scripts/selfdeploy/verify.sh <sha12>
```

hook は `<sha12> <release-dir> <staging-api-url> <token-file>` を受け、snapshot API に接続する gateway を
一時起動する。結果は `verify.json` の `4c web-parity` に記録し、失敗なら verify は非0。

## 切り替えと復旧

`promote.sh` は daemon の切り替え後、current の更新前に `web-follow.sh <new> <old>` を呼ぶ。
新 app を一時起動してから旧 web を止め、新 web を起動し、health の release が一致した後に current を更新する。
初回（旧 web なし）も新 web を起動する。起動失敗は旧 web を復旧して promote を失敗にする。
このとき daemon は新しい版の場合があり、ログを確認して修復後に同じ promote を再実行する。

復旧には `scripts/selfdeploy/rollback.sh` を使う。動作中の release や秘密ファイルを削除しない。
