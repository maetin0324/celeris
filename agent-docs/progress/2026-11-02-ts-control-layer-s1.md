---
title: S1 — 埋込み ScriptRuntime（deno_core actor）と最小 Host API、独立 TS bundle の配備経路
tasks: [01M6DJ6QCMCWT80MTYJ7SNE0XK]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S1 — 埋込み ScriptRuntime と最小 Host API、独立 TS bundle の配備経路

完了日: 2026-11-02（task 01M6DJ6QCMCWT80MTYJ7SNE0XK。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- `crates/script-runtime/`（package `celeris-script`、`deno_core =0.413.0` の写し）に、専用 thread が `JsRuntime` を持つ actor、
  watchdog・heap/input/in-flight 上限、Host API `celeris:host` v1、manifest の検証と配備を作った。daemon は `[script]` が無ければ isolate を起こさない。
- TS は `control-ts/` で型検査と bundle を分けて作る。TS の本文を変えて bundle を作り直しても Rust の compile は 0 回だった。
- 許容 overhead（境界 ADR (f)）の 6 項目はすべて閾値内（下の表）。S2 へ進まない基準には当たらない。
- S1 で決めたこと（置き場・配備先・時計と乱数・terminate 後の isolate・V8 archive・`deno_core` の写し・`cargo tree -d`）は
  [境界 ADR の付記](../adr/2026-11-02-ts-control-layer-boundary.md) に書いた。
- 本番 config の書き換えは要らない。host の V8 archive は人が置き済み。release gate での確認は未実施（「本番への移行」）。

## 受け入れ条件ごとの証拠
各 unit の記録は `agent-docs/progress/2026-11-02-ts-control-layer-s1/<unit key>.md`。

| 条件 | 証拠 file | 確かめる command |
|---|---|---|
| 0. sync/async/exception/cancel/timeout/busy queue/restart/権限拒否の統合試験がある | `crates/script-runtime/tests/runtime.rs`（`runtime_sync`・`runtime_async`・`runtime_exception`・`runtime_cancel`・`runtime_timeout_sync_loop`・`runtime_busy`・`runtime_restart`・`runtime_generation_discard`・`runtime_denied_capability`・`runtime_denied_scope`・`runtime_import_rejected` ほか）、`crates/celeris/tests/script_runtime.rs`（daemon の結線 6 本）。記録は crate-skeleton.md・runtime-guard.md・host-api.md・daemon-wire.md | `cargo nextest run -p celeris-script` と `cargo nextest run -p celeris -E 'test(script_runtime_)'` |
| 1. 変更の影響範囲の試験が通る | 各 unit の記録の「証拠」節（daemon-wire の時点で Rust 6963 passed・21 skipped、web unit 697 passed） | `bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check` |
| 2. TS bundle だけの更新で cargo build が走らない | bundle-load.md（`touch` と本文の変更の両方で Compiling 0 行、新しい hash の bundle を同じ binary が load） | `control-ts` で `corepack pnpm@12.6.0 run bundle` の後に `cargo build -p celeris-script --bins -v 2>&1 \| grep -c Compiling` が 0。`grep -rn 'include_str!\|include_bytes!' crates/ \| grep control-ts` が空、`test ! -e crates/script-runtime/build.rs` |
| 3. 既存の Rust 処理が変わらず動く | crate-skeleton.md（workspace の serde_json に `preserve_order`・`float_roundtrip` が入らない）、daemon-wire.md（`[script]` 無しで thread を作らない `script_runtime_absent_no_thread`）、notify-flake.md | `cargo tree --workspace -e features -i serde_json \| grep -cE 'preserve_order\|float_roundtrip'` が 0、`bash scripts/dev/test-affected.sh` |
| 依存の制約（境界 ADR (b) と付記） | daemon-wire.md、overhead.md | `cargo tree -i deno_core --workspace -e normal --prefix none \| awk '{print $1}' \| sort -u` が `celeris`・`celeris-script`・`celerisctl`・`deno_core` だけ |
| schema と生成型が一致する | host-api.md、control-ts.md | `cargo run -q -p celeris-script --bin script-schema \| cmp - docs/api/script-host/host-v1.schema.json`、`control-ts` で `run gen:types` の後 `git diff --exit-code src/host.d.ts` |
| 実 bundle を load できる | bundle-load.md | `cargo run -q -p celeris-script --bin script-load-check -- --bundle-dir control-ts/dist` が exit 0 |
| 許容 overhead | overhead.md、overhead.json | `script-bench --bundle-dir control-ts/dist --iters 1000 --out <file>`（release build）と overhead.md の「測り方」 |

## unit と commit（件名）
- crate-skeleton: `phase 1: celeris-script crate の骨組み（JsRuntime actor と bounded queue の handle）`、
  `phase 1: deno_core の写しを置き serde_json の float_roundtrip・preserve_order を外す（[patch.crates-io]、ADR）`、
  `phase 1: source-size の検査で crates/*/vendor/ の写しを vendored に分類して測らない`、
  `phase 1: check-doc-links の対象から deno_core の写しを外す`、
  `phase 1: campaign の ops_r2 試験の unix socket を /tmp 直下の短い dir に置き、vendor-deno-core.sh --check は network に出ない`、
  ほか進捗の更新 4 件（`phase 1: crate-skeleton の進捗を更新（…）`）
- runtime-guard: `phase 1: runtime-guard: watchdog による V8 中断・cancel・generation の破棄・上限・監視`、
  `phase 1: script-runtime の actor は stats を公開してから返事を送る（runtime_generation_discard の in_flight 競合）`
- host-api: `phase 1: host-api: celeris:host v1 の native op・capability/scope 検査・DTO と JSON Schema・import 拒否`、
  進捗の更新 2 件
- control-ts: `phase 1: control-ts: 型検査と bundle を分けた TS package・生成型・ESM bundle と manifest`、`phase 1: control-ts の進捗に検査の結果を書く`
- bundle-load: `phase 1: bundle-load: manifest 検証と配備、script-load-check と script-bench`
- daemon-wire: `phase 1: daemon-wire: [script] の結線と architecture-map`、
  `phase 1: daemon-wire: celerisctl の release build の実測と plan_issue の再申告を進捗に書く`、
  `phase 1: daemon-wire: 境界 ADR に celerisctl への波及の付記、進捗を done に`、`phase 1: daemon-wire: 進捗に再実行した検査の結果を書く`
- notify-flake: `phase 1: notify-flake: 偽 webhook が受けた接続を必ず応答してから閉じる`
- overhead: `phase 1: overhead: 判定遅延・RSS・release build と binary の増分を測る`
- close: `phase 1: close: 境界 ADR に S1 の付記、S1 の進捗まとめ`

## overhead の結果（overhead.json。境界 ADR (f) の閾値）
| 項目 | 測定値 | 閾値 | 判定 |
|---|---|---|---|
| warm の判定遅延 p50 | 0.087 ms | ≤ 1 ms | 内 |
| warm の判定遅延 p99 | 0.117 ms | ≤ 5 ms | 内 |
| RSS 増分 isolate 1 個 | 24.0 MiB | ≤ 100 MiB | 内 |
| RSS 増分 isolate 2 個 | 31.1 MiB | ≤ 200 MiB | 内 |
| `celeris` の cold release build 増分 | +2.4 秒（132.3 → 134.6） | ≤ 60 秒 | 内 |
| `celeris` の release binary 増分 | +64.2 MiB（79.6 → 143.7） | ≤ 80 MiB | 内（余裕 15.8 MiB） |

- 参考（閾値の対象外）: `celerisctl` の release binary は 36.5 MiB → 93.2 MiB（+56.7 MiB）。
- `task-dispatch`・`task-api` は `deno_core` を持たない（`cargo tree --locked -e normal -p task-dispatch -i deno_core` が「did not match any packages」）。
  test-build の秒は測っていないが、依存の木に V8 が無いので増えない。
- build は共用 host で各 1 回の計測（1 分 load average 9〜14）。V8 は prebuilt の archive を link した。

## 未解決事項
1. Host API の schema の nullable の欠け: `host-v1.schema.json` の `SelftestEcho*.note` が `{"type":"string"}` で null を表せず、生成型も `note: string`
   （control-ts.md 未解決 1）。実行時の DTO は null を受ける。直すときは `crates/script-runtime/src/dto.rs`・schema・生成型・
   `tests/fixtures/selftest-bundle/manifest.json` の `schema_sha256` を同じ変更で直す（bundle-load.md 未解決 1）。
2. bundle を配備する command が無い。`deploy_bundle` は library の関数だけで、人の手順に書ける操作が無い（bundle-load.md 提案 1）。
   S1 では daemon が bundle を判定に使わないので、本番への配備手順は書いていない。
3. release gate（`scripts/selfdeploy/release.sh`）は `control-ts` の bundle を作らない。release gate での V8 archive の使用も未確認（S1 では release を作っていない）。
4. 同じ isolate で並行する要求どうしは、bundle が他の要求の token を保存すれば相手の scope で op を呼べる（host-api.md）。要求ごとに V8 context を分けるかは S2 で決める。
5. bundle の top-level の同期無限ループで daemon の起動が止まらないこと（`LOAD_TIMEOUT` 10 秒）は試験していない（CPU を焼く試験を置かない規則）。
6. `[script] state_dir` を worker の run から書けなくする保護は無い。業務領域を TS に切り替える段より前に要否を決める。
7. 起動時と作り直し時の bundle の評価は 10 秒の watchdog だけで守っている。heap 上限に event loop の中で当たると、未完の全要求に `HeapLimit` を返す。
8. `celeris` の release binary の増分は閾値の 80% を超えている。V8 の feature や TS 側の依存を増やす変更では overhead の計測を再実行する。

## 提案
1. package の分割（案 B）を、業務領域を TS に切り替える段（S2 の `compare` 以降）より前に検討する。V8 を持たない lib の package と
   `celeris` の daemon binary の package に分け、`celerisctl` は lib だけに依存させる。script layer は daemon の `run` へ注入する形にする。
   効果: `celerisctl` から `deno_core`・V8 が外れ、release binary が約 57 MiB 小さくなり、境界 ADR (b) の元の制約（`deno_core` を持つのは
   `celeris-script` と daemon だけ）に戻る。影響: `crates/celerisctl/Cargo.toml`・`scripts/selfdeploy/release.sh`（`-p celeris`）・
   `-p celeris` を使う検査・`crates/celeris/tests/`。影響の全量は未確認。feature で分ける案は `release.sh` の `-p celeris -p celerisctl` で
   feature が統合されるので不採用（daemon-wire.md）。
2. bundle を配備する command（例 `script-deploy --dist <dir> --state-dir <dir>`）を、`[script]` を本番で使う段より前に足す（未解決 2）。
3. `CLAUDE.md` の `test-affected.sh` の説明に `control-ts/` の型検査と試験が走ることを足す（control-ts.md 提案）。この unit は記録だけなので変えていない。
4. `crates/task-dispatch/src/dispatcher/worker_finish.rs`: check が不合格の run の `decisions` も記録する（crate-skeleton.md 提案）。
5. worker の `result.json` に `summary` と `yield.plan_issue` を両方書くと plan_issue が捨てられる。手引きに明記するか plan_issue を優先する（daemon-wire.md 提案 2）。

## 本番への移行
S1 では本番 config を書き換えない。`[script]` は書かない（無ければ isolate を起こさず、既存の挙動のまま動く）。
人が行う確認は次の 2 つ。

1. host の V8 archive の配置（2026-11-02 に人が実施済み。確認だけ）
   - `ls -l /local/celeris/data/scratch/rusty-v8/v150.4.0/librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz` が 38,799,023 byte。
   - `cd /local/celeris/data/scratch/rusty-v8/v150.4.0 && sha256sum -c SHA256SUMS` の simdutf の行が `OK`。
   - `grep -A3 '^\[env\]' ~/.cargo/config.toml` に `RUSTY_V8_ARCHIVE = "/local/celeris/data/scratch/rusty-v8/v150.4.0/librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz"` がある。
   - release を作る user（`release.sh` を動かす user）の `~/.cargo/config.toml` が同じ file であること。別の user で動かすなら、その user の設定にも同じ行を足す。
2. release gate の確認（次に S1 を含む release を作るとき）
   - いつもの手順で `scripts/selfdeploy/release.sh` を走らせ、段 `cargo-clippy`・`cargo-test`・`cargo-build` が通ることを確かめる。
   - V8 が network から取られていないことは、release の build が使った target dir で
     `grep -h 'static lib URL' <target dir>/release/build/v8-*/stderr` が上の archive の path を出すことで確かめる（未確認。S1 では release を作っていない）。
   - archive が無い・simdutf 無しの版を指している場合、`cargo-build` 段が `undefined symbol simdutf__*` か download の失敗で落ちる。そのときは 1. を直してから作り直す。
