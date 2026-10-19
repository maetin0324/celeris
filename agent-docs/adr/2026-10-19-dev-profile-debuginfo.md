# ADR: dev/test profile の debuginfo を減らす

- 日付: 2026-10-19
- 状態: 採用

## 背景
task/WU ごとの cargo target が 33〜40 GB になり、[scratch] 上限 160 GiB に 4 本ほどで達して GC が実行中 task の target も回収していた。workspace に `[profile]` が無く dev/test は既定の `debug = 2`（完全な debuginfo）で、target の大半は debuginfo だった。

## 決定
1. `[profile.dev] debug = "line-tables-only"`。panic の backtrace の file:line は残る。変数の型情報は無くなる（debugger での変数表示は不可）。`test` profile は dev を継ぐので同じになる。
2. `[profile.dev.package."*"] debug = 0`。依存 crate（workspace member 以外）は行情報も持たない。依存の中の backtrace は関数名のみ。
3. `split-debuginfo` は既定のまま（Linux は off）。unpacked にすると .dwo が target に残り小さくならないため変えない。
4. `incremental` は Cargo.toml では決めない。host の環境（`CARGO_INCREMENTAL`）に任せる。
5. release profile は変えない。
6. 旧い挙動（debug = 2）に戻す切替 flag は作らない。必要なら `CARGO_PROFILE_DEV_DEBUG` で手元だけ上書きできる（環境変数は Cargo.toml に優先する）。

## 帰結
- target の大きさと build の I/O が減る。測定は `agent-docs/progress/2026-10-19-dev-profile-debuginfo.md`。
- 試験の panic message・位置は line tables で保たれる。backtrace の内容に依存する試験は無い（`grep backtrace crates` は real_asset_grade.rs の 1 件のみ）。
