---
title: アーキ v4 G compare — after の中央値を A と比べ、10 % を超えて遅い組に印を付ける
tasks: [01M7MZTKBKXKEVBWS87PS5J9NK]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# compare（WorkUnit）

before は A の baseline.md（main bc665168、A の run の成果物 `artifacts/baseline.md`）の §5.3・§5.5 の値。after は measure 段の
[scope.tsv](scope.tsv)・[build-check.tsv](build-check.tsv)・[build-test.tsv](build-test.tsv)・[ta-s1.tsv](ta-s1.tsv)・[ta-mix.tsv](ta-mix.tsv)・[ta-s2.tsv](ta-s2.tsv)
の `rev=after`・`valid=yes` の行（`git merge-base HEAD main` の tree。測定の規則は [rules.md](rules.md)）。
印の規則: `after_median` が `a_median × 1.10` を超える組は `flag=control`（control の leaf が before を同じ host で 3 回測り直す）、ほかは `ok`。
結果の表は [flags.tsv](flags.tsv)（列 `metric scenario a_median after_median ratio flag`）。この leaf は TSV を読んで比べただけで、build も測定もしていない。

## 0. 結論

- 10 組のうち **印が付いたのは health:S2 の 1 組だけ**（after の中央値 96.09 s、A 81.92 s、+17.3 %。A の +10 % は 90.11 s）。
  control-build はこの組の before を測り直す。control-s1・control-s2 は印が無いので測らない（test_affected の S1 +1.4 %、S2 +8.0 %）。
- 印の無い 9 組のうち +5 % を超えたのは health:S1（+8.1 %）・test_affected:S2（+8.0 %）・check_warm:S2（+6.1 %）・test_build_warm:S2（+5.1 %）。
  4 組とも user の CPU 時間の中央値は A の −3.2〜+3.5 % で、wall の差の方が大きい。
- health:S2 の user の中央値は A の +3.5 %（+2.40 s）で、wall の差（+14.17 s）の 2 割に満たない。残りは wall − user − sys の差（A 4.1〜8.3 s、
  after 10.6〜17.6 s）で、CPU を使っていない待ちの時間が延びた。コード由来（検査器の変更・repo の増加）か host 由来かは未確認（§5 の候補）。
- after の 30 行はすべて valid で、`src_tree` は 30 行とも scenarios.env の `AFTER_CRATES_TREE` と一致した（不一致 0）。
- 試験の数（nextest の passed）は S1 8,436 → 8,481（+45）、S2 3,917 → 3,938（+21）。`#[test]`・`#[tokio::test]` の属性の数の差と一致した（§2）。

## 1. before/after の表（10 組）

wall の秒。A の run・中央値は baseline.md §5.3（build）と §5.5（health・test-affected）、A の health の user は A の `timing-raw.log` の値。

| metric | scenario | A の run 1・2・3 | A の中央値 | after の run 1・2・3 | after の中央値 | 比 | A の +10 % | 印 | user の中央値（A → after） |
|---|---|---|---|---|---|---|---|---|---|
| check_cold | - | 53.65・53.67・53.56 | 53.65 | 54.90・53.52・53.90 | 53.90 | 1.005 | 59.02 | ok | 216.0 → 216.48（+0.2 %） |
| check_warm | S1 | 41.99・42.47・43.34 | 42.47 | 42.75・42.64・42.64 | 42.64 | 1.004 | 46.72 | ok | 58.7 → 59.15（+0.8 %） |
| check_warm | S2 | 14.67・14.63・15.41 | 14.67 | 15.75・14.62・15.56 | 15.56 | 1.061 | 16.14 | ok | 18.9 → 18.92（+0.1 %） |
| test_build_cold | - | 142.39・142.85・140.23 | 142.39 | 143.36・148.53・147.03 | 147.03 | 1.033 | 156.63 | ok | 978.8 → 979.88（+0.1 %） |
| test_build_warm | S1 | 138.69・135.69・140.00 | 138.69 | 134.76・138.10・141.75 | 138.10 | 0.996 | 152.56 | ok | 754.0 → 754.29（+0.0 %） |
| test_build_warm | S2 | 76.15・78.31・77.64 | 77.64 | 81.60・84.99・80.95 | 81.60 | 1.051 | 85.40 | ok | 306.3 → 296.62（−3.2 %） |
| health | S1 | 87.82・86.56・81.48 | 86.56 | 97.06・92.78・93.58 | 93.58 | 1.081 | 95.22 | ok | 69.45 → 71.19（+2.5 %） |
| health | S2 | 81.76・81.92・85.96 | 81.92 | 98.73・96.09・90.65 | **96.09** | **1.173** | 90.11 | **control** | 69.46 → 71.86（+3.5 %） |
| test_affected | S1 | 893.41・878.66・900.34 | 893.41 | 911.49・901.20・906.03 | 906.03 | 1.014 | 982.75 | ok | 4,354 → 4,492.85（+3.2 %） |
| test_affected | S2 | 695.36・655.59・647.32 | 655.59 | 722.11・707.76・653.34 | 707.76 | 1.080 | 721.15 | ok | 3,104 → 3,093.42（−0.3 %） |

- after の各組は 3 行ずつ（valid=no の行は 0）。開始時の 1 分 load は全行で 4 以下（0.62〜3.98）。exit は health が 6 行とも 1（const の probe で
  fail になるのが手順どおり。rules.md）、ほかは 24 行とも 0。
- health:S1 の after の run 1（97.06 s）は A の +10 %（95.22 s）を超えたが、中央値は超えない。health:S2 は 3 回とも A の 3 回の最大（85.96 s）より遅い。
- test_affected:S2 の中央値は +10 % まで 13.39 s。run 1（722.11 s）は +10 % を 0.96 s 超えるが、直前の状態が A の中央値の回と違う（ta-mix.md §3）。

## 2. src_tree と試験の数

src_tree:

| | tree（`git rev-parse <rev>:crates`） | 行 | 不一致 |
|---|---|---|---|
| after（`git merge-base HEAD main` = この WorkUnit の HEAD の crates/ と同じ） | scenarios.env の `AFTER_CRATES_TREE`（TSV の `src_tree` の列と同じ値） | 30 | 0 |
| before（A、bc665168） | `git rev-parse bc665168:crates` の値（after の tree とは違う） | A の記録に src_tree の列は無い（A は bc665168 の作業ツリーで測った。baseline.md §1） | — |

health の `--base` は A が bc665168、after が after の rev（どちらも「測った tree の HEAD」）で、規則は同じ。

試験の数（test-affected の log。A は A の `ta-S*.log`、after は ta-s1・ta-mix・ta-s2 の run の `raw/ta-S*-*.log`）:

| | A | after | 差 |
|---|---|---|---|
| S1 の nextest（passed / skipped / binary） | 8,436 / 22 / 312 | 8,481 / 22 / 312 | +45 / 0 / 0 |
| S2 の nextest（passed / skipped / binary） | 3,917 / 11 / 181（133 skipped） | 3,938 / 11 / 181（133 skipped） | +21 / 0 / 0 |
| `#[test]`・`#[tokio::test]` の属性（crates/ 全体） | 8,820 | 8,865 | +45 |
| 同（S2 の 5 crate: task-dispatch・celeris・celerisctl・celeris-campaign・llm-proxy） | 2,910 | 2,931 | +21 |
| doc test（`test result:` の行 / passed / ignored） | 14 / 3 / 2 | 14 / 3 / 2 | 0 |
| web（S1）: vitest file・test / node --test | 101・705 / 110 | 101・705 / 110 | 0 |
| ts-verify（S1） | 20 passed | 20 passed | 0 |
| test-affected の中の health（findings） | 697（new 0） | 695（new 0） | −2 |

- 属性の数は `git grep -h -E '^\s*#\[(tokio::)?test(\(|\]|$)' <rev> -- crates | wc -l`。nextest の passed の差と同じ値になったので、
  増えた分は B〜F が足した試験（E は +19 と記録。F までの分を含めて +21）。どの試験かの一覧は作っていない（未確認）。

## 3. test-affected の内訳の差

| | A の中央値の回 | after の中央値の回 | 差 |
|---|---|---|---|
| S1 の test build（`Finished`、Compiling の行） | run 1 2m 14s（200 行。web install を含む回） | run 3 2m 09s（24 行） | −5 s |
| S1 の nextest の Summary の秒（3 回の中央値） | 503.2 s | 505.865 s | +0.5 % |
| S1 の wall − nextest（3 回の中央値） | 389.1 s | 400.2 s | +2.8 % |
| S2 の test build（3 回の中央値） | 1m 18s（run 2・3 は Compiling 5 行） | 1m 25s（run 2・3 は Compiling 5 行） | +7 s |
| S2 の nextest の Summary の秒（3 回の中央値） | 406.636 s | 422.878 s | +4.0 % |
| S2 の wall − build − nextest（3 回の中央値） | 161.6 s | 163.2 s | +1.0 % |

- test-affected の対象（`--list`）は S1・S2 とも A と同じ（S1: 14 crate ＋ web typecheck+unit ＋ test-ts-verify、S2: 5 crate）。scope.md §2。
- S2 の wall の差（+52.17 s）は、run 2 の nextest（454.890 s、A の 3 回の最大より +36.9 s）と build の数秒による。同じ起点の run 3 は A の run 3 の +0.9 %。
  user の中央値は −0.3 % で、CPU の仕事の量は増えていない（ta-s2.md §3）。

## 4. 変更波及の before/after（scope.md §2 の要約）

| | S1 A | S1 after（4 file） | S2 A | S2 after |
|---|---|---|---|---|
| 変える production file | 3 | 4 | 4 | 4 |
| 変える単位（repo-health の単位） | 3 | 4（＋ `task-api/src/org_write`） | 2 | 2 |
| 変える crate | 2 | 2 | 1 | 1 |
| 直接 import する単位（C7b） | 92 | 93 | 6 | 6 |
| 推移的に import する単位 / package（C7b） | 369 / 12 | 365 / 12 | 14 / 4 | 14 / 4 |
| 再 build される crate | 11 ＋ 1 | 11 ＋ 1（同じ組） | 4 ＋ 1 | 4 ＋ 1（同じ組） |
| test-affected の対象 | 14 crate ＋ web ＋ ts-verify | 同じ | 5 crate | 同じ |

- S1 の file・単位は 3 → 4 に増えた（D が plan と apply を `org_write.rs` に移した分）。import の単位の差（92 → 93、369 → 365）は B の検査器の変更の範囲
  （after の検査器で bc665168 を数えた値は 93 / 365。scope.md §2 の before'）。S2 は変わらない。変更波及は A より減っていない。

## 5. 原因の候補（未確認。cause の leaf が絞る）

### health:S2（印あり、+17.3 %）と health:S1（印なし、+8.1 %）

| | A の 3 回 | after の 3 回 |
|---|---|---|
| S1 の wall − user − sys | 9.59・8.75・4.17 s | 16.68・13.05・13.70 s |
| S2 の wall − user − sys | 4.20・4.11・8.26 s | 17.55・14.74・10.62 s |
| user（中央値） | S1 69.45 s・S2 69.46 s | S1 71.19 s・S2 71.86 s |
| max RSS | 約 611 MB | 644〜646 MB |
| 開始時の load（1 分 / 5 分） | 1 分 0.3〜1.3（A §1。5 分は記録が無い） | S1 3.91・3.50・2.56 / 8.48・7.00・5.78、S2 2.02・3.55・3.93 / 4.73・4.46・4.42 |

1. host 由来（待ちの時間）: wall の差の大半（S2 で +14.17 s のうち約 +10 s）は wall − user − sys の延び。after の health は、並行の WorkUnit と
   release の build の余波で 5 分 load 4.4〜8.5 の中で始まった（scope.md §3）。A は 1 分 load 0.3〜1.3。repo-health は git の子 process で
   snapshot を読むので、host の I/O の混み具合が待ちに出る可能性がある。
2. コード由来（検査器の変更）: bc665168 → after で `scripts/dev/repo-health` は 6 file・+320 −50 行変わった（`health/lang_deps.py` +151 −31、
   `rules/deps.py` +45 −6、`rules/size_growth.py` +10 −8、`health/profile.py` +32 −3、`profiles/celeris.toml` +76 −2、schema +6。B の `#[path]` の論理 module の追跡と SCC の内部の新しい辺の検出、
   C の profile の補完）。user の +1.7〜+2.4 s と RSS の +33 MB はこの分の可能性がある。
3. コード由来（repo の増加）: health が読む head の file は rust 1,859 → 1,864、markdown 2,577 → 2,625、python 291 → 293。crates/ は 80 file・+6,803 −1,716 行。
   dispatcher の論理 module は 70 → 77 箇所（B の `#[path]` の追跡で health_review の file が入った分を含む）。
4. 分け方（cause の leaf への提案）: control-build が A の手順で bc665168 の health:S2（A の検査器 × bc665168）を同じ host・同じ時刻帯で 3 回測る。
   それが A の値（81.92 s）に近ければ 2・3 のコード由来、after に近ければ 1 の host 由来。2 と 3 を分けるには、after の検査器 × bc665168
   （scope.md の before' と同じ組み合わせ）の health を 1〜3 回足す。health:S1 も同じ原因の候補を持つが印は無いので、測り直すかは cause の leaf が決める。

### 印の無い +5 % 超の 3 組

- test_affected:S2（+8.0 %）: user −0.3 %。nextest の 1 回（run 2、454.890 s）の延びと test build の +7 s による。SLOW の試験の顔ぶれは A と同じで、
  どの試験が延びたかは絞れていない（ta-s2.md §3）。試験の数は +21（+0.5 %）。
- check_warm:S2（+6.1 %）: user +0.1 %。各回の幅（14.62〜15.75 s）が A の幅（14.63〜15.41 s）と重なる。
- test_build_warm:S2（+5.1 %）: user −3.2 %。開始時の 5 分 load が 10〜13（直前の自分の build の余波。A の 5 分 load は記録が無い）。
- 3 組とも E が変えた task-dispatch（bc665168 → after で 43 file・+3,220 −896 行）を build・試験するが、CPU の仕事の量（user）は増えていないので、
  コード由来の悪化はこの値からは読み取れない（未確認）。

## 6. 再現コマンド（repo の root で。`ART` はこの run の成果物 dir）

```sh
. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env
# A の中央値（baseline.md §5.3・§5.5 の値を写した表）と、after の TSV の valid=yes の中央値・比・印
python3 -I $ART/raw/compare.py agent-docs/progress/2026-11-19-arch-v4-g $ART/raw/a-medians.tsv $AFTER_CRATES_TREE \
  > agent-docs/progress/2026-11-19-arch-v4-g/flags.tsv 2> $ART/raw/compare-summary.txt
# 試験の属性の数（before と after）
for rev in bc665168c9caf1898adc4c611404bbee056daf9a $(git merge-base HEAD main); do
  git grep -h -E '^\s*#\[(tokio::)?test(\(|\]|$)' $rev -- crates | wc -l
  git grep -h -E '^\s*#\[(tokio::)?test(\(|\]|$)' $rev -- crates/task-dispatch crates/celeris crates/celerisctl crates/celeris-campaign crates/llm-proxy | wc -l
done
# 検査器と crates/ の差分の量
git diff --shortstat bc665168c9caf1898adc4c611404bbee056daf9a $(git merge-base HEAD main) -- scripts/dev/repo-health
git diff --shortstat bc665168c9caf1898adc4c611404bbee056daf9a $(git merge-base HEAD main) -- crates
```

生の記録（この run の成果物 dir の `raw/`）: `a-medians.tsv`（A の値と出典の節）・`compare.py`・`flags.tsv`・`compare-summary.txt`（組ごとの行数・run・exit・
開始時の load・user の中央値・wall − user − sys）。A の log は A の run の成果物（`ta-S*.log`・`health-S*.out`・`timing-raw.log`）を読んだだけで、変えていない。

## 7. 未達・要判断

| 項目 | 状態 |
|---|---|
| health:S2 の +17.3 % | 原因は未確認。control-build が before を同じ host で 3 回測り直し、cause の leaf がコード由来か host 由来かを分ける |
| review の時間と費用の before/after | 未達/要判断（scope.md §4。after の窓は 38 時間。同じ長さの窓が揃うのは 2026-11-25T06:00Z） |
| health・review の費用（USD） | health は LLM を使わないので費用は CPU 時間（上の user）だけ。review の費用は上の行と同じく未達/要判断 |
| 増えた試験 +45・+21 の一覧 | 作っていない（未確認） |
