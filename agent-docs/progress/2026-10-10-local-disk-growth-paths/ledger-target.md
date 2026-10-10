# ledger-target

- `browser-ledger.sh` は明示 `CARGO_TARGET_DIR` を優先し、未指定なら固定 build tree と owner `release-build` で `sd_scratch_lease` を解決する。
- 共通 `sd_release_build_target` は lease 取得時に `.cargo-target` symlink を現 lease へ原子的に更新する。lease が失敗した場合、dangling link は除去してローカル fallback directory を使い、存在する symlink は古い lease とみなして拒否する。
- `browser_ledger_default_target.sh` は偽 celerisctl・runner と一時 directory で、lease target、dangling link、caller target 優先を検証する。
- 対象4試験と `scripts/selfdeploy/tests/*.sh` 全件が exit 0。`crates/` の差分なし。
