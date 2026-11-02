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
  `runtime_sync`・`runtime_async`・`runtime_exception`・`runtime_busy` の 4 試験が通った（2 回目の run でも提出時の HEAD で 4 passed）。
  ただし通るのは、simdutf 版の V8 archive を run の `$TMPDIR` に取得し `RUSTY_V8_ARCHIVE` を一時的に上書きしたとき。
- 人が置いた archive（非 simdutf 版）では試験 binary の link が落ちる。deno_core 0.413.0 の `Cargo.toml` が
  `deno_v8` を `features = ["simdutf"]`・`default-features = false` で使うため（決定 `v8-archive-simdutf`）。
- deno_core 0.413.0 は serde_json の `raw_value`・`float_roundtrip`・`preserve_order` を必須で有効にする（`[dependencies.serde_json]`、optional でない）。
  workspace をまとめて build すると全 crate の serde_json に効き、`cargo clippy --workspace -- -D warnings` が task-api の
  `clippy::result_large_err` 851 件（`ApiProblem` が 144 byte になる。`extra: Map<String, Value>` が IndexMap になるため）で落ちる（決定 `serde-json-unify`）。
  release gate の `cargo build --release -p celeris -p celerisctl -p celeris-credentiald` の graph には今は `preserve_order` が入らない
  （`cargo tree -p celeris -p celerisctl -p celeris-credentiald -e features -i serde_json | grep -c preserve_order` → 0、`--workspace` → 3）。
  S1 の `daemon-wire` で `celeris` が `celeris-script` に依存した時点で daemon の binary にも効く。

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
| rustc の版 | `rustc --version` | rustc 1.98.1（2026-09-01）、cargo 1.98.1。`cargo build -p celeris-script` は警告なしで通る |
| 4 試験 | `RUSTY_V8_ARCHIVE=$TMPDIR/v8dl/librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz cargo nextest run -p celeris-script` | exit 0、4 tests run: 4 passed |
| 4 試験（host の設定のまま） | `cargo nextest run -p celeris-script` | exit 101。link 失敗: `wild: error: Undefined symbol simdutf__utf8_length_from_latin1, referenced by libdeno_core-*.rlib` |
| debug profile で release archive を link | 上の 4 試験（`test` profile は debug、archive は `release`） | link でき、試験が通る（simdutf 版のとき） |
| deno_core を推移的に持つ crate | `cargo tree -i deno_core --workspace -e normal --prefix none \| awk '{print $1}' \| sort -u` | `celeris-script`・`deno_core` だけ |
| build.rs | `ls crates/script-runtime` | `Cargo.toml src tests`（build.rs 無し） |
| 版の衝突 | `cargo tree -d -p celeris-script -e normal` | 複数版: `getrandom` 0.3.4/0.4.3、`syn` 2.0.119/3.0.5（どちらも workspace に既にあった組。新しい版の重複は 0）。`serde`・`serde_core`・`smallvec` は同じ版が host 用と target 用で 2 回出るだけ |
| fmt | `cargo fmt --all -- --check` | exit 0 |
| clippy（crate） | `cargo clippy -p celeris-script --tests -- -D warnings` | exit 0 |
| clippy（workspace） | `cargo clippy --workspace -- -D warnings` | exit 101: task-api の `result_large_err` 851 件（関数 556・closure 295、`ApiProblem` 144 byte）。`crates/script-runtime` を members から外すと 0 件 |
| test-affected | `bash scripts/dev/test-affected.sh` | 未実行。Cargo.lock の変更で全 crate と web が対象になり、上の 2 件の決定で結果が変わるため |

## V8 archive の使い方
- v8 150.4.0 の build.rs は `RUSTY_V8_ARCHIVE` があればその file を `target/<profile>/gn_out/obj/librusty_v8.a` に展開して使い、
  network に出ない（build の stderr に `static lib URL: <path>` と `Detected GZIP archive` が出る）。src_binding は crate 同梱の
  `gen/src_binding_simdutf_release_x86_64-unknown-linux-gnu.rs` を使うので `RUSTY_V8_SRC_BINDING_PATH` は要らない。
- 必要な archive は `librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz`（38,799,023 byte、
  sha256 `f48762ca10d1f1fc605a441c5ae430ec8ce1e9e80f14d78fbc42cb878c30b476`）。GitHub の release v150.4.0 の asset digest と一致することを
  前の run で確かめ、この run でも取得して sha256 と `gzip -t` が合った。
- 人が置いた `librusty_v8_release_x86_64-unknown-linux-gnu.a.gz`（sha256 `5869fdb7071b1f21f200a97ce97d20125877e0f31cd802ed99de2c53f488b06c`）は
  simdutf 無しの版で、deno_core 0.413.0 とは link できない。
- `RUSTY_V8_ARCHIVE` は cargo の `[env]` で渡すので、値を替えると v8 の build script が走り直す（`rerun-if-env-changed`）。

## serde_json `preserve_order` の影響の監査（この run で読んだ範囲）
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
2. serde_json の feature 統合（決定 `serde-json-unify`）。選択肢:
   - 受け入れる: `ApiProblem.extra` を `Box` に入れて clippy を通し、上の「変わる」4 か所を key の辞書順で直列化する形に直して hash の値を保つ。
     残り 70 件の監査は `daemon-wire` の前に別 unit で行う。
   - deno_core の写しを repo に置いて 3 feature を外す（`[patch.crates-io]`）: workspace の JSON の意味は変わらないが、
     deno_core の版を上げるたびに写しを作り直す。前の run で写しに patch を当てて 4 試験は通った。
   - 前の run の記録: 写しを `--config patch.crates-io.deno_core.path=…` で当てたとき `cargo clippy --workspace` の task-api は同じ誤りで落ちた。理由は未確認。

## 提案
- 境界 ADR (e) の表の `v8` の行に「deno_core は `simdutf` feature を必須にするので archive は `librusty_v8_simdutf_release_<target>.a.gz`」と付記する
  （S1 の close unit）。
- 境界 ADR (b) の依存の制約に「deno_core が serde_json の feature を workspace 全体へ広げる」ことと、決定 `serde-json-unify` の答えを付記する。
