---
title: アーキ v4 D usecase — org の plan と tx 内の適用を org_write.rs に集約し、HTTP と CoS の adapter から一方向に呼ぶ
tasks: [01M7QFK5QKTMKCK57NKH36HFYX, 01M7MZTKB4RYM80S6CT3RST2N9]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# usecase（WorkUnit org-write・verify）

完了日: 2026-11-18

[ADR 2026-11-18-org-write-shared-use-case](../../adr/2026-11-18-org-write-shared-use-case.md) の D1〜D4 と付記「2 つの形の使い分け」を実装し（WorkUnit org-write）、
統合後の HEAD で確かめた（WorkUnit verify。コードは変えていない）。
基準: C5・repo health の before は `git merge-base HEAD main`、範囲の check は `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## commit（件名）
- `arch-v4 D usecase: org の plan と tx 内の apply を org_write.rs に集約し、HTTP と CoS の adapter から一方向に呼ぶ（org_commit を消す）`
- `arch-v4 D usecase: 進捗に Box 化の後の再検査の結果を書く`
- `integrate wu/org-write (phase impl)`
- `arch-v4 D usecase verify: 統合後の HEAD で契約試験・C5・repo health を確かめ usecase.md を仕上げる`

## 変更
- 新しい file `crates/task-api/src/org_write.rs`（325 行）と `lib.rs` の `mod org_write;`。
  - 依存は `task_core`・`crate::problem`・`crate::types` と外部 crate（axum::http・rusqlite・schemars・serde・serde_json・time）だけ。
    state・middleware・schema・handlers・cos・skills・`super::` は本文にも comment にも無い（verify で grep して出力なし）。
  - 足した定義: `plan_delete(store, id)`（`load_org_node` で 404 → `OrgWrite::Delete`）、`OrgWrite::target_id()`、
    `OrgWritten { Node(Box<OrgNode>), Deleted { id, existed } }`、`OrgWritten::into_node()`（HTTP の upsert の route が stored node を取り出す。
    `Deleted` は internal）、`apply(tx, write, actor)`（`org_upsert_tx(tx, node, browser.then_some(actor))`・`org_delete_tx`。tx を開かず commit しない）。
- `handlers/org.rs`（445 → 149 行）: 4 route は `no_query` → `require_admin` → `read_json` の後、`state.blocking` の中で
  `org_write::plan_*` → `store.write_tx(|tx| org_write::apply(tx, &write, "admin"))`。status・body・`tracing::info!` は今と同じ。
  delete は `existed` によらず 204。`crate::cos`・`OperationAudit`・`Applied` の参照は無い。
- `skills.rs`（455 → 456 行）: mount・unmount の route は同じ形。`plan_skill_mount` は `org_write::OrgWrite` を返し、`org_write::load_org_node` を使う。
  `put_skill_op`・`delete_skill_op` の `Applied` は別の領域なので触っていない。
- `cos/ops/admin.rs`（739 → 738 行。`dispatch` 関数は 3 行減）: `org.*` の腕は `org_write::plan_*` と `skills::plan_skill_mount` で plan し、
  失敗は `audit.reject` のまま。`audit.apply_checked` の closure で `org_write::apply(tx, &write, "cos")` を呼び、`Node` → node の JSON、
  `Deleted { existed: true }` → `{"id", "deleted": true}`、`existed: false` → `Err(org_node_not_found)`。`CosOperation` をそのまま返す（`audited` を通さない）。
- `handlers.rs`（420 → 412 行）、`schema.rs`（`crate::org_write::BrowserSettingsPatch` を参照する）。
- `docs/architecture-map.md`: 「組織・案件・報告」の行に `crates/task-api/src/org_write.rs`・ADR の link・`org_write_contract.rs` を足した。

## 消した関数と移した定義
- 消した関数: `handlers::org::org_commit`（direct と audited の 2 分岐）と、その中の `crate::cos::operations::{OperationAudit, Applied}` の参照。
  org での `Applied::Direct(Some(..))`・`Applied::Audited`・`.direct()`・`audited(org::org_commit(..))` と `Option<&OperationAudit>` の引数も消えた。
- 移した定義（写していない。handlers に定義も re-export も無い）:
  - `handlers/org.rs` から: `OrgWrite`・`plan_create`・`plan_patch`・`plan_browser_settings`・補助の `validate_genre`・`validate_profile_body`・
    `set_credential_use`・`parse_browser_settings_json`・`BrowserSettingsPatch`。
  - `handlers.rs` から: `load_org_node`（と、使われなくなった import `OrgNode`・`SqliteStore`）。
- `cos/ops/admin.rs` の inline の削除 plan（`use crate::handlers::org;` と `crate::handlers::load_org_node(store, &id)?` → `OrgWrite::Delete`）を消し、
  `org_write::plan_delete` を呼ぶ形にした。
- `skills.rs` の `crate::handlers::org::org_commit(.., None)?.direct()`（2 か所）・`crate::handlers::org::OrgWrite`（2 か所）・`handlers::load_org_node` の import を消した。
- verify で確かめた（HEAD）: `git grep -nE 'handlers::org|handlers::load_org_node|org_commit' -- crates/task-api/src` → 出力なし。
  `git grep -nE '(fn load_org_node|struct BrowserSettingsPatch|enum OrgWrite\b|pub use .*(OrgWrite|BrowserSettingsPatch|load_org_node))' -- crates`
  → `org_write.rs` の 3 行だけ（23・39・95 行目）。`grep -n cos crates/task-api/src/handlers/org.rs` → 出力なし。

## ADR からの差
- `OrgWritten::Node` は `Box<OrgNode>`（ADR は `Node(OrgNode)`）。`clippy::large_enum_variant`（608 byte 対 25 byte）で `-D warnings` が落ちたため。
  `OrgWrite::Upsert` の `node: Box<OrgNode>` と同じ扱い。
- `OrgWritten::into_node()` を足した。HTTP の 5 route（org.rs 3・skills.rs 2）が同じ写像を使うので、各 adapter に写さない（規約 2）。

## C5 の before → after
式（`<rev>` は `git merge-base HEAD main` と HEAD）:
- handlers → cos: `git grep -nE 'crate::cos::' <rev> -- crates/task-api/src/handlers crates/task-api/src/handlers.rs | wc -l`
- cos → handlers: `git grep -nE '(crate|super(::super)*)::handlers' <rev> -- crates/task-api/src/cos | wc -l`
- 除外ありの数え方は、上の `wc -l` の前に `grep -vE "$X"` を挟む（`$X` は A の baseline.md §7 C4 の除外の式
  `(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$`）。adr.md の before（22 行・52 行）はこの数え方の値。

| 指標 | 数え方 | before（merge-base） | after（HEAD） |
|---|---|---|---|
| `crate::cos::` を書く handlers の行 | 除外なし | 22 行（13 file） | 19 行（12 file） |
| 同上 | 除外あり（A の §7 C4） | 22 行 | 19 行 |
| `(crate\|super(::super)*)::handlers` を書く cos の行 | 除外なし | 52 行（9 file、うち `super::` 4） | 50 行（9 file、うち `super::` 4） |
| 同上 | 除外あり（A の §7 C4） | 52 行 | 50 行 |

- 2 つの数え方は同じ値になった。対象の path（handlers/・handlers.rs・cos/）に除外の式に当たる試験の file が無いため。
- 消えた行（`git grep` の結果の diff）: handlers → cos は `handlers/org.rs` の 3 行（`audit: Option<&crate::cos::operations::OperationAudit>`・
  `-> Result<crate::cos::operations::Applied<..>>`・`use crate::cos::operations::Applied;`）。cos → handlers は `cos/ops/admin.rs` の 2 行
  （`use crate::handlers::org;`・`crate::handlers::load_org_node(store, &id)?;`）。ADR §6 の予想（22 → 19、52 → 50）と一致した。
- 残りの辺（HEAD）: cos → handlers の 50 行は `handlers::accounts` 9・`task_actions` 7・`parse_project_id` 6・`handlers` 単体 6・`authority` 5・
  `read_json` 4・`projects` 3 ほか。handlers → cos の 19 行は `crate::cos::operations` 18・`crate::cos::routes` 1。handlers⇄cos の単位の循環は残る（org 以外の領域の辺）。

## 契約試験（提出時の HEAD で実行）
- `cargo nextest run -p task-api -E 'test(org_write_contract) | test(cos_ops_admin_config) | binary(organization)'` → exit 0、30 試験 pass・0 fail・0 skip
  （filter の外の 842 は対象外）。内訳は `cargo nextest list` で `org_write_contract` 6・`cos_ops_admin_config` 4・`organization` 20。
- `org_write_contract` の 6 件: `org_http_and_cos_responses_keep_their_shape`・`org_refusals_keep_status_code_and_cos_records_the_reason`・
  `org_writes_need_an_admin_principal_and_cos_writes_need_the_cos_run`・`cos_org_writes_commit_row_browser_event_operation_and_audit_together`・
  `cos_org_write_failing_inside_the_transaction_leaves_the_org_unchanged`・`http_browser_write_failing_after_the_row_leaves_the_org_unchanged`。
- 試験は変えていない: `integrate wu/contract-tests` の merge commit から HEAD までの `git diff --name-only -- crates/task-api/tests` → 0 行。
- 生成物と store も変えていない: `git diff --name-only $(git merge-base HEAD main) HEAD -- docs/api/v1/api-v1.schema.json web/api/generated
  crates/task-api/src/cos/operations.rs crates/task-core` → 0 行。

## repo health（base は `git merge-base HEAD main`、head は作業ツリー）
- `python3 scripts/dev/repo-health/repo_health.py --repo "$(pwd)" --base $(git merge-base HEAD main) --head worktree --policy-version 1 --out <json>`
  → exit 0、`verdict=pass`、new=0・worsened=0・existing=697。
- `deps.cycle` → pass（findings 17 はすべて既存）。`size.responsibility-growth` → pass（findings 94 はすべて既存）。
  ほかの必須 rule（deps.violation・arch-map.*・doc.links・adr.numbers・progress.index・human-text・source-size・policy.changed）も pass。
- 行数: `cos/ops/admin.rs` 739 → 738、`handlers/org.rs` 445 → 149（どちらも純増していない）。

## 検査（提出時の HEAD で実行）
- WorkUnit org-write（統合前の作業ツリー）: `bash scripts/dev/test-affected.sh` → exit 0（nextest 6109 試験 pass・21 skip、web 705 試験 pass、doc test pass）。
  `Box` にした後に `cargo nextest run -p task-api` → 870 試験 pass（2 skip）、`bash scripts/dev/check-health.sh` → exit 0。
- WorkUnit verify（統合後の HEAD。この WU の変更は本 file だけ）: 上の契約試験・repo health に加えて次を実行した。
  - `bash scripts/dev/test-affected.sh`（基準 `$CELERIS_WU_BASE`）→ exit 0。「試験対象の変更なし（docs のみ等）」、repo-health verdict=pass（new=0・worsened=0）。
  - `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
  - `bash scripts/dev/check-doc-links.sh` → exit 0（ok）。`bash scripts/dev/check-adr-numbers.sh` → exit 0（458 file）。
    `sh scripts/dev/progress-index.sh --check` → exit 0。

## 未解決（store-prune・metrics への引き継ぎ）
- `store-prune`（親の次の leaf）が消すもの: `crates/task-core/src/store/org.rs` の `SqliteStore::org_upsert_browser_settings`（190 行目）・
  private の `org_upsert_with_actor`（198 行目）・`org_delete`（259 行目）。production の呼び出し元はもう無い
  （残りは `org_upsert_browser_settings` → `org_upsert_with_actor` の 1 行と、`store/tests.rs` 2813〜2825 行の `org_delete` の試験 4 か所。試験は `org_delete_tx` へ移す）。
- 同じく説明の行: `crates/task-core/src/store/org.rs` の doc comment 207 行目（`as org_upsert_browser_settings does`）と 270 行目（`org_delete:` の参照）、
  `docs/api/v1/api.md` 408 行目（`DELETE /org/{id}` の行の「store `org_delete`」）、`docs/ops/browser-department.md` 112 行目
  （`org_upsert_browser_settings` の参照）。`agent-docs/web/feature-parity.md` 21 行目の `org_delete` は web の操作名なので対象外。
- `metrics` が測るもの: C2（task-api の SCC の単位・組・行。予想は行 632 → 622、単位 49・組 261 は不変）・C7b・C12。この WU では測っていない。

## 提案
- 次の pilot の候補: cos → handlers の残り 50 行の最多は `handlers::accounts`（9 行）と `handlers::task_actions`（7 行）。
  handlers⇄cos の単位の循環を解くには、この 2 領域にも org と同じ「plan と tx 内の apply を共有 module に置く」形を当てる必要がある。
- C5 の除外の式は handlers/・cos/ では値を変えない（除外に当たる file が無い）。後の task は除外なしの式だけを書けば足りる。
