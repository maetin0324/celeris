---
title: dagbench_harness の「graded run pins changed」（fix-dagbench-pins）
tasks: [01M62S8XWCZDCY2ZBVQ35176MH]
status: done
updated: 2026-10-30
---
# dagbench_harness の「graded run pins changed」（fix-dagbench-pins）

完了日: 2026-10-30

## 結論

段 fixpipe の統合 check で `celeris-campaign::dagbench_harness` の
`graded_faults_exclude_external_wait_and_post_from_redo` が `graded run pins changed; use a fresh output directory`
で落ちた原因は、grading の pin のうち **`grader_sha256`（grader binary の内容 hash）** が run の途中で変わったこと。
試験の fixture が共有 target の `celeris-campaign`（`CARGO_BIN_EXE_celeris-campaign`）を直に pin しており、
同じ nextest の中で別の試験がその binary を作り直していた。直し方は試験の fixture が grader の写しを run の root に
固定すること。pin の比較（`RunHeader::write`）は変えていない。daemon・`crates/campaign/src` は変えていない。

## 原因

- `RunHeader::write` は graded run の `run.json` を 1 回目に書き、以後の書き込み（`run()` の冒頭・故障 run の子
  process `child_control`・runner 完了後）で内容の一致を確かめる。pin（`Grading::pin`）は grader binary の sha256、
  `good_artifact` の tree の sha256、case・observed の sha256 を含む。
- `good_artifact`・case・observed は試験ごとの tempdir にあり、run の中で書く処理は無い（`materialize` は写しを
  attempt dir に作り、上書きはその写しに対して行う）。変わりうるのは共有 target の grader binary だけ。
- 段 fixpipe で入った `crates/celeris/tests/ops_r1_pipeline.rs`（pipe-e2e）の `campaign_bin()` は、試験中に
  `cargo build -p celeris-campaign --bin celeris-campaign` を毎回走らせる。nextest の build（`-p celeris -p celeris-campaign …`
  の feature 集合・test profile）と feature の集合が違うので、celeris-campaign を compile し直し、
  `target/debug/celeris-campaign` を別の inode・別の内容で置き換える。test-affected の対象に `celeris` と
  `celeris-campaign` が並ぶ統合 check では、この build が 77 秒走る dagbench の graded 試験と重なる。
  単独実行では build が起きないので通る。
- 確認（この WU の target で実行）: nextest build 後の `celeris-campaign` は sha256 `12653c5f…`・inode 139252561。
  `cargo build -p celeris-campaign --bin celeris-campaign` の後は sha256 `55ea27c4…`・inode 139308239（Compiling celeris-campaign）。
- 既に `real_k3_repetitions_from_formal_entry` と `common/dagbench_durable_checks.rs` は同じ理由で grader を写して
  いたが、`graded_faults_exclude_external_wait_and_post_from_redo` などほかの fixture 利用者は写していなかった。

## 直し方

- `crates/campaign/tests/common/dagbench_grading.rs`: `freeze_grader(dir, source)` を足し、`fixture`・`k3_grading` は
  grader を `root/grader` に写して pin する（写しは `cp` の別 process で書き、ETXTBSY を避ける）。grader の出所を
  指定できる `fixture_with_grader` を足した。
- 個別に写していた `real_k3_repetitions_from_formal_entry`（dagbench_harness.rs）と `dagbench_durable_checks.rs` の
  写しを消した（fixture が写す）。
- pin の比較は緩めていない。固定した grader そのものが変われば、固定済みの graded run の再開は今までどおり拒む。

## 回帰試験

`crates/campaign/tests/dagbench_graded.rs` の `graded_pins_survive_a_relinked_shared_grader_but_reject_a_changed_one`。
負荷を使わず、cargo の uplift と同じ rename による file の差し替えで relink を決定的に起こす。

1. 共有 path を直に pin した grading は、relink の後の RunHeader の書き込みで `graded run pins changed` になる（統合 check の失敗の再現）。
2. fixture の写しを pin した grading は、共有 path の relink の前後で同じ raw に書き続けられ、pin も同じ。
3. 写した grader そのものを差し替えると、今までどおり `graded run pins changed` で拒む。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p celeris-campaign --test dagbench_graded`: 5 passed（回帰試験を含む）。
- `bash scripts/dev/test-affected.sh`: exit 0。対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
  nextest 2850 passed・9 skipped・0 failed（`ops_r1_pipeline` と同じ run の中で
  `graded_faults_exclude_external_wait_and_post_from_redo` が 100 秒で pass。dagbench_harness・dagbench_graded の全試験 pass）。doc test ok。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 変更の commit（件名）

- fix-dagbench-pins: dagbench の grading fixture が grader の写しを pin する（共有 target の relink で pin が変わる）

## 未解決

- `crates/celeris/tests/ops_r1_pipeline.rs` が試験中に `cargo build` で共有 target の binary を置き換える挙動は残る
  （この WU の範囲は crates/campaign）。共有 target の binary を内容で pin するほかの試験があれば同じ形で落ちうる。

## 提案

- `ops_r1_pipeline.rs` の `campaign_bin()` は binary が無いときだけ build する（`cap_probe_real_daemon.rs` と同じ）か、
  写しを使う形にすると、並走する試験の binary を置き換えない。
