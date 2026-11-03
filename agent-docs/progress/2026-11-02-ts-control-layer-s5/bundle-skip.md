---
title: bundle-skip — bundle の無い・古い環境で bundle を読む試験が黙って SKIP・誤って落ちないようにする
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
updated: 2026-11-03
completed: 2026-11-03
---
# bundle-skip — bundle の無い・古い環境で bundle を読む試験が黙って SKIP・誤って落ちないようにする

## 結論
境界 ADR 付記 S5-9（S4 未解決 8）の方式を実装した。
- `test-affected.sh` は、対象 crate に `celeris` か `celeris-script` が入るとき、control-ts の source から使い捨ての dir に bundle を作る。その dir を `CELERIS_SCRIPT_BUNDLE_DIR` で渡し、`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` を立てる。task の根の `control-ts/dist` は読まず、書き換えもしない。
- `control-ts/dist` を読むときは、dist の `inputs.sha256`（新設）を作業ツリーの source から計算した値と比べる。合わない dist は古いとみなして SKIP する。このため、古い dist で本物の host の試験が落ちることはない。
- SKIP は `CELERIS_SCRIPT_SKIP_LOG` に 1 行ずつ記録する。`test-affected.sh` は `script bundle SKIP: N 件` を出し、自分で作った bundle で N > 0 なら exit 1 にする。
- 評価器・安全検査は変えていない。manifest の形（`celeris.script-manifest/1`）も変えていない。

## 変更
commit `phase s5: bundle-skip: 古い・無い dist で bundle を読む試験が誤らないようにする` と、続く記録・clippy 対応の commit。
- `control-ts/scripts/build-bundle.mjs`: dist に `inputs.sha256` を書く。値は、`src/` の下の全 file と `bundle.config.json`・`package.json`・`pnpm-lock.yaml` を 1 行 `<sha256>  <control-ts からの相対 path>\n` で path 昇順に並べた本文の sha256。build が失敗したときは古い `inputs.sha256` も消す。`inputsSha256` を export した。
- `control-ts/scripts/build-bundle.test.mjs`: 試験を 2 件足した（dist の `inputs.sha256` が `inputsSha256()` と一致すること、5 file の固定値 `6027c7bb…` と、source を変えると値が変わること）。失敗時に `inputs.sha256` が残らないことの検査も足した。
- `crates/celeris/tests/support/script_compare.rs`:
  - `dist_dir()` は、`CELERIS_SCRIPT_BUNDLE_DIR` があればその dir を返し、無ければ `control-ts/dist` を返す。`deploy_bundle(&dist_dir(), …)` を使う 3 か所（cos_digest 2・lane_rules 1）も、判定と同じ dir を読む。
  - `dist_bundle`: `CELERIS_SCRIPT_BUNDLE_DIR` がある場合、使えない bundle は失敗にする。無い場合は `check_dist`（領域・capability の照合に `inputs.sha256` の照合を加えたもの）で判定する。
  - `usable_bundle` を純粋な判定 `check_bundle`（`Result`）と、SKIP・失敗・log を扱う `take_bundle`／`take_bundle_logged` に分けた。分けたのは、helper の単体試験の意図した SKIP を log に数えないため。
  - `control_ts_inputs_sha256` は build-bundle と同じ計算をする。`CELERIS_SCRIPT_CONTROL_TS_DIR` は control-ts の root を差し替える試験専用の変数で、使うのは shell 試験だけ。
- `crates/celeris/tests/script_compare_route_priority.rs`: 古さの試験を `check_bundle` に移し、試験を 3 件足した。source を変えた dist・`inputs.sha256` の無い dist を使わないこと、dist の無い環境で SKIP が log に 2 行残り require では失敗すること、Rust と JS の hash が固定値で一致すること。
- `crates/celeris/tests/script_compare_{lane_rules,cos_digest}.rs`: 先頭の注釈だけを直した。
- `scripts/dev/test-affected.sh`: bundle の作成（`build_script_bundle`。node_modules が無ければ offline で install する）、変数の受け渡し、SKIP の集計を足した。pnpm の決め方は `pnpm_cmd` にまとめた。bundle を作れないとき、次の path の変更があれば exit 1 にする: `control-ts/`・`docs/api/script-host/`・`crates/script-runtime/`・`crates/celeris/tests/script_*`・`crates/celeris/tests/support/`・`crates/celeris/src/daemon/script_*`・`test-affected.sh`。それ以外の変更では変数を渡さず、理由を 1 行出して進む。
- `scripts/dev/tests/script-bundle-fresh.sh`（新規。引数なし）: 下の証拠の 10 項目を確かめる。

`crates/script-runtime/tests/script_host_runner.rs` は試験の中で手書きの bundle を作り、`control-ts/dist` を読まない。そのため ADR S5-9 (1) で名前の挙がったこの file は変えていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 条件 0（古い manifest の dist で落ちない）と条件 1（REQUIRE=1 で SKIP 0）: `bash scripts/dev/tests/script-bundle-fresh.sh` → exit 0、全 10 項目 ok。
  - (a) dist が無い: 本物の `script_compare_*` が exit 0 で、SKIP 12 件が log に残る。REQUIRE=1 では失敗する。
  - (b) 偽の古い dist（領域・capability は今の config と同じ、本文は常に例外を投げる、`inputs.sha256` が合わない）: exit 0 で、SKIP 12 件はすべて `inputs.sha256` の不一致による。REQUIRE=1 では失敗する。
  - (c) (b) と同じ dist で `inputs.sha256` だけを正しい値にすると、例外を投げる本文が使われて試験が落ち、SKIP は 0 件。(b) の SKIP が hash の照合によることを示す。
  - (t1)〜(t5) test-affected.sh の結線（git fixture と偽の cargo・node）。bundle を使い捨ての dir に作り、`CELERIS_SCRIPT_BUNDLE_DIR` と REQUIRE=1 を渡す。`control-ts/dist` は変わらない。作った bundle で SKIP 1 件なら exit 非 0。bundle に届く変更で作れなければ cargo の前に非 0。届かない変更では変数を渡さずに進む。celeris・celeris-script を選ばない変更では bundle を作らない。
- 新しい bundle を使った本物の試験: `CELERIS_SCRIPT_BUNDLE_DIR=<作った dir> CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris -E 'binary(/^script_/)'` → 40 passed、SKIP log は 0 行。同じ試験を dist も変数も無しで走らせると 40 passed、SKIP log は 12 行。
- 条件 2: `bash scripts/dev/tests/test-affected-list.sh` → `test-affected-list: ok`（exit 0）。ほかの shell 試験も通った: `ts-verify-classify.sh` 45 passed、`script-runner-cache-key.sh` 35 passed、`edit-activate-policy-ts.sh` 90 passed、`test-parallel-fail-names.sh`・`progress_index_commit_hashes.sh`・`progress_transition_merge.sh`・`progress_union_merge.sh` all passed。いずれも bash で実行し exit 0。
- 条件 3:
  - `bash scripts/dev/test-affected.sh` → exit 0。対象は celeris・celeris-script・celerisctl。control-ts は typecheck と unit 試験（vitest 100 passed、node --test 13 passed を含む）。`ts-verify.sh --grade` は rust の変更のため従来の経路へ回った。`test-ts-verify.sh` は 18 passed。bundle を作り、nextest 949 passed・0 skipped、`script bundle SKIP: 0 件`。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
  - `cargo fmt --all -- --check` → exit 0。
  - 追加で `cargo clippy -p celeris --test script_compare_route_priority --test script_compare_cos_digest --test script_compare_lane_rules -- -D warnings` も error 0（`.err().expect()` を `expect_err` に直した）。

## 作業中の環境の注意
- この WU の `CARGO_TARGET_DIR` は別 WU の target から seed されていた。`v8` の build script の出力が別 WU の dir（`gn_out/obj`）を指していたため、link で `rusty_v8` が見つからなかった。`cargo clean -p v8` で自分の target の v8 だけを作り直して解消した。

## 未解決
1. release gate（`scripts/selfdeploy/release.sh`）には同じ方式を入れていない（ADR S5-9 の最後の項で S5 では決めないとした）。release gate の全試験は `control-ts/dist` を読み、無い・古いときは SKIP する。
2. bundle を作るたびに node で build-bundle を走らせる（今の host で数秒）。時間は測っていない（未確認）。

## 提案
- release gate でも、test-affected と同じく使い捨ての dir に bundle を作り、`CELERIS_SCRIPT_BUNDLE_DIR` を渡す。そうすれば、release の試験でも bundle を読む試験の SKIP を 0 件にできる。
