---
title: アーキ v4 親 closeout store — H の (2)(3) SQLite 参照・Store の公開面・Dispatcher の参照・Applied の形を HEAD で測り直し照合する
tasks: [01M7MZT8KX547464YYS76QNP4G]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# store（親 01M7MZT8KX547464YYS76QNP4G の closeout の再計測、WorkUnit）

H の [replan/struct-store.tsv](../2026-11-19-arch-v4-h/replan/struct-store.tsv) の全 46 行（16 metric）を HEAD で測り直し、[store.tsv](store.tsv) に書いた。
HEAD は main（子 A〜H の入った状態）を取り込んだ親の branch で、件名は `integrate wu/sync (phase sync)`。
crates/・scripts/・web/・Cargo.* と policy の file は変えていない（変えた file はこの md と store.tsv だけ）。

## 0. 結論

- 46 行のうち、一致（yes）46 行、説明できる差（explained）0 行、説明できない差（no）0 行、時間の行（timing）0 行（struct-store.tsv に時間の行は無い）。
- H が測った HEAD は `integrate wu/r6-release (phase fix2)`（`7be366f75aeaa533814a13d081153f90422da704`、main の祖先）である。
  根拠: [replan/struct-store.md](../2026-11-19-arch-v4-h/replan/struct-store.md) の冒頭が「この WorkUnit の base は件名 `integrate wu/r6-release (phase fix2)`」と書き、
  H の rc-store の WorkUnit の成果物 `raw/HEAD/wide`（git grep の出力）の行頭の rev もこの commit である。
- その commit と HEAD の間で crates/・scripts/・web/・Cargo.toml・Cargo.lock・crates/*/Cargo.toml の差分は 0 file である（§2）。そのため値が変わる理由は無く、実測も全行一致した。
- 一致は件数だけではない。H の rc-store の WorkUnit が保存した file と行の一覧（`raw/HEAD/` の rusq・sql・conn・wide・afn・ca・fn と values.tsv・stderr.txt）と、
  今の HEAD で同じ台本が出した一覧は、git grep の行頭の rev を除いて byte 単位で同じだった（§3）。
- H の値の意味（変わっていない）:
  - (2) `rusqlite` を書く production file 74（B0 74 から減っていない。H の未達のまま）、SQL 文字列 49、接続を開く file（`conn_open_files`）45。
  - (3) `SqliteStore` の impl 102・`pub` method 631・`pub(crate)` 67・private 41、task-core の逆依存の crate 11、`&Connection` の引数は task-core 320 行・76 file。
  - Dispatcher は self.store が request/C3 で 38 file・630 回、self.config が 31 file・212 回（分母 69、論理 76）。
  - `Applied` の形は広い数え 15 file（回帰条件 18 以下を満たす）、狭い数え 11、関数 49。

## 1. 測った値（store.tsv の要約）

値は H の after → 今の HEAD。全行 match は yes。

| 指標（checker） | scope | H の after | 今の HEAD |
|---|---|---|---|
| `rusqlite_prod_files`（a-c4） | task-core/src/store/ の外の production | 74 | 74 |
| `sql_string_prod_files`（a-c4） | 同上 | 49 | 49 |
| `conn_open_files`（r7-d1） | 同上 | 45 | 45 |
| `conn_arg_*`（a-c6）行 / file | task-core・task-core/src/store・task-api・task-ops | 320/76・205/49・15/10・7/4 | 320/76・205/49・15/10・7/4 |
| `conn_fn_closure`（a-c6） | task-core・task-api・task-ops | 13・4・4 | 13・4・4 |
| `sqlitestore_impl`（a-c6） | plain・path・trait | 102・105・6 | 102・105・6 |
| `sqlitestore_pub`（a-c6） | plain・path | 631・634 | 631・634 |
| `sqlitestore_pub_crate`（a-c6） | plain・path | 67・67 | 67・67 |
| `sqlitestore_private`（a-c6） | plain | 41 | 41 |
| `sqlitestore_rdeps`（a-c7） | normal-transitive | 11 | 11（宣言 normal 35・dev 14） |
| `dispatcher_files`（a-c3・e-e1） | request・logical | 69・76 | 69・76 |
| `dispatcher_store_files`（C3 / E1） | request・logical | 38/40・42/44 | 38/40・42/44 |
| `dispatcher_store_hits`（C3 / E1） | request・logical | 630/756・658/788 | 630/756・658/788 |
| `dispatcher_config_files`（C3 / E1） | request・logical | 31/31・35/35 | 31/31・35/35 |
| `dispatcher_config_hits`（C3 / E1） | request・logical | 212/219・218/225 | 212/219・218/225 |
| `applied_narrow`（d-narrow）・`applied_wide`（d-wide）・`applied_functions`（h-awk） | crates/task-api/src | 11・15・49 | 11・15・49 |

参考（store.tsv の行ではない）:
- 同じ台本を H が測った HEAD（`integrate wu/r6-release (phase fix2)`）にも当て、45 行（逆依存の行を除く）が今の HEAD と同じ値になった。
- 台本の stderr の交差確認も H と同じ: `git grep -o` の形で self.store 38 file・630 回、self.config 31 file・212 回。impl block は plain 102・path 105。
- 非試験の `#[path]` は H と同じ 8 file（design_gate・design_verdict・plan_adoption・policy_review・worker_profile・check_item・review_limits・inventory）と lib.rs:47 の
  `project_decider_ts.rs` で、論理 scope は 76 file のまま。
- 逆依存の 11 crate: celeris・celeris-campaign・celeris-credentiald・celeris-mcp・celerisctl・llm-proxy・routing-eval・task-api・task-dispatch・task-ops・task-worker（workspace member 15）。

## 2. H が測った HEAD と今の HEAD の差

`git log 7be366f7..HEAD` は 18 commit（merge 8・merge でない 10）。変わった file は agent-docs/progress/ の下の 10 file だけである
（agent-docs/progress の直下 1・2026-11-19-arch-v4-h/ 1・2026-11-19-arch-v4-h/replan/ 8）。

crates/・scripts/・web/・Cargo.* を変えた commit: **無し**（`git diff --name-only 7be366f7 HEAD -- crates/ scripts/ web/ Cargo.toml Cargo.lock 'crates/*/Cargo.toml'` は 0 行）。

範囲の commit（件名。どれも文書だけ）:
- merge: `integrate wu/sync (phase sync)`・`arch-v4 親 sync: 子 A〜H の入った main を取り込む`・`integrate wu/publish (phase publish)`・`integrate wu/finish (phase final)`・
  `integrate wu/rewrite (phase rewrite)`・`integrate wu/rc-store (phase recount)`・`integrate wu/rc-loc (phase recount)`・`integrate wu/rc-gate (phase recount)`
- merge でない: `arch-v4 H R9 publish: …` 2 件、`arch-v4 H R8f finish: …` 2 件、`arch-v4 H R8e rewrite: …`、`arch-v4 H R8c rc-loc: …` 2 件、`arch-v4 H R8d rc-gate: …`、
  `arch-v4 H R8a rc-deps: …`、`arch-v4 H R8b struct-store: …`

## 3. 手順と検査器の版

- 数え方は H の [replan/struct-store.md](../2026-11-19-arch-v4-h/replan/struct-store.md) §5・§6 と [struct-store.md](../2026-11-19-arch-v4-h/struct-store.md) §6 のとおり:
  A の `baseline.md` §7 の C3・C4・C6・C7、E の measure-commands.md の E1、D の付記の 2 本の command、H の補助の awk、
  R7 の ADR（[2026-11-19-arch-v4-measure-rules-addendum](../../adr/2026-11-19-arch-v4-measure-rules-addendum.md)）の D1。repo-health の checker は使わない。
- 台本は H の rc-store の WorkUnit が上の snippet を 1 本にした `raw/measure.sh`
  （`/local/celeris/data/workspaces/01M7MZTKBPZPQE4XN451F8X84F/wu/rc-store/artifacts/raw/measure.sh`）を変えずに写して使った（写しと元は cmp で一致）。
  写しと出力はこの WorkUnit の run の成果物 dir の `raw/`（`raw/measure.sh`・`raw/HEAD/`・`raw/7be366f7/`）にある（repo には入れていない）。
- 逆依存の crate（C7）は HEAD の作業ツリー（clean）で `cargo metadata --format-version 1 --offline`（cargo 1.98.1）を取り、workspace member の path 依存のうち
  kind が normal の辺を逆にたどった。
- 所要時間は台本 1 回が約 5 秒、cargo metadata が数秒。時間は store.tsv の key ではないので照合しない。

## 再現コマンド（repo の root で）

```sh
R=$(git log --format=%H -1 --grep='^integrate wu/r6-release (phase fix2)$' HEAD)   # H が測った HEAD（7be366f7…）
S=/local/celeris/data/workspaces/01M7MZTKBPZPQE4XN451F8X84F/wu/rc-store/artifacts/raw/measure.sh   # H の台本
mkdir -p $TMPDIR/st/HEAD $TMPDIR/st/R
SAVE=$TMPDIR/st/HEAD bash $S HEAD > $TMPDIR/st/HEAD/out.tsv 2> $TMPDIR/st/HEAD/err.txt   # 45 行（metric scope value）
SAVE=$TMPDIR/st/R    bash $S $R   > $TMPDIR/st/R/out.tsv    2> $TMPDIR/st/R/err.txt
diff $TMPDIR/st/HEAD/out.tsv $TMPDIR/st/R/out.tsv                                    # 出力なし
H=/local/celeris/data/workspaces/01M7MZTKBPZPQE4XN451F8X84F/wu/rc-store/artifacts/raw/HEAD
diff $H/values.tsv $TMPDIR/st/HEAD/out.tsv; diff $H/stderr.txt $TMPDIR/st/HEAD/err.txt   # 出力なし
for f in rusq sql conn afn ca fn; do cmp $H/$f $TMPDIR/st/HEAD/$f; done                # 出力なし
diff <(sed 's/^[0-9a-f]*://' $H/wide) <(sed 's/^[0-9a-f]*://' $TMPDIR/st/HEAD/wide)    # 出力なし（行頭の rev だけが違う）
# C7 逆依存（task-core の normal の直接・間接の逆依存 11、宣言 normal 35・dev 14）
cargo metadata --format-version 1 --offline > $TMPDIR/meta.json
python3 - $TMPDIR/meta.json <<'EOF'
import json,sys
m=json.load(open(sys.argv[1])); ws=set(m['workspace_members'])
pk=[p for p in m['packages'] if p['id'] in ws]; names={p['name'] for p in pk}; rev={}; n={'normal':0,'dev':0,'build':0}
for p in pk:
    for d in p['dependencies']:
        if d['name'] in names and d.get('path'):
            k=d['kind'] or 'normal'; n[k]+=1
            if k=='normal': rev.setdefault(d['name'],set()).add(p['name'])
st=['task-core']; out=set()
while st:
    for r in rev.get(st.pop(),()):
        if r not in out: out.add(r); st.append(r)
print(n, len(out), sorted(out))
EOF
# H が測った HEAD と今の HEAD の source の差（0 行）
git diff --name-only $R HEAD -- crates/ scripts/ web/ Cargo.toml Cargo.lock 'crates/*/Cargo.toml' | wc -l
git log --format='%s' $R..HEAD
```

## 4. 未達・要判断（H の記録のまま。この再計測で変わったものは無い）

- (2) A の C4 の `rusqlite` を書く file は B0 74 → 74 で減っていない（H の replan/struct-store.md §7）。
- (3) `SqliteStore` の公開面と `&Connection` の引数は、H の measure の時点から変わっていない。
- Dispatcher は出現数が減ったが、参照を持つ file の数は E の後から変わっていない。

判定と人への質問は親の close の WorkUnit が行う（この WorkUnit は数え直しと照合だけ）。
