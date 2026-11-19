---
title: アーキ v4 H struct-deps — 循環・SCC の内部辺・handlers⇄cos・health の既存負債を A の規則で before と after に数え直す
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# struct-deps（WorkUnit）

before は main `bc665168c9caf1898adc4c611404bbee056daf9a`（以下 B0）。after は `git merge-base HEAD main`（件名 `integrate wu/close (phase close)`。
アーキ v4 G までが入った main。この WorkUnit の HEAD と同じ tree）。規則と snippet は A の baseline.md（A の run の成果物 `artifacts/baseline.md`）の
§4・§6・§7 をそのまま使い、新しい計測ツールは作っていない。数は [struct-deps.tsv](struct-deps.tsv) にある（16 key、検査器の版ごとの行）。
crates/・scripts/・web/ は変えていない。

検査器の版は 2 つそろえた（E の申し送り）。
- checker=b0: B0 の `scripts/dev/repo-health` を `git archive` で `$TMPDIR/rh-B0` に出したもの。
- checker=head: after の tree の `scripts/dev/repo-health`（アーキ v4 B の変更が入った版）。

## 0. 結論

- 循環と SCC は、単位の数と組の数が before と after で同じだった（両方の検査器で同じ）。減ったのは行の単位の辺だけである。
  - module の循環は 17 → 17。
  - task-core の SCC は 55 単位・210 組のまま（組の集合も同じ）。行の辺は 617 → 621（+4）。+4 はすべて `task-core/src/chat/attachments.rs → store` で、F の provenance-read の変更による。
  - task-api の SCC は 49 単位・261 組のまま（組の集合も同じ）。行の辺は b0 で 628 → 618、head で 632 → 622（どちらも −10）。
    内訳は D の org の pilot の −12（`handlers/org.rs → cos` −3、`cos/ops/admin.rs → handlers` −2、`cos/ops/admin.rs → skills` −2、`skills.rs → handlers` −4、`schema.rs → handlers` −1）と、
    F の knowledge-pin の +2（`cos/ops/decisions.rs → knowledge`）。
- handlers⇄cos の行は減ったが、単位の循環は解けていない（D の結論と同じ）。
  - handlers → cos は 22 → 19 行（13 → 12 file）。git grep と両方の検査器の辺で同じ値になった。
  - cos → handlers は `crate::` が 48 → 46 行（9 file のまま）、`super::` が 4 → 4 行（`cos/ops/admin.rs` の accounts）、A の C5 の式（両方の合計）は 52 → 50 行。
    head の検査器は `super::` を辺にするので 52 → 50、b0 の検査器は 48 → 46。
- health の既存負債は件数も finding の scope の集合も変わらなかった: `size.responsibility-growth` 94 → 94、`dup.contract` 210 → 210、owner 不一致（`arch-map.wrong_owner`）31 → 31。
- `#[path]` で物理と論理が食い違う file は task-dispatch で 9 → 9（A §3.2 の 8 と `lib.rs:47` の 1）。workspace 全体では 15 → 15（H の追加の数え方）。
  宣言の一覧は行番号を除いて before と after で同じ。
- Cargo の依存の辺は 49 → 49（normal 35・dev 14）、crate の循環は 0 → 0。Cargo.toml・Cargo.lock の差分は 0。
- 共通受入条件 (1)（import・依存・循環の内部の辺）に当てると、単位と組は減っておらず、行の辺は 2 つの SCC の合計で −6 本（task-api −10、task-core +4）だけである。
  「循環の内部の辺が減った」と言えるのは行の単位だけで、組の単位では**未達**。done と扱うかは draft・close の段と人の決定（unmet）に渡す。
- A の未解決（docs/architecture-map.md が `#[path]` の 8 file のうち 3 file を書いていない）は after でも残っている（§3）。

## 1. A・D・E と並べた before/after

| 指標（TSV の key） | A（baseline.md §4。b0 × B0） | D の値（metrics.md） | E の値（metrics.md） | H before（B0） | H after |
|---|---|---|---|---|---|
| Cargo の依存の辺（cargo_edges） | 49（normal 35・dev 14） | — | — | 49 | 49 |
| crate の循環（crate_cycles） | 0 | — | — | 0 | 0 |
| module の循環（module_cycles） | 17 | 17（D の after） | 17（B0・B1・E の HEAD） | b0 17 / head 17 | b0 17 / head 17 |
| task-core の SCC の単位（tc_scc_units） | 55 | — | — | b0 55 / head 55 | b0 55 / head 55 |
| task-core の SCC の組（tc_scc_pairs） | 210 | — | — | b0 210 / head 210 | b0 210 / head 210 |
| task-core の SCC の行（tc_scc_lines） | 617 | — | — | b0 617 / head 617 | b0 621 / head 621 |
| task-api の SCC の単位（ta_scc_units） | 49 | 49 | — | b0 49 / head 49 | b0 49 / head 49 |
| task-api の SCC の組（ta_scc_pairs） | 261 | 261 | — | b0 261 / head 261 | b0 261 / head 261 |
| task-api の SCC の行（ta_scc_lines） | 628 | head × B0 632 → D の after 620 | — | b0 628 / head 632 | b0 618 / head 622 |
| handlers → cos の行（handlers_to_cos_lines） | 22（13 file） | 22 → 19（12 file） | — | grep 22 / b0 22 / head 22 | grep 19 / b0 19 / head 19 |
| cos → handlers の行（cos_to_handlers_lines） | 辺 48（9 file）、C5 の式 52 | 辺 head 52 → 50、C5 の式 52 → 50 | — | grep crate:: 48 / C5 の式 52 / b0 48 / head 52 | grep crate:: 46 / C5 の式 50 / b0 46 / head 50 |
| cos → handlers の `super::` の行（cos_to_handlers_super_lines） | 4 | 4 → 4 | — | 4 | 4 |
| responsibility-growth（responsibility_growth） | 94 | — | 94（3 rev とも） | head 94（b0 も 94） | head 94 |
| dup.contract（dup_contract） | 210 | — | 210（3 rev とも） | head 210（b0 も 210） | head 210 |
| owner 不一致（owner_mismatch） | 31 | — | — | head 31 | head 31 |
| `#[path]` の食い違い（path_attr_files） | 8 ＋ 逆向き 1 | — | — | task-dispatch 9 / workspace 15 | task-dispatch 9 / workspace 15 |

参考（TSV の key ではない）:
- 循環の合計（全 17 SCC の単位・組・行）: b0 192・742・1,731 → 192・742・1,725、head 198・756・1,827 → 198・756・1,821。
  D の after（head）は 1,815 だった。H の after との差 +6 は F の 2 つの変更（task-core +4・task-api +2）による。
- dispatcher を含む SCC: b0 3 単位・4 組・22 本 → 同じ、head 4・7・51 → 同じ（E の値と同じ）。
- D の after の task-api の行（620）と H の after（622）の差 +2 は F の `cos/ops/decisions.rs → knowledge`。

## 2. A と違う数え方と理由

1. **b0 の検査器で after を測る方法**。B0 の検査器は after の profile を読めない（`profile_invalid: … review.high_risk[0] は class・paths だけ`。C が
   `review.high_risk` に `invariants` の欄を足したため。E の metrics.md と同じ現象を再現した）。そこで A §7 C2 の snippet（G の scope の `raw/c2.py`）の
   `snapshot.resolve('.',B,B)` を `snapshot.resolve('.',B0,B)` にし、profile を B0（base の tree）から読み、測る tree は after にした。
   B0 と after の profile の差は `review.high_risk` と `review.invariants` の 4 欄だけで、deps・size の規則の設定は同じ（tomllib で読み比べた）。
   A の snippet の「worktree を測るときは snapshot.resolve の第 3 引数を替える」と同じ種類の変更で、規則は変えていない。
   after の profile のまま b0 で走らせた値は無い（上の error で止まる）。
2. **C1 の b0 の行が無い key**。responsibility_growth・dup_contract・owner_mismatch は C1（`repo_health.py`）の findings で、C1 は profile を CLI では差し替えずに
   repo から読むので、b0 × after は上と同じ error で測れない。checker=head の行だけを置いた。b0 × B0 は responsibility-growth 94・dup.contract 210・deps.cycle 17 で
   A と同じだった。arch-map の 6 rule は `checker_missing`（`git archive` で `scripts/dev/check-architecture-map.py` を取り出さなかった）で 0 になったので、
   owner 不一致の b0 は A の 31 を引く。head × before と head × after は scope の集合まで同じなので、件数の比較は検査器の版によらない。
3. **head の検査器の値が A と違う理由**。head の検査器（B の変更）は `super::` の path と `#[path]` の論理 module を辺の単位に使う。そのため B0 でも
   task-api の SCC の行が 628 → 632（`super::` の 4 行）、cos → handlers の辺が 48 → 52、dispatcher の SCC が 3・4・22 → 4・7・51、循環の合計が 192・742・1,731 →
   198・756・1,827 になる。before と after の差はどちらの検査器でも同じ向き・同じ大きさだった。
4. **path_attr_files の数え方**。A §3.2 は task-dispatch だけを見て「物理は health_review・論理は dispatcher の 8 file」と「物理は dispatcher・論理は crate 直下の 1 file」を挙げた。
   H は両方を足して 9 とした（A の値は 8 ＋ 1）。さらに workspace 全体の production の `#[path]`（A §6 の X の除外と `*_tests.rs`・fixture を除く）を、
   repo-health の単位（`crates/<c>/src/<第 1 要素>`）が物理と論理で違うかで数えた: task-dispatch 9、task-worker 5（`browser.rs` の auth・conformance・launcher_run、
   `browser_cdp_sink.rs` の credential_ipc・post_login）、task-core 1（`lib.rs` の `health_repeat` が `model/` にある）で 15。単位が同じ宣言（`cron_tick.rs` の 4 件、
   `dispatch_run.rs`、`script_compare.rs`、`health_review/gate.rs`、`celeris/src/daemon/bootstrap.rs`、`task-core` の `browser`・`execution_gate`・`knowledge`）は数えない。
5. **cargo_edges の before**。A の値（49）を使った。Cargo.toml・Cargo.lock は B0 と after で差分 0 なので、after の作業ツリーで数えた cargo metadata の値が before と同じ tree の値になる。
   crate の循環は cargo metadata の normal の辺の強連結成分（2 crate 以上）で数えた（A は数え方の snippet を書いていない）。
6. **handlers ⇄ cos の 2 つの数**。A §6 の 2 に従い、repo-health の辺（C2 の task-api の SCC で src_unit・dst_unit が handlers と cos の組）と C5 の `git grep` の行を並べた。
   C5 の cos → handlers の式は `crate::` と `super::` を合わせて数えるので、`crate::` だけの行（cos_to_handlers_lines の grep の行）と `super::` だけの行
   （cos_to_handlers_super_lines）に分け、A の式の合計の行も残した。

## 3. A の未解決: architecture-map の `#[path]` の 3 file（残っている）

after の `docs/architecture-map.md:67` は、`#[path]` で読む file として design_gate・design_verdict・plan_adoption・policy_review・worker_profile の 5 件だけを書いている。
`health_review/check_item.rs`（`dispatcher::phase_integration` の子）・`health_review/review_limits.rs`（`dispatcher::review_verdict` の子）・`health_review/inventory.rs`
（`dispatcher::cron_tick` の子）の 3 件は、`#[path]` の行に無い（file 名は地図に 1 回ずつ出るが、`#[path]` の説明の中ではない）。B0 と同じ状態である。
B0..after の `docs/architecture-map.md` の差分（C・D・E・F の close）に、この 3 件の名を含む行は無い（`git diff` の `+`・`-` の行で 0 件）。この WorkUnit は直さない（提案 1）。

## 4. 提案（起票は人が決める）

1. `docs/architecture-map.md:67` の `#[path]` の説明に check_item・review_limits・inventory の 3 件を足す（文書だけの変更。source は変えない）。
2. handlers ⇄ cos の組（261）を減らす次の候補は、D の提案と同じく `cos/ops/admin.rs` の accounts の 9 行（`crate::` 5 行と `super::` 4 行。after の `git grep -n 'handlers::accounts'`。
   D の metrics.md §9 は「うち `super::` 3 行」と書いたが、after で数えると 4 行）と `cos/operations.rs` の HTTP の道具である。`super::` の 4 行は b0 の検査器では辺にならないので、比較には head の検査器の値を使う。
3. task-core の SCC は F の変更で行が +4 になった（`chat/attachments.rs → store`）。組は増えていないので deps.cycle は fail にならない。回帰条件として、
   head の検査器で task-core の SCC の組 210・task-api の SCC の組 261・module の循環 17 を超えないことを H の final-report に置くとよい。

## 5. 再現コマンド

repo の root で実行する。`ART` はこの run の成果物 dir、`G` は G の scope の成果物の `raw/`（`c2.py` の元）。

```sh
B0=bc665168c9caf1898adc4c611404bbee056daf9a; H=$(git merge-base HEAD main); O=$TMPDIR/out; mkdir -p $O
R=$TMPDIR/rh-B0; mkdir -p $R
git archive $B0 scripts/dev/repo-health scripts/dev/source-size-report.py scripts/dev/source-size-report.toml | tar -x -C $R
# C1（A §7 C1。約 1 分 36 秒ずつ。3 本を同時に走らせた）
python3 scripts/dev/repo-health/repo_health.py --repo . --base $B0 --head $B0 --policy-version 1 --out $O/c1-head-B0.json
python3 scripts/dev/repo-health/repo_health.py --repo . --base $H --head $H --policy-version 1 --out $O/c1-head-H.json
python3 $R/scripts/dev/repo-health/repo_health.py --repo . --base $B0 --head $B0 --policy-version 1 --out $O/c1-b0-B0.json   # arch-map.* は checker_missing
python3 -c "import json,collections,sys;r=json.load(open(sys.argv[1]));c=collections.Counter(f['rule_id'] for f in r['findings']);print({k:c[k] for k in ('deps.cycle','size.responsibility-growth','dup.contract','arch-map.wrong_owner')})" $O/c1-head-H.json
#   scope の集合の比較: findings を rule_id ごとに json.dumps(f['scope'],sort_keys=True) の Counter にして before と after の差を取る（差 0）
# C2（A §7 C2 の snippet = G の raw/c2.py。第 4 引数で profile を読む rev を指定できるようにした写し）
sed "s/^snap=snapshot.resolve('.',B,B)/PB=sys.argv[4] if len(sys.argv)>4 else B; snap=snapshot.resolve('.',PB,B)/" $G/c2.py > $TMPDIR/c2.py
python3 $TMPDIR/c2.py scripts/dev/repo-health $B0 $O/edges-head-B0.json
python3 $TMPDIR/c2.py scripts/dev/repo-health $H  $O/edges-head-H.json
python3 $TMPDIR/c2.py $R/scripts/dev/repo-health $B0 $O/edges-b0-B0.json
python3 $TMPDIR/c2.py $R/scripts/dev/repo-health $H  $O/edges-b0-H.json $B0      # profile は B0 から読む（§2 の 1）
#   出力の 'cycles' の数 = module_cycles、'crates/task-core/src/store' を含む SCC = tc_scc_*、'crates/task-api/src/handlers' を含む SCC = ta_scc_*、
#   'handlers->cos'・'cos->handlers' の行 = repo-health の辺。行の辺の差は (src, dst_unit) ごとの Counter の差で見た
# C5（A §7 C5）
X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'
for B in $B0 $H; do
  git grep -nE 'crate::cos::' $B -- crates/task-api/src/handlers crates/task-api/src/handlers.rs | grep -vE "$X" | wc -l        # 22 / 19
  git grep -nE '(crate|super(::super)*)::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                         # 52 / 50
  git grep -nE 'crate::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                                         # 48 / 46
  git grep -nE 'super(::super)*::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l                               # 4 / 4
  git grep -n '#\[path' $B -- crates | grep -vE "$X"     # path_attr_files の元の一覧（§2 の 4 の規則で分ける）
done
# C7（cargo metadata。workspace_members の dependencies のうち path のある workspace の crate を kind ごとに数え、normal の辺で強連結成分を取る）
git diff --stat $B0 $H -- Cargo.toml Cargo.lock 'crates/*/Cargo.toml'        # 差分 0
cargo metadata --format-version 1 --offline > $O/meta-H.json                   # members 15、edges 49（normal 35・dev 14）、crate_cycles 0
# 地図の確認（§3）
for f in design_gate design_verdict plan_adoption policy_review worker_profile check_item review_limits inventory; do
  echo "$f $(git show $H:docs/architecture-map.md | grep '#\[path\]' | grep -c $f)"; done                      # 後ろの 3 件が 0
# profile の差（§2 の 1）
python3 -c "import subprocess,tomllib; ..."   # B0 と after の celeris.toml を tomllib で読み、差のある key を出す（review.* だけ）
```

生の記録（この run の成果物 dir の `raw/`）: `c2.py`（上の写し）、`c2-{head,b0}-{B0,H}.txt`、`c2-b0-H-own.txt`（after の profile で止まった記録）、
`c1-{head-B0,head-H,b0-B0}.{out,time}`、`edges-*.json.gz`、`c1-*.json.gz`。測った後に `git status --porcelain -- crates scripts web` は空だった。
