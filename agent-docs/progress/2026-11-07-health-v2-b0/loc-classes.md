---
title: 健全性 v2 B0 loc-classes — 全追跡行の排他的分類と基準計測
tasks: [01M6TYVJSPEYA2P8B7G56HSQ73]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性 v2 B0 loc-classes — 全追跡行の排他的分類と基準計測

完了日: 2026-11-07

## 結論
- 基準（着手時の `git merge-base HEAD main`。件名「repair: main 取り込みの衝突解消の import を整形する」）の全追跡行は
  **1,512,205 行**。12 class の合計・言語別の合計・`git grep -I -c '' <基準>` の合計が一致する（二重計上なし）。
- 人の見立て 20〜30 万行に対する 4 つの見方は次のとおり。どれも分類規則 `source-size-exclusive/1` での値。

| 見方 | 含む class | 行数 | うち Rust |
|---|---|---:|---:|
| 本体のみ | production（全言語） | 366,177 | 320,064 |
| 自前総量（テスト込み） | production + inline_test + external_test + tooling + evaluation_infra + migration | 967,314 | 828,913 |
| 全言語（全追跡行） | 12 class すべて | 1,512,205 | 897,685 |
| 外部分離候補 | evaluation_infra + vendor | 256,886 | 189,185 |

- 本体のみの Rust（320,064 行）だけで見立ての上限 30 万行を約 2 万行超える。全言語の本体（366,177 行）は上限より
  66,177 行、下限 20 万行より 166,177 行多い。試験（Rust の external_test 368,793 + inline_test 19,266）は本体の Rust より多い。
- この数は目標ではない。最初の slice の実績で見立てを更新する前提で、行数の削減量を受け入れ条件にしない。

## class × 言語（行数、基準の commit の tree を読む）

| class | rust | typescript | javascript | python | shell | sql | markdown | json | toml | yaml | other | 計 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| production | 320,064 | 34,834 | 4,044 | 4,596 | 51 | 0 | 0 | 136 | 2,125 | 0 | 327 | 366,177 |
| external_test | 368,793 | 26,571 | 8,050 | 2,977 | 7,696 | 0 | 0 | 0 | 18 | 0 | 0 | 414,105 |
| inline_test | 19,266 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 19,266 |
| generated | 0 | 7,855 | 0 | 0 | 0 | 0 | 0 | 36,040 | 0 | 0 | 0 | 43,895 |
| vendor | 68,395 | 1,469 | 3,284 | 24,287 | 70 | 0 | 9,342 | 314 | 328 | 5 | 4,442 | 111,936 |
| fixture | 143 | 397 | 7 | 1,625 | 747 | 0 | 331 | 30,170 | 13 | 0 | 2,705 | 36,138 |
| evaluation_infra | 120,790 | 0 | 0 | 16,595 | 6,541 | 0 | 0 | 919 | 71 | 0 | 34 | 144,950 |
| tooling | 0 | 177 | 572 | 4,642 | 12,530 | 0 | 0 | 517 | 474 | 21 | 654 | 19,587 |
| docs | 234 | 0 | 0 | 7,078 | 2,191 | 0 | 181,566 | 75,712 | 1,119 | 20 | 20,475 | 288,395 |
| schema | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 54,438 | 0 | 0 | 0 | 54,438 |
| migration | 0 | 0 | 0 | 0 | 0 | 3,229 | 0 | 0 | 0 | 0 | 0 | 3,229 |
| lockfile | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 5,813 | 4,276 | 10,089 |
| 計 | 897,685 | 71,303 | 15,957 | 61,800 | 29,826 | 3,229 | 191,239 | 198,246 | 4,148 | 5,859 | 32,913 | 1,512,205 |

raw（基準 SHA・環境・分類規則の版と config の sha256 付き）は [loc-baseline.json](loc-baseline.json)。
計測環境: host home-dev、Python 3.13.3、git 2.48.1（JSON の `environment`）。

## 分類の読み方（数を使う前に知っておくこと）
- 規則は `scripts/dev/source-size-report.toml` の `[exclusive_classes]`（順序付き、最初に当たった規則の class）。
  production と判定された Rust file だけを、gate と同じ `analyze_rust_file` で production と inline_test に行単位で分ける。
- external_test の Rust には `src/**/tests.rs`・`*_tests.rs` の file 単位の試験 module を含む（例: task-dispatch/src の 91,869 行）。
  Cargo の test target 数とは別の数え方。test target の列挙は WU test-targets の担当。
- evaluation_infra は crates/campaign・routing-eval・jev-classifier の全体（その試験を含む）と、scripts/campaign・
  scripts/model-routing・scripts/dev の pilot/bench/soak/比較 driver。本体の試験とは重ならない。
- vendor は crates/script-runtime/vendor（deno_core の写し、68,395 行が Rust）と、upstream を SOURCE.md に記した
  config/skills の 5 skill（yomiyasu の Python 24,287 行など）。
- docs は agent-docs/・docs/・`*.md`。agent-docs/progress の計測 raw（json/jsonl/log、python の計測台本の写し）も docs に入る。
- production の toml 2,125 行は config/*.example.toml。python 4,596 行は crates/task-worker/src の実行時 helper
  （langmem・paperqa・LDR・browser の台本）。
- 生成物の印は先頭 10 行の `@generated`・`This file was automatically generated`・`DO NOT EDIT`。vendor と lockfile は印より先に決まる。
- 行は「改行の数 + 改行で終わらない最終行」。binary（先頭 8000 byte に NUL）と symlink は 0 行として file 数にだけ入る。
- 境界の正負例（inline test の境界・生成物の印・vendor/fixture の path・合計一致）は
  `scripts/tests/test_source_size_exclusive.py` が固定する。規則を変えたら `version` を上げる。

## 再計測の手順
```sh
# 基準の commit の tree を読む（作業ツリーの変更に左右されない）
python3 scripts/dev/source-size-report.py --format json --exclusive-classes \
  --rev "$(git merge-base HEAD main)" --out <出力先>.json
# 表で見る
python3 scripts/dev/source-size-report.py --exclusive-classes --rev "$(git merge-base HEAD main)"
# 作業ツリー（追跡 file の今の内容）を測る: --rev を外す。JSON の worktree_dirty が変更の有無を示す
python3 scripts/dev/source-size-report.py --format json --exclusive-classes
# 合計の独立検算（totals.tracked_lines と一致するはず）
git grep -I -c '' "$(git merge-base HEAD main)" | awk -F: '{s+=$NF} END{print s}'
# file ごとの class と当たった規則: --files を足す
```
合計が一致しないと exit 3。計測は約 12 秒（CPU 1 本）。

## 変更
- `phase 1: source-size-report に全追跡行の排他的 class 分類を足す` — `--exclusive-classes`・`--rev`・`--files`、
  toml の `[exclusive_classes]`、unittest 22 件、この進捗と loc-baseline.json。
- `phase 1: 排他的分類の試験の入力 path を doc link 検査の対象から外す` — 新しい試験の `docs/…` は分類器の入力値なので
  `scripts/dev/check-doc-links.sh` の FOREIGN_DOCS に足した（既存の test_source_size_report.py と同じ扱い）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `python3 -m unittest discover -s scripts/tests -p 'test_source_size*.py'` → exit 0（60 tests OK、うち新規 22）
- `python3 scripts/dev/check-source-size.py` → exit 0（0 failure、3 debt warning、1 reasoned exception。変更前と同じ）
- 既存の `run_report` を基準の版と今の版で同じ tree に走らせ、totals と warnings が一致（gate の判定の入力が不変）
- `git diff "$(git merge-base HEAD main)" -- scripts/dev/source-size-baseline.json scripts/dev/check-source-size.py` → 差分 0
- `sh scripts/dev/check-doc-links.sh` → ok、`--self-test` → exit 0
- `git grep -I -c '' <基準>` の合計 1,512,205 = `totals.tracked_lines` = `totals.classified_lines`

## 未解決
- class の境界は path 規則による。evaluation_infra と tooling の境（例: scripts/dev/pc-rollout は tooling に置いた）は
  後続の capability 地図で見直す余地がある。
- crates/celeris/.tmp16bo4f/（試験の KB の残り）が追跡されている。fixture に分類したが、追跡すべきかは未確認。

## 提案
- 後続の slice の前後で同じコマンドを `--rev` 付きで流し、class ごとの差を記録する（行の付け替えと純減を区別できる）。
