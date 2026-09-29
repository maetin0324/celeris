# Browser capability Phase 2

---
tasks: [01M3MZKB3DFYJNBH015MJGQ0BT]
---

完了日: 2026-09-29。契約は [ADR-0080](../adr/0080-browser-phase2-policy-broker-approval.md)（Phase 1 は ADR-0078、[phase-browser.md](phase-browser.md)）。
本番の昇格は行っていない（人が GUI で行う）。

## 実装したもの

- **policy 生成（D1）**: admin grant ∩ task policy ∩ backend が対応する action から実効 policy を作り、agent-browser の action policy（内部 action 名の空でない allow と default deny）と allowed-domains を生成する。空集合・未知の action・壊れた schema は起動前に固定コードで Err を返す。shim は許可外の action と許可外 host への open を substrate に渡す前に止める（否定テスト: `evaluate`・`cookies_get`・`state_save` が allow に入らないこと、許可外 domain は拒否されること、改ざんした policy は拒否されること）。
- **credential broker（D2/D3）**: `crates/celeris-credentiald`。`CredentialProvider` 抽象の上に最初の provider として手動登録を実装した。XChaCha20-Poly1305 で暗号化して保存し、権限を検査する。control IPC と resolve IPC を分ける。lease は task・run・session・origin に束縛した一回限りのもので、監査も付ける。固定版 agent-browser の plugin bridge（`agent-browser.plugin.v1` / `credential.resolve`）を使う。LLM には success/failure だけを返す。秘密値の保存方法は ADR-0080 D3 に書いた。
- **wait と承認（D4/D5）**: migration 0032（main の 0031_task_tree の後へ振り直した。`browser_waits`・`browser_credentials`・`browser_approvals`。秘密値の列は無い）。task は Blocked のまま、wait の reason として WAITING_FOR_AUTH/APPROVAL を持つ。作成・解決・期限切れ・cancel は task の遷移と同じトランザクションで確定し、version の CAS で重複を防ぐ。人の操作には bearer に加えて GUI 専用鍵の Ed25519 human attestation を要求する。
- **結線（e2e）**: 承認後の run は承認 wait を一度だけ消費し、credentiald に bind と grant を求める。origin が合わないときや失敗したときは lease を失効させ、再試行しない Error にする。認証後の区間では観測系の action を外し、Live View も止める。
- **GUI（D5/D6）**: task 画面の「ブラウザの人待ち」に登録フォームと承認・拒否の操作を置いた。本人 session・exact Origin・CSRF・wait の version を検査する。Live View は `/browser/live/:taskId/:runId` の本人 guard を通る経路だけにし、loader data と SSE から raw `live_view_url` を消した。relay を設定した場合は本人だけが閲覧できる。未設定時は 503 `live_view_relay_unavailable` を返す。詳細は `gui/docs/PROGRESS.md`。

## Live View relay（live-gui WU、2026-09-29、ADR-0080 D6）

- **設定**: `CELERIS_GUI_LIVE_VIEW_UPSTREAM=<loopback host:port>`（`127.0.0.1` / `[::1]` / `localhost` 以外は起動時に exit 2）。起動時に固定した宛先だけを使う。client からの URL・port は upstream の宛先にならない。run の `live_view_url` は「設定済みか」の gate にだけ使い、宛先にも応答にも使わない。未設定なら従来どおり本人にも 503 `live_view_relay_unavailable` を返し、リンクも出さない。
- **入口**: `GET /browser/live/:taskId/:runId` は owner grant と run の guard（task/run の対応、active、RUNNING、設定済み、認証区間外）を通したうえで、upstream `GET /`（`?port=` は 1〜5 桁のときだけ）を relay する。ヘッダは relay 側で決める（no-store、no-referrer、nosniff、X-Frame-Options DENY、`script-src 'self' 'unsafe-inline'` と `connect-src 'self'` の CSP）。upstream の Set-Cookie・Location などは捨てる。React Router の nonce CSP を避けるため、express の middleware（`gui/server/app.ts`）で React Router より前に処理する。
- **束縛**: 入口を開いた本人の session（cookie ID のハッシュ）に、見ている task/run をメモリ上で束縛する。dashboard は絶対 URL を使うので、GUI の origin の root で次のものを受ける。`GET /_next/static/*`、`GET /api/sessions`、`GET /api/chat/status`、`GET /api/session/:port/{tabs,status}`、WebSocket `/api/session/:port/stream`。同じものを `/browser/live/:taskId/:runId/...` の下でも受け、その場合 prefix の run は束縛と一致しなければならない。これらを relay する条件は「owner で、束縛があり、束縛した run が今も guard を通る（結果は 3 秒 cache）」ことである。未認証は 401、他の session と認証無効は 403、束縛なしと他 run は 404。
- **拒否**: 既定で拒否する allow list にしてある。上記以外の `/api/*` と `/_next/*`、および GET 以外のすべては `403 live_view_action_denied` になる（`/api/exec`、`/api/kill`、`POST /api/sessions`、`/api/chat`、`/api/models` を含む）。stream で client から upstream へ転送するのは `ack` と `config`（`maxFps` と `pacing` だけ。client ごとの frame 配信の設定）の JSON だけである。`input_mouse`・`input_keyboard`・`input_touch`、binary、その他は捨てて数だけ記録し、接続は切らない。WebSocket は Origin の完全一致も要求する。
- **失効**: logout、grant の期限切れ、再登録（`onOwnerRevoked`）で束縛をすべて捨て、既存の WebSocket を close 1001 で切る。接続中も 5 秒ごとに owner と run を照合し、RUNNING/active でなくなるか認証区間に入ったら 1008 で切る。ログに出すのは判断コードと捨てた message の数だけで、URL・token・cookie・session ID は出さない。
- **実装**: `gui/app/celeris/browser-live.server.ts`（guard、設定、束縛、入口）、`browser-live-relay.server.ts`（express middleware と `upgrade`）、`ws-codec.server.ts`（RFC 6455 の最小実装。依存は追加していない）。`server.js` が `attachLiveViewUpgrade` を http.Server に付ける。
- **確認**: 単体テストは `gui/test/unit/browser-live-relay.test.ts`（偽 upstream の HTTP/WS を使う 15 件）。実機では、`/tmp` の使い捨てスクリプトで実 agent-browser 0.38.1 の dashboard（loopback）、`pnpm build` した `server.js`、Playwright の chromium を使い、次を確かめた。本人 A では HTML、13 個の `/_next/static` asset、`/api/sessions`、`/api/chat/status`、`/api/session/<port>/tabs` が 200 で、stream の WS が開き frame が届いた（dashboard は「Live」表示）。dashboard 上で click と key 入力をすると `input_mouse` と `input_keyboard` が送られたが、fixture のクリック数は 0 のまま、title も変わらなかった。`POST /api/exec` と `/api/models` は 403。別ログイン B では HTML、asset、API が 403 で WS は失敗。未認証では HTML、asset、API が 401 で WS は失敗。他 run は 404。A の logout で既存 WS が閉じた。GUI のログに upstream の port、URL、token は出なかった。dashboard は外部画像（svgl.app、google favicon）を読もうとするが、CSP の `img-src` で止まる。
- **残る限界**: (1) dashboard と GUI は同じ UID で動く。同一 UID の相手が loopback の dashboard へ直接接続することは防げない（loopback の Host/Origin なら upstream は token を要求しない）。(2) dashboard は namespace 内の全 session を表示する。本人の session だけを namespace に置く運用が前提である。束縛は run 単位の認可であり、dashboard 内の session 選択までは絞れない。(3) 別 network namespace や外部 host からの到達不能の probe はしていない（未検証）。(4) `/favicon.ico` は relay しない（404）。

## 実 daemon・実 GUI・実ブラウザの Live/認証確認（g14、live-gui WU、2026-09-29）

- **何を確かめるか**: 使い捨ての daemon と GUI（`server.js`）を立て、Playwright の chromium で人の操作を通す。(a) WAITING_FOR_AUTH → owner 登録 → GUI の資格情報フォーム → Ready → 新しい run。(b) 承認 → resume、拒否 → failed（`approval_denied`）。(c) Live View は owner が dashboard を見られ、ログイン済みの非 owner は 403、未認証は 401、`POST /api/exec` は 403。(d) password の sentinel が DB・WAL・events API・daemon と GUI のログ・artifacts に無いこと。
- **コマンド**: `gui/scripts/browser-live-e2e.sh`（spec は `gui/e2e/g14-browser-live.spec.ts`、fixture は `gui/test/celeris/`）。
- **結果**: 4/4 passed（2026-09-29）。(d) は 90 ファイルを走査して 0 hit。
- **証跡**: スクリーンショットとログは WU の artifacts（リポジトリの外）の `g14/` に置いた。
- **seeding の注意**: `browser_updated` の RUNNING event は、dispatcher の browser supervisor だけが出す（acp/claude-code と LLM が要る）。この e2e では scratch の SQLite に 1 件だけ直接入れている。同じ理由で、試験用の task には browser 用 skill を付けていない。したがって supervisor が実際にブラウザを起動して event を出す経路は、この試験の対象外である。
- **環境**: scratch の port は 27700（daemon）、27710（upstream の dashboard）、27848（GUI）。本番には触れていない。

## 実 agent-browser での auth login 確認（live-gui WU）

2026-09-29。`scripts/browser-auth-login-check.py`。検証環境に置いた固定版 agent-browser 0.38.1 を使い、`celeris-credentiald` は scratch の HOME と XDG_RUNTIME_DIR に立てた。fixture は 127.0.0.1 の自己署名 HTTPS。ネットワークにも LLM にも出ていない。詳細な記録（手順、各 variant の exit code、生ログ）は WU の artifacts の `auth-login/REPORT.md`。

**当時の判定: 部分的に確認**。以下は修正前の調査記録。後続の `4cedc73f75ba` で 5 件を修正し、worker 設定で実 agent-browser 0.38.1 の `auth login` 成功、lease 再使用拒否、sentinel 0 件を確認した（live-gui WU `auth-wiring/production5`）。

- 確認できたこと: bridge が `credential.resolve` を受ける。resolve socket 経由で broker が lease を消費する（journal に `use/consumed`）。`--no-navigate` で origin を照合しフォームを埋めて submit する。fixture が正しい資格情報を受け取り、ログイン後に `LOGIN-OK-<rand>` を `get text body` と `snapshot` の両方で観測した。同じ lease の再使用は拒否された（`deny/used`）。stdout・stderr・journal・scratch 全体に sentinel は 0 hit。
- 現在の結線との差（5 件）:
  1. upstream config の `plugins` は map ではなく配列 `[{name,command,args,capabilities:["credential.read"]}]`。map だと config の読み込みで全コマンドが exit 1 になる。
  2. `auth login` の argv は `auth login <name> --credential-provider P --item L …`。Celeris の `<name>` 省略と `--credential-ref` は不正（`unknown flag`）。
  3. binding token の FD 3 が plugin に届かない。plugin を起動するのは CLI ではなくセッションの daemon で、daemon は自分を起動した CLI（最初の `open`）の FD 3 だけを継承して保持する。`auth login` の CLI にだけ渡した FD 3 は無い（bridge は `denied`）。
  4. segment policy の allow に `url` が無く、`get url` が `Action 'url' denied by policy` になる。
  5. `--action-policy` のパスを変えると daemon が再起動し（`restartedBackground:true`）、ログインした状態が消える。harness は segment と別の policy パスを使うので、ログイン状態が harness に残らない。同じパスで中身だけ書き換えた場合は維持された。
- 推奨する修正（未実装。`crates/` は変更していない）:
  - `segment_upstream_config` の `plugins` を配列にする。
  - `use_credential` の argv を `auth login celeris-credential --credential-provider celeris-credential --item <lease> --no-navigate --url <origin>/` にする。`fake-agent-browser.py` も実バイナリの契約（配列 plugins、`--item`、positional name）に合わせて拒否させ、契約試験を足す。
  - `segment_policy` の allow に `url` を加える。
  - policy のパスを segment と harness で同一にして、segment 後に中身を atomic に書き換える（または daemon の再起動を避ける別の方法）。ADR にする。
  - bridge が失敗したときも stdout に `{"protocol":"agent-browser.plugin.v1","success":false}` を書き、broker に deny を残す。
  - 使用済み lease の `revoke` は成功を返さず `used` と区別する。
- 注意: 実バイナリは plugin 要求の `url` に `--url` の値をそのまま入れる。ブラウザが観測した URL ではないので、broker の origin 照合は二重の確認にすぎない。注入前の origin 検証は agent-browser 側（`--no-navigate --url`、scheme・host・port）が行う。broker は `https://` の origin しか受け付けず、loopback の http の例外は無い。
- token の受け渡しは後続の `4cedc73f75ba` で最初の `open` に FD 3 を渡す方式を採用した。daemon が起動する Chrome などの子への FD 継承は未確認。
- 限界: sentinel の走査に Chrome の一時 profile は入っていない（scratch の外にあり、close で消える）。

## 証拠（release WorkUnit、2026-09-28、base 29d933f）

| 検査 | コマンド | 結果 |
| --- | --- | --- |
| Rust format | `cargo fmt --all` を 2 回実行してから `cargo fmt --all -- --check` | exit 0、差分なし |
| Rust tests | `cargo test --workspace` | exit 0、2745 passed / 0 failed / 7 ignored（3m19s） |
| Rust lint | `cargo clippy --workspace -- -D warnings` | exit 0 |
| GUI types | `cd gui && pnpm typecheck` | exit 0 |
| GUI tests | `cd gui && pnpm test` | exit 0、77 files / 1194 passed |
| GUI lint | `cd gui && pnpm lint` | exit 0（295 files、info 2） |
| 端から端 | `cargo test -p task-api --test browser_e2e`（e2e WU） | 3 passed（成功、拒否から failed、origin 不一致から deny。sentinel は全走査） |

### release.sh / verify.sh（ADR-0040 D5、2026-09-29）

- 検証した SHA は `18ca76d57fce26d965349e49835d84813fb41459`（全体検査結果を記録したコミット。この節の追記は docs だけを変える）。
- `scripts/selfdeploy/release.sh 18ca76d57fce26d965349e49835d84813fb41459`: exit 0、gate.json は ok=true、schema_version=32（5m43s。GUI の typecheck/test/build/mobile-audit/e2e-mock を含む）。
- `SD_REPO=$HOME/workspace/agent-platform scripts/selfdeploy/verify.sh 18ca76d57fce`: exit 0、**ok=true、live_ok=false**。check 1〜4・4b（gui-e2e）・6（smoke）は true。check 5（n-1-compat）だけが false になる。本番の現行 5fcb7eebbe9a は schema 29 までしか扱えず、このリリースは 0030（cluster_connection_log）・0031（browser_waits）・0032（browser_task_policies）を適用して 32 にするため、`SchemaTooNew` で起動しない。これは決定的に起きることで、一過性の失敗ではないので再実行していない。
- 本番へは昇格していない。昇格すると N-1 の rollback ができない（schema 32 の DB を旧 binary が読めない）ことを、人は昇格前に承知しておく必要がある。

## main 追従後の検査（2026-09-29）

`main` の `bd3b2b3` を競合なく取り込み、`git merge-base --is-ancestor main HEAD` は exit 0。browser migration は 0032/0033、schema version は 33 のままで、今回の merge に migration 番号の衝突は無かった。

| コマンド | 結果 |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo test --workspace` | exit 0、2833 passed / 0 failed / 7 ignored |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cd gui && pnpm typecheck` | exit 0 |
| `cd gui && pnpm test` | exit 0、78 files / 1213 passed |

実 GUI の登録→再開と承認、本人限定 Live View は live-gui WU の `g14-final`（4 passed、スクリーンショットあり）で確認した。実 agent-browser 0.38.1 の worker 設定による `auth login` は同 WU の `auth-wiring/production5`（ログイン成功、lease 再使用拒否、sentinel 0 件）で確認した。

## 未解決事項

- Phase 3〜4 に回すもの: persistent auth（認証 state の再利用）、GUI 本体への live stream 統合、container/egress 隔離。読み取り専用 relay は本 Phase で実装・確認した。
- 外部 host / 別 network namespace から dashboard に到達できないことは未 probe。同一 UID による loopback 直結と、dashboard が namespace 内の全 session を列挙する限界も残る。
- g14 e2e の `browser_updated` は scratch DB への seeding である。supervisor が実ブラウザを起動して Live View の event を出す経路は、実 LLM の環境で確かめていない。
- GUI の control socket は Node から SO_PEERCRED を読めない。file mode（0700/0600）だけで守っており、同一 UID の相手は区別できない。

## 提案

- `4cedc73f75ba` で配列 plugins、`--item`、`url`、policy パス、FD 3 の受け渡し、fake parser を修正し、`scripts/browser-auth-login-check.py` の worker 設定による実 agent-browser 0.38.1 の認証成功を確認した。
- Live View relay は live-gui WU で実装した（上の節）。残る限界（同一 UID の直接接続、dashboard の namespace 内 session 一覧）は container/egress 隔離の task で扱う。
