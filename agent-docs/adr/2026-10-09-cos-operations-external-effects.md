# ADR 2026-10-09: CoS 操作の外部副作用を二段階で監査する

---
tasks: [01M4F5KS8E1MXZDNJTRAVFJESW]
---

- 状態: 実装済み（2026-10-09。末尾の付記）
- 関連: [全変更操作 ADR](2026-10-09-cos-operations-all-mutations.md)、[ops-common](../progress/2026-10-09-cos-operations-all-mutations/ops-common.md)

## D1 問題

SQLite の監査記録と Git、GitHub、knowledge repository などの外部副作用は同一 transaction にできない。
副作用を先に実行すると daemon 停止時に証跡がなくなり、監査を先に applied とすると実際の変更がない記録になる。

## D2 共通規則

外部副作用のある操作は `begin_external` で `cos_operations` を `pending` として監査記録し commit してから副作用を一度だけ実行し、
結果が確定したら `finish_external` で結果と `applied` event/card を記録する。副作用を始める前の検証失敗は `rejected` として記録する。
`pending` の同じ idempotency key の再送は副作用を再実行せず既存 operation を返し、状態照会または人の解決を待つ。

## D3 回復

起動時、古い `pending` は注入時計で定義した猶予を超えたものだけ `needs_remediation` に遷移させる。
これは副作用の成功・失敗を推測しないための状態である。再送・自動再実行はしない。
閾値、遷移と監査 event の更新は migration 無しで既存 `cos_operations` の状態と列を使う。

## D4 operation 分類

- A: SQLite 内で原子化できる領域更新。現行の単一 transaction の適用を使う。
- B: 計画採用など複数の内部書き込みを伴うが外部副作用がない操作。domain transaction を共有し、監査を同じ transaction に含める。
- C: Git/GitHub/knowledge repository 等の外部副作用。D2 の二段階監査を使う。

tasks の execution-plan POST と tree/adopt は B、changes/integrate と PR merge および knowledge page PUT は C。

## D5 全変更操作との関係

[全変更操作 ADR](2026-10-09-cos-operations-all-mutations.md) D3 の ALLOWED/EXCLUDED 網羅性を維持する。
この ADR は外部副作用操作の監査方式だけを定め、操作範囲や人の除外決定を変更しない。

## 付記: 実装（2026-10-09、task 01M4F5KS8E1MXZDNJTRAVFJESW）

- 共通 helper: tc の `begin_external`・`finish_external`・起動時の古い pending の回収（`needs_remediation`）、
  ta の `OperationAudit::external`（効果 closure: `Ok(Ok)` = applied、`Ok(Err)` = 変更前の拒否で rejected、`Err` = 結果不明で pending のまま）と
  `cos::operations::external_store`（DB 1 transaction の write を C に入れる版。失敗は全て rejected）。
  daemon 起動時の回収の猶予は 30 分（cl `daemon/bootstrap.rs` の `COS_EXTERNAL_PENDING_GRACE_SECS`）。
- C にした操作: task の integrate・PR merge、案件の文書（docs page・maintenance）、KB page PUT、cron の手動実行、
  daemon channel（reload・notify test・replay・check・account 退避）、model の発見、release promote、skill の KB 書き込み、
  provider・account の file 書き込み、chat 添付の upload・削除、成果物の文書昇格、組織ノードへの話しかけ、browser の設定・待ち・制御。
  B は execution-plan POST（`execution_plan_adopt_tree_tx`）と tree/adopt（`tree_adopt_apply_tx`）。
- 再送が副作用を再実行しないこと・結果不明が pending に残ること・変更前の拒否が rejected になることは fake の効果で数えて試験した
  （ta `tests/cos_ops_projects_cron_changes.rs`・`cos_ops_projects_cron_docs.rs`・`cos_ops_admin_config.rs`・`cos_ops_ops_surface.rs`）。
- 残る限界: provider・account の file 効果は SQLite commit が失敗すると file だけ残り得る。browser の DB write は 2 段の間に入る
  （browser store に `_tx` 版を作れば A にできる。進捗の提案）。
