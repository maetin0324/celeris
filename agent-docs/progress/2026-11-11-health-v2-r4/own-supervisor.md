---
title: "健全性v2 R4 own-supervisor: 実行中の run の表と lease の監督を RunSupervisor へ移す"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-13
---
# 健全性v2 R4 own-supervisor

完了日: 2026-11-13

## 概要

- ADR 2026-11-11 dispatcher-state-ownership D1 の表で unit 列が own-supervisor の 7 field（`running`・`reviewing`・`infra_backoff`・`just_aborted`・
  `lost_review_watch`・`ownerless_reconciled_at`・`terminal_records_reconciled_at`）を `Dispatcher` から消し、
  `crates/task-dispatch/src/run_supervisor.rs` の `RunSupervisor`（private field 7 件）に移した。`RunKey`・`RunEntry`・`ReviewEntry` も同じ file に移した
  （owner が `dispatcher` module を使わないため）。
- 手元の run の問い合わせ（`runs`・`run_keys`・`holds_run_id`・`holds_run_or_review`・`runs_for_task`・`has_task_run`・`alive_task_run`・
  `worker_run_count`・`cos_run_count`）、abort（`abort_runs`・`take_run`・`stop_process`・`stop_review`）、shutdown の割り込み（`take_runs`・`take_reviews`）、
  見失った review の判定（`take_lost_reviews`）、照合の周期（`reconcile_due`）は `RunSupervisor` の method。実行中の run の鍵は `run_keys`（読み取り専用）で返す。
- `reclaim_expired_leases`（D3）: 回収の判断を純粋関数 `reclaim_reason`（期限・工程の lease・手元の run・持ち主の居ないこと → 回収の理由）、
  `reclaim_keys`（手元の run の鍵 → 回収する鍵）、`reclaim_verdict`（理由・infra の回数・上限 → trigger・終了記録の文と分類・WU を戻す理由・backoff）に分けた。
  lease の解放と終了記録（`WorkerFinished` を遷移と同じ transaction で積む）と backoff の期限は `RunSupervisor::release_lease`。
  WU・統合の照合と、待ち（`checking`・`integrating`・`pending_subjects`）の扱いは `leases.rs` の入口に残した（D7: own-coord-b が `ExecutionCoordination` に移す）。
- D4 retry-backoff: `run_supervisor::infra_retry_after(n, max_infra_retries)` を 1 か所に定義し、worker 完了（`worker_finish.rs` の adapter の失敗・
  終了処理の失敗 2 か所）と lease の回収・shutdown の割り込み（`leases.rs`）が使う。`max_infra_retries` と直接比べる式は crate の本体に残っていない。
- D4 lease-renew: `renew_lease` の呼び元は変えていない（`Ok(false)` は通常の回収へ、`Err` は次の tick へ）。`run_holds_lease`・`orphan.rs` の
  `holder_gone`・`ownerless_run_decision` と tick の呼び出し順も変えていない。
- 消したもの: `Dispatcher::running_for_task`・`Dispatcher::stop_review`（呼び元は `RunSupervisor` の method を直接呼ぶ）、`dispatcher.rs` の `ORPHAN_WHY`
  （`run_supervisor` へ移した）、reclaim と終了処理の中の上限比較と backoff の書き込みの写し。
- `dispatcher/tests/structure_guard.rs` の `OWNER_STRUCTS` に `RunSupervisor` を足した。`structure_guard_run_supervisor` が D2 の 1〜3・7（field が private で
  7 件、`Dispatcher` を受けない・`dispatcher` module を使わない）を source から確かめる。
- `dispatcher.rs` の module map に `run_supervisor` の行（層 owner）を足し、`leases` の行を入口に直した（移した module の旧い行は無い）。
  ADR-0082 に付記、`docs/architecture-map.md` に owner の行を足し、ownerless の行の path に `run_supervisor.rs` を足した。ADR 2026-11-11 は変えていない。
- 基準は `$CELERIS_WU_BASE`。

## 実装前 brief

1. 根本原因: run の集合（`running`・`reviewing`）と backoff・照合の状態が `Dispatcher` の field で、`leases.rs`・`worker_finish.rs`・`dispatch_run.rs` ほか
   どの module でも書けた。`reclaim_expired_leases`（219 行）は回収の判断と lease の解放・終了記録・backoff の書き込みが 1 関数に混ざり、
   infra 再試行の上限比較が worker 完了と回収と shutdown で 5 か所に写されていた。
2. 責務 owner: `RunSupervisor`（`run_supervisor.rs`）と、その module の純粋関数（`reclaim_reason`・`reclaim_keys`・`reclaim_verdict`・`infra_retry_after`）。
3. 既存の再利用先: `task_ops::derive::infra_backoff_delay`・`consecutive_infra_requeues`、`TaskStore::apply_transition_with_events`・`renew_lease`、
   既存の lease・orphan・ownerless・review の試験と cex-lease の反例試験。
4. 選択肢: field を包んで `self.supervisor.running` と書くだけ（不採用。D2 の梱包だけ）／判断を純粋関数、副作用を owner の method、待ちは入口（採用）。
5. 新しい状態・分岐・fallback・port: 無し。`ReclaimReason`・`ReclaimVerdict`・`Reconcile` は値の束。
6. 撤去対象: 上の「消したもの」。
7. 不変条件と試験: 手元で生きている run の lease は回収しない（`cex_lease_restart`・`orphan_takeover`）、上限以内だけ再試行（`infra_retry_after_stops_after_the_limit`）、
   回収の理由と trigger（`reclaim_reason_follows_expiry_and_holders`・`reclaim_verdict_chooses_trigger_backoff_and_reason`・`reclaim_keys_selects_the_task_runs`）、
   照合の周期（`reconcile_due_tracks_each_interval`）、owner の形（`structure_guard_run_supervisor`・`structure_guard_owner_structs_have_no_test_fields`）。

## 試験の追加と変更

- 足した試験（`run_supervisor.rs`）: 各試験の doc comment に守る不変条件と既存の試験で検出できない理由を書いた。
- 変えた試験: field の直参照を method に置き換えた（読みは `runs()`・`reviews()`、handle を待つ・`since` を差し替える試験は `#[cfg(test)]` の
  `runs_mut_for_test`・`reviews_mut_for_test`、`clear_infra_backoff_for_test`・`reset_reconcile`・`watches_lost_reviews`）。assertion は変えていない。

## 証拠（提出時の HEAD で実行）

commit（件名）: 「phase 1: move the run table and lease supervision to RunSupervisor」「phase 1: keep run_supervisor out of the review dependency cycle」
「phase 1: own-supervisor の進捗に検査の結果を書く」。

- `bash scripts/dev/test-affected.sh`: exit 0。repo-health は verdict=pass fail_grade=0（new=1 は fail-grade でない）、nextest 3866 passed・11 skipped。
  1 回目は deps.cycle で exit 1（`run_supervisor` が `crate::review` を使い、base の dispatcher・review・undeclared_artifacts の循環に 4 つ目の節点として入った）。
  `ReviewEntry`・`RunSupervisor` が review の対象を型引数で受けるように直し（`Dispatcher` は `RunSupervisor<ReviewSubject>`）、2 回目で通った。
  `structure_guard_run_supervisor` は `crate::review` の語も拒む。
- `cargo nextest run -p task-dispatch`: 1811 passed（1 回目の commit の時点。試験の assertion は変えていない）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `python3 scripts/dev/check-architecture-map.py`: OK（1032 件）。
- `grep` で `self.running` などの 7 field の直参照が `run_supervisor.rs` の外（CoS chat と棚卸しの別の struct の同名 field を除く）に 0 件、
  `dispatcher.rs` の `Dispatcher` の field に 7 件のどれも無いことを確かめた。`max_infra_retries` と直接比べる式は crate の本体に 0 件（`infra_retry_after` の定義だけ）。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更は `crates/task-dispatch/`・`agent-docs/adr/0082-dispatcher-module-split.md`・`docs/architecture-map.md`・この進捗だけ。

## 未解決事項

- `holds_task_in_hand` と `abort_stale_runs`・`abort_all_runs` の中の `checking`・`integrating`・`pending_subjects`・`awaiting_children` の扱いは `Dispatcher` に残る（D7 の移行順のとおり own-coord-b が移す）。
- `Dispatcher::take_running_by_run_id` は routing の文脈の解放を伴うので `leases.rs` の入口に残した。

## 提案

- 無し。
