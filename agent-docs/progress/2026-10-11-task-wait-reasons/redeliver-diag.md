# 配送差し戻し: web bundle の pnpm-store 削除失敗

tasks: [01M4NAKDCCS5XYAAJ518MVZ134]

## 診断

対象ログ `/local/celeris/state/releases/.deliveries/01M4NAKDCCS5XYAAJ518MVZ134/7a2432a207f2df358c05e667c47e8e2e1342370f/prepare.log` の最後は次のとおり。

```text
2026-10-11T17:40:35Z [release] step web-pnpm-release: exit 0 in 20.707s
0 skipped, 0 error(s)
rm: cannot remove '/local/celeris/state/releases/7a2432a207f2.partial/web/app/.pnpm-store/v11/files': Directory not empty
2026-10-11T17:40:57Z [release] ERROR: web bundle failed; no release directory created
```

`scripts/selfdeploy/release.sh` の bundle 処理は、tarball 展開先 `$STAGE/web/app` に対して `rm -rf "$dir"` を実行してから作り直す。これは pnpm の offline install 中に展開されたアプリ内 `.pnpm-store` も含む再帰削除で、失敗を再試行せず bundle 全体を失敗にする。prepare.log では cargo test・clippy・build、web config/install/typecheck/test/release が全て exit 0 であり、失敗はこの後の梱包時削除だけだった。

原因は `.pnpm-store/v11/files` を削除中にディレクトリ内容が変わった競合と判断する。`Directory not empty` は再帰削除の実行中にエントリが残ったことを示すが、ログだけでは並行 writer（pnpm/別プロセス）を特定できない。release script は毎回 `$STAGE` を先に削除し、bundle 内でも `$dir` を作り直すため、永続的な残留物だけが原因とは考えにくい。処理は失敗を正しく blocking にしている一方、削除を再試行しないので、今回の事象が一過性か継続的な競合かは一度のログだけでは断定できない。従って本 task のコードに原因はなく、修正対象は範囲外の release packaging 側である。

## 再検査

- `bash scripts/dev/test-parallel.sh`（run 既定 TMPDIR）: exit 130 で中断。browser/credentiald の unix socket 試験が `path must be shorter than SUN_LEN` / fixture readiness timeout になった。既知の長い TMPDIR 環境問題を再確認。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0。nextest は 5,198 passed / 0 failed / 13 skipped。doc tests は 3 passed / 0 failed、summary は 5,201 passed / 0 failed / 14 ignored。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `pnpm -C web typecheck && pnpm -C web test`: exit 0。typecheck (`tsc -b`) 成功、Vitest 95 files / 691 tests passed、Node server tests 93 passed / 0 failed。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: exit 0（開始時 snapshot からの変更 path なし）。
- `git diff --quiet "${CELERIS_WU_BASE:-HEAD}" -- crates/ web/ scripts/`: exit 0。コード差分なし。

## 提案

release script 側の別 task で、web bundle 前の stage cleanup に対する限定回数の retry と、削除失敗時に対象 tree の残存パス・並行 pnpm process の診断情報を gate log に残すことを検討する。さらに同一 SHA の release prepare を再実行し、同じ cleanup error が出るか記録する。scripts/ の変更は本 task では行わない。
