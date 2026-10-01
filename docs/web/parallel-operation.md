# web/ の並行運用: unit・selfdeploy の web 段・staging の確認手順と戻し方

---
tasks: [01M3TG9K4VV5Z4GZ0WY4DCVBZS]
---

P6-02（ADR-0096）。gui/（`:7700`）と web/（ADR-0081 の SPA + gateway）を**同じ celeris に対して並行に**動かす。
gui/ の unit・配信・昇格は変えない。人の判断待ち（H2・H3・H7・H10）は「決まるまでの扱い」で進めている
（implementation-plan.md §3、ADR-0096）。

## 1. 何が入ったか

| もの | 場所 | 要点 |
|---|---|---|
| ADR | `docs/adr/0096-web-parallel-operation.md` | H2（port）・H3（cookie）・H7（gate は gui/ のまま）・H10（staging は人） |
| unit | `deploy/systemd/celeris-web@.service` | `%i` = release の sha12。`~/.local/celeris/releases/<sha12>/web/app/` から `node server/index.js`。bind の既定は `127.0.0.1:7720`。上書きは `~/.config/celeris/web.env` |
| install-units | `scripts/selfdeploy/install-units.sh` | `celeris-web@.service` も `~/.config/systemd/user/` に置く（置くだけ。enable は人） |
| release の web 段 | `scripts/selfdeploy/release.sh` | gui/ の gate の後ろに `web-pnpm-install` → `web-pnpm-typecheck` → `web-pnpm-test` → `web-pnpm-release`。**非 blocking**。通れば release の `web/<tarball>` と展開済み `web/app/`（`pnpm install --prod --offline` 済み） |
| gate.json / manifest.json | `releases/<sha12>/` | `web: {ok, blocking: false, failed_step, skipped, reason, pnpm, steps[], bundle}`。`ok` / `gate_ok` は従来どおり gui/ の gate だけ |
| テスト | `scripts/selfdeploy/tests/release_web_stage_nonblocking.sh` | web 段が落ちても release が作られ `gate_ok: true`（偽の corepack/pnpm、本番・staging に触れない） |

web/ の pnpm は `corepack pnpm@<web/package.json の packageManager>`（いまは 12.6.0）で固定。host の pnpm や gui/ の 11.27.0 とは独立。
`SD_GATE_SKIP_WEB=1` で web 段を飛ばせる。`SD_WEB_PNPM` で呼び出しを差し替えられる（テスト用）。
verify.sh（staging の自動検査）と promote.sh（昇格）は web/ に触れない。

## 2. staging での実 celeris 確認（H10）— 人に依頼

ワーカーの run は staging（`:7701/:7711/:7712`）と本番に接続しない。以下は人が手元で行う。
所要は 10 分程度。gui/ の配信（`:7700`）には一切触れない。

### 2.1 前提
- 対象 release `<sha12>` が `~/.local/celeris/releases/<sha12>/` にあり、`gate.json` の `web.ok` が `true`、
  `web/app/server/index.js` と `web/app/node_modules/` があること（無ければ web 段が落ちている。`gate-logs/web-*.log` を見る）。
  ```sh
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["web"])' ~/.local/celeris/releases/<sha12>/gate.json
  ls ~/.local/celeris/releases/<sha12>/web/app/server/index.js ~/.local/celeris/releases/<sha12>/web/app/node_modules >/dev/null && echo ok
  ```
- unit を置く（置くだけ。enable しない）: `scripts/selfdeploy/install-units.sh`（または `cp deploy/systemd/celeris-web@.service ~/.config/systemd/user/ && systemctl --user daemon-reload`）。
- web/ 用の鍵と password（H3: gui/ のものを流用しない）:
  ```sh
  (umask 077; head -c 32 /dev/urandom | base64 >~/.config/celeris/web.session-secret)
  (umask 077; printf '%s\n' '<web 用の password>' >~/.config/celeris/web.password)
  ```

### 2.2 staging の celeris に向けて起動（既定 `127.0.0.1:7720`）
staging の celeris（verify.sh が使う `127.0.0.1:7711`。動いていなければ本番の `127.0.0.1:7710` を**読むだけ**の確認でもよい）に向ける。
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
4. 読み取りだけの parity e2e を実 celeris に対して流す（書き込みの spec は流さない）:
   ```sh
   corepack pnpm@12.6.0 -C web install --frozen-lockfile
   E2E_BASE_URL=http://127.0.0.1:7720 E2E_SKIP_FAKE_DAEMON=1 corepack pnpm@12.6.0 -C web e2e parity/ --grep-invert 'write|mutation|action'
   ```
   環境変数の名前は `web/e2e/` の設定（`playwright.config.ts`）に合わせる。e2e の harness が実 celeris を指せない場合は、
   ブラウザで `/`, `/tasks`, `/projects`, `/org`, `/releases`, `/clusters` を開いて 200 と一覧の描画を確かめる。
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
- staging（2.2）と verify.sh は**実行していない**（H10: 人に依頼）。
