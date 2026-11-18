---
title: アーキ v4 D usecase — org の plan と tx 内の適用を org_write.rs に集約し、HTTP と CoS の adapter から一方向に呼ぶ
tasks: [01M7QFK5QKTMKCK57NKH36HFYX, 01M7MZTKB4RYM80S6CT3RST2N9]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# usecase（WorkUnit org-write）

[ADR 2026-11-18-org-write-shared-use-case](../../adr/2026-11-18-org-write-shared-use-case.md) の D1〜D4 と付記「2 つの形の使い分け」を実装した。
基準は `$CELERIS_WU_BASE`（C5 の before は `git merge-base HEAD main`）。検査は提出時の HEAD の作業ツリーで実行した。

## 変更
- 新しい file `crates/task-api/src/org_write.rs`（325 行）と `lib.rs` の `mod org_write;`。
  - 移した定義（handlers に定義も re-export も残していない）: `BrowserSettingsPatch`・`OrgWrite`・`plan_create`・`plan_patch`・
    `plan_browser_settings`・private の `parse_browser_settings_json`・`set_credential_use`・`validate_genre`・`validate_profile_body`
    （以上 `handlers/org.rs` から）、`load_org_node`（`handlers.rs` から）。
  - 足した定義: `plan_delete(store, id)`（`load_org_node` で 404 → `OrgWrite::Delete`）、`OrgWrite::target_id()`、
    `OrgWritten { Node(Box<OrgNode>), Deleted { id, existed } }`、`OrgWritten::into_node()`（HTTP の upsert の route が stored node を取り出す。
    `Deleted` は internal）、`apply(tx, write, actor)`（`org_upsert_tx(tx, node, browser.then_some(actor))`・`org_delete_tx`。tx を開かず commit しない）。
  - 依存は `task_core`・`crate::problem`・`crate::types` と外部 crate（axum::http・rusqlite・schemars・serde・serde_json・time）だけ。
- `handlers/org.rs`（445 → 149 行）: 4 route は `no_query` → `require_admin` → `read_json` の後、`state.blocking` の中で
  `org_write::plan_*` → `store.write_tx(|tx| org_write::apply(tx, &write, "admin"))`。status・body・`tracing::info!` は今と同じ。
  delete は `existed` によらず 204。`crate::cos`・`OperationAudit`・`Applied` の参照は無い。
- `skills.rs`（455 → 456 行）: mount・unmount の route は同じ形。`plan_skill_mount` は `org_write::OrgWrite` を返し、`org_write::load_org_node` を使う。
  `put_skill_op`・`delete_skill_op` の `Applied` は別の領域なので触っていない。
- `cos/ops/admin.rs`（739 → 738 行。`dispatch` 関数は 3 行減）: `org.*` の腕は `org_write::plan_*` と `skills::plan_skill_mount` で plan し、
  失敗は `audit.reject` のまま。`audit.apply_checked` の closure で `org_write::apply(tx, &write, "cos")` を呼び、`Node` → node の JSON、
  `Deleted { existed: true }` → `{"id", "deleted": true}`、`existed: false` → `Err(org_node_not_found)`。`CosOperation` をそのまま返す（`audited` を通さない）。
- `schema.rs`: `crate::org_write::BrowserSettingsPatch` を参照する。
- `docs/architecture-map.md`: 「組織・案件・報告」の行に `crates/task-api/src/org_write.rs`・ADR の link・`org_write_contract.rs` を足した。

## 消した関数・参照
- `handlers::org::org_commit`（direct と audited の 2 分岐）と、その中の `crate::cos::operations::{OperationAudit, Applied}` の参照。
- org での `Applied::Direct(Some(..))`・`Applied::Audited`・`.direct()`・`audited(org::org_commit(..))`。
- `cos/ops/admin.rs` の `use crate::handlers::org;` と削除の inline plan（`crate::handlers::load_org_node` と `OrgWrite::Delete`）。
- `skills.rs` の `crate::handlers::org::org_commit(.., None)?.direct()`（2 か所）と `crate::handlers::org::OrgWrite`（2 か所）、`handlers::load_org_node` の import。
- `handlers.rs` の `load_org_node` と、使われなくなった import `OrgNode`・`SqliteStore`。
- task-core の `org_upsert_browser_settings`・`org_delete` は production の呼び出し元が無くなった。撤去は次の leaf `store-prune`（task-core はこの WU で変えない）。

## ADR からの差
- `OrgWritten::Node` は `Box<OrgNode>`（ADR は `Node(OrgNode)`）。`clippy::large_enum_variant`（608 byte 対 25 byte）で `-D warnings` が落ちたため。
  `OrgWrite::Upsert` の `node: Box<OrgNode>` と同じ扱い。
- `OrgWritten::into_node()` を足した。HTTP の 5 route（org.rs 3・skills.rs 2）が同じ写像を使うので、各 adapter に写さない（規約 2）。

## C5（A の §7 C5 の 2 本。before は `git merge-base HEAD main`、after は作業ツリー）
| 指標 | before | after |
|---|---|---|
| `crate::cos::` を書く handlers の行 | 22 行（13 file） | 19 行（12 file） |
| `(crate\|super(::super)*)::handlers` を書く cos の行 | 52 行（9 file） | 50 行（9 file） |

ADR §6 の予想（22 → 19、52 → 50）と一致した。C2・C7b・C12 は `metrics` が測る。

## 検査（提出時の HEAD の作業ツリーで実行）
- `bash scripts/dev/test-affected.sh` → exit 0。repo-health verdict=pass（deps.cycle pass、size.responsibility-growth pass、new=0 worsened=0）、
  web 101 file・705 試験 pass、nextest 6109 試験 pass（21 skip）、doc test pass。
  この実行の後に `OrgWritten::Node` を `Box` にした（clippy の指摘）。その後に task-api の試験と health を再実行した（下）。
- `Box` にした後の HEAD で: `cargo nextest run -p task-api` → 870 試験 pass（2 skip）。`bash scripts/dev/check-health.sh` → exit 0、verdict=pass
  （deps.cycle pass、size.responsibility-growth pass、new=0 worsened=0）。`sh scripts/dev/progress-index.sh --check` → exit 0。
- `cargo nextest run -p task-api -E 'test(org_write_contract) | test(cos_ops_admin_config) | binary(organization)'` → 30 試験 pass
  （`org_write_contract` 6・`cos_ops_admin_config` 4・`organization` 20）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲: `git diff --name-only "$CELERIS_WU_BASE" -- crates/task-api/tests crates/task-core crates/task-api/src/cos/operations.rs docs/api/v1 web/api/generated` → 0 行。
- 残り: `git grep org_commit -- . ':!agent-docs'` → 出力なし。`crates/task-api/src` で `handlers::org`・`handlers::load_org_node` → 出力なし。

## 未解決
- 無し（`store-prune` が task-core の wrapper と doc comment を直す）。
