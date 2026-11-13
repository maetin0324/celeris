---
title: 健全性v2 G4 fix-v8-env — 統合の target の v8 の link-search が消えた scratch seed の世代を指す build 環境の不具合を確かめて記録する
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: done
updated: 2026-11-11
---
# 健全性v2 G4 fix-v8-env — 統合の target の v8 の link-search が消えた scratch seed の世代を指す build 環境の不具合を確かめて記録する

完了日: 2026-11-11

段 sync の統合（integrate-sync）が落ちた原因は build 環境（task の `CARGO_TARGET_DIR`）にあり、repo の欠陥ではない。この unit は repo のコード・試験・検査器・policy を変えていない。変えたのはこの file と task の進捗 [2026-11-10-health-v2-g4.md](../2026-11-10-health-v2-g4.md) だけ。基準は `$CELERIS_WU_BASE`（task は `git merge-base HEAD main`）。

## 症状
integrate-sync の check のうち、fix-sync の `cargo nextest run -p task-dispatch -E 'test(health_generic_)'` が v8 の lib のコンパイルで `could not find native static library rusty_v8` になり、exit 101 で落ちた。同じ統合の test-affected（health を含む）は 3744 件すべて合格し、clippy・fmt・文書検査は exit 0 だった。

## 原因
- 統合の check が使う task の target は scratch seed から作られた（target の隣の `target-origin.json` が `origin: seed` で、写し元は seed の古い世代の `target`）。
- v8 crate の build script の出力 `debug/build/v8-*/output` には `cargo:rustc-link-search=<seed の世代>/target/debug/gn_out/obj` が書かれている。cargo は写した target の build script の出力のうち、`root-output` に記録した OUT_DIR の prefix だけを新しい場所に付け替える。`gn_out` は OUT_DIR の外（`target/debug/gn_out`）にあるので付け替わらず、seed の世代の path のまま残る。
- v8 の build script の入力は変わっていないので、cargo は build script を再実行せず、残った出力をそのまま使う。
- main が進んで seed が新しい世代に替わり、古い世代の dir が消えた。その後に v8 の lib を再コンパイルすると、link-search の dir が無いので rustc が `rusty_v8` を見つけられない。
- 確認: seed の pool の下には `checkout`・`current` と現行の世代の dir 1 つしか無く、task の target の `target-origin.json` が指す古い世代の dir は無い。

## planner run の対処
G4 の計画 v8 の planner run が、task の target で `cargo clean -p v8` を実行した（この unit は実行していない）。この unit の開始時に、task の target の `debug/build/` に `v8-*` の dir が無いことを確かめた。次に v8 を build するときは、`~/.cargo/config.toml` の `RUSTY_V8_ARCHIVE`（`/local/celeris/data/scratch/rusty-v8/v150.4.0/` の archive）から offline で作り直せる。

## この unit の検査（提出時の HEAD で実行）
この unit の `CARGO_TARGET_DIR` は WorkUnit 用の target で、`target-origin.json` は `origin: adopted`（別 task の target を引き継いだもの）。

1. link-search の dir の存在: `for f in "$CARGO_TARGET_DIR"/debug/build/v8-*/output; do grep -oP '(?<=rustc-link-search=)(native=)?\K.*' "$f" | while read -r p; do [ -d "$p" ] && echo "EXISTS $p" || echo "MISSING $p"; done; done` → link-search は 1 件で、`EXISTS`（現行の seed の世代の `target/debug/gn_out/obj`。`librusty_v8.a` が在る）。`MISSING` は 0 件。消えた dir が無いので `cargo clean -p v8` は実行しなかった。nextest の後にもう一度実行し、同じ結果（`EXISTS` 1 件、`MISSING` 0 件）だった。
2. `cargo nextest run -p task-dispatch -E 'test(health_generic_)'` → exit 0。build に 1 分 36 秒（`Compiling v8 v150.4.0` を含み、link の失敗は無い）。`28 tests run: 28 passed, 1664 skipped`（2 binaries、12.7 秒）。
3. 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` と `git diff --name-only $CELERIS_WU_BASE HEAD` → 変更はこの file と `agent-docs/progress/2026-11-10-health-v2-g4.md` の 2 path。
4. 文書検査: `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）。`sh scripts/dev/progress-index.sh --check` → exit 0。`python3 scripts/dev/check-human-text.py --root .` → exit 0（0 findings）。
5. `bash scripts/dev/test-affected.sh` と `cargo fmt --all -- --check` の結果は下の「repo の検査」。

## repo の検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0。`test-affected: 試験対象の変更なし（docs のみ等）`。続けて走る health の試験は `Ran 94 tests`、`OK (expected failures=2)`（G1・G2）、台本 7 件 OK、`repo_health_gate: all checks passed`。
- `cargo clippy --workspace -- -D warnings` → exit 0（52 秒）。
- `cargo fmt --all -- --check` → exit 0。

注意: この unit の target の link-search も seed の世代の dir を指している。今は現行の世代なので在るが、seed がもう一度新しい世代に替わると同じ失敗が起きる。

## 変更（commit の件名）
- `phase 1: record fix-v8-env cause and build environment checks`

## 未解決
- seed の世代交代で、seed から作った（または引き継いだ）target の v8 の link-search が消えた path を指す欠陥は残っている。G4 の範囲外。

## 提案
1. scratch seed の仕組みの側で直す別 task を起こす（G4 の範囲外）。owner は `crates/task-worker/src/scratch/`（`reflink.rs` の `seed_target`、`seed.rs` の世代の入れ替え）。案は次のどちらか。
   - (a) seed から target を写すとき（と target を引き継ぐとき）に、`debug/build/*/output` の `rustc-link-search` が target の外の seed の世代を指す package を見つけたら、その package の build の出力を消す（`cargo clean -p <package>` と同じ効果）。
   - (b) seed の古い世代を消す前に、その世代を指す target が残っていないかを確かめ、残っていれば消さない。
   (a) は target の側だけで閉じ、世代が消えた後でも直せるので (a) を推す。決定的な試験は、一時の seed と target を作り、`output` に seed の世代の path を書いてから世代の dir を消し、写した target の build が通ることを確かめる形にできる。
