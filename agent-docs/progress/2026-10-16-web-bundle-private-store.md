---
title: release の web-bundle 段が落ちる: 原因は同じ sha の release.sh の並走（lock の誤判定）
tasks: [web-bundle-private-store]
status: done
completed: 2026-10-16
updated: 2026-10-16
---
# release の web-bundle 段が落ちる: 原因は同じ sha の release.sh の並走（lock の誤判定）

## 原因（証拠は ADR 2026-10-16-release-lock-bundled-scripts）
- delivery の prepare（`.deliveries/…/prepare.log`: 開始 04:08:33Z・05:32:33Z）と別起動の release.sh（journal の `systemd-run … release.sh`:
  04:12:01Z・05:35:20Z）が同じ sha で並走した。
- 2 本目は 1 本目の flock を「漏れた fd」と判定して退け、取り直した。痕跡: `.build/.lock-77ba8dd4689c.leaked-20261016T041203Z`、
  `.lock-release.leaked-20261016T041204Z`、`.build/.lock-99a7967d54c3.leaked-20261016T053522Z`、`.lock-release.leaked-20261016T053524Z`。
- 判定が外れた理由: 1 本目は `bash /local/celeris/state/current/scripts/release.sh`（同梱 scripts は平ら＝path に `selfdeploy/` が無い）で、
  `sd_lock_leaked_holders` は `*selfdeploy/*.sh*` だけを本物と数え、cmdline を 120 文字に切ってから判定していた。
- 並走した 2 本は同じ `<sha12>.partial` を `rm -rf` / `mkdir` / `tar` するので、04:17:20Z に `tar: dist: Cannot mkdir`（展開先の消失）、
  05:42:32Z に `ERR_PNPM_NO_LOCKFILE` と後始末の `rm: … Directory not empty`（prepare.log に原文）になった。
- 前回の「共有 store の丸写しが壊れた」は誤り（store の mtime との一致は偶然）。ADR 2026-10-16-web-bundle-private-store は副次の改善として残す。

## 変更
- `scripts/selfdeploy/lib.sh`: `sd_cmd_is_selfdeploy_script` を追加し、同梱 `…/scripts/{release,verify,promote,browser-ledger}.sh` も本物の持ち主と数える。判定は全文で行う。
- `scripts/selfdeploy/tests/release_lock_bundled_scripts.sh`: 決定的な再現と回帰固定（fifo 待ち、負荷なし）。
- `web/scripts/release.sh`: 共有 store の丸写しをやめ、配布物専用 store に `pnpm fetch --prod`（副次）。
- ADR 2 本（上記）。

## 検証
- 旧 lib.sh で試験: `FAIL: second release got rc=0, want 75`（`moved the leaked lock aside … taking a fresh lock`）。新 lib.sh: `ok`・rc 0。
- 一時 dir で `CELERIS_WEB_RELEASE=tmptest bash web/scripts/release.sh` → exit 0、tarball 7571 entry（旧 約 1.7 万）、展開 → `pnpm install --prod --offline --frozen-lockfile` →
  `import ok`（server/app.js）。release.sh（selfdeploy）そのものは worker から実行しない。

## 未解決・提案
- 並走を作った 2 つの起動元（delivery の prepare と、別途 systemd-run で起こす prep）が同じ sha を同時に起こす運用は残る。lock が直ったので 2 本目は
  exit 75（EX_TEMPFAIL）で諦めるだけになる。起動側で同じ sha の prepare が走っているか確認する仕組みは別 task の提案。
- 失敗した release の gate-logs（`.build/<sha12>/`）は次の release が `rm -rf "$FAILED_DIR"` で消す。回収する仕組みが無い。
