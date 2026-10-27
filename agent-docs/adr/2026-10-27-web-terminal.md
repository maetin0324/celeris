# ADR 2026-10-27: web のクラスタ画面から host と登録済みクラスタの対話シェル（web 端末）を開く

---
tasks: [01M5ZA2NP31AVVKQ9F4T3A52VB]
---

- 状態: 採用（実装済み）
- 関係: ADR-0018（クラスタは人が張った ControlMaster を借りる）、ADR-0032（GUI からの接続・TOTP 中継）、
  ADR-0060（ssh master を `systemd-run --user --scope` で daemon の cgroup の外に置く）、
  [2026-10-27-api-principals](2026-10-27-api-principals.md)（主体 `human:web`）、
  [2026-10-07-browser-trusted-devices](2026-10-07-browser-trusted-devices.md)・browser Live View（`web/server/browser-live.js` の WebSocket guard）、
  ADR-0135（LAN 入口 7721 は `systemd-socket-proxyd` で loopback の web gateway へ中継）

## 背景

利用者（2026-10-27）は、web のクラスタ画面から celeris が動く host と登録済みクラスタの対話シェルを開きたい。
端末は rmaeda の権限（sudo 可）そのもので、Celeris の中で最も強い操作になる。開ける主体・入口・寿命・記録を
先に決める。

## 決定

### D1 実行の場所: web gateway が PTY helper を systemd scope で起こす

- PTY と子 process を持つのは **web gateway（Node BFF）が起こす小さな helper** `celerisctl terminal-pty` とする。
  helper は Rust で書き、`openpty` → 子を `setsid` + 制御端末にして argv を exec → stdin の frame（データ・
  リサイズ）を PTY へ、PTY の出力を stdout へ流すだけをする。LLM も DB も触らない。
- daemon（dispatcher・store）は PTY を持たない。daemon が受け持つのは **開く許可と対象の解決と監査の記録**
  （D4）だけで、決定性を崩さない。native module（node-pty）は入れない（offline の配布・prebuild の問題を避ける）。
- gateway は helper を `systemd-run --user --scope --collect --unit celeris-terminal-<id>` の中で起こす
  （ADR-0060 の master と同じく web・daemon の cgroup の外）。scope に `MemoryMax`・`CPUQuota`・`TasksMax` を付ける。
  `systemd-run` が無い環境（試験）は `inline`（gateway の直接の子）で起こす。設定 `CELERIS_WEB_TERMINAL_LAUNCHER`
  （`auto`（既定）/ `systemd-run` / `inline`）。
- 終了の保証（孤児を残さない）: helper は stdin の EOF（gateway の終了・切断）・SIGTERM・子の終了のどれでも、
  **子の session（sid = shell の pid）に属する全 process** に SIGHUP、猶予 2 秒で SIGKILL を送る。job control の
  background job（別 process group）も同じ session なので消える。さらに helper が `celeris-terminal-*.scope` の中に
  いれば、最後にその cgroup の `cgroup.kill` に 1 を書き、`setsid` で session を抜けた process も消す。

### D2 寿命: 切断で必ず終了する（再接続しない）

- WebSocket が切れたら端末は終わる（同じ PTY への再接続はしない）。再接続の窓を設けると、別の端末・別の経路から
  生きた root 相当の shell を拾える時間ができ、窓の間の出力を保持する buffer も要る。長い処理は端末の中で
  `tmux`・`nohup` を使う（その場合 scope の `cgroup.kill` で消えるので、残したい処理は端末の外の job にする）。
- 終了の契機: 画面の「終了」、WebSocket の切断、idle timeout（入出力どちらも無い時間。既定 1800 秒）、最大継続時間
  （既定 28800 秒）、web の logout（その login session が開いた端末を全て閉じる）。step-up の失効は開いている
  端末を止めない（開く時だけ要求する）。
- 同時に開ける数は gateway 全体で既定 4（`CELERIS_WEB_TERMINAL_MAX_SESSIONS`）。上限を超えた open は開かない。
  `CELERIS_WEB_TERMINAL_IDLE_SECS`・`CELERIS_WEB_TERMINAL_MAX_SECS` で変える。
- **daemon の再起動**: 端末は gateway と scope に属するので切れない。ただし開くとき（D4 の許可）と閉じた記録に
  daemon が要る。daemon が止まっている間に閉じた端末の終了記録は残らない（gateway の log に件数だけ出る）。
- **release の昇格**: promote は web gateway を新しい release に入れ替える（`web-follow.sh`）。gateway の終了で
  helper の stdin が閉じ、端末は全て終わる。画面は端末の上部にこの 2 点を常に示す。

### D3 開ける主体・入口・step-up

- 開けるのは web に login した人だけ。gateway は login session（cookie）と **step-up**（web のパスワードの再入力。
  既定 600 秒で失効、`CELERIS_WEB_TERMINAL_STEP_UP_SECS`）の両方を要求する。パスワード認証の無い構成
  （`CELERIS_WEB_PASSWORD_FILE` 無し）では端末は使えない。
- daemon の `POST /api/v1/terminals`・`POST /api/v1/terminals/{id}/close` は主体が `human:web` のときだけ受け、
  それ以外（`human:<他>`・`operator:*`・`mcp:*`・`cos:*`・認証の無い構成の `human:local`）は 403。MCP の道具・
  CoS の操作の許可表には入れない（CoS ops の除外表に載せる）。worker・campaign の run は API token を持たない。
- WebSocket（`/terminal/ws`）は upgrade の時点で、Host が許可の一覧にあること、`Origin` が自分の origin
  （`http(s)://<Host>`）と完全一致すること、login session と step-up が有効なことを検査し、外れたら 403 で拒否する
  （browser Live View の guard と同じ作り）。別 origin のページからの接続（CSWSH）は Origin で落ちる。
- **入口の制限**: 端末を許す Host は loopback（`localhost`・`127.0.0.1`・`::1`）と `CELERIS_WEB_TERMINAL_HOSTS`
  （カンマ区切り。本番は tailscale の名前を入れる）だけ。LAN 入口 7721 は `systemd-socket-proxyd` が loopback へ
  中継するので接続元の address では区別できず、Host（`192.168.1.103:7721`）で区別する。7721 の Host は既定で
  許可に入らないので、平文の LAN から端末の入出力は流れない。LAN を許す設定は作らない（作るなら別の ADR で
  TLS を先に入れる）。Host は client が送る値だが、browser は偽れない。browser 以外の client が Host を偽っても、
  login session と step-up のパスワードが要り、平文で流すのはその client 自身の選択になる。
- 画面と本 ADR に明記する: **ローカルの端末は celeris を動かすユーザー（rmaeda）の権限（sudo 可）と同じ**。

### D4 対象の解決と記録

- `POST /api/v1/terminals` の body は `{"target": "local" | "cluster:<id>", "client": {...}}`。daemon が対象を解決して
  argv と cwd を返す。gateway は argv を自分で組まない。
  - `local`: daemon を動かすユーザーの login shell（passwd の shell、無ければ `/bin/sh`）を `-l` で、cwd は HOME。
  - `cluster:<id>`: daemon の snapshot でその cluster の ControlMaster が繋がっていなければ 409
    `cluster_not_connected`（画面は「接続」（ADR-0032）へ誘導する）。繋がっていれば
    `ssh -tt -o BatchMode=yes -o ControlMaster=no <host> -- 'cd <work_dir> && exec "${SHELL:-/bin/sh}" -l'`。
    `BatchMode=yes` で、master が直前に切れても対話認証（パスワード・TOTP）は出ずに失敗する。work_dir は
    `cluster_settings` の上書き → config の順、どちらも無ければ 409 `work_dir_unset`（`~` には入らない）。
    node 間 ssh は扱わない。
- 記録は追記専用の表 `terminal_session_events`（UPDATE・DELETE は trigger で拒否）。1 端末に `opened` と `closed`
  の 2 行。欄は session id、時刻、対象（`local` / `cluster:<id>`）、主体（`human:web`）、接続元（gateway が渡す
  Host と接続元 address）、終了理由（`user`・`disconnect`・`idle_timeout`・`max_duration`・`logout`・`exited`・
  `spawn_failed`・`gateway_shutdown`）、継続秒数、子の exit code。
- **端末の入出力は記録しない**。opt-in の記録も今回は作らない（作るなら、入力のエコー抑止中のバイトを落とす等の
  秘密の扱いを別 ADR で決める）。gateway・daemon・helper の log には入出力・cookie・パスワード・token を出さない
  （出すのは session id・対象・理由・秒数だけ）。

### D5 運用との関係

- 端末は Celeris の task の状態を変えない。DB を触る経路は D4 の監査記録だけで、task・events には書かない。
- 端末の中の `ssh -O exit <host>` は止めない（止める仕組みは作れない）。代わりに画面のクラスタ端末に
  「`ssh -O exit` は worker が使う接続も閉じる」と警告を出す。
- xterm.js（`@xterm/xterm` 6.0.0、`@xterm/addon-fit` 0.11.0）を版固定で SPA に同梱する。xterm は描画用の
  `<style>` 要素を作るので、SPA の document の CSP に `style-src-elem 'self' 'unsafe-inline'` を足す
  （`script-src` は `'self'` のまま。style 属性は従来どおり CSSOM からだけ変える）。WebSocket のサーバ側は
  `ws` 8.22.0 を版固定で使う。

## 試験（R6）

- `celerisctl terminal-pty` の試験（偽 shell = `/bin/sh` の固定スクリプト）: 入出力、リサイズで `stty size` が変わる、
  stdin の EOF で background job を含む session の process が消える、子の終了後に残った job が消える。
- gateway の試験（`node --test`、偽 helper、偽 daemon）: 開く・入出力・リサイズ frame・切断で helper が終わる・
  idle timeout・最大継続・同時数の上限・logout・step-up 無し / 失効・Origin 違反・LAN の Host・daemon の 403/409。
- daemon の試験: `human:web` 以外の主体は 403、未接続のクラスタは 409、記録の追記と UPDATE/DELETE の拒否。
- 本物の browser・ssh の e2e は release gate に任せる。

## 戻し方

`git revert` で戻す。表 `terminal_session_events` は migration で足しただけで、戻しても残す（データは消さない）。
運用で止めるだけなら `CELERIS_WEB_TERMINAL=off` を web.env に書いて web を再起動する。
