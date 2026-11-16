---
title: 健全性v2 V0 target-env — 引き継いだ target の v8 の link-search を確かめる script と、統合の失敗の原因の記録
tasks: [01M6TYVJT97XH88TEEAVRD7JK6]
status: done
updated: 2026-11-16
completed: 2026-11-16
---
# 健全性v2 V0 target-env

完了日: 2026-11-16

## 結論
段 verify の統合（integrate-verify）が落ちた原因は build 環境（task の target）にあり、repo の欠陥ではない。planner run の `cargo clean -p v8` の後、task の target の MISSING は 0 件になった。この unit の target にも同じ欠陥が 1 件あり、`cargo clean -p v8` で直して build が通った。この unit は repo のコード・検査器・policy・fixture を変えていない。足したのは script [target-link-check.sh](target-link-check.sh) とこの file だけ。

## 症状
integrate-verify の check のうち、generic の check 1 と `invariants.py` が task-dispatch の build で `could not find native static library rusty_v8` になり、exit 101 で落ちた。

## 原因
- task の target（`/local/celeris/data/scratch/targets/task-01M6TYVJT97XH88TEEAVRD7JK6/target`）の `target-origin.json` は `origin: adopted`、`adopted_from: task-01M6TYVJT7EJ6WWHEF0EBH3JPS`（R6 の target を引き継いだもの）。
- 引き継ぎは `crates/task-worker/src/scratch.rs` の `allocate_locked` が R6 の target の dir を `std::fs::rename` で動かす。動かした後、元の path には何も残らない。
- v8 crate の build script の出力 `debug/build/v8-<hash>/output` には `cargo:rustc-link-search=<R6 の target>/debug/gn_out/obj` が書かれていた。cargo は `root-output` に記録した前の OUT_DIR（`debug/build/v8-<hash>/out`）の文字列だけを新しい場所に付け替える。`gn_out` は OUT_DIR の外にあるので、R6 の path のまま残る。
- v8 の build script の入力は変わっていないので、cargo は build script を再実行せず、残った出力を使う。v8 の lib を再コンパイルすると link-search の dir が無いので、rustc は `rusty_v8` を見つけられない。
- G4 の [fix-v8-env](../2026-11-10-health-v2-g4/fix-v8-env.md)（scratch seed の世代が消えた場合）と同じ欠陥で、今回は target の引き継ぎで起きた。KB の `environment/celeris/scratch-seed-v8-stale-link-search.md` も同じ欠陥を書いている。

## 対処
- 計画 v2 の planner run が task の target で `cargo clean -p v8` を実行した（58 files、449.2MiB）。この unit は task の target を読むだけで、変えていない。
- この unit の target（`$CARGO_TARGET_DIR`。`target-origin.json` は `origin: adopted`、`adopted_from: task-01M6TYVJT97XH88TEEAVRD7JK6/wu-01M7GRJKJN2CTN0NWHJ9DG09NP`）にも MISSING が 1 件あった。v8 の出力の 1 つが、引き継ぎ元の WorkUnit の target の `debug/gn_out/obj`（今は無い）を指していた。この unit は自分の target で `cargo clean -p v8` を実行し（53 files、632.8MiB）、build し直した。

## script
`sh agent-docs/progress/2026-11-15-health-v2-v0/target-link-check.sh <target dir>` は `<target dir>/*/build/*/output` の `rustc-link-search` の値（`native=` などの前置きを外す）を 3 つに分ける。
- 同じ dir の `root-output` の path かその下（cargo が付け替える）。
- それ以外で dir が在る。
- それ以外で dir が無い。`MISSING <path> <- <output>` と出す。

MISSING があれば exit 1、無ければ件数を 1 行に出して exit 0。引数が無い・空なら exit 2。`--self-test` は `$TMPDIR` に偽の target を作り、上の 3 例と空の引数を確かめる。`/bin/sh` の dash で動かした。

## 検査の結果（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
1. `sh agent-docs/progress/2026-11-15-health-v2-v0/target-link-check.sh --self-test` → exit 0。`self-test: 3 cases ok`（root-output の下と、外で在るは MISSING にしない。外で無いは MISSING で exit 1。空の引数は exit 2）。`dash` で直接呼んでも同じ。
2. task の target に当てた（読むだけ）→ exit 0。`outputs=56 link-search=4 under-root-output=4 outside-present=0 missing=0`。v8 の build の出力は無い（planner run が消した）。残る 4 件（aws-lc-sys・libsqlite3-sys 2 件・ring）は root-output の下で、cargo が付け替える。
3. この unit の target に当てた（clean の前）→ exit 1。MISSING 1 件（引き継ぎ元の WorkUnit の target の `debug/gn_out/obj` <- `debug/build/v8-<hash>/output`）。`outputs=58 link-search=6 under-root-output=4 outside-present=1 missing=1`。外で在る 1 件は、もう 1 つの v8 の出力が指す scratch seed の現行の世代の `debug/gn_out/obj`。
4. この unit の target で `cargo clean -p v8` → `Removed 53 files, 632.8MiB total`。
5. この unit の target で `cargo nextest run -p task-dispatch -E 'test(/health_generic_/)'` → exit 0。build は 1 分 36 秒（`Compiling v8 v150.4.0` を含み、link の失敗は無い）。`28 tests run: 28 passed, 1872 skipped`（2 binaries、13.1 秒）。
6. 5 の後にこの unit の target に当てた → exit 0。`outputs=57 link-search=5 under-root-output=4 outside-present=1 missing=0`。v8 の link-search は自分の target の `debug/gn_out/obj`（`librusty_v8.a` が在る）を指す。task の target にもう一度当てて、2 と同じ（exit 0、missing=0）。
7. repo の検査は下の「repo の検査」。

## repo の検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0。`test-affected: 試験対象の変更なし（docs のみ等）`。続けて走る check-health は `repo-health: verdict=pass exit=0`（fail_grade=0、new=0、worsened=0。source-size は doc_only で not_applicable）。
- `cargo clippy --workspace -- -D warnings` → exit 0（49 秒）。
- `cargo fmt --all -- --check` → exit 0。
- 文書検査: `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/progress-index.sh --check`、`python3 scripts/dev/check-human-text.py --root .` → いずれも exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` と `git diff --name-only $CELERIS_WU_BASE HEAD` → 変更は `agent-docs/progress/2026-11-15-health-v2-v0/` の 2 path（この file と `target-link-check.sh`）。`crates`・`web`・`control-ts`・`scripts`・`tests` の差分は 0。

## 変更（commit の件名）
- `health-v2 V0 target-env: 引き継いだ target の link-search を確かめる script と原因の記録`

## 未解決
- target の引き継ぎ（rename）と scratch seed の世代交代で、OUT_DIR の外を指す build 出力（今の repo では v8 の `gn_out/obj`）が消えた path を指す欠陥は残っている。V0 の範囲外（人の回答で、恒久の直しは提案に書くだけと決まった）。
- 引き継ぎ元が消えたとき、v8 の lib が既に compile 済みで再コンパイルされなければ link は起きず、欠陥は表に出ない。表に出るかは cargo の fingerprint 次第で、事前には分からない。

## 提案
1. 別 task で、`crates/task-worker/src/scratch/` の側で直す（owner は `scratch.rs` の `allocate_locked`〈引き継ぎ〉と `scratch/reflink.rs` の `seed_target`〈seed の写し〉）。target を引き継いだ直後と seed から写した直後に、`debug/build/*/output` の `rustc-link-search` のうち、同じ dir の `root-output` の下に無く、引き継ぎ元（または seed の世代）の path を指す package を見つけ、その package の build の出力を消す（`cargo clean -p <package>` と同じ効果）。引き継ぎ元の path は rename の後に必ず無くなり、seed の世代はいずれ消えるので、dir が今在るかどうかでなく、元の場所を指すかどうかで判定する。
2. 決定的な試験は、一時の target を作り、`output` に元の場所の `debug/gn_out/obj` を、`root-output` に元の OUT_DIR を書いてから引き継ぎ（または seed の写し）を行い、その package の出力が消えていて、root-output の下の package の出力は残ることを確かめる形にできる。この script の `--self-test` の 3 例と同じ分け方を使う。
3. 1 が入るまで、統合の check が v8 の link で落ちたら、この script を task の target に当てて MISSING を確かめ、`cargo clean -p v8` で直す（planner か人が実行する）。
