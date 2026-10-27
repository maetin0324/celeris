---
title: 継いだ Remote の作業場所で web-research などの子が担当なしで止まる不具合の修正
tasks: [01M5XEE1ZRQ0VJZA7SG94F9F3X]
status: done
updated: 2026-10-27
---
# 継いだ Remote の作業場所で web-research などの子が担当なしで止まる不具合の修正

完了日: 2026-10-27

判断は [ADR 2026-10-27-inherited-remote-unassigned-child](../adr/2026-10-27-inherited-remote-unassigned-child.md)。
発端は 01M5WC8PPW（2026-10-26）と 01M5XEA3EE（2026-10-27）。どちらも親が BenchFS の Sirius 作業場所で、web-research の子が
「担当が見つからない: … 道具 cluster:sirius」で blocked になり、人が PATCH で workspace を local に直した。

## 変更

- `crates/task-core/src/matching_candidates.rs`（新規）: matching の候補の条件（`matching_harness`・`required_cluster_tool`・
  `accepts`・`any_candidate_with_tool`）。`task_ops::matching::decide` はこれを使うように書き換えた（挙動は同じ）。
- `crates/task-core/src/delegate.rs`: `downgrade_inherited_remote_if_needed` を消し、`downgrade_inherited_remote(&mut Task, was_explicit, org)`
  にした。担当未定でも、受けられて `cluster:<id>` を持つノードが無ければ Local(<task_id>) に落とし、理由を
  `routing.workspace_downgrade` に書く。委譲（`materialize_delegated_logging`）と計画（`plan::materialize_logging`）は組み立て後に呼ぶ。
- `crates/task-ops/src/tree.rs`: 木の子は skills を決めた後に 1 回だけ判定する。
- `crates/task-ops/src/add.rs`: `cluster`・`workspace` を省いてリポジトリの置き場から Remote を継いだタスクにも同じ関数を通す。
- `crates/task-ops/src/actions.rs`: 明示の担当の `cluster:<id>` 検査の写しを `matching::assignee_has_cluster_tool` の呼び出しにした。
- `crates/task-core/src/model.rs`: `TaskRouting.workspace_downgrade`。schema（`docs/api/v1/api-v1.schema.json`・`event.schema.json`・
  `docs/protocol/worker-protocol.schema.json`）と web の生成型を再生成。`docs/api/v1/api.md` に 1 句追加。
- ADR-0062 D6 に付記。

## 試験

- (a) `delegate::tests::inherited_remote_goes_local_when_no_cluster_node_takes_the_harness`、
  `matching::tests::inherited_remote_child_without_a_cluster_taker_is_downgraded_and_assigned`（降格後 web-research に割り当たる）、
  `add::tests::inherited_remote_without_a_cluster_taker_is_created_local`（POST /tasks の親付き作成）。
- (b) `delegate::tests::inherited_remote_stays_when_a_cluster_node_takes_the_harness`、
  `matching::tests::inherited_remote_child_with_a_cluster_taker_stays_remote_and_is_assigned_there`。
- (c) `delegate::tests::explicit_remote_is_never_downgraded`、add の試験の明示 `cluster`。
- ほか `inherited_remote_stays_when_matching_does_not_run`、`downgrade_rule_still_applies_when_an_assignee_is_known`。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `bash scripts/dev/test-affected.sh` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 本番への移行

不要（config・org・DB の変更なし。`routing.workspace_downgrade` は任意欄で、旧い行はそのまま読める）。

## 未解決事項

- 既に blocked の旧タスクは自動では直らない（人の PATCH 済み）。

## 提案

なし。
