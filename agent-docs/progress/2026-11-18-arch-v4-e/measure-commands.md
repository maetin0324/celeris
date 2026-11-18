---
title: アーキ v4 E — 計測コマンド（A の baseline.md §7 の写しと E の補助の数）
tasks: [01M7MZTKB6GQ241F6JJYSPJNS1]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# アーキ v4 E — 計測コマンド

A（task 01M7MZTKAXM8GJGK8BPE13CQRM）の `artifacts/baseline.md` §7 の C1・C2・C3・C7b・C9・C12 を、後の unit が repo の中で読めるように
そのまま写した（行を足さず、変えていない。`B=` の行と code block の囲みだけを足した）。repo の root で走らせる。`B` は before で
`bc665168c9caf1898adc4c611404bbee056daf9a`、after で測る commit の sha にする。規則（誤検出の扱い）は baseline.md §6
（要点は [A の進捗](../2026-11-17-arch-v4-a.md) の「B〜H が使う誤検出の扱い」）。設計は
[ADR 2026-11-18 dispatcher-store-access-pilot](../../adr/2026-11-18-dispatcher-store-access-pilot.md)。

## A の baseline.md §7 の写し

```sh
B=bc665168c9caf1898adc4c611404bbee056daf9a   # after では HEAD の sha
# C1 repo-health（循環・responsibility-growth・dup.contract・owner 不一致・時間）
/usr/bin/time -v python3 scripts/dev/repo-health/repo_health.py --repo . --base $B --head $B --policy-version 1 --out $TMPDIR/health.json
#   数: python3 -c "import json,collections;r=json.load(open('$TMPDIR/health.json'));print(collections.Counter(f['rule_id'] for f in r['findings']))"
#   循環の単位と組: 各 deps.cycle finding の rationale「N 単位の循環（辺 M 組）」
# C2 循環の全部の辺（evidence の 40 組の上限を超えて出す。既存の rules/deps.py の _components をそのまま呼ぶ）
python3 - $B $TMPDIR/cycle-edges.json <<'EOF'
import sys,json,collections,importlib.util
sys.dont_write_bytecode=True; sys.path.insert(0,'scripts/dev/repo-health')
from health import snapshot, profile as profile_mod, engine
from health.adapters import Classifier
B=sys.argv[1]; snap=snapshot.resolve('.',B,B); prof=profile_mod.resolve(snap,None)
spec=importlib.util.spec_from_file_location('repo_health_rules.deps','scripts/dev/repo-health/rules/deps.py')
deps=importlib.util.module_from_spec(spec); spec.loader.exec_module(deps)
ctx=engine.Context(snap,prof,prof.rule_config('deps.cycle'),[],{},[],[],Classifier(prof.classes),'full')
comps,edges=deps._components(ctx,snap.head_tree); out=[]
for comp in sorted(comps,key=lambda c:-len(c)):
    ns=set(comp); inner=[e for e in edges if e.src_unit in ns and e.dst_unit in ns]
    out.append({'units':sorted(ns),'unit_count':len(ns),'unit_pair_edges':len({(e.src_unit,e.dst_unit) for e in inner}),'line_edges':len(inner),
                'edges':[{'src':e.src,'line':e.line,'src_unit':e.src_unit,'dst_unit':e.dst_unit,'kind':e.kind,'excerpt':e.excerpt} for e in inner]})
json.dump({'base':B,'cycles':out},open(sys.argv[2],'w'),ensure_ascii=False,indent=1)
for c in out: print(c['unit_count'],c['unit_pair_edges'],c['line_edges'],c['units'][0])
EOF
#   (worktree を測るときは snapshot.resolve('.', B, 'worktree') にする)
# C3 dispatcher の参照（依頼の scope）
git ls-tree -r --name-only $B crates/task-dispatch/src/dispatcher/ | grep -vcE '(^|/)(tests?)(/|\.rs$)|_tests?\.rs$'          # 69
for p in 'self\.store' 'self\.config'; do
  git grep -o "$p" $B -- crates/task-dispatch/src/dispatcher/ | grep -vE ':[^:]*((^|/)(tests?)(/|\.rs:)|_tests?\.rs:)' > $TMPDIR/m
  echo "$p files=$(cut -d: -f2 $TMPDIR/m | sort -u | wc -l) occ=$(wc -l < $TMPDIR/m)"; done                                  # 44/633, 35/218
#   論理 scope（§6 の 1）= 69 file − lib.rs:47 の project_decider_ts.rs ＋ #[path] の 8 file
#   （after で場所が変わったら git grep -n '#\[path' $B -- crates/task-dispatch/src で選び直す）
X3='(^|/)(tests?)(/|\.rs$)|_tests?\.rs$'
{ git ls-tree -r --name-only $B crates/task-dispatch/src/dispatcher/ | grep -vE "$X3" | grep -v '^crates/task-dispatch/src/dispatcher/project_decider_ts\.rs$'
  for f in design_gate design_verdict plan_adoption policy_review worker_profile check_item review_limits inventory; do echo crates/task-dispatch/src/health_review/$f.rs; done; } > $TMPDIR/logical.txt
wc -l < $TMPDIR/logical.txt                                                                                                     # 76
for p in 'self\.store' 'self\.config'; do xargs -a $TMPDIR/logical.txt -I{} sh -c "git show $B:{} | grep -o '$p' | wc -l" |
  awk -v p="$p" '{o+=$1; if($1>0)f++} END{print p, "files="f, "occ="o}'; done                                               # 48/661, 39/224
#   #[path] の 8 file だけなら: self.store 5/29、self.config 4/6（上の for の 8 file を同じ awk に通す）
# C7b 変える単位を import する単位（§5.2 の「直接」「推移的」の行）。既存の lang_deps.build の全部の辺から cargo・cargo-dev を除いた
#   use・import の辺（crate をまたぐ use を含む。deps.cycle の _cycle_edges は crate をまたぐ use を除くので値が違う）の逆向きをたどる。
#   引数は「名前:単位,単位」（単位は repo-health の単位 = crates/<c>/src/<第 1 要素>）
python3 - $B S1:crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store S2:crates/task-dispatch/src/dispatcher,crates/task-dispatch/src/run_settlement <<'EOF'
import sys,collections
sys.dont_write_bytecode=True; sys.path.insert(0,'scripts/dev/repo-health')
from health import snapshot, profile as profile_mod, engine, lang_deps
from health.adapters import Classifier
B=sys.argv[1]; snap=snapshot.resolve('.',B,B); prof=profile_mod.resolve(snap,None)
ctx=engine.Context(snap,prof,prof.rule_config('deps.cycle'),[],{},[],[],Classifier(prof.classes),'full')
rev=collections.defaultdict(set)
for e in lang_deps.build(ctx,snap.head_tree).edges:
    if not e.kind.startswith('cargo') and e.src_unit!=e.dst_unit: rev[e.dst_unit].add(e.src_unit)
for arg in sys.argv[2:]:
    name,units=arg.split(':'); us=set(units.split(','))
    direct=set().union(*(rev[u] for u in us))-us
    seen=set(us); st=list(us)
    while st:
        for y in rev[st.pop()]:
            if y not in seen: seen.add(y); st.append(y)
    trans=seen-us; crates={u.split('/src/')[0] for u in trans}
    print(name,'direct',len(direct),'transitive',len(trans),'packages',len(crates),sorted(crates))
EOF
#   before: S1 direct 92 transitive 369 packages 12、S2 direct 6 transitive 14 packages 4（約 20 s）
# C9 test-affected の時間: scenario の変更を入れた状態で /usr/bin/time -v bash scripts/dev/test-affected.sh（終わったら git checkout -- <files>）
# C12 LoC（分類規則は base に固定）
git show $B:scripts/dev/source-size-report.py > $TMPDIR/ssr.py; git show $B:scripts/dev/source-size-report.toml > $TMPDIR/ssr.toml
python3 $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev $B --format json --out $TMPDIR/ssr.json
#   views.product_only.lines / rust_lines、class_totals.external_test + inline_test、views.own_total_with_tests
```

## E の補助の数（A と違う数え方。理由は ADR の「数え方と数えないもの」）

A の C3 は 1 行の中の `self.store` だけを数える。rustfmt が `self` と `.store` を別の行に割った参照（例
`crates/task-dispatch/src/dispatcher/cos_chat/relay_rejudge.rs` の 27-28 行）は数えない。E は C3 を主の数とし、次の数を並べる。

```sh
B=bc665168c9caf1898adc4c611404bbee056daf9a   # after では HEAD の sha
X3='(^|/)(tests?)(/|\.rs$)|_tests?\.rs$'
# E1 改行をまたぐ参照も数える（file 全体に self\s*\.\s*(store|config)\b を当てる）。$TMPDIR/logical.txt は C3 で作ったもの
git ls-tree -r --name-only $B crates/task-dispatch/src/dispatcher/ | grep -vE "$X3" > $TMPDIR/scope.txt
for L in scope logical; do xargs -a $TMPDIR/$L.txt -I{} sh -c "git show $B:{} | perl -0777 -ne '\$s=()=/self\s*\.\s*store\b/g; \$c=()=/self\s*\.\s*config\b/g; print \"\$s \$c\n\"'" |
  awk -v l=$L '{if($1>0)sf++; so+=$1; if($2>0)cf++; co+=$2} END{print l, "store files="sf, "occ="so, "config files="cf, "occ="co}'; done
#   before: scope store 46/759 config 35/227、logical store 50/791 config 39/233
# E2 SqliteStore を書く file の数（scope・論理 scope）と crate 全体の数（dispatcher/ の外へ移して減らしていないことを見る）
echo "scope SqliteStore files=$(git grep -l 'SqliteStore' $B -- crates/task-dispatch/src/dispatcher/ | sed "s/^$B://" | grep -vcE "$X3")"   # 27
xargs -a $TMPDIR/logical.txt -I{} sh -c "git show $B:{} | grep -c SqliteStore" | awk '$1>0{f++} END{print "logical SqliteStore files="f}'  # 29
for p in 'self\.store' 'self\.config' 'SqliteStore'; do
  git grep -o "$p" $B -- crates/task-dispatch/src/ | grep -vE ':[^:]*((^|/)(tests?)(/|\.rs:)|_tests?\.rs:)' > $TMPDIR/m
  echo "crate $p files=$(cut -d: -f2 $TMPDIR/m | sort -u | wc -l) occ=$(wc -l < $TMPDIR/m)"; done
echo "crate nontest files=$(git ls-tree -r --name-only $B crates/task-dispatch/src/ | grep -vcE "$X3")"
#   before: crate self.store 53/683、self.config 40/238、SqliteStore 50/195、非試験 138 file
# E3 対象 file ごとの共有の状態への経路（ADR の guard と同じ token。comment の行も数えるので、guard の 0 と違うことがある）
D=crates/task-dispatch/src/dispatcher
for f in behind_target auto_leaf write_set_gate write_set_record cos_chat/relay_rejudge cos_chat/slot_wait; do
  git show $B:$D/$f.rs | perl -0777 -ne '$n=0; for $t (qr/self\s*\.\s*store\b/, qr/self\s*\.\s*config\b/, qr/\bSqliteStore\b/, qr/\bDispatcher\b/,
    qr/\bCosChatLaunch(Config)?\b/, qr/\bDispatchConfig\b/, qr/use\s+super::\*/) { $c=()=/$t/g; $n+=$c; print "$c " } print "total=$n\n"' | sed "s|^|$f: |"; done
#   列の順: self.store self.config SqliteStore Dispatcher CosChatLaunch(Config) DispatchConfig `use super::*`
#   before: behind_target 2 1 0 2 0 0 0（計 5）、auto_leaf 1 1 0 1 0 0 1（4）、write_set_gate 2 0 0 1 0 0 1（4）、
#           write_set_record 2 0 0 2 0 0 1（5）、cos_chat/relay_rejudge 5 1 1 4 3 0 0（14）、cos_chat/slot_wait 8 1 0 2 2 0 0（13）
#   after の目標: 6 file とも comment を除いて 0（guard の試験 store_access_guard_* が確かめる）
```
