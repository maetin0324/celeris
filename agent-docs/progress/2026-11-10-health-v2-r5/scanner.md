---
title: 健全性v2 R5 scanner — 試験関数の走査・setup の hash・同値 scenario の検出・fragment と --check の台本
tasks: [01M7388T31FEX7W469M2TJS236]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 scanner — 試験関数の走査・setup の hash・同値 scenario の検出・fragment と --check の台本

完了日: 2026-11-10

WorkUnit `scanner`。台本 [`contracts.py`](contracts.py)（python3 の標準 library だけ。build・試験をせず、crates/ を変えない）と、map leaf の共通手順 [`fill-guide.md`](fill-guide.md) を書いた。基準は `$CELERIS_WU_BASE`。数は提出時の HEAD で `--scan` を実行した結果。

## 結論
- 対象の 3 crate で試験は 1864 件、excluded は 2 件。和の 1866 は計画の check と同じ `git grep -E '^[[:space:]]*#\[(tokio::)?test'`（行頭の空白の後に attribute が始まる行）の行の数と一致する。
- 不変条件の試験（`invariant=true`）は 874 件。
- 組の数は duplicate_setup が 81、equivalent_scenario が 6。
- `crates/celeris/src/daemon/cos_reload_tests.rs` はどの `mod` 宣言からも届かず、compile されていない。この file の 2 件の試験は走っていない（excluded の `unreachable`。提案 1）。

## crate ごとの数

| crate | 試験 | excluded | invariant=true |
|---|---|---|---|
| task-api | 855 | 0 | 354 |
| celeris | 804 | 2 | 494 |
| celerisctl | 205 | 0 | 26 |
| 計 | 1864 | 2 | 874 |

fragment ごとの試験の数: api-a 320、api-b 320、api-c 215、celeris-src 537、cli 472（計画の celeris-src 539 との差の 2 件は `cos_reload_tests.rs` の試験）。

invariant=true の根拠: architecture-map の関連試験だけが 734 件、B0 の invariants の evidence_path だけが 53 件、両方が 87 件。

## excluded（2 件）

| path:line | 理由 |
|---|---|
| `crates/celeris/src/daemon/cos_reload_tests.rs:133` | unreachable: crate の root から mod 宣言で届かない file |
| `crates/celeris/src/daemon/cos_reload_tests.rs:146` | unreachable: 同上 |

行の途中にある `#[test]`・`#[tokio::test` は数えない。この base では次の 3 行がそれに当たり、tests にも excluded にも入らない: `crates/celeris/src/lib/tests.rs:560` と `:650`（doc comment の中）、`crates/celeris/tests/campaign_acceptance_e2e.rs:581`（文字列 literal の中）。

## 組の数（owner fragment = 先頭の member の path）

| fragment | duplicate_setup | equivalent_scenario |
|---|---|---|
| api-a | 36 | 1 |
| api-b | 8 | 0 |
| api-c | 3 | 0 |
| celeris-src | 9 | 0 |
| cli | 25 | 5 |
| 計 | 81 | 6 |

duplicate_setup の member は計 240 helper（最大の組は 14 file の `g`）。equivalent_scenario の 6 組は全部同じ file の中の組で、member は計 12 試験。

## 台本の形
- 走査: HEAD の `crates/{task-api,celeris,celerisctl}` の tree id と、`agent-docs/adr` の tree・B0 capability-map・architecture-map・`scripts/dev/check-architecture-map.py` の blob id を記録し、`git cat-file --batch` で読む。作業ツリーは読まない。rebase で commit が消えても、記録した tree と blob の id で走査し直せる。
- 字句解析: コメント（入れ子の `/* */` を含む）・文字列（raw string・byte string）・文字と lifetime・数を分ける。数える行は、行頭の空白の後に `#[test` か `#[tokio::test` が始まる行（計画の check の `git grep -E '^[[:space:]]*#\[(tokio::)?test'` と同じ）。その `#` の位置がコメント・文字列・`macro_rules!` の中・fn 以外の item にあるか、`#[test_case]` のような別の attribute なら excluded にする。
- module: crate の root（`src/lib.rs`・`src/main.rs`・`src/bin/`・`tests/*.rs`）から `mod x;`（`#[path]` と inline mod の中の宣言を含む）を辿り、`mod_path` と「cfg(test) の境界を宣言した src file（試験 module の持ち主）」を決める。
- setup: 試験の本体が呼ぶ自由関数（method 呼び出し・macro・型の関連関数を除く）を、同じ file の helper に結ぶ。無ければ、修飾の path（`common::boot`）か use 文の module 名を持つ helper に結ぶ。tests/ の試験は tests/common・tests/support、src の試験は src の試験 module の helper だけを見る。`fs::write` のような std の呼び出しは結ばない。
- invariant: B0 の evidence_path が試験の file ならその file の試験、src file ならその file を持ち主とする試験 module の試験を true にする。architecture-map の関連試験の列は検査器の `split_cells`・`collect_backtick_paths`・`expand_braces` で読み（二重に実装しない）、列の（…）の中の識別子（`health_evidence_` など）は fn 名の prefix か glob として絞り込む。
- 組: duplicate_setup は試験 code の中の、impl・trait の外の試験でない fn のうち、本体が空でなく、2 file 以上で本体の hash が同じもの。equivalent_scenario は invariant でない試験のうち、同じ file か同じ B0 capability（試験の file か持ち主の src file を B0 の `tests`・`owners` が指す）で literal を伏せた本体の hash が同じもの。組は走査で決まり、mapping に依らない。

## 前回の run の check の不合格と直し
- 1 回目の提出では、計画の check が exit 1 だった（`tests 1864 excluded 5 grep 1866`）。台本は行の途中の `#[test]`・`#[tokio::test` も数えていた（`#\[test\]|#\[tokio::test`）。計画の check は行頭の空白の後に attribute が始まる行だけを数える。差の 3 行は doc comment の中の 2 行と文字列 literal の中の 1 行だった。
- 直し: `GREP_RE` を check と同じ形（行頭から `[ \t\r\f\v]*#\[(?:tokio::)?test`）にし、`--scan`・`--check` が呼ぶ `git grep` の pattern も `^[[:space:]]*#\[(tokio::)?test` にした。`#[test_case]` のような別の attribute の行は excluded の `not_test_attr` にする。self-test の fixture に行頭と行の途中の両方の例を入れた。
- 前回の run が作業ツリーに残した、埋めていない `mapping/*.json` の雛形 5 file（未追跡）を消した。map leaf が自分の HEAD から作り直す。

## 証拠（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --self-test` → exit 0（`self-test: ok`）。fixture は入れ子の mod・`#[path]` の試験 module・raw string の行頭の `#[test]`・block comment の行頭の `#[test]`・行の途中の `#[test]`（`//` の後・doc comment・文字列の中。数えない）・`#[test_case]`・コメントだけ違う helper・literal だけ違う試験・impl の method・`std::fs::write`・届かない file・macro の雛形・evidence_path の試験 module。grep の行数 12・試験 6 を確かめる。旧い数え方（行の途中も数える）に戻すと self-test は 2 件の失敗で exit 1 になる。
- 計画の check（`git grep -hcE '^[[:space:]]*#\[(tokio::)?test' -- crates/task-api/ crates/celeris/ crates/celerisctl/` の和と `--scan` の tests・excluded の和を比べる）をそのまま実行 → exit 0、`tests 1864 excluded 2 grep 1866`。`--scan` の stderr は `tests=1864 excluded=2 git_grep_lines=1866 match=yes`。crate ごとの grep の数は task-api 855、celeris 806、celerisctl 205。
- 配線の確認（repo の外の `$TMPDIR/demo` に仮の fragment を作った。repo には mapping を置いていない）:
  - `--scaffold <name> --dir $TMPDIR/demo` を 5 fragment で実行 → exit 0（api-a 50 file・320 試験・37 組、api-b 44・320・8、api-c 51・215・3、celeris-src 60・537・9、cli 104・472・30）。同じ name で 2 回目 → exit 2（上書きしない）。
  - 埋めていない雛形の `--check-fragment`（api-c）→ exit 1（436 件の不備）。仮に埋めた 5 fragment と old-impl の `--check-fragment` → 全部 exit 0。
  - `--build --dir $TMPDIR/demo > contracts.json` → exit 0。`--check contracts.json --dir $TMPDIR/demo` → exit 0（`check: tests=1864 excluded=2 git_grep=1866 ok`）。
  - 負の例: equivalent_scenario を採らない → `--check` exit 1（`candidates.equivalent_scenario が 0 件`）。invariant の試験を old_impl の members に入れる・試験 file を mutation_target にする・履歴に無い commit 件名 → `--check-fragment` exit 1（3 件）。知らない capability → exit 1。contracts.json の masked_sha256 を書き換える → `--check` exit 1。
- `bash scripts/dev/test-affected.sh` → exit 0（`試験対象の変更なし（docs のみ等）`、repo-health verdict=pass、new=0、worsened=0）。
- `cargo clippy --workspace -- -D warnings` → exit 0、Finished（build 済みの target で 5 秒。依存の deno_core の警告 1 件は workspace の外）。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → いずれも exit 0。
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`。
- 範囲: `git diff --name-only $CELERIS_WU_BASE` は `agent-docs/progress/2026-11-10-health-v2-r5/` の 3 file だけ。crates/・scripts/・.config/ に差分なし。

## commit
- `R5 scanner: contracts.py（試験関数の走査・組の検出・fragment と --check）と map leaf の手順`
- `R5 scanner: 進捗に提出時の HEAD での検査の結果を書く`
- `R5 scanner: 試験の行を計画の check と同じく行頭の #[test で数える`

## 未解決事項
- equivalent_scenario の「同じ capability」は B0 の capability（file 単位の `tests`・`owners`）で決める。map leaf が付けた capability は組の検出に使わない。組を走査だけで決め、並行する map leaf が同じ組の集合を見るための選択。mapping の capability で初めて同じになる組は検出しない。
- setup は直接呼ぶ helper だけを数える。helper が呼ぶ helper（`dechunk` など）は setup に入らないので、duplicate_setup の 81 組のうち 13 組は `users` が空。
- 同名の試験 file（tests/common/mod.rs など）が複数の test binary から届くときは、最初の root の `mod_path` を記録する。

## 提案
1. `crates/celeris/src/daemon/cos_reload_tests.rs` は compile されていない（`daemon.rs` にも他の file にも `mod cos_reload_tests` が無い）。architecture-map の 147 行はこの file を関連試験に挙げている。`daemon.rs` に `#[cfg(test)] mod cos_reload_tests;` を戻すか、file と地図の記述を消すかを決める task を立てる。`daemon.rs` は R5 の write set の外なので、この task では変えない。
2. architecture-map の検査器（`scripts/dev/check-architecture-map.py`）は関連試験の path の実在だけを見る。compile されない試験 file を挙げた行は検出できない。`contracts.py` の unreachable の判定と同じ「crate の root から mod 宣言で届くか」の検査を足す案がある（scripts/ は R5 の write set の外）。
