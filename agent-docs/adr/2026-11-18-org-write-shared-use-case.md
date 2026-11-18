---
tasks: [01M7MZTKB4RYM80S6CT3RST2N9]
---
# ADR 2026-11-18: org の書き込みを task-api の共有 use-case（`org_write.rs`）へ集約し、HTTP と CoS の adapter から一方向に呼ぶ

- 日付: 2026-11-18
- 状態: Accepted（決定だけ。この ADR を書いた WorkUnit `adr` は `crates/`・`web/` を変えていない。旧コードに当てる契約試験は WorkUnit
  `contract-tests`、集約は `usecase`、store の wrapper の撤去は `store-prune`、計測は `metrics`・`timing`、まとめは `close` が行う）
- 関係: [ADR-0033 組織・案件・報告](0033-organization-projects-and-reports.md) D1（`/org` の編集）、
  [ADR 2026-10-09 CoS の全変更操作](2026-10-09-cos-operations-all-mutations.md)（不変条件 `cos-audit-same-tx`）、
  [ADR 2026-10-27 API の主体](2026-10-27-api-principals.md)（`api-principal-from-token`）、
  [ADR 2026-11-04 project controller](2026-11-04-project-controller.md) D12（`SqliteStore::write_tx` で他の crate の `*_tx` を束ねる）、
  [ADR 2026-11-17 deps.cycle の新しい組と論理 module](2026-11-17-repo-health-scc-edges-logical-modules.md)（アーキ v4 B。`super::` の辺を数える）、
  [ADR 2026-11-17 境界の意味を変える改修の risk](2026-11-17-boundary-change-risk-review.md)（アーキ v4 C。`store` class の設計 review）、
  [2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。
- 計測の規則: アーキ v4 A の baseline（task 01M7MZTKAXM8GJGK8BPE13CQRM の artifacts の `baseline.md` §7）の C2・C5・C7b・C12 をそのまま使う。
  before の値は [進捗 adr.md](../progress/2026-11-18-arch-v4-d/adr.md)。

## 1. 根本原因

症状: task-api の 49 単位の SCC の中で、`handlers` と `cos` が互いを参照する。org の周りでは次の 10 行が辺になっている（`git merge-base HEAD main`
＝ `phase fix` の統合の commit で、HEAD の検査器の C2 で確認）。

| 向き | 行 | 中身 |
|---|---|---|
| handlers → cos | `crates/task-api/src/handlers/org.rs:99-101`（3 行） | `org_commit` が CoS adapter の型 `cos::operations::OperationAudit`・`Applied` を引数と戻り値に取る |
| cos → handlers | `crates/task-api/src/cos/ops/admin.rs:313,339`（2 行） | CoS の `org.*` が `handlers::org` の `plan_*`・`OrgWrite`・`org_commit` と `handlers::load_org_node` を呼ぶ |
| skills → handlers | `crates/task-api/src/skills.rs:412,431,446,451`（4 行） | HTTP の skill の mount・unmount が `handlers::org::org_commit(.., None)?.direct()` と `OrgWrite` を使う |
| schema → handlers | `crates/task-api/src/schema.rs:119`（1 行） | OpenAPI の schema が `handlers::org::BrowserSettingsPatch` を参照する |

原因は 2 つある。

1. org の業務処理（入力の検証 `plan_create`・`plan_patch`・`plan_browser_settings` と、DB への適用 `org_commit`）が HTTP adapter の
   `handlers/org.rs` に置かれている。CoS adapter（`cos/ops/admin.rs`）と HTTP の skill の route（`skills.rs`）は、業務処理を使うために
   HTTP adapter を import する。
2. `org_commit` が「誰が呼んだか」で 2 つに分かれている。`audit: None`（HTTP）は `SqliteStore::org_upsert_browser_settings`・
   `org_upsert`・`org_delete`（それぞれ自分で transaction を開く wrapper）を呼び、`audit: Some`（CoS）は
   `OperationAudit::apply_checked` の transaction の中で `org_upsert_tx`・`org_delete_tx` を呼ぶ。同じ書き込み（upsert・browser の
   監査行・delete）を 2 回実装しており、分岐のために HTTP adapter が CoS の型を知る。さらに CoS の `org.delete` は、削除の plan
   （`load_org_node` の後に `OrgWrite::Delete`）を `admin.rs:337-341` に inline で書き直している（HTTP は `delete_org_node` に同じ処理がある）。

## 2. 決定

### D1. 共有 use-case `crates/task-api/src/org_write.rs`

`lib.rs` に `mod org_write;` を足し、次を置く（既存の定義は `handlers/org.rs`・`handlers.rs` から移す。移動そのものは成果に数えない）。

- `BrowserSettingsPatch`（`schema.rs` は `crate::org_write::BrowserSettingsPatch` を参照する）。
- `OrgWrite { Upsert { node: Box<OrgNode>, browser: bool }, Delete(String) }` と、対象の id を返す `OrgWrite::target_id()`。
- `load_org_node`（`handlers.rs:370` から移す。呼び出し元は `handlers/org.rs`・`skills.rs`・`cos/ops/admin.rs` だけ）。
- plan（読み取りと検証だけ。DB を書かない）: `plan_create(store, genres, OrgCreateBody)`・`plan_patch(store, genres, id, Value)`・
  `plan_browser_settings(store, genres, id, Value)`・`plan_delete(store, id)`（`load_org_node` で 404 を判定して `OrgWrite::Delete`）。
  補助の `parse_browser_settings_json`・`set_credential_use`・`validate_genre`・`validate_profile_body` も同じ file の private にする。
- apply（1 つの transaction の中で書く）: `apply(tx: &Transaction<'_>, write: &OrgWrite, actor: &str) -> Result<OrgWritten, ApiProblem>`。
  - `Upsert { node, browser }` → `SqliteStore::org_upsert_tx(tx, node, browser.then_some(actor))` → `OrgWritten::Node(stored)`。
  - `Delete(id)` → `SqliteStore::org_delete_tx(tx, id)` → `OrgWritten::Deleted { id, existed }`。
  - transaction を開かず、commit もしない（呼び出し元の transaction に入る）。

依存の向き: `org_write` は `task_core`・`crate::problem`・`crate::types`（と外部 crate の serde・serde_json・schemars・axum::http・time・rusqlite の型）
だけを使う。`problem` と `types` は task-api の SCC の外にある（C2 の JSON で確認）。`state`・`middleware`・`schema`・`handlers`・`cos`・
`skills` は SCC の中なので `org_write` から使わない。これで `org_write` は SCC の外の単位になり、adapter からの辺は一方向になる。

### D2. HTTP adapter（`handlers/org.rs`・`skills.rs`）

各 route は今の順で `no_query` → `require_admin` → `read_json` を行い、`state.blocking` の中で
`let write = org_write::plan_*(store, ..)?; store.write_tx(|tx| org_write::apply(tx, &write, "admin"))` を呼ぶ（`ApiProblem` は
`From<StoreError>` を持つので `write_tx` の `E` にそのまま使える）。response は今と同じ:
`OrgWritten::Node` は今と同じ status と node の JSON、`Deleted` は `existed` によらず 204（今の `delete_org_node` は `org_delete` の `false` を無視している）。
`tracing::info!` の行は残す。`skills.rs` の mount・unmount も `plan_skill_mount`（戻り値を `org_write::OrgWrite` にする）の後に同じ形で呼ぶ。
`org_list` は読み取りなので変えない。

### D3. CoS adapter（`cos/ops/admin.rs` の `org.*` の腕）

plan は `org_write::plan_create`・`plan_patch`・`plan_browser_settings`・`plan_delete` と `crate::skills::plan_skill_mount` で作る。
plan の失敗は今と同じく `audit.reject(store, "org", &id, problem)` で拒否の行と理由付きの監査 event を残す。適用は
`audit.apply_checked(store, "org", &write.target_id(), matched.action, |tx| ..)` の closure で `org_write::apply(tx, &write, "cos")` を呼び、
`Node(n)` は node の JSON、`Deleted { existed: true }` は `{"id": id, "deleted": true}`、`Deleted { existed: false }` は
`Err(ApiProblem::org_node_not_found(id))`（transaction は rollback され、`apply_checked` が rejected の行を残す。今と同じ）にする。
腕は `apply_checked` の `CosOperation` をそのまま返す（`audited(Applied)` の包みを通さない）。

### D4. 変えないもの（互換）

- actor の値: HTTP の browser settings は `admin`、CoS の browser settings は `cos`、それ以外の upsert は `None`（`org_browser_events` を書かない）。
- HTTP と CoS の response の status・body と problem の code（`validation`・`unknown_site_policy`・`org_node_not_found`・store の拒否の code）。
- 削除で行が無いとき: HTTP は plan の `load_org_node` で 404、plan と apply の間で消えたときは 204（今と同じ）。CoS は plan で
  `org_node_not_found`（reject）、apply で消えていれば `org_node_not_found`（rollback と rejected の行）。
- 認可: HTTP は `require_admin`（今と同じ。CoS の bearer も通る）、CoS は `/api/v1/cos/operations` の既存の主体の判定。どちらも変えない。
- plan の読み取りは transaction の外、検証つきの書き込み（`org_upsert_tx` の `validate_upsert`、`org_delete_tx` の `InUse`）は transaction の中という
  今の分け方を変えない。

### D5. 新しい状態・分岐・fallback・port

- 足すもの: `OrgWritten { Node(OrgNode), Deleted { id: String, existed: bool } }` の 1 つ。HTTP（無い行の削除は 204）と CoS（無い行の削除は
  `org_node_not_found`）で扱いが違うので、apply は事実（消えたか）を返し、response の写像は adapter が持つ。
- 足さないもの: trait・Port・Facade・Manager・fallback・新旧切替の flag・config の欄・API の欄。`apply` は既存の `*_tx` を呼ぶ関数で、
  型を包む層ではない。

### D6. 実装前 brief（7 項目）との対応

1. 根本原因 = §1。2. 責務 owner = `org_write.rs`（org の業務処理）、`handlers/org.rs`・`skills.rs`（HTTP の写像）、`cos/ops/admin.rs`（CoS の写像と監査）、
   `task-core::store::org`（tx の中の書き込み）。owner 地図は `docs/architecture-map.md` の「組織・案件・報告」の行に `crates/task-api/src/org_write.rs` を
   足す（`usecase` が行う）。3. 既存の再利用先 = `SqliteStore::write_tx`・`org_upsert_tx`・`org_delete_tx`・`OperationAudit::apply_checked`・`reject`、
   既存の試験 `crates/task-api/tests/organization.rs`・`crates/task-api/tests/cos_delivery_suite/cos_ops_admin_config.rs`
   （`cos_ops_admin_config_org_and_skills_are_audited`）・`crates/task-core/src/store/tests.rs` の org の試験。4. 選択肢 = §3。5. 新しい状態 = D5。
   6. 撤去 = §4。7. 不変条件 = §5。

## 3. 比べた案

| 案 | 内容 | 判定と理由 |
|---|---|---|
| A（採用） | task-api 内の `org_write.rs` に plan と tx 内の apply を置き、HTTP と CoS の adapter が一方向に呼ぶ | `org_commit` の 2 分岐と CoS の inline の削除 plan が消え、書き込みは 1 本になる。problem の code・`ApiProblem` をそのまま使えるので写像の重複が出ない |
| B | plan と apply を task-ops へ移す | 退けた。plan は `ApiProblem` の code（`unknown_site_policy`・`validation` の field）と `ValidationError` を返す。task-ops は task-api を知らないので、新しい error 型と `ApiProblem` への写像が要り、同じ仕様の二重実装（規約 2）と新しい境界の型が増える。task-ops の他の呼び出し元（celeris-mcp・celerisctl）は今この plan を使わないので、移しても共有は増えない |
| C | `OperationAudit`（と `Applied`）を SCC の外の下位 module へ移し、`org_commit` は残す | 退けた。`OperationAudit` は `cos::operations` の chat の error・監査の文脈・`cos_operation_apply` と結び付いており、移すと CoS の内部を大きく動かす移動になる（成果に数えない）。HTTP adapter が「audit の有無」で分岐する 2 本の書き込みは残り、cos → `handlers::org` の逆参照も残る |
| D | plan を `cos` 側へ写し、HTTP 側も残す | 退けた。同じ検証を 2 か所に持つ（規約 2、別 module へのコピーは task の不合格の条件） |

## 4. 撤去

同じ変更（`usecase`・`store-prune`）で消す。旧新の並存は無い。

- `crates/task-api/src/handlers/org.rs`: `org_commit`（direct と audited の 2 分岐）と、その中の `use crate::cos::operations::{OperationAudit, Applied}` の参照（99-101 行）。
  `OrgWrite`・`plan_*`・`BrowserSettingsPatch`・補助の関数の handlers 側の定義（`org_write.rs` へ移る）。
- `crates/task-api/src/handlers.rs`: `load_org_node`（`org_write.rs` へ移る）。
- `crates/task-api/src/cos/ops/admin.rs`: `use crate::handlers::org;`（313 行）、削除の inline plan（337-341 行の `crate::handlers::load_org_node` と `OrgWrite::Delete`）、
  `audited(org::org_commit(.., Some(audit))?)`（344 行）。
- `crates/task-api/src/skills.rs`: `crate::handlers::org::org_commit(.., None)?.direct()`（412・431 行）と `crate::handlers::org::OrgWrite`（446・451 行）。
- org での `cos::operations::Applied` の使用（`Applied::Direct(Some(node))`・`Applied::Audited`・`.direct()`・`audited(..)`）。`Applied` 自体は accounts・clusters などの
  他の操作が使うので残す。
- `crates/task-core/src/store/org.rs`: production の呼び出し元が無くなる `SqliteStore::org_upsert_browser_settings`・`org_upsert_with_actor`（private。
  `org_upsert_browser_settings` だけが使う）・`org_delete`。`org_upsert`（`org_directory.rs`、celeris-mcp が使う）は残す。
  試験 `org_delete_refuses_while_a_task_is_open_or_children_remain`（`store/tests.rs:2799`）は `store.write_tx(|tx| SqliteStore::org_delete_tx(tx, ..))` に移し、
  拒否（`InUse`）・成功・無い id の `false` の 3 つの検査を保つ。doc comment の旧い名前（`org_upsert_tx` の「as `org_upsert_browser_settings` does」、
  `org_delete_tx` の「Same refusals as `org_delete`」）を直す。

規模の目安（約束ではない。`metrics` が C12 で測る）: production は `org_commit` の約 50 行・store の wrapper 約 30 行・CoS の inline plan 約 5 行が消え、
`apply` と `OrgWritten` で約 35 行が増える。行数の純減は task の目的ではないが、増えたら `metrics` が理由を書く。

## 5. 不変条件と試験

`contract-tests` が旧コード（集約の前）に当てて値を固定し、`usecase`・`store-prune` の後に同じ試験が変更なしで通ることを確かめる。
試験を足す理由（既存の試験で検出できない点）は `contract-tests` の進捗に書く（規約 10）。

| 不変条件 | 試験で確かめること |
|---|---|
| `cos-audit-same-tx` | CoS の `org.create`・`org.update`・`org.browser_settings`・`org.delete` の成功で、org の行と `cos_operations` の行と監査 event（AuditContext の reason・policy_version を含む）が同じ transaction で入る。反例: apply の中で拒否される書き込み（子 node や未終了 task がある node の `org.delete` の `InUse`、`validate_upsert` の拒否）で org の行が変わらず、operation は rejected で detail（拒否理由）を持つ（rollback） |
| 拒否理由 | plan の拒否（`validation`・`unknown_site_policy`・`org_node_not_found`）は HTTP では同じ status と code の problem、CoS では `audit.reject` の rejected の行と理由付きの event になる。値は旧コードで固定する |
| Authorization（`api-principal-from-token`） | HTTP の `POST/PATCH/DELETE /org…` と skill の mount・unmount は、token が無ければ今と同じ拒否（`require_admin`）。CoS の操作は CoS の主体でなければ今と同じ拒否 |
| response の互換 | HTTP の status と node の JSON、DELETE の 204。CoS の operation の `result`（node の JSON、削除は `{"id", "deleted": true}`）と `state` |
| actor | `org_browser_events` の actor が HTTP の browser settings で `admin`、CoS で `cos`。browser でない upsert では行が増えない |
| 依存の向き | `org_write` の単位が task-api の SCC に入らない。`handlers/org.rs` に `crate::cos::` が無く、`cos/` に `handlers::org`・`handlers::load_org_node` が無い（§6 の C2・C5） |
| `no-unwrap-outside-tests`・`events-append-only`・`task-state-in-sqlite` | 新しい code に `unwrap()` を置かない。events は既存の `*_tx`・`cos_operation_apply` が追記するだけで、書き換えない |

既存の試験 `crates/task-api/tests/organization.rs`・`cos_ops_admin_config_org_and_skills_are_audited`・`crates/task-core/src/store/tests.rs` の org の試験は消さない。

## 6. 計測

A の baseline §7 の command を同じ規則で before（`git merge-base HEAD main`）と after（`integrate-impl` の後の HEAD）に当てる。値は進捗に書く。

- C2（循環の全部の辺）: task-api の SCC の単位・組・行、`handlers → cos` と `cos → handlers` の行の辺（本数と file 数）、org の 10 行が消えたか、
  `crates/task-api/src/org_write` が SCC の外か。検査器は HEAD の `scripts/dev/repo-health`（B の後の数え方。A の旧い検査器は C が足した profile の欄
  `review.high_risk[].invariants` を読めず merge-base では動かない）。A の値（旧い検査器 × `bc665168`）は並べて書く。
- C5（`git grep` の 2 本）: `crate::cos::` を書く handlers の行、`(crate|super(::super)*)::handlers` を書く cos の行。
- C7b（S1 の変える単位を import する単位、直接と推移的）: A の S1 の単位（`handlers`・`cos`・`task-core/src/store`）のまま測る。
- C12（source-size-report）: `scripts/dev/source-size-report.py` と `source-size-report.toml` を base から `git show` で取り出し、
  `--exclusive-classes --rev <rev> --format json` を base と HEAD に当てる。production 全言語・production Rust・own_total_with_tests を分けて書く。
- 予想（約束ではない）: org の 10 行が消え、C5 は handlers → cos 22 → 19 行、cos → handlers 52 → 50 行、C2 の task-api の SCC の行は 632 → 622。
  単位（49）と組（261）は変わらない見込み（`handlers ⇄ cos` は accounts・clusters・projects・tasks などの辺で残る）。
  C6 の `&Transaction` の引数は task-api で 1 増える（`apply` の `tx`。SQL 文は書かず、task-core の `*_tx` を呼ぶだけ）。結果が予想と違えば `metrics` が理由を書き、
  達成と書かない。
- 時間（C8・C9）は `timing` が A の S1 の file の一覧のまま測る（`org_write.rs` を一覧に足さない。足すなら両方の値を書く）。

## 後続 WorkUnit への引き継ぎ

- `contract-tests`: §5 の表の行ごとに旧コードで値を固定する。CoS の rollback の反例は決定的な入力（子 node のある node の削除など）で作り、時間に依存させない。
- `usecase`: D1〜D4。`docs/architecture-map.md` の「組織・案件・報告」の行に `org_write.rs` を足す。`cos/operations.rs`（`authz` class）は変えない。
- `store-prune`: §4 の task-core の 3 関数と試験の移し替え。`crates/task-core/src/store/` は `store` class なので、この ADR を設計 brief として参照する。
- `metrics`・`timing`: §6。
