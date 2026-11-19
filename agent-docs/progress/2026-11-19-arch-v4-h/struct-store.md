---
title: アーキ v4 H struct-store — Store の公開面・SQLite 参照・Dispatcher の参照・Applied の形の残数（A の規則の before/after）
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# struct-store（WorkUnit）

before は main `bc665168c9caf1898adc4c611404bbee056daf9a`（A の基準）、after は `git merge-base HEAD main`（件名 `integrate wu/close (phase close)`、
アーキv4 G の close の統合）。数え方は A の `artifacts/baseline.md` §7 の C3・C4・C6・C7 と E の measure-commands.md の E1、D の付記の 2 本の command。
before の値は A・E・D の記録と全部一致した。数値の正本は [struct-store.tsv](struct-store.tsv)（列 `metric scope before after checker note`）。
source（`crates/`・`scripts/`・`web/`）と policy の file は変えていない。この WorkUnit の変更はこの md・tsv と
[ADR 2026-11-18 org-write-shared-use-case](../../adr/2026-11-18-org-write-shared-use-case.md) の付記の表のセルだけ。

## 0. 結論

- (2) SQLite を直接参照する production file は減っていない。`rusqlite` を書く file（task-core/src/store/ を除く）は 74 → 75（+1）、
  SQL 文字列を持つ file は 50 → 50。+1 は D が足した共有 use-case `crates/task-api/src/org_write.rs`（`use rusqlite::Transaction`。呼び手の
  transaction を受けて書く）。F は file の中の自前の接続を消したが、file の数は変えていない（F の進捗と同じ）。**未達/要判断。**
- (3) SQL の公開面は少し減った。`SqliteStore` の `pub` method 633 → 631（D の store-prune が `org_upsert_browser_settings`・`org_delete` を消した。
  private も 42 → 41 で `org_upsert_with_actor`）、`pub(crate)` 67 → 67、impl block 102 → 102、逆依存の crate 11 → 11。
  `&Connection`・`&Transaction` の引数は task-api 14 → 15（+1 は org_write.rs:77）、task-core 320・store/ 205・task-ops 7 は同じ。
  `Fn(&Connection)` の closure 引数は task-ops 5 → 4（F が `apply_project_action` の closure を消した）。
- Dispatcher の参照は E の値のまま（F・G は dispatcher を変えていない）。self.store を書く file は依頼 scope C3 44 → 38・E1 46 → 40、
  論理 scope C3 48 → 42・E1 50 → 44（どれも −6）。self.config は C3・E1 とも依頼 35 → 31・論理 39 → 35（−4）。分母は依頼 69・論理 76 で変わらない。
  出現数は C3 で self.store 633 → 633、self.config 218 → 220（+2 は rustfmt で割れていた 2 か所を 1 行にした分。E1 では 227 → 227）。
- `Applied` の形（`Option<&OperationAudit>`）の file は、狭い command 14 → 14、広い command 19 → 18。**回帰条件（広いほうが 18 以下）を満たす。**
  型の定義側 `cos/operations.rs` を除く操作の側は 17 file・54 関数。F の後に無い `record_op`・`project_action_op` を ADR の付記の表から除いた。

## 1. Store の公開面と SQLite の参照（A §7 C4・C6・C7）

| 指標 | 範囲 | before | after | 差 | 注 |
|---|---|---|---|---|---|
| `rusqlite` を書く production file | task-core/src/store/ の外 | 74 | 75 | +1 | org_write.rs（D）。crate 別 after: task-core 42・task-api 17・task-ops 8・celeris 4・ほか 4 |
| SQL 文字列を持つ production file | 同上 | 50 | 50 | 0 | file の集合も同じ |
| `&Connection`/`&Transaction` の引数（行） | task-core | 320（76 file） | 320（76 file） | 0 | |
| 同上 | task-core/src/store/ | 205（49 file） | 205（49 file） | 0 | task-core の内数。file 数は A に無く H が同じ grep で数えた |
| 同上 | task-api | 14（9 file） | 15（10 file） | +1 | org_write.rs:77 `tx: &Transaction<'_>` |
| 同上 | task-ops | 7（4 file） | 7（4 file） | 0 | lifecycle.rs の `apply_project_change_tx` の引数が `&rusqlite::Connection` → `&rusqlite::Transaction<'_>`（F） |
| `Fn(&Connection)` の closure 引数 | task-core / task-api / task-ops | 13 / 4 / 5 | 13 / 4 / 4 | −1 | task-ops/src/lifecycle.rs の `apply_project_action`（F） |
| `impl SqliteStore` の block | plain / path 付き込み / trait | 102 / 105 / 6 | 102 / 105 / 6 | 0 | 88 file のまま |
| `SqliteStore` の `pub` method | plain / path 付き込み | 633 / 636 | 631 / 634 | −2 | `org_upsert_browser_settings`・`org_delete`（D の store-prune） |
| `SqliteStore` の `pub(crate)` 等 | plain / path 付き込み | 67 / 67 | 67 / 67 | 0 | |
| `SqliteStore` の private method | plain | 42 | 41 | −1 | `org_upsert_with_actor`（D） |
| task-core の逆依存の crate | 通常依存の直接・間接 | 11 | 11 | 0 | Cargo の宣言 normal 35・dev 14 も同じ |

A の数え方の外の補足（F の進捗の値。H は再計測していない）: task-core に `&SqliteStore` を取る `pub` の自由関数 `attachment_provenance` が
F で 1 増えた（7 → 8）。method を増やさない代わりに関数にしたもので、上の `pub` method の数には入らない。

## 2. Dispatcher の参照（A §7 C3 と E の E1）

scope の `request` は依頼の scope（dispatcher/ の非試験 69 file）、`logical` は論理 scope（69 − `project_decider_ts.rs` ＋ `#[path]` の 8 file）。
after で `git grep -n '#\[path' <rev> -- crates/task-dispatch/src` を引き直した。非試験の `#[path]` は before・after とも同じ 8 file と lib.rs:47 で、
論理 scope は同じ 76 file になった。C3 は 1 行の中の `self.store`、E1 は改行をまたぐ `self\s*\.\s*(store|config)\b`。値は「file 数 / 出現数」。

| scope / 数え方 | 分母 | self.store before | self.store after | self.config before | self.config after |
|---|---|---|---|---|---|
| request / C3 | 69 → 69 | 44 / 633 | 38 / 633 | 35 / 218 | 31 / 220 |
| request / E1 | 69 → 69 | 46 / 759 | 40 / 759 | 35 / 227 | 31 / 227 |
| logical / C3 | 76 → 76 | 48 / 661 | 42 / 661 | 39 / 224 | 35 / 226 |
| logical / E1 | 76 → 76 | 50 / 791 | 44 / 791 | 39 / 233 | 35 / 233 |

E の metrics.md（HEAD = E の提出時）の値と全部同じ。E の後の D〜G の統合で dispatcher の参照は変わっていない。

## 3. `Applied` の形の残数（D の付記の 2 本の command）

| command | before | after | 注 |
|---|---|---|---|
| 狭い `git grep -l 'Option<&OperationAudit>' crates/task-api/src` | 14 | 14 | file の集合も同じ（`handlers/org.rs` はもともと入らない） |
| 広い `git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" crates/task-api/src` | 19 | 18 | −1 は `handlers/org.rs`（D）。回帰条件 18 以下を満たす |
| `git grep -lE '\bApplied\b' crates/task-api/src`（付記の照合用） | 19 | 18 | 広い command と同じ集合 |
| 補助: 広い command の file で該当の引数を受ける関数 | 58 | 55 | −3 は `org_commit`（D）・`record_op`・`project_action_op`（F） |

after の file ごとの関数と領域（広い command の 18 file。★は狭い command に当たらない file）:

| 領域 | file | 関数 |
|---|---|---|
| （型の定義側） | `cos/operations.rs` | `file_op` |
| 承認 | `approvals.rs` | `decide_op`・`create_standing_rule_op`・`delete_standing_rule_op` |
| 変更 | `changes.rs` | `changes_effect`・`integrate_op`・`pr_merge_op` |
| 判断 | `decisions.rs` | `answer_op`・`withdraw_op`・`revise_op`・`resolve_op` |
| 文書 | `docs.rs` | `docs_effect`・`reject_for`・`docs_init_op`・`put_page_op`・`delete_page_op`・`promote_op`・`maintenance_op` |
| 知識 | `knowledge.rs` | `accept_op`・`reject_op`・`put_page_op`（`put_page_op` は完全修飾の書き方） |
| 案件 | `handlers/projects.rs` | `create_project_op`・`put_project_brief_op` |
| task | `handlers/task_actions.rs` | `retry_op`・`patch_task_op`・`create_comment_op`・`reopen_op`・`rereview_op` |
| task | `handlers/tasks.rs` | `create_task_op`・`create_task_linked_op` |
| task | `integrate_task.rs` | `integrate_task_op` |
| task | `lifecycle.rs` | `task_pause_op` |
| 実行計画 | `execution.rs` ★ | `execution_plan_adopt_op`・`tree_adopt_op`・`phase_gate_op`・`decompose_op` |
| cron | `cron_jobs.rs` ★ | `type Audit<'a> = Option<&'a OperationAudit>` を受ける `create_job_op`・`update_job_op`・`delete_job_op`・`pause_job_op`・`resume_job_op`・`run_job_op` と補助の `reject_with`・`resolve_for` |
| clusters | `handlers/clusters.rs` ★ | `put_local_settings_op`・`put_cluster_settings_op` |
| inbox の通知 | `inbox_notifications.rs` ★ | `notice_read_op`・`notice_read_all_op` |
| skills | `skills.rs` | `put_skill_op`・`delete_skill_op` |
| repos | `repos.rs` | `create_repo_op`・`patch_repo_op`・`delete_repo_op` |
| model_catalog | `model_catalog.rs` | `put_override_op`・`delete_override_op` |

領域ごとの残数（file / 関数）: 承認 1/3、変更 1/3、判断 1/4、文書 1/7、知識 1/3、案件 1/2、task 4/9、実行計画 1/4、cron 1/8、clusters 1/2、
inbox の通知 1/2、skills 1/2、repos 1/3、model_catalog 1/2。計 17 file・54 関数（ほかに型の定義側 1 file・1 関数）。

### 領域ごとの移行の提案（起票は人が決める。H は起票しない）

ADR の付記の「撤去の条件」は「その領域の共有の操作を変える task が、同じ変更で org_write の形へ移す」。下の表はその上で、先に専用の task を
立てる価値がある領域と、次に変える task に任せる領域を分けた提案。

| 領域 | 提案 | 理由 |
|---|---|---|
| 知識（`accept_op`・`reject_op`・`put_page_op`） | 移行の task を提案する | F が同じ file の `record_op` を移し、HTTP は `write_tx`、CoS は `reject`・`apply_checked` の手順と契約試験が既にある。`knowledge.rs` は `transaction` class の高 risk の path で、2 つの形が同じ file に並ぶ状態を早く解く価値がある（F の提案 3） |
| task の `lifecycle.rs`（`task_pause_op`） | 移行の task を提案する（知識と別の 1 件） | F が同じ file の `project_action_op` を移した。1 関数で、F の lifecycle-tx と同じ手順を当てられる見込み（H は確かめていない） |
| skills（`put_skill_op`・`delete_skill_op`） | 移行の task を提案する | `skills.rs` は D で org の書き込みを org_write に移した HTTP adapter で、同じ file に 2 つの形が並ぶ |
| 承認・変更・判断・文書・案件・task（`task_actions.rs`・`tasks.rs`・`integrate_task.rs`）・実行計画・cron・clusters・inbox の通知・repos・model_catalog | 意図して残す（その領域を次に変える task が移す） | 領域ごとに HTTP と CoS の契約試験を旧コードで先に固定する作業が要る（D の付記の「D がこの task で全部を移さない理由」と同じ）。新しい操作をこの形で足さないことは広い command の回帰条件（18 以下）で見る |

## 4. ADR の付記の表の修正

[ADR 2026-11-18 org-write-shared-use-case](../../adr/2026-11-18-org-write-shared-use-case.md) の「`Applied` の形を残す file」の 2 つの表のセルだけを、
after の関数に合わせた。表の外の本文・決定・見出しは変えていない。

- `knowledge.rs` の行: `record_op` を除き、`accept_op`・`reject_op`・`put_page_op` を書いた。
- `lifecycle.rs` の行: `project_action_op` を除き、`task_pause_op` だけにした。
- `handlers/org.rs` の行: `org_commit` は D が消したことを書いた（本文の「上の 14 と次の 5」と合わせるため行は残した）。
- ほかの行は after の関数と一致した（上の §3 の表と照合）。

## 5. A と違う数え方

- E1（改行をまたぐ参照）: E の申し送りで C3 と並べた。C3 は rustfmt の改行で出現数が変わる（self.config の +2）。両方の値を §2 に書いた。
- `conn_arg_task_core_store` の file 数（49）: A は store/ の行の数（205）だけを書いていた。同じ grep の出力の file 数を足した。
- 逆依存の crate 数: before は `git archive bc665168 Cargo.toml Cargo.lock crates tests/e2e` を `$TMPDIR` に出して `cargo metadata --offline` を当てた
  （作業ツリーを切り替えないため）。値は A と同じ 11・normal 35・dev 14。
- `Applied` の関数の数（58 → 55）: A に無い補助の数。D の付記の表の照合に使った awk（下の再現コマンド）。

## 6. 再現コマンド（repo の root で。`B` を before と after の sha に替える）

```sh
B=bc665168c9caf1898adc4c611404bbee056daf9a   # after は B=$(git merge-base HEAD main)
X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'; P='^crates/(campaign|routing-eval|jev-classifier)/'
# C4 rusqlite・SQL 文字列（task-core/src/store/ を除く production file）
git grep -l 'rusqlite' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/' | wc -l
git grep -lE '"[^"]*\b(SELECT .* FROM|INSERT (OR [A-Z]+ )?INTO|UPDATE [a-z_]+ SET|DELETE FROM)\b' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/' | wc -l
# C6 &Connection の引数（crate ごと。store/ は ^crates/task-core/src/store/ で絞る）と Fn の closure
git grep -nE '\b[a-z_]+: &(mut )?(rusqlite::)?(Connection|Transaction)\b' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" > $TMPDIR/ca
for c in task-core task-api task-ops task-core/src/store; do echo "$c files=$(grep "^crates/$c/" $TMPDIR/ca | cut -d: -f1 | sort -u | wc -l) occ=$(grep -c "^crates/$c/" $TMPDIR/ca)"; done
git grep -nE 'Fn(Once|Mut)?\(&(mut )?(rusqlite::)?(Connection|Transaction)' $B -- 'crates/*.rs' | sed "s/^$B://" | grep -vE "$X" | grep -vE "$P" | cut -d/ -f2 | sort | uniq -c
# C6 SqliteStore: impl block と pub method は A の baseline.md §7 C6 の git grep と python の snippet をそのまま使う（path 付きは同 §7 の注の正規表現）
# C7 逆依存: cargo metadata --format-version 1 --offline の workspace_members の normal 依存を逆にたどる（before は git archive した tree で実行）
# C3・E1 dispatcher: E の measure-commands.md の C3（request と logical.txt）と E1 の snippet をそのまま使う
# Applied の形（D の付記）
git grep -l 'Option<&OperationAudit>' $B -- crates/task-api/src | wc -l
git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" $B -- crates/task-api/src | wc -l
# 補助: file ごとの関数（type Audit の行は除く）
for f in $(git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" $B -- crates/task-api/src | sed "s/^$B://"); do
  git show $B:$f | awk -v f=$f '/^[[:space:]]*(pub(\([a-z]+\))? )?(async )?fn [a-z_0-9]+/ {match($0,/fn [a-z_0-9]+/); fn=substr($0,RSTART+3,RLENGTH-3)}
    /^type Audit</ {next}
    /Option<&('"'"'[a-z_]+ )?(crate::cos::operations::)?OperationAudit>/ || /: Audit<'"'"'_>/ {print f, fn}'; done | sort -u | wc -l
```

## 7. 未達・要判断

- (2) `rusqlite` を書く file の数は 74 → 75 で減っていない。SQL 文字列の file も 50 → 50。D・F は file の中の接続・closure を減らしたが、
  A の正規表現は「接続を開く行」と「呼び手の `&Transaction` を受ける行」を区別しないので、file の数には出ない。未達/要判断として final-report に渡す。
- (3) `&Connection`・`&Transaction` の引数は task-api で +1。D の共有 use-case が呼び手の transaction を受ける形にしたためで、接続を開く file は増えていない。

## 8. 提案（起票は人が決める）

- §3 の「領域ごとの移行の提案」の 3 件（知識・`task_pause_op`・skills）。
- F の提案 1・2（`chat/attachments.rs`・`cos/inbox.rs` の自前の接続を store の接続へ移す、`state.chat.attachment_db_path` を消す条件）は
  `rusqlite` を書く file の数を減らす候補になる。H は起票しない。
- A の C4 の正規表現に「接続を開く行」（`Connection::open`・`TransactionBehavior`・`busy_timeout`）の file 数を補助の指標として並べると、
  F のような改善が数に出る。次の計測の規則の変更の提案として書く（H は規則を変えない）。

## 提出

commit は件名 `arch-v4 H struct-store: Store の公開面・SQLite 参照・Dispatcher の参照・Applied の形の残数を数え、ADR の付記の表を直す`。
WorkUnit の check（17 key・applied_wide の一致・doc link・source 差分 0・範囲）は提出時の HEAD で実行した。
