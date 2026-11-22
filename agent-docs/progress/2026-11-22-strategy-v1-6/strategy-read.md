---
title: 戦略層 v1 子 6 strategy-read — StrategyRef の型を task-core に 1 つ置き、cos_operations の strategy の照会を task_core::strategy_reads に 1 本化し、ADR の置き場を直す
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: strategy-read
status: done
updated: 2026-11-22
---
# strategy-read（WorkUnit）

完了日: 2026-11-22。計画の全 check（再計画の後の 6 件）は提出時の HEAD で exit 0（下の「検査」）。
設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md) の D1.1・D11（adr-revise で改訂した版）。この WU の最後の run で、D1.1・D11・足さないもの・実装前 brief・保証の表の照会の置き場を crate 直下に直し、末尾に付記（WU strategy-read）を足した。
基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## commit（件名）

1. `strategy-v1 6 strategy-read: StrategyRef を task-core に 1 つ置き、cos_operations の strategy の照会を strategy_reads.rs に 1 本化`
2. `strategy-v1 6 strategy-read: 照会の試験を strategy_reads_tests.rs に移し、repo health の size の不合格と人への決定を進捗に書く`
3. `strategy-v1 6 strategy-read: 案 (a) の health の確認を進捗に書く`
4. `strategy-v1 6 strategy-read: 照会を store の外の task_core::strategy_reads の自由関数に移す（人の決定 strategy-read-store-size の (a)）`
5. `strategy-v1 6 strategy-read: 置き場の移動の後の検査と再計画の申告を進捗に書く`
6. `strategy-v1 6 strategy-read: check の不合格の確認と再計画の再申告を進捗に書く`
7. `strategy-v1 6 strategy-read: ADR D1.1・D11 の照会の置き場を crate 直下の task_core::strategy_reads に直し、理由を付記に書く`
8. `strategy-v1 6 strategy-read: 再計画の後の全 check の結果と完了を進捗に書く`

## 実装前 brief

1. 根本原因: 戦略の文書の参照の型が 2 つ（`task_api::cos::strategy_gate::StrategyRef` と `task_worker::protocol::ParentCheckpointStrategy`）あり、
   `cos_operations` の strategy 付き `task.create` の照会が task-api の gate の中の生の SQL（`write_tx` の中の SELECT）にしか無かった。
   controller-checkpoint が同じ照会を写すと規約 2（`duplicate_implementation`）に当たる（計画 v1 の設計 review の blocking）。
2. 責務 owner: 型は task-core の葉の module `strategy_ref`、照会は task-core の crate 直下の module `strategy_reads`（store の外。人の決定 `strategy-read-store-size` の (a)。初めの実装は ADR のとおり `store/strategy_reads.rs` に置いたが、repo health で落ちたので移した）。
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
- 照会: `crates/task-core/src/strategy_reads.rs`（新、56 行）に自由関数 `strategy_task_creates(store: &SqliteStore, target: Option<TaskId>) -> Result<Vec<(String, StrategyRef)>, StoreError>`。
  `with_read_conn`（`pub(crate)`）で SELECT だけを行う。`lib.rs` に `pub mod strategy_reads;` の 1 行。`cos_inbox.rs` と同じく store の外に置き、store と `strategy_ref` に一方向に依存する（task-core の中でこれを use する module は無い）。
  store の差分は 0（`git diff --name-only $CELERIS_WU_BASE -- crates/task-core/src/store` が空）。
- gate: `strategy_gate.rs` は `pub(crate) use task_core::strategy_ref::StrategyRef;` で型を読み込み、`execution_facts` は `strategy_reads::strategy_task_creates(store, None)` の全件から版ごとに name・sha256 で選ぶ（選別はこれまでどおり gate）。
  消した関数・型: `StrategyRef` の定義、`StrategyOperation`、`strategy_operations`。`cos/operations.rs` は変えていない（`super::strategy_gate::StrategyRef` のまま通る）。
- task-worker: `ParentCheckpointRequest.strategy` を `task_core::strategy_ref::StrategyRef` にし、`ParentCheckpointStrategy` を消した。`task_id` は `TaskId` から `String` になった（ADR D1.1 のとおり）。
  試験 `parent_evaluation_wire_*` は型の名前の置き換え（sample の literal と md の照合の型の名前）だけを直した。
- 文書: `docs/protocol/worker-protocol.schema.json` を `UPDATE_SCHEMA=1 cargo nextest run -p task-worker --lib -E 'test(committed_schema_matches_generated)'` で再生成
  （`$defs.ParentCheckpointStrategy` が消え `$defs.StrategyRef` が足された）。`docs/protocol/worker-protocol.md` の `strategy` の行を `StrategyRef` に直した。
  `docs/api/v1/api-v1.schema.json` と `web/api/generated/` は変わらない（task-api の `committed_schema_matches_generated` が再生成なしで pass）ので、`gen:types` は流していない。

## 試験

守る不変条件と、既存の試験で検出できない理由は `crates/task-core/src/strategy_reads_tests.rs` の先頭に書いた（子 5 の gate の試験は HTTP の 422 を見るので、
照会が別の action・未適用の行・strategy の無い行を拾っても fixture の件数では検出できない）。

- `strategy_reads_all_rows_in_order`: applied の strategy 付き 3 行を `created_at` の順で返し、拒否された `task.create`・strategy の無い `task.create`・strategy の付いた別の action（`task.update`）を返さない。
- `strategy_reads_by_target`: `Some(id)` は `target_id` の行だけ。拒否・strategy 無し・存在しない task は空。
- `strategy_reads_same_created_at_orders_by_id`: 同じ `created_at` は `id` の順。

実装前の失敗（`strategy_task_creates` が `Ok(Vec::new())` を返す stub の状態で、初めの置き場 `store/strategy_reads.rs` で実行。試験の中身は移動の前後で同じで、呼び出しの形だけ `strategy_task_creates(&store, …)` に変えた）:

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

## LoC（`git diff --numstat $CELERIS_WU_BASE`、提出時の HEAD）

| 区分 | 追加 | 削除 | 純増 |
|---|---:|---:|---:|
| production Rust（strategy_gate.rs 10/57、strategy_ref.rs 20/0、strategy_reads.rs 56/0、lib.rs 2/0、protocol.rs 1/1、parent_checkpoint.rs 4/14） | 93 | 72 | +21 |
| test Rust（strategy_reads_tests.rs 229/0、parent_checkpoint/tests.rs 3/3） | 232 | 3 | +229 |
| 文書（worker-protocol.md 4/2、worker-protocol.schema.json 30/30） | 34 | 32 | +2 |

production の純増 +21 の理由: 照会が task-api の private 関数から task-core の公開の関数になり、module の doc comment（ADR の参照と store の外に置いた理由）と SQL の定数の説明が増えた。
型の数は 2 → 1、`cos_operations` の strategy の照会は 1 か所（`git grep -n "strategy.name') IS NOT NULL" -- crates` は `crates/task-core/src/strategy_reads.rs` の 1 件）。

## 検査（提出時の HEAD。再計画の後の計画の check）

1. check 1 `cargo nextest run -p task-core -p task-api -p task-worker --no-fail-fast -E 'test(/strategy_reads_|strategy_gate_|committed_schema_matches_generated|parent_evaluation_wire_/)'`
   → 34 tests run: 34 passed, 3724 skipped（exit 0）。
2. check 2（`pub struct StrategyRef` が crates で 1 件、`ParentCheckpointStrategy` 無し、`strategy_gate.rs` に `FROM cos_operations`・`StrategyOperation` 無し、
   `crates/task-core/src/strategy_reads.rs` に `FROM cos_operations` あり、store の下に照会の file 無し、ADR に旧い置き場の 3 つの形が無い）→ exit 0。
3. check 3 `bash scripts/dev/test-affected.sh` → exit 0。repo health は `verdict=pass exit=0 fail_grade=0`（`size.responsibility-growth` pass・`deps.cycle` pass・`doc.links` pass・`adr.numbers` pass、
   `new=8 worsened=2` は fail-grade でない）。nextest は `8720 tests run: 8720 passed (7 slow), 31 skipped`。task-api・task-core の変更なので web の typecheck と unit 試験も走り、通った。
4. check 4 `cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check && sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh`
   → exit 0（`check-doc-links: ok`、`check-adr-numbers: ok (479 files)`）。
5. check 5（`git merge-base HEAD main` から store・migration・`model/events.rs`・`store/events.rs`・`cos/operations.rs` の差分 0、`strategy_reads.rs` の追加行に書き込みの SQL の句が無い）→ exit 0。
6. 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ exit 0。変更した path は `crates/task-core/src/{lib,strategy_ref,strategy_reads,strategy_reads_tests}.rs`・`crates/task-api/src/cos/strategy_gate.rs`・
   `crates/task-worker/src/protocol.rs`・`crates/task-worker/src/protocol/parent_checkpoint.rs`・`…/parent_checkpoint/tests.rs`・`docs/protocol/worker-protocol.{md,schema.json}`・
   ADR 2026-11-22-parent-checkpoint-evaluator・この進捗。

## 未解決

- 人の決定 `strategy-read-store-size` の答えは (a)（CoS の代答。人の選択ではない）: 照会を crate 直下の `task_core::strategy_reads` の自由関数に移し、ADR D1.1・D11 とこの unit の check を再計画で直す。
  repo health の profile・rule は変えていない（例外も足していない）。
- 経過: 置き場を移した後も計画の check 2 と範囲 check が旧い置き場を見ていた。1 回目の run は進捗に再計画の申告を書いて done を返し（check が不合格）、2 回目の run は `plan_issue` を返し、再計画で check が直った。最後の run で ADR を直し、全 check を再実行した。
- ADR の付記: 置き場を変えた理由（store の module は `size.responsibility-growth` の上限に達しており、1 行の増加でも repo health が落ちる）、高 risk の class `store` に当たらなくなったこと、
  controller-checkpoint の呼び出しの形 `strategy_task_creates(store, Some(root))` を書いた。
- 未確認: 照会の性能（`cos_operations` の全件を読む gate の照会は旧い生の SQL と同じ条件・同じ順で、索引も同じ。本番の行数での時間は測っていない）。

## 提案

- 計画の check に path を固定した grep を置くときは、人の決定で置き場が変わりうる file（repo health の size の上限に近い module の下）なら、決定の答えを待ってから check を確定する。
