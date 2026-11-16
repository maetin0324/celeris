---
tasks: [01M7J3NKV7PY9DPT0K9D1VX4B7]
---
# ADR 2026-11-16: TaskStore の trait と単一実装の store trait を撤去し、SqliteStore の固有 method にする

- 状態: 採用・実装済み（task 01M7J3NKV7PY9DPT0K9D1VX4B7、LoC 削減 v3 の子 C）。
- 置き換える前提: [ADR 2026-11-10 TaskStore の consumer 駆動の port](2026-11-10-taskstore-consumer-ports.md) の D2（TaskStore は能力の trait を supertrait に持ち、`dyn TaskStore` の consumer はそのまま動く）と、[ADR-0033](0033-organization-projects-and-reports.md) D3・D5 の報告・承認の store を「TaskStore の supertrait」として置く形。どちらの ADR にも付記でこの ADR を指した。
- 監査: LoC 削減 v3 の子 A の候補 C1（TaskStore の trait と 1 行転送）と C3（単一実装の supertrait）。

## 文脈
- `TaskStore`（`crates/task-core/src/store/task_store.rs`、1,181 行）は fn 宣言 181・store の supertrait 18（ほかに `Send`・`Sync`）を持つ trait で、実装は `SqliteStore` の 1 つだけだった（試験の偽物も無い）。
- `impl TaskStore for SqliteStore`（`task_store_impl.rs`、1,299 行）の method のうち 144 個は、領域の file の `<method>_impl` を呼ぶだけの 1 行転送だった。
- 同じく実装が `SqliteStore` だけの store trait が 15 個あった（`ApprovalStore`・`BrowserWaitStore`・`ClusterJobWaitStore`・`CronJobStore`・`DeliveryStore`・`DiskWatchStore`・`NoticeStore`・`KnowledgeRunStore`・`McpClientStore`・`McpCallStore`・`NodeSessionStore`・`NotificationStore`・`ReportStore`・`ScriptInboxStore`・`ModelCatalogStore`）。
- 2 つ目の実装が無いので、trait は差し替えの口として使われていない。method を 1 つ足すたびに宣言・転送・本体の 3 か所を書く必要があり、production の行数の約 2,600 行がこの写しだった。

## 決定

### D1. TaskStore の trait を消し、method を SqliteStore の固有 method にする
- 1 行転送の 144 個は、領域の file の `<method>_impl` を `pub fn <method>` に改名して転送を消す。本体（SQL・transaction）は 1 行も変えない。
- 転送でない method と trait の既定の本体（`create_task`・`apply_transition`・`record_route_features` など 9 個）は、`task_store_impl.rs` の `impl SqliteStore` に置く。
- 呼び手は `&dyn TaskStore`・`Arc<dyn TaskStore>`・`S: TaskStore + ?Sized` の代わりに `&SqliteStore`・`Arc<SqliteStore>` を受ける。method の名前・引数・戻り値は変えない。
- `task_store.rs` は消す。

### D2. 実装が SqliteStore だけの store trait 15 個を固有 method にする
- 文脈に挙げた 15 個の trait の宣言を消し、`impl X for SqliteStore` を `impl SqliteStore` にする（本体は残す）。trait の既定の本体は固有 method に移す。
- celeris-mcp の `McpState` の `node_sessions`（`store` を `NodeSessionStore` として見る handle）は、trait が無くなると `store` と同じものになるので消し、`store` を使う。

### D3. 残す trait
- 試験の偽物という 2 つ目の実装がある trait は、差し替えの口として使われているので残す: `OrgDirectory`・`TaskQuery`（celeris-mcp の `tools/fake_ports.rs`）、`ScriptDomainStore`（task-dispatch の `FlakyRollback`）。
- `CredentialWaitStore`（path class `credential`）は、設計 review の範囲を増やさないため、この変更では残す。`KnowledgeUseStore`・`RequestLog` は監査 A の候補の外で、この変更では扱わない。

### D4. 変えないもの
- SQL・schema・migration・event の kind と payload・transaction の境界・lease の照合は変えない。method の境界も変えないので、state と event を同じ transaction で書く規則（ADR 2026-11-10 D5）はそのまま成り立つ。
- 試験は消さない。試験の中の `use task_core::TaskStore` などの import と `dyn TaskStore` の型注釈だけを直す。

### D5. 退けた案
- **trait を残して転送だけを macro で生成する案**: 行数は減るが、宣言と本体の二重の契約は残る。実装が 1 つのままなら trait が守るものが無い。
- **consumer ごとの port を増やし続ける案（ADR 2026-11-10 の 2 回目以降）**: 偽物を書く consumer が無いまま trait を足すと、宣言の写しが増えるだけになる。偽物が要る consumer が出たときに、その consumer が使う method だけの trait を足す（`OrgDirectory`・`TaskQuery` と同じ形）。

## 結果
- 呼び手は具体型の `SqliteStore` に依存する。偽物で試験したい consumer は、D3 の形で能力の trait を足す。
- 構造指標の `task_store_fn_decls`・`task_store_supertraits`（`scripts/dev/repo-health/profiles/celeris.toml`）は、数える file が無くなり `None` になる。profile は policy の file で、変えると `policy.changed` が別の review を求めるので、この変更では触らない（撤去の提案は task の進捗ファイルに書いた）。
- 行数の前後は task の進捗ファイル `agent-docs/progress/2026-11-16-loc-reduction-v3-c.md` にある。
