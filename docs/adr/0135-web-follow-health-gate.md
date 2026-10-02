# ADR-0135: web release の依存保証・web-follow の起動確認・LAN 中継の start-limit 対策

---
tasks: [01M3YT4PT3EP8A38111BXH1DCF]
---

- 日付: 2026-10-02
- 状態: Accepted（方式の決定。実装は後続 WorkUnit: release-deps / follow-health / lan-units / verify-web）
- 関係: ADR-0081 付記 2026-10-02 (C)(D)、web ADR-W3（`docs/web/adr/web-0003-parallel-operation.md`）D3
- 対象: `scripts/selfdeploy/release.sh`（`bundle_web`）、`scripts/selfdeploy/web-follow.sh`、`scripts/selfdeploy/lib.sh`、`scripts/selfdeploy/verify.sh`、`deploy/systemd/celeris-web-lan.{socket,service}`

## 事故の記録

- 2026-10-02 17:19 UTC、release `ae780a918695` を live で昇格した。`promote.sh` の web-follow が `celeris-web@95ac16442f92` を止めて `celeris-web@ae780a918695` に切り替えた。
- 新 unit の `node server/index.js` は `ERR_MODULE_NOT_FOUND` で起動できず、Web UI（`127.0.0.1:7720` と LAN `192.168.1.103:7721`）が止まった。運用者が `celeris-web@95ac16442f92` に戻して復旧した。
- 観測: `~/.local/celeris/releases/ae780a918695/web/app` には `server/`・`dist/` 等があるが `node_modules` が無い。tarball `web/celeris-web-0.1.0-ae780a918695.tar.gz` の中身は `.pnpm-store` 11614 件だけで `node_modules` を含まない。前の release `95ac16442f92` の `web/app` には `node_modules`（210M）がある。
- `gate.json` の `web` は `ok=true`・`blocking=false` だった。web-follow の切替条件（`web.ok=true` かつ `web/app/server/index.js` がある）を満たしたため切り替えた。条件は「ファイルがある」ことしか見ておらず、「起動できる」ことを見ていなかった。
- web unit が再起動するたびに、LAN の socket 起動の中継 `celeris-web-lan.service` が `start-limit-hit` で止まった。運用者が `reset-failed` と socket の再起動をしていた。LAN の 2 unit は人が手で `~/.config/systemd/user` に置いたもので、リポジトリに無い。

## D1: release の web 段は実行依存を確かめ、無ければ `web.ok=false`

`release.sh` の `bundle_web` は、tarball 展開と `pnpm install --prod --offline --frozen-lockfile` の後に次を確かめる。どれかが失敗したら `WEB_OK=false`、`WEB_FAILED_STEP=web-bundle` にし、理由を `.gate-web-bundle.log` とログに残す（従来どおり非 blocking）。

1. `web/app/node_modules/` が実在のディレクトリとして解決できる（symlink でもよい）。
2. `web/app/server/index.js` の import が解決できる。`node` で `server/` の依存を実際に読み込む確認をする。サーバを listen させない形で行う（例: `node --input-type=module -e "await import('express')"` を `server/` の import 先ごとに、または listen しない import 専用の入口）。外部ネットワークには出ない。

依存の置き方は次のどちらでもよい。実装 WU が選ぶ。

- (a) 現状どおり `web/app` 内で offline install し、その結果を上の 2 点で確かめる。
- (b) gui と同じ方式。`releases/.pnpm-prod-cache/<key>/` で 1 度だけ prod install し、`web/app/node_modules` をそこへの相対 symlink にする。key は web の lockfile と pnpm 版から作る。掃除（`SD_RELEASE_PRUNE`）は gui と同じく、どの release からも指されない key を消す。

どちらを選んでも、上の 2 点の確認は必須とする。確認が通らない release は `web.ok=false` となり、D2 より前の段で web-follow から外れる。

### 根因の仮説（未確定）

- `web/scripts/release.sh` は stage で一度 offline install した後に `rm -rf "$STAGE/node_modules"` している。tarball に `node_modules` を入れないのは設計どおり。展開先での再 install が前提になっている。
- `bundle_web` の offline install が exit 0 で終わったのに `node_modules` を作らなかった理由の候補:
  1. pnpm 12（`packageManager: pnpm@12.6.0`）の既定が global virtual store 等に変わり、依存を `web/app` の外（ユーザ共通の store や virtual store）に置いた。または、`node_modules` の作成を省いた。
  2. `cp -a "$STORE" "$STAGE/.pnpm-store/"` は store を `.pnpm-store/<store 版名>/` として写す。この配置が pnpm 12 の `storeDir: .pnpm-store` の期待と合わず、「入れるものが無い」と判断された。
  3. `SD_WEB_PNPM` または `corepack` で起動された pnpm の版や作業ディレクトリが想定と違い、別の場所に install した（`.pnpm-store` 件数が多いのに `node_modules` が無いことと合う）。
- 確定には `ae780a918695` の build 木の `.gate-web-bundle.log` を読み、同じ tarball で offline install を再現する必要がある。D1 の確認は根因が何であっても壊れた release を通さない。

## D2: web-follow は新 app の起動を確かめてから切り替え、失敗時は旧 web を残す

共通関数として `lib.sh` に `sd_web_app_probe <app_dir> <port> <sha12>` を置く（成功で 0）。

- `app_dir` の `node server/index.js` を、`CELERIS_WEB_BIND=127.0.0.1:<port>` と `CELERIS_WEB_RELEASE=<sha12>` で一時起動する。他の env は本番 unit と同じ値の読み方（`~/.config/celeris/web.env` 等）に合わせる。
- `/healthz` が HTTP 200 で、本文の `release` が `<sha12>` と一致するまで待つ。上限は異常時の保険として長めにとる。上限で失敗したら、起動ログの末尾を出して非 0 を返す。
- 終わったら一時プロセスを必ず止める（trap）。port が使用中なら probe 失敗とする。他の port に勝手に移らない。

`web-follow.sh` の手順:

1. 既存の条件（旧 unit が active、`gate.json` の `web.ok=true`、`web/app/server/index.js` がある）に加え、`web/app/node_modules` があることを確かめる。
2. **切替前**: `sd_web_app_probe "$rel/web/app" "${SD_WEB_PROBE_PORT:-7729}" "$NEW"`（既定 `127.0.0.1:7729`）。失敗なら旧 unit に触れず warning を出して exit 0。
3. 切替: 従来どおり新を start、旧を stop。
4. **切替後**: 本番 bind（unit の `CELERIS_WEB_BIND`。`web.env` の上書きを含む）の `/healthz` が 200 かつ `release=<NEW>` かを確かめる。失敗なら新を stop・disable し、旧を start・enable し直して warning で exit 0。
5. どの場合も exit 0（昇格を失敗にしない。ADR-0081 付記 (C) の非 blocking を保つ）。celeris@ の unit には触れない。

試験は `scripts/selfdeploy/tests` に置く。`systemctl` と `node` を偽物に差し替え、外部ネットワークには出ない。少なくとも次を確かめる。`node_modules` の無い web app では切り替えない。probe が失敗したら旧 web を止めない。切替後の確認が失敗したら旧に戻す。

## D3: LAN 中継 unit をリポジトリに置き、start-limit-hit を防ぐ

- `deploy/systemd/celeris-web-lan.socket` と `celeris-web-lan.service` を置く。socket は `192.168.1.103:7721` を listen する（公開先 address は人の環境なので、unit の既定値かコメントで示し、人が置き換えられるようにする）。service は `systemd-socket-proxyd 127.0.0.1:7720` で loopback gateway へ中継する。
- service の `[Unit]` に `StartLimitIntervalSec=0` を書き、start limit を無効にする。web の再起動中に接続が来て中継が連続で失敗しても、unit が `start-limit-hit` で止まらない。
- `web-follow.sh` は最後（切替の成否によらず unit を触った場合）に `systemctl --user reset-failed celeris-web-lan.service celeris-web-lan.socket` を実行し、`systemctl --user start celeris-web-lan.socket` を実行する。unit が無い環境では warning だけで続ける。
- unit の install（`install-units.sh` で置く、または人が置く）と `systemctl --user daemon-reload` は人が実行する（本番 host の操作。ADR-0095 付記 D-d）。手順は記録の WU が `docs/PROGRESS.md` に書く。

## D4: verify.sh に web app の起動確認を非 blocking で足す

- `verify.sh` は、release の `gate.json` が `web.ok=true` のとき、staging の空き port で `sd_web_app_probe` を実行する。結果を `record` で残す（非 blocking。verify の exit code は変えない）。`web.ok=false` または `web/app` が無いときは skip と理由を記録する。
- web ADR-W3 D3 の「verify.sh は web gateway を起動しない」は、この一時起動の確認に限って上書きする。本番 unit は起動しない。

## 影響と後戻り

- 変更は selfdeploy の shell と systemd unit に閉じる。daemon・DB・API は変えない。
- D2 の probe は昇格のたびに node を 1 回一時起動する。数秒の遅れはあるが、昇格は失敗しない。
- 後戻りは低コスト（関数と条件を外すだけ）。LAN unit を置いた後の撤去は人が行う。
