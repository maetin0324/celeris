---
status: running
task: 01M5AH1A1F6YMPCCJTCGBC84H5
---
# dev/test profile の debuginfo 削減

## 完了
- ADR `agent-docs/adr/2026-10-19-dev-profile-debuginfo.md`、Cargo.toml に `[profile.dev] debug="line-tables-only"` と `[profile.dev.package."*"] debug=0`（release 不変）。

## 測定（未完）
- 同 commit を `git archive` した写し（base, `CARGO_PROFILE_DEV_DEBUG=2`）と変更後（env 無し）を `cargo test --workspace --no-run -j 4` で並走。host 負荷が高く run の壁時計（600 秒）で打ち切られた。
- 打ち切り時点（同じ時間の途中経過・完成値ではない）: base target 2.38 GiB (2494142 KB) / after target 1.40 GiB (1470795 KB)、約 41% 減。build 時間は未取得。
- 注意: 本 run の環境には `CARGO_PROFILE_DEV_DEBUG=line-tables-only` が既に設定されており、環境変数は Cargo.toml に優先する。

## 測定（範囲を絞った完走値、run #2）
host load 約 65 のため workspace 全体は 600 秒で完走しない。`-p task-core -p task-ops` で測った（同じ commit の写しと現 worktree を並走、各 `-j 4`、空の scratch dir、終了後に削除）:
- base: `CARGO_PROFILE_DEV_DEBUG=2 cargo test -p task-core -p task-ops --no-run -j 4` → 174.19 s、`du -s --apparent-size` = 1383461 KB (1.32 GiB)
- after: `env -u CARGO_PROFILE_DEV_DEBUG cargo test -p task-core -p task-ops --no-run -j 4` → 163.58 s、729025 KB (0.70 GiB)
- target は約 47% 減、build 時間は約 6% 短縮（並走・高負荷下の値）。
- backtrace 依存の試験: `grep backtrace crates` は campaign/tests/real_asset_grade.rs の 1 件のみ（panic 位置に依存しない）。

## 未実施
- 完走した前後の測定、test-parallel.sh / clippy / fmt の実行、commit。
