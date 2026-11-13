---
title: "健全性v2 R4 adr-align: ADR D1・D4 の表を field と共有判定の唯一の正本にし、担当 unit の列を足す"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-13
---
# 健全性v2 R4 adr-align: ADR D1・D4 の表を field と共有判定の唯一の正本にし、担当 unit の列を足す

完了日: 2026-11-13

## 概要

[ADR 2026-11-11-dispatcher-state-ownership](../../adr/2026-11-11-dispatcher-state-ownership.md) だけを直した。コードは変えていない。

- D1 の field → owner の表（116 行）の最後に unit 列を足した。unit 別の行数: own-cluster 25・own-maint 10・own-routing 15・
  own-hooks 10・own-quota 4・own-supervisor 7・own-coord-a 14・own-coord-b 16・own-orchestration 7・stay 8。
- 終わった 4 unit（own-cluster・own-maint・own-routing・own-hooks）の行は owner 列を実際の置き場にした。候補と違う置き場は 17 件
  （候補 composition-root の port 14 件、候補 test-hooks のクラスタの probe 2 件、`task_workspaces`）。`TestHooks` の行は owner 列に
  `TestHooks` の中の名（`now`・`ablation` の `EffectAblation` の `disable_write_set_gate` など）を書いた。
- まだ `Dispatcher` にある field は owner を変えず、unit 列を objective の規則で決めた。ExecutionCoordination の行の分け方:
  - own-coord-a（14）: `browser_ledger`・`browser_prereq_rescan`・`cluster_waiting`・`credential_busy`・`credential_held`・`integrated_deps`・
    `review_resync_halted`・`review_sync_queue`・`review_sync_seq`・`run_write_bases`・`write_reservations`・`write_set_hold_seq`・
    `write_set_waits`・`wu_prepare_failures`
  - own-coord-b（16）: `accepting_new_work`・`awaiting_children`・`awaiting_human`・`checking`・`dispatch_waits`・`integrating`・
    `last_tick_gap_ms`・`last_tick_started`・`liveness_checked_at`・`max_concurrency`・`pending_subjects`・`rx`・`stall_watch`・
    `stalled_tasks`・`ticks`・`tx`
  - `accepting_new_work`・`max_concurrency` は objective の列挙（write set・review の同期・統合の依存・WU 準備の失敗・credential・browser・
    cluster の待ち）に当たらないので「残り」として own-coord-b にした。`liveness_checked_at` は停滞の見張りの時刻なので own-coord-b。
- `ProbePorts` の module を `crates/task-dispatch/src/probe_ports.rs` に決め、owner 列と D1 の owner 表に書いた。owner 表は `TestHooks`
  （`dispatcher/tests/hooks.rs`）と `ProbePorts` の行を分けて 9 行にし、field 数を直した（30・7・19・25・12・2・3・8・10、計 116）。
  「移し済み」の列を足した。
- main の取り込み（sync-policy）で `Dispatcher` に増えた field は無い。統合 commit「integrate wu/own-hooks (phase owners-edge)」と
  提出時の HEAD で、`pub struct Dispatcher` の field の一覧が同じ（60 件。owner の struct の 4 件を除く 56 件が表の未移動の行と一致）。
- 付記 own-cluster・own-maint・own-routing・own-hooks の field の一覧を「置き場は D1 の表」に置き換え、移した理由だけを残した。
  食い違いを 2 件直した:
  - own-routing の付記の初版は `llm_route_registry`・`harness_adapters` を「表に無い」と書いたが、2 件とも表に候補 composition-root の行があった。
  - own-hooks の付記の初版は ablation の引数渡しを admit・review-decide が行うと書いたが、計画では write-set gate は own-coord-a、
    stale 優先は own-coord-b が行う。表の書き手の扱いと付記をこれに合わせた。
  - own-maint の付記の「ExecutionCoordination を作る unit が `task_workspaces` を移すかを決める」は、表の owner を `HostMaintenance`
    （unit own-maint）にしたので、「own-coord-a・own-coord-b は移さない」に改めた。
- D4 の表の最後に「共有の関数」と「担当 unit」の列を足し、各関数の契約を表の下に書いた:

  | id | 共有の関数 | 担当 unit | 今の実装 |
  |---|---|---|---|
  | continuation-resume | `sessions::stored_session_fresh_reason` | settle | task・WU は `sessions::decide_continuation`（呼ぶのは 1 か所）、CoS chat は `decide_cos_chat_session` が同じ種類の照合をする。両方が呼ぶ照合を新しく定義する |
  | retry-backoff | `run_supervisor::infra_retry_after` | own-supervisor | `max_infra_retries` との比較が `leases.rs` に 2 か所、`worker_finish.rs` に 3 か所 |
  | review-result | `review_decision::failed_checks` | review-decide | `review::failed_checks`（呼ぶのは `review_verdict.rs` の 1 か所）を移して WU の check と統合の check も使う |
  | run-close | `run_settlement::run_end_record` | settle | 無し。終了記録の組み立てだけで、run を閉じる関数にしない |
  | wu-failure | `run_settlement::replan_or_ask` | settle | `replans_so_far < self.effective_max_replans` の比較が `worker_finish.rs`・`work_units.rs`（2 か所）・`phase_integration.rs` にある |
  | lease-renew | none | none | `renew_lease` の store 操作は既に共通 |

  4 件の新しい名は、提出時の HEAD の `crates/task-dispatch/src` に同名の `fn` が無いことを grep で確かめた（後の unit の check は
  「定義が 1 file だけ」を見る）。`failed_checks` は今 `review.rs` に 1 つあり、review-decide が `review_decision` へ移す。
- D7 の移行順に adr-align と own-coord-a・own-coord-b を足し、owners-core の 5 unit を計画の依存どおり直列に書いた。own-supervisor から
  own-coord-b までは、lease の回収で消す待ち（`awaiting_human`・`checking`・`integrating`・`pending_subjects`）を `Dispatcher` の薄い入口が
  `RunSupervisor` の返す鍵で消し、own-coord-b が `ExecutionCoordination` の method に移すと書いた。
- ADR の状態の行を「採用（実装中）」にし、unit の一覧に own-coord-a・own-coord-b を足した。

## 変更
- `phase 1: adr-align: D1・D4 の表に担当 unit の列を足し、付記の field の割当てを表に畳む` — 上の ADR の変更と、この進捗ファイル。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- 計画の check 0（D1 の 116 行の unit 列・`Dispatcher` の field がすべて行にある・終わった unit の field が `Dispatcher` に無い）→ exit 0
- 計画の check 1（D4 の 5 行の `module::fn` と担当 unit、lease-renew は none none）→ exit 0
- 計画の check 2 `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- 計画の check 3（範囲。ADR とこの進捗の dir だけ）→ exit 0
- repo の検査の結果は下の「repo の検査」に書く。

## 未解決

- D4 の `sessions::stored_session_fresh_reason` は、今の `decide_continuation` と `decide_cos_chat_session` の照合の順序と理由が違う
  （task・WU は adapter・account・cwd を別の理由にし、cwd の違いは session を作らない理由になる）。settle は理由ごとの session の作成と
  引退を変えずに共有する必要がある。等価にできなければ settle が replan を申告する（ADR の契約に書いた）。

- `tree_units.rs` にも同じ比較（`replans_left`、節点の replan の残り）が 1 か所ある。D4 の wu-failure の行は equivalence の 3 経路
  （worker 完了・遅延 WU・統合）だけを挙げている。settle がこの比較も `run_settlement::replan_or_ask` で置き換えられるかを確かめる。

## 提案

- 無し。
