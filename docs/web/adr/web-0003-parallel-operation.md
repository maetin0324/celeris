# web ADR-W3: web/ の並行運用（port・cookie・selfdeploy の非 blocking 段・staging 確認）

---
tasks: [01M3TG9K4VV5Z4GZ0WY4DCVBZS]
---

- Date: 2026-10-01
- Status: Accepted（H2・H3・H7・H10 は「決まるまでの扱い」。人の決定が出たら追記で上書きする）
- 関連: ADR-0081（web SPA と gateway）、ADR-0040 D1/D2（selfdeploy のリリース gate）、ADR-0041（gate.json・changes.json）、
  docs/web/implementation-plan.md §3（H2・H3・H7・H10）と §10 P6-02

## 文脈

web/（ADR-0081 の SPA と薄い gateway）は Phase 5 の gate（遅延・security・mobile/a11y・parity 総点検）を通り、
P6-01 で配布物（`pnpm -C web release`）ができた。次は gui/ と**同じ celeris に対して並行に**動かし、
dogfood（P6-03）と配信切替（P6-04）の材料を作る段である。ここで 4 つの点が人の判断待ちのまま残っている
（implementation-plan.md §3）。P6-02 はそれらを「決まるまでの扱い」で進め、人に新たに聞かない。

## 決定

### D1（H2）port — 本番の port は決めない。開発・検査は `127.0.0.1:7720`
- web gateway の unit `deploy/systemd/celeris-web@.service` の bind の既定は `127.0.0.1:7720`（loopback）。
  本番向けの bind・許可 Host・password/session secret の場所は `~/.config/celeris/web.env`
  （`EnvironmentFile=-`）で与える。**既定値を本番の port や `0.0.0.0` に固定しない**。
- celeris-gui@.service と `:7700` の配信は変えない。staging の verify.sh が使う `:7701/:7711/:7712` も変えない。

### D2（H3）認証・session — web/ は自分の cookie 名と署名鍵を持ち、gui/ の cookie を読まない
- web gateway は `CELERIS_WEB_SESSION_SECRET_FILE` / `CELERIS_WEB_PASSWORD_FILE` を使う（既定は `~/.config/celeris/web.session-secret` /
  `web.password`）。gui/ の `gui.session-secret` / `gui.password` を指さない。cookie 名も web/ 固有（P1-05 の実装どおり）。
- 利用者は gui/ と web/ に別々に login する。並行運用中に session を共有する仕組みは作らない。

### D3（H7）release と selfdeploy — gate は gui/ のまま。web/ の段は非 blocking
- `scripts/selfdeploy/release.sh` の gate（cargo fmt/test/clippy/build と gui/ の pnpm の段）は変えない。
  その**後ろ**に web/ の段（`web-pnpm-install` → `web-pnpm-typecheck` → `web-pnpm-test` → `web-pnpm-release`）を足す。
- web/ の段が落ちても `GATE_OK` は倒さず、リリース（`releases/<sha12>/`）は作られ、昇格（promote.sh）は gui/ だけの
  リリースとして進む。落ちた段と exit code は gate.json の `steps[]` と `web`（`ok` / `failed_step` / `blocking: false`）に残す。
  落ちた段より後ろの web/ の段は `skipped: true` と理由を記録する。
- web/ の段が通ったときだけ、配布物（tar.gz）をリリースの `web/` に置き、展開して `pnpm install --prod --offline` した
  `web/app/` を作る。manifest.json の `web` に結果を書く。`gate_ok` は従来どおり gui/ の gate だけを表す。
- ビルドする sha に `web/` が無ければ web/ の段は `skipped`（理由 `no web/ directory`）。`SD_GATE_SKIP_WEB=1` でも飛ばせる。
- web/ の pnpm は `corepack pnpm@<web/package.json の packageManager の版>` で固定して呼ぶ（host の pnpm と gui/ の版に依存しない）。
- verify.sh（staging）に web/ の段は足さない（H10 の人の確認が先）。promote.sh は web/ の unit を起こさない。

### D4（H10）staging の実 celeris 確認は人に依頼する
- 手順は docs/web/parallel-operation.md に書く（unit の設置・`web.env`・起動・`web/e2e/parity/` の読み取りだけの e2e・戻し方）。
  ワーカーの run は staging（`:7701/:7711/:7712`）と本番に接続せず、verify.sh を実機で走らせない。
- 戻し方: `systemctl --user disable --now celeris-web@<sha12>.service`、commit の revert。gui/ の配信は変わらない。

## 影響
- gate.json の読み手（`/releases` の画面）は `web` と `blocking: false` を無視してよい（追加の欄）。
- selfdeploy のテスト `scripts/selfdeploy/tests/release_web_stage_nonblocking.sh` が「web/ の段が落ちても gui の昇格が進む」ことを
  偽の corepack/pnpm で確かめる（本番のパス・ネットワークに触れない）。
- docs/selfdeploy.md の更新は P6-04（配信切替）の範囲。

## 付記（2026-10-02）: gateway の dotfiles と web の release 追従

本付記は ADR-0081（web SPA と gateway）と web ADR-W3（`docs/web/adr/web-0003-parallel-operation.md`、unit や docs で ADR-0096 と呼ばれている文書）の両方に同じ内容で置く。

### (A) 事象と原因
- 本番 release `ea86af6307f8` の web を `~/.local/celeris/releases/<sha12>/web/app`（`celeris-web@<sha12>` の WorkingDirectory）から起動すると、`/healthz` は ok だが `/`・`/login`・`/inbox` など全画面が `not found`（404）になった。同じ release を `/var/lib/celeris/web/<sha12>/app` にコピーして起動すると 200 だった。
- 原因: `web/server/app.js` の `res.sendFile(path.join(distDir, "index.html"))` と `express.static(path.join(distDir, "assets"))` は、send の既定 `dotfiles: "ignore"` で動く。root を渡さない `sendFile` は**絶対 path 全体**を dotfiles 判定にかけるため、途中の `.local` を隠しファイルと見なして 404 を返す。開発・試験の path にドットの dir が無かったため表に出なかった。

### (B) 決定: root を渡し、dotfiles は既定のまま
- SPA の HTML は `res.sendFile("index.html", { root: distDir })` で送る（相対名 + root）。
- `express.static` は assets の dir を root にし、`dotfiles` は既定（`ignore`）のまま。`dotfiles: "allow"` は使わない。
- send は root より上の path を dotfiles 判定に入れないので、配置先の path（`.local` 等を含んでも）に依らず動く。dist の中のドットファイル（`.env` 等）は引き続き配信しない。
- 試験: `.local` を含む一時 dir に release 相当の dist を置いて gateway を起動し、`/` と SPA の path（例 `/inbox`）が 200、dist 内の `.env` 等のドットファイルが 404 になることを確かめる。

### (C) web の release 追従
- 事象: 前日（2026-10-01）の dogfood の web は、task の作業場所（staging 成果物）への symlink を持つ release から起動されていた。作業場所の片付けで中身が消え、全画面 404 になった。
- 決定: `scripts/selfdeploy/promote.sh` は昇格に成功した**後**に `scripts/selfdeploy/web-follow.sh <new_sha12> <old_sha12>` を呼ぶ。
- `web-follow.sh` は `celeris-web@<old_sha12>` が active のときだけ動く。新 release の `gate.json` の `web.ok=true` と `web/app/server/index.js` の存在を確かめてから、`celeris-web@<new_sha12>` を起動・有効化し、`celeris-web@<old_sha12>` を停止・無効化する。
- 条件を満たさないとき（旧 web が動いていない・新 release の web 段が不合格・配布物が無い）は何もせず、理由をログに出す。web の追従の失敗で昇格を失敗にしない（D3/H7 の非 blocking を保つ）。

### (D) unit は daemon を起こさない
- `deploy/systemd/celeris-web@.service` から `Wants=celeris@%i.service` を外し、`After=` だけを残す。web の起動が `celeris@<sha12>` を起こして daemon の handoff を引き起こさないため（2026-10-01 の事故）。

### (E) 本番 host の操作は人が行う
- 一時回避の `~/.config/systemd/user/celeris-web@ea86af6307f8.service.d/override.conf` の撤去、`systemctl --user daemon-reload`、web の再起動は、人が `docs/selfdeploy.md` の手順で行う。worker の run は本番 host を操作しない。
- web ADR-W3 D3 の「promote.sh は web/ の unit を起こさない」という既存の記述は、本付記 (C) で上書きする（promote.sh は旧 web が動いているときに限り web を新 release へ追従させる）。
