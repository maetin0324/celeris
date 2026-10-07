# ADR 2026-10-07: ブラウザ実行の owner session に「信頼できるデバイス」を入れ、再起動・promote 後も再承認なしで復帰する

- 日付: 2026-10-07
- 状態: 提案（task 01M4ADMXWYHPSVEJBJ5JPCR6PS。方式 `cookie-then-passkey` と期限 `sliding-90` は計画承認時の人の決定）
- 関連: ADR 2026-10-05-browser-department-web-live-view D2.1（web の owner session）、ADR-0080 D6（owner grant）、
  ADR-0099・ADR-0113（attestation 署名）、ADR-0135（web release と web-follow）、ADR-0095 付記 D-d（本番操作は人）

## 1. 文脈

人の要望（2026-10-07）: 信頼できるデバイスを登録したら、daemon と web を再起動してもそのまま使えるようにしたい。

### 1.1 現状（2026-10-07、base `3cf9da6c` のコードで確認）

- **owner session はメモリだけ**: `web/server/browser-live.js` の `createBrowserLive` が `let owner = null` を持ち、
  `approve()` が `owner = { session, expires: now() + 24h }` を入れる。web の再起動・promote（web-follow が新 release の
  web を起動する）で消え、24 時間でも切れる。logout・別 session の承認（`revoke()`）でも消える。
- **承認の経路**: `POST /browser/owner-session` が 12 hex・1 回限り・5 分の challenge を出し、本番 host で
  `celerisctl browser owner-session approve <challenge> --socket <CELERIS_WEB_OWNER_SOCKET>` を打つ。socket は 0700 dir・0600。
- **owner の識別**: `auth.sessionKey(req)` = `sha256("browser-owner\0" + login cookie の値)`。login cookie
  （`__celeris_web_session`、`web/server/auth.js`）は `{iat, id}` を HMAC-SHA256 で署名した値で 24 時間有効。
  鍵は `CELERIS_WEB_SESSION_SECRET_FILE`。無ければ process ごとの乱数で、再起動で全 login が無効になる
  （本番は `scripts/selfdeploy/lib.sh` の `sd_web_load_env` が `~/.config/celeris/web.session-secret` を指すので login 自体は再起動を越える）。
  cookie 属性は `httpOnly`・`SameSite=Strict`・`Secure` は `req.socket.encrypted` のときだけ・`path=/`。
- **daemon への署名**: web は Ed25519 秘密鍵（`CELERIS_WEB_ATTESTATION_KEY_FILE`）で `RelayClaims` / `AttestationClaims`
  （TTL 20 秒、daemon 上限 30 秒）を署名し、daemon（`crates/task-api/src/browser_live.rs` の `verify`）が公開鍵で検証する。
  web → daemon は `Authorization: Bearer <api.token>`。
- **promote 時の probe**: `scripts/selfdeploy/lib.sh` の `sd_web_app_probe`（web-follow.sh・verify.sh が使う）は
  `sd_web_load_env` で本番の `web.env` を読み、新 release の `server/index.js` を一時 port で起動する。
  `web.env` に `CELERIS_WEB_OWNER_SOCKET` があれば、probe の web も `startSocket()` で同じ path に listen しようとする
  （現行コードは unlink しないので、本番 web が持つ socket があれば EADDRINUSE で probe が落ちる。stale socket の unlink を
  足せば本番の socket を奪う）。端末の書き込みを足すと、probe が本番の端末の行を回転させる恐れもある。

## 2. 決定

### D1. 端末の秘密の方式: 乱数 cookie（今回）→ passkey（https 化の後）

人の決定 `cookie-then-passkey`。今回は cookie を実装し、WebAuthn passkey は web が https で配られるようになってから
同じ表に方式の欄を足して追加する（今回は作らない）。

| | cookie に 32 byte 乱数 | WebAuthn passkey |
|---|---|---|
| LAN の http（`http://192.168.1.103:7721`） | 使える。ただし `Secure` を付けられない（下記） | **使えない**。`navigator.credentials` は secure context（https か `localhost`）でしか公開されず、LAN の IP の http は secure context ではない |
| 秘密の在りか | ブラウザの cookie store（httpOnly で JS から読めない） | 認証器（OS・セキュリティキー）。抜き出せない |
| 盗まれたとき | 平文の経路・端末の侵害で値が漏れ得る。回転と再提示検知（D4）で被害を一度の窓に絞る | 漏れない（署名鍵は端末外に出ない） |
| 実装量 | 小（Node crypto だけ） | 大（attestation/assertion の CBOR 解析、rpId・origin の固定、依存の追加） |
| 再起動の耐性 | daemon の DB にあれば越える | 同じ |

- **http の制約**: `Secure` 属性付き cookie は http では保存・送信されない。`__Host-` / `__Secure-` 接頭辞も `Secure` を要するので http では使えない。
  したがって http では cookie の値は平文で LAN を流れる。
- **緩和策**:
  1. 端末 cookie だけでは何もできない。必ず password login 済みの session と組にする（D3）。
  2. `httpOnly`・`SameSite=Strict`・path を `/browser/owner-session` に絞り、他の要求には載せない。
  3. 使うたびに秘密を回転し（D4）、旧秘密の再提示はその端末を失効させる。盗聴した値は、本人が次に使うまでの間に一度だけ、
     しかも password も持つ者にしか使えず、使われれば本人の次の復帰で再提示が検知されて端末が落ちる。
  4. 要求が https（`req.socket.encrypted`、または今後の信頼できる reverse proxy）なら `Secure` を付ける。
  5. 運用の推奨として、LAN 外に出さない・https 化（reverse proxy / tailnet の証明書）を進める。https 化は passkey の前提でもある。
- **cookie の名前・値・属性**:
  - 名前 `__celeris_web_device`（login cookie `__celeris_web_session` と並べる。gui/ の cookie とは別）。
  - 値 `<device_id>.<secret>`。`device_id` は ULID（26 文字、秘密ではない）、`secret` は `randomBytes(32)` の base64url（43 文字）。
  - `httpOnly`、`SameSite=Strict`、`Secure` は https のときだけ、`path=/browser/owner-session`、
    `Max-Age` = その端末の現在の期限までの秒数（D4）。
  - 形の不正・長さ超過（256 byte）の cookie は読まずに無視し、`Set-Cookie` で消す。

### D2. 保存: daemon の SQLite に hash だけ。web は daemon token と Ed25519 assertion で呼ぶ

- 表 `browser_trusted_devices`（task-core の migration。版数は store 葉が実装時点の空き番号を使う）:
  `id`（ULID）、`name`（人が付ける名前、64 文字まで）、`secret_hash`（現行秘密の SHA-256 hex）、
  `prev_secret_hash`（直前の秘密の hash。再提示検知用。1 世代だけ）、`created_at`、`last_used_at`、`expires_at`、
  `absolute_expires_at`、`revoked_at`、`revoked_reason`、`actor_id`。
- **秘密の値は web の外に出さない**。web が `sha256("celeris-device\0" + secret)` を計算し、daemon には hash だけを送る。
  DB・events・log・API の応答・`result.json` のどこにも秘密の値は現れない。hash は秘密ではないが、API の応答にも返さない。
- 時刻は store に注入した時計で扱い、期限の試験は時計の差し替えで決定的に行う。
- daemon の端点（`/api/v1/browser/trusted-devices`）は既存の daemon token（Bearer）に加え、web の Ed25519 署名を要する。
  - 署名の鍵・公開鍵の設定・ring での検証は既存のまま使う（鍵と検証の仕組みは変えない）。
  - claims は新しい型 `DeviceClaims`
    `{ purpose: "device_register"|"device_resume"|"device_revoke"|"device_list", owner_session_id, actor_id, device_id?, presented_hash?, next_hash?, expires_at }`。
    TTL は既存と同じ 20 秒（daemon 上限 30 秒）。`purpose` で既存の `RelayClaims`/`AttestationClaims` と取り違えない。
  - 端点:
    - `POST .../trusted-devices`: 登録。
    - `POST .../trusted-devices/resume`: 検証と回転を 1 つの transaction で行う。
    - `GET .../trusted-devices`: 一覧。`id`・`name`・`created_at`・`last_used_at`・`expires_at`・`absolute_expires_at` を返す。
    - `POST .../trusted-devices/{id}/revoke`: 失効。
  - 登録・一覧・失効は、web が現在の owner session を持つときだけ署名する。resume は「login session あり＋端末 cookie あり」で署名する（D3）。

### D3. 復帰: password login 済み session ＋ 有効な端末 cookie のときだけ owner session を作る

- **初回登録**: host CLI 承認で owner session を得たあと、画面の「この端末を信頼する」で `POST /browser/owner-session/device`
  （名前を付ける）を呼ぶ。web が秘密を作り、hash を daemon に登録し、cookie を返す。owner session でなければ 403 `not_owner`。
- **復帰**: `POST /browser/owner-session/resume`。条件は 3 つ: 有効な login cookie（`auth.sessionKey` が値を返す）、
  `__celeris_web_device` cookie、daemon の resume が成功すること。成功時の処理:
  1. web は新しい秘密を作り、daemon が `presented_hash` を `next_hash` へ原子的に置き換える。
  2. web は新しい cookie を返し、メモリに `owner = { session, device_id, expires }` を作る（既存の `revoke()` で前の owner は落とす）。
  3. `expires` は `min(now + 24h, login cookie の期限, 端末の expires_at)`。
- 画面は owner でないとき（`GET /browser/owner-session` が `isOwner: false`）に、端末 cookie があれば自動で resume を 1 回試み、
  失敗したら従来の challenge 表示に戻る。web は cookie の有無を JS に見せない（httpOnly）。
  そのため `GET /browser/owner-session` は `trustedDevice: true|false`（端末 cookie を受けたか）を返す。
- **本人確認の強さは下げない**: 端末 cookie だけ、または password だけでは owner にならない。password ＋ 登録端末の 2 要素が揃うときだけ、
  CLI 承認の代わりになる。
- **再起動を越える**: owner はメモリのままでよい。再起動・promote 後は、最初の画面の読み込みで resume が走って作り直される。
  daemon の再起動は端末の行（SQLite）に影響しない。login cookie は本番では `web.session-secret` で再起動を越える。
  `CELERIS_WEB_SESSION_SECRET_FILE` が無い構成では login からやり直しになるが、login 後の再承認は要らない。
- **同時の resume**: 1 つの web process 内で同じ `device_id` の resume は in-flight を共有して 1 回にまとめる
  （複数タブが同時に旧 cookie で来ても、旧秘密の再提示と誤判定しない）。

### D4. 期限・上限・回転・失効

人の決定 `sliding-90` を次のように読む（計画時の推奨どおり）。

- **期限**: 最後の使用から 30 日で失効する。使う（resume が成功する）と `expires_at = min(now + 30 日, absolute_expires_at)` に延長する。
- **絶対上限**: 登録から 90 日（`absolute_expires_at`）。延長はこれを越えない。越えたら CLI 承認から再登録する。
- **上限数**: 有効（未失効・未期限切れ）な端末は 5 台まで。6 台目の登録は 409 `device_limit` で拒否する。古い端末を黙って追い出さず、人が一覧から失効させる。
- **回転**: resume のたびに秘密を替える（D3）。登録時の秘密も一度だけ使える。
- **再提示（使い回し）の検知**: `presented_hash` が現行の `secret_hash` ではなく `prev_secret_hash` と一致したら、盗用か複製とみなす。
  その端末を `revoked_reason = "reuse"` で失効させ、拒否する（web のメモリの owner がその端末由来なら落とす）。
- **拒否**: 未知の id、hash 不一致、失効済み、期限切れ（`expires_at` / `absolute_expires_at` ≤ now）は 403 `device_rejected`。
  web は cookie を消す。拒否の理由の区別は events に残し、HTTP 応答では区別しない。
- **失効**: 一覧から人が失効（`revoked_reason = "owner"`）させる。daemon の行を即時に `revoked_at` にし、web はメモリの owner が
  その `device_id` 由来なら即座に `revoke()`（live の WebSocket も閉じる）。
  - 端末の daemon API を呼ぶのは web だけなので、失効は必ず web を通り、同じ process のメモリに反映できる。
  - promote の切り替え中に旧 web が残る数秒は、旧 web の owner の `expires` が端末の期限を越えないことで抑える。
- logout はその login session の owner を落とすが、端末の登録は残す（次の login で resume できる）。

### D5. events

daemon が端末の操作を events に追記する（追記専用。秘密も hash も載せない）。

| event | 主な欄 |
|---|---|
| `browser_trusted_device_registered` | `device_id`, `name`, `actor_id`, `expires_at`, `absolute_expires_at` |
| `browser_trusted_device_used` | `device_id`, `actor_id`, `expires_at`（延長後） |
| `browser_trusted_device_revoked` | `device_id`, `actor_id`, `reason`（`owner` / `reuse`） |
| `browser_trusted_device_rejected` | `device_id`（実在する id のときだけ）, `actor_id`, `reason`（`unknown` / `mismatch` / `revoked` / `expired` / `reuse` / `limit`） |

- `actor_id` は web の `CELERIS_WEB_OWNER_ID`（既定 `owner`）を claims で受ける。
- 実在しない id の拒否は `device_id` を載せない。攻撃者が選んだ文字列を events に書かない。
- 期限切れは受け身なので、使われたときの `rejected(expired)` だけを残す。

### D6. host CLI 承認は残す

`celerisctl browser owner-session approve` と owner socket の経路は変えない。用途は 2 つ:

- 端末の初回登録（owner session を得てから登録する）。
- 登録端末を失ったときの復旧（別端末で CLI 承認 → 一覧から失った端末を失効）。

端末が 1 台も無くても従来どおり使える。

### D7. promote の probe と共存する: `CELERIS_WEB_PROBE=1`

- `sd_web_app_probe` は起動する node に `CELERIS_WEB_PROBE=1` を付ける（web-follow.sh・verify.sh の両方に効く）。
  加えて `CELERIS_WEB_OWNER_SOCKET` を unset して起動する（二重の保護）。
- web は `CELERIS_WEB_PROBE=1` のとき:
  - owner socket を開かない（`startSocket()` が null）。
  - 端末の書き込み（登録・resume による回転と最終使用の更新・失効）をしない。端末系の端点は 503 `probe_mode` を返す。
  - `/healthz` など probe が見る応答は変えない。
- これで probe が本番の owner socket を奪う・落ちる、本番の端末の秘密を回転させて本人の cookie を無効にする、の両方を防ぐ。

## 3. 代替案と却下理由

- **owner session そのものを SQLite に永続化する**（再起動後も同じ owner を使う）: login cookie を盗めば端末に関係なく owner になれる。
  login cookie の 24h を越えて owner を延ばす理屈も無い。本人確認が password 1 要素に下がるので却下。
- **WebAuthn passkey を今回入れる**: LAN の http では使えず、現状の運用（`http://192.168.1.103:7721`）で要望を満たせない。
  https 化の後に追加する（人の決定 `cookie-then-passkey`）。
- **端末の hash を web の file（`~/.config/celeris/web-private/`）に置く**: daemon の DB に比べて events・transaction・試験の基盤が無い。
  release ごとに置き場が分かれる。probe も同じ file に書き得る。却下。
- **秘密の値を daemon に送って daemon が hash する**: daemon の要求 log・panic 時の出力に値が出る経路が増える。web で hash して送る。
- **回転しない長期秘密**: 盗聴された値が期限まで使え、検知もできない。http では特に危険なので却下。
- **上限超過で最古の端末を自動失効**: 人の知らないうちに使っている端末が落ちる。拒否して人に選ばせる。
- **CLI 承認の廃止**: 初回登録と端末喪失時の復旧の経路が無くなる。残す。

## 4. 影響と葉の分担

- task-core（store 葉）: migration・store・Event 4 種・注入した時計。
- task-api（api 葉）: `/api/v1/browser/trusted-devices` と `DeviceClaims` の検証。
- selfdeploy（probe-guard 葉）: `sd_web_app_probe` に `CELERIS_WEB_PROBE=1` と socket の unset。
- web gateway（web-server 葉）: 登録・resume・一覧・失効、in-flight の集約、probe mode、失効の即時反映。
- web 画面（web-ui 葉）: 登録ボタン・自動復帰・一覧と失効。
- 試験で確かめること:
  - 登録 → web 再起動 → 再承認なしで owner。
  - 失効・期限切れ（時計の差し替え）・誤った秘密・旧秘密の再提示が拒否される。
  - 上限 5 台。probe mode で socket と書き込みが無い。
  - events と log に秘密が出ない。
- 残る危険: http の LAN では端末 cookie が平文で流れる（D1 の緩和策を参照）。https 化と passkey は別 task。
