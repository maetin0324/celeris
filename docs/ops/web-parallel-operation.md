# web/ の並行運用: unit・selfdeploy の web 段・staging の確認手順と戻し方

---
tasks: [01M3TG9K4VV5Z4GZ0WY4DCVBZS, 01M3W79QE2ZD22YW7P499PZKPP, 01M3YBGM64RYPEY9NZANF79A0M]
---

gui/（`:7700`）と web/（ADR-0081 の SPA + gateway）を**同じ celeris に対して並行に**動かす。
gui/ の unit・配信・昇格は変えない。設計は [web ADR-W3](../../agent-docs/web/adr/web-0003-parallel-operation.md)
（H2 port・H3 cookie・H7 gate は gui/ のまま・H10 staging は人）。release・verify・promote 全体の手順は
[selfdeploy.md](selfdeploy.md)。

## 1. 構成

| もの | 場所 | 要点 |
|---|---|---|
| unit | `deploy/systemd/celeris-web@.service` | `%i` = release の sha12。`~/.local/celeris/releases/<sha12>/web/app/` から `node server/index.js`。bind の既定は `127.0.0.1:7720`、API は `127.0.0.1:7710`。上書きは `~/.config/celeris/web.env` |
| install-units | `scripts/selfdeploy/install-units.sh` | `celeris-web@.service` も `~/.config/systemd/user/` に置く（置くだけ。enable は人） |
| release の web 段 | `scripts/selfdeploy/release.sh` | gui/ の gate の後ろに `web-pnpm-install` → `web-pnpm-typecheck` → `web-pnpm-test` → `web-pnpm-release`。**非 blocking**。通れば release に `web/<tarball>` と展開済み `web/app/`（`pnpm install --prod --offline` 済み） |
| gate.json / manifest.json | `releases/<sha12>/` | `web: {ok, blocking: false, failed_step, skipped, reason, pnpm, steps[], bundle}`。`ok` / `gate_ok` は gui/ の gate だけで決まる |
| verify の検査 4c | `scripts/selfdeploy/verify.sh` | `SD_VERIFY_WEB_HOOK` を指定したときだけ web/ の読み取り parity を足す（§2） |
| promote の追従 | `scripts/selfdeploy/web-follow.sh` | 昇格後、旧 release の web が動いていれば新 release の web へ移す（[selfdeploy.md §4e](selfdeploy.md#4e-web-の追従web-followsh)） |
| 試験 | `scripts/selfdeploy/tests/release_web_stage_nonblocking.sh`、`promote_web_follows_release.sh` | 偽の corepack/pnpm・systemctl。本番・staging に触れない |

web/ の pnpm は `corepack pnpm@<web/package.json の packageManager>` で固定（host の pnpm や gui/ の版とは独立）。
`SD_GATE_SKIP_WEB=1` で web 段を飛ばせる。`SD_WEB_PNPM` で呼び出しを差し替えられる（試験用）。

## 2. 初めて起動する・staging で確かめる

### 2.1 前提
- 対象 release `<sha12>` の `gate.json` の `web.ok` が `true` で、`web/app/server/index.js` と `web/app/node_modules/` があること
  （無ければ web 段が落ちている。`gate-logs/web-*.log` を見る）。
  ```sh
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["web"])' ~/.local/celeris/releases/<sha12>/gate.json
  ls ~/.local/celeris/releases/<sha12>/web/app/server/index.js ~/.local/celeris/releases/<sha12>/web/app/node_modules >/dev/null && echo ok
  ```
- unit を置く: `scripts/selfdeploy/install-units.sh`。
- web/ 用の鍵と password（H3: gui/ のものを流用しない。unit はこの 2 つのファイルを読む）:
  ```sh
  (umask 077; head -c 32 /dev/urandom | base64 >~/.config/celeris/web.session-secret)
  (umask 077; printf '%s\n' '<web 用の password>' >~/.config/celeris/web.password)
  ```

### 2.2 staging に対する自動検査（verify.sh の検査 4c）
`verify.sh` が起こす staging celeris（`:7711`）と GUI（`:7701`）が生きている間に、hook が release の web gateway を
loopback で起こし、読み取り専用の parity spec（`web/e2e/parity/real-staging-readonly.spec.ts`。主要 GET・gui/web 同時 health・
SPA の 6 画面）を流す。hook が起こした gateway は終了時に止まる。本番の `:7700/:7710` には接続しない。
```sh
SD_REPO="$PWD" SD_VERIFY_WEB_HOOK="$PWD/web/scripts/staging-readonly-parity.sh" \
  WEB_STAGING_LOG_DIR=<ログを置く絶対パス> scripts/selfdeploy/verify.sh <sha12>
```
結果は `verify.json` の `checks` の `4c web-parity`。指定したときは `verify.json.ok` の条件に入る。

### 2.3 unit で起動する
`~/.config/celeris/web.env` に bind と向け先を書く（無ければ unit の既定 = `127.0.0.1:7720`、API `:7710`）。
staging（`:7711`）に向けて手で確かめるときは `CELERIS_API_URL=http://127.0.0.1:7711` にする。
```sh
cat >~/.config/celeris/web.env <<'ENV'
CELERIS_WEB_BIND=127.0.0.1:7720
CELERIS_API_URL=http://127.0.0.1:7710
ENV
systemctl --user start celeris-web@<sha12>.service
systemctl --user status celeris-web@<sha12>.service --no-pager
curl -fsS http://127.0.0.1:7720/healthz    # name と release=<sha12> が返る
```
確かめる点:
1. `/healthz` が `release: <sha12>` を返す。
2. gui/ が引き続き 200（`curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7700/healthz`）。
3. ブラウザで `http://127.0.0.1:7720/login`（ssh の port forward 可）。web/ の password で login し、gui/ の session とは
   独立に動く（cookie 名が違うので、gui/ で logout しても web/ は切れない）。
4. `journalctl --user -u celeris-web@<sha12>.service -n 50` にエラーが無い。

以後の release の切り替えは `promote.sh` が `web-follow.sh` で行う（[selfdeploy.md §4e](selfdeploy.md#4e-web-の追従web-followsh)）。
web は必ず昇格した release の `web/app` から動かす。

### 2.4 公開の bind（H2）
loopback 以外の bind（`0.0.0.0:<port>` 等）・`CELERIS_WEB_ALLOWED_HOSTS` は人が決めてから `web.env` に書く。
loopback 以外に bind するときは `CELERIS_WEB_PASSWORD_FILE`（unit の既定 `~/.config/celeris/web.password`）が必須
（gateway が起動を拒む）。

## 3. 止める・戻す
```sh
systemctl --user disable --now celeris-web@<sha12>.service   # web/ の gateway を止めて無効化
rm -f ~/.config/celeris/web.env                               # 任意: 設定を消す
```
gui/ の unit・`:7700` の配信・celeris には影響しない。release.sh の web 段だけ止めたいときは `SD_GATE_SKIP_WEB=1` で
release.sh を回す（web 段が無い release も gui/ の昇格には影響しない）。
