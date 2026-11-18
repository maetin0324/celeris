---
title: アーキ v4 F adr — knowledge.rs・lifecycle.rs の transaction の境界の before の値
tasks: [01M7RFMBFY4M3H3Z78KGN51RYB]
status: running
updated: 2026-11-18
---
# アーキ v4 F — before の値（WorkUnit `before`）

ADR `agent-docs/adr/2026-11-18-knowledge-lifecycle-tx-boundary.md` と進捗の本体 `agent-docs/progress/2026-11-18-arch-v4-f.md` は
WorkUnit `adr-doc` が書く。この file は before の値と再現 command だけを持つ。`crates/`・`web/`・`agent-docs/adr/` は変えていない。

## 基準

- `B=$(git merge-base HEAD main)`。計測の時点で HEAD・main の先端・merge-base は同じ commit だった。
- その commit の件名は「Merge commit '…' into celeris/01M7MZTKB6GQ241F6JJYSPJNS1」（引用の中は D の統合の commit の hash なので省く）。
  first parent の件名は「integrate wu/close (phase close)」（アーキ v4 E の close の統合）。アーキ v4 D・E の統合の後の値である。
- 規則と command は A の `baseline.md`（`/local/celeris/data/workspaces/01M7MZTKAXM8GJGK8BPE13CQRM/artifacts/baseline.md`）の §6・§7 を
  そのまま `$B` に当てた。C2 の検査器（`scripts/dev/repo-health`）は作業ツリーの版で、`$B` と同じ内容。
- 生の出力（C2 の JSON・snippet・source-size-report の JSON）は run の artifacts の `raw/` に置いた（`cycle-edges.json`・`ssr-before.json` など）。

## 値

### C2 循環の辺（§7 C2 の snippet）

| 指標 | before |
|---|---|
| 循環の件数、単位・組・行の辺の合計 | 17 件、198・756・1,815 |
| task-api の SCC の単位・組・行の辺 | 49・261・620 |
| `handlers → cos`（src_unit が handlers、dst_unit が cos） | 19 本・12 file |
| `cos → handlers`（src_unit が cos、dst_unit が handlers） | 50 本・9 file |
| SCC の中の `knowledge` 単位の辺 | 出 10（cos 7・handlers 1・middleware 1・state 1）、入 18（schema 10・cos 5・chat 1・handlers 1・skills 1） |
| SCC の中の `lifecycle` 単位の辺 | 出 5（cos・handlers・middleware・state・milestones 各 1）、入 9（cos 7・handlers 1・schema 1） |

- `handlers → cos` の file ごと: `handlers.rs` 1、`accounts.rs` 2、`authority.rs` 1、`clusters.rs` 6、`idempotency.rs` 1、`project_controller.rs` 1、
  `projects.rs` 1、`reload.rs` 1、`roadmap.rs` 1、`system.rs` 2、`task_actions.rs` 1、`tasks.rs` 1（`handlers/` の下、`handlers.rs` を除く）。
- `cos → handlers` の file ごと: `cos/inbox.rs` 3、`cos/knowledge_use.rs` 2、`cos/mod.rs` 2、`cos/operations.rs` 3、`cos/ops/admin.rs` 13、
  `cos/ops/projects.rs` 17、`cos/ops/tasks.rs` 8、`cos/override_op.rs` 1、`cos/review.rs` 1。
- `crates/task-api/src/knowledge` と `crates/task-api/src/lifecycle` は task-api の SCC の中、`problem`・`types` は外。
- 参考: D の before（D の `adr.md`、D・E の統合の前）は task-api の SCC 49・261・632、`handlers → cos` 22 本・13 file、`cos → handlers` 52 本・9 file。

### C4 SQLite を直接参照する production file（§7 C4 の 2 本、crate ごと）

| crate | `rusqlite` を書く file | SQL 文字列の file |
|---|---|---|
| celeris | 4 | 3 |
| celeris-mcp | 1 | 1 |
| celerisctl | 1 | 0 |
| llm-proxy | 1 | 1 |
| task-api | 17 | 4 |
| task-core（`store/` を除く） | 42 | 38 |
| task-dispatch | 1 | 0 |
| task-ops | 8 | 3 |
| 合計 | 75 | 50 |

- task-api の `rusqlite` の 17 file: `chat/attachments.rs`、`cos/operations.rs`、`cos/ops/surface.rs`、`cos/review.rs`、`cos/triage_view.rs`、
  `handlers/authority.rs`、`handlers/idempotency.rs`、`handlers/project_controller.rs`、`handlers/roadmap.rs`、`handlers/tasks.rs`、`human_epoch.rs`、
  `knowledge.rs`、`lib.rs`、`org_write.rs`、`problem.rs`、`repos.rs`、`state.rs`。
- task-ops の `rusqlite` の 8 file: `comment.rs`、`decision.rs`、`gate.rs`、`lifecycle.rs`、`op_writes.rs`、`roadmap.rs`、`routing_export.rs`、`routing_stage.rs`。
- SQL 文字列の task-api の 4 file: `chat/attachments.rs`、`cos/review.rs`、`cos/triage_view.rs`、`handlers/project_controller.rs`。
  task-ops の 3 file: `roadmap.rs`、`routing_export.rs`、`routing_stage.rs`（`knowledge.rs`・両方の `lifecycle.rs` は SQL 文字列を持たない）。

### C5 HTTP と CoS の相互参照（§7 C5 の 2 本の行数）

| 指標 | before |
|---|---|
| `crate::cos::` を書く handlers の行 | 19 行（12 file） |
| `(crate\|super(::super)*)::handlers` を書く cos の行 | 50 行（`crate::` 46 ＋ `super::` 4、9 file） |

### C6 SQL の公開面（§7 C6）

| 指標 | before |
|---|---|
| `&Connection`・`&Transaction` の引数の行（§7 の command のまま） | 347（celeris 1、llm-proxy 4、task-api 15、task-core 320、task-ops 7） |
| 同じ（path だけに除外の式を当て直した値。下の「限界」の 2） | 336（task-core 314、task-ops 2、他は同じ） |
| `Fn(&Connection)`・`Fn(&Transaction)` の引数の行 | 22（task-api 4、task-core 13、task-ops 5） |
| `impl SqliteStore` の block（`git grep`） | 102（88 file）。path 付きを含めると 105 |
| trait の impl（`impl … for SqliteStore`） | 6 |
| pub method（python の snippet） | block 102、`pub` 631、`pub(crate..)` 67、private 41 |
| pub method（path 付きの block も数える版） | block 105、`pub` 634、`pub(crate..)` 67、private 41 |

- task-api の `&Connection`・`&Transaction` の 15 行: `cos/review.rs:68`、`handlers/authority.rs:501`、`handlers/idempotency.rs:90`、
  `handlers/project_controller.rs:59,104`、`handlers/roadmap.rs:165`、`handlers/tasks.rs:619,713`、`human_epoch.rs:40,58,70,91`、`knowledge.rs:774`、
  `org_write.rs:77`、`repos.rs:154`。
- task-ops の 7 行: `lifecycle.rs:411`（`apply_project_change_tx` の `tx: &rusqlite::Connection`）、`roadmap.rs:233`、
  `project_budget_tests.rs:23,41,334`・`stranded/tests.rs:801,826`（試験の file。限界の 2）。
- `Fn(&Connection)` の task-api の 4 行: `cos/operations.rs:267,316`、`cos/ops/surface.rs:270`、`repos.rs:148`。
  task-ops の 5 行: `comment.rs:251`、`decision.rs:596`、`gate.rs:516`、`lifecycle.rs:110`（`apply_project_action` の `also`）、`op_writes.rs:50`。
- 参考: A の before（A の基準の main）は pub method の snippet で 102・633・67・42、path 付きで 105・636・67・42。差は D の store-prune の分。
  `SqliteStore::attachment_provenance` を足すと `pub` は 1 増える（ADR の D2 が明記する）。

### 3 file の `rusqlite` の出現数と行番号

| file | 出現数 | 行（`git grep -n`） |
|---|---|---|
| `crates/task-api/src/knowledge.rs` | 8 | 656・661（2 回）・669・774・873・874・878 |
| `crates/task-ops/src/lifecycle.rs` | 2 | 110・411 |
| `crates/task-api/src/lifecycle.rs` | 0 | 無し |

- `knowledge.rs`: 656・661 は `open_attachment_index`（受信箱の provenance 用に読み取り専用の接続を自分で開く）、669 は `candidate_provenance` の
  `conn: Option<&rusqlite::Connection>`、774 は `pin_candidate_tx` の `tx: &rusqlite::Transaction<'_>`、873・874・878 は `record_op` の HTTP の pin
  （`rusqlite::Connection::open` と `TransactionBehavior::Immediate` の自前の transaction）。
- `task-ops/src/lifecycle.rs`: 110 は `apply_project_action` の `also: FnOnce(&rusqlite::Connection)`、411 は `apply_project_change_tx` の `tx`。

### `project_controller_human_bump_tx` の呼び手 file（`git grep -l … -- crates`）

| 区分 | file（行） |
|---|---|
| 定義 | `crates/task-core/src/store/project_wake.rs`（428、`pub fn`） |
| production の呼び手 | `crates/task-api/src/human_epoch.rs`（46、`HumanEpoch::bump_tx`）、`crates/task-api/src/handlers/authority.rs`（634、`post_dismiss`）、`crates/task-core/src/store/authority.rs`（988、`authority_unblock_tx`） |
| 試験の file | `crates/task-dispatch/src/dispatcher/tests/` の `long_horizon_bench.rs`（550・1263）、`long_horizon_pilot.rs`（267）、`project_achievement.rs`（278）、`project_recovery.rs`（202）、`project_wake.rs`（62・791・820） |

- `git grep -l` の file は 9（定義 1・production の呼び手 3・試験 5）。task-ops には呼び手が無い。
- 定義の file の `#[cfg(test)]` は 914 行目から始まり、428 行目の定義と呼び手の 988 行目（別 file の `store/authority.rs`）は試験の外にある。

### D の付記「2 つの形の使い分けと撤去の条件」の 2 本の command（`$B` の `crates/task-api/src`）

| command | file 数 |
|---|---|
| 狭い形 `git grep -l 'Option<&OperationAudit>'` | 14 |
| 広い形 `git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>"` | 18 |

- 狭い形の 14 file: `approvals.rs`、`changes.rs`、`cos/operations.rs`（型の定義側）、`decisions.rs`、`docs.rs`、`handlers/projects.rs`、
  `handlers/task_actions.rs`、`handlers/tasks.rs`、`integrate_task.rs`、`knowledge.rs`、`lifecycle.rs`、`model_catalog.rs`、`repos.rs`、`skills.rs`。
- 広い形はこの 14 に `cron_jobs.rs`、`execution.rs`、`handlers/clusters.rs`、`inbox_notifications.rs` の 4 を足した 18 file。
  `git grep -lE '\bApplied\b' $B -- crates/task-api/src` も同じ 18 file を返す（`diff` で差 0）。
- F が移す対象は `knowledge.rs`（`record_op`・`accept_op`・`reject_op`）と `lifecycle.rs`（`project_action_op`・`task_pause_op`）で、どちらも両方の command に入る。
  F が `project_action_op` と `record_op` だけを移すと、同じ file の他の関数が残るので file 数は変わらない見込み（約束ではない。after は F の measure が測る）。

### source-size-report（`$B` の script と toml、`--exclusive-classes --rev $B`）

| 指標 | before（行） |
|---|---|
| `views.product_only.lines`（production の全言語） | 375,818 |
| `views.product_only.rust_lines`（production の Rust） | 331,127 |
| `views.own_total_with_tests.lines` | 1,030,359 |
| `views.own_total_with_tests.rust_lines` | 883,284 |
| `class_totals.external_test` | 459,062 |
| `class_totals.inline_test` | 21,408 |

- F の目的は transaction の境界の移動で、行数の純減を受け入れ条件に置かない（変更の規約 11）。値は増減の確認のために測った。
- 参考: 3 file の行数は `knowledge.rs` 1,122、`task-ops/src/lifecycle.rs` 459、`task-api/src/lifecycle.rs` 268（`git show $B:<file> | wc -l`）。

## 再現 command（repo の root で、そのまま貼って実行する）

```sh
B=$(git merge-base HEAD main)
X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'; P='^crates/(campaign|routing-eval|jev-classifier)/'
mkdir -p "$TMPDIR"
# C2 循環の全部の辺（A の §7 C2 の snippet の写し。約 30 s）
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
# C2 の task-api の SCC と handlers⇄cos の辺（本数と file 数）、knowledge・lifecycle の単位の辺
python3 -I - $TMPDIR/cycle-edges.json <<'EOF'
import json,sys,collections
d=json.load(open(sys.argv[1]))
print('total',len(d['cycles']),sum(x['unit_count'] for x in d['cycles']),sum(x['unit_pair_edges'] for x in d['cycles']),sum(x['line_edges'] for x in d['cycles']))
c=[c for c in d['cycles'] if any(u.startswith('crates/task-api/') for u in c['units'])][0]
print('task-api SCC units',c['unit_count'],'pairs',c['unit_pair_edges'],'lines',c['line_edges'])
H='crates/task-api/src/handlers'; C='crates/task-api/src/cos'
for s,t in ((H,C),(C,H)):
    es=[e for e in c['edges'] if e['src_unit']==s and e['dst_unit']==t]; fc=collections.Counter(e['src'] for e in es)
    print(s.split('/')[-1],'->',t.split('/')[-1],len(es),'edges',len(fc),'files',dict(fc))
for u in ('crates/task-api/src/knowledge','crates/task-api/src/lifecycle'):
    o=[e for e in c['edges'] if e['src_unit']==u]; i=[e for e in c['edges'] if e['dst_unit']==u]
    print(u,'out',len(o),dict(collections.Counter(e['dst_unit'].split('/')[-1] for e in o)),'in',len(i),dict(collections.Counter(e['src_unit'].split('/')[-1] for e in i)))
EOF
# C4（§7 C4 の 2 本）
git grep -l 'rusqlite' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/' | cut -d/ -f2 | sort | uniq -c
git grep -lE '"[^"]*\b(SELECT .* FROM|INSERT (OR [A-Z]+ )?INTO|UPDATE [a-z_]+ SET|DELETE FROM)\b' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/' | cut -d/ -f2 | sort | uniq -c
#   file の一覧は末尾の `| cut -d/ -f2 | sort | uniq -c` を `| grep -E '^crates/task-(api|ops)/'` に替える
# C5（§7 C5 の 2 本。file 数は -n を -l に、super:: だけは 'super(::super)*::handlers'）
git grep -nE 'crate::cos::' $B -- crates/task-api/src/handlers crates/task-api/src/handlers.rs | grep -vE "$X" | wc -l
git grep -nE '(crate|super(::super)*)::handlers' $B -- crates/task-api/src/cos | grep -vE "$X" | wc -l
# C6（§7 C6）
git grep -nE '\b[a-z_]+: &(mut )?(rusqlite::)?(Connection|Transaction)\b' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | cut -d/ -f2 | sort | uniq -c
git grep -nE '\b[a-z_]+: &(mut )?(rusqlite::)?(Connection|Transaction)\b' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | cut -d: -f1 | grep -vE "$X" | cut -d/ -f2 | sort | uniq -c   # path だけに除外を当て直した値
git grep -nE 'Fn(Once|Mut)?\(&(mut )?(rusqlite::)?(Connection|Transaction)' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | cut -d/ -f2 | sort | uniq -c
git grep -nE '^\s*impl(<[^>]*>)? SqliteStore\b' $B -- crates | grep -vE "$X" | wc -l
git grep -lE '^\s*impl(<[^>]*>)? SqliteStore\b' $B -- crates | grep -vE "$X" | wc -l
git grep -nE '^\s*impl(<[^>]*>)? ((crate|task_core)::(store::)?)?SqliteStore\s*\{' $B -- crates | grep -vE "$X" | wc -l
git grep -nE '^\s*impl(<[^>]*>)? [A-Za-z_:<>]+ for ((crate|task_core)::(store::)?)?SqliteStore\b' $B -- crates | grep -vE "$X" | wc -l
#   pub method（impl SqliteStore の block の中だけ、括弧を数える。A の snippet の写し）
python3 - $B <<'EOF'
import subprocess,re,sys
B=sys.argv[1]; X=re.compile(r'(^|/)(tests?)(/|\.rs$)|_tests?\.rs$')
files=[l.split(':',1)[1] for l in subprocess.run(['git','grep','-lE',r'^\s*impl(<[^>]*>)? SqliteStore\b',B,'--','crates/task-core/src'],capture_output=True,text=True).stdout.split()]
blocks=pub=crate=priv=0
for f in [f for f in files if not X.search(f)]:
    lines=subprocess.run(['git','show',f'{B}:{f}'],capture_output=True,text=True).stdout.split('\n'); i=0
    while i<len(lines):
        if re.match(r'^impl(<[^>]*>)? SqliteStore\b',lines[i]):
            blocks+=1; depth=lines[i].count('{')-lines[i].count('}'); j=i+1
            while j<len(lines) and depth>0:
                L=lines[j]
                if depth==1:
                    if re.match(r'^    pub (async )?(unsafe )?fn ',L): pub+=1
                    elif re.match(r'^    pub\((crate|super|in [^)]*)\) (async )?fn ',L): crate+=1
                    elif re.match(r'^    (async )?fn ',L): priv+=1
                depth+=L.count('{')-L.count('}'); j+=1
            i=j
        else: i+=1
print('impl blocks',blocks,'pub',pub,'pub(crate..)',crate,'private',priv)
EOF
#   path 付きの block も数える版（A の §7 の注のとおり 2 か所の正規表現を替えたもの）
python3 - $B <<'EOF'
import subprocess,re,sys
B=sys.argv[1]; X=re.compile(r'(^|/)(tests?)(/|\.rs$)|_tests?\.rs$')
files=[l.split(':',1)[1] for l in subprocess.run(['git','grep','-lE',r'^[[:space:]]*impl(<[^>]*>)? ((crate|task_core)::(store::)?)?SqliteStore[[:space:]]*\{',B,'--','crates/task-core/src'],capture_output=True,text=True).stdout.split()]
blocks=pub=crate=priv=0
for f in [f for f in files if not X.search(f)]:
    lines=subprocess.run(['git','show',f'{B}:{f}'],capture_output=True,text=True).stdout.split('\n'); i=0
    while i<len(lines):
        if re.match(r'^impl(<[^>]*>)? ((crate|task_core)::(store::)?)?SqliteStore\s*\{',lines[i]):
            blocks+=1; depth=lines[i].count('{')-lines[i].count('}'); j=i+1
            while j<len(lines) and depth>0:
                L=lines[j]
                if depth==1:
                    if re.match(r'^    pub (async )?(unsafe )?fn ',L): pub+=1
                    elif re.match(r'^    pub\((crate|super|in [^)]*)\) (async )?fn ',L): crate+=1
                    elif re.match(r'^    (async )?fn ',L): priv+=1
                depth+=L.count('{')-L.count('}'); j+=1
            i=j
        else: i+=1
print('impl blocks',blocks,'pub',pub,'pub(crate..)',crate,'private',priv)
EOF
# 3 file の rusqlite の出現数と行番号
for f in crates/task-api/src/knowledge.rs crates/task-ops/src/lifecycle.rs crates/task-api/src/lifecycle.rs; do echo "$f $(git show $B:$f | grep -o rusqlite | wc -l)"; done
git grep -n rusqlite $B -- crates/task-api/src/knowledge.rs crates/task-ops/src/lifecycle.rs crates/task-api/src/lifecycle.rs
# project_controller_human_bump_tx の呼び手 file（行は -l を -n に）
git grep -l project_controller_human_bump_tx $B -- crates | sed "s/^$B://"
# D の付記の 2 本の command（$B の crates/task-api/src）と Applied の照合
git grep -l 'Option<&OperationAudit>' $B -- crates/task-api/src | sed "s/^$B://" | tee $TMPDIR/narrow.txt | wc -l
git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" $B -- crates/task-api/src | sed "s/^$B://" | tee $TMPDIR/wide.txt | wc -l
comm -13 $TMPDIR/narrow.txt $TMPDIR/wide.txt
git grep -lE '\bApplied\b' $B -- crates/task-api/src | sed "s/^$B://" | diff - $TMPDIR/wide.txt && echo same
# source-size-report（script と toml は $B から取り出す。約 13 s）
git show $B:scripts/dev/source-size-report.py > $TMPDIR/ssr.py; git show $B:scripts/dev/source-size-report.toml > $TMPDIR/ssr.toml
python3 -I $TMPDIR/ssr.py --root . --config $TMPDIR/ssr.toml --exclusive-classes --rev $B --format json --out $TMPDIR/ssr.json
python3 -I -c "import json,sys;r=json.load(open(sys.argv[1]));v=r['views'];c=r['class_totals'];print(v['product_only']['lines'],v['product_only']['rust_lines'],v['own_total_with_tests']['lines'],v['own_total_with_tests']['rust_lines'],c['external_test'],c['inline_test'])" $TMPDIR/ssr.json
```

after は `B` を F の HEAD（作業ツリーなら C2 は `snapshot.resolve('.', B, 'worktree')`、grep は `git grep --untracked` で作業ツリー）に替えて同じ command を当てる。
source-size-report の script と toml は after でも上の `$B`（before の基準）から取り出す。

## 数えないもの・近似の限界（A の §6 の規則と、この計測で確かめたこと）

1. 単位は物理 path（repo-health の lang_deps）。C2 の `knowledge`・`lifecycle` の単位は `crates/task-api/src/knowledge`・`crates/task-api/src/lifecycle`（§6 の 1）。
2. 除外の式 `$X` は行末の `$` を使うので、`-n` の出力（`path:行:本文`）には `*_tests.rs`・`tests.rs` の除外が効かない（`tests/` の dir は効く）。
   C6 の `&Connection` の行は `task-ops/src/project_budget_tests.rs` 3 行・`task-ops/src/stranded/tests.rs` 2 行と task-core の 6 行を含む。
   A の値と比べるため表の主値は §7 の command のまま（347）とし、path だけに除外を当て直した値（336）を並べた。C4（`-l`）と C5 の file には試験の file が無い。
3. repo-health は `super::` の path を辺にしない（§6 の 2）。ただし今の検査器は `super::` を解決するので、C2 の `cos → handlers` 50 本は C5 の `super::` の 4 行を含む。
   C5 は `crate::` と `super::…handlers` の両方の行数を並べた。
4. 循環の大きさは単位の数と組の数で比べ、行の辺の本数も並べる（§6 の 3）。既存の SCC の中に辺を足しても単位の数は変わらないので、組の数と本数で悪化を見る。
5. web の 3 件（TanStack Router の route ⇄ screen）と評価基盤（campaign 46 単位・routing-eval 2 単位）の循環は数から除かず、そのまま 17 件に入る（§6 の 4・5）。
6. SqliteStore の pub method は括弧を数える近似で、文字列・comment の中の括弧で誤ることがある。`pub` と `pub(crate)` を分けて出す（§6 の 6）。
   snippet が見るのは `crates/task-core/src` の `impl SqliteStore` の block だけで、他の crate の trait の impl（6 件）は method に数えない。
7. `rusqlite` を書く file・SQL 文字列・`&Connection` の引数は `git grep` の近似で、inline の `#[cfg(test)]` の中も数える（§6 の 8）。
   3 file の `rusqlite` の出現数は doc comment・型の path・関数の呼び出しを区別せずに数える（`knowledge.rs` の 661 行は 1 行に 2 回）。
8. SQL 文字列の式は 1 行に収まる文字列だけを見る。複数行の SQL の 2 行目以降の `FROM` などは拾わない（C4 の 2 本目）。
9. D の付記の狭い command は完全修飾の `Option<&crate::cos::operations::OperationAudit>` と lifetime 付きの別名を拾わない。広い command が拾う（D の付記のとおり）。
10. build・test-affected の時間（C8・C9）、C7b の波及、C10、C11 はこの WorkUnit では測っていない（F の依頼の範囲外）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- 上の再現 command を `$B` に当てた。C2 は約 30 s、source-size-report は約 13 s で exit 0。値は上の表。
- この file の sh の block を `awk` で取り出して `bash` でそのまま実行した → exit 0（約 41 s）。出力は上の表の値と全部一致した。

## 検査
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。
- `sh scripts/dev/check-doc-links.sh` → exit 0。`sh scripts/dev/check-adr-numbers.sh` → exit 0（`ok (460 files)`）。
- `git diff --name-only "$CELERIS_WU_BASE" -- crates web agent-docs/adr | wc -l` → 0。`sh "$CELERIS_WU_SCOPE_PATHS"` → この file だけ。
- `bash scripts/dev/test-affected.sh` → exit 0（`試験対象の変更なし（docs のみ等）`、repo-health は verdict=pass、new 0・worsened 0）。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告は vendor の `crates/script-runtime/vendor/deno_core/uv_compat/tcp.rs:818` の deprecated 1 件だけで、workspace の lint の対象外）。

## 未解決
- 無し。

## 提案
- 無し。
