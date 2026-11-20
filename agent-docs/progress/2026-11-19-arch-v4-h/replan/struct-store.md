---
title: アーキ v4 H R8b — Store の公開面・SQLite 参照・Dispatcher の参照・Applied の形を R1〜R7 の後に数え直す
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# replan の struct-store（WorkUnit rc-store）

H の [struct-store.md](../struct-store.md) と同じ手順・同じ key・同じ scope と checker の文字列で、R1〜R7 が H の branch に入った後の HEAD を数え直した。
数値の正本は [struct-store.tsv](struct-store.tsv)（列 `metric scope before after checker note`）。before は H の
[struct-store.tsv](../struct-store.tsv) の after 列の写し、after は HEAD（この WorkUnit の base は件名 `integrate wu/r6-release (phase fix2)`。
この WorkUnit は docs だけを足したので、crates/ の tree は base と同じ）。source（`crates/`・`scripts/`・`web/`）と policy の file は変えていない。

## 0. 結論

- `Applied` の形（`Option<&OperationAudit>`）の file は、広い数え 18 → 15、狭い数え 14 → 11、補助の関数の数え 55 → 49。
  減ったのは R5 が移した `knowledge.rs`・`lifecycle.rs`・`skills.rs` の 3 file と 6 関数だけで、足された file・関数は 0。
  **回帰条件（広い数えが 18 以下）を満たす**（15）。操作の側（型の定義側 `cos/operations.rs` を除く）は 14 file・48 関数、12 領域が残る。
- (2) SQLite を直接参照する production file（A の C4）は `rusqlite` 75 → 74、SQL 文字列 50 → 49（どちらも R4）。B0 と比べると `rusqlite` は 74 → 74 で
  **減っていない（未達/要判断のまま）**。SQL 文字列は B0 50 → 49。R7 の補助の指標 `conn_open_files`（接続を開く行を持つ file）は
  H の base 47 → 45（R4 の −2。B0 は 48 なので B0 比 −3）。
- (3) SQL の公開面は R1〜R7 で変わっていない。`SqliteStore` の impl 102・`pub` method 631・`pub(crate)` 67・private 41・逆依存の crate 11、
  `&Connection`・`&Transaction` の引数（task-core 320・store/ 205・task-api 15・task-ops 7）と `Fn(&Connection)` の closure（13・4・4）も同じ数。
- Dispatcher の参照は、file の数が変わらず（self.store は request/C3 38・logical/C3 42、self.config は 31・35）、出現数だけが減った
  （R3。self.store −3、self.config −8。4 scope とも同じ差）。分母は依頼 69・論理 76 のまま。
- H の base から HEAD までに入った commit は、H の WorkUnit と R1〜R7 の子 task・WorkUnit のものだけだった。**R 以外の main の commit は HEAD に無い**
  （下の「基点」）。

## 基点

| 名 | commit | 説明 |
|---|---|---|
| B0 | `bc665168c9caf1898adc4c611404bbee056daf9a` | A の基準（main、2026-11-17） |
| H measure | 件名 `integrate wu/close (phase close)`（commit 日時 2026-11-19T10:37:13Z、アーキv4 G の close の統合） | H の struct-store の after。H の base で、今も `git merge-base HEAD main` と同じ commit（main の先端もこの commit） |
| replan の後 | 提出時の HEAD | R1〜R7 の後。first-parent は H の measure・verify・report の統合 7 件と R の統合 8 件（`integrate wu/r1-prompts`〜`integrate wu/r6-release`） |

R 以外の main の commit の確認: `git log --format='%s' <H measure>..HEAD` の 127 件は、`integrate wu/…` の merge と、件名が `arch-v4 H …`（H の WorkUnit と
R1〜R5・R7）か R5・R6 の WorkUnit の件名（`r5 contract:`・`adr:`・`counts:`・`summary:`・`close:`・`coverage:`）の commit だけだった。
`integrate wu/` 以外の merge は 0 件（main の取り込みの merge は無い）。main の先端は H measure と同じ commit なので、H measure の後の main の commit は無い。

## 1. Store の公開面と SQLite の参照（A §7 C4・C6・C7 と R7 の D1）

| 指標 | 範囲 | B0 | H measure | replan の後 | 差（H → replan） | 注 |
|---|---|---|---|---|---|---|
| `rusqlite` を書く production file | task-core/src/store/ の外 | 74 | 75 | 74 | −1 | 外れた 2（R4: `task-api/src/chat/attachments.rs`・`task-api/src/cos/triage_view.rs`）、入った 1（R4 triage: `task-core/src/cos_inbox.rs`）。crate 別 after: task-core 43・task-api 15・task-ops 8・celeris 4・ほか 4 |
| SQL 文字列を持つ production file | 同上 | 50 | 50 | 49 | −1 | file の集合の差は上と同じ |
| `conn_open_files`（R7 の D1。checker `r7-d1`） | 同上 | 48 | 47 | 45 | −2 | R4 が 2 file の `Connection::open`・`busy_timeout` を消した。入った file 0。`cos_inbox.rs` は接続を開かず `SqliteStore::with_read_conn` を使う |
| `&Connection`/`&Transaction` の引数（行・file） | task-core | 320・76 | 320・76 | 320・76 | 0 | |
| 同上 | task-core/src/store/ | 205・49 | 205・49 | 205・49 | 0 | |
| 同上 | task-api | 14・9 | 15・10 | 15・10 | 0 | 行番号の移動だけ（authority.rs・project_controller.rs・roadmap.rs・tasks.rs・knowledge.rs・repos.rs） |
| 同上 | task-ops | 7・4 | 7・4 | 7・4 | 0 | |
| `Fn(&Connection)` の closure 引数 | task-core / task-api / task-ops | 13 / 4 / 5 | 13 / 4 / 4 | 13 / 4 / 4 | 0 | |
| `impl SqliteStore` の block | plain / path 付き込み / trait | 102 / 105 / 6 | 102 / 105 / 6 | 102 / 105 / 6 | 0 | |
| `SqliteStore` の `pub` method | plain / path 付き込み | 633 / 636 | 631 / 634 | 631 / 634 | 0 | R4 の `cos_inbox.rs` は method でなく自由関数（`&SqliteStore` を取る `pub fn` 4 つ）なので、この数に入らない |
| `SqliteStore` の `pub(crate)` 等 | plain / path 付き込み | 67 / 67 | 67 / 67 | 67 / 67 | 0 | |
| `SqliteStore` の private method | plain | 42 | 41 | 41 | 0 | |
| task-core の逆依存の crate | 通常依存の直接・間接 | 11 | 11 | 11 | 0 | 宣言 normal 35・dev 14 も同じ。H measure から HEAD まで Cargo.toml・Cargo.lock の差分 0 |

## 2. Dispatcher の参照（A §7 C3 と E の E1）

HEAD で `git grep -n '#\[path' HEAD -- crates/task-dispatch/src` を引き直した。非試験の `#[path]` は H measure と同じ 8 file
（design_gate・design_verdict・plan_adoption・policy_review・worker_profile・check_item・review_limits・inventory）と lib.rs:47 の
`project_decider_ts.rs` で、論理 scope は 76 file のまま。値は「file 数 / 出現数」。

| scope / 数え方 | 分母（B0 → H → replan） | self.store B0 | H measure | replan の後 | self.config B0 | H measure | replan の後 |
|---|---|---|---|---|---|---|---|
| request / C3 | 69 → 69 → 69 | 44 / 633 | 38 / 633 | 38 / 630 | 35 / 218 | 31 / 220 | 31 / 212 |
| request / E1 | 69 → 69 → 69 | 46 / 759 | 40 / 759 | 40 / 756 | 35 / 227 | 31 / 227 | 31 / 219 |
| logical / C3 | 76 → 76 → 76 | 48 / 661 | 42 / 661 | 42 / 658 | 39 / 224 | 35 / 226 | 35 / 218 |
| logical / E1 | 76 → 76 → 76 | 50 / 791 | 44 / 791 | 44 / 788 | 39 / 233 | 35 / 233 | 35 / 225 |

H measure → replan の差は R3 の 4 file だけ: self.store は `tree_units.rs` −1・`cos_chat/root_done.rs` −2、self.config は `cos_chat/triage.rs` −3・
`cos_chat/fallback.rs` −5（file ごとの `grep -o` の数を H measure と HEAD で比べた）。参照を持つ file は 1 つも 0 にならず、file 数は変わらない。
R3 の [measure.md](../../2026-11-19-arch-v4-h-r3/measure.md) の after（store 633 → 630、config 220 → 212）と一致した。

## 3. `Applied` の形の残数（D の付記の 2 本の command）

| command | B0 | H measure | replan の後 | 注 |
|---|---|---|---|---|
| 狭い `git grep -l 'Option<&OperationAudit>' crates/task-api/src` | 14 | 14 | 11 | −3 = `knowledge.rs`・`lifecycle.rs`・`skills.rs`（R5） |
| 広い `git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" crates/task-api/src` | 19 | 18 | 15 | 同じ 3 file。入った file 0。**回帰条件 18 以下を満たす** |
| 補助: 広い command の file で該当の引数を受ける関数 | 58 | 55 | 49 | −6 = `accept_op`・`reject_op`・`put_page_op`（knowledge.rs）、`task_pause_op`、`put_skill_op`・`delete_skill_op`（R5） |

replan の後の file ごとの関数と領域（広い command の 15 file。★は狭い command に当たらない file）:

| 領域 | file | 関数 |
|---|---|---|
| （型の定義側） | `cos/operations.rs` | `file_op` |
| 承認 | `approvals.rs` | `decide_op`・`create_standing_rule_op`・`delete_standing_rule_op` |
| 変更 | `changes.rs` | `changes_effect`・`integrate_op`・`pr_merge_op` |
| 判断 | `decisions.rs` | `answer_op`・`withdraw_op`・`revise_op`・`resolve_op` |
| 文書 | `docs.rs` | `docs_effect`・`reject_for`・`docs_init_op`・`put_page_op`・`delete_page_op`・`promote_op`・`maintenance_op` |
| 案件 | `handlers/projects.rs` | `create_project_op`・`put_project_brief_op` |
| task | `handlers/task_actions.rs` | `retry_op`・`patch_task_op`・`create_comment_op`・`reopen_op`・`rereview_op` |
| task | `handlers/tasks.rs` | `create_task_op`・`create_task_linked_op` |
| task | `integrate_task.rs` | `integrate_task_op` |
| 実行計画 | `execution.rs` ★ | `execution_plan_adopt_op`・`tree_adopt_op`・`phase_gate_op`・`decompose_op` |
| cron | `cron_jobs.rs` ★ | `type Audit<'a> = Option<&'a OperationAudit>` を受ける `create_job_op`・`update_job_op`・`delete_job_op`・`pause_job_op`・`resume_job_op`・`run_job_op` と補助の `reject_with`・`resolve_for` |
| clusters | `handlers/clusters.rs` ★ | `put_local_settings_op`・`put_cluster_settings_op` |
| inbox の通知 | `inbox_notifications.rs` ★ | `notice_read_op`・`notice_read_all_op` |
| repos | `repos.rs` | `create_repo_op`・`patch_repo_op`・`delete_repo_op` |
| model_catalog | `model_catalog.rs` | `put_override_op`・`delete_override_op` |

領域ごとの残数（file / 関数）: 承認 1/3、変更 1/3、判断 1/4、文書 1/7、案件 1/2、task 3/8、実行計画 1/4、cron 1/8、clusters 1/2、inbox の通知 1/2、
repos 1/3、model_catalog 1/2。計 14 file・48 関数（ほかに型の定義側 1 file・1 関数）。H measure の表から、知識 1/3・task の `lifecycle.rs` 1/1・skills 1/2 の
3 行が消えた（R5。今の名前は [ADR 2026-11-18 org-write-shared-use-case](../../../adr/2026-11-18-org-write-shared-use-case.md) の付記の「移した file」）。

### 領域ごとの移行の提案（起票は人が決める。この WorkUnit は起票しない）

H の struct-store.md §3 で「移行の task を提案する」とした 3 領域（知識・`task_pause_op`・skills）は R5 が移した。残る 12 領域は、H の struct-store.md と同じく
「意図して残す（その領域の共有の操作を次に変える task が、同じ変更で org_write の形へ移す）」とする。領域ごとに HTTP と CoS の契約試験を旧コードで先に固定する
作業が要り（D の付記の「D がこの task で全部を移さない理由」）、R5 の 6 関数の移行も契約試験 `applied_contract_` を先に置いてから移した。
新しい操作をこの形で足さないことは、広い command の回帰条件で見る。

## 4. ADR の付記の表との照合

[ADR 2026-11-18 org-write-shared-use-case](../../../adr/2026-11-18-org-write-shared-use-case.md) の付記「`Applied` の形を残す file」の 2 つの表
（狭い 11 file、広いだけの 4 file）を §3 の表と照合し、file と関数が全部一致した（R5 の WorkUnit `after` が HEAD に合わせた）。
この WorkUnit は ADR を変えていない（範囲は replan/struct-store.md・tsv だけ）。

## 5. 手順と検査器の版

- 数え方は H の struct-store.md §6 と同じ: A の `baseline.md` §7 の C3・C4・C6・C7、E の measure-commands.md の E1、D の付記の 2 本の command、
  H の補助の awk、R7 の ADR（[2026-11-19-arch-v4-measure-rules-addendum](../../../adr/2026-11-19-arch-v4-measure-rules-addendum.md)）の D1。
  どれも `git grep`・`git show`・python の snippet で、repo-health の checker は使わない。scope と checker の文字列は H の tsv のまま。
- 同じ snippet を H measure の commit に当て、H の struct-store.tsv の after 列の 45 行が全部一致することを確かめてから HEAD に当てた
  （手順が H と同じことの確認）。`conn_open_files` は H measure で 47・B0 で 48 で、R7 の ADR の D2 の表と一致した。
- `conn_open_files` の checker は `r7-d1`（R7 の ADR の D2 は「checker の欄で区別する」）。R4 の metrics.tsv は同じ指標を checker `a-c4` で書いている。数え方は同じ。
- 逆依存の crate は HEAD の作業ツリーで `cargo metadata --format-version 1 --offline`（cargo 1.98.1）。H measure から HEAD まで Cargo.toml・Cargo.lock の差分が 0 なので、
  H measure の値（11）は作り直していない。

## 6. 再現コマンド（repo の root で）

```sh
B0=bc665168c9caf1898adc4c611404bbee056daf9a
H=$(git log --first-parent --format=%H -1 --grep='^integrate wu/close (phase close)$' HEAD)   # H measure（G の close の統合）
R=HEAD
# C4・C6・C3・E1・Applied の snippet は H の struct-store.md §6 と A の baseline.md §7・E の measure-commands.md をそのまま使い、B を $H と $R に替える。
# R7 の D1（conn_open_files）
X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'; P='^crates/(campaign|routing-eval|jev-classifier)/'
for B in $B0 $H $R; do git grep -lE 'Connection::open|TransactionBehavior|busy_timeout' $B -- 'crates/*.rs' | sed "s/^$B://" \
  | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/' | wc -l; done                      # 48 47 45
# file の集合の差（rusqlite の例。conn_open・SQL 文字列も同じ形）
diff <(git grep -l rusqlite $H -- 'crates/*.rs' | sed "s/^$H://" | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/') \
     <(git grep -l rusqlite $R -- 'crates/*.rs' | sed "s/^$R://" | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/')
# dispatcher の file ごとの差（R3 の 4 file）
for r in $H $R; do git grep -c -E 'self\.(store|config)' $r -- crates/task-dispatch/src/dispatcher/ | sed "s/^$r://" > $TMPDIR/pf.$(git rev-parse --short $r); done
# Applied の形（回帰条件: 広いほうが 18 以下）
git grep -l 'Option<&OperationAudit>' crates/task-api/src | wc -l                                              # 11
git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" crates/task-api/src | wc -l       # 15
# R 以外の main の commit の確認
git log --merges --format='%s' $H..HEAD | grep -v '^integrate wu/'                                             # 出力なし
git log --no-merges --format='%s' $H..HEAD | grep -vE '^(arch-v4 H |r5 contract:|adr:|counts:|summary:|close:|coverage:)'   # 出力なし
git merge-base HEAD main; git rev-parse main                                                                   # どちらも $H
# 逆依存の crate
cargo metadata --format-version 1 --offline > $TMPDIR/meta.json   # workspace_members の normal 依存を逆にたどる（11、宣言 normal 35・dev 14）
```

この WorkUnit の計測の台本（上の snippet を 1 本にしたもの。H measure と HEAD に当てた）は run の成果物 dir の `raw/measure.sh` に置いた（repo には入れていない）。

## 7. 未達・要判断

- (2) A の C4 の `rusqlite` を書く file は B0 74 → replan の後 74 で、B0 と比べて減っていない。R4 は task-api の 2 file から接続と `rusqlite` を消したが、
  受信箱の読み取りを task-core の `cos_inbox.rs`（chat module の行の上限のため新しい top-level module）に置いたので、file は 1 つ増えた。
  接続を開く file（`conn_open_files`）は B0 48 → 45 で減っている（R7 の D2 により、A との比較の合否は C4 で決め、`conn_open_files` は補助）。
- (3) `SqliteStore` の公開面と `&Connection` の引数は R1〜R7 で変わっていない（R1〜R7 に公開面を減らす子は無かった）。
- Dispatcher: 出現数は減ったが、参照を持つ file の数は E の後から変わっていない（R3 の進捗の「未達」と同じ）。

## 8. 提案（起票は人が決める）

- 回帰条件の上限: 広い数えは 15 になった。上限を 18 のままにするか、15 に下げるかは人が決める（下げれば R5 の後の状態から増やさない条件になる）。
- `Applied` の形の残る 12 領域は §3 のとおり「その領域を次に変える task が移す」。専用の移行の task を立てるかは人が決める。
