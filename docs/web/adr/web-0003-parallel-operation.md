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
