---
title: アーキ v4 F metrics — 共通受入条件 (1)〜(6) の before→after（A と同じ規則）
tasks: [01M7MZTKB8A4NJQEEA35Q4G5JV]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# アーキ v4 F — after の値（WorkUnit `metrics`）

完了日: 2026-11-19。ADR は [2026-11-18-knowledge-lifecycle-tx-boundary](../../adr/2026-11-18-knowledge-lifecycle-tx-boundary.md)、
before の値と command は [adr.md](adr.md)。この WorkUnit は `crates/`・`web/` を変えていない（文書はこの file だけ）。

## 結論

- 減ったもの: `knowledge.rs` の自前の接続の行 10 → 0、task-ops の `lifecycle.rs` の Connection の closure の行 3 → 0、
  `Fn(&Connection)` の引数 22 → 21、3 file の `rusqlite` の出現 10 → 2、task-ops が開く transaction 1 → 0、
  crate の外へ `&Connection` を渡す公開の関数 2 → 0（`apply_project_action` の撤去と `provenance_for_owner` の `pub` の撤去）、
  production の行 375,818 → 375,781（−37、Rust も −37）。
- 変わらないもの: `rusqlite` を書く production file 75、SQL 文字列の file 50、`&Connection`・`&Transaction` の引数の行 347、
  handlers⇄cos の行 19・50、循環 17 件・単位 198・組 756、C7b の逆 import、`project_controller_human_bump_tx` の呼び手 file 9、D の付記の 2 本の command 14・18。
- 増えたもの: 循環の中の行の辺 1,815 → 1,821（+6。新しい組は 0）、task-core の `&SqliteStore` を取る `pub` の自由関数 7 → 8（`attachment_provenance`）、
  試験の行 +841、own_total_with_tests +804。
- 未達/要判断: (5) の build・test-affected の 3 回比較（子 G が測る）と review の費用。repo_health の全 rule の時間は 2 回ずつだけ測った。

## 基準

- before: `B=$(git merge-base HEAD main)`。adr.md の before と同じ commit（件名「Merge commit '…' into celeris/01M7MZTKB6GQ241F6JJYSPJNS1」）。
- after: `H=$(git rev-parse HEAD)`。この WorkUnit の base（`$CELERIS_WU_BASE`、件名「WU tracked scope base」）の tree と同じで、
  その tree は「integrate wu/knowledge-pin (phase impl)」と同じ（`git diff --quiet $CELERIS_WU_BASE HEAD` が exit 0）。
  impl の段の 3 unit（lifecycle-tx・provenance-read・knowledge-pin）と prep の段（adr・contract-tests）の後の値である。
- 計測は commit された tree に `git grep`・`git show` で当てた（作業ツリーは使っていない）。この WorkUnit の commit は文書だけなので、
  提出時の HEAD の `crates/` は計測した tree と同じ。
- 規則は A の `baseline.md` §6・§7。生の出力は run の artifacts の `raw/`（`measure-base.txt`・`measure-head.txt`・`cycle-edges-*.json`・
  `ssr-*.json`・`health-*.json`・`c7b.txt`・`checks.txt`）。

## (1) import・依存・循環の中の辺（C1・C2・C7）

| 指標 | before | after | 差 |
|---|---|---|---|
| 循環の件数（C2、repo-health の `deps.cycle` の finding も同じ） | 17 | 17 | 0 |
| 循環の単位・組の合計 | 198・756 | 198・756 | 0・0 |
| 循環の行の辺の合計 | 1,815 | 1,821 | +6 |
| task-api の SCC の単位・組・行の辺 | 49・261・620 | 49・261・622 | 0・0・+2 |
| task-core の SCC の単位・組・行の辺 | 55・210・617 | 55・210・621 | 0・0・+4 |
| SCC の中の `knowledge` 単位の辺 | 出 10、入 18（cos 5） | 出 10、入 20（cos 7） | 入 +2 |
| SCC の中の `lifecycle` 単位の辺 | 出 5、入 9 | 出 5、入 9 | 0（1 本消えて 1 本足された） |
| Cargo の依存（`Cargo.toml`・`Cargo.lock` の差） | 49 辺（A の値） | 差 0 | 0 |

行の辺の差（cycle-edges の JSON を `(src, src_unit, dst_unit, 本文)` で突き合わせた）:

| | src | 辺 | 本文 |
|---|---|---|---|
| 消えた | `task-api/src/cos/ops/decisions.rs` | cos → knowledge | `crate::knowledge::record_op(…)` |
| 消えた | `task-api/src/cos/ops/projects.rs` | cos → lifecycle | `audited(crate::lifecycle::project_action_op(` |
| 足された | `task-api/src/cos/ops/decisions.rs` | cos → knowledge | `record_candidate`・`pin_candidate_tx`・`discard_candidate` の 3 行 |
| 足された | `task-api/src/cos/ops/projects.rs` | cos → lifecycle | `crate::lifecycle::project_trigger(action)` |
| 足された | `task-core/src/chat/attachments.rs` | chat → store | `attachment_provenance` の `crate::store::SqliteStore`・`StoreError` の 4 行 |

- 新しい組（単位の対）は 0。足された 6 本はどれも既存の組（cos → knowledge、cos → lifecycle、chat → store）の中にある。
  B の `deps.cycle` は新しい組で落ち、既存の組の中の行の増加では落ちない（health は base・head とも 17 件）。
- 行の辺の +6 は悪化として数える。CoS の腕が旧 `record_op`・`project_action_op` の 1 呼び出しから、共有の関数を 3 つ・1 つ直接呼ぶ形
  （D の付記の org_write の形）になったため。task-core の +4 は store の読み取りの接続を通すために chat が store の型を名指しする分。

## (2) SQLite を直接参照する実装 file（C4）と 3 file の参照

| 指標 | before | after | 差 |
|---|---|---|---|
| `rusqlite` を書く production file（task-core/src/store/ を除く） | 75（task-api 17、task-ops 8） | 75（task-api 17、task-ops 8） | 0 |
| SQL 文字列を持つ production file（同じ除外） | 50 | 50 | 0 |
| `knowledge.rs` の `rusqlite` の出現 | 8 | 1 | −7 |
| task-ops の `lifecycle.rs` の `rusqlite` の出現 | 2 | 1 | −1 |
| task-api の `lifecycle.rs` の `rusqlite` の出現 | 0 | 0 | 0 |
| `knowledge.rs` の `rusqlite::Connection` | 4 | 0 | −4 |
| `knowledge.rs` の自前の接続の行（Z1） | 10 | 0 | −10 |
| task-ops の `lifecycle.rs` の Connection の closure の行（Z2） | 3 | 0 | −3 |
| crates 全体で `apply_project_action`・`project_action_op`・`record_op`・`open_attachment_index` を書く file | 7 | 0 | −7 |
| `write_tx` を呼ぶ code の行（task-ops `lifecycle.rs`・task-api `knowledge.rs`・task-api `lifecycle.rs`） | 1・0・0 | 0・1・1 | transaction を開く所が task-ops から HTTP adapter へ移った |

- Z1 は `Connection::open|open_with_flags|OpenFlags|transaction_with_behavior|TransactionBehavior|busy_timeout|open_attachment_index|record_op` の行。
  before の 10 行は `knowledge.rs` の 654・661・663・714・751・813・874・875・878・922。after は 0 行（`git grep` が exit 1）。
- Z2 は `Fn(Once|Mut)?(&Connection|&Transaction)|apply_project_action|also: F` の行。before の 3 行は 102・107・110。after は 0 行（exit 1）。
- after に残る `rusqlite` の 2 行: `knowledge.rs:772`（`pin_candidate_tx` の `tx: &rusqlite::Transaction<'_>`）、task-ops `lifecycle.rs:398`
  （`apply_project_change_tx` の `tx: &rusqlite::Transaction<'_>`）。どちらも呼び手が `write_tx` か `OperationAudit::apply`・`apply_checked` で開いた transaction を受ける引数で、
  接続を開かない。C4 は `rusqlite` の語を 1 回でも書く file を数えるので、この 2 file は after でも数に入る。
- C4 の file の数は減っていない。減ったのは file の中の自前の接続と closure の行で、上の Z1・Z2 の行で示す。

## (3) knowledge・lifecycle と cos の相互参照、SQL の公開面（C2・C5・C6）

| 指標 | before | after | 差 |
|---|---|---|---|
| C5 `crate::cos::` を書く handlers の行 | 19（12 file） | 19（12 file） | 0 |
| C5 `(crate\|super(::super)*)::handlers` を書く cos の行 | 50（9 file） | 50（9 file） | 0 |
| C2 `handlers → cos`・`cos → handlers` の辺 | 19・50 | 19・50 | 0 |
| C2 cos → knowledge・cos → lifecycle の行の辺 | 5・7 | 7・7 | +2・0 |
| C6 `&Connection`・`&Transaction` の引数の行（§7 の command のまま） | 347（task-ops 7） | 347（task-ops 7） | 0 |
| C6 同じ（path だけに除外を当て直した値） | 336 | 336 | 0 |
| C6 `Fn(&Connection)`・`Fn(&Transaction)` の引数の行 | 22（task-ops 5） | 21（task-ops 4） | −1 |
| C6 `impl SqliteStore` の block・file・path 付き・trait の impl | 102・88・105・6 | 102・88・105・6 | 0 |
| C6 `SqliteStore` の method（snippet）`pub`・`pub(crate..)`・private | 631・67・41 | 631・67・41 | 0 |
| C6 同じ（path 付きの block も数える版） | 634・67・41 | 634・67・41 | 0 |
| task-core の `&SqliteStore` を第 1 引数に取る `pub` の自由関数（Z14、A に無い数え方） | 7 | 8 | **+1（増加）** |
| crate の外へ `&Connection` を渡す公開の関数（`apply_project_action` の closure、`provenance_for_owner`） | 2 | 0 | −2 |
| task-ops `lifecycle.rs` の `pub fn` | 7 | 7 | 0（1 消して 1 足した） |
| task-api `knowledge.rs`・`lifecycle.rs` の `pub(crate) fn` | 7・4 | 9・3 | +2・−1 |
| D の付記の狭い形・広い形の command（task-api の file 数） | 14・18 | 14・18 | 0・0 |
| `Option<&OperationAudit>` を取る関数（`knowledge.rs`・`lifecycle.rs`） | `record_op`・`accept_op`・`reject_op`／`project_action_op`・`task_pause_op` | `accept_op`・`reject_op`／`task_pause_op` | −1・−1 |

SqliteStore の公開面の数え方（A と違う数え方。両方の値を書く）:
- A の §7 C6 の snippet（`impl SqliteStore` の block の中の `pub` の method）は 631 → 631 で変わらない。
  ADR の D2 は `attachment_provenance` を `SqliteStore` の method にして 631 → 632 と見込んだが、WorkUnit provenance-read は
  `size.responsibility-growth`（`chat::SqliteStore` の method 114 → 115 で fail）を避けるため、`store: &SqliteStore` を受ける `pub fn`
  （`task_core::chat::attachments::attachment_provenance`）にした（[provenance-read.md](provenance-read.md)）。そのため snippet には現れない。
- store の公開面としては 1 増えたので、増加として数える（Z14 の 7 → 8、snippet の値に足すと 632 相当）。公開面の縮小には数えない。
- 縮小に数えるのは次の 2 つだけ: `task_core::chat::attachments::provenance_for_owner`（`conn: &Connection` を取る）の `pub` の撤去と、
  `task_ops::lifecycle::apply_project_action` の `FnOnce(&rusqlite::Connection)` の closure の撤去。
- `apply_project_change_tx` の引数は `&rusqlite::Connection` → `&rusqlite::Transaction<'_>`（transaction の外の接続で呼べない）。C6 の正規表現は
  `Connection` と `Transaction` の両方に当たるので 347 は変わらない。縮小には数えない。
- D の付記の 2 本の file 数が変わらないのは、`knowledge.rs` に `accept_op`・`reject_op`、`lifecycle.rs` に `task_pause_op` が残るから（adr.md の見込みのとおり）。
  F が移したのは `record_op` と `project_action_op` の 2 関数で、関数の単位では 5 → 3。

`project_controller_human_bump_tx` の呼び手 file（`git grep -l … -- crates`）:

| | before | after |
|---|---|---|
| 定義 | `task-core/src/store/project_wake.rs` | 同じ |
| production の呼び手 | `task-api/src/human_epoch.rs`、`task-api/src/handlers/authority.rs`、`task-core/src/store/authority.rs` | 同じ |
| 試験 | `task-dispatch/src/dispatcher/tests/` の `long_horizon_bench.rs`・`long_horizon_pilot.rs`・`project_achievement.rs`・`project_recovery.rs`・`project_wake.rs` | 同じ |
| 合計・task-ops/src | 9 file・0 | 9 file・0（`diff` の差 0） |

- HTTP の案件の操作の加算は、after では `task-api/src/lifecycle.rs:82` の `write_tx` の中の `human.bump_tx`（`human_epoch.rs:47` が定義を呼ぶ）の 1 か所。
  CoS の `project.*` の腕（`cos/ops/projects.rs`）の `HumanEpoch` の出現は 1 → 0（加算しない、は before と同じ）。

新しい trait・Port（Z6）:
- `git diff $B $H -- crates` の追加行と削除行に `trait <大文字>`・`Port`・`impl … for …` の行は 0（どちらも `grep` が exit 1）。
- crates の trait の定義の行 119 → 119、`…Port` の語 6 → 6。TaskStore のような trait も feature ごとの Port も足していない。

## (4) 変更波及（`git diff --stat $B $H`、C7b、test-affected の `--list`）

| 区分 | file | 単位（repo-health の単位） | crate |
|---|---|---|---|
| production | 7（`cos/ops/decisions.rs`・`cos/ops/projects.rs`・`human_epoch.rs`・`knowledge.rs`・task-api `lifecycle.rs`・task-core `chat/attachments.rs`・task-ops `lifecycle.rs`） | 6（task-api の cos・human_epoch・knowledge・lifecycle、task-core の chat、task-ops の lifecycle） | 3（task-api・task-core・task-ops） |
| 試験 | 4（`cos_delivery_suite/main.rs`・`tx_boundary_contract.rs`・dispatcher `tests/project_wake.rs`・task-ops `lifecycle/tests.rs`） | — | 3（task-api・task-dispatch・task-ops） |
| 文書 | 10（ADR 4〔新 1・付記 3〕、進捗 6） | — | — |

C7b（変える単位を import する単位。A の §7 の snippet を `$B` と `$H` に当てた）:

| 単位の組 | before direct・transitive・packages | after |
|---|---|---|
| F の production の 6 単位 | 35・363・12 | 35・363・12 |
| task-ops の lifecycle だけ | 3・58・3 | 3・58・3 |
| task-api の knowledge だけ | 5・57・3 | 5・57・3 |
| task-core の chat だけ | 27・368・12 | 27・368・12 |
| 参考: A の S1（handlers・cos・store） | 94・366・12 | 94・366・12 |
| 参考: A の S2（dispatcher・run_settlement） | 6・14・4 | 6・14・4 |

- 逆 import は変わらない。F は単位の間の依存の組を足しも消しもしていない（(1) のとおり）。
- S1 の before が A の値（bc665168 で 92・369・12）と違うのは D・E の統合の分。S1・S2 の after の選び直しは子 G の仕事で、ここでは同じ単位を当てただけ。
- F 全体の差分の `test-affected --list`（`CELERIS_WU_BASE=$B`）: 14 crate（celeris、celeris-campaign、celeris-credentiald、celeris-mcp、celeris-script、celerisctl、
  e2e、llm-proxy、routing-eval、task-api、task-core、task-dispatch、task-ops、task-worker）＋ web の typecheck+unit ＋ test-ts-verify。
  task-core の `chat/attachments.rs` を変えたので、task-core の逆依存の全部が対象になる。task-api・task-ops だけの unit（lifecycle-tx）は 9 crate ＋ web だった（lifecycle-tx.md）。

## (5) 時間と費用 — 未達/要判断

| 指標 | before | after | 判定 |
|---|---|---|---|
| `repo_health.py --base X --head X` の全 rule（A の §7 C1） | 1 回目 wall 1:45.59、user 71.44 s、sys 11.39 s、RSS 643 MB（開始時の 1 分 load 0.78）。2 回目（再現 command の再実行）wall 1:41.10、user 71.36 s | 1 回目 wall 1:42.95、user 71.69 s、sys 11.31 s、RSS 644 MB（同 1.04）。2 回目 wall 1:37.18、user 71.72 s | 2 回ずつで、A の同条件 3 回に足りないので比較しない。未達/要判断 |
| repo-health の finding の数（rule ごと） | deps.cycle 17、size.responsibility-growth 94、dup.contract 210、arch-map.wrong_owner 31 ほか | 全 rule で同じ | 差 0 |
| incremental build（cold・warm の check・test）・test-affected の時間の 3 回比較 | 測っていない | 測っていない | 未達/要判断（子 G が A の §5 の 2 シナリオで同条件 3 回を測る） |
| review の時間と費用 | 測っていない | 測っていない | 未達/要判断（この WorkUnit は LLM を呼ばない。子 G・H の範囲） |

- 理由: 依頼の (5) は同条件 3 回の比較を求める。F の objective は build・test-affected の 3 回比較を子 G に回すと決めている。
  repo_health の wall の差（1 回目 −2.6 s、2 回目 −3.9 s）は user time がほぼ同じ（差 0.4 s 以下）で、揺れと区別できないので、改善とも悪化とも書かない。
- 参考（比較ではない）: impl の 3 unit の test-affected は各 unit の差分に対して 769 s（knowledge-pin）、14 m 40 s（lifecycle-tx）、16 m 20 s（provenance-read）。
  対象の crate が違うので互いに比べられない。
- 次の候補: 子 G が F の HEAD と `git merge-base HEAD main` で、A の §7 C8・C9 の手順（waitload を含む）を各 3 回当てる。

## (6) 行数（source-size-report、`$B` の script と toml を `git show` で取り出して `$B`・`$H` に当てた）

| 指標 | before | after | 差 |
|---|---|---|---|
| `views.product_only.lines`（production の全言語） | 375,818 | 375,781 | −37 |
| `views.product_only.rust_lines`（production の Rust） | 331,127 | 331,090 | −37 |
| production の Rust 以外（上の 2 つの差） | 44,691 | 44,691 | 0 |
| `views.own_total_with_tests.lines` | 1,030,359 | 1,031,163 | +804 |
| `views.own_total_with_tests.rust_lines` | 883,284 | 884,088 | +804 |
| `class_totals.external_test` | 459,062 | 459,903 | +841 |
| `class_totals.inline_test` | 21,408 | 21,408 | 0 |

file ごと（`git diff --numstat $B $H -- crates`、追加・削除）:

| file | 追加 | 削除 | 純 |
|---|---|---|---|
| `task-api/src/knowledge.rs` | 65 | 100 | −35 |
| `task-api/src/lifecycle.rs` | 22 | 54 | −32 |
| `task-ops/src/lifecycle.rs` | 12 | 25 | −13 |
| `task-api/src/cos/ops/decisions.rs` | 22 | 4 | +18 |
| `task-api/src/cos/ops/projects.rs` | 15 | 8 | +7 |
| `task-core/src/chat/attachments.rs` | 18 | 1 | +17 |
| `task-api/src/human_epoch.rs` | 2 | 1 | +1 |
| production の計 | 156 | 193 | −37 |
| `task-api/tests/cos_delivery_suite/tx_boundary_contract.rs`（契約・反例試験 8 件） | 839 | 0 | +839 |
| `task-api/tests/cos_delivery_suite/main.rs` | 1 | 0 | +1 |
| `task-dispatch/src/dispatcher/tests/project_wake.rs` | 8 | 10 | −2 |
| `task-ops/src/lifecycle/tests.rs` | 4 | 1 | +3 |
| 試験の計 | 852 | 11 | +841 |

- 3 file の行数: `knowledge.rs` 1,122 → 1,087、task-ops `lifecycle.rs` 459 → 446、task-api `lifecycle.rs` 268 → 236。
- 行数の純減は F の受け入れ条件に無い（変更の規約 11、ADR §7）。production の −37 は成果の主張ではなく増減の記録。

## 消したもの（path と関数・型）

| path | 消したもの |
|---|---|
| `crates/task-api/src/knowledge.rs` | `open_attachment_index`（添付の DB の file を読み取り専用で開き busy_timeout 10 秒を置く関数） |
| `crates/task-api/src/knowledge.rs` | `record_op`（`attachment_db: Option<&Path>`・`audit: Option<&OperationAudit>` を受け `Applied` を返す）と、その中の `rusqlite::Connection::open`・busy_timeout 10 秒・`TransactionBehavior::Immediate` の自前の transaction |
| `crates/task-api/src/knowledge.rs` | `candidate_provenance` の `conn: Option<&rusqlite::Connection>` の引数 |
| `crates/task-api/src/lifecycle.rs` | `project_action_op`（`Option<&OperationAudit>` を受け `Applied` を返す）、`project_action_name` |
| `crates/task-ops/src/lifecycle.rs` | `apply_project_action<F>`（`also: F`、`F: FnOnce(&rusqlite::Connection) -> Result<(), StoreError>` と、その中の `store.write_tx`） |
| `crates/task-core/src/chat/attachments.rs` | `provenance_for_owner` の `pub`（private の関数として残る） |
| `crates/task-api/src/cos/ops/projects.rs` | `project.*` の腕の `audited(..)` の包みと `HumanEpoch::NotCounted` の受け渡し |

## 足したもの・変えたもの

| path | 足したもの・変えたもの | 公開面 |
|---|---|---|
| `crates/task-core/src/chat/attachments.rs` | `attachment_provenance(store: &SqliteStore, owner_kind, owner_id) -> Result<Vec<AttachmentProvenance>, StoreError>`（`with_read_conn` で読む） | crate の外へ `pub`、増加 +1。`SqliteStore` の method ではない |
| `crates/task-ops/src/lifecycle.rs` | `project_action_result(store, id, change, cancelled) -> Result<ProjectLifecycle, OpsError>`（旧 `apply_project_action` の後半。書いた後に読むだけ） | crate の外へ `pub`、+1（`apply_project_action` の −1 と相殺） |
| `crates/task-ops/src/lifecycle.rs` | `apply_project_change_tx` の `tx` を `&rusqlite::Connection` → `&rusqlite::Transaction<'_>` | 数は同じ |
| `crates/task-api/src/knowledge.rs` | `record_candidate`・`discard_candidate`（`pub(crate)`）、`pin_candidate_tx` を private → `pub(crate)` | crate の中、+3 −1 |
| `crates/task-api/src/lifecycle.rs` | `project_trigger` を private → `pub(crate)` | crate の中、+1 −2 |
| `crates/task-api/src/knowledge.rs:741`・`lifecycle.rs:82` | HTTP adapter の `store.write_tx`（knowledge の pin、案件の操作と `HumanEpoch::bump_tx`） | — |
| `crates/task-api/tests/cos_delivery_suite/tx_boundary_contract.rs` | 契約・反例試験 8 件（WorkUnit contract-tests、旧コードで固定） | 試験 |

- 新しい trait・Port・flag・fallback・状態は無い（(3) の Z6）。旧新の並存も無い（旧経路を同じ変更で消した）ので、撤去の条件と期限は要らない。

## 再現 command（repo の root で）

```sh
B=$(git merge-base HEAD main); H=$(git rev-parse HEAD)
# 1. adr.md の before の block（C2・C4・C5・C6・3 file の rusqlite・bump_tx の呼び手・D の付記の 2 本・source-size-report）を
#    B と H の両方に当てる。script と toml は block の中で $B から取り出すので、H の回だけ取り出し元を before の基準に固定する。
awk '/^```sh$/{f=1;next} /^```$/{if(f){exit}} f' agent-docs/progress/2026-11-18-arch-v4-f/adr.md \
  | sed -e 's/^B=\$(git merge-base HEAD main)$/B=${REV:?}/' -e 's#mkdir -p "\$TMPDIR"#export TMPDIR="$OUTD"; mkdir -p "$TMPDIR"#' \
        -e 's#git show \$B:scripts/dev/source-size-report#git show $SSRBASE:scripts/dev/source-size-report#g' > $TMPDIR/rev-commands.sh
REV=$B SSRBASE=$B OUTD=$TMPDIR/m-base bash $TMPDIR/rev-commands.sh > $TMPDIR/measure-base.txt 2>&1
REV=$H SSRBASE=$B OUTD=$TMPDIR/m-head bash $TMPDIR/rev-commands.sh > $TMPDIR/measure-head.txt 2>&1
diff $TMPDIR/measure-base.txt $TMPDIR/measure-head.txt
# 2. 循環の行の辺の差
python3 -I - $TMPDIR/m-base/cycle-edges.json $TMPDIR/m-head/cycle-edges.json <<'EOF'
import json,sys,collections
def load(p):
    s=collections.Counter()
    for c in json.load(open(p))['cycles']:
        for e in c['edges']: s[(e['src'],e['src_unit'],e['dst_unit'],e['excerpt'].strip())]+=1
    return s
b=load(sys.argv[1]); h=load(sys.argv[2])
print('removed'); [print(' ',k,v) for k,v in (b-h).items()]
print('added'); [print(' ',k,v) for k,v in (h-b).items()]
EOF
# 3. C7b（A の §7 の snippet の写し。引数に F の 6 単位と S1・S2 を渡す。約 20 s × 2）
cat > $TMPDIR/c7b.py <<'EOF'
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
F=F:crates/task-api/src/cos,crates/task-api/src/human_epoch,crates/task-api/src/knowledge,crates/task-api/src/lifecycle,crates/task-core/src/chat,crates/task-ops/src/lifecycle
for r in $B $H; do python3 $TMPDIR/c7b.py $r $F S1:crates/task-api/src/handlers,crates/task-api/src/cos,crates/task-core/src/store \
  S2:crates/task-dispatch/src/dispatcher,crates/task-dispatch/src/run_settlement; done
# 4. 変更波及と行数
git diff --stat $B $H; git diff --numstat $B $H -- crates
CELERIS_WU_BASE=$B bash scripts/dev/test-affected.sh --list
# 5. repo_health の全 rule の時間（A の §7 C1。1 回ずつ）
for r in $B $H; do /usr/bin/time -v python3 scripts/dev/repo-health/repo_health.py --repo . --base $r --head $r --policy-version 1 --out $TMPDIR/health-$r.json 2>&1 >/dev/null | grep -E 'Elapsed|User time|System time|Maximum resident'; done
# 6. 自前の接続・closure・旧関数（Z1・Z2・Z4）
for r in $B $H; do
  git show $r:crates/task-api/src/knowledge.rs | grep -cE 'Connection::open|open_with_flags|OpenFlags|transaction_with_behavior|TransactionBehavior|busy_timeout|open_attachment_index|record_op\b'
  git show $r:crates/task-ops/src/lifecycle.rs | grep -cE 'Fn(Once|Mut)?\(&(mut )?(rusqlite::)?(Connection|Transaction)|apply_project_action\b|also: F'
  git grep -lwE 'apply_project_action|project_action_op|record_op|open_attachment_index' $r -- crates | wc -l
done
# 7. 新しい trait・Port（Z6）。追加行・削除行とも出力なし（exit 1）
git diff $B $H -- crates | grep -E '^\+' | grep -vE '^\+\+\+' | grep -nE '\btrait\s+[A-Z]|\bPort\b|impl\s+[A-Za-z_:<>]+\s+for\s'
git diff $B $H -- crates | grep -E '^-' | grep -vE '^---' | grep -nE '\btrait\s+[A-Z]|\bPort\b|impl\s+[A-Za-z_:<>]+\s+for\s'
for r in $B $H; do git grep -nE '^\s*(pub(\([a-z]+\))? )?trait [A-Z]' $r -- 'crates/*.rs' | wc -l; git grep -nwE '[A-Za-z]*Port' $r -- 'crates/*.rs' | wc -l; done
# 8. task-core の &SqliteStore を第 1 引数に取る pub の自由関数（Z14）
for r in $B $H; do n=0; for f in $(git ls-tree -r --name-only $r -- crates/task-core/src | grep -E '\.rs$' | grep -vE '(^|/)(tests?)(/|\.rs$)|_tests?\.rs$'); do
  c=$(git show $r:$f | perl -0777 -ne '$c=0; while(/^pub fn \w+(?:<[^>]*>)?\(\s*\w+: &(?:crate::store::|crate::|task_core::)?SqliteStore\b/mg){$c++} print $c'); n=$((n+c)); done; echo $n; done
# 9. 関数の行の差（production）と project_controller_human_bump_tx の呼び手の差
git diff -U0 $B $H -- crates/task-api/src crates/task-core/src crates/task-ops/src ':!*/tests.rs' ':!*/tests/*' | grep -E '^(\+\+\+|[-+]\s*(pub(\([a-z]+\))? )?(async )?fn )'
diff <(git grep -l project_controller_human_bump_tx $B -- crates | sed "s/^$B://") <(git grep -l project_controller_human_bump_tx $H -- crates | sed "s/^$H://") && echo same
git grep -n project_controller_human_bump_tx $H -- crates/task-ops/src; echo "rc=$?"   # rc=1（0 件）
```

- 1 の `SSRBASE` の置換は、adr.md の block が `git show $B:scripts/dev/source-size-report.*` と書くのを、H の回でも before の基準から取り出すためのもの。
  今回 `$B` と `$H` の間で `scripts/dev/source-size-report.py`・`.toml` は変わっていない（`git diff --stat` に無い）ので、置換の有無で値は同じ。
- 1 の C2 は約 30 s、1 は 1 回あたり約 36〜39 s。5 は 1 回あたり約 1 分 40 秒。block 全体は約 5 分。

## A と違う数え方（理由と両方の値）

1. SqliteStore の公開面: A の snippet の値（631 → 631）と、`&SqliteStore` を取る task-core の `pub` の自由関数（Z14、7 → 8）を並べた。
   理由: `attachment_provenance` が method ではなく自由関数になり、snippet が拾わないため。公開面の増加 +1 として数える。
2. Z1・Z2・Z4・Z14 と関数の単位の数（`Option<&OperationAudit>` を取る関数、`pub fn`・`pub(crate) fn` の数）は A の §7 に無い。
   理由: A の C4・C6 は file と引数の正規表現の数で、file の中の自前の接続や closure の有無を区別しない。F の objective が 0 件を示すことを求めるので足した。
   A の command の値（C4 75・50、C6 347・336・22、snippet 631）は上の表にそのまま並べた。
3. C7b は A の S1・S2 に加えて F の 6 単位にも当てた。理由: F の変更波及を F が変えた単位で見るため。S1・S2 の値も並べた。

## 未達/要判断

- (5) の build・test-affected の 3 回比較と review の費用: 測っていない（子 G の範囲）。repo_health の全 rule の時間は 2 回ずつで、比較の材料にしない。
- (2) の `rusqlite` を書く file の数（75）と (3) の `&Connection`・`&Transaction` の引数の行（347）は減っていない。
  残る 2 行は `write_tx`・監査の transaction から渡される `&Transaction` の引数で、A の正規表現では接続を開く行と区別できない。
- (1) の循環の中の行の辺は +6（新しい組は 0）。D の付記の形へ移した CoS の腕が共有の関数を直接呼ぶ分と、task-core の chat が store の型を名指しする分。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

| 受け入れ条件 | command | 結果 |
|---|---|---|
| 0 | 上の「再現 command」の block を `awk` で取り出して `bash` でそのまま実行 | exit 0（約 5 分）。1 の出力は最初の計測と base・head とも一致（`diff` の差 0）。他の値も上の表と一致 |
| 1 | 再現 command の 6（Z1・Z2・Z4）と `git grep -nE '<Z1 の式>' HEAD -- crates/task-api/src/knowledge.rs`・`git grep -nE '<Z2 の式>' HEAD -- crates/task-ops/src/lifecycle.rs crates/task-ops/src/lifecycle` | before 10・3・7 → after 0・0・0。HEAD の 2 本の `git grep` は出力なし（exit 1） |
| 2 | (5) と「未達/要判断」の節 | build・test-affected の 3 回比較と review の費用を未達/要判断と書いた |
| 3 | 再現 command の 1（C6 の snippet）・8（Z14）・9 | snippet 631 → 631、Z14 7 → 8（増加 +1）、`project_controller_human_bump_tx` の呼び手 file 9 → 9（`same`）、task-ops/src 0 件（rc=1） |
| — | `git diff --quiet $CELERIS_WU_BASE HEAD -- crates web` | exit 0（`crates/`・`web/` の差 0） |
| — | `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` | exit 0・0・0 |
| — | `sh "$CELERIS_WU_SCOPE_PATHS"` | この file の 1 path だけ |
| — | `cargo fmt --all -- --check` | exit 0 |
| — | `bash scripts/dev/test-affected.sh` | exit 0（約 2 分 20 秒。「試験対象の変更なし（docs のみ等）」、repo-health verdict=pass、new=0・worsened=0） |
| — | `cargo clippy --workspace -- -D warnings` | exit 0（約 48 秒） |

リリースゲート: 未実施（統合の後の release の gate で走る）。本番への昇格はしていない。

回帰条件: この WorkUnit は文書だけを足した。値が後の変更で変わったら、上の再現 command を新しい `git merge-base HEAD main` と HEAD に当て直す。

## 未解決

- 無し。

## 提案

- close の unit が F の ADR の D2 の「`SqliteStore` の `pub` method は 1 増える（631 → 632）」に付記を足し、実装の形（自由関数、snippet の値は 631 のまま、
  Z14 の 7 → 8）を書く（provenance-read.md の提案と同じ）。
- 子 G は (5) を測るとき、F の変更が task-core（`chat/attachments.rs`）を含み test-affected の対象が 14 crate になることを前提にする。
