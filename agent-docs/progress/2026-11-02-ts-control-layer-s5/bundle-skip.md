---
title: bundle-skip — bundle の無い・古い環境で bundle を読む試験が黙って SKIP・誤って落ちないようにする
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
updated: 2026-11-03
completed: 2026-11-03
---
# bundle-skip — bundle の無い・古い環境で bundle を読む試験が黙って SKIP・誤って落ちないようにする

## 結論
S4 未解決 8 を解いた。bundle を読む本物の host の試験（`crates/celeris/tests/script_compare_*`）は、`control-ts/dist` を読まない。試験の support が control-ts の source から `$TMPDIR` に bundle を作って読む。
- task の根に古い dist が残っていても、試験はそれを読まないので落ちない。dist が無い環境でも、node と `control-ts/node_modules` があれば SKIP しない。
- bundle を作れない（node が動かない等）ときは SKIP する。`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` では SKIP せず失敗にする。
- `test-affected.sh` は `celeris` か `celeris-script` を試すとき bundle を作り、`CELERIS_SCRIPT_BUNDLE_DIR` と `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` を渡す。SKIP の件数を出し、作った bundle で SKIP が 1 件以上あれば exit 1 にする。
- 評価器・安全検査・manifest の形（`celeris.script-manifest/1`）・`control-ts/` の中身は変えていない。

## 1 回目の run からの変更
1 回目の run は check で 2 件落ちた。
1. 範囲 check: `control-ts/scripts/build-bundle.mjs`（dist に `inputs.sha256` を書く）と、その試験は範囲外だった。2 つとも base の内容に戻した。
2. `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris -E 'test(/script_compare/)'` が、dist の無い worktree で失敗した。1 回目は REQUIRE=1 で dist が無いと失敗にしていた。今回は試験が source から bundle を作るので、dist が無くても通る。

このため、境界 ADR 付記 S5-9 の (1)（変数が無ければ `control-ts/dist` を読む）と (2)（dist の `inputs.sha256` を照合する）とは違う方式にした。ADR は範囲外なので書き換えていない（下の「提案」）。(3)〜(5) はそのまま。

## 変更
- `crates/celeris/tests/support/script_compare.rs`
  - `dist_bundle`: `CELERIS_SCRIPT_BUNDLE_DIR` があればその dir を読み、使えなければ失敗にする。無ければ `fresh_bundle_dir()` を読む。作れない・古い bundle は SKIP する。REQUIRE=1 なら失敗にする。`control-ts/dist` は読まない。
  - `fresh_bundle_dir` / `build_bundle_cached`: `$TMPDIR/celeris-script-bundle/<内容 hash>` に `build-bundle.mjs` の `buildBundle({ outDir })` で bundle を作る（node で実行）。同じ hash の dir があれば作らない。nextest は試験ごとに process を分けるので、`<hash>.lock` を `create_new` で取った process だけが作り、ほかの process は出来上がるのを待つ（上限 300 秒）。作業用の dir を作ってから rename する。
  - `control_ts_inputs_sha256`: 内容 hash の計算元は `src/` の下の全 file、`bundle.config.json`・`package.json`・`pnpm-lock.yaml`・`scripts/build-bundle.mjs`、host の schema（`docs/api/script-host/host-v1.schema.json`）。
  - `dist_dir()` は判定に使ったのと同じ dir（変数の dir か、作った dir）を返す。`deploy_bundle` に渡す 3 か所はこれを使う。
  - SKIP の記録（`take_bundle`／`take_bundle_logged`、`CELERIS_SCRIPT_SKIP_LOG`）は 1 回目と同じ。
- `crates/celeris/tests/script_compare_route_priority.rs`: 古さの判定の試験 4 件は `check_bundle` を直接呼ぶ形のまま残した。次の 2 件を足した。
  - `script_compare_fresh_bundle_follows_source`: 偽の control-ts で、作った dir の名前が内容 hash であること、同じ source なら作り直さないこと、`src/`・`build-bundle.mjs`・schema のどれを変えても hash が変わって作り直すこと、作業用の dir と lock が残らないことを確かめる。node が動かなければ SKIP する。
  - `script_compare_unbuildable_bundle_is_skipped_and_logged`: build が失敗すると SKIP になり、log に 1 行ずつ残ること、require では失敗することを確かめる。
- `crates/celeris/tests/script_compare_{lane_rules,cos_digest}.rs`: 先頭の注釈を直しただけ。
- `scripts/dev/test-affected.sh`: 1 回目と同じ（bundle を作り、変数を渡し、SKIP を数える）。
- `scripts/dev/tests/script-bundle-fresh.sh`（新規。引数なしで実行）: 1 部は本物の試験を control-ts の写しに向けて走らせる（写しは git の repo にし、node_modules は作業ツリーへの symlink にする。`CELERIS_SCRIPT_CONTROL_TS_DIR` で写しを指す）。
  - (a) dist が無い。
  - (b) 偽の古い dist（領域・capability は config と同じで、本文は常に例外を投げる）がある。
  - (c) (b) の偽の dist を `CELERIS_SCRIPT_BUNDLE_DIR` で読ませると落ちる。(b) で偽の dist を読んでいないことの裏付け。
  - (d) node が動かない。

  2 部（t1〜t5）は `test-affected.sh` の結線を試す。内容は 1 回目と同じ。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 範囲 check（計画の 1 本目と同じ command）: 範囲外の path は 0 件。
- 条件 0・1:
  - `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris -E 'test(/script_compare/)'`（この worktree には `control-ts/dist` が無い）: exit 0。25 passed。
  - `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris -E 'binary(/^script_/)'`: 39 passed、0 skipped。
- 条件 2:
  - `sh scripts/dev/tests/script-bundle-fresh.sh`: exit 0、全 11 項目 ok。
    - (a) dist が無い: exit 0、SKIP 0。REQUIRE=1 でも exit 0。
    - (b) 偽の古い dist: exit 0、SKIP 0。REQUIRE=1 でも exit 0。dist は書き換わらない。
    - (c) 偽の dist を読ませると、その例外で落ちる。
    - (d) node が動かない: exit 0、SKIP 13 件が log に残る。REQUIRE=1 では失敗する。
    - (t1)〜(t5): ok。
  - `bash scripts/dev/tests/test-affected-list.sh`: `test-affected-list: ok`、exit 0。
- 条件 3:
  - `bash scripts/dev/test-affected.sh`: exit 0。対象は celeris・celeris-script・celerisctl。bundle を作り、nextest は 948 passed・0 skipped。`script bundle SKIP: 0 件`。`test-ts-verify.sh` も走った。
  - `cargo clippy --workspace -- -D warnings`: exit 0。
  - `cargo fmt --all -- --check`: exit 0。
  - 追加で、試験の 3 binary に `cargo clippy -p celeris --test …`（route_priority・cos_digest・lane_rules）を走らせた。error は 0 件。

## 未解決
1. release gate（`scripts/selfdeploy/release.sh`）の全試験も、試験の support が source から bundle を作る。release の環境に node と `control-ts/node_modules` が無ければ SKIP する。release で REQUIRE=1 を立てるかは決めていない（ADR S5-9 の最後の項）。
2. bundle を作る時間は測っていない（未確認）。同じ source なら `$TMPDIR` の中で使い回す。`test-affected.sh` の nextest は 35.8 秒だった。
3. `test-affected.sh` も bundle を作って `CELERIS_SCRIPT_BUNDLE_DIR` で渡すので、作るのが二重になっている（試験の support は渡された dir を使い、自分では作らない）。test-affected で作るのは、node_modules を offline で入れることと、作れない理由を cargo の前に出すため。

## 提案
- 境界 ADR 付記 S5-9 の (1)(2) を実装に合わせて直す。直す後の内容は「試験は `control-ts/dist` を読まない。`CELERIS_SCRIPT_BUNDLE_DIR` が無ければ、control-ts の source の内容 hash を名前にした `$TMPDIR` の dir に bundle を作って読む。`inputs.sha256` は使わない」。ADR の file はこの unit の範囲外なので、s5-close で直す。
