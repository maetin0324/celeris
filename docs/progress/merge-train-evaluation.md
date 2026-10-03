# merge train / project-level integration branch の評価

tasks: [01M3Z6NZ0RBF4ZH2H2KC69QT9S]

測った日: 2026-10-03（UTC 04:5x 頃）。本番 DB と release 一覧は読み取りだけで見た。DB への書き込み・daemon の再起動・設定の変更はしていない。

## 境界

Phase 1〜5 の導入時点。日時は git の commit 日時（UTC）。

| Phase | ADR（日付・追加 commit） | 完了の記録 | 統合 commit | 本番 release に入っているか |
|---|---|---|---|---|
| 1 review 前の target 同期・merge candidate 固定 | ADR-0118（2026-10-02、`034d7e91` 2026-10-02 00:37:36） | docs/PROGRESS.md「Phase 1 …（ADR-0118、2026-10-02）」 | `a00c28b2` 2026-10-02 06:34:42 integrate wu/p1-sync (phase p1) | 入っていない |
| 2 review 前同期の IntegrationRepair | ADR-0120（2026-10-02、`6719fc8e` 2026-10-02 06:49:46） | docs/PROGRESS.md「Phase 2 …（ADR-0120、2026-10-02）」 | `6a385df3` 2026-10-02 10:20:51 integrate wu/p2-repair (phase p2) | 入っていない |
| 3 Claude session resume | ADR-0140 claude-session-resume（旧 ADR-0124）（2026-10-02、`ce2c836f` 2026-10-02 10:37:46） | docs/PROGRESS.md「Phase 3 …（ADR-0140、2026-10-02）」 | `86841071` 2026-10-02 13:33:34 integrate wu/p3-session (phase p34) | 入っていない |
| 4 atomic direct route | ADR-0124 atomic-direct-route（2026-10-02、`b8c1c897` 2026-10-02 10:38:59） | docs/PROGRESS.md「… Phase 4 統合検証（2026-10-02）」、docs/progress/phase-direct-route.md | `db7abe3e` 2026-10-02 13:41:08 p4-fast（直行経路）を p3-session と統合 | 入っていない |
| 5 write-set 並列制御・behind 指標 | ADR-0130（2026-10-02、`9868fec1` 2026-10-02 14:01:29） | docs/PROGRESS.md「…（完了 2026-10-02、ADR-0130）」、docs/progress/phase-writeset.md | `44bbad17` 2026-10-02 20:26:11 integrate wu/p5-writeset (phase p5) | 入っていない |

確かめ方:

```sh
git log --format='%h %ci %s' --grep='^integrate'
git log --diff-filter=A --format='%h %ci' -- docs/adr/0118-* docs/adr/0120-* docs/adr/0124-* docs/adr/0130-*
ls ~/.local/celeris/releases/        # 0b8a225629fd 1b3c4ee6ac93 41366893a593 a6fb0793075b ae780a918695 ea2d9d325281
readlink ~/.local/celeris/current    # releases/ea2d9d325281
for c in a00c28b2 6a385df3 86841071 db7abe3e 44bbad17; do
  for r in 0b8a225629fd 1b3c4ee6ac93 41366893a593 a6fb0793075b ae780a918695 ea2d9d325281; do
    git merge-base --is-ancestor $c $r && echo "$c in $r" || echo "$c NOT in $r"; done; done
# → 5 commit とも 6 release のどれの祖先でもない（main の祖先でもない）。
git merge-base --is-ancestor 44bbad17 celeris/01M3Z6NZ0RBF4ZH2H2KC69QT9S && echo yes   # → yes（task branch にだけある）
```

本番 DB の schema も同じことを示す。本番 DB の migration 0037 は `0037_events_delivery_skipped_index.sql`（release `ea2d9d325281` の tree）で、Phase 1 の `0037_review_target_sync.sql` ではない。Phase 5 の `0039_write_sets`・`0040_behind_targets` は無い。

```sh
git ls-tree --name-only ea2d9d325281 crates/task-core/migrations/ | tail -3   # …0036_browser_trusted_login, 0037_events_delivery_skipped_index, 0041_feed_notices
sqlite3 "file:/var/lib/celeris/celeris.sqlite3?mode=ro" "select version, applied_at from schema_migrations where version>=36"   # 36, 37, 41
sqlite3 "file:/var/lib/celeris/celeris.sqlite3?mode=ro" "select count(*) from pragma_table_info('deliveries') where name in ('target_sha','reviewed_sha','merge_candidate_sha')"   # 0
sqlite3 "file:/var/lib/celeris/celeris.sqlite3?mode=ro" "select count(*) from sqlite_master where name in ('behind_targets','write_sets')"   # 0
```

DB の場所: `crates/celeris/src/config/db.rs` の既定は `~/.local/celeris/celeris.sqlite3` だが、そこには `celeris.sqlite3.moved-20260924-080845` しか無い。docs/ops/home-nfs-migration-2026-09-25.md に書かれているとおり、本番 DB は `/var/lib/celeris/celeris.sqlite3`（2026-10-03 04:56 更新、WAL あり）。

## 実測

DB は `sqlite3 "file:/var/lib/celeris/celeris.sqlite3?mode=ro"`（以下 `$RO`）で開いた。指標の取り方は実装の読み込みで決めた: `task-core/src/model.rs` の `Event::ReviewTargetSynced` / `ReviewTargetAdvanced` / `IntegrationRepairScheduled` / `Resolved` / `Exhausted`、`task-core/src/execution.rs` の `is_integration_repair_unit`（`kind = repair` かつ title が `repair (integration_repair)` で始まる WU）、`task-core/src/behind_target.rs` と migration `0040_behind_targets.sql`。session・直行経路・run 指標は `task-core/src/model.rs` の `Usage.session_resumed`・`RunMetrics.wall_ms`・`Event::ExecutionRouted`、`task-core/src/execution_metrics.rs`（`session_resumed` が無い旧 run は unknown）、`task-core/src/node_session.rs`（migration `0038_work_unit_sessions.sql`）。

| 指標 | 導入前 | 導入後 | command / SQL |
|---|---|---|---|
| pre-review sync の回数・衝突率 | 取れない: 導入前は機構が無く、記録する event も無い | 取れない: 本番未反映（Phase 1 の `a00c28b2` はどの release にも入っていない）。本番 DB の該当 event は 0 件、`deliveries` に `target_sha` 等の列が無い | `sqlite3 "$RO" "select count(*) from events where json_extract(json,'$.type') in ('review_target_synced','review_target_advanced')"` → 0。`sqlite3 "$RO" "select count(*) from pragma_table_info('deliveries') where name in ('target_sha','reviewed_sha','merge_candidate_sha')"` → 0 |
| integration repair の回数 | 取れない: 導入前は機構が無い。参考: 統合の結果は `task_integrations` に merge/done 62 件、conflict 0 件 | 取れない: 本番未反映（Phase 2 の `6a385df3` はどの release にも入っていない）。該当 event 0 件、integration_repair WU 0 件 | `sqlite3 "$RO" "select count(*) from events where json_extract(json,'$.type') in ('integration_repair_scheduled','integration_repair_resolved','integration_repair_exhausted')"` → 0。`sqlite3 "$RO" "select count(*) from work_units where kind='repair' and json_extract(json,'$.title') like 'repair (integration_repair)%'"` → 0。参考: `sqlite3 "$RO" "select method, state, count(*) from task_integrations group by 1,2"` → `merge\|done\|62` |
| Claude session reuse 率 | 0 / 1435（claude-code の run で `runs.session_id` が入っているのは 0 件。`usage_json` に `session_resumed` 欄（`task-core/src/model.rs` の `Usage.session_resumed`）を持つ run も 0 件。session resume の機構が無い） | 取れない: 本番未反映（Phase 3 の `86841071` はどの release にも入っていない）。本番 DB に `work_unit_sessions` table（migration 0038）が無い | `sqlite3 "$RO" "select count(*), count(session_id) from runs where adapter='claude-code'"` → `1435\|0`。`sqlite3 "$RO" "select key, count(*) from runs, json_each(runs.usage_json) group by key"` → `cache_creation_tokens`・`cache_read_tokens`・`cost_usd`・`input_tokens`・`output_tokens` だけ。`sqlite3 "$RO" "select count(*) from sqlite_master where name='work_unit_sessions'"` → 0 |
| atomic direct route（fast path）率 | 0（直行経路は無い）。参考: Complexity Gate の判定 `execution_gated` 160 件のうち atomic 50 件（policy 31・human 14・hint 5）、compound 110 件 | 取れない: 本番未反映（Phase 4 の `db7abe3e` はどの release にも入っていない）。`execution_routed` event 0 件 | `sqlite3 "$RO" "select count(*) from events where json_extract(json,'$.type')='execution_routed'"` → 0。`sqlite3 "$RO" "select json_extract(json,'$.decision.mode'), json_extract(json,'$.decision.source'), count(*) from events where json_extract(json,'$.type')='execution_gated' group by 1,2"` → `atomic\|hint\|5`, `atomic\|human\|14`, `atomic\|policy\|31`, `compound\|hint\|5`, `compound\|human\|69`, `compound\|policy\|36` |
| task あたり run 数・wall time・入力 token | done の execute task 358 件（runs の期間 2026-09-25〜2026-10-03 全体。Phase 3/4 は本番未反映なので全件が導入前）: run 数 平均 4.39・中央値 1。wall time（`metrics_json.wall_ms` の和）平均 1941 秒・中央値 175 秒。入力 token: `input_tokens` の和 平均 4,269,905、`input_tokens`+`cache_read_tokens`+`cache_creation_tokens` の和 平均 8,735,901・中央値 163,716 | 取れない: 本番未反映（Phase 3 `86841071`・Phase 4 `db7abe3e` はどの release にも入っていない）。統合 commit 日時 2026-10-02 13:33:34 以後に始まった done execute task の run は 319 件あるが、どれも導入前の release で動いている | `sqlite3 "$RO" "with t as (select r.task_id, count(*) n, sum(json_extract(r.metrics_json,'$.wall_ms'))/1000.0 wall_s, sum(json_extract(r.usage_json,'$.input_tokens')) inp, sum(coalesce(json_extract(r.usage_json,'$.input_tokens'),0)+coalesce(json_extract(r.usage_json,'$.cache_read_tokens'),0)+coalesce(json_extract(r.usage_json,'$.cache_creation_tokens'),0)) inp_all from runs r join tasks k on k.id=r.task_id where k.status='done' and k.kind='execute' group by r.task_id), o as (select *, row_number() over (order by n) rn_n, row_number() over (order by wall_s) rn_w, row_number() over (order by inp_all) rn_i, count(*) over () c from t) select c, round(avg(n),2), max(case when rn_n=(c+1)/2 then n end), round(avg(wall_s)), max(case when rn_w=(c+1)/2 then round(wall_s) end), round(avg(inp)), round(avg(inp_all)), max(case when rn_i=(c+1)/2 then inp_all end) from o"` → `358\|4.39\|1\|1941.0\|175.0\|4269905.0\|8735901.0\|163716`。`sqlite3 "$RO" "select count(*) from runs r join tasks k on k.id=r.task_id where k.status='done' and k.kind='execute' and r.started_at >= '2026-10-02T13:33:34'"` → 319 |
| behind commits・age の分布 | 取れない: 導入前は観測しておらず、記録する table も無い | 取れない: 本番未反映（Phase 5 の `44bbad17` はどの release にも入っていない）。本番 DB に `behind_targets` table が無い | `sqlite3 "$RO" "select count(*) from sqlite_master where name in ('behind_targets','write_sets')"` → 0 |

事前に調べた event の種類（本番 DB 全体、上位の抜粋）にも `review_target_*`・`integration_repair_*` は無い:

```sh
sqlite3 "file:/var/lib/celeris/celeris.sqlite3?mode=ro" "select json_extract(json,'$.type') t, count(*) from events group by t order by 2 desc"
```

## 評価

evaluate 葉（2026-10-03）。'## 実測' の表のとおり Phase 1〜5 の「導入後」は本番未反映で 0 件なので、ここでは「導入前」の本番 DB と Git から **root task 間の統合失敗の残り**を読み取り専用で補い、その上で merge train / project-level integration branch の要否を判断する。追加の問い合わせは同じ `$RO` と、登録リポジトリの Git（`git merge-tree --write-tree`・`git rev-list`。ref は動かしていない）だけを使った。

### 1. root task 間の統合失敗の残り（導入前の実測）

| 指標 | 値 | command / SQL |
|---|---|---|
| root の delivery 数（= main へ取り込もうとした root task 数） | 56 件（56 task）。うち push 済み 32 件、`ready` 23 件、`blocked` 31 件（reviewer 不合格 11・その他 20） | `sqlite3 "$RO" "select json_extract(json,'$.state'), json_extract(json,'$.decision'), count(*) from deliveries group by 1,2"` → `blocked\|\|2`, `blocked\|0\|11`, `blocked\|1\|18`, `preparing\|1\|1`, `ready\|1\|23`, `reviewing\|\|1` |
| root の merge-base ずれで `merge_base` repair を起票した task | **14 task・15 WU**（13 task は 1 回、1 task は 2 回）。delivery を持つ 56 task のうち 13 task = **23%** | `sqlite3 "$RO" "select status, count(*), count(distinct task_id) from work_units where kind='repair' and json_extract(json,'$.title') like 'repair (merge_base)%' group by 1"` → `done\|15\|14`。`select count(distinct d.task_id) from deliveries d join work_units w on w.task_id=d.task_id and w.kind='repair' and json_extract(w.json,'$.title') like 'repair (merge_base)%'` → 13 |
| `merge_base` repair が上限（ADR-0072 D16 の 2 回）に達して `[needs-human]` で止まった delivery | **6 件 / 56 = 11%**。detail に「main が進んだ・既定ブランチの更新を取り込んでから」を含む reviewer 不合格を足すと 10 件 / 56 = 18% | `sqlite3 "$RO" "select json_extract(json,'$.state'), substr(json_extract(json,'$.detail'),1,200) from deliveries where json_extract(json,'$.detail') like '%merge_base%' or json_extract(json,'$.detail') like '%merge-base%'"` → `[needs-human] 配送の局所修復が上限に達しました: merge_base` ×6。`… like '%既定ブランチの更新%' or … like '%main は%進んで%'` を足すと `10\|10` |
| 段末統合（ADR-0074/0079 の `integrate-<stage>`）の衝突率 | `phase_integrated` 352 件（複数ブランチの merge 86 件）に対し `merge_conflict` repair **15 件 / 352 = 4.3%**（複数 merge の 86 件に対しては 17%）、全件 done（自動修復で解消） | `sqlite3 "$RO" "select substr(json_extract(json,'$.class'),1,60), count(*) from events where json_extract(json,'$.type')='repair_scheduled' group by 1"` → `test_small\|61`, `merge_conflict\|15`, `planner\|11`, `lint\|4`, `merge_base\|2`, `format\|2`, `review_timeout\|1`。`select case when json_array_length(json_extract(json,'$.merged'))>1 then 'multi' else 'single' end, count(*) from events where json_extract(json,'$.type')='phase_integrated' group by 1` → `multi\|86`, `single\|266` |
| 段末統合の「merge は通ったが検査が落ちた」率 | `work_unit_checks_failed` 96 件・79 統合 WU / 352 = **22%**。`test_small` repair 61 件のうち done 56・blocked 3・cancelled 1・superseded 1 | `select count(*), count(distinct json_extract(json,'$.work_unit_id')) from events where json_extract(json,'$.type')='work_unit_checks_failed'` → `96\|79`。`select status, count(*) from work_units where kind='repair' and json_extract(json,'$.title') like 'repair (test_small)%' group by 1` |
| root の取り込み頻度（merge train が直列化する対象の到着率） | push 済み 32 件を 2026-09-23〜10-03 の 11 日に分散。最大の日は 2026-10-02 の **11 件**、次が 10-03 の 7 件。run を持つ root task は同じ 1 時間に最大 16 task | `sqlite3 "$RO" "select substr(json_extract(json,'$.pushed_at'),1,10) d, count(*) from deliveries where json_extract(json,'$.pushed_at') is not null group by d"`。`select count(distinct r.task_id), substr(r.started_at,1,13) h from runs r join tasks k on k.id=r.task_id where k.parent_id is null group by h order by 1 desc limit 1` → `16\|2026-10-02T13` |
| 取り込み時点の behind（push 済み delivery の `head` と、`pushed_at` 時点の main の近似値 `git rev-list -1 --before=<pushed_at> main` との差。commit 日時基準の近似） | n=33、**中央値 3 commit、p75 17、最大 173**。13 件は 0。100 commit 超は 3 件（いずれも task の生存が 3 日以上） | `sqlite3 -separator ' ' "$RO" "select task_id, json_extract(json,'$.head'), json_extract(json,'$.pushed_at') from deliveries where json_extract(json,'$.pushed_at') is not null"` の各行で `git rev-list --count <head>..$(git rev-list -1 --before=<pushed_at> main)` |
| 今 main に入っていない root/子 task ブランチの behind と衝突（2026-10-03 時点の断面） | 182 本の `celeris/<id>` のうち main 未取り込み **38 本**。main との `merge-tree` が衝突 **25 本**、clean 11 本、不明 2 本（60 秒で打ち切り）。生きている task（ready/running）の root 7 本では、衝突が `docs/PROGRESS.md` だけのもの 2 本（local-hot-data とその子）、crates/ に及ぶもの 5 本はすべて Phase 1〜5 の木（`01M3WZJ9C8…`、behind 429・ahead 127）とその兄弟 | `for b in $(git for-each-ref --format='%(refname:short)' 'refs/heads/celeris/*'); do git merge-base --is-ancestor $b main \|\| { git rev-list --count $b..main; git merge-tree --write-tree --name-only main $b; }; done`（出力は run の artifacts にだけ残した） |

読み取れること:

- **段の中（同じ task の WU・子 task 同士）の統合は既に merge train の形をしている**。ADR-0074 D1.4 / ADR-0079 D5 の `integrate-<stage>` は、段の枝を `seq` 順に 1 本ずつ `--no-ff` で親ブランチへ merge し、merge 結果の tree で検査を再実行し、落ちたら `merge_conflict` / `test_small` repair を同じ段に足す。衝突率 4.3%、検査不合格 22% はどちらも自動修復で done に収まっており（`merge_conflict` 15/15 done、`test_small` 56/61 done）、段の中に別の仕組みを足す理由は見当たらない。
- **残っている失敗は root と main の間に集中している**。delivery 56 件のうち 23% が `merge_base` repair を要し、11% は repair 上限に達して人待ちになった。原因は「review を通した SHA と main が離れていた」こと（reviewer の不合格文も「main は cb230f1 まで進んでおり、マージ結果ツリーで fmt が落ちる」の形）で、まさに ADR-0118（review 前に target へ rebase、`reviewed_sha = merge_candidate_sha` を固定、stale なら再 sync 3 attempt）、ADR-0120（rebase 衝突を成果保持の IntegrationRepair 2 回で解消、超えたら従来の `merge_base` 経路へ）、ADR-0130 D4/D5（behind の観測と stale の優先）が狙った箇所である。**この 3 つは本番にまだ 1 件も入っていない**（'## 境界'）ので、「既存機構で足りない」と言える証拠は今は無い。
- behind の分布は裾が長い（中央値 3、最大 173）が、大きい値は 3 日以上生きた task に限られる。merge train は「取り込みの瞬間の main」を揃える仕組みで、task が数日かけて積み上げた差分と main の意味的な衝突（`docs/PROGRESS.md` の同じ節、ADR 番号・migration 番号の重複）は解かない。生きている root 7 本の衝突 file が `docs/PROGRESS.md` に偏るのも、順番待ちでは消えない種類の衝突である。
- 到着率は 1 日最大 11 件、平均 3 件/日。直列化した train は 1 件あたり release（約 8 分）+ verify（約 75 秒）を要するので、11 件でも 1.7 時間で捌ける規模であり、待ち時間が問題になるほどの混雑はまだ無い。逆に言えば、取り込みは既に `release.sh` の lock と delivery の compare-and-swap fast-forward（ADR-0043/0051）で事実上 1 本ずつ直列に行われており、train を導入しても直列化そのものは増えない。

### 2. merge train / project-level integration branch が解く問題と増やす費用

| 案 | 解く問題 | 増やす費用・副作用 | 既存機構との重なり |
|---|---|---|---|
| **merge train**（取り込み待ちの root を列に並べ、前の候補を含む仮 main に rebase して検査し、順に fast-forward） | review 合格から取り込みまでに main が進む窓。2 件以上が同時に `ready` になったとき、2 件目を 1 件目込みで検査できる | 列の直列化（前の候補の検査が終わるまで後ろは着手できない）。**壊れた train の巻き戻し**: 列の途中で検査が落ちると後ろ全員の仮 main が無効になり再検査になる（1 件あたり約 9 分の release/verify が列の長さ分やり直し）。列の長さが 1 のときは Phase 1 の stale 再 sync と同じ動作になる | ADR-0118 D4「root の merge/delivery 直前に target と候補を照合し、違えば再 sync → 全 checks → reviewer」が列長 1 の train に等しい。ADR-0130 D5 の stale 優先が列の順序付けに当たる。釣り合うのは「同時に ready な delivery が常態的に 2 件以上」のときだけで、実測の到着率（平均 3 件/日、最大 11 件/日）ではその状態がまれ |
| **project-level integration branch**（案件ごとに `celeris/project-<id>` を置き、root は main でなくそこへ取り込み、案件の区切りで main へ） | 同じ案件の root 同士の衝突を main に出す前に吸収する。main の履歴が案件単位にまとまる | target が 2 段になり、behind・stale・review を **2 回**払う（root→案件 branch、案件 branch→main）。案件 branch 自体が main から遅れ、取り込み時に大きな merge_base ずれになる（今の長命 root と同じ問題を 1 段上に移すだけ）。ADR-0079 D6「main と比べるのは root だけ」の不変条件を変える | **ADR-0079 の木がこれを既に提供している**: 関連する実装を 1 つの親 task の子 task として並べれば、親ブランチ `celeris/<parent_id>` が integration branch、段末の `integrate-<stage>` が子の順次取り込みと再検査、main への取り込みは親 1 回になる。Phase 1〜5 の木（root `01M3WZJ9C8…` に 5 Phase の子）がその実例で、子 5 本は親に入り main には親から 1 回で入る。足りないのは「案件 = 木」ではなく、独立に起票された root 同士を後から束ねる手段だけである |

既存機構で**足りる点**: 段の中の順次 merge・再検査・自動修復（実測で全件 done）、取り込みの直列化（lock と CAS）、review→merge の窓（ADR-0118、本番反映待ち）、衝突の成果保持修復（ADR-0120、同）、behind の観測（ADR-0130、同）、案件単位の integration branch（ADR-0079 の親ブランチ）。

既存機構で**足りない点**（merge train でも解けないもの）: (a) `docs/PROGRESS.md`・ADR 番号・migration 番号のような**共有の連番・共有の節**で起きる意味的衝突。これは file の分割（task ごとの progress file、番号の予約）で減らす種類の問題で、取り込みの順序付けでは減らない。(b) 独立に起票された root を後から 1 つの親に束ねる操作（adopt）。(c) `merge_base` repair 上限到達後の人待ち 6 件の内訳が event に残らず、「衝突か、検査の不合格か」を今の DB から区別できない。ADR-0120 D5 の event（`integration_repair_*`）が本番に入れば区別できる。

## 結論

**現時点では不要**。merge train / project-level integration branch は作らない。ADR も follow-up task も追加しない。

理由: 実測で残っている統合失敗は root と main の間（delivery 56 件の 23% が `merge_base` repair、11% が repair 上限で人待ち、取り込み時 behind 中央値 3・最大 173）に集中しているが、その窓を閉じるために設計した Phase 1〜5（ADR-0118/0120/0130）は本番 release にまだ 1 件も入っておらず（'## 境界'）、「導入後」の数字が全て 0 件のままである。導入前の失敗を根拠に、同じ窓を狙う 2 つ目の仕組みを重ねる判断はできない。段の中の統合は既に merge 順次・再検査・自動修復の形で動いており（衝突 4.3%、検査不合格 22%、どちらも自動修復で done）、案件単位の integration branch は ADR-0079 の親ブランチで表現できる。

判断を変える条件（再評価の閾値）。Phase 1〜5 を含む release が本番 `current` になった日以降の delivery を母数にし、**母数が 30 件に達した時点**（到着率 3 件/日なら約 10 日後）で次を同じ `$RO` から測り直す。1 つでも超えたら merge train を「必要」として再評価し、そのとき ADR と follow-up を作る。

| 閾値 | 測り方 |
|---|---|
| `[needs-human]` で止まる delivery（`merge_base` 上限 + `integration_repair_exhausted`）が **10% 超**（導入前 11%から下がっていない） | `select count(*) from deliveries where json_extract(json,'$.detail') like '[needs-human]%' and json_extract(json,'$.pushed_at') is null` と `select count(*) from events where json_extract(json,'$.type')='integration_repair_exhausted'` |
| `review_target_advanced` の attempt が上限 3 に達して停止した root が **5% 超**、または 1 delivery あたりの再 sync→再検査の平均回数が **1.5 回超**（stale の往復が review の費用を増やしている） | `select json_extract(json,'$.attempt'), count(*) from events where json_extract(json,'$.type')='review_target_advanced' group by 1` |
| 同じ repo で **同時に `ready` の delivery が 2 件以上**ある時間が全体の 20% 超、または取り込み時 behind の中央値が **20 commit 超**（列に並べる価値が出る混雑） | `deliveries` の `state='ready'` の時刻の重なり、`behind_targets` の `behind_target_commits`（migration 0040）の delivery 直前の値 |
| `merge_conflict`/`test_small` repair で解消できなかった段末統合（blocked・cancelled のまま）が **10% 超** | `select status, count(*) from work_units where kind='repair' and json_extract(json,'$.title') like 'repair (merge_conflict)%' group by 1` |

閾値に届かない場合に先に手を付けるべきもの（merge train より安く、実測の衝突 file に直接効く。本文書では提案に留め、task は作らない）: `docs/PROGRESS.md` を task ごとの `docs/progress/<slug>.md` に分けて統合時の衝突源を消すこと、ADR 番号・migration 番号を起票時に予約すること、ADR-0120 D5 の event で `merge_base` 上限到達の内訳（衝突か検査不合格か）を残すこと。
