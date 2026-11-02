---
title: S0 adr — Rust/TS 境界 ADR（Host API・generation・版固定・数値目標と停止基準）
tasks: [01M6DCPKPJZZ9F9J4QJ47FH8KK]
status: done
updated: 2026-11-02
---
# S0 adr — Rust/TS 境界 ADR

完了日: 2026-11-02

## 変更
- `agent-docs/adr/2026-11-02-ts-control-layer-boundary.md` を追加した。節 (a) 責任境界、(b) 接続、(c) Host API 契約 `celeris:host`、
  (d) generation と状態モデル、(e) 版の固定、(f) 数値目標と停止基準、(g) 凍結環境と識別。
- コード・`docs/architecture-map.md` は変えていない（地図の行は `crates/script-runtime/` を作る S1 で足す。実在しない path を書かないため）。

## 版の確かめ方（2026-11-02 に実行。外部 network は crates.io と npm registry の読み出しだけ）
```sh
curl -s https://index.crates.io/de/no/deno_core | tail -1      # 0.413.0、deps: v8 = deno_v8 ^0.5.0、tokio ^1.47.1
curl -s https://index.crates.io/de/no/deno_v8 | tail -1        # 0.5.0、deps: v8 ^150.4.0
curl -s https://crates.io/api/v1/crates/v8/150.4.0             # 2026-07-26、yanked false、rust_version null
curl -s https://registry.npmjs.org/typescript                  # latest 7.0.2（2026-07-08）
curl -s https://registry.npmjs.org/esbuild                     # latest 0.28.2（2026-08-08）
curl -sL https://static.crates.io/crates/deno_core/deno_core-0.413.0.crate | sha256sum
```
`deno_core`・`deno_v8`・`v8` の `.crate` は `$TMPDIR` に落として `Cargo.toml`（edition 2024、`rust-version` なし）と
`v8` の `build.rs`（`RUSTY_V8_ARCHIVE`・`RUSTY_V8_MIRROR` で事前 build の archive を取る）を読んだ。build はしていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `git diff --stat "$CELERIS_WU_BASE" -- crates web` → 出力なし
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok
- `bash scripts/dev/check-doc-links.sh` → ok、`bash scripts/dev/check-adr-numbers.sh` → ok
- `sh scripts/dev/progress-index.sh --check`、`bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check` → 報告本文に結果を書く

## 未解決
- rustc 1.98.1 で `deno_core` 0.413.0 が build できるか、依存の版衝突があるかは未確認（S1 の最初の `cargo check`・`cargo tree -d`）。
- `v8` 150.4.0 の事前 build archive の大きさと sha256 は未確認（S1 で取得して記録する）。
- daemon の現在の RSS は未計測。RSS の許容増分は絶対値（100 MiB・200 MiB）で書いた。
- TS の型検査・bundle の秒は未計測（S3 で測る）。

## 提案
- S1 の範囲 check に `Cargo.toml`・`Cargo.lock`・`crates/script-runtime/`・`crates/celeris/`・`docs/architecture-map.md`・`docs/api/script-host/` を最初から含める。
