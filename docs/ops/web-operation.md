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
