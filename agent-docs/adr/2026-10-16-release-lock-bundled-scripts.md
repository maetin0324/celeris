# ADR 2026-10-16: 同梱 script の release.sh が持つ lock を「漏れ」と誤判定しない（web-bundle 段が落ちた原因）

- 日付: 2026-10-16
- 状態: 採用・実装済み
- 関連: ADR-0041 D2（直列化）、ADR-0040 D6（scripts の同梱）、2026-10-16-web-bundle-private-store

## 背景
2026-10-16、release.sh の web-bundle 段が 3 回中 2 回落ちた。04:17Z（77ba8dd4）は
`tar: dist: Cannot mkdir: No such file or directory`・`index.db: Cannot open`・`Skipping to next header`、
05:42Z（99a7967d）は offline prod install が `ERR_PNPM_NO_LOCKFILE`（その後始末の
`rm -rf $STAGE` も `.pnpm-store/v11/files: Directory not empty` で止まった）。再実行では通った。

## 原因（証拠）
**同じ sha の release.sh が 2 本、同時に走っていた。** 2 本目は 1 本目の flock を「漏れた fd」と誤判定して退け、
自分の lock を取り直したので直列化が効かなかった。

1. 2 本の起動元が違う。
   - 1 本目: delivery の `start_prepare` → `current/scripts/prepare.sh` → `bash <current>/scripts/release.sh <sha>`
     （`.deliveries/<task>/<sha>/prepare.log`。開始 04:08:33Z / 05:32:33Z）。
   - 2 本目: 別の起動（`systemd-run … bash …/scripts/selfdeploy/release.sh <sha> > scratch/prep-<sha12>/release.log`。
     開始 04:12:01Z / 05:35:20Z。journal の `Started run-r…service`）。
2. 退けられた lock の痕跡が 2 本目の開始直後に残っている（`/local/celeris/state/releases`）:
   - `.build/.lock-77ba8dd4689c.leaked-20261016T041203Z`（mtime 04:08:33 = 1 本目が作った inode）、新しい `.lock-77ba8dd4689c` は 04:12:03。
   - `.lock-release.leaked-20261016T041204Z`（04:12:04）。
   - 05:xx も同じ形: `.build/.lock-99a7967d54c3.leaked-20261016T053522Z`、`.lock-release.leaked-20261016T053524Z`。
   `sd_lock_or_tempfail`（lib.sh）は flock が取れないと `sd_lock_leaked_holders` を呼び、持ち主の中に
   cmdline が `*selfdeploy/*.sh*` に合う process が無ければ lock を `.leaked-<時刻>` へ退けて取り直す。
3. 1 本目の cmdline は `bash /local/celeris/state/current/scripts/release.sh <sha>`。release.sh は梱包時に scripts を
   **平らに** `<release>/scripts/` へ写す（release.sh の `cp -p "$sh" "$STAGE/scripts/…"`）ので、path に `selfdeploy/` が無い。
   さらに `timeout … bash <path>` は 120 文字で切って判定していた。よって実行中の本物の release.sh が
   「selfdeploy の script ではない持ち主」と判定され、lock が退けられた（ADR-0041 の想定外）。
4. 2 本とも `STAGE="$REL.partial"`（`<sha12>.partial`）を固定名で使う。時刻は次のとおり重なりと整合する（2 本目の release.log は
   再実行で上書きされて残っていないので、2 本目側の時刻は開始時刻と段の所要からの推定）:
   - 04:17:20Z、1 本目の `web-pnpm-release` が exit 0 になった直後に `web-bundle: tar failed`（prepare.log）。2 本目は開始から
     約 5 分（04:12:01 + test/clippy/build、1 本目は 04:08:53 → 04:16:52）で `rm -rf "$STAGE"; mkdir -p "$STAGE/bin"` に達する頃。
     1 本目の展開先 `$STAGE/web/app` が消えたので `tar -C` の `dist` の mkdir が ENOENT、以降の `index.db` も開けず
     `Skipping to next header`。
   - 05:42:32Z、1 本目の後始末 `rm -rf "$STAGE"` が `…/web/app/.pnpm-store/v11/files: Directory not empty`（2 本目が同じ path へ
     展開中）。`ERR_PNPM_NO_LOCKFILE` は 2 本目の `rm -rf "$dir"` が 1 本目の `cd "$dir"` の中身を消した結果。
   - 3 回目（04:17:40 / 05:43:10 の再実行、prep 側）は先の 1 本が終わっていて 1 本だけで、通った。
5. 決定的な再現（`scripts/selfdeploy/tests/release_lock_bundled_scripts.sh`、fifo 待ちで CPU を使わない）: 平らな
   `<release>/scripts/release.sh` が lock を持つ間に 2 本目が `sd_lock_or_tempfail` を呼ぶと、修正前は
   `moved the leaked lock aside … taking a fresh lock` で rc=0（= 並走）、修正後は rc=75。

「共有 pnpm store の丸写しが壊れた」では 2 件の症状（展開先 dir の消失、lockfile の消失、`rm` の Directory not empty）を
説明できない。store は別の副次要因（下記）として扱う。

## 決定
- D1: `sd_cmd_is_selfdeploy_script`（lib.sh）を追加し、`sd_lock_leaked_holders` はそれで本物の持ち主を判定する。
  `…/selfdeploy/*.sh` に加え、同梱された平らな `…/scripts/{release,verify,promote,browser-ledger}.sh` も本物。
  判定は 120 文字に切る前の cmdline 全文で行う（表示だけ切る）。旧い判定（`*selfdeploy/*.sh*` のみ・切った後で判定）は消した。
- D2: lock を持つ script ではない持ち主（漏れた fd）の lock を退ける従来の挙動は変えない（2026-09-21 の `podman info` 事故への対策）。
- D3: 試験 `scripts/selfdeploy/tests/release_lock_bundled_scripts.sh` で上の 2 つを固定する。

## 移行
config 欄の削除なし。本番への移行は、次の release 以降の `current/scripts/lib.sh` が新しくなること（通常の昇格）だけ。
