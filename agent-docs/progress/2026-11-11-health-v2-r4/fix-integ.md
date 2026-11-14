---
title: "健全性v2 R4 fix-integ: 統合の完了（統合 WU の done・PhaseIntegrated・Task の遷移）を再起動に耐える形にする"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 fix-integ

完了日: 2026-11-14

store-scope: a

## 概要

- 反例（`cex-review.md` の未解決 1、試験 7）を owner で直した。`finish_phase_integration` は統合 WU の done・次の段の WU の
  ready・`PhaseIntegrated` と Task の遷移を別々に書いていたので、統合 WU の done の後で落ちると `PhaseIntegrated` が残らず、
  途中確認の段では `PhaseGate` を飛ばす窓があった。
- 決定 store-scope の答えは a（operator の判断。人は 2026-11-14 08:07Z から不在中の inbox 対応を operator に任せた）。
  `TaskStore::phase_integration_finish` を 1 つ足し、統合 WU の done・promote の行とその `WorkUnitTransitioned`・
  `PhaseIntegrated` などの events・Task の遷移を 1 つの Immediate transaction で書く。中身は既存の `update_work_unit_tx`・
  `append_event_tx`・`apply_transition_tx` を組むだけ。SQL 文・migration・列は足していない。`repair_apply` は変えていない。
- 判断は ADR [2026-11-14-phase-integration-finish-one-write](../../adr/2026-11-14-phase-integration-finish-one-write.md) に書いた。

## 作業前 brief

1. 根本原因: `dispatcher/phase_integration.rs` の `finish_phase_integration` が、統合 WU の done（`work_unit_transition`）、
   次の段の promote（`promote_newly_ready`、WU ごとに `work_unit_transition`）、`PhaseIntegrated` と Task の遷移
   （`apply_transition_with_events`）を 3 回に分けて書いていた。症状は再起動の後に出る（`PhaseIntegrated` 0 件、途中確認の
   段では次の dispatch の `promote_newly_ready` が次の段を ready にし、`PhaseGate` を飛ばす）。
2. 責務 owner: 判断は `finish_phase_integration`、原子的な書き込みは store（`task-core/src/store/execution/work_units.rs`）。
3. 既存の再利用先: tx 関数 `update_work_unit_tx`・`append_event_tx`・`apply_transition_tx`（`execution_plan_replan_tx` と同じ
   組み方）、`task_core::newly_ready`、`reconcile_integration`。試験は `cex_review_authority.rs` の `crash_mid_integration_check`・
   `restarted_parallel_dispatcher`・`phase_integrated`。
4. 選択肢: a（store の method 1 つで 1 transaction）と b（store を変えず、再起動の照合で記録し直す）。決定どおり a。b は窓を
   残し、照合の経路を 1 本増やす。
5. 新しい状態・分岐・fallback・port: store の method 1 つ（`phase_integration_finish`）。Task の遷移が通らないときに統合 WU を
   pending に戻す分岐（ADR D3。以前は統合 WU の done だけが残った）と、その reason の定数
   `INTEGRATION_RESULT_NOT_APPLIED`。新しい状態・列・fallback は無し。
6. 撤去対象: `finish_phase_integration` の中の 3 回の書き込み（`work_unit_transition`・`promote_newly_ready`・
   `apply_transition_with_events` の呼び出し）。`promote_newly_ready` の行と event の組み立ては `ready_promotions` に移し、
   `promote_newly_ready`（人の gate の後の dispatch で使う）はそれを書くだけにした。試験 7 の旧い準備（統合 WU の done を
   store に直接書く）は、修正の後は起きない状態なので消した。
7. 不変条件: 統合の完了は 1 回の書き込みで、再起動の後に `PhaseIntegrated` はちょうど 1 件、merge は二重にならず、途中確認の
   段の `PhaseGate` を飛ばさない。試験は試験 7・`cex_integration_restart_keeps_the_stage_pause`・
   `phase_integration_finish_writes_nothing_when_the_transition_is_rejected`。

## 変更

- `crates/task-core/src/store/task_store.rs`・`task_store_impl.rs`・`execution/work_units.rs`: `phase_integration_finish` と
  その本体 `phase_integration_finish_impl`。
- `crates/task-dispatch/src/dispatcher/phase_integration.rs`: `finish_phase_integration` は、統合 WU を done に置いた行の集合
  から trigger・events・promote の行を組み立て、`phase_integration_finish` で 1 回だけ書く。worktree の削除
  （`remove_phase_worktrees` に括った。中身は以前と同じ）と最終レビューの起動は commit の後。遷移が通らなければ
  `reconcile_integration(task, "integration_result_not_applied")`。
- `crates/task-dispatch/src/dispatcher/work_units.rs`: `ready_promotions`（純粋関数。`promote_newly_ready` と
  `finish_phase_integration` が共有）。
- 試験: `cex_review_authority.rs` の試験 7 と `cex_integration_restart_keeps_the_stage_pause`、fixture の
  `crash_mid_integration_check_with`（途中確認の段 `build` と次の段 `ship`）・`finish_crashed_integration_checks`。
  `task-core/src/store/tests.rs` の `phase_integration_finish_writes_nothing_when_the_transition_is_rejected`。

## 試験 7 の修正の前と後

- 修正の前（基準 `$CELERIS_WU_BASE` の本体のまま、試験 7 に `PhaseIntegrated` 1 件の assert だけを足した）:
  `cargo nextest run -p task-dispatch -E 'test(cex_integration_restart_after_the_integration_unit_is_done_does_not_integrate_again)'`
  は exit 100。`cex_review_authority.rs:1161` の `assert_eq!` が `left: 0`・`right: 1` で落ちた（再起動の後に
  `PhaseIntegrated` が 0 件）。
- 修正の後: 同じ試験は PASS。準備は答え a なので「1 回の書き込みの前で落ちた形」に変えた（落ちた daemon の統合は merge と
  検査を終え、最後の `IntegrationCheckFinished` まで残ったが、完了は書いていない。統合 WU は running）。再起動の後に
  `PhaseIntegrated` はちょうど 1 件（merged の a・b は両方 skipped）、統合 WU の done は 1 回、merge commit は WU のブランチ
  ごとに 1 つ、最終レビューは 1 回。準備を変えた理由は試験の doc comment に書いた。
- `cex_integration_restart_keeps_the_stage_pause`: `pause_after = after(build)` の段 `build` の統合の完了の前で落ちた後、再起動
  した dispatcher は `PhaseGate` で Blocked（`awaiting_human`）にし、`PhaseIntegrated`・`PhaseReported` は 1 件ずつ、段 `ship`
  の WU `c` は pending のままで走らない。完了を書いた後にもう一度作り直して 3 tick 回しても同じ。

## 統合 WU を done にするほかの経路（目的 (4)）

統合 WU（kind `integrate`）を done にする書き手は `finish_phase_integration` だけ（`reason: "integrated"` は 1 か所。
`grep -rn '"integrated"' crates --include=*.rs` で確認）。ほかに WU を done にする経路は寄せない。理由:

- `tree_units.rs` の子 task の done（kind task の WU）と `task_ops::tree_adopt` の子 task の採用は、行と events を
  `work_unit_transition_with_events` などの 1 回の書き込みで書き、Task の遷移を伴わない。
- `review_verdict.rs` の atomic task の暗黙の WU `main`（done）の実体化は、`repair_apply` の 1 transaction で計画・行・遷移を書く。

## 確かめられない事項

- 書き込みを再び分ける退行は、dispatcher の試験では検出できない（書き込みの間で落とすには試験用の hook が要り、本体に試験用の
  分岐を足さない方針に反する）。代わりに store の試験が 1 transaction であることを、試験 7 と段の確認の試験が書き込みの前と
  後の再起動を固定する。`finish_phase_integration` の中に `work_unit_transition`・`promote_newly_ready`・
  `apply_transition_with_events` の呼び出しが無いことは下の grep で確かめた。
- ADR D3 の分岐（Task の遷移が通らない）は、統合の結果を読んだ後に人が Task を変える競合でだけ通り、試験で決定的に作れない。
  store が何も書かないことは store の試験で確かめた。

## 本番への移行

config の変更は無い。更新前の版の daemon が窓の中で落ちて残した行（`PhaseIntegrated` の無い done の統合 WU）を新しい版は
記録し直さない（ADR D4）。人は本番に入れる前に、本番 DB に対して read only で次を実行し、0 件であることを確かめる。1 件以上
なら、その task の次の段が人の確認の段かを見て、必要なら task を止めて確認する。

```sh
sqlite3 -readonly <本番 DB の path> "SELECT w.task_id, w.key FROM work_units w JOIN tasks t ON t.id = w.task_id \
 WHERE w.kind = 'integrate' AND w.status = 'done' AND w.phase IS NOT NULL \
 AND t.status NOT IN ('done','failed','cancelled') AND NOT EXISTS (SELECT 1 FROM events e WHERE e.task_id = w.task_id \
 AND json_extract(e.json,'$.type') = 'phase_integrated' AND json_extract(e.json,'$.work_unit_id') = w.id);"
```

## 証拠（提出時の HEAD で実行）

（下の「検査」に記入）

## 未解決事項

- 無し（上の「確かめられない事項」の 2 点は試験の限界として記録した）。

## 提案

- `on_integration_finished` の stale discard（Task が running でない統合の結果を捨てる）も、統合 WU を running のまま残す。
  同じ `reconcile_integration` で pending に戻すかを別 task で決める（この WU の範囲外。今回は変えていない）。
