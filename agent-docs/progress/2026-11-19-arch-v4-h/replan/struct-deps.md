---
title: アーキ v4 H R8a rc-deps — 循環・SCC の内部辺・handlers⇄cos・health の既存負債を R1〜R7 の後に数え直す
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# rc-deps（WorkUnit、提案 R8 の数え直し）

before は H の measure の after（[../struct-deps.tsv](../struct-deps.tsv) の after 列の写し）である。その commit は、件名 `integrate wu/close (phase close)` の main の commit で、
この WorkUnit の開始時の `git merge-base HEAD main` と同じ。after は提出時の HEAD（R1〜R7 の integrate が入った H の branch）。
数える key・scope・checker の文字列・手順は H の [struct-deps.md](../struct-deps.md)（§5 再現コマンド）と同じで、16 key・29 行を
[struct-deps.tsv](struct-deps.tsv) に置いた（列は同じ）。crates/・scripts/・web/ と policy の file は変えていない。

検査器の版は H と同じ 2 つである。
- checker=b0: B0（`bc665168c9caf1898adc4c611404bbee056daf9a`）の `scripts/dev/repo-health` を `git archive` で `$TMPDIR/rh-B0` に出したもの。
- checker=head: HEAD の `scripts/dev/repo-health`。H の measure の after から HEAD までに `scripts/dev/repo-health/`（`profiles/celeris.toml` を含む）の差分は 0 なので、
  H の measure の head と同じ検査器である。

## 0. 結論

- 循環の数と単位の数は変わらない。module の循環は 17 → 17、task-core の SCC は 55 単位、task-api の SCC は 49 単位のまま（両方の検査器で同じ）。
- task-api の SCC の単位の組は 261 → 228（−33、増えた組 0）。行の辺は head で 622 → 535（−87）、b0 で 618 → 535（−83）。
  内訳は R2 の −31 組・−84 行（head）、R4 の −1 行、R5 の −2 組・−2 行。
- task-core の SCC は組 210 → 210（組の集合も同じ）。行の辺は 621 → 627（+6、すべて R4 の `chat/attachments.rs → store`）。
- handlers⇄cos の行は両方向で減った。ただし両方向とも残っているので、単位の循環は解けていない。
  - handlers → cos は 19 → 14 行（12 → 9 file）。grep と両方の検査器で同じ値になった。
  - cos → handlers は 50 → 29 行（head と C5 の式）、`crate::` は 46 → 29 行（9 → 3 file）、`super::` は 4 → 0 行。
    `super::` が 0 になったので、b0 と head の検査器が同じ 29 を返す。
- health の既存負債は件数も finding の scope の集合も変わらない: `size.responsibility-growth` 94、`dup.contract` 210、`arch-map.wrong_owner` 31。
- `#[path]` の食い違いは task-dispatch 9・workspace 15 のまま。Cargo の辺は 49（normal 35・dev 14）、crate の循環は 0 のまま。
- R2 の進捗（[metrics.tsv](../../2026-11-19-arch-v4-h-r2/metrics.tsv)）の 9 行は、before も after も `integrate wu/r2-cos-edges (phase fix1)` での実測と一致した。
  HEAD との違いは ta_scc_pairs（230 → 228）と ta_scc_lines（538 → 535）の 2 key だけで、R4・R5 の変更による（§3）。
- 共通受入条件 (1) に当てると、組の単位で減ったのは task-api の SCC の 33 組（12.6 %）だけである。module の循環の数・SCC の単位の数・task-core の組は減っていない。
  done とするかは rewrite・finish の段と人の決定（final-unmet）に渡す。

## 1. B0・H の measure・replan の後の表

値は TSV の行ごと。B0 は H の struct-deps.tsv の before、H の measure は同じ TSV の after（= この TSV の before）、replan の後はこの TSV の after。

| 指標（TSV の key） | checker | B0 | H の measure | replan の後 |
|---|---|---|---|---|
| Cargo の依存の辺（cargo_edges） | - | 49 | 49 | 49 |
| crate の循環（crate_cycles） | - | 0 | 0 | 0 |
| module の循環（module_cycles、C2 の SCC の数） | b0 | 17 | 17 | 17 |
| module の循環（module_cycles、C1 の findings） | head | 17 | 17 | 17 |
| task-core の SCC の単位（tc_scc_units） | b0 / head | 55 / 55 | 55 / 55 | 55 / 55 |
| task-core の SCC の組（tc_scc_pairs） | b0 / head | 210 / 210 | 210 / 210 | 210 / 210 |
| task-core の SCC の行（tc_scc_lines） | b0 / head | 617 / 617 | 621 / 621 | 627 / 627 |
| task-api の SCC の単位（ta_scc_units） | b0 / head | 49 / 49 | 49 / 49 | 49 / 49 |
| task-api の SCC の組（ta_scc_pairs） | b0 / head | 261 / 261 | 261 / 261 | 228 / 228 |
| task-api の SCC の行（ta_scc_lines） | b0 / head | 628 / 632 | 618 / 622 | 535 / 535 |
| handlers → cos の行（handlers_to_cos_lines、grep） | - | 22 | 19 | 14 |
| handlers → cos の行（handlers_to_cos_lines、辺） | b0 / head | 22 / 22 | 19 / 19 | 14 / 14 |
| cos → handlers の行（cos_to_handlers_lines、grep の `crate::`） | - | 48 | 46 | 29 |
| cos → handlers の行（cos_to_handlers_lines、A の C5 の式） | - | 52 | 50 | 29 |
| cos → handlers の行（cos_to_handlers_lines、辺） | b0 / head | 48 / 52 | 46 / 50 | 29 / 29 |
| cos → handlers の `super::` の行（cos_to_handlers_super_lines） | - | 4 | 4 | 0 |
| responsibility-growth（responsibility_growth） | head | 94 | 94 | 94 |
| dup.contract（dup_contract） | head | 210 | 210 | 210 |
| owner 不一致（owner_mismatch） | head | 31 | 31 | 31 |
| `#[path]` の食い違い（path_attr_files、task-dispatch） | - | 9 | 9 | 9 |
| `#[path]` の食い違い（path_attr_files、workspace） | - | 15 | 15 | 15 |

参考（TSV の key ではない）:
- 循環の合計（全 17 SCC の単位・組・行）: head は 198・756・1,827（B0）→ 198・756・1,821（H の measure）→ 198・723・1,740（replan の後）。
  b0 は 192・742・1,731 → 192・742・1,725 → 192・709・1,648。H の measure から後の差（組 −33、行 head −81・b0 −77）は、task-api の差と task-core の +6 の和と一致する。
  ほかの 15 の SCC は変わっていない。
- dispatcher を含む SCC: b0 は 3 単位・4 組・22 行、head は 4・7・51 で、3 つの時点とも同じ（R3 は dispatcher の SCC の辺を変えていない）。

## 2. R ごとの寄与（head の検査器、各 integrate commit で測った）

| 時点（件名） | ta の組 | ta の行 | tc の行 | handlers → cos | cos → handlers（辺） |
|---|---|---|---|---|---|
| H の measure の after | 261 | 622 | 621 | 19 | 50 |
| `integrate wu/r2-cos-edges (phase fix1)`（R1 を含む） | 230 | 538 | 621 | 14 | 29 |
| `integrate wu/r3-dispatcher (phase fix1)` | 230 | 538 | 621 | 14 | 29 |
| `integrate wu/r4-conn (phase fix1)` | 230 | 537 | 627 | 14 | 29 |
| `integrate wu/r5-applied (phase fix2)` | 228 | 535 | 627 | 14 | 29 |
| HEAD（`integrate wu/r6-release (phase fix2)`） | 228 | 535 | 627 | 14 | 29 |

b0 の検査器は R2・R4・R5 の integrate commit で測り、ta の組は 230・230・228、ta の行は 538・537・535、tc の行は 621・627・627 だった（R2 の手前の ta の行は 618）。
- R1（`integrate wu/r1-prompts (phase fix1)`）の crates/ の差分は task-worker と task-dispatch の 5 file だけで、2 つの SCC に入る file は無い。R1 の後と R2 の後の差は
  R2 の進捗の before → after と同じなので、R1 の寄与は 0 である。
- R2: 組 −31（`<module> → handlers` 29 は HTTP の道具の移動、`notify → cos`・`releases → cos` の 2 は Effect の移動）、行 −84（head。b0 は −80。差の 4 行は
  `cos/ops/admin.rs` の `super::` の accounts の行）。足された辺は `cos/ops/admin.rs → middleware` の 2 行。
- R3: task-api・task-core の SCC は変わらない（変更は task-dispatch の 4 file）。
- R4: task-core の `chat/attachments.rs → store` +6、task-api の `schema.rs → cos` −1。
- R5: 組 −2（`lifecycle → cos`・`skills → cos`）、行 −8 +6（`cos/ops/admin.rs → skills` −2、`knowledge.rs → cos` −4、`lifecycle.rs → cos` −1、`skills.rs → cos` −1、
  `cos/ops/decisions.rs → knowledge` +6）。R5 の ADR の付記（2 組が消え、新しい組 0）と同じ。
- R6・R7a・R7b: crates/ の Rust の辺を変えていない（R6 の後で値は同じ。R7 の 2 つは docs だけ）。

## 3. R2 の進捗の値との照合

R2 の [metrics.tsv](../../2026-11-19-arch-v4-h-r2/metrics.tsv) の 9 行の before は、この TSV の before（H の measure の after）と同じだった。after は
`integrate wu/r2-cos-edges (phase fix1)` での実測（ta の組 230・行 538、handlers → cos 14、cos → handlers 29（grep の `crate::`・C5 の式・辺）、`super::` 0）と一致した。
HEAD との食い違いは次の 2 key で、理由は R2 の後の R4・R5 の変更である。
- ta_scc_pairs（head）: R2 の after 230、HEAD 228。R5 で `lifecycle → cos`・`skills → cos` の 2 組が消えた。
- ta_scc_lines（head）: R2 の after 538、HEAD 535。R4 で `schema.rs → cos` の 1 行、R5 で差し引き 2 行が消えた。

R2 の note は R2 の行の差を「消えた 91・増えた 7」と書いている。この WorkUnit の差分は (src の file, dst の単位) ごとの行の数の差なので、「消えた 86・増えた 2」になる。
同じ組の中で消えて足された行は差し引きされるので、どちらの数も 5 少ない。差し引きはどちらも −84 で同じである（R2 の note によると、足された 5 行は use の分割で
handlers に残した名前の行）。

## 4. R 以外の commit

- main は H の measure の after から進んでいない（`git log <H の measure の after>..main` は 0 件）。HEAD にある main の commit は H の measure の after とその祖先だけなので、
  R 以外の main の commit は無い。
- H の measure の after から HEAD までの first-parent の commit のうち、R の integrate でないもの（`integrate wu/loc-cost (phase measure)`・`integrate wu/struct-store (phase measure)`・
  `integrate wu/gate-cases (phase verify)`・`integrate wu/prompt-audit (phase verify)`・`integrate wu/review-prod (phase verify)`・`integrate wu/safety (phase verify)`・
  `integrate wu/draft (phase report)`）は、crates/・scripts/・web/ の file を 0 件しか変えていない（docs だけ）。

## 5. 数え方（H と同じ・違う点）

- H の struct-deps.md §2 の 1〜6 をそのまま使った。b0 の検査器は HEAD の profile も読めない（`profile_invalid: … review.high_risk[0] は class・paths だけ`。
  記録は成果物 dir の `raw/c2-b0-HEAD-own.txt`）ので、H と同じく profile は B0 から読み、測る tree を HEAD にした。
- C1 の b0 の行は H と同じく置かない（b0 × HEAD は同じ error で測れない）。head × HEAD の findings の scope の集合を H の measure の after（H の struct-deps の WorkUnit の成果物の
  `raw/c1-head-H.json.gz`）と比べ、4 rule（deps.cycle・size.responsibility-growth・dup.contract・arch-map.wrong_owner）とも差は 0 だった。
  `size.responsibility-growth` は scope が同じで evidence の行番号だけが変わった finding が 2 件ある（task-api の `state` の `struct Inner` 83–156 → 83–159、
  `cos` の `fn resolve` 459–700 → 460–701）。
- path_attr_files: HEAD の `git grep -n '#\[path' -- crates`（A §6 の X を除く）は H の measure の after より 1 行多い。足された行は
  `crates/task-worker/src/claude_code/tests.rs` の `#[path = "planner_prompt_tests.rs"]`（R1）で、試験の file なので H の規則（`*_tests.rs`・試験を除く）で数えない。
- 行の辺の差は、edges json の (src, dst_unit) ごとの Counter の差で取った（H と同じ）。
- docs/architecture-map.md の `#[path]` の 3 file（H の struct-deps.md §3）は、R7a で `#[path]` の行に足された（check_item・review_limits・inventory とも
  `git show HEAD:docs/architecture-map.md | grep '#\[path\]' | grep -c <name>` が 1）。TSV の key ではない。

## 6. 再現コマンド

repo の root で実行する。`ART` はこの run の成果物 dir の `raw/`（H の measure が使った `c2.py` の写しを置いた。第 4 引数で profile を読む rev を指定できる）、
`S` は H の struct-deps の WorkUnit の成果物の `raw/`（`/local/celeris/data/workspaces/01M7MZTKBPZPQE4XN451F8X84F/wu/struct-deps/artifacts/raw`。H の measure の after の edges と C1 の json がある）。

```sh
B0=bc665168c9caf1898adc4c611404bbee056daf9a; PA=$(git merge-base HEAD main)   # この WorkUnit の時点で H の measure の after と同じ commit
H=$(git rev-parse HEAD); O=$TMPDIR/out; mkdir -p $O; R=$TMPDIR/rh-B0; mkdir -p $R
git archive $B0 scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml | tar -x -C $R
git diff --stat $PA $H -- scripts/dev/repo-health                                   # 差分 0（checker=head は H の measure と同じ版）
# C1（1 分 52 秒）
python3 scripts/dev/repo-health/repo_health.py --repo . --base $H --head $H --policy-version 1 --out $O/c1-head-HEAD.json
#   deps.cycle 17・size.responsibility-growth 94・dup.contract 210・arch-map.wrong_owner 31。scope の集合は H の struct-deps.md §5 の方法で比べる（差 0）
# C2（各約 23 秒。並べて走らせた）
python3 $ART/c2.py scripts/dev/repo-health $H $O/edges-head-HEAD.json
python3 $ART/c2.py $R/scripts/dev/repo-health $H $O/edges-b0-HEAD.json $B0           # profile は B0 から読む
python3 $ART/c2.py $R/scripts/dev/repo-health $H $O/edges-b0-HEAD-own.json            # HEAD の profile では profile_invalid で止まる（exit 1）
#   R ごとの寄与（§2）: 同じ 2 本を各 integrate commit に当てる
for s in r2-cos-edges r3-dispatcher r4-conn r5-applied; do
  c=$(git log --first-parent --format=%H -1 --grep="^integrate wu/$s " HEAD)
  python3 $ART/c2.py scripts/dev/repo-health $c $O/edges-head-$s.json
  python3 $ART/c2.py $R/scripts/dev/repo-health $c $O/edges-b0-$s.json $B0
done
#   key の取り出しと差: 下の keys.py（成果物 dir の tools/keys.py）。第 2 引数を与えると (src, dst_unit) ごとの行の差と組の差を出す
python3 keys.py $O/edges-head-HEAD.json $S/edges-head-H.json.gz
python3 keys.py $O/edges-b0-HEAD.json $S/edges-b0-H.json.gz
# C5（A §7 C5）
X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'
for B in $PA $H; do
  git grep -nE 'crate::cos::' $B -- crates/task-api/src/handlers crates/task-api/src/handlers.rs | grep -vE "$X" | wc -l   # 19 / 14
  git grep -nE '(crate|super(::super)*)::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                    # 50 / 29
  git grep -nE 'crate::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                                    # 46 / 29
  git grep -nE 'super(::super)*::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                          # 4 / 0
  git grep -n '#\[path' $B -- crates | grep -vE "$X" | sed -E "s/^$B://; s/:[0-9]+:/: /" > $O/path-attr-$B.txt
done
diff $O/path-attr-$PA.txt $O/path-attr-$H.txt        # 足された行は claude_code/tests.rs の planner_prompt_tests.rs の 1 行だけ
# C7（cargo metadata。H の struct-deps.md §5 と同じ数え方）
git diff --stat $PA $H -- Cargo.toml Cargo.lock 'crates/*/Cargo.toml'           # 差分 0
cargo metadata --format-version 1 --offline > $O/meta-HEAD.json                    # members 15、edges 49（normal 35・dev 14）、crate_cycles 0
# R 以外の main の commit（§4）
git log --oneline $PA..main | wc -l                                                 # 0
git log --first-parent --format='%h %s' $PA..HEAD | while read h s; do echo "$(git diff --name-only $h^1 $h -- crates scripts web | wc -l) $s"; done
```

`keys.py`（成果物 dir の `tools/keys.py` の数え方を短くした版。出力の形は違うが、同じ値を出すことを HEAD で確かめた）:

```python
import sys,json,gzip,collections
def load(p):
    f=gzip.open(p,'rt') if p.endswith('.gz') else open(p)
    return json.load(f)
TC='crates/task-core/src/store'; TA='crates/task-api/src/handlers'; CO='crates/task-api/src/cos'
def keys(d):
    cy=d['cycles']; r={'module_cycles':len(cy)}
    r['total']=[sum(c['unit_count'] for c in cy),sum(c['unit_pair_edges'] for c in cy),sum(c['line_edges'] for c in cy)]
    for c in cy:
        if TC in c['units']: r['tc']=(c['unit_count'],c['unit_pair_edges'],c['line_edges'],c)
        if TA in c['units']: r['ta']=(c['unit_count'],c['unit_pair_edges'],c['line_edges'],c)
    ta=r['ta'][3]
    r['h2c']=sum(1 for e in ta['edges'] if e['src_unit']==TA and e['dst_unit']==CO)
    r['c2h']=sum(1 for e in ta['edges'] if e['src_unit']==CO and e['dst_unit']==TA)
    return r
a=keys(load(sys.argv[1]))
print('module_cycles',a['module_cycles'],'total',a['total'])
print('tc',a['tc'][:3],'ta',a['ta'][:3],'handlers->cos',a['h2c'],'cos->handlers',a['c2h'])
if len(sys.argv)>2:
    b=keys(load(sys.argv[2]))
    for k in ('tc','ta'):
        cb,ca=b[k][3],a[k][3]
        pb={(e['src_unit'],e['dst_unit']) for e in cb['edges']}; pa={(e['src_unit'],e['dst_unit']) for e in ca['edges']}
        print(k,'pairs removed',sorted(pb-pa),'added',sorted(pa-pb))
        lb=collections.Counter((e['src'],e['dst_unit']) for e in cb['edges']); la=collections.Counter((e['src'],e['dst_unit']) for e in ca['edges'])
        print(k,'lines removed',dict(lb-la),'added',dict(la-lb))
```

生の記録（この run の成果物 dir の `raw/`）: `c2.py`、`c2-{head,b0}-*.txt`（HEAD と各 integrate commit）、`c2-b0-HEAD-own.txt`、`c1-head-HEAD.{out,time,json.gz}`、
`edges-*.json.gz`、`path-attr-*.txt`。測った後に `git status --porcelain -- crates scripts web` は空だった。
