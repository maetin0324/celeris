---
title: アーキ v4 H loc-cost — LoC の増減・S1/S2 と修正の変更波及・build/test-affected/health の時間を A の規則でまとめる
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# loc-cost（WorkUnit）

before は main `bc665168c9caf1898adc4c611404bbee056daf9a`（以下 B0）。after は HEAD（`git merge-base HEAD main` = 件名 `integrate wu/close (phase close)` の commit に、
docs だけを変える struct-deps の commit が乗ったもの。crates/・scripts/・web/ は merge-base と同じ）。規則と snippet は A の baseline.md
（A の run の成果物 `artifacts/baseline.md`）の §4・§6・§7 をそのまま使い、新しい計測ツールは作っていない。数は [loc-cost.tsv](loc-cost.tsv)
（列 `metric scope before after checker note`。before は A の値、after は HEAD か G の値、出所は note）にある。
crates/・scripts/・web/ は変えていない（`test-affected.sh --list` のために入れた comment の probe は戻した。§5）。

## 0. 結論

- (6) LoC（分類規則は B0 に固定）: production は全言語・Rust とも **+822 行**（375,099 → 375,921、Rust 330,408 → 331,230）。試験 +4,743 行、
  own_total_with_tests +5,829 行。アーキ v4 の B〜G の合計で、production は純減していない（v4 の依頼は一律の純減を求めていない）。G の値と同じ。
- (4) 変更波及: S1 は変える file・単位が 3 → 4（D の `org_write.rs`）、crate は 2 → 2。S2 は 4 file・2 単位・1 crate のまま。
  import する単位（C7b）は検査器の版をそろえると before と after で同じ（b0: S1 92/369 → 92/369、head: 93/365 → 93/365、S2 は両方で 6/14 → 6/14）。
  `test-affected.sh --list` の対象も A と同じ（S1 14 crate ＋ web ＋ ts-verify、S2 5 crate）。**変更波及は A より減っていない**。
- 修正の波及（C10）: A の窓では A の値を再現した（no-merges 194 件・複数 region 46.9 %・複数 module 69.1 %）。B0 の後の窓は no-merges で 1 件、
  merge を含めて 9 件しかない。9 件はアーキ v4 の C・E・F の commit と WU の統合、または main の変更を E・F の branch に取り込んだ merge で、
  通常の修正 1 件とは粒度が違う。**割合は比べられない**（未達/要判断の材料）。
- (5) 時間: G の測定の tree と HEAD の間で crates/・scripts/・web/・Cargo.lock・Cargo.toml の差分は 0 だった。規則どおり 10 組とも G の中央値を写し、
  H は何も測り直していない。health:S2 は A の +17.3 %（96.09 s。G の同じ host の before の測り直し 87.26 s、コード由来 +2.02 s、人の決定 h1=keep）、
  ほかの 9 組は A の +10 % 以内。

## 1. (6) LoC（A §7 C12、分類規則は B0 に固定）

`git show bc665168:scripts/dev/source-size-report.py` と `.toml` を `$TMPDIR` に出し、B0 と HEAD に `--exclusive-classes --rev <rev> --format json` で当てた。
2 つの JSON の `config_sha256` は同じ（同じ規則）。merge-base に当てた値は HEAD と同じ（差は docs の区分だけ）。

| 区分 | before（B0） | after（HEAD） | 差 |
|---|---|---|---|
| production 全言語（`views.product_only.lines`） | 375,099 | 375,921 | **+822** |
| production Rust（`views.product_only.rust_lines`） | 330,408 | 331,230 | **+822** |
| 試験 = `class_totals.external_test` + `inline_test` | 455,363 + 21,408 = 476,771 | 460,106 + 21,408 = 481,514 | +4,743（Rust +4,265、Python +478） |
| own_total_with_tests 全言語（`views.own_total_with_tests.lines`） | 1,025,677 | 1,031,506 | +5,829 |
| own_total_with_tests Rust（`views.own_total_with_tests.rust_lines`） | 879,344 | 884,431 | +5,087 |
| tooling（`class_totals.tooling`） | 25,253 | 25,517 | +264（Python +190、TOML +74） |

- before は A の §4 (6) と一致した。after は G の進捗の (6) と一致した。
- production の差 +822 はすべて Rust で、ほかの言語の production の差は 0 行。

## 2. (4) 変更波及（A §5.2・§7 C7・C7b）

S1・S2 の file は G の [scenarios.env](../2026-11-19-arch-v4-g/scenarios.env)（G の scope.md §1 で選び直したもの）。HEAD の source は G の測定の tree と同じなので、
同じ意味の変更で変える file は G の選び直しのままである。

| | S1 A | S1 after（HEAD） | S2 A | S2 after（HEAD） |
|---|---|---|---|---|
| 変える production file（`s1_files`・`s2_files`） | 3 | **4** | 4 | 4 |
| 変える単位（`s1_units`・`s2_units`） | 3 | **4**（＋ `task-api/src/org_write`） | 2 | 2 |
| 変える crate（`s1_crates`・`s2_crates`） | 2 | 2 | 1 | 1 |
| 直接 import する単位（C7b、b0 の検査器 × B0 → HEAD） | 92 | 92 | 6 | 6 |
| 推移的に import する単位 / package（C7b、b0） | 369 / 12 | 369 / 12 | 14 / 4 | 14 / 4 |
| 直接 import する単位（C7b、head の検査器 × B0 → HEAD） | 93（before'） | 93 | 6 | 6 |
| 推移的に import する単位 / package（C7b、head） | 365 / 12（before'） | 365 / 12 | 14 / 4 | 14 / 4 |
| test-affected の対象（`--list`、comment の probe） | 14 crate ＋ web ＋ ts-verify | 同じ集合 | 5 crate | 同じ集合 |

- 検査器の版（E の申し送り）: b0 は B0 の `scripts/dev/repo-health` と `source-size-report.{py,toml}` を `git archive` で `$TMPDIR/rh-B0` に出したもの。
  b0 は after の profile を読めない（`profile_invalid: … review.high_risk[0] は class・paths だけ`）ので、struct-deps.md §2 の 1 と同じく
  `snapshot.resolve('.', B0, HEAD)`（profile は B0、測る tree は HEAD）にした。b0 × B0 は A の 92 / 369 / 12・6 / 14 / 4 を再現した。
- head の検査器の before'（93 / 365）は G の scope.md §2 と同じ値。A の 92 / 369 との差は B の検査器の変更による（G と同じ判断）。
- S1 の after は 4 単位（org_write・handlers・cos・store）で数えた。A の 3 単位で HEAD を数えると b0 93 / 370、head 94 / 366（`org_write` が store を
  import する単位として数えに入るため。G の scope.md §2 の「3 file の参考」と同じ観察）。
- `--list` は S1・S2 それぞれ 4 file に comment の probe を入れて走らせ（`git diff --stat -- crates` は 4 file・4 行）、直後に `git checkout -- <file>` で戻した
  （`git status --porcelain -- crates` は 0 行）。S1 の web の理由は「web の型の元になる crate の変更: task-api task-core」、ts-verify は「runner の cache key の入力の変更」。

## 3. 修正の波及（A §7 C10）

A の snippet（baseline.md §7 C10。B0 の source-size-report で production を判定）をそのまま 2 つの窓に当てた。

| 窓 | 数え方 | commit | production を変えた | 複数 region | 複数 module |
|---|---|---|---|---|---|
| A の窓（`$B`、SINCE 2026-10-28T00:00:00Z）の再現 | no-merges | 492 | 194 | 91（46.9 %） | 134（69.1 %） |
| 同 | with-merges | 653 | 246 | 122（49.6 %） | 175（71.1 %） |
| B0 の後（`$B..HEAD`、SINCE 2026-11-17T00:00:00Z） | no-merges | 12 | **1** | 0（0.0 %） | 0（0.0 %） |
| 同 | with-merges | 41 | **9** | 5（55.6 %） | 5（55.6 %） |

- A の窓の再現は A の §2 #13・§7 C10 の値と 4 列とも一致した。
- B0 の後の窓は件数が少なすぎて、A の窓と割合を比べられない。no-merges の 1 件は C の `PathClass に任意の欄 invariants を足し、往復試験と schema を更新する`
  （production は task-core の design_review の 1 file、ほかは試験と schema）。ほかの非 merge の commit 11 件は進捗の文書（A の 7 件・C の repair 2 件・H の struct-deps 1 件）と
  C の ADR 1 件で、production を変えていない。
- with-merges の 9 件の内訳（WorkUnit の成果物 `raw/c10-after-list.out`）:
  - C の 3 件: 上の PathClass の commit、`integrate wu/prompts (phase impl)`（17 file・3 crate）、`integrate wu/cases (phase verify)`（1 file）。
  - E の 2 件: `integrate wu/wu-region (phase impl)`・`integrate wu/cos-region (phase impl)`（どちらも task-dispatch の dispatcher だけ）。
  - F の 2 件: `integrate wu/lifecycle-tx (phase impl)`（4 file・2 crate）・`integrate wu/knowledge-pin (phase impl)`（3 file・2 crate）。
  - main の変更を E・F の branch に取り込んだ merge 2 件: E の branch への merge（13 file・4 crate。D の org の pilot、C の追補の理由の読み、アーキ v4 の外の
    planner の lease の修正を束ねる）と F の branch への merge（4 file・3 crate。アーキ v4 の外の worker の決定の修正）。
- 1 件が WorkUnit 1 つ分か複数 task 分の変更を束ねるので、通常の修正 1 件と粒度が違う。55.6 % を A の 49.6 % と並べて「悪化」とも「改善」とも言えない。
- 次の候補: 同じ snippet を、release の後に通常の修正が数十件たまった窓（例: B0 の後の 2 週間）に当てる。H の範囲では実施しない。

## 4. (5) build・test-affected・health の時間（10 組、wall の秒）

G の測定の tree は件名 `arch-v4 G scope: after の S1・S2 の file を選び直し、…` の commit の親（F の merge の commit）。その tree と HEAD の間で
`git diff --name-only <G の測定の tree> HEAD -- crates scripts web Cargo.lock Cargo.toml` は 0 行だった（差分の file の一覧は空）。
`crates/`・`scripts/`・`web/` の tree の hash も一致し、HEAD の `crates/` は G の scenarios.env の `AFTER_CRATES_TREE` と同じ。G の TSV の after の 30 行の
`src_tree` もすべて同じ値だった。したがって規則どおり 10 組とも G の中央値を写し、H は health も含めて測り直していない。
test-affected と cargo の時間は、規則により差分の有無によらず測り直していない。

| metric（TSV の key） | A の中央値（before） | G の after の 3 回 | after の中央値（比） | after の出所 | source の差分 |
|---|---|---|---|---|---|
| check_cold | 53.65 | 54.90・53.52・53.90 | 53.90（1.005） | G の build-check.tsv。H は測り直していない | 無し |
| check_warm_s1 | 42.47 | 42.75・42.64・42.64 | 42.64（1.004） | G の build-check.tsv。同上 | 無し |
| check_warm_s2 | 14.67 | 15.75・14.62・15.56 | 15.56（1.061） | G の build-check.tsv。同上 | 無し |
| test_build_cold | 142.39 | 143.36・148.53・147.03 | 147.03（1.033） | G の build-test.tsv。同上 | 無し |
| test_build_warm_s1 | 138.69 | 134.76・138.10・141.75 | 138.10（0.996） | G の build-test.tsv。同上 | 無し |
| test_build_warm_s2 | 77.64 | 81.60・84.99・80.95 | 81.60（1.051） | G の build-test.tsv。同上 | 無し |
| health_s1 | 86.56 | 97.06・92.78・93.58 | 93.58（1.081） | G の scope.tsv。同上 | 無し |
| health_s2 | 81.92 | 98.73・96.09・90.65 | **96.09（1.173）** | G の scope.tsv。G の before の測り直し 87.26 s（同じ host、3 回の中央値） | 無し |
| test_affected_s1 | 893.41 | 911.49・901.20・906.03 | 906.03（1.014） | G の ta-s1.tsv・ta-mix.tsv。同上 | 無し |
| test_affected_s2 | 655.59 | 722.11・707.76・653.34 | 707.76（1.080） | G の ta-s2.tsv・ta-mix.tsv。同上 | 無し |

- H は G の 6 つの TSV の `rev=after`・`valid=yes` の行から中央値を計算し直し、G の進捗の (5) の表と同じ値になることを確かめた（各組 3 行、30 行）。
- health:S2 は A の +10 %（90.11 s）を超える。G の cause・fix で、コード由来は `deps.violation` の CPU +2.02 s（+2.3 %）、残りは測った時間帯の host の差と
  切り分け、人が h1 で「直さない（keep）」と決めた。H の数え方でも悪化は残る（draft・close で未達/要判断として扱う材料）。
- review の時間と費用は review-prod の WorkUnit の範囲で、この表には入れていない。

## 5. 再現コマンド

repo の root で。`B=bc665168c9caf1898adc4c611404bbee056daf9a`、`H=HEAD`。生の出力は WorkUnit の成果物 dir の `raw/`（repo には置かない）。

```sh
# (6) LoC（A §7 C12。分類規則は base に固定）
git show $B:scripts/dev/source-size-report.py > $TMPDIR/ssr.py; git show $B:scripts/dev/source-size-report.toml > $TMPDIR/ssr.toml
for r in $B HEAD; do python3 $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev $r --format json --out $TMPDIR/ssr-$r.json; done
#   views.product_only.lines / rust_lines、class_totals.external_test + inline_test、views.own_total_with_tests.lines / rust_lines、
#   言語別は matrix.<class>.<lang>.lines。config_sha256 が 2 つで同じことを確かめる

# (4) 変える file・単位・crate
. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env   # S1_FILES（4 file）・S2_FILES（4 file）
#   単位 = crates/<c>/src/<第 1 要素>（.rs を除く）、crate = crates/<c>

# (4) C7b（A §7 C7b の snippet を $TMPDIR/c7b.py に写したもの。head の検査器）
python3 $TMPDIR/c7b.py HEAD S1h:crates/task-api/src/org_write,crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store \
  S1:crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store S2:crates/task-dispatch/src/dispatcher,crates/task-dispatch/src/run_settlement
python3 $TMPDIR/c7b.py $B  <同じ引数>          # before'（head の検査器 × B0）
# b0 の検査器: B0 の検査器と source-size-report を出し、sys.path を差し替え、snapshot.resolve('.', B0, 測る rev) にした写し（c7b-b0.py）
mkdir -p $TMPDIR/rh-B0 && git archive $B scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml | tar -x -C $TMPDIR/rh-B0
sed "s#sys.path.insert(0,'scripts/dev/repo-health')#sys.path.insert(0,'$TMPDIR/rh-B0/scripts/dev/repo-health')#; s#snap=snapshot.resolve('.',B,B)#snap=snapshot.resolve('.',B.split('@')[-1],B.split('@')[0])#" $TMPDIR/c7b.py > $TMPDIR/c7b-b0.py
python3 $TMPDIR/c7b-b0.py $B <同じ引数>; python3 $TMPDIR/c7b-b0.py HEAD@$B <同じ引数>   # 引数の「測る rev@profile の rev」

# (4) test-affected の対象（comment の probe。G の probe-comment.sh の写し）
bash probe-comment.sh $S1_FILES; git diff --stat -- crates; bash scripts/dev/test-affected.sh --list; git checkout -- $S1_FILES
bash probe-comment.sh $S2_FILES; git diff --stat -- crates; bash scripts/dev/test-affected.sh --list; git checkout -- $S2_FILES
git status --porcelain -- crates                              # 空であること

# 修正の波及（A §7 C10 の snippet を baseline.md から $TMPDIR/c10.py に写したもの。約 60〜85 s と 1〜2 s）
for m in no-merges with-merges; do python3 $TMPDIR/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-10-28T00:00:00Z $m $B; done          # A の窓
for m in no-merges with-merges; do python3 $TMPDIR/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-11-17T00:00:00Z $m "$B..HEAD"; done   # B0 の後
#   commit ごとの内訳は c10.py の集計の直前に print を 1 行足した写しで出した

# (5) G の測定の tree と HEAD の source の差分（0 行なら G の値を写す）
G=$(git log --format=%H --grep='^arch-v4 G scope' -1 main)^
git diff --name-only $G HEAD -- crates scripts web Cargo.lock Cargo.toml | wc -l      # 0
git rev-parse $G:crates HEAD:crates $G:scripts HEAD:scripts $G:web HEAD:web           # 組ごとに同じ値
# G の TSV の after の中央値
python3 -I -c "import csv,statistics,collections;d=collections.defaultdict(list)
for f in ['scope','build-check','build-test','ta-s1','ta-mix','ta-s2']:
  for r in csv.DictReader(open(f'agent-docs/progress/2026-11-19-arch-v4-g/{f}.tsv'),delimiter='\t'):
    if r['rev']=='after' and r['valid']=='yes': d[(r['metric'],r['scenario'])].append(float(r['seconds']))
[print(k,statistics.median(v),v) for k,v in sorted(d.items())]"
```

## 6. A と違う数え方

- C7b は A の snippet のほかに、b0 の検査器で HEAD を測るために `snapshot.resolve` の base 側を B0 にした（b0 は HEAD の profile を読めないため）。
  A と同じ検査器の値（b0）と今の検査器の値（head）を両方 TSV に書いた。
- C10 の after の窓は baseline.md §7 C10 の「after（H）」の行のとおり REV を `$B..HEAD`、SINCE を 2026-11-17T00:00:00Z にした。数え方は A と同じ。
- ほかは A の snippet をそのまま使った。

## 7. 回帰条件

- LoC: 同じ規則（B0 の source-size-report）で production が 375,921 行を超えて増えたら、増えた区分と file を調べる（G の後の基準）。
- 変更波及: S1・S2 の file を選び直したとき、変える crate・C7b の import する単位（b0 で 92 / 369・6 / 14、head で 93 / 365・6 / 14）・`--list` の対象が上の表より増えたら原因を調べる。
- 時間: G の回帰条件（A の +10 % を超えたら before を同じ host で 3 回測り直す。health は同時に並べて rule ごとの計時）を使う。
  HEAD の crates/・scripts/・web/・Cargo.lock が G の測定の tree と変わったら、この表の値は G の値として写せない。health の S1・S2 を G の rules.md の手順で 3 回ずつ測り直す。

## 8. 変更（commit の件名）

- `arch-v4 H loc-cost: LoC・変更波及・修正の波及・build/test-affected/health の時間を A の規則で before と after にまとめる`（この md と loc-cost.tsv だけ）

## 9. 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- loc-cost.tsv の 23 key: WorkUnit の check の 1 番目の command を走らせ exit 0（36 行。key によっては検査器・数え方ごとに 2 行）。
- `grep -q 'source-size-report' loc-cost.md && grep -q '再現コマンド' loc-cost.md`: exit 0。
- `git diff --quiet "$(git merge-base HEAD main)" -- crates scripts web Cargo.toml Cargo.lock` と未追跡の source: 差分 0・未追跡 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` が出す path は loc-cost.md と loc-cost.tsv だけ。
- 全体の試験・clippy・fmt は走らせていない（source を変えていない。全体の試験は release gate だけで走らせる規則）。リリースゲートは未実施で、本番への昇格はしていない。
