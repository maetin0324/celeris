---
title: S1 crate-skeleton — celeris-script crate の骨組み（JsRuntime を専用 thread で持つ actor と bounded queue の handle）
tasks: [01M6DJ6QCMCWT80MTYJ7SNE0XK]
status: done
updated: 2026-11-02
---
# S1 crate-skeleton — celeris-script の骨組み

完了日: 2026-11-02（範囲 check の広げ方は「未解決事項」1）

## 結論
- `crates/script-runtime/`（package `celeris-script`、`deno_core = "=0.413.0"`、既定 feature のみ）を作り、
  `runtime_sync`・`runtime_async`・`runtime_exception`・`runtime_busy` の 4 試験が host の設定のまま通る（提出時の HEAD で 4 passed）。
- V8 archive: 人が置いた simdutf 版（決定 `v8-archive-simdutf` の答え「archive 配置済み」）を `~/.cargo/config.toml` の `[env] RUSTY_V8_ARCHIVE` 経由で使う。
- serde_json: 人の答え「patch」に従い、crates.io の deno_core 0.413.0 の写しを `crates/script-runtime/vendor/deno_core/` に置き、
  root の `[patch.crates-io]` で使う。写しは `Cargo.toml` の serde_json の features から `float_roundtrip`・`preserve_order` を消しただけで、
  `raw_value` は残した。判断は実装前に ADR `agent-docs/adr/2026-11-02-deno-core-vendored-serde-json-features.md` に書いた。
  workspace の serde_json に `preserve_order`・`float_roundtrip` は入らない（`cargo tree --workspace -e features -i serde_json | grep -cE 'preserve_order|float_roundtrip'` → 0）。
- `float_roundtrip` は外せる。外して落ちる deno_core 自身の試験は inspector の経路の 3 件だけで、`celeris-script` は inspector を使わない（下の表）。
- 写しを置いたことで、repo の文書・大きさの検査 2 本（`check-source-size.py`・`check-doc-links.sh`）が写しを読んで落ちたので、写しを対象から外した。
  この 2 本と ADR は計画の範囲 check（`Cargo.toml|Cargo.lock|crates/script-runtime/|agent-docs/progress/2026-11-02-ts-control-layer-s1`）の外にある。

## 変更
- `Cargo.toml`: workspace の `members` に `crates/script-runtime` を足し、`exclude` に `crates/script-runtime/vendor`、
  `[patch.crates-io] deno_core = { path = "crates/script-runtime/vendor/deno_core" }` を足した。
- `crates/script-runtime/vendor/`: `deno_core/`（写し、2.6 MB）、`deno_core.lock.toml`（版 0.413.0、`.crate` の sha256
  `7ad7d5294f640fd99db08c455e2453c9860b1fa728b76fbdacf0eed3e308d48a`、消した feature）、`vendor-deno-core.sh`（作り直し、`--check` で照合）。
- `agent-docs/adr/2026-11-02-deno-core-vendored-serde-json-features.md`: 写しの置き方の ADR（範囲外）。
- `scripts/dev/source-size-report.py`: `^crates/[^/]+/vendor/` を `vendored` に分類し大きさを測らない（範囲外）。
- `scripts/dev/check-doc-links.sh`: `ALWAYS_EXCLUDE` に `crates/script-runtime/vendor/` を足した（範囲外。写しの README.md 等の link は元の repo を指す）。
- `Cargo.lock`: deno_core 0.413.0 と推移的依存（新規 package 93 件）を足した。既存の package では icu 系 7 件が semver の範囲で上がった
  （`icu_locale_core` 2.3.0→2.4.0、`icu_provider` 2.3.1→2.4.0、`tinystr` 0.8.4→0.8.5、`yoke`・`yoke-derive` 0.8.3→0.8.4、
  `zerovec` 0.11.8→0.11.9、`zerovec-derive` 0.11.6→0.11.7）。
- `crates/script-runtime/Cargo.toml`・`src/lib.rs`・`src/error.rs`・`src/handle.rs`・`src/actor.rs`・`tests/runtime.rs` を追加した。
  - `lib.rs`: 公開は `ScriptRuntimeHandle`・DTO（`ScriptSource`・`ScriptCall`・`ScriptReply`・`RuntimeStats`）・`ScriptError` と定数
    （`QUEUE_CAPACITY` 64・`MAX_IN_FLIGHT` 64・`DISPATCH_FUNCTION`）だけ。`deno_core`・`v8` の型は公開しない。
  - `actor.rs`: `std::thread` 1 本が `JsRuntime` を生成・所有・破棄する。thread の中の current-thread tokio runtime が 1 つの `poll_fn` で
    shutdown の合図・queue の取り出し・`poll_event_loop`・未完の要求の結果を回し続ける。V8 platform の初期化は `std::sync::Once` で process に 1 回。
    未完が `MAX_IN_FLIGHT` に達したら queue から取り出さない（backpressure）。受け手が落ちた要求（cancel・timeout）の結果は捨てる。
    script の返した `undefined` と JSON にできない値は `InvalidOutput`。
  - `handle.rs`: bounded `tokio::sync::mpsc`（容量 64）、要求ごとの `oneshot`、`request_id` は ULID、`deadline` は `timeout` から計算して
    `tokio::time::timeout_at` で待つ（過ぎたら `Timeout`）。`try_send` が満杯なら `Busy`。最後の handle の Drop か `shutdown()` で
    actor を止めて thread を join し、未完と queue の要求には `Terminated` を返す。監視は `stats()`・`watch_stats()`・`queued()`。
  - `error.rs`: `ScriptError::{Exception, Timeout, Busy, InvalidOutput, Denied, Terminated, GenerationGone}`（serde の `tag = "kind"`）。
  - 試験の script は `tests/runtime.rs` の固定の JS 文字列。業務 TS は入れていない。`build.rs` は無い。
- watchdog（同期無限ループの V8 中断）・generation の破棄・heap/input 上限は並行の unit `runtime-guard` の範囲なので入れていない。
  `ActorRequest.deadline` は watchdog が読む欄として置いた。

## 確かめたこと（提出時の HEAD で実行。base は `$CELERIS_WU_BASE`）
| 項目 | コマンド | 結果 |
|---|---|---|
| rustc の版 | `rustc --version` | rustc 1.98.1（2026-09-01）、cargo 1.98.1。`cargo build -p celeris-script` は通る（写しの deno_core に rustc の警告 1 件: `TcpStream::set_linger` の deprecated。path 依存なので cap-lints されない。error ではない） |
| 4 試験 | `cargo nextest run -p celeris-script`（host の設定のまま） | exit 0、4 tests run: 4 passed |
| 試験名 | `cargo test -q -p celeris-script --test runtime -- --list` | runtime_async・runtime_busy・runtime_exception・runtime_sync |
| debug profile で release archive を link | 上の 4 試験（`test` profile は debug、archive は `release`） | link でき、試験が通る |
| deno_core を推移的に持つ crate | `cargo tree -i deno_core --workspace -e normal --prefix none \| awk '{print $1}' \| sort -u` | `celeris-script`・`deno_core` だけ（写しの path） |
| build.rs | `test ! -e crates/script-runtime/build.rs` | 無い |
| 版の衝突 | `cargo tree -d -p celeris-script -e normal --depth 0` | `getrandom` 0.3.4/0.4.3、`syn` 2.0.119/3.0.5（どちらも workspace に既にあった組）。`serde`・`serde_core`・`smallvec` は同じ版が host 用と target 用で出るだけ |
| serde_json の feature | `cargo tree --workspace -e features -i serde_json \| grep -cE 'preserve_order\|float_roundtrip'` | 0 |
| 写しの照合 | `sh crates/script-runtime/vendor/vendor-deno-core.sh --check` | exit 0（`.crate` の sha256 一致、Cargo.toml の差分は 2 行だけ） |
| deno_core 自身の試験（写し） | 写しを `$TMPDIR` に複写し `[workspace]` を足して `cargo test --lib` | 473 passed・7 failed・2 ignored |
| deno_core 自身の試験（元の crate） | `.crate` を展開した物で同じ | 476 passed・4 failed・2 ignored |
| 差の 3 件 | 上の 2 つの比較 | `runtime::tests::misc::test_preserve_float_precision_from_local_inspector_evaluate` の case 1〜3（入力 0.9999999999999999 等。inspector の protocol の JSON を `serde_json::from_str` で読むと最後の桁が変わる。deno の issue 29059）。deno_core の中で serde_json の文字列 parse を使うのは `inspector.rs` の 4 か所だけで、`celeris-script` は inspector を有効にしない |
| 元の crate でも落ちる 4 件 | 同上 | `tests::located_script_name`・`tests::test_rebuild_async_stubs`（deno の CLI と repo の file が要る）、`uv_compat::tests::pipe_*_transfers_raw_fd_ownership` 2 件。写しの変更と無関係 |
| serde_v8 0.322.0 の試験 | `.crate` を展開し `cargo test --lib` | 4 passed。結合試験は dev-dependency `serde_v8_utilities` が公開 crate に無く build できない。serde_v8 自身は serde_json に `raw_value` だけを付け、写しの変更の影響を受けない |
| fmt | `cargo fmt --all -- --check` | exit 0（写しは member でないので対象外） |
| clippy（workspace） | `cargo clippy --workspace -- -D warnings` | exit 0（前の run の task-api の `result_large_err` 851 件は 0 件） |
| health | `bash scripts/dev/check-health.sh` | exit 0（source-size gate 0 failure、check-doc-links ok） |
| test-affected | `bash scripts/dev/test-affected.sh` | exit 100。web: typecheck 通過、vitest 101 file・697 passed。Rust: 6921 tests run: 6917 passed・4 failed・21 skipped。落ちた 4 件は下の「未解決事項」2 |

## V8 archive の使い方
- v8 150.4.0 の build.rs は `RUSTY_V8_ARCHIVE` があればその file を `target/<profile>/gn_out/obj/librusty_v8.a` に展開して使い、
  network に出ない（build の stderr に `static lib URL: <path>` と `Detected GZIP archive` が出る）。src_binding は crate 同梱の
  `gen/src_binding_simdutf_release_x86_64-unknown-linux-gnu.rs` を使うので `RUSTY_V8_SRC_BINDING_PATH` は要らない。
- 必要な archive は `librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz`（38,799,023 byte、
  sha256 `f48762ca10d1f1fc605a441c5ae430ec8ce1e9e80f14d78fbc42cb878c30b476`）。GitHub の release v150.4.0 の asset digest と一致することを
  前の run で確かめ、この run でも取得して sha256 と `gzip -t` が合った。
- 人が 2026-11-02T09:34Z に simdutf 版を `/local/celeris/data/scratch/rusty-v8/v150.4.0/` に置き、`RUSTY_V8_ARCHIVE` をその path に替えた。
- 先に置かれた `librusty_v8_release_x86_64-unknown-linux-gnu.a.gz`（sha256 `5869fdb7071b1f21f200a97ce97d20125877e0f31cd802ed99de2c53f488b06c`）は
  simdutf 無しの版で、deno_core 0.413.0 とは link できない。
- `RUSTY_V8_ARCHIVE` は cargo の `[env]` で渡すので、値を替えると v8 の build script が走り直す（`rerun-if-env-changed`）。

## 参考: serde_json `preserve_order` を受け入れた場合の影響の監査（前の run。patch を選んだので直していない）
`preserve_order` が有効になると `serde_json::Map` が BTreeMap（key の辞書順）から IndexMap（挿入順）に変わる。
`serde_json::to_value(struct)`・`json!({...})`・file から読んだ `Value` を、key を並べ直さずに直列化して sha256 を取る所は値が変わる。
- 変わらない（key を BTreeMap で並べ直してから直列化する）: `crates/campaign/src/manifest.rs` の `canonical`（`compute_sha256`）、
  `crates/campaign/src/lock.rs` の `lock_hash`、`crates/campaign/src/variant.rs` の `canonical_sha`、`crates/task-core/src/variant_wire.rs` の
  `effective_profile_sha256`、`crates/task-api/src/cos/operations.rs` の `canonical_json`（`request_hash`）、
  `crates/campaign/src/dagbench/pbs/intent.rs` の `IdCanon`（struct の欄を辞書順に宣言）。
- 変わる（並べ直していない）: `crates/campaign/src/manifest.rs` の `to_value(m)` に `sha256` を挿入して `to_vec` する関数（1535 行付近）、
  `crates/routing-eval/src/manifest.rs` の `hash_value`、`crates/task-core/src/browser.rs` の policy の `sha256:` 計算（`json!` を `to_string`）、
  `crates/campaign/tests/variant_pin_b1c1.rs` の `json!` の sha256（「serde_json の object は key 順固定」と注記あり）。
- 未監査: sha256 と `serde_json::Value`/`json!` を同じ file で使う file は crates/ に 80 件あり、上の 10 件以外は読んでいない。

## 未解決事項
1. 範囲 check: ADR と `scripts/dev/source-size-report.py`・`scripts/dev/check-doc-links.sh` の変更は、計画の範囲 check の外にある。
   ADR は人の条件「置き方の判断は実装前に ADR に書く」と CLAUDE.md の規則によるもの、2 本の検査は写しを repo に置くと
   `test-affected.sh`（`check-health.sh`）が落ちるための変更。範囲 check に `agent-docs/adr/2026-11-02-deno-core-vendored-serde-json-features.md`、
   `scripts/dev/source-size-report.py`、`scripts/dev/check-doc-links.sh` を足す replan が要る。
2. test-affected の 4 件の失敗: `celeris-campaign` の `ops_r2_socket::{socket_bind_is_only_the_socket_and_in_the_allow_table, socket_flag_rejects_non_sockets}`、
   `ops_r2_b_start::{b_daemon_starts_after_the_seed_is_unpacked_into_the_project_repo_and_matches_a, b_command_refuses_bad_arguments_and_missing_prerequisites}`。
   どれも `UnixListener::bind` が `path must be shorter than SUN_LEN` で落ちる。この run の `CARGO_TARGET_DIR` は 104 文字で、
   `CARGO_TARGET_TMPDIR` の下の tempdir に `mux/master.sock` を作ると 108 byte を超える。この unit の変更（celeris-script・写し）とは無関係で、
   直すのは campaign の試験の範囲（tempdir を短い path に置く）。
3. 決定の出し方（前の run の記録）: `worker_finish.rs` は WU の判定が `completed` のときだけ `decisions` を読むので、check が落ちる run の決定は捨てられる。

## 提案
- `crates/task-dispatch/src/dispatcher/worker_finish.rs`: check が不合格の run の `decisions`（`needed_before: self`）も記録して WU を `blocked(decision)` にする。
- 境界 ADR (e) の表の `v8` の行に「deno_core は `simdutf` feature を必須にするので archive は `librusty_v8_simdutf_release_<target>.a.gz`」と付記する（S1 の close unit）。
- 境界 ADR (b) の依存の制約に、deno_core の写しと serde_json の feature の扱い（ADR 2026-11-02-deno-core-vendored-serde-json-features）を付記する（S1 の close unit）。
- S1 の host-api unit: inspector を有効にしないこと、数値を serde_json の文字列 parse で渡さないことを試験で固定する。
- campaign の試験の unix socket は `CARGO_TARGET_TMPDIR` の深さに依らない短い path に置く（未解決事項 2）。
