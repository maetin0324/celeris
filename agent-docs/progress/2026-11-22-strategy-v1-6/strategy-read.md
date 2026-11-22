---
title: 戦略層 v1 子 6 strategy-read — StrategyRef の型を task-core に 1 つ置き、cos_operations の strategy の照会を strategy_reads.rs に 1 本化
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: strategy-read
status: blocked
updated: 2026-11-22
---
# strategy-read（WorkUnit）

完了日: 未完（`bash scripts/dev/test-affected.sh` の repo health が落ちる。下の「未解決」）。
設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md) の D1.1・D11（adr-revise で改訂した版）。ADR は書いていない。
基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## commit（件名）

1. `strategy-v1 6 strategy-read: StrategyRef を task-core に 1 つ置き、cos_operations の strategy の照会を strategy_reads.rs に 1 本化`

## 実装前 brief

1. 根本原因: 戦略の文書の参照の型が 2 つ（`task_api::cos::strategy_gate::StrategyRef` と `task_worker::protocol::ParentCheckpointStrategy`）あり、
   `cos_operations` の strategy 付き `task.create` の照会が task-api の gate の中の生の SQL（`write_tx` の中の SELECT）にしか無かった。
   controller-checkpoint が同じ照会を写すと規約 2（`duplicate_implementation`）に当たる（計画 v1 の設計 review の blocking）。
2. 責務 owner: 型は task-core の葉の module `strategy_ref`、照会は task-core の store（`store/strategy_reads.rs`）。
3. 既存の再利用先: `SqliteStore::with_read_conn`（読み取り専用の接続）、行を作る試験は `cos_operation_apply_at`（CoS の operations の経路）。
   挙動の回帰は子 5 の gate の試験 `strategy_gate_`（変えていない）と `committed_schema_matches_generated`。
4. 選択肢: (a) 型を task-worker に置く → task-api が task-worker に依存するので採らない。(b) 照会を `write_tx` のまま store に移す → 書かない照会に書き込みの lock を取る理由が無いので、
   `with_read_conn` にした。(c) 全件と task で絞る形を別の SQL にする → 同じ条件の写しになるので、1 つの定数 `STRATEGY_TASK_CREATES` に `(?1 IS NULL OR target_id = ?1)` を置いた（採用）。
5. 新しい状態・分岐・fallback・port: 無し。表・索引・migration・event の種類も足さない。`sha256`・`task_id` の欠けを空文字にする扱いは gate の旧い照会（`sha256` の `unwrap_or_default`）のまま。
6. 撤去対象: `task_api::cos::strategy_gate` の `StrategyRef` の定義・`StrategyOperation`・`strategy_operations`（生の SQL）と不要になった `serde`・`StoreError` の use、
   `task_worker::protocol::ParentCheckpointStrategy`（`protocol.rs` の `pub use`、schema の `$defs`、worker-protocol.md の記述）。
7. 不変条件: 照会は `task.create`・`applied`・`payload_json.strategy.name` あり の行だけを `created_at, id` の順で返す（`strategy_reads_*`）。gate の X(v)・違反の code・422 は変わらない
   （`strategy_gate_*` 18 件を変えずに pass）。API の schema（`docs/api/v1/api-v1.schema.json`）は変わらない（`committed_schema_matches_generated`。型の doc comment・serde の属性をそのまま移した）。

## 何をしたか

- 移した型: `StrategyRef { task_id: String, name: String, sha256: String, candidate: Option<String> }`（`deny_unknown_fields`、`candidate` の serde 属性と doc comment はそのまま）を
  `crates/task-core/src/strategy_ref.rs`（新、20 行。use は serde だけ、derive に schemars）へ移した。`lib.rs` に `pub mod strategy_ref;` の 1 行。
  `git grep -n "pub struct StrategyRef" -- crates` は `crates/task-core/src/strategy_ref.rs` の 1 件だけ。
- 照会: `crates/task-core/src/store/strategy_reads.rs`（新、54 行）に `SqliteStore::strategy_task_creates(target: Option<TaskId>) -> Result<Vec<(String, StrategyRef)>, StoreError>`。
  `with_read_conn` で SELECT だけを行う。`store/mod.rs` に `mod strategy_reads;` の 1 行。
- gate: `strategy_gate.rs` は `pub(crate) use task_core::strategy_ref::StrategyRef;` で型を読み込み、`execution_facts` は `store.strategy_task_creates(None)` の全件から版ごとに name・sha256 で選ぶ（選別はこれまでどおり gate）。
  消した関数・型: `StrategyRef` の定義、`StrategyOperation`、`strategy_operations`。`cos/operations.rs` は変えていない（`super::strategy_gate::StrategyRef` のまま通る）。
- task-worker: `ParentCheckpointRequest.strategy` を `task_core::strategy_ref::StrategyRef` にし、`ParentCheckpointStrategy` を消した。`task_id` は `TaskId` から `String` になった（ADR D1.1 のとおり）。
  試験 `parent_evaluation_wire_*` は型の名前の置き換え（sample の literal と md の照合の型の名前）だけを直した。
- 文書: `docs/protocol/worker-protocol.schema.json` を `UPDATE_SCHEMA=1 cargo nextest run -p task-worker --lib -E 'test(committed_schema_matches_generated)'` で再生成
  （`$defs.ParentCheckpointStrategy` が消え `$defs.StrategyRef` が足された）。`docs/protocol/worker-protocol.md` の `strategy` の行を `StrategyRef` に直した。
  `docs/api/v1/api-v1.schema.json` と `web/api/generated/` は変わらない（task-api の `committed_schema_matches_generated` が再生成なしで pass）ので、`gen:types` は流していない。

## 試験

守る不変条件と、既存の試験で検出できない理由は `crates/task-core/src/store/strategy_reads_tests.rs` の先頭に書いた（子 5 の gate の試験は HTTP の 422 を見るので、
照会が別の action・未適用の行・strategy の無い行を拾っても fixture の件数では検出できない）。

- `strategy_reads_all_rows_in_order`: applied の strategy 付き 3 行を `created_at` の順で返し、拒否された `task.create`・strategy の無い `task.create`・strategy の付いた別の action（`task.update`）を返さない。
- `strategy_reads_by_target`: `Some(id)` は `target_id` の行だけ。拒否・strategy 無し・存在しない task は空。
- `strategy_reads_same_created_at_orders_by_id`: 同じ `created_at` は `id` の順。

実装前の失敗（`strategy_task_creates` が `Ok(Vec::new())` を返す stub の状態で実行）:

```
cargo nextest run -p task-core --lib -E 'test(strategy_reads_)'
  FAIL (1/3) store::strategy_reads::tests::strategy_reads_same_created_at_orders_by_id
  FAIL (2/3) store::strategy_reads::tests::strategy_reads_by_target
  FAIL (3/3) store::strategy_reads::tests::strategy_reads_all_rows_in_order
  Summary 3 tests run: 0 passed, 3 failed   (exit 100。left: [] と期待の行の不一致)
```

実装後（提出時の HEAD）:

- `cargo nextest run -p task-core --lib -E 'test(strategy_reads_)'` → 3 tests run: 3 passed（exit 0）。
- `cargo nextest run -p task-api -p task-worker -p task-core --lib -E 'test(strategy_gate_) | test(parent_evaluation_wire_) | test(parent_evaluation_absent_) | test(committed_schema_matches_generated) | test(strategy_reads_)'`
  → 37 tests run: 37 passed（exit 0）。内訳: `strategy_gate_` 18・`parent_evaluation_wire_` 5・`parent_evaluation_absent_` 3・`committed_schema_matches_generated` 8・`strategy_reads_` 3。

## LoC（`git diff --numstat $CELERIS_WU_BASE`）

| 区分 | 追加 | 削除 | 純増 |
|---|---:|---:|---:|
| production Rust（strategy_gate.rs 9/57、strategy_ref.rs 20/0、strategy_reads.rs 54/0、lib.rs・mod.rs 各 1/0、protocol.rs 1/1、parent_checkpoint.rs 4/14） | 90 | 72 | +18 |
| test Rust（strategy_reads_tests.rs 229/0、parent_checkpoint/tests.rs 3/3） | 232 | 3 | +229 |
| 文書（worker-protocol.md 4/2、worker-protocol.schema.json 30/30） | 34 | 32 | +2 |

production の純増 +18 の理由: 照会が task-api の private 関数から task-core の公開の store の関数になり、module の doc comment（ADR の参照）と SQL の定数の説明が増えた。
型の数は 2 → 1、`cos_operations` の strategy の照会は 1 か所（`git grep -n "strategy.name') IS NOT NULL" -- crates` は `strategy_reads.rs` の 1 件）。

## 検査（提出時の HEAD）

- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（依存の `deno_core` の deprecated の警告 1 件だけで、workspace の警告なし）。
- `bash scripts/dev/test-affected.sh` → exit 1。先頭の `scripts/dev/check-health.sh` の必須 rule `size.responsibility-growth` が fail し、試験の段に進まない:
  - `crates/task-core/src/store::SqliteStore` の method の数 514 → 515（閾値 60、delta worsened）。
  - `crates/task-core/src/store` の module の本体の行 16928 → 16954（閾値 8000、delta worsened）。
  - 他の必須 rule（`deps.cycle`・`deps.violation`・`arch-map.*`・`doc.links`・`progress.index`・`human-text` ほか）は pass。
- 計画の check 1（`cargo nextest run -p task-core -p task-api -p task-worker --no-fail-fast -E 'test(/strategy_reads_|strategy_gate_|committed_schema_matches_generated|parent_evaluation_wire_/)'`）→ 34 tests run: 34 passed（exit 0）。
- 計画の check 2（`pub struct StrategyRef` が 1 つ・`ParentCheckpointStrategy` と gate の `FROM cos_operations`・`StrategyOperation` が無い・`strategy_reads.rs` に `FROM cos_operations`）→ exit 0。
- 計画の check 5（migration・events・operations.rs の差分なし、store の差分は `strategy_reads(_tests)?.rs`・`mod.rs` だけ、書き込みの SQL の句なし）と範囲 check → 範囲外の path なし。
- store の差分: `git diff --name-only $CELERIS_WU_BASE -- crates/task-core/src/store crates/task-core/migrations crates/task-core/src/model/events.rs crates/task-api/src/cos/operations.rs`
  は `crates/task-core/src/store/mod.rs`（1 行）・`strategy_reads.rs`・`strategy_reads_tests.rs` だけ。`strategy_reads.rs` と `mod.rs` の差分に書き込みの SQL の句は無い。

## 未解決

- 計画の check 3（`bash scripts/dev/test-affected.sh`）が、ADR D1.1・D11 と計画の check 2・5 が求める置き場（`crates/task-core/src/store/strategy_reads.rs` の `SqliteStore` の method）のままでは通らない。
  `size.responsibility-growth` は store の module の行と `SqliteStore` の method の数が 1 でも増えれば worsened（fail-grade）にし、「file を分けても減らない」。
  check 2 は SQL が `store/strategy_reads.rs` にあることを、範囲 check は store の外の新しい file が無いことを求めるので、この WU の範囲では両立しない（規約 9。範囲の外に回避策を置いていない）。
- 人への決定 `strategy-read-store-size` を result.json に出した（needed_before: self）。案:
  - (a) 照会を store の外の葉でない module `crates/task-core/src/strategy_reads.rs` の自由関数 `strategy_task_creates(store: &SqliteStore, target)`（`with_read_conn`。`cos_inbox.rs` と同じ形）に移し、
    ADR D1.1・D11 と この unit の check 2・5・範囲を直す（再計画）。store の差分は 0 になり、health は通る見込み（未確認）。推奨。
  - (b) store に置いたまま、repo health の profile に store の例外を足す（policy の変更なので人の承認が要る）。
- controller-checkpoint は `strategy_task_creates(Some(root))` の最初の行を root の参照に使う（ADR D1.1）。どちらの案でも関数の名前と戻り値は同じ。

## 提案

- (a) を採る。理由: この repo は大きさの上限に達した module の外に一方向の module を置く形（`cos_inbox`・`chat_revision`・`discussion_citation`・`chat_workspace_files`）を既に使っている。
  `strategy_ref.rs` は葉のまま（ADR の deps.cycle の前提）にし、照会の module だけが store と `strategy_ref` に一方向に依存する。
