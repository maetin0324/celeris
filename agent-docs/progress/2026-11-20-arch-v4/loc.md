---
title: アーキ v4 親 closeout loc — H の (4)(5)(6) 変更波及・LoC・修正の波及・health と build の時間を HEAD で測り直して照合する
tasks: [01M7MZT8KX547464YYS76QNP4G]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# loc（親 closeout の WorkUnit、H の loc-cost の再計測）

H の値は [../2026-11-19-arch-v4-h/replan/loc-cost.tsv](../2026-11-19-arch-v4-h/replan/loc-cost.tsv) の after 列。
HEAD は main（子 A〜H の入った状態）を取り込んだ親の branch（件名 `integrate wu/sync (phase sync)`）。
H の 36 行を同じ順・同じ metric・scope・checker で [loc.tsv](loc.tsv) に置いた（列は metric・scope・checker・h_after・remeasured・match・note）。
手順は H の [replan/loc-cost.md](../2026-11-19-arch-v4-h/replan/loc-cost.md) §5 と [loc-cost.md](../2026-11-19-arch-v4-h/loc-cost.md) §5、
A の [進捗](../2026-11-17-arch-v4-a.md) と A の artifacts/baseline.md §7 の C7b・C8・C10 のまま使った。crates/・scripts/・web/・Cargo.* と policy の file は変えていない。

## 0. 結論

- 36 行のうち **一致（yes）23 行・説明できる差（explained）3 行・説明できない差（no）0 行・時間の行（timing）10 行**。
- (6) LoC: B0 の source-size-report の規則で HEAD を数え、H の値と 6 行すべて一致した（production 全言語 376,018・Rust 331,327、試験 484,793、
  own_total_with_tests 1,034,966・Rust 887,705、tooling 25,601）。B0 からは production **+919 行**で、純減していないことも H のとおり。
- (4) 変更波及: S1・S2 の file・単位・crate（4・4・2 と 4・2・1）、C7b の import する単位（S1 は b0 で 76 / 372・head で 77 / 368、S2 は 6 / 14）、
  `test-affected.sh --list` の対象（S1 14 crate ＋ web ＋ ts-verify、S2 5 crate）はすべて H と一致した。
- 修正の波及（C10）: A の窓は A の値を再現した（no-merges 194 件・46.9 %・69.1 %、with-merges 246 件・49.6 %・71.1 %）。
  B0 の後の窓の with-merges の 3 行は差がある（14 → 10 件、42.9 → 60.0 %、57.1 → 60.0 %）。HEAD の first-parent が親の branch になり、
  H の R1〜R5 の 5 件の integrate が `integrate wu/sync (phase sync)` の 1 件にまとまったためである。同じ snippet を `$B..main` に当てると H と同じ 14 件・42.9 %・57.1 % になる（§3）。
- (5) 時間: 8 行を 1 回ずつ測った。H の値との比は 0.903〜1.060。health は S1 95.84 s・S2 90.92 s で、A の値の 1.107 倍・1.110 倍（A の +10 % を超えたまま）。
  test-affected の 2 行は測っていない（H の規則「test-affected の時間は測り直さない」。1 回で約 12〜15 分かかる）。
- H が測った HEAD（件名 `arch-v4 H R8a rc-deps: 循環・SCC の内部辺・handlers⇄cos・health の既存負債を R1〜R7 の後の HEAD で数え直し、…` の commit）と
  今の HEAD の crates/・scripts/・web/・Cargo.* の差分は **0 file**。値の差はすべて git の履歴の形（C10）か時間の揺れ（timing）である。

## 1. H が測った HEAD と今の HEAD

- H の rc-loc は件名 `arch-v4 H R8c rc-loc: LoC・変更波及・health と warm check の時間を R1〜R7 の後の HEAD で数え直し、…` の commit で md と tsv だけを足した。
  測った tree はその親の件名 `arch-v4 H R8a rc-deps: …` の commit（main の祖先）である。
- その commit から HEAD までに入った commit（H の rc-loc の 2 件・publish などの文書の commit、親の `arch-v4 親 sync: 子 A〜H の入った main を取り込む`
  と `integrate wu/sync (phase sync)`）は、どれも crates/・scripts/・web/・Cargo.* を変えていない（`git diff --stat <H の rc-deps> HEAD -- crates scripts web Cargo.toml Cargo.lock` が空）。

## 2. G の値を写した行の tree と HEAD の差

H は check_cold・test_build_cold・test_build_warm_s1・test_build_warm_s2・test_affected_s1・test_affected_s2 の 6 行を G の中央値から写した。
G の tree は scenarios.env の `AFTER_CRATES_TREE`。main の first-parent でこの crates の tree を持つ commit は 12 件で、最も新しいものは件名 `integrate wu/draft (phase report)`、
source（crates/・scripts/・web/・Cargo.*）は G の close の件名 `integrate wu/close (phase close)` と同じ。その後 HEAD までに source を変えた main の commit は次の 6 件である。

| 件名 | crates/・scripts/ の差 |
|---|---|
| `integrate wu/r1-prompts (phase fix1)` | 5 file、+526 −6 |
| `integrate wu/r2-cos-edges (phase fix1)` | 70 file、+1,778 −858 |
| `integrate wu/r3-dispatcher (phase fix1)` | 4 file、+55 −62 |
| `integrate wu/r4-conn (phase fix1)` | 19 file、+1,069 −259 |
| `integrate wu/r5-applied (phase fix2)` | 8 file、+1,394 −363 |
| `integrate wu/r6-release (phase fix2)` | 15 file、+436 −115（scripts だけ） |

合計は crates/ 95 file +4,822 −1,548、scripts/ 15 file +436 −115、web/・Cargo.* は 0。上の 6 行の今回の値は HEAD の tree の値で、H の値（G の tree の値）と tree が違う。

## 3. 修正の波及（A §7 C10 の snippet）

| 窓 | 数え方 | commit | production を変えた | 複数 region | 複数 module |
|---|---|---|---|---|---|
| A の窓（`$B`、SINCE 2026-10-28T00:00:00Z） | no-merges | 492 | 194 | 91（46.9 %） | 134（69.1 %） |
| A の窓 | with-merges | 653 | 246 | 122（49.6 %） | 175（71.1 %） |
| B0 の後（`$B..HEAD`、SINCE 2026-11-17T00:00:00Z） | no-merges | 11 | 1 | 0（0.0 %） | 0（0.0 %） |
| B0 の後（`$B..HEAD`） | with-merges | 41 | 10 | 6（60.0 %） | 6（60.0 %） |
| 参考: H が測った tree（`$B..<H の rc-deps>`） | no-merges / with-merges | 12 / 56 | 1 / 14 | 0 / 6（42.9 %） | 0 / 8（57.1 %） |
| 参考: `$B..main` | no-merges / with-merges | 11 / 61 | 1 / 14 | 0 / 6（42.9 %） | 0 / 8（57.1 %） |

- A の窓の 2 行は A の §2 #13・§7 C10 の値と 4 列とも一致した。H の measure の値とも一致する。
- H が測った tree に当て直すと H の値（with-merges 14 件・42.9 %・57.1 %）を再現した。
- HEAD の窓の with-merges の 10 件は、H の measure の 9 件（`PathClass に任意の欄 invariants を足し、往復試験と schema を更新する` と、
  C〜F の integrate と sync の merge の 8 件）と `integrate wu/sync (phase sync)` の 1 件（production の 79 file、task-api・task-core・task-dispatch・task-worker）。
  H の 14 件は 9 件と R1〜R5 の 5 件の integrate を別に数えていた。親の branch では main の取り込みが 1 件の merge なので、5 件が 1 件になった。
- no-merges の commit 数の 12 → 11 は、H の branch の first-parent にあった rc-deps の commit（文書だけ）が HEAD の first-parent に無いためで、production を変えた 1 件は同じ。
- 件数が少なく、WU や branch の単位の merge を 1 件と数えるので、B0 の後の窓は A の窓と割合を比べられない（H と同じ結論）。

## 4. 時間の行（1 回だけ測った。3 回の中央値は測っていない）

G の rules.md の driver（`c1-health-tsv.sh`・`build-check.sh`・`probe-const.sh`）を H の rc-loc の raw から写し、回数を 1 回、waitload の上限を 120 s にした版
（`health-once.sh`・`build-once.sh`。`build-once.sh` は A §7 C8 の test build の行のとおり `cargo nextest run --workspace --no-run --offline` を足した）で測った。
同じ親 task の他の WorkUnit（deps・gate・store）が並行して走っていた。測った時刻は 2026-11-20T15:57Z〜16:18Z。

| key | 今回（s） | H の値（s） | 比 | 開始時の 1 分 load | 備考 |
|---|---|---|---|---|---|
| health_s1 | 95.84 | 98.74 | 0.971 | 3.72 | user 75.43 s、findings 698・new 3・worsened 1（H と同じ）、A 86.56 の 1.107 倍 |
| health_s2 | 90.92 | 100.68 | 0.903 | 3.96 | user 76.14 s、findings 699・new 4・worsened 2（H と同じ）、A 81.92 の 1.110 倍 |
| check_cold | 55.75 | 53.90 | 1.034 | 2.75 | H の値は G の写し（§2） |
| check_warm_s1 | 42.90 | 43.08 | 0.996 | 3.86 | prep 0.4 s |
| check_warm_s2 | 15.65 | 14.76 | 1.060 | 2.93 | prep 43.8 s |
| test_build_cold | 154.41 | 147.03 | 1.050 | 2.56 | H の値は G の写し |
| test_build_warm_s1 | 140.33 | 138.10 | 1.016 | 3.71（5 分 8.11） | H の値は G の写し、prep 3.0 s |
| test_build_warm_s2 | 82.30 | 81.60 | 1.009 | 3.81（5 分 11.83） | H の値は G の写し、prep 130.2 s |
| test_affected_s1・s2 | 測っていない | 906.03・707.76 | - | - | H の規則で測り直さない。H の値は G の写し |

- 8 回とも開始時の 1 分 load は 4 以下（valid=yes）、exit は health が 1（const の probe で fail が手順どおり）、cargo は 0。
- 1 回だけなので、H の値との差（−9.7 %〜+6.0 %）がコードによるのか host の負荷によるのかは区別できない。health は A の +10 % を超えたままで、
  G の回帰条件（A の +10 % を超えたら before を同じ host で 3 回測り直す）に当たる状態は H の replan と変わらない。

## 再現コマンド

repo の root で実行する。`ART` はこの WorkUnit の成果物 dir の `raw/`（`/local/celeris/data/workspaces/01M7MZT8KX547464YYS76QNP4G/wu/loc/artifacts/raw`。
driver の写し、`static.sh`、`build_loc_tsv.py`、生の出力がある）。driver の元は H の rc-loc の raw（`/local/celeris/data/workspaces/01M7MZTKBPZPQE4XN451F8X84F/wu/rc-loc/artifacts/raw`）。

```sh
B=bc665168c9caf1898adc4c611404bbee056daf9a
HM=$(git log --format=%H -1 --grep='^arch-v4 H R8a rc-deps: 循環' main)   # H が測った tree
git diff --stat $HM HEAD -- crates scripts web Cargo.toml Cargo.lock         # 空であること
# (6) LoC・(4) C7b と test-affected --list（comment の probe を戻して crates の差分 0 を確かめる）・C10 の 2 つの窓をまとめて測る（約 3 分）
ART=$ART bash $ART/static.sh
#   static.sh の中身（H の replan/loc-cost.md §5 と同じ）:
#   git show $B:scripts/dev/source-size-report.py > $TMPDIR/ssr.py; git show $B:scripts/dev/source-size-report.toml > $TMPDIR/ssr.toml
#   for r in $B $HM HEAD; do python3 -I $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev $r --format json --out $ART/ssr-<short>.json; done
#   python3 $ART/c7b.py HEAD $ARGS; python3 $ART/c7b-b0.py HEAD@$B $ARGS   # ARGS の S1h・S1・S2。tsv の S1 は S1h の行
#   $ART/probe-comment.sh $S1_FILES; bash scripts/dev/test-affected.sh --list; git checkout -- $S1_FILES（S2 も同じ）
#   python3 $ART/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-10-28T00:00:00Z <no-merges|with-merges> $B        # A の窓
#   python3 $ART/c10.py $TMPDIR/ssr.py $TMPDIR/ssr.toml 2026-11-17T00:00:00Z <no-merges|with-merges> "$B..HEAD" # B0 の後の窓（参考に $B..$HM・$B..main）
git status --porcelain -- crates                                             # 空であること
# (5) 時間の行を 1 回ずつ
. agent-docs/progress/2026-11-19-arch-v4-g/scenarios.env
TSV=$ART/health.tsv OUT=$ART WAITCAP=120 DEADLINE=$(( $(date +%s) + 1200 )) AFTER_REV=$(git rev-parse HEAD) S1_FILES="$S1_FILES" S2_FILES="$S2_FILES" bash $ART/health-once.sh
for a in "cold" "warm S1" "warm S2" "tcold" "twarm S1" "twarm S2"; do TSV=$ART/build-check.tsv OUT=$ART WAITCAP=120 DEADLINE=$(( $(date +%s) + 1200 )) bash $ART/build-once.sh $a; done
git status --porcelain -- crates                                             # 空であること
# G の tree と HEAD の差（§2）: main の first-parent で crates の tree が AFTER_CRATES_TREE と同じ最も新しい commit から HEAD まで
for c in $(git rev-list --first-parent main --since=2026-11-18T00:00Z); do [ "$(git rev-parse $c:crates)" = "$AFTER_CRATES_TREE" ] && echo $c; done | head -1
# tsv を作る
python3 -I $ART/build_loc_tsv.py
```

## 6. 範囲と戻し

- comment の probe（S1・S2 の 4 file、各 1 行）と const の probe（各回）の後は `git checkout --` で戻し、`git status --porcelain -- crates` は毎回空だった。
- この WorkUnit が変えた file は `agent-docs/progress/2026-11-20-arch-v4/loc.tsv` と `loc.md` だけ。crates/・scripts/・web/・Cargo.*・policy の file の差分は 0。

## 7. 変更（commit の件名）

- `arch-v4 親 closeout loc: H の (4)(5)(6) を HEAD で測り直し、loc.tsv と loc.md を書く`
- `arch-v4 親 closeout loc: 提出時の HEAD で回した検査の結果を loc.md に書く`
- `arch-v4 親 closeout loc: loc.md の再現コマンドの見出しを「## 再現コマンド」にする`

## 8. 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` と `git diff --name-only $CELERIS_WU_BASE HEAD` は loc.md・loc.tsv の 2 file だけ。`git status --porcelain -- crates` は空。
- `sh scripts/dev/progress-index.sh --check` → ok（exit 0）。`bash scripts/dev/check-doc-links.sh` → ok（exit 0）。
- `bash scripts/dev/test-affected.sh` → exit 0（docs のみ。repo-health verdict=pass new=0 worsened=0、試験対象の変更なし）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 全体の試験は source を変えていないので走らせていない。リリースゲートの確認は gate の WorkUnit が行う。本番への昇格はしていない。
