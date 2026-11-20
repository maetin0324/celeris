---
title: アーキ v4 H R8c rc-loc — LoC・変更波及・health と warm check の時間を R1〜R7 の後に数え直す
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# rc-loc（WorkUnit、提案 R8 の数え直し）

before は H の measure の after（[../loc-cost.tsv](../loc-cost.tsv) の after 列の写し）である。その source の commit は件名 `integrate wu/close (phase close)` の main の commit
（この WorkUnit の開始時の `git merge-base HEAD main` と同じ。以下 PA）。after は提出時の HEAD（R1〜R7 の integrate と rc-deps の commit が入った H の branch）。
key・scope・checker の文字列・手順は H の [loc-cost.md](../loc-cost.md)（§5 再現コマンド）と同じで、23 key・36 行を [loc-cost.tsv](loc-cost.tsv) に置いた（列は同じ）。
crates/・scripts/・web/ と policy の file は変えていない（`--list` と時間の測定の probe は各回の後に `git checkout --` で戻した。§4）。

## 0. 結論

- (6) LoC（分類規則は B0 に固定）: H の measure の後、production は全言語・Rust とも **+97 行**（375,921 → 376,018）。B0 からは **+919 行**（375,099 → 376,018）で、
  R1〜R7 の後も production は純減していない。試験は +3,279 行（Rust +3,177）、own_total_with_tests は +3,460 行。
- (4) 変更波及: S1・S2 の変える file・単位・crate は H の measure と同じ（S1 4・4・2、S2 4・2・1）。`--list` の対象も同じ（S1 14 crate ＋ web ＋ ts-verify、S2 5 crate）。
  C7b の S1 を直接 import する単位は **93 → 77（head）、92 → 76（b0）に減った**（R2 が HTTP の道具を `http_io` へ移し、17 module が handlers を import しなくなった）。
  推移的に import する単位は 365 → 368（head）・369 → 372（b0）に **3 増えた**（R2 の `http_io`・`account_write` と R4 の task-core の `cos_inbox` が足された単位）。
  S2 は 6 / 14 のまま。変える crate と test-affected の対象は減っていない。
- 修正の波及（C10）: B0 の後の窓の with-merges は 9 → 14 件（R1〜R5 の integrate の 5 件）。件数が少なく、WU 単位の merge を 1 件と数えるので、A の窓と割合は比べられない（H と同じ）。
- (5) 時間: 測り直した 4 key は health:S1 93.58 → **98.74 s**（A の +14.1 %）、health:S2 96.09 → **100.68 s**（A の +22.9 %）、
  check warm S1 42.64 → 43.08 s、check warm S2 15.56 → 14.76 s。health は両方とも A の +10 % を超えた。H の値からの増え（+5.5 %・+4.8 %）が
  コードの変更によるのか、時間帯の host の差によるのかは切り分けていない（同じ時間帯の before の測り直しはしていない。§6 の提案）。
  ほかの 6 key（check cold・test build 3 つ・test-affected 2 つ）は H の値を写し、測り直していない。

## 1. B0・H の measure・replan の後の表

B0 は H の loc-cost.tsv の before、H の measure は同じ TSV の after（= この TSV の before）、replan の後はこの TSV の after。

| 指標（TSV の key） | checker | B0 | H の measure | replan の後 |
|---|---|---|---|---|
| production 全言語（loc_prod_all） | ssr@bc665168 | 375,099 | 375,921 | **376,018** |
| production Rust（loc_prod_rust） | ssr@bc665168 | 330,408 | 331,230 | **331,327** |
| 試験（loc_test） | ssr@bc665168 | 476,771 | 481,514 | 484,793 |
| own_total_with_tests 全言語（loc_own_total） | ssr@bc665168 | 1,025,677 | 1,031,506 | 1,034,966 |
| own_total_with_tests Rust（loc_own_total_rust） | ssr@bc665168 | 879,344 | 884,431 | 887,705 |
| tooling（loc_tooling） | ssr@bc665168 | 25,253 | 25,517 | 25,601 |
| S1 の file・単位・crate（s1_files・s1_units・s1_crates） | - | 3・3・2 | 4・4・2 | 4・4・2 |
| S2 の file・単位・crate（s2_files・s2_units・s2_crates） | - | 4・2・1 | 4・2・1 | 4・2・1 |
| S1 を直接 import する単位（s1_direct_importers） | b0 / head | 92 / 93（before'） | 92 / 93 | **76 / 77** |
| S1 を推移的に import する単位（s1_transitive_importers） | b0 / head | 369 / 365（before'） | 369 / 365 | 372 / 368 |
| S2 を直接・推移的に import する単位（s2_direct_importers・s2_transitive_importers） | b0 | 6・14 | 6・14 | 6・14 |
| test-affected の crate（s1_ta_crates・s2_ta_crates） | - | 14・5 | 14・5 | 14・5 |
| C10 no-merges の件数・複数 region %・複数 module %（fix_*） | ssr@bc665168 | 194・46.9・69.1（A の窓） | 1・0.0・0.0 | 1・0.0・0.0 |
| C10 with-merges の件数・複数 region %・複数 module %（fix_*） | ssr@bc665168 | 246・49.6・71.1（A の窓） | 9・55.6・55.6 | 14・42.9・57.1 |
| check cold（check_cold、s） | - | 53.65 | 53.90 | 53.90（写し） |
| check warm S1（check_warm_s1、s） | - | 42.47 | 42.64 | **43.08** |
| check warm S2（check_warm_s2、s） | - | 14.67 | 15.56 | **14.76** |
| test build cold・warm S1・warm S2（test_build_*、s） | - | 142.39・138.69・77.64 | 147.03・138.10・81.60 | 同じ（写し） |
| health S1（health_s1、s） | head | 86.56 | 93.58 | **98.74** |
| health S2（health_s2、s） | head | 81.92 | 96.09 | **100.68** |
| test-affected S1・S2（test_affected_*、s） | - | 893.41・655.59 | 906.03・707.76 | 同じ（写し） |

- LoC の言語別の差（H の measure → HEAD、B0 の source-size-report の `matrix`）: production Rust +97、external_test Rust +3,177・shell +102、tooling shell +84、
  fixture shell +135、docs markdown +8,136・other +702・shell +473。production のほかの言語の差は 0 行。
- C7b の「H の measure」の列は PA に同じ snippet を当て直して再現した（head 93 / 365、b0 92 / 369、S2 6 / 14）。
- C7b の集合の差（head の検査器、S1 の 4 単位、PA → HEAD）:
  - 直接 import する単位から消えた 17: task-api の `browser`・`browser_identity`・`conversation`・`harnesses`・`llm_sources`・`mcp_admin`・`memory`・
    `milestones`・`project_plan`・`releases`・`reports`・`routing`・`routing_eval`・`routing_stage`・`run_events`・`script_behavior_changes`・`tree`。
    足された 1: task-core の `cos_inbox`（R4）。
  - 推移的に import する単位に足された 3: task-api の `http_io`・`account_write`（R2）、task-core の `cos_inbox`（R4）。消えた単位は 0。
- C10 の with-merges で足された 5 件: `integrate wu/r1-prompts (phase fix1)`（1 file）、`integrate wu/r2-cos-edges (phase fix1)`（67 file、task-api だけ・複数 module）、
  `integrate wu/r3-dispatcher (phase fix1)`（4 file、dispatcher だけ）、`integrate wu/r4-conn (phase fix1)`（11 file、task-api・task-core）、
  `integrate wu/r5-applied (phase fix2)`（6 file、task-api だけ・複数 module）。R6 の差分は scripts/selfdeploy で、B0 の規則では production でない。R7 は docs だけ。

## 2. R 以外の commit

- main は PA から進んでいない（`git log PA..main` は 0 件）。HEAD にある main の commit は PA とその祖先だけなので、**R 以外の main の commit は無い**。
- PA から HEAD までの first-parent の commit のうち R の integrate でないもの（`integrate wu/loc-cost (phase measure)`・`integrate wu/struct-store (phase measure)`・
  `integrate wu/gate-cases (phase verify)`・`integrate wu/prompt-audit (phase verify)`・`integrate wu/review-prod (phase verify)`・`integrate wu/safety (phase verify)`・
  `integrate wu/draft (phase report)` と rc-deps の `arch-v4 H R8a rc-deps: …`）は docs だけを変えている（rc-deps の [struct-deps.md](struct-deps.md) §4 と同じ）。

## 3. (6) LoC の測定（ADR 2026-11-16-reduction-goal-delivery、分類規則は B0 に固定）

`git show bc665168:scripts/dev/source-size-report.py` と `.toml` を `$TMPDIR` に出し、B0・PA・HEAD に `--exclusive-classes --rev <rev> --format json` で当てた。
3 つの JSON の `config_sha256` は同じ値だった（同じ規則）。B0 と PA の値は H の loc-cost.tsv の before と after に一致した。

## 4. (5) health と warm check の測り直し（G の rules.md の手順）

- 手順: G の [rules.md](../../2026-11-19-arch-v4-g/rules.md) の「## TSV」「## probe と戻し方」の規則で、G の scope と build-check の run の driver
  （`c1-health-tsv.sh`・`build-check.sh`・`probe-const.sh`。WorkUnit の成果物 dir の `raw/` に写した）をそのまま走らせた。各回の直前に
  waitload（開始時の 1 分 load average が 4 以下になるまで待つ）を通した。6 つの組とも 4 以下で始まり、待ちは 0 s、valid=no の回は無い。
- health の `--base` は HEAD（G は `AFTER_REV=$H` で、測った branch の HEAD。同じ意味）。`--head worktree` で、測っている間 repo の中に未追跡の file は無かった
  （TSV と md は測り終えてから書いた）。warm check は各回の前に `git checkout -- <file>` → 計らない check（prep 14.6〜44.3 s、exit 0）→ const の probe。
- build-check.sh は G の `AFTER_CRATES_TREE` と HEAD の `crates/` の tree が違うと `src_tree … != AFTER_CRATES_TREE …` の 1 行を出す（S1・S2 で各 1 回出た）。R1〜R6 が crates/ を変えたためで、
  測定には影響しない。
- 測った時刻: health 2026-11-20T10:21:57Z〜10:31:54Z、warm check 10:32:03Z〜10:38:22Z。

| key | 3 回の wall（s） | 中央値 | 開始時の 1 分 load | user の中央値 | exit | 内訳 |
|---|---|---|---|---|---|---|
| health_s1 | 98.74・99.39・96.31 | **98.74** | 1.71・2.07・2.77 | 74.95 s | 1・1・1（const の probe で fail が手順どおり） | fail は size.responsibility-growth。findings 698・new 3・worsened 1 |
| health_s2 | 91.47・109.87・100.68 | **100.68** | 1.73・1.35・2.40 | 74.77 s | 1・1・1 | fail は size.responsibility-growth・source-size。findings 699・new 4・worsened 2 |
| check_warm_s1 | 43.02・43.18・43.08 | **43.08** | 2.68・1.93・1.77 | 58.76 s | 0・0・0 | prep 44.3・42.8・43.4 s |
| check_warm_s2 | 14.76・15.96・14.56 | **14.76** | 2.05・2.34・2.35 | 18.90 s | 0・0・0 | prep 43.4・15.5・14.6 s |

- 比（after ÷ A、after ÷ H の値）: health:S1 1.141・1.055、health:S2 1.229・1.048、check warm S1 1.014・1.010、check warm S2 1.006・0.949。
- health の user の時間も G の after（S1 71.19 s、S2 71.86 s）より 3.8 s・2.9 s（+5.3 %・+4.0 %）多い。findings は G の after（S1 699・new 4、S2 700・new 5）より 1 件少ない。
  user の増えから、wall の増えの一部は検査器が読む tree の増え（docs +9,311 行、試験 +3,279 行など）による可能性がある。未確認。
- 測り直していない 6 key: check_cold・test_build_cold・test_build_warm_s1・test_build_warm_s2 は G の tree の値（この WorkUnit の objective が測り直す key を 4 つに限った）、
  test_affected_s1・test_affected_s2 は規則（test-affected の時間は測り直さない）。R1〜R6 で crates/ が変わったので、この 6 key の after は HEAD の tree の値ではない。

## 5. 再現コマンド

repo の root で実行する。`ART` はこの WorkUnit の成果物 dir の `raw/`（`/local/celeris/data/workspaces/01M7MZTKBPZPQE4XN451F8X84F/wu/rc-loc/artifacts/raw`。
H の loc-cost の WorkUnit の `c7b.py`・`c10.py`・`c10-list.py`・`probe-comment.sh` と G の `c1-health-tsv.sh`・`build-check.sh`・`probe-const.sh` の写しと生の出力がある）。

```sh
B=bc665168c9caf1898adc4c611404bbee056daf9a; PA=$(git merge-base HEAD main)   # この WorkUnit の時点で H の measure の after と同じ commit
# (6) LoC（source-size-report の分類規則は B0 に固定）
git show $B:scripts/dev/source-size-report.py > $TMPDIR/ssr.py; git show $B:scripts/dev/source-size-report.toml > $TMPDIR/ssr.toml
for r in $B $PA HEAD; do python3 -I $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev $r --format json --out $ART/ssr-$(git rev-parse --short=8 $r).json; done
#   views.product_only.lines / rust_lines、class_totals.external_test + inline_test、views.own_total_with_tests.lines / rust_lines、class_totals.tooling。
#   言語別は matrix.<class>.<lang>.lines。config_sha256 が 3 つで同じことを確かめる

# (4) S1・S2 の file（G の scenarios.env）と C7b
. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env
ARGS="S1h:crates/task-api/src/org_write,crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store S1:crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store S2:crates/task-dispatch/src/dispatcher,crates/task-dispatch/src/run_settlement"
python3 $ART/c7b.py HEAD $ARGS; python3 $ART/c7b.py $PA $ARGS                       # head の検査器（PA から scripts/dev/repo-health の差分 0）
mkdir -p $TMPDIR/rh-B0 && git archive $B scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml | tar -x -C $TMPDIR/rh-B0
sed "s#sys.path.insert(0,'scripts/dev/repo-health')#sys.path.insert(0,'$TMPDIR/rh-B0/scripts/dev/repo-health')#; s#snap=snapshot.resolve('.',B,B)#snap=snapshot.resolve('.',B.split('@')[-1],B.split('@')[0])#" $ART/c7b.py > $ART/c7b-b0.py
python3 $ART/c7b-b0.py HEAD@$B $ARGS; python3 $ART/c7b-b0.py $PA@$B $ARGS          # b0 の検査器（profile は B0 から読み、測る tree は HEAD・PA）
#   集合の差: c7b.py の print を集合の出力に替えた写し（$ART/c7b-sets.py）を PA と HEAD に当てて比べる

# (4) test-affected の対象（comment の probe、戻して空を確かめる）
$ART/probe-comment.sh $S1_FILES; git diff --stat -- crates; bash scripts/dev/test-affected.sh --list; git checkout -- $S1_FILES
$ART/probe-comment.sh $S2_FILES; git diff --stat -- crates; bash scripts/dev/test-affected.sh --list; git checkout -- $S2_FILES
git status --porcelain -- crates                                                     # 空であること

# 修正の波及（A §7 C10 の snippet。B0 の後の窓）
for m in no-merges with-merges; do python3 $ART/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-11-17T00:00:00Z $m "$B..HEAD"; done
python3 $ART/c10-list.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-11-17T00:00:00Z with-merges "$B..HEAD"   # commit ごとの内訳

# (5) health:S1・S2 と check warm S1・S2（G の rules.md。waitload・const の probe・valid 3 回・TSV に 1 回 1 行）
TSV=$ART/health.tsv OUT=$ART DEADLINE=$(( $(date +%s) + 1500 )) AFTER_REV=$(git rev-parse HEAD) S1_FILES="$S1_FILES" S2_FILES="$S2_FILES" bash $ART/c1-health-tsv.sh
for s in S1 S2; do TSV=$ART/build-check.tsv OUT=$ART DEADLINE=$(( $(date +%s) + 900 )) bash $ART/build-check.sh warm $s; done
#   中央値は TSV の valid=yes の 3 行の seconds の中央値。終わったら git status --porcelain が空であること
```

## 6. H と違う点・提案

- H の measure は 10 組の時間をすべて G の値から写した（source の差分 0）。この WorkUnit は R1〜R6 で crates/ が変わったので、objective の指示どおり 4 key を測り直し、
  6 key は写した。手順・検査器の版・key・scope・checker の文字列は H と同じ。
- C7b・C10・LoC は H と同じ snippet を同じ引数で HEAD に当てた。数え方の違いは無い。
- 提案（起票していない。起票は人が決める）:
  1. health は H の値からも +5 % 前後増え、A の +10 % を両方の S で超えた。G の回帰条件（A の +10 % を超えたら before を同じ host で 3 回測り直す）に当たるので、
     G の control-health.sh の手順で、PA の tree と HEAD を同じ時間帯に交互に 3 回ずつ測り、コード由来の差（rule ごとの計時）を切り分ける。
  2. 写した 6 key（cargo の cold と test build、test-affected）は HEAD の tree の値ではない。R の後の値が要るなら、release の後か次の計測の task で測り直す。

## 7. 回帰条件

- LoC: B0 の source-size-report の規則で production が 376,018 行を超えて増えたら、増えた区分と file を調べる（R の後の基準）。
- 変更波及: S1・S2 の file を選び直したとき、変える crate・C7b の import する単位（S1 は b0 で 76 / 372・head で 77 / 368、S2 は 6 / 14）・`--list` の対象が
  上の表より増えたら原因を調べる。
- 時間: G の回帰条件（A の +10 % を超えたら before を同じ host で 3 回測り直す。health は同時に並べて rule ごとの計時）を使う。health:S1・S2 は今この条件に当たっている（§6 の提案 1）。

## 8. 変更（commit の件名）

- `arch-v4 H R8c rc-loc: LoC・変更波及・health と warm check の時間を R1〜R7 の後の HEAD で数え直し、replan/loc-cost.tsv と .md を書く`（この md と loc-cost.tsv だけ）

## 9. 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- WorkUnit の check の 1〜4（23 key が数で入る・before が H の after と一致・md の語・範囲）を走らせた。結果は WorkUnit の報告に書く。
- crates/・scripts/・web/ は変えていない（`git status --porcelain -- crates` は各測定の後に空）。全体の試験・clippy・fmt は source を変えていないので走らせていない。
  リリースゲートは未実施で、本番への昇格はしていない。
