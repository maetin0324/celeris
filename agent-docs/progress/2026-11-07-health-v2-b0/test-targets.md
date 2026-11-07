---
title: 健全性v2 B0 test-targets — Cargo test target の列挙と test build/link・edit→verify 時間の基準
tasks: [01M6TYVJSPEYA2P8B7G56HSQ73]
status: done
updated: 2026-11-07
---

# 健全性v2 B0 test-targets

## 結論
- workspace の Rust の test target（`cargo metadata` の kind=test）は 352 個。全部が `<crate>/tests/` 直下の file で、入れ子の target（`tests/<dir>/main.rs`）と `tests/` の外の target は 0 個。
- `tests/` の下の `.rs` は 373 個。差の 21 個は target にならない支援 module（`tests/common/*`・`tests/support/*` など）。
- 過去の集計は両者を混ぜていた。`source-size-report.py` の `rust_external_test` は 788 file で、内訳は test target 352・`tests/` の支援 module 21・`src/` の中の試験 module（`tests.rs`・`*_tests.rs` など。lib/bin の unittest binary に入り、target ではない）412・ルートの `tests/support/*.rs` 3。788 を「結合試験の数」と読むと約 2.2 倍になる。
- 主要 5 crate の test binary 数（`cargo nextest list --list-type binaries-only` の実数）は metadata からの予測と全部一致した: task-core 3・task-dispatch 2・task-api 114・task-worker 25・celeris 57。

## 基準と環境
- 基準: 着手時の `git merge-base HEAD main`（件名「repair: main 取り込みの衝突解消の import を整形する」）。SHA は test-targets.json の `base.sha`。
- host: 24 CPU、Linux 6.8.12-9-pve、cargo 1.98.1 と cargo-nextest 0.9.146（版は JSON の `host`）。CARGO_TARGET_DIR は celeris が渡した task の scratch（開始時 1.3G、依存の一部は未 build）。
- load average は各計測の前後に `/proc/loadavg` を記録した。計測中の cargo 自身の並列 build を含み、他 process の負荷と分けていない。計測中の 1 分値は 2〜36。

## 手順（同じ手順で再計測できる）
1. `python3 scripts/dev/test-target-inventory.py --json`（raw: `test-targets-raw/inventory.json`）
2. `bash agent-docs/progress/2026-11-07-health-v2-b0/test-targets-measure.sh build <crate> - <out>`: `cargo nextest run -p <crate> --no-run` の秒数、`Compiling` 行の数、binary 数。
3. `... test-targets-measure.sh edit <crate> crates/<crate>/src/lib.rs <out>`: lib.rs の末尾にコメントを 1 行足す → `CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list`（対象数と秒数） → 対象 crate 全部の `cargo nextest run -p … --no-run`（再 build 秒数） → `git checkout` で戻す。
4. 全 workspace の試験の実行と並走負荷はしていない。試験は実行せず、build と link だけを測った。

## 結果（秒）
| crate | build --no-run（1 回目） | test binary | --list の対象 crate | 再 build R1 | R2 | R3（settle 後） |
|---|---|---|---|---|---|---|
| task-core | 44.5 | 3 | 14（全 crate） | 144.0 | 139.3 | 142.1 |
| task-dispatch | 91.4 | 2 | 5 | 86.7 | 94.9 | 79.7 |
| task-api | 84.9 | 114 | 7 | 113.0 | 105.4 | 113.3 |
| task-worker | 54.7 | 25 | 7 | 135.7 | 134.6 | 98.0 |
| celeris | 82.2 | 57 | 2 | 32.7 | 81.7 | 84.9 |

- `--list` 自体は 0.10〜0.13 秒。task-api と task-core の変更では web の typecheck+unit も対象になる。task-core の変更では test-ts-verify も対象になる。これらは未計測（下記）。
- build の 1 回目の秒数は依存の build を含む（task-core 以外は前の crate の build の後に測ったので、共有の依存は済んでいた）。

## 数値の読み方（交絡）
- 戻した編集は mtime を変えるので、その crate は次の計測で dirty になる。R1・R2 の task-worker・task-dispatch・celeris（R2）はこの分を含む。
- `-p` で選ぶ crate の組が前回と違うと、feature の統合が変わって workspace の crate が作り直される。R3 は 14 crate 全部の組で settle（144 秒）した後に測った。それでも celeris は 12 crate、task-dispatch は 8 crate をコンパイルした（JSON の `compiled`）。test-affected.sh の実運用でも、前回と違う組で走ると同じことが起きる。
- 交絡のない値は 2 つ。celeris の R1（同じ組の直後。celeris と celerisctl の 2 crate だけ、32.7 秒）と、task-core の 3 回（毎回 14 crate 全部、139〜144 秒）。それ以外の行は上限として読む。

## 未計測と理由
- task-dispatch・task-api・task-worker の交絡のない 1 crate 編集の値: 各計測の前に同じ組で settle する必要がある。1 回の run の壁時計（3600 秒）に収まらなかった。
- web の typecheck+unit と test-ts-verify の時間: 今回の範囲（Rust の test build/link）の外。
- 試験の実行時間: 全 workspace の試験は release gate だけで走らせる規則のため、測っていない。

## 証拠コマンド（提出時の HEAD で実行）
- `python3 -m unittest scripts/tests/test_test_target_inventory.py` → 9 tests OK
- `bash scripts/dev/test-affected.sh` / `cargo clippy --workspace -- -D warnings` / `cargo fmt --all -- --check` → 結果は WU の報告に書く

## commit
- phase B0: test target と tests/ の支援 file を分けて数える test-target-inventory.py を足す
- phase B0: test target と test build/link・edit→verify 時間の基準を記録する

## 提案
- `source-size-report.py` の `rust_external_test` を「test target」「tests/ の支援 module」「src の試験 module」に分けて出す（loc-classes WU の分類と合わせる）。
- 前回と違う crate の組で test-affected.sh が走ると workspace の crate が作り直される。この cost を減らす案（workspace の feature の統一など）を後続の工程の候補にする。
