---
title: K2 ops-r2 試行開始前チェックの残り — 漏洩検査の公開 archive 除外と submodule 込みの種
tasks: [01M69YZVW27JDDR1115KFCD6NH]
status: done
updated: 2026-10-31
---
# 漏洩検査の公開 archive 除外と submodule 込みの種（親 task 01M69YXTFWZT56XCZFY2PCPZMB の項目 1・2）

完了日: 2026-10-31

方針は ADR [2026-10-31-ops-r2-preflight-fixes](../../adr/2026-10-31-ops-r2-preflight-fixes.md) の付記 D-public・D-seed2 に書いた。

## 変更
- 項目 1: `scripts/campaign/check-ops-r1-leak.py --public-archive <tar> <sha256>`（複数可）。
  - tar の sha256 が引数と違えば exit 2。tar の生 byte と member（入れ子 archive を含む）に現れる private の文字列 needle を検査から外す。
  - 外すのは private の文字列だけ。private の sha256・`--forbidden-sha`・`--canary` は外さない。
  - 結果の `checked.public_archive` には外した数（`excluded_count`）と長さ（`excluded_lengths`）、前後の文字列数だけを書く。文字列・hash・JSON path は書かない。
  - 外した後に private の文字列が 0 件なら `warning: no_private_strings_left`、半分以上を外したら `warning: most_private_strings_removed`。
    `limits` の `public_archive_exclusion` と stdout にも出す。pass/fail は変えない。
- 項目 2: `scripts/campaign/ops-r2-seed.sh make --archive <公開 tar> --case <case.json> --assets <asset store> --out <tar>`。旧 `--repo`・`--rev` は exit 2。
  - 公開 tar の sha256 を case.json の `initial.archive_sha256` と照合する（公開 tar は変えない）。
  - `initial.submodules` の各 tar を `SHA256SUMS` と照合し、浅い path から `<path>/` の下に入れて非圧縮の tar 1 つにする。owner は 0、pax の global header は落とす。
  - 同じ path の file が 2 つの tar にあれば exit 1。`place`・`unpack` は変えていない。
- 試験: `scripts/campaign/test_check_ops_r1_leak.py` に 6 件、新しい `scripts/campaign/test_ops_r2_seed.py` に 8 件。
  `scripts/campaign/run-ops-r1-leak-tests.sh` が両方を走らせる。`crates/campaign/tests/ops_r2_seed.rs` は新しい `make` に合わせ、submodule の中身が展開されることを足した。
- 文書: `docs/ops/ops-r2-k2-preflight.md` §4（`--public-archive` と種の scan）と §6.1（新しい `make`、初期ツリーの定義）、`docs/ops/ops-r1-leak-check.md`（引数と結果の欄）。

commit（件名）:
- ops-r2: 漏洩検査が公開の初期 archive の needle を外し、種に submodule の中身を入れる
- 進捗: leak-seed の bundle 名から hash 様の表記を外す
- 進捗: leak-seed の提出時の検査結果

## case.json の初期ツリーの定義と採点の関係
- case.json の `initial.archive_sha256` は `git archive <initial.commit>` の tar の sha256 で、submodule を含まない
  （`crates/campaign/src/cases.rs` の `Initial` の注記「submodule は含まない」）。gitlink の path（`lib/pluvio`・`lib/locusta`・`ior_integration/ior`）は空の dir になる。
- asset store の benchfs の bundle（初期 commit を含む）から `git archive` した tar の sha256 は
  b1ae8f58dd48c8fdb451dfb6b26287c188eeba62bfbdeaa8a518a760c6c980bf で、case.json の値と一致した（2026-10-31）。
- submodule 5 件（入れ子 2 件を含む）は `initial.submodules` の `<path>@<40 桁 commit>` で固定され、実体は asset store の
  `submodules/<末尾名>-<commit 先頭 8 桁>.tar`（`SHA256SUMS` に載る）。
- 採点器は試行の種を使わない。`grade --initial-tree`（ADR 2026-10-18-grade-initial-tree-baseline D2）の木は grader 側の
  `asset_assembly::assemble_initial` が bundle を clone・固定 commit に checkout し、同じ submodule の tar を各 path に展開し、
  `initial.lock.json` の内容 hash で照合して組む。この木は submodule を含み、baseline build はこの木で走る。
- 実 asset で種を作り（sha256 233d38cbbc390cd9374287b9eb618e715c4088e4e858f4eb179d1ed6e08b4d60、2 回作って一致）、展開した木を
  `assemble_initial` と同じ手順（bundle の checkout ＋ submodule の tar の展開）で手で組んだ木と比べた。`diff -r --no-dereference -x .git` も
  mode の一覧の diff も差が無かった。file は 2016 件。したがって試行に渡す木と採点の baseline の木は同じ内容になる。
- 限界: `ops-r2-seed.sh` は tar の sha256 で照合する。`initial.lock.json` の内容 hash での照合は採点器の `assemble_initial` だけが行う。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/campaign/run-ops-r1-leak-tests.sh` → exit 0、61 tests OK（漏洩検査 53・種 8）
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
  nextest 2919 tests run: 2919 passed、9 skipped。文書検査（doc-links・human-text・progress-index ほか）も通過
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `cargo nextest run -p celeris-campaign --test ops_r2_seed --test ops_r2_trial_a_command` → 4 passed
- 実 asset での漏洩検査（private は合成の dummy、本物の private は開いていない）: 種の state を `--scan`、公開 tar と submodule tar 5 件を
  `--public-archive` → exit 0、0.5 秒、`excluded=1 private_strings=2->1 warning=most_private_strings_removed`

## 未解決
- 評価器の uid（celeris-eval）が `$P/k2-initial.tar` と asset store の submodule tar を読めるかは未確認。読めなければ §4 の漏洩検査は exit 2 で止まる。
- 本物の private で外れる数・長さは未確認（この run は private を開かない）。`warning` が出たら operator が承認票の判断に回す。

## 本番への移行（operator が実行する）
1. `docs/ops/ops-r2-k2-preflight.md` §6.1 で公開 tar を作り、sha256 が case.json と一致することを確かめ、新しい `make` で種を作り直し、A・B の state に `place` する
   （前の種は submodule が空。`place` は既存の種と sha256 が違うと拒むので、`$S_A/seed`・`$S/seed` の古い `benchfs.tar` を先に退避する）。
2. §4 の漏洩検査を `--public-archive`（`PUB`）付きでやり直し、`checked.public_archive` の数と `warning` を記録する。
3. §6.2 の command を新しい `SEED_SHA` で出し直す。

## 提案
- なし
