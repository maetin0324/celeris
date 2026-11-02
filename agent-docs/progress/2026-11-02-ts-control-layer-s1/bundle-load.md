---
title: S1 bundle-load — manifest 検証と配備・実 bundle の load 確認と遅延計測の道具
tasks: [01M6DJ6QCMCWT80MTYJ7SNE0XK]
status: done
updated: 2026-11-02
---
# S1 bundle-load — manifest 検証と配備

完了日: 2026-11-02

## 結論
- `control-ts` の実 bundle を `script-load-check` が load し、試験用の領域 `host_selftest` を 1 回呼んで exit 0 で終わる。
- TS の本文を変えて bundle を作り直しても、`cargo build -p celeris-script --bins` の Compiling は 0 行で、新しい hash の bundle を同じ binary が load する。
- 配備は library の関数だけで、配備する command は無い（提案 1）。

## 変更
`phase 1: bundle-load: manifest 検証と配備、script-load-check と script-bench`
- `crates/script-runtime/src/manifest.rs`: `celeris.script-manifest/1` を `deny_unknown_fields` で読む。検証は `verify_bundle`、読み込みは `load_bundle_dir`・`load_deployed`、配備は `deploy_bundle`。
- `crates/script-runtime/src/selftest.rs`: 試験用の領域の要求と結果の照合。
- `crates/script-runtime/src/bin/script-load-check.rs`・`script-bench.rs`。
- `crates/script-runtime/tests/manifest.rs`: 指定の 5 本と、配備・bin の試験 5 本。
- `crates/script-runtime/tests/fixtures/selftest-bundle/`: 手書きの JS と manifest（業務 TS ではない）。
- 依存: `sha2`、dev の `tempfile`（どちらも既に lock にある版）。

## 検証の決めごと
- `host_api` は ADR (c) の `^<major>` の形だけを受ける。`^1.2`・`>=1` は major を読まずに拒否する。この binary が `^1.2` を満たすかを判定する処理を持たないため。
- `schema_sha256` は `host_api_schema_json()` の sha256 と比べる。この値と `docs/api/script-host/host-v1.schema.json` の byte の一致は `script-schema` の検査で確かめている。
- `bundle_sha256` は path に使うので、小文字 16 進 64 桁以外を拒否する。`load_deployed` は配備先の dir 名と manifest の hash の一致も確かめる。

## 配備の決めごと
- 一時 dir `.tmp-<ULID>` に 2 file を書いて fsync し、`<state_dir>/script-bundles/<bundle_sha256>/` へ rename する。検証した byte をそのまま書く。
- 既に配備先が在れば、hash を検証して再利用する。`toolchain`・`source_commit` だけが違う再配備は既存の manifest を残す。
- `host_api`・`schema_sha256`・`domains`・`capabilities`・`checkpoint_versions` が違えば `DeployConflict` にする。既存が壊れていれば検証の誤りで止め、上書きしない。
- `checkpoint_versions` は `celeris.script-checkpoint/<n>` の n として `u32` の配列で読む（ADR は n の型を書いていない）。

## script-bench の出力
`latency_ms.{p50,p99}` と `rss_mib.{one,two}` は `overhead` unit の `overhead.json` と同じ形で、ほかに `profile`・`iters`・`warmup`・`rss_baseline_mib`・`bundle_sha256` を持つ。
- 判定 1 回は `host_selftest` の呼び出し（async op 1 回を含む）。isolate ごとに warm up 100 回を捨てる。
- `rss_mib` は最初の isolate を作る前からの差で、`two` は 2 個の合計。
- 参考値（debug build、1000 回）: p50 0.270 ms、p99 0.329 ms、RSS one 26.6 MiB、two 33.7 MiB。閾値の判定は `overhead` unit が行う。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p celeris-script`: 37 tests passed。`manifest_hash_mismatch`・`manifest_host_api_major_mismatch`・`manifest_schema_mismatch`・`manifest_unknown_field`・`manifest_load_fixture` を含む。
- `cargo run -q -p celeris-script --bin script-load-check -- --bundle-dir control-ts/dist`: exit 0。
- bundle を作り直した後の `cargo build -p celeris-script --bins -v | grep -c Compiling`: `touch` の場合も、`src/main.ts` の本文を一時的に変えて hash が変わった場合も 0。変えた本文の bundle も同じ binary が exit 0 で load した（本文は戻した）。
- `crates/` に `control-ts` の `include_str!`・`include_bytes!` は無く、`crates/script-runtime/build.rs` も無い。
- `bash scripts/dev/test-affected.sh`: exit 0（Cargo.lock の変更で全 crate と web が対象）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- 範囲 check: 範囲外の path なし。

## 未解決事項
1. schema を変えると `tests/fixtures/selftest-bundle/manifest.json` の `schema_sha256` が古くなり、`manifest_load_fixture` が `SchemaMismatch` で落ちる。これは意図した動きで、schema を変える unit が fixture の hash も直す。control-ts の未解決事項 1（`note` の nullable）を直すときも同じ。その修正はこの unit では行っていない。
2. bundle が同じで `capabilities` などだけを変えた manifest は `DeployConflict` で配備できない。配備先は hash ごとに 1 つなので、今の形では古い dir を消すしかない。古い dir の扱いは S1 に無い（`script_generations` と合わせて決める）。
3. crash で残った `.tmp-*` は消さない。読み込みには影響しない。

## build の注意
- reflink seed の target では `could not find native static library rusty_v8` が出た。自分の `CARGO_TARGET_DIR` の `debug/build/v8-*` と `debug/.fingerprint/v8-*` を消して直った。
- cargo 1.98.1 では `env!("CARGO_BIN_EXE_<name>")` がこの crate で compile に失敗した（原因は未確認）。repo の他の試験と同じく実行時の `std::env::var` で読む。

## 提案
1. bundle を配備する command（例 `script-deploy --dist <dir> --state-dir <dir>`）を足す。今は `deploy_bundle` を呼ぶ bin が無く、人の手順に書ける操作が無い。`close` unit で手順を書くときに要否を決める。
