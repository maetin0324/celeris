---
status: done
task: 01M5AH1A1F6YMPCCJTCGBC84H5
completed: 2026-10-19
---
# dev/test profile の debuginfo 削減

## 完了
- ADR `agent-docs/adr/2026-10-19-dev-profile-debuginfo.md`（line-tables-only / 依存 debug=0 / split-debuginfo 既定 / incremental は host 任せ / release 不変 / 切替 flag なし）。
- Cargo.toml に `[profile.dev] debug = "line-tables-only"` と `[profile.dev.package."*"] debug = 0`。`[profile.release]` は追加・変更なし（test profile は dev を継ぐ）。

## 測定
同じ commit（f2fd7114 の木）を `git archive` した写し（base、`CARGO_PROFILE_DEV_DEBUG=2` で旧既定を再現）と変更後（`env -u CARGO_PROFILE_DEV_DEBUG`）を、それぞれ `/local/celeris/data/scratch` 配下の空の専用 `CARGO_TARGET_DIR` で並走させ、各 `-j 4`（計 8）、終了後に dir を削除した。
host load 約 65 のため workspace 全体の `--no-run` は run の壁時計 600 秒で完走しなかったので、範囲を絞った完走値を正とする。

| | コマンド | build 時間 | `du -s --apparent-size` |
|---|---|---|---|
| base | `CARGO_PROFILE_DEV_DEBUG=2 cargo test -p task-core -p task-ops --no-run -j 4` | 174.19 s | 1383461 KB (1.32 GiB) |
| after | `env -u CARGO_PROFILE_DEV_DEBUG cargo test -p task-core -p task-ops --no-run -j 4` | 163.58 s | 729025 KB (0.70 GiB) |

- target 約 47% 減、build 時間 約 6% 短縮（並走・高負荷下）。
- 参考（workspace 全体 `cargo test --workspace --no-run -j 4` を同時間で打ち切った途中値、完成値ではない）: base 2494142 KB / after 1470795 KB（約 41% 減）。

## panic 位置・backtrace 依存の試験
- `grep -rn backtrace crates` は `crates/campaign/tests/real_asset_grade.rs` の 1 件のみで panic 位置に依存しない。line tables で panic の file:line は残る。全試験は下の test-parallel で通過。

## 検査
- `cargo fmt --all -- --check`: exit 0
- `cargo clippy --workspace -- -D warnings`: exit 0
- `bash scripts/dev/test-parallel.sh`: exit 0（reviewer 再実行: passed 5857, failed 0, ignored 15, nextest 254.5 s、`test-parallel: ok`）

## 未解決事項
- workspace 全体の完走した前後測定は高負荷のため取れていない（範囲を絞った値で代替）。load が低い時に `cargo test --workspace --no-run -j 8` で取り直すとよい。
- worker 環境には `CARGO_PROFILE_DEV_DEBUG=line-tables-only` が既に渡されており、env は Cargo.toml に優先する。今後 Cargo.toml が正本になるので、Celeris 側でこの env を渡すのをやめるかは別判断。

## 提案
- Celeris が worker に渡す `CARGO_PROFILE_DEV_DEBUG` を撤去し、Cargo.toml の profile を唯一の正本にする（breaking-changes-by-default）。
