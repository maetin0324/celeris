---
title: S1 crate-skeleton — celeris-script crate の骨組み（JsRuntime を専用 thread で持つ actor と bounded queue の handle）
tasks: [01M6DJ6QCMCWT80MTYJ7SNE0XK]
status: blocked
updated: 2026-11-02
---
# S1 crate-skeleton — celeris-script の骨組み

完了日: 未完（人の決定 2 件待ち。下の「未解決事項」）

## 結論
- `crates/script-runtime/`（package `celeris-script`、`deno_core = "=0.413.0"`、既定 feature のみ）を作り、
  `runtime_sync`・`runtime_async`・`runtime_exception`・`runtime_busy` の 4 試験が通った。
  ただし通ったのは、simdutf 版の V8 archive を run の `$TMPDIR` に取得し `RUSTY_V8_ARCHIVE` を一時的に上書きしたとき。
- 人が置いた archive（非 simdutf 版）では試験 binary の link が落ちる。deno_core 0.413.0 が `deno_v8` を `features = ["simdutf"]` 固定で使うため。
- deno_core 0.413.0 は serde_json の `raw_value`・`float_roundtrip`・`preserve_order` を必須で有効にする。
  workspace をまとめて build すると全 crate の serde_json に効き、`cargo clippy --workspace -- -D warnings` が task-api の
  `clippy::result_large_err` 5 件（`crates/task-api/src/approvals.rs`、`ApiProblem` が 144 byte 以上になる）で落ちる。

## 変更
- `Cargo.toml`: workspace の `members` に `crates/script-runtime` を足した。
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
| rustc の版 | `rustc --version` | rustc 1.98.1（2026-09-01）。`cargo build -p celeris-script` は警告なしで通る |
| 4 試験 | `RUSTY_V8_ARCHIVE=$TMPDIR/v8dl/librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz cargo nextest run -p celeris-script` | exit 0、4 tests run: 4 passed。`cargo test -p celeris-script` でも同じ process の中で 4 passed |
| 4 試験（host の設定のまま） | `cargo nextest run -p celeris-script` | link 失敗: `wild: error: Undefined symbol simdutf__convert_utf16le_to_utf8_with_errors`（deno_core の rlib から参照） |
| debug profile で release archive を link | 上の 4 試験（`test` profile は debug、archive は `release`） | link でき、試験が通る（simdutf 版のとき） |
| deno_core を推移的に持つ crate | `cargo tree -i deno_core --workspace -e normal --prefix none \| awk '{print $1}' \| sort -u` | `celeris-script`・`deno_core` だけ |
| build.rs | `ls crates/script-runtime` | `Cargo.toml src tests`（build.rs 無し） |
| 版の衝突 | `cargo tree -d -p celeris-script -e normal` | 複数版: `getrandom` 0.3.4/0.4.3、`syn` 2.0.119/3.0.5（どちらも workspace に既にあった組。`cargo tree -d --workspace` を base と比べ、新しい版の重複は 0）。`serde`・`serde_core`・`smallvec` は同じ版が host 用と target 用で 2 回出るだけ |
| fmt | `cargo fmt --all -- --check` | exit 0 |
| clippy（crate） | `cargo clippy -p celeris-script --tests -- -D warnings` | exit 0 |
| clippy（workspace） | `cargo clippy --workspace -- -D warnings` | 失敗: task-api の `result_large_err` 5 件。`crates/script-runtime` を members から一時的に外すと 0 件 |
| test-affected | `bash scripts/dev/test-affected.sh` | 未実行。Cargo.lock の変更で全 crate と web が対象になり、上の 2 件の決定で結果が変わるため |

## V8 archive の使い方
- v8 150.4.0 の build.rs は `RUSTY_V8_ARCHIVE` があればその file を `target/<profile>/gn_out/obj/librusty_v8.a` に展開して使い、
  network に出ない（build の stderr に `static lib URL: <path>` と `Detected GZIP archive` が出る）。src_binding は crate 同梱の
  `gen/src_binding_simdutf_release_x86_64-unknown-linux-gnu.rs` を使う。
- 必要な archive は `librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz`（38,799,023 byte、
  sha256 `f48762ca10d1f1fc605a441c5ae430ec8ce1e9e80f14d78fbc42cb878c30b476`）。GitHub の release v150.4.0 の asset digest と一致することを
  この run で確かめた（API `repos/denoland/rusty_v8/releases/tags/v150.4.0`）。
- 人が置いた `librusty_v8_release_x86_64-unknown-linux-gnu.a.gz`（sha256 `5869fdb7071b1f21f200a97ce97d20125877e0f31cd802ed99de2c53f488b06c`）は
  simdutf 無しの版で、deno_core 0.413.0 とは link できない。
- `RUSTY_V8_ARCHIVE` は cargo の `[env]` で渡すので、値を替えると v8 の build script が走り直す（`rerun-if-env-changed`）。

## 未解決事項
1. V8 archive の差し替え（決定 `v8-archive-simdutf`）。人の手順:
   ```sh
   cd /local/celeris/data/scratch/rusty-v8/v150.4.0
   curl -fL -o librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz \
     https://github.com/denoland/rusty_v8/releases/download/v150.4.0/librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz
   echo "f48762ca10d1f1fc605a441c5ae430ec8ce1e9e80f14d78fbc42cb878c30b476  librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz" | sha256sum -c
   gzip -t librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz
   sha256sum librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz >> SHA256SUMS
   # ~/.cargo/config.toml の [env] RUSTY_V8_ARCHIVE を上の file の絶対 path に替える
   ```
   確認: worktree で `cargo nextest run -p celeris-script` が 4 passed になる。
2. serde_json の feature 統合（決定 `serde-json-unify`）。deno_core 0.413.0 の `Cargo.toml` は serde_json を
   `features = ["raw_value", "float_roundtrip", "preserve_order"]` で要求し、optional でないので外せない。workspace をまとめて build すると
   （clippy・nextest の `--workspace`、release gate、S1 の後半で `celeris` binary が celeris-script に依存した後の daemon の build）
   全 crate の `serde_json::Map` が BTreeMap（key の辞書順）から IndexMap（挿入順）に変わり、float の解析も変わる。
   JSON の直列化と sha256 を同じ file で使う crate の file は 130 件あり、key 順に依存する hash があるかはこの run では監査していない（未確認）。
   - 試したこと: deno_core の写しから serde_json の 3 feature を外し `--config patch.crates-io.deno_core.path=…` で当てた。
     `cargo tree --workspace -e features -i serde_json` には `preserve_order` が出なくなり 4 試験も通ったが、`cargo clippy --workspace` の
     task-api は `preserve_order` 入りの serde_json の artifact で検査され、同じ 5 件で落ちた。理由は未確認。

## 提案
- 境界 ADR (e) の表の `v8` の行に「deno_core は `simdutf` feature を必須にするので archive は `librusty_v8_simdutf_release_<target>.a.gz`」と付記する
  （S1 の close unit）。
- 境界 ADR (b) の依存の制約に「deno_core が serde_json の feature を workspace 全体へ広げる」ことと、決定 `serde-json-unify` の答えを付記する。
