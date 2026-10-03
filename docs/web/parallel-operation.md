# web/ の並行運用: unit・selfdeploy の web 段・staging の確認手順と戻し方

---
tasks: [01M3TG9K4VV5Z4GZ0WY4DCVBZS, 01M3W79QE2ZD22YW7P499PZKPP, 01M3YT4PT3EP8A38111BXH1DCF]
---

P6-02（web ADR-W3）。gui/（`:7700`）と web/（ADR-0081 の SPA + gateway）を**同じ celeris に対して並行に**動かす。
gui/ の unit・配信・昇格は変えない。人の判断待ち（H2・H3・H7・H10）は「決まるまでの扱い」で進めている
（implementation-plan.md §3、web ADR-W3）。

## 1. 何が入ったか

| もの | 場所 | 要点 |
|---|---|---|
| ADR | [web ADR-W3](adr/web-0003-parallel-operation.md) | H2（port）・H3（cookie）・H7（gate は gui/ のまま）・H10（staging は人） |
| unit | `deploy/systemd/celeris-web@.service` | `%i` = release の sha12。`~/.local/celeris/releases/<sha12>/web/app/` から `node server/index.js`。bind の既定は `127.0.0.1:7720`。上書きは `~/.config/celeris/web.env` |
| LAN 中継 | `deploy/systemd/celeris-web-lan.socket`・`celeris-web-lan.service` | 現行 host の `192.168.1.103:7721` を gateway の `127.0.0.1:7720` へ中継。別の host では socket の `ListenStream` を設置前に変更する。service の start limit と socket の trigger limit は無効 |
| install-units | `scripts/selfdeploy/install-units.sh` | web gateway と LAN 中継の 3 unit を `~/.config/systemd/user/` に置いて daemon-reload する。enable・start はしない |
| release の web 段 | `scripts/selfdeploy/release.sh` | gui/ の gate の後ろに `web-pnpm-install` → `web-pnpm-typecheck` → `web-pnpm-test` → `web-pnpm-release`。**非 blocking**。通れば release の `web/<tarball>` と展開済み `web/app/`（`pnpm install --prod --offline` 済み） |
| gate.json / manifest.json | `releases/<sha12>/` | `web: {ok, blocking: false, failed_step, skipped, reason, pnpm, steps[], bundle}`。`ok` / `gate_ok` は従来どおり gui/ の gate だけ |
| テスト | `scripts/selfdeploy/tests/release_web_stage_nonblocking.sh` | web 段が落ちても release が作られ `gate_ok: true`（偽の corepack/pnpm、本番・staging に触れない） |

web/ の pnpm は `corepack pnpm@<web/package.json の packageManager>`（いまは 12.6.0）で固定。host の pnpm や gui/ の 11.27.0 とは独立。
`SD_GATE_SKIP_WEB=1` で web 段を飛ばせる。`SD_WEB_PNPM` で呼び出しを差し替えられる（テスト用）。
verify.sh（staging の自動検査）は `SD_VERIFY_WEB_HOOK` を指定した場合だけ web/ の読み取り parity を追加する。promote.sh（昇格）は昇格の後に `web-follow.sh` を呼び、旧 release の web が動いていれば新 release の web へ移す（下の §1.1）。

### 1.1 promote 時の web の追従（2026-10-02 付記 (C)(D)）

- `scripts/selfdeploy/promote.sh` は celeris と gui の切替の後に `scripts/selfdeploy/web-follow.sh <new_sha12> <old_sha12>` を呼ぶ。
  `celeris-web@<old>` が active で、新 release の `gate.json` の `web.ok=true` かつ `web/app/server/index.js` があるときだけ、
  `celeris-web@<new>` を start/enable して旧を stop/disable する。条件を満たさなければ何もしない。失敗しても昇格は失敗にしない。
- `celeris-web@.service` は `Wants=celeris@%i.service` を持たない。web の起動で daemon（`celeris@`）を起こさない。web-follow.sh も `celeris@` に触れない。
- 試験: `bash scripts/selfdeploy/tests/promote_web_follows_release.sh`（偽の systemctl と一時 dir の releases。本番に触れない）。
- 本番の一時回避（`celeris-web@ea86af6307f8.service.d/override.conf`）の撤去・unit の置き直し・web の再起動・`curl` で `/` が 200 の確認は、
  人が [docs/selfdeploy.md §4e](../selfdeploy.md#4e-web-の追従web-followsh) の手順で行う。

## 2. staging での実 celeris 確認（H10）

P6-02 の残りの task は、`verify.sh` が起こす staging celeris と gui/ が生きている間に
release の web gateway を loopback で起こし、読み取り専用の parity spec を流す。
本番の `:7700/:7710` には接続せず、gui/ の配信には触れない。

### 2.1 前提
- 対象 release `<sha12>` が `~/.local/celeris/releases/<sha12>/` にあり、`gate.json` の `web.ok` が `true`、
  `web/app/server/index.js` と `web/app/node_modules/` があること（無ければ web 段が落ちている。`gate-logs/web-*.log` を見る）。
  ```sh
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["web"])' ~/.local/celeris/releases/<sha12>/gate.json
  ls ~/.local/celeris/releases/<sha12>/web/app/server/index.js ~/.local/celeris/releases/<sha12>/web/app/node_modules >/dev/null && echo ok
  ```
- unit を置く（置くだけ。enable・start しない）: `scripts/selfdeploy/install-units.sh`。この操作は人が本番 host で行う。LAN 中継を使う host では、その前に socket の `ListenStream` が host の LAN address と一致するか確認する。
- web/ 用の鍵と password（H3: gui/ のものを流用しない）:
  ```sh
  (umask 077; head -c 32 /dev/urandom | base64 >~/.config/celeris/web.session-secret)
  (umask 077; printf '%s\n' '<web 用の password>' >~/.config/celeris/web.password)
  ```

### 2.2 staging の celeris に向けて起動（既定 `127.0.0.1:7720`）
staging の celeris（verify.sh が使う `127.0.0.1:7711`）に向ける。
`~/.config/celeris/web.env` に書く（無ければ unit の既定 = loopback 7720、`:7710`）:
```sh
cat >~/.config/celeris/web.env <<'ENV'
CELERIS_WEB_BIND=127.0.0.1:7720
CELERIS_API_URL=http://127.0.0.1:7711
ENV
systemctl --user start celeris-web@<sha12>.service
systemctl --user status celeris-web@<sha12>.service --no-pager
curl -fsS http://127.0.0.1:7720/healthz    # name と release=<sha12> が返る
```
確認する点（記録は docs/PROGRESS.md の Phase 6 節か、task の成果物 `web-phase56-handoff.md` へ）:
1. `/healthz` が `release: <sha12>` を返す。
2. gui/（`http://<host>:7700`）が引き続き 200（`curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7700/healthz`）。
3. ブラウザで `http://127.0.0.1:7720/login`（ssh の port forward 可）。web/ の password で login し、gui/ の session とは独立に動く
   （gui/ で logout しても web/ は切れない。cookie 名が違う）。
4. 読み取りだけの parity e2e を実 celeris に対して流す（書き込みの spec は流さない）。
   この task の自動検査は `verify.sh` の optional hook で行う。hook が作った gateway は終了時に止まる:
   ```sh
   SD_REPO="$PWD" SD_VERIFY_WEB_HOOK="$PWD/web/scripts/staging-readonly-parity.sh" \
     WEB_STAGING_LOG_DIR=<run の絶対 artifacts path> scripts/selfdeploy/verify.sh <sha12>
   ```
   `web/e2e/parity/real-staging-readonly.spec.ts` の 3 件だけを流し、実 celeris の主要 GET、
   gui/web 同時 health、SPA の 6 画面を確認する。Playwright の設定は `WEB_E2E_REAL_BASE_URL` のとき既存 server を使う。
5. `journalctl --user -u celeris-web@<sha12>.service -n 50` にエラーが無い。

### 2.3 本番の port（H2）は決めない
本番向けの bind（`0.0.0.0:<port>`）・`CELERIS_WEB_ALLOWED_HOSTS` は人が決めてから `web.env` に書く。
P6-02 では unit の既定を loopback 7720 に固定しており、enable しない限り起動しない。

## 3. 戻し方（rollback）
```sh
systemctl --user disable --now celeris-web@<sha12>.service   # web/ の gateway を止めて無効化
rm -f ~/.config/celeris/web.env                               # 任意: 設定を消す
git revert <P6-02 の commit>                                  # unit・release.sh の web 段・ADR を戻す
```
gui/ の unit・`:7700` の配信・celeris には触れていないので、配信は変わらない。release.sh の web 段を戻す前に作られた release は
`web/` ディレクトリを持つだけで、gui/ の昇格には影響しない。緊急に web 段だけ止めたいときは revert せずに `SD_GATE_SKIP_WEB=1` で
release.sh を回す。

## 4. 検証（この葉で実行したもの）
- `for t in scripts/selfdeploy/tests/*.sh; do bash "$t"; done` — 7 本すべて ok（新テスト `release_web_stage_nonblocking.sh` を含む）。
  `release_gui_skip_and_shared_tree.sh` と `release_parallel_test_gate.sh` は、偽の cargo が `celeris-credentiald` を作らず
  base でも落ちていた（browser phase 2 の commit で release.sh が同梱するようになった分）。偽の cargo に 1 語足して直した。
- `git diff --quiet $(git merge-base HEAD main) -- gui crates docs/api` — 差分なし（V1）。
- P6-02 staging 実機確認（2026-10-01、release `6ef01deff025`）: `release.sh HEAD` exit 0、`gate.json` の `ok=true`・`web.ok=true`、`web/app/server/index.js` と prod `node_modules/` を確認。`verify.sh 6ef01deff025` exit 1（`verify.json.ok=false`、`live_ok=false`）。現行 schema 34 の celeris binary を同じ snapshot に起動し、gui `:7701` と web `http://127.0.0.1:7720` を同時起動。実 staging celeris `:7711` への `web/e2e/parity/real-staging-readonly.spec.ts` は 3 passed / exit 0。verify は DB schema 34 が release binary の上限 33 を超えて check 1 で失敗。この parity は release binary の検査を代替しない。本番 `:7700/:7710` には接続せず、web gateway は hook の終了時に停止。生ログ: `/var/lib/celeris/workspaces/01M3W79QAQ06PG1M0MCK5HRZ5K/artifacts/release-local-retry.log`、`verify.log`、`manual-parity.log`、`web-parity-e2e.log`、`web-gateway.log`。
- gate.json の web 節: `{"ok":true,"blocking":false,"failed_step":"","skipped":false,"reason":"","pnpm":"pnpm@12.6.0","steps":["web-pnpm-install","web-pnpm-typecheck","web-pnpm-test","web-pnpm-release"],"bundle":{"tarball":"web/celeris-web-0.1.0-6ef01deff025.tar.gz","app":"web/app"}}`。
- P6-02 staging 再検証（2026-10-01、release `bf54b41ad627`）: schema 34 の実 snapshot を読むため、この branch に現行 `main` を merge して schema 36 の HEAD を release。`release.sh HEAD` exit 0、`gate.json` の `ok=true`・`web.ok=true`、`web/app/server/index.js` と prod `node_modules/` を確認。web 節は `{"ok":true,"blocking":false,"failed_step":"","skipped":false,"reason":"","pnpm":"pnpm@12.6.0","steps":["web-pnpm-install","web-pnpm-typecheck","web-pnpm-test","web-pnpm-release"],"bundle":{"tarball":"web/celeris-web-0.1.0-bf54b41ad627.tar.gz","app":"web/app"}}`。
- `verify.sh bf54b41ad627` exit 0、`verify.json.ok=true`、gui `:7701` と web `127.0.0.1:7720` が同時に healthy。実 staging celeris `:7711` に対する読み取り専用 `web/e2e/parity/real-staging-readonly.spec.ts` は 3 passed / exit 0。verify の check 1〜4・4b・4c・6 は true。N-1 check 5 は旧 binary schema 34 が migration 後の schema 36 を読めず false（`live_ok=false`）。本番 `:7700/:7710` には接続せず、hook が gateway を停止。終了後 staging の 4 port に listener はない。生ログは run artifacts の `release-schema36-retry.log`、`verify-schema36-retry.log`、`web-parity-e2e.log`、`web-gateway.log`、`staging-state/staging/logs/e2e-staging.log`。詳細は [Phase 6 の記録](../progress/phase-web.md)。
