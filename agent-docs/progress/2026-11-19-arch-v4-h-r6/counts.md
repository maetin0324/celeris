---
title: アーキ v4 H R6 counts — test-parallel.sh の Summary 解析を直し gate.json の cargo_test に nextest の件数を入れる
tasks: [01M7VJY8Q1MMAH87CSPQZWC7VY]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# counts（WorkUnit）

完了日: 2026-11-20

## 結論

- [ADR 2026-11-20 release-record-summary-log](../../adr/2026-11-20-release-record-summary-log.md) の D1 を実装した。
  `scripts/dev/test-parallel.sh` の集計の python は、nextest の Summary 行の括弧の注記（`(1 slow)`・`(3 slow, 1 leaky)` など）を
  外してから `<数> <語>` の組に割る。`CELERIS_TEST_SUMMARY`（＝gate.json の `cargo_test`）に `nextest_passed`・`nextest_skipped`・
  `doc_passed`・`doc_ignored` を足した。`passed`・`failed`・`ignored` は nextest と doc-test の和のまま。
- 既存の `scripts/selfdeploy/tests/release_parallel_test_gate.sh` は base で落ちていた。fixture を直して exit 0 にし、
  新しい試験の file を足さずに期待値を広げた（下の「変えた試験」）。
- `docs/ops/selfdeploy.md`（gate.json の `cargo_test` の欄）と `docs/ops/nextest.md`（集計の節と欄の表）を新しい欄に合わせた。
- `scripts/selfdeploy/release.sh`・`crates/`・policy の file は変えていない。本番の release.sh・verify.sh・promote.sh は走らせていない。

## commit（件名）

- 「counts: test-parallel.sh の Summary 行の注記を外して数え、cargo_test に nextest と doc-test の内訳を足す」

## 変えた試験（`release_parallel_test_gate.sh`）

- fixture の不足（base で落ちた原因）: 偽の木に `scripts/dev/lib/check-slot.sh`（ADR 2026-11-12-host-check-slots の後に
  test-parallel.sh が 55 行で読む）を写し、`scripts/selfdeploy/cos-skills.txt` を注釈 1 行だけで置いた（`sd_bundle_cos_skills` が読む。
  `release_web_required.sh` と同じ置き方）。試験の不変条件は変えていない。
- 偽の cargo の Summary 行を本物と同じ注記付きにした: 通る場合 `7 tests run: 7 passed (1 slow), 2 skipped`、
  失敗の場合 `7 tests run: 6 passed (1 slow, 1 leaky), 1 failed, 2 skipped`。
- 場合 1 の dict の一致に `nextest_passed` 7・`nextest_skipped` 2・`doc_passed` 1・`doc_ignored` 0 を足した（`passed` 8・`ignored` 2 は同じ）。
- 場合 2（nextest の失敗）の期待値を dict の一致にし、`passed` 7・`failed` 1・`ignored` 2・`nextest_passed` 6・`nextest_skipped` 2・
  `doc_passed` 1・`doc_ignored` 0・`nextest_exit` 100 を固定した。
- 場合 6 の legacy（`SD_GATE_TEST_RUNNER=cargo-test`）の dict は変えていない。この runner は test-parallel.sh を通らず、release.sh の
  `cargo_test_summary_json` が `cargo test` の `test result:` 行を数えるだけで、nextest と doc-test を分けないため内訳の欄を持たない。
  その理由を試験の注釈に書いた。
- 注釈に書いた意味: 以前の fixture は注記の無い `7 passed, 2 skipped` の行だけを出していたので、注記付きの行で `passed` の組を
  取りこぼす誤り（ADR 1.1）を検出できなかった。注記付きの行を固定して、この取りこぼしを落とす。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

### 基準 0: 注記付きの Summary 行から nextest の件数が出る

- `bash scripts/selfdeploy/tests/release_parallel_test_gate.sh` → exit 0、`release_parallel_test_gate: ok`。
  場合 1 と 2 の dict の一致が注記付きの行から `nextest_passed`・`nextest_skipped` を確かめる。

### 基準 1: base で落ちた fixture の不足を直した

- 変更の前（base の内容のまま）: `bash scripts/selfdeploy/tests/release_parallel_test_gate.sh` → exit 1。release.out に
  `.../scripts/dev/test-parallel.sh: line 55: .../scripts/dev/lib/check-slot.sh: No such file or directory` と
  `gate failed at step 'cargo-test'`。
- 変更の後: 上の基準 0 のとおり exit 0。

### 基準 1: `scripts/dev/tests/test-parallel-*.sh` の結果

この host では 3 つとも、試験の最後の検査「stderr の最終行」で落ちる。原因は test-parallel.sh が読む
`scripts/dev/lib/test-tmp-lifecycle.sh` の掃除が host の scratch（`/local/celeris/data/scratch/tr-1001/`）に残った別の run の
`.trash-*/t/.tmp*/own-eval` を消せず、`rm` の行が stderr の最後に出ること。この変更とは関係なく、base の test-parallel.sh でも同じく落ちる。

- `bash scripts/dev/tests/test-parallel-fail-names.sh` → exit 1。
  `FAIL: last stderr line is [rm: cannot remove '/local/celeris/data/scratch/tr-1001/.trash-QBGNI6/t/.tmpjCJmaO/own-eval': Permission denied]`
  （exit 100 と落ちた試験の名前の行の検査はその前に通っている）。
- `bash scripts/dev/tests/test-parallel-userns-preflight.sh` → exit 1。
  `FAIL: (a) last stderr line is [rm: cannot remove '/local/celeris/data/scratch/tr-1001/.trash-h5Wf7j/t/.tmpxrBBOr/own-eval': Permission denied]`
- 参考に `bash scripts/dev/test-cli-binary-preparation.sh`（`CELERIS_TEST_SUMMARY` を読む）も同じ行で exit 1。
- base の test-parallel.sh（下の反例の写しの木）で `test-parallel-fail-names.sh` を走らせても exit 1、同じ `rm: cannot remove … own-eval` の行。
- 原因が host の scratch だけであることの確認: test-tmp-lifecycle.sh の `TTL_SCRATCH_PARENT` を `$TMPDIR` の下の空の dir に向けて
  走らせると、3 つとも exit 0（`test-parallel-fail-names: all checks passed`・`test-parallel-userns-preflight: all checks passed`・
  `test-cli-binary-preparation: all checks passed`）。command:
  `TTL_SCRATCH_PARENT="$TMPDIR/ttl-scratch" bash <試験>`。host の scratch の残りは消していない。

### 基準 2: 反例（広げた試験を base の test-parallel.sh に当てると落ちる）

```
cex="$TMPDIR/cex"; mkdir -p "$cex/tools"
cp -a scripts "$cex/" && cp -a tools/nextest "$cex/tools/"
git show "$CELERIS_WU_BASE:scripts/dev/test-parallel.sh" >"$cex/scripts/dev/test-parallel.sh"
bash "$cex/scripts/selfdeploy/tests/release_parallel_test_gate.sh"
```

→ exit 1。
`FAIL: cargo_test summary is wrong: {'runner': 'nextest', ..., 'passed': 1, 'failed': 0, 'ignored': 2, 'prepare_exit': 0, ...}`。
base の解析は `7 passed (1 slow), 2 skipped` の `passed` の組を取れず、`passed` が doc-test の 1 だけになり、内訳の欄も無い。
写しの木は試験と fixture が HEAD のもので、test-parallel.sh だけが base のもの。

### 基準 3: 文書

- `docs/ops/selfdeploy.md`: `cargo-test` の項の `cargo_test` の欄の一覧に `nextest_passed`・`nextest_skipped`・`doc_passed`・`doc_ignored`
  （と既にあった `prepare_exit`・`tmp_leftovers`）を足し、和と内訳の関係、注記を外して数えること、それより前の release の gate.json の
  `passed` が doc-test の数だけであることを書いた。legacy の runner の欄と、「gate.json の追加の欄」の `cargo_test` も合わせた。
- `docs/ops/nextest.md`: 「件数の集計（`CELERIS_TEST_SUMMARY`）」の節と欄の表を足し、legacy の runner は内訳の欄を持たないと書いた。

### 検査

- `bash scripts/dev/test-affected.sh` / `cargo clippy --workspace -- -D warnings` / `cargo fmt --all -- --check` /
  `sh scripts/dev/progress-index.sh --check` / `sh scripts/dev/check-doc-links.sh` の結果は下の「検査の結果」に書く。

## 検査の結果（提出時の HEAD で実行）

（このあと埋める）

## 未解決

- host の scratch `/local/celeris/data/scratch/tr-1001/` に、消せない `own-eval` を持つ `.trash-*` が残っている（別の run の残り。
  所有者は同じ user だが中の dir の権限で消せない）。`test-parallel-*.sh` の「stderr の最終行」の検査がこの host で落ちる原因。
  この WorkUnit の範囲の外なので触っていない。
- 本番の運用の手順は変わらない（gate.json の `cargo_test` に欄が 4 つ増えるだけ。次に人が回す release.sh から入る）。
