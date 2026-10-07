# ブラウザの信頼端末: 配送後に本番で行う確認手順

---
tasks: [01M4ADMXWYHPSVEJBJ5JPCR6PS]
---

信頼端末（[ADR 2026-10-07-browser-trusted-devices](../../agent-docs/adr/2026-10-07-browser-trusted-devices.md)）を
本番に配送した後、運用セッション（人）が本番 host で行う確認の手順。手順 1 は読み取りだけ。手順 2〜5 は web の画面と
host の CLI 承認で行い、本番 DB を直接書き換えない。release・promote そのものは [selfdeploy.md](selfdeploy.md) の手順で行う。

前提と用語:

- web の URL を `<web>` と書く（例 `http://192.168.1.103:7721`。bind は `~/.config/celeris/web.env`）。
- 本番 DB は `/local/celeris/data/db/celeris.sqlite3`（[local-hot-data-migration.md](local-hot-data-migration.md)）。
  読むときは必ず `sqlite3 -readonly` を使う。
- 端末 cookie は `__celeris_web_device`（値 `<device_id>.<secret>`、httpOnly・SameSite=Strict・`Path=/browser/owner-session`、
  https のときだけ Secure）。login cookie `__celeris_web_session` とは別。期限は最後の使用から 90 日で、使うと延長される。
  有効な端末は 5 台まで。復帰のたびに秘密が回転する。
- 端末の画面: 一覧と失効は `<web>/browser/devices`（「信頼できる端末」）。登録は `<web>/browser` と Live View の
  「この端末を信頼する」。

## 1. release 後に migration 0057（信頼端末の表）が本番 DB に入ったか

新 release の daemon が current になり稼働した後に読む。

```sh
readlink ~/.local/celeris/current        # releases/<sha12>。この sha が信頼端末を含む release であること
sqlite3 -readonly /local/celeris/data/db/celeris.sqlite3 \
  "SELECT version FROM schema_migrations WHERE version >= 54 ORDER BY version;"
sqlite3 -readonly /local/celeris/data/db/celeris.sqlite3 ".schema browser_trusted_devices"
sqlite3 -readonly /local/celeris/data/db/celeris.sqlite3 \
  "SELECT count(*) FROM browser_trusted_devices;"
```

確認方法（期待する出力）:

- `schema_migrations` に `57` がある。55・56 は main 側の migration（cos_run_credentials・cos_triage）で、
  それを含む release なら `55`・`56` も出る。含まない release では 55・56 は予約で欠番のまま（`RESERVED_VERSIONS`）。
- `.schema` に `CREATE TABLE browser_trusted_devices (id TEXT PRIMARY KEY, name …, method …, secret_hash …, prev_secret_hash …,
  created_at …, last_used_at …, expires_at …, absolute_expires_at …, revoked_at …, revoked_reason …, actor …)` と
  `idx_browser_trusted_devices_secret` が出る。
- 初回は `count(*)` が `0`。
- 表が無い（`Error: no such table`）なら、daemon がまだ旧 release で動いているか migration が失敗している。
  `journalctl --user -u celeris@<sha12>.service -n 50 --no-pager` を見て、[selfdeploy.md](selfdeploy.md) の戻し方に従う。

## 2. 初回: host CLI 承認 → 端末登録

1. 登録したい端末のブラウザで `<web>` に password で login し、`<web>/browser` を開く。
   「本人として登録」の challenge（12 桁の hex）を発行する。
2. 本番 host で、web が listen している owner socket を指定して承認する（5 分以内・1 回限り）。

   ```sh
   SOCK=$(grep '^CELERIS_WEB_OWNER_SOCKET=' ~/.config/celeris/web.env | cut -d= -f2-)
   celerisctl browser owner-session approve <challenge> --socket "$SOCK"
   ```

   期待する出力: `approved: this GUI session is now the browser owner`（exit 0）。
   `owner-session approve failed: unknown_challenge` なら challenge の期限切れか打ち間違い。画面で発行し直す。
3. 画面が owner になったら「この端末を信頼する」に名前（既定は UA から）を入れて登録する。

確認方法:

- 画面に登録済みの表示と `<web>/browser/devices` への link が出る。一覧に今の端末が「有効」で 1 行出る。
  CLI 承認で作った owner には端末が結び付かないので、登録直後は「この端末」badge が出ない（次の復帰から出る。下の「既知の制限」）。
- ブラウザの開発者ツール（Application → Cookies）に `__celeris_web_device` があり、Path が `/browser/owner-session`、
  HttpOnly・SameSite=Strict。https でなければ Secure は付かない。
- 本番 DB（読み取り）に登録の event がある。秘密も hash も載っていないこと:

  ```sh
  sqlite3 -readonly /local/celeris/data/db/celeris.sqlite3 \
    "SELECT ts, json FROM events
      WHERE json_extract(json,'$.type') LIKE 'trusted_device_%'
      ORDER BY id DESC LIMIT 10;"
  ```

  期待する出力: `trusted_device_registered` の行（`device_id`・`name`・`method":"cookie"`・`actor`・`expires_at`、
  `absolute_expires_at` は null）。`secret`・`hash` を含む欄は無い。
- 6 台目を登録しようとすると拒否される（409 `device_limit`）。古い端末を一覧から失効させてから登録する。

## 3. web 再起動と promote（web-follow）の後に、challenge なしで owner に戻る

再起動・promote は通常の運用の機会に行う（この確認のために本番を再起動しなくてよい）。

```sh
SHA=$(basename "$(readlink ~/.local/celeris/current)")
systemctl --user restart celeris-web@"$SHA".service      # 再起動の場合
systemctl --user is-active celeris-web@"$SHA".service    # active
# promote の場合は promote.sh が web-follow.sh を呼ぶ（selfdeploy.md §4e）。新 sha の unit が active であること:
systemctl --user list-units 'celeris-web@*' --no-pager
```

確認方法:

- 登録済みの端末で `<web>/browser` を開き直す（login cookie は `~/.config/celeris/web.session-secret` で再起動を越えるので、
  login からやり直しにはならない）。challenge の表示を経ずに owner の画面になる。
- 一覧（`<web>/browser/devices`）でその端末に「この端末」badge が付き、「最終使用」が今の時刻、期限が今から 90 日後になる。
- events に `trusted_device_used`（`device_id`・延長後の `expires_at`）が 1 行増える（手順 2 の SQL）。
- promote の probe は端末を書き換えない（`CELERIS_WEB_PROBE=1` と owner socket の unset。ADR D7）。promote の前後で
  probe 由来の `trusted_device_used`・`trusted_device_rejected` が増えていないこと、本番の owner socket が残っていることを見る:

  ```sh
  test -S "$SOCK" && echo socket-ok
  ```

- challenge が出たら: 画面の alert に理由（失効・期限切れ・不一致 / 接続不可）が出る。
  接続不可なら daemon（`celeris@<sha12>`）の稼働を、拒否なら events の `trusted_device_rejected` の `reason` を見る。

## 4. 登録端末の一覧と失効（失効後は即 challenge）

1. owner の画面で `<web>/browser/devices` を開く。各端末の名前・登録日時・最終使用・期限・状態（有効 / 失効済み /
   使い回し検知 / 期限切れ）が出る。秘密・hash は出ない。
2. 失効させる端末の「失効」を押し、確認ダイアログで確定する。

確認方法:

- その行の状態が「失効済み」になる。events に `trusted_device_revoked`（`reason":"owner"`）が出る。
- 今使っている端末を失効させた場合: その場で owner が落ち（Live View の接続も閉じる）、画面は challenge の表示に戻る。
  端末 cookie も消える。読み込み直しても自動復帰しない。
- 別の端末を失効させた場合: 失効した端末で `<web>/browser` を開くと、自動復帰が拒否され challenge の表示になる
  （events に `trusted_device_rejected`）。
- DB の読み取りでも確かめられる（hash の列は選ばない）:

  ```sh
  sqlite3 -readonly /local/celeris/data/db/celeris.sqlite3 \
    "SELECT id, name, datetime(last_used_at,'unixepoch'), datetime(expires_at,'unixepoch'),
            datetime(revoked_at,'unixepoch'), revoked_reason
       FROM browser_trusted_devices ORDER BY created_at;"
  ```

  期待する出力: 失効させた行の `revoked_at` に時刻、`revoked_reason` が `owner`。

## 5. 端末を失ったときの復旧

失った端末の cookie が他人の手にあっても、password login が無ければ owner にはなれない（ADR D3）。それでも速やかに失効させる。

1. **別の登録端末がある**: その端末で `<web>/browser/devices` を開き（自動復帰で owner になる）、失った端末の行を失効させる。
2. **登録端末が他に無い**: 手元のブラウザで login し、手順 2 の host CLI 承認（`celerisctl browser owner-session approve`）で
   owner になってから、`<web>/browser/devices` で失った端末を失効させる。CLI 承認は端末の有無に関係なく使える（ADR D6）。
3. 新しい端末を使うなら、その端末で手順 2 の「この端末を信頼する」を行って登録し直す。

確認方法:

- 一覧で失った端末が「失効済み」、events に `trusted_device_revoked`（`reason":"owner"`）。
- 失った端末の秘密が使われると、本人の側の復帰か盗用側の復帰のどちらかで旧秘密の再提示が起き、端末が
  `revoked_reason = 'reuse'` で自動失効する（一覧の「使い回し検知」、events の `trusted_device_revoked`・`reason":"reuse"`）。
  見つけたら password（`~/.config/celeris/web.password`）の変更を検討する（本手順の外。人が決める）。

## 既知の制限

- CLI 承認で作った owner には `deviceId` が付かない。登録直後は「この端末」badge が出ず、その端末を一覧から失効させても
  今の owner は落ちない（次の読み込みからは端末として扱われない）。次の復帰の後からは付く。
- 端末の失効・一覧は web の画面（gateway）からだけ行える。daemon の端点は web の Ed25519 署名を要するので、
  `celerisctl` や `curl` から直接は失効できない。CLI 承認 → 画面での失効が復旧の経路。
- 自動復帰は画面の読み込みごとに 1 回だけ試す。失敗後は読み込み直すまで challenge の表示のまま。
- LAN の http では端末 cookie が平文で流れる。https 化と passkey は別 task（ADR D1）。
