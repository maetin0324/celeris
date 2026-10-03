---
title: blocked repair WU による replan の繰り返しの修正（ADR-0134）
tasks: [01M3YMQ8G597TJ5J4H550QPW2P]
status: done
updated: 2026-10-02
---
# blocked repair WU による replan の繰り返しの修正（ADR-0134）

> 旧 `docs/PROGRESS.md`（現 `agent-docs/PROGRESS.md`）に main が足した節を ADR-0128 D6 に従い sync-main-2 migrate-docs（task 01M3Z8CXYG1J6BZCQ87YS3FC67）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

## blocked repair WU による replan の繰り返しの修正

完了日: 2026-10-02。ADR-0134 D1/D2 の実装に対し、dispatcher 再現試験
`blocked_repair_replan_loop_runs_the_new_leaf_once` を追加した。一時 git repo で段 `core` の統合検査を落とし、
daemon が作った `repair-core-1` が `blocked(plan_issue)` になる出来事を待つ。次の planner run が同じ段へ
`e2e-cancel` を足し、葉の実行で不正な file を直してから再統合する。最後に repair WU は `superseded`、
`e2e-cancel` と統合 WU は `done`、計画は v2 まで、planner run は 1 回だけであることを確かめる。
偽 adapter と出来事待ちだけを使い、負荷を掛ける時間依存試験にはしない。

- `cargo test -p task-dispatch --lib blocked_repair_replan_loop -- --nocapture` → exit 0（1 passed）。
- `cargo test --workspace` → exit 0（通常権限で全 workspace・doctest が通過）。最初の sandbox 内実行は
  exit 101 で `instance_handoff` 8 件中 5 件が失敗した。原因は worker DB guard の user namespace probe が
  `Operation not permitted` となったこと。通常権限で同じコマンドを再実行すると、この 5 件も通過した。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

未解決: ADR-0134 D3 の WU 単位の取り下げ・再開 API は未実装。planner が代わりの葉を足さない
場合は人による task 単位の replan が必要。

解消済み: `replay`（`task_ops::replay::rebuild_work_units_and_runs`）の replan 畳み込みが、
`execution.rs` の replan と同じ判定関数 `is_blocked_daemon_repair`（ADR-0134 D1）を呼ぶよう直した。
`WorkUnitTransitioned { to: superseded, reason: "replan v…" }` を見た時点でその行が
blocked daemon repair だったことを覚えておき、続く `ExecutionPlanned` の畳み込み
（`apply_replan_step`）でその key を `superseded` に確定し、統合 WU の `depends_on` からも外す。
試験 `replay_matches_live_after_blocked_repair_superseded`・
`replay_matches_live_after_blocked_repair_superseded_limit`（`crates/task-ops/src/replay/tests.rs`）
で、plan_issue と limit それぞれの場面で replay の work_units（key・status・depends_on 等）が live と
一致し、統合 WU の depends_on に superseded の repair key が残らないことを確認した。

- `cargo test -p task-ops --lib blocked_repair_superseded` → exit 0（5 passed: 上の 2 件 +
  `execution::tests::blocked_repair_superseded_on_limit`・`_on_plan_issue`・`_not_for_live_repair_units`）。
- `cargo test --workspace` → exit 0 相当。1 回目は `tests/e2e/tests/api_scenarios.rs` の
  `writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked`
  （時間依存の tick 待ち、instance_handoff とは別件）が `not all tasks finished` で落ちたが、
  `cargo test -p e2e --test api_scenarios writes_from_celerisctl_and_api_while_celeris_ticks_fast_never_hit_database_is_locked`
  を単独で再実行すると 1 passed で通った。共用 host の負荷による flaky で、本修正と無関係。
  その他は 507 passed（1 回目の集計、上記 1 件を除く）。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo test -p task-ops --lib blocked_repair_superseded` を再検証（run #3）→ exit 0（5 passed、上と同じ 5 件）。
  `cargo test -p task-dispatch --lib blocked_repair_replan_loop` → exit 0（1 passed）。
  `cargo test --workspace` の再実行では exit 101 だったが、落ちたのは `task-worker` の
  `browser_launcher_ptrace::launcher_chrome_denies_daemon_uid_ptrace` 1 件のみ（host の launcher binary
  が新しい protocol に未更新・run sandbox の userns 制約によるもので、本修正・instance_handoff とは別件。
  ADR-0115/0116 の既知事項）。他の全テストバイナリは `test result: ok`。
  `cargo clippy --workspace -- -D warnings` → exit 0（警告なし、再検証）。

### 人が task `01M3YF3NS2EGTZD2BBWNPG1K28` を再開する手順

1. この修正を含む release を人が作成・検証し、本番 daemon を人が差し替える。旧 daemon のまま再開しない。
   release/verify と daemon 差し替えは運用手順に従い、人が結果を確認する。
2. task 詳細で一時停止中と最新の計画版・承認待ちの有無を確認する。管理 API の
   `POST /api/v1/tasks/01M3YF3NS2EGTZD2BBWNPG1K28/resume`（空の本文）で一時停止を解除する。
   計画の承認待ちなら、`e2e-cancel` が段 `core` にあることを確認してから、同 task の
   `POST .../execution/plan-gate` に `{"action":"approve"}` を送る。既に計画が active で、
   `repair-core-2` が blocked のままなら、`POST .../execution/decompose` に
   `{"mode":"compound","note":"blocked repair を退役させ、e2e-cancel を走らせる"}` を送って
   1 回の replan を依頼し、新計画を確認して承認する。
3. `GET /api/v1/tasks/01M3YF3NS2EGTZD2BBWNPG1K28/task-tree` の該当節点で、
   `repair-core-2` の `status` が `superseded`、`e2e-cancel` が `done`、`integrate-core`
   が `done` になったことを確認する。task 詳細の run 履歴で `e2e-cancel` の worker run、
   計画履歴で新しい版への更新を確認する。版と planner run がさらに繰り返し増える場合は
   task を一時停止し、その run の結果と event を調べる。

### main merge（browser 試験の起動競合修正の取り込み）

main（ffb87b0d を含む）を `--no-ff` で merge した。merge-tree に衝突は無く、`docs/PROGRESS.md`
だけが自動 merge された。`cargo test -p task-worker --test browser_h3_wire` → exit 0（2 passed）、
`cargo test --workspace` → exit 0（全 test バイナリで 0 failed）、
`cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
