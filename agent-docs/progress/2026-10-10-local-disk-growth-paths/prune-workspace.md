---
tasks: [01M4HWZMWSTH3HYPCRRPE6PXB0]
status: done
completed: 2026-10-10
---

# release prune を全 workspace target に広げる（prune-workspace）

## 変更

- `scripts/selfdeploy/lib.sh` の `sd_release_prune_stale_test_binaries`: 候補名を `cargo metadata --no-deps --offline` の全 package・全 target 名と package 名の和にした（失敗時は member の `Cargo.toml` と `tests/*.rs`・`src/bin/*.rs`・`benches/*.rs`・`examples/*.rs` から作る）。`.d` が `/registry/src/` か `/git/checkouts/` を参照する組は依存 crate として残す。古い workspace lib の `lib<stem>.rlib`/`.rmeta` も刈る。`SD_RELEASE_PRUNE_DRY_RUN=1` で消さずに数と `st_blocks*512` の大きさを log に出す。
- `scripts/selfdeploy/tests/release_prune_stale_test_binaries.sh`: integration test（`notify-<hash>`）、crates 外 member（`tests/e2e` 相当の `api_scenarios-<hash>`）、registry・git checkout を参照する同名の依存 crate（残る）、marker より新しい binary（残る）、metadata だけが知る `[[test]] name`、dry run の fixture を追加。
- ADR `agent-docs/adr/2026-10-10-local-disk-growth-paths.md` に「付記 2026-10-10: release prune の対象」。

## 証拠

- `TMPDIR=/tmp bash scripts/selfdeploy/tests/release_prune_stale_test_binaries.sh` → exit 0（`release_prune_stale_test_binaries: ok`）。dry run 9 file 報告・削除 0、fallback で 9 file 削除・依存 2 組残す、metadata で追加 2 file 削除。

## 本番 target の実測（読み取りだけの dry run）

対象 `/local/celeris/data/scratch/targets/release-build/target`（2026-10-10 08:4xZ、`/local` 300G 中 196G 使用 67%）。marker `.celeris-release-build-start` はまだ無い（本変更が未配備）ので、start は進行中の release の開始（`.rustc_info.json` の mtime 08:40:45Z = 1791621645）を使った。tree はこの worktree（同じ workspace member）。関数本体を抜き出して `SD_RELEASE_PRUNE_DRY_RUN=1` で実行（削除なし。前後で `debug/deps` は build 進行で 3782→3800 に増えただけ）。

| start | 候補 file | 大きさ（allocated） |
|---|---|---|
| 2026-10-10 08:40:45Z（今回の release の開始） | 307 | 27067383808 B（25.2 GiB） |
| 2026-10-10 00:00Z | 85 | 0.46 GiB |
| 2026-10-09 00:00Z | 29 | 0.14 GiB |

workspace の実行ファイルの内訳（start = 08:40:45Z）: 全 199 個・53.1 GiB、今回の build 済み 124 個・34.5 GiB（残る。build 進行中で途中の値）、古い package 名のもの 8 個・3.1 GiB（旧規則でも刈れた）、古い integration test 等 67 個・15.4 GiB（新規則で初めて刈れる）。依存 crate として残した一致は 0（cargo metadata の target 名で依存と衝突するものは今の本番 target に無い）。target 名 159・package 11。

人が後で取り直す手順: 配備後に `release.sh` の log の `release prune:` 行を見る（配備直後の 1 回目は marker が無いので 0 件で、2 回目から刈る）。手で dry run するなら、`scripts/selfdeploy/lib.sh` から関数を抜き出し `SD_RELEASE_PRUNE_DRY_RUN=1 sd_release_prune_stale_test_binaries <target> <tree> "$(cat <target>/.celeris-release-build-start)"`（`sd_log` は `echo` で代用）。

## 未解決事項

- 配備直後 1 回目の release は marker が無く刈らない（ADR 付記の残る誤差）。
- 改名・削除した test の古い binary は名前が一致せず残る（D3 の上限で作り直すまで）。
- 全体試験・clippy は後段 reclose で取る（この WU は selfdeploy の shell と文書だけ）。

## 提案

- なし
