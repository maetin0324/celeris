---
title: S1 daemon-wire — daemon への結線と celerisctl への deno_core の波及
tasks: [01M6DJ6QCMCWT80MTYJ7SNE0XK]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S1 daemon-wire — daemon への結線

## 結論
- `[script]` の結線は動く。`script_runtime_` の試験と architecture-map の検査は通る。
- 計画 v4（案 A）で受け入れ条件 1 は「`deno_core` を推移的に持つ workspace crate が `celeris-script`・`celeris`・`celerisctl` だけ」になった。`task-core`・`task-dispatch`・`task-api` は 0 件。
- 境界 ADR に (b) の前提の誤りと `celerisctl` への波及の付記を足した。

## 変更
`phase 1: daemon-wire: [script] の結線と architecture-map`、`phase 1: daemon-wire: 境界 ADR に celerisctl への波及の付記、進捗を done に`
- `crates/celeris/src/config/script.rs`: 任意の `[script]`（`state_dir`・`active_bundle_sha256`・`input_limit_bytes`・`heap_limit_bytes`）。値の形の誤りは設定エラー。`config::fields` では起動時のみの欄。
- `crates/celeris/src/daemon/script_layer.rs`: 起動時の検証と actor の起動、失敗時の log と通知（`BadNews`、束ね key `script_bundle_rejected`）、停止時の join。
- `crates/celeris/src/daemon/run.rs`: `?` で抜ける経路の後で起動し、MCP を止めた後で止める。S1 では handle を判定に使わない。
- `crates/celeris/tests/script_runtime.rs`: `celeris::run` を試験 process で動かし、`/proc/self/task` で actor thread を観察する。
- `config/celeris.example.toml`・`docs/architecture-map.md`。
- `crates/celerisctl/src/commands/worker_tests.rs`: `script: None`（計画 v4 で範囲内になった）。

## 決めごと
- `state_dir` は必須にした。既存の config に state dir の欄が無く、DB の dir から推測すると verify の `--db` 上書きで読む先が変わるため。
- `--mode verify` と standby でも bundle を load する。S1 では load の成否しか結果に影響しない。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p celeris -E 'test(script_runtime_)'`: exit 0、9 tests passed。`script_runtime_absent_no_thread`・`script_runtime_valid_bundle_loads`・`script_runtime_bad_bundle_fail_closed` を含む。
- `python3 -I scripts/dev/check-architecture-map.py`: exit 0、818 件のパスを確認。
- `bash scripts/dev/test-affected.sh`: exit 0。Rust 6963 tests passed、21 skipped。web の unit 697 tests passed（101 file）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `cargo tree -i deno_core --workspace -e normal --prefix none`: `celeris`・`celeris-script`・`celerisctl`・`deno_core`。計画 v4 の受け入れ条件 1 を満たす。
- `cargo tree -p task-api -p task-dispatch -p task-core -e normal`: `deno_core` は 0 件。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変更 14 path はすべて計画 v4 の範囲内。
- `sh scripts/dev/progress-index.sh --check`: exit 0。

## 未解決事項
1. 解決済み（計画 v4 で案 A を採用し、境界 ADR に付記を足した）。受け入れ条件 1。依存の実測と選択肢は次のとおり。
   - debug build の `celerisctl` に `v8::`・`deno_core::` の定義が 57044 個ある（`nm -C --defined-only`、`celeris` は 68020 個）。release build でも linker は消さない（attempt 2 で `cargo build --release -p celerisctl` を実測。binary 97,713,712 byte、`v8::`・`deno_core::`・`simdutf` の定義 58423 個、`nm -S` の大きさの和 21.0 MiB。V8 を持たない base の release binary との比較は未測定）。
   - 案 A（check と ADR (b) を直す）: `celerisctl` を許可リストに足す。`task-core`・`task-dispatch`・`task-api` に V8 を持たせないという ADR の目的は保てるが、`celerisctl` の build と binary は重くなる。
   - 案 B（package を分ける）: V8 を持たない lib の package と、`celeris` の binary の package に分け、`celerisctl` は lib だけに依存する。ADR (b) のとおりになるが、`crates/celerisctl/Cargo.toml`、`scripts/selfdeploy/release.sh`、`-p celeris` を使う検査、`crates/celeris/tests/` が影響を受ける。script layer は `run` へ注入する形に変わる。影響の全量は未確認。
   - 不採用（feature で分ける）: `celeris` に既定 feature を、`celerisctl` に `default-features = false` を一時的に指定して確かめた。`cargo tree --workspace` には `celerisctl` が残る。`-p celerisctl` 単独なら `deno_core` は 0 件だが、`cargo build -p celeris -p celerisctl`（`release.sh`）では feature が統合される。
2. 解決済み（計画 v4 で範囲内になった）。`crates/celerisctl/src/commands/worker_tests.rs` に `script: None` を 1 行足した。`Config` を欄ごとに書く試験で、足さないと `test-affected.sh` がコンパイルで落ちる。範囲 check の許可の外なので、範囲を直す必要がある。
3. bundle の top-level の同期無限ループで daemon の起動が止まらないことは試験していない。CPU を焼く試験を置かない規則のため。コードでは actor の `LOAD_TIMEOUT`（10 秒）で watchdog が止める形だが、この経路の実測は無い。
4. `state_dir` を worker の run から書けなくする保護は入れていない。S1 では bundle を判定に使わない。業務領域を切り替える段より前に要否を決める。

## 提案
2. 前回の run は `result.json` に `summary` と `yield.plan_issue` を両方書いた。`summary` が `yield` より優先されるため（`crates/task-worker/src/adapter.rs` の優先順位）、run は done として扱われ、plan_issue は捨てられて同じ check で retry になった。worker の手引き（plan_issue の書き方）に「`summary` を一緒に書かない」と明記するか、両方あるときは plan_issue を優先するよう直す。attempt 2 は `yield` だけを書く。
1. 境界 ADR (b) の「`celeris`（binary）」の前提を直す。`celeris` は lib でもあり、`celerisctl` が依存している。S0 の棚卸しはこの依存を見落としていた。
