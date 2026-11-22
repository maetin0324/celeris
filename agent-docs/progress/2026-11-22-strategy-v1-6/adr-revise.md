---
title: 戦略層 v1 子 6 adr-revise — ADR の改訂（StrategyRef と照会の 1 本化・reviewer run の起動口の共通化・評価の状態の正本）
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: adr-revise
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# adr-revise（WorkUnit）

完了日: 2026-11-22。
対象は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md)。コードは変えていない。
基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## commit（件名）

1. `strategy-v1 6 adr-revise: ADR の改訂（StrategyRef と照会の 1 本化・reviewer run の起動口・評価の状態の正本）と WU の進捗`

## 何を直したか（設計 review の 3 件）

1. D1.1: 戦略の参照の型を `task_core::strategy_ref::StrategyRef`（新しい葉の module `crates/task-core/src/strategy_ref.rs`）の 1 つにし、
   `cos_operations` の strategy 付き `task.create` の照会を `SqliteStore::strategy_task_creates(Option<TaskId>)`（`crates/task-core/src/store/strategy_reads.rs`）の 1 か所にした。
   子 5 の gate の生の SQL・`StrategyOperation`・task-api の型の定義と、task-worker の `ParentCheckpointStrategy` を消すと書いた。`cos/operations.rs` は変えない（gate が `pub(crate) use`）。
2. D2.3・D11: 設計 review の起動・実行・終わりを `design_gate.rs` の中で 1 つの起動口 `start_side_review(task, section, kind)` に一般化し、設計 review と途中評価の両方が使う。
   途中評価の起動の条件は `has_task_run(root)` が偽・`tree_limit_hold_for` が偽・試行の数が上限未満。`holds_design_review` は `holds_side_review` に置き換え、leases.rs は呼び出しの 1 行だけ。
   置き場は `design_gate.rs` に決めた（`dispatcher` の子で、新しい file に分けても repo health の単位は同じ `task-dispatch/src/dispatcher` になり、file と `mod` の行が増えるだけ）。
3. D1.3・D2.2・D3.4・D5.3: 状態（発火済み・実行中・評価済み・試行の数・due）を判断の行（id・triggers・effect_state）と root の `ArtifactProduced` の name だけから純関数 `checkpoint_states` で導く。
   `checkpoint_evaluation_open` は実行中だけを見る。試行は起動の時点で書く `parent-evaluation/<id>.attempt-<n>.json` の name で数える。最終の記録から `triggers` を外した。写し済みの判定も時刻を比べない。

D11 の write set に adr-revise・strategy-read・review-launcher の行を足し、controller-checkpoint の行を直した（`strategy_reads.rs` は strategy-read に移し、`parent_checkpoint.rs` と `design_gate.rs` の variant・腕を足した）。
高 risk の class に `design_gate.rs`（review-gate）・`leases.rs`（lease）・`strategy_gate.rs`（class の正規表現の外だが authz の gate として同じ扱い）を書いた。
実装前 brief・守る不変条件・保証と実装の対応・未確認を同じく直し、末尾に改訂の付記を足した。

## 実装前 brief（この WU）

1. 根本原因: ADR が照会と型の二重化・起動の写し・状態の 2 つの出どころを許す設計だった（設計 review の blocking 3 件）。
2. 責務 owner: この ADR（設計の正本）。
3. 既存の再利用先: 子 5 の gate の照会、`design_gate` の設計 review の起動、`has_task_run`・`tree_limit_hold_for`、`ArtifactProduced`、判断の表。
4. 選択肢: 起動口を `design_gate.rs` の中で一般化するか、新しい `health_review/reviewer_run.rs` に置くか。前者を採った（理由は上の 2）。
5. 新しい状態・分岐・fallback・port: 無し（文書の改訂だけ）。
6. 撤去対象: ADR の旧い記述（照会が二重実装に当たらないという文、記録の triggers からの導出、時刻による試行の数え方、時刻による写し済みの判定）。引用では残していない。
7. 不変条件: doc の link と ADR の番号の検査が通る（下の検査）。

## 検査（提出時の HEAD で実行）

- `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0。
- `sh scripts/dev/check-adr-numbers.sh` → `check-adr-numbers: ok (479 files)`、exit 0。
- `sh scripts/dev/check-adr-numbers.sh --refs --base "$CELERIS_WU_BASE"` → 「この branch で足した ADR は無い」「ok」、exit 0。
- 旧い記述が残っていないことの grep: `grep -n "applied_at\|design_review_runs\|成果物の時刻\|strategy_ref_of_task\|二重実装に当たらない" <ADR>` → 0 件。
  `StrategyOperation`・`ParentCheckpointStrategy`・`holds_design_review`・`start_design_review` は「消すもの」「撤去対象」「付記」の文だけに出る。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → この ADR とこの file の 2 件だけ。
- その他の検査の結果は result の summary に書く（Rust のコードの変更は無い）。

## 未解決事項

- 起動口が run を起こしてから試行の記録を書くまでに daemon が止まると、試行が上限を 1 回超えうる（ADR の「未確認」）。
- `project_proposal` から `parent_checkpoint` への依存の組が task-core の既存の循環に入るかは、controller-checkpoint の final review の health の材料で確かめる（ADR D11 は validate が snapshot の値だけを見る形を求めた）。

## 提案

- 無し（設計の変更は ADR の改訂の付記に書いた）。
