# Phase guardrail: source-size-report（ADR-0083）

work unit `guardrail`（task 01M3Q6F0Y8M0HDMF6Y68G8519M）。P0〜P2 の構造分割が今後も維持されるように、
production の肥大化・巨大 inline test・gitignore された未追跡 mod を毎回可視化する warning-only ツールを追加した。

## 成果物

- `scripts/dev/source-size-report.py` — 分類（production / inline test / external test / 生成物）と 3 種の
  チェック（production サイズ・inline test mod サイズ・untracked かつ gitignore された `mod` 参照）。
  既定 exit 0、`--strict` のときだけ非除外の warning があれば exit 1。`--format json` で機械可読。
- `scripts/dev/source-size-report.toml` — 閾値（production 2,000 行・inline test mod 300 行、
  audit.md §9 の案のまま）と例外 1 件（`crates/task-api/src/types.rs`、理由付き）。
- `scripts/tests/test_source_size_report.py` — 28 tests（stdlib `unittest` のみ、都度 `git init` した一時
  repo に対して実行。ネットワーク不使用）。
- `scripts/selfdeploy/release.sh` — `cargo-clippy` の直後に `source-size-report` 段を追加（`--strict` なし）。
- `docs/adr/0083-source-size-guardrail.md`。

## 実行結果（2026-09-30、HEAD 時点）

```
$ python3 -m unittest scripts.tests.test_source_size_report
Ran 28 tests in 0.310s
OK

$ python3 scripts/dev/source-size-report.py --strict; echo "exit: $?"
...
21 active warning(s), 1 excepted
exit: 1

$ python3 scripts/dev/source-size-report.py; echo "exit: $?"
...(同じ report)...
exit: 0

$ cargo fmt --all -- --check; echo $?
0

$ cargo clippy --workspace -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 28.53s
(warning 0 件、exit 0)

$ cargo test --workspace
exit: 0, "N passed" 行の合計 2886、FAILED/panicked 0 件
```

現 HEAD で `--strict` は 21 件の非除外 warning を出す（task-core の inline `mod tests` 19 件が
300 行超、`dispatcher.rs` 2,214 行・`paperqa.rs` 2,269 行が production 2,000 行超）。これは「P2 で
cohesive と判断して残した」または「まだ手を付けていない」ファイル群であり、この WU の役割はそれを
分割することではなく、今後も見え続けるようにすることである。どのファイルをどう扱うかの最終的な棚卸しは
`final` work unit の前後比較レポートを参照。untracked かつ gitignore された `mod` 参照は現 HEAD には 0 件。

## 既知の限界

- inline test / cfg(test) item の抽出は rustfmt 整形（brace が行頭 indent 位置に来る）に依存するヒューリスティック。
  `wu/audit/artifacts/loc_audit.py` と同じ前提を引き継いでいる。
- `mod` 解決はネストした `mod foo { mod bar; }` の入れ子 path までは追わない（トップレベルの
  `mod x;` 宣言のみ)。現 HEAD ではこの限界による false negative は確認されていない（warning 0 件）。
