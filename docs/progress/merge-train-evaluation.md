---
tasks: [01M3Z6NZ0RBF4ZH2H2KC69QT9S]
---
# merge train / project-level integration branch の評価

測った日: 2026-10-03（UTC 04:5x 頃）。本番 DB と release 一覧は読み取りだけで見た。DB への書き込み・daemon の再起動・設定の変更はしていない。

## 境界

Phase 1〜5 の導入時点。日時は git の commit 日時（UTC）。

| Phase | ADR（日付・追加 commit） | 完了の記録 | 統合 commit | 本番 release に入っているか |
|---|---|---|---|---|
| 1 review 前の target 同期・merge candidate 固定 | ADR-0118（2026-10-02、`034d7e91` 2026-10-02 00:37:36） | docs/PROGRESS.md「Phase 1 …（ADR-0118、2026-10-02）」 | `a00c28b2` 2026-10-02 06:34:42 integrate wu/p1-sync (phase p1) | 入っていない |
| 2 review 前同期の IntegrationRepair | ADR-0120（2026-10-02、`6719fc8e` 2026-10-02 06:49:46） | docs/PROGRESS.md「Phase 2 …（ADR-0120、2026-10-02）」 | `6a385df3` 2026-10-02 10:20:51 integrate wu/p2-repair (phase p2) | 入っていない |
| 3 Claude session resume | ADR-0124 claude-session-resume（2026-10-02、`ce2c836f` 2026-10-02 10:37:46） | docs/PROGRESS.md「Phase 3 …（ADR-0124、2026-10-02）」 | `86841071` 2026-10-02 13:33:34 integrate wu/p3-session (phase p34) | 入っていない |
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
