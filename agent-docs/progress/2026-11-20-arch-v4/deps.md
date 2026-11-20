---
title: アーキ v4 親 closeout deps — H の (1)(3) 循環・SCC の内部辺・handlers⇄cos・Cargo の辺・#[path] を HEAD で測り直し照合する
tasks: [01M7MZT8KX547464YYS76QNP4G]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# deps（親 01M7MZT8KX547464YYS76QNP4G の closeout の再計測、WorkUnit）

H の [replan/struct-deps.tsv](../2026-11-19-arch-v4-h/replan/struct-deps.tsv) の全 29 行（16 key）を HEAD で測り直し、[deps.tsv](deps.tsv) に書いた。
HEAD は main（子 A〜H の入った状態）を取り込んだ親の branch で、件名は `integrate wu/sync (phase sync)`。
crates/・scripts/・web/・Cargo.* と policy の file は変えていない（変えた file はこの md と deps.tsv だけ）。

## 0. 結論

- 29 行のうち、一致（yes）29 行、説明できる差（explained）0 行、説明できない差（no）0 行、時間の行（timing）0 行（struct-deps.tsv に時間の行は無い）。
- H が測った HEAD は `integrate wu/r6-release (phase fix2)`（`7be366f75aeaa533814a13d081153f90422da704`、main の祖先）である。
  根拠: H の rc-deps の WorkUnit の成果物 `raw/head.txt` がこの commit を指し、[replan/struct-deps.md](../2026-11-19-arch-v4-h/replan/struct-deps.md) §2 の表の HEAD の行も同じ件名である。
- その commit と HEAD の間で crates/・scripts/・web/・Cargo.toml・Cargo.lock・crates/*/Cargo.toml の差分は 0 file である（§2）。そのため値が変わる理由は無く、実測も全行一致した。
- 一致は件数だけではない。
  - C2（辺の snippet）の edges json の `cycles` は、head と b0 の両方の検査器で H の json と完全に同じだった。
  - C1（repo_health）の head×HEAD の findings は、snapshot の rev を除いて H の json と finding ごとに同じだった（4 rule とも scope の集合の差 0）。
  - C1 の head×B0・b0×B0 は、H の struct-deps の WorkUnit の B0 の記録と finding の json まで同じだった（B0 の before の値の確認。deps.tsv の行ではない）。
- H の値の意味（変わっていない）: module の循環 17、task-core の SCC 55 単位・210 組・627 行、task-api の SCC 49 単位・228 組・535 行、handlers→cos 14 行、cos→handlers 29 行。
  handlers⇄cos は両方向とも残り、単位の循環は解けていない（H の未達の記録のとおり）。

## 1. 測った値（deps.tsv の要約）

| key | checker | H の after | 今の HEAD | match |
|---|---|---|---|---|
| cargo_edges | - | 49 | 49（normal 35・dev 14） | yes |
| crate_cycles | - | 0 | 0 | yes |
| module_cycles（C2 の SCC の数 / C1 の findings） | b0 / head | 17 / 17 | 17 / 17 | yes |
| tc_scc_units | b0 / head | 55 / 55 | 55 / 55 | yes |
| tc_scc_pairs | b0 / head | 210 / 210 | 210 / 210 | yes |
| tc_scc_lines | b0 / head | 627 / 627 | 627 / 627 | yes |
| ta_scc_units | b0 / head | 49 / 49 | 49 / 49 | yes |
| ta_scc_pairs | b0 / head | 228 / 228 | 228 / 228 | yes |
| ta_scc_lines | b0 / head | 535 / 535 | 535 / 535 | yes |
| handlers_to_cos_lines（grep / 辺） | - / b0 / head | 14 / 14 / 14 | 14 / 14 / 14（9 file） | yes |
| cos_to_handlers_lines（grep の `crate::` / A の C5 の式 / 辺） | - / - / b0 / head | 29 / 29 / 29 / 29 | 29 / 29 / 29 / 29（3 file） | yes |
| cos_to_handlers_super_lines | - | 0 | 0 | yes |
| responsibility_growth | head | 94 | 94 | yes |
| dup_contract | head | 210 | 210 | yes |
| owner_mismatch | head | 31 | 31 | yes |
| path_attr_files（task-dispatch / workspace） | - | 9 / 15 | 9 / 15 | yes |

参考（deps.tsv の行ではない）:
- 循環の合計（全 17 SCC の単位・組・行）: head 198・723・1,740、b0 192・709・1,648。H の replan の後の値と同じ。
- C1 の B0 の値: head×B0 は deps.cycle 17・size.responsibility-growth 94・dup.contract 210・arch-map.wrong_owner 31（findings 697）。
  b0×B0 は 17・94・210・0（findings 557。arch-map は `checker_missing`、exit 2 の `verdict=error`。H の記録と同じ）。
- C1 の所要時間は 3 本を同時に走らせて各 1 分 42 秒（H の rc-deps の head×HEAD は単独で 1 分 52 秒）。時間は deps.tsv の key ではないので照合しない。
- C5 の B0 の値も H と同じ: handlers→cos 22 行・13 file、cos→handlers は A の式 52・`crate::` 48・`super::` 4。
- docs/architecture-map.md の `#[path]` の行には 8 file（design_gate・design_verdict・plan_adoption・policy_review・worker_profile・check_item・review_limits・inventory）が各 1 回出る。
  H の replan/struct-deps.md §5 の記述（R7a で後ろの 3 件が足された）と同じ。
- `#[path]` の 9・15 は、H の struct-deps.md §2 の 4 の規則（物理の単位と論理の単位が違う production の宣言）を数える script で数え直した。
  task-dispatch 9（dispatcher → health_review の 8 と lib.rs → dispatcher/project_decider_ts.rs の 1）、task-worker 5、task-core 1。B0 でも同じ 9・15。

## 2. H が測った HEAD と今の HEAD の差

`git diff --name-only 7be366f75aeaa533814a13d081153f90422da704 HEAD -- crates scripts web Cargo.toml Cargo.lock 'crates/*/Cargo.toml'` は 0 行である。
`scripts/dev/repo-health/`（`profiles/celeris.toml` を含む）の差分も 0 なので、checker=head は H と同じ版の検査器である。

間にある merge でない commit は次の 10 件で、どれも crates/・scripts/・web/・Cargo.* の file を 0 件しか変えていない（agent-docs/ の H の進捗と final-report.md だけ）。
- `arch-v4 H R8a rc-deps: 循環・SCC の内部辺・handlers⇄cos・health の既存負債を R1〜R7 の後の HEAD で数え直し、replan/struct-deps.tsv と .md を書く`
- `arch-v4 H R8b struct-store: R1〜R7 の後の Store の公開面・SQLite 参照・Dispatcher の参照・Applied の形を数え直し、conn_open_files を足す`
- `arch-v4 H R8c rc-loc: LoC・変更波及・health と warm check の時間を R1〜R7 の後の HEAD で数え直し、replan/loc-cost.tsv と .md を書く`
- `arch-v4 H R8c rc-loc: md から commit でない hash を消す`
- `arch-v4 H R8d rc-gate: R1〜R7 の後の HEAD で判定経路・prompt・反例の試験と本物の検査器の probe を再実行し replan/gate.md・gate.tsv に書く`
- `arch-v4 H R8e rewrite: final-report.md に replan の後の節を足し、未達・要判断の表と結論・回帰条件・リリースゲートを R1〜R7 の後の値で書き直す`
- `arch-v4 H R8f finish: 決定 final-unmet（c）に従い進捗の本体を blocked にし、replan の後の節を指す`
- `arch-v4 H R8f finish: 人の答え a に従い進捗の本体を done で閉じ、残る未達の一覧と review の比較の担当（operator）を書く`
- `arch-v4 H R9 publish: final-report.md の review の比較の担当を operator にし、task の成果物 dir に写す`
- `arch-v4 H R9 publish: 進捗の本体の証拠から旧い担当の語句の引用を消す`

merge の commit は H の integrate（`integrate wu/rc-gate (phase recount)`・`integrate wu/rc-loc (phase recount)`・`integrate wu/rc-store (phase recount)`・
`integrate wu/rewrite (phase rewrite)`・`integrate wu/finish (phase final)`・`integrate wu/publish (phase publish)`）と、親の sync
（`arch-v4 親 sync: 子 A〜H の入った main を取り込む`・`integrate wu/sync (phase sync)`）である。main は HEAD より先に進んでいない（`git log HEAD..main` は 0 件）。

## 3. 数え方（H と同じ・違う点）

- H の [struct-deps.md](../2026-11-19-arch-v4-h/struct-deps.md) §2 の 1〜6 と §5、[replan/struct-deps.md](../2026-11-19-arch-v4-h/replan/struct-deps.md) §5・§6 の手順をそのまま使った。
  b0 の検査器は B0（`bc665168c9caf1898adc4c611404bbee056daf9a`）の `scripts/dev/repo-health` を `git archive` で出したもの。HEAD の profile は読めない（`profile_invalid`）ので、
  H と同じく profile は B0 から読み、測る tree を HEAD にした。C1 の b0 の行は H と同じく置かない。
- C2 の snippet は H の rc-deps の WorkUnit の `raw/c2.py` を写した。G の `raw/c2.py` に H の struct-deps.md §5 の sed を当てた結果と `diff` で同じであることを確かめた。
- C1 の findings の比較は、rule_id ごとの scope の集合の差（H と同じ）に加えて、finding の json 全体の比較もした。head×HEAD は snapshot の rev（測った commit）だけが違い、
  rev を置き換えると全 695 件が一致した。
- `#[path]` は、H と同じく `git grep` の一覧（行番号を除く）を H の rc-deps の記録と `diff` で比べた（差 0）。加えて、9・15 を規則どおりに数える script（`tools/path_units.py`。
  宣言の相対 path を物理の path にし、`#[path]` で読む file の論理の単位を宣言側から引き継ぎ、lib.rs の宣言は `crates/<c>/src/<mod 名>` を論理の単位にする）で数え直した。
  H は 15 の内訳を手で数えたと書いている。この script は H の数え方を変えずに機械で再現したもので、値は同じだった。
- cargo_edges は cargo metadata の workspace_members の dependencies のうち、path のある workspace の crate を (依存先, kind) ごとに 1 回数えた（`tools/cargo_edges.py`）。
  crate の循環は normal の辺の強連結成分（2 crate 以上）。

## 再現コマンド

repo の root で実行する。`A` はこの run の成果物 dir（`/local/celeris/data/workspaces/01M7MZT8KX547464YYS76QNP4G/wu/deps/artifacts`。`raw/` に生の記録、
`tools/` に keys.py・c1cmp.py・path_units.py・cargo_edges.py）、`RC` は H の rc-deps の WorkUnit の成果物の `raw/`、`S` は H の struct-deps の WorkUnit の成果物の `raw/`。
keys.py は H の replan/struct-deps.md §6 の keys.py に、`cycles` の json の一致を出す 3 行を足したもの。

```sh
B0=bc665168c9caf1898adc4c611404bbee056daf9a; HM=7be366f75aeaa533814a13d081153f90422da704   # HM = H が測った HEAD（integrate wu/r6-release (phase fix2)）
H=$(git rev-parse HEAD); O=$A/raw; R=$TMPDIR/rh-B0; mkdir -p $O $R
RC=/local/celeris/data/workspaces/01M7MZTKBPZPQE4XN451F8X84F/wu/rc-deps/artifacts/raw
S=/local/celeris/data/workspaces/01M7MZTKBPZPQE4XN451F8X84F/wu/struct-deps/artifacts/raw
git archive $B0 scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml | tar -x -C $R
git diff --name-only $HM $H -- crates scripts web Cargo.toml Cargo.lock 'crates/*/Cargo.toml' | wc -l          # 0
cp $RC/c2.py $O/c2.py
# C1（3 本を同時に。各 1 分 42 秒）
python3 scripts/dev/repo-health/repo_health.py --repo . --base $B0 --head $B0 --policy-version 1 --out $O/c1-head-B0.json   # exit 0
python3 scripts/dev/repo-health/repo_health.py --repo . --base $H --head $H --policy-version 1 --out $O/c1-head-HEAD.json    # exit 0
python3 $R/scripts/dev/repo-health/repo_health.py --repo . --base $B0 --head $B0 --policy-version 1 --out $O/c1-b0-B0.json  # exit 2（verdict=error、H と同じ）
python3 $A/tools/c1cmp.py $O/c1-head-HEAD.json $RC/c1-head-HEAD.json.gz   # 17・94・210・31、4 rule の scope の差 0
python3 $A/tools/c1cmp.py $O/c1-head-B0.json $S/c1-head-B0.json.gz       # finding の json まで一致
python3 $A/tools/c1cmp.py $O/c1-b0-B0.json $S/c1-b0-B0.json.gz           # finding の json まで一致
# C2（3 本を同時に。各約 23 秒）
python3 $O/c2.py scripts/dev/repo-health $H $O/edges-head-HEAD.json
python3 $O/c2.py $R/scripts/dev/repo-health $H $O/edges-b0-HEAD.json $B0          # profile は B0 から読む
python3 $O/c2.py $R/scripts/dev/repo-health $H $O/edges-b0-HEAD-own.json          # exit 1（profile_invalid。H と同じ error 文）
python3 $A/tools/keys.py $O/edges-head-HEAD.json $RC/edges-head-HEAD.json.gz      # 17・tc (55,210,627)・ta (49,228,535)・14・29、差 0、cycles json identical True
python3 $A/tools/keys.py $O/edges-b0-HEAD.json $RC/edges-b0-HEAD.json.gz          # 同上
# C5（A §7 C5）
X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'
for B in $B0 $H; do
  git grep -nE 'crate::cos::' $B -- crates/task-api/src/handlers crates/task-api/src/handlers.rs | grep -vE "$X" | wc -l   # 22 / 14
  git grep -nE '(crate|super(::super)*)::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                    # 52 / 29
  git grep -nE 'crate::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                                    # 48 / 29
  git grep -nE 'super(::super)*::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                          # 4 / 0
  git grep -n '#\[path' $B -- crates | grep -vE "$X" | sed -E "s/^$B://; s/:[0-9]+:/: /" > $O/path-attr-$B.txt
done
diff $O/path-attr-$H.txt $RC/path-attr-$HM.txt                       # 差 0
python3 $A/tools/path_units.py $H                                    # {'task-core': 1, 'task-dispatch': 9, 'task-worker': 5} total 15
# C7
cargo metadata --format-version 1 --offline > $O/meta-HEAD.json      # exit 0
python3 $A/tools/cargo_edges.py $O/meta-HEAD.json                    # members 15 edges 49 {normal 35, dev 14} crate_cycles 0
# 地図の確認
for f in design_gate design_verdict plan_adoption policy_review worker_profile check_item review_limits inventory; do
  echo "$f $(git show $H:docs/architecture-map.md | grep '#\[path\]' | grep -c $f)"; done   # 8 件とも 1
# H が測った HEAD との差の commit（§2）
git log --no-merges --format='%H %s' $HM..$H | while read h s; do echo "$(git diff --name-only $h^ $h -- crates scripts web Cargo.toml Cargo.lock | wc -l) $s"; done   # 10 件とも 0
```

生の記録（成果物 dir の `raw/`）: `head.txt`、`c1-{head-B0,head-HEAD,b0-B0}.{json.gz,out,time}`、`c2.py`、`c2-{head-HEAD,b0-HEAD,b0-HEAD-own}.txt`、`edges-*.json.gz`、
`path-attr-*.txt`、`path-units-{HEAD,B0}.txt`、`meta-HEAD.json.gz`。
