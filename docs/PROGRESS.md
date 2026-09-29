# PROGRESS — taskd

現在地: **Phase 119、Phase E6、Phase F4b まで本番反映（release c51837427ac5、schema 28）。F5-1 dogfood の 3 回目を準備中**。以後の追記は `docs/progress/phase-F.md` へ。

詳細な履歴と証跡は下記の分割ファイルを参照。既存の `docs/PROGRESS.md` 参照はこの目次を入口として維持する。

## 目次

- [Browser capability Phase 1](progress/phase-browser.md) — ADR-0078、既存 harness + agent-browser、管理者 grant・session・監査・dashboard 導線。最新 main 再統合後の gate 2026-09-28（Rust 2678 passed、GUI 1173 passed、mobile-audit 0 violations）。本番未昇格。

- [Phase 1–50（Phase 0 の初期記録を含む）](progress/phase-001-050.md)
- [Phase 51–100](progress/phase-051-100.md)
- [Phase 101–150](progress/phase-101-150.md)
- [Phase E](progress/phase-E.md)
- [Phase F](progress/phase-F.md) — 最終報告: [ADR-0074 Phase F 最終報告](execution-parallel-report-2026-09-28.md)（2026-09-28）。F6: 既存の Task / 案件を後から分解の経路に入れる（`POST /tasks/{id}/execution/decompose`・MCP `task_decompose`・retry の再判定）、案件の名前・説明の編集（2026-09-28、未昇格）
- [Phase R（再帰的な task 分解、ADR-0079）](progress/phase-R.md) — R0（設計: 節点は task だけ、段階の unit は leaf か子 task、max_depth 3、決定の要求、子は親ブランチへ取り込み、案件計画の廃止）完了 2026-09-28。R1a（plan/3 の型と検証、`Task.tree`、migration 0031 = **schema 31、昇格は stop → start**、木の Event 8 種、`[execution.tree]` 既定 `enabled = false`）完了 2026-09-28。R1b（kind task の unit からの子 task の生成・状態の写し・子を含む段階の完了・`awaiting_children`・subtree の中止の連鎖〈`parent_cancelled`〉・木での委譲の禁止・`review: human` の途中確認。migration なし）完了 2026-09-28。R1c（子のブランチ `celeris/<child_id>` を段階の基点から切る・統合 WU が子のブランチを親ブランチへ merge して `PhaseIntegrated.merged` に子を残す・子の最終レビューは親のブランチと比べる・子は `deliveries` / 人の取り込み〈409 `tree_child`〉/ `TaskReady` を持たず root だけが main へ。migration なし）完了 2026-09-28。R2a（深さの gate の閾値 `5 + gate_depth_step × (d − 1)`・木の子は shadow / off でも gate を採用・計画の採用時の unit の gate〈leaf ↔ task の上げ下げ、`UnitGateOverridden`〉・木の上限〈計画の段階・段階あたり・子 task・`max_depth`・木の leaf / run / replan / トークン〉の超過は `kind: limit` の決定の要求と `blocked(decision)` で超えた分だけを止める・深さ別の reviewer の run と定価を含む木の数え上げ。migration なし）完了 2026-09-29。R2b（/3 の planner に深さ・残りの深さ・leaf の基準・計画と木の残りの上限・祖先・`stages_hint`〈`Task.routing.stages_hint`〉を渡す・/3 の 2 回不正は atomic に倒さず `kind: plan_invalid` の決定の要求〈task は ready のまま run を止める〉・/3 の replan〈全体を書き done の unit は持ち越し〉・子の work の失敗 → 親の replan〈子の理由と checkpoint を planner へ、同じ unit から attempt + 1 の子〉・節点の `max_replans` 超過は `limit:max_replans`・子の基盤の失敗は 1 回だけ自動で作り直し、再度なら unit `blocked(infra)` と障害通知。migration なし）完了 2026-09-29。R3a（決定の要求の回答 API〈`GET /decisions`・`GET /tasks/{id}/decisions`・`POST /decisions/{id}/answer|withdraw|revise`〉と MCP `decision_list` / `decision_answer`〈`tasks:interact`〉・採用で計画の決定を path 付きの要求にし答えの無い決定に依存する unit だけを止める・回答の効き目は選択肢 → 効き目の表で決定的〈待つ unit の再開、limit の `raise-once`・`replan`・`withdraw`、plan_invalid の `replan`〈note を planner へ〉・`atomic`・`cancel`〉・答えを子の objective と leaf の前置きに固定の書式で注入・worker の `result.json` の `decisions`〈`self` だけがその unit を止める、上限超過は 1 件に束ねる〉・受信箱の `decisions` と件数・Discord 通知〈run ごとに束ね、24 時間後に 1 回だけ再通知〉。migration なし）完了 2026-09-29（worktree、main 未 merge）。R3b（root の /3 の計画は決定を含む・`review: human` の段階・上限の 0.8 以上のどれかで `awaiting_plan_approval` に止まり〈`PlanGate`・`PlanApprovalRequested`、unit を 1 つも起こさない〉、`POST /tasks/{id}/execution/plan-gate {action: approve|replan|withdraw}` と MCP `task_plan_gate`〈`tasks:interact`〉で応える・承認の要らない計画は報告の流れに 1 件だけ〈通知なし〉・受信箱の `attention.plan_approval` と通知 `plan_approval`〈計画の決定を束ねる〉・木の生存確認〈`task_core::tree::liveness`、理由なく止まった節点に `liveness_timeout_secs` 既定 600 秒で `StallDetected` と `tree-stall:` の障害通知を 1 回〉・人の replan の 1 回目が不正でも 2 回目の試行が起きる・木の子の `task_failed` と子の run の悪い知らせを鳴らさない。migration なし）完了 2026-09-29（worktree、main 未 merge）。次は R4a（木と roll-up の API）
- [Phase K（知識ベース）](progress/phase-K.md) — K-1（知識の置き場の整理と配置ガード。案件の `slug` = migration 0029）完了 2026-09-28（worktree、main 未 merge）
- [Phase G（ビルドキャッシュの 2 層化、ADR-0075）](progress/phase-G.md) — G0（設計）完了 2026-09-28。G1（scratch pool + semantic GC + celerisctl / metrics）完了・本番反映 2026-09-28。G2（sccache L1 の配線 + `CARGO_INCREMENTAL=0`）完了 2026-09-28。G3（L2: webdav の階層 cache server + flusher + L2 の GC + 監視）完了 2026-09-28（worktree、main 未 merge。cache server の有効化は人）。G3-fix1（継いだ `RUSTC_WRAPPER` / `SCCACHE_*` を run と checks から外す）完了 2026-09-28（worktree）。SD-1（release / verify の所要時間の短縮: 共有 target・GUI の段の skip・本番依存の cache・verify の所要時間）完了 2026-09-28（worktree）。SD-2（release の gate の `cargo-test` をテストバイナリ並列に: cargo-nextest 0.9.146、214 s → 115 s〈実行 202 s → 78 s〉）完了 2026-09-28（worktree）。SD-3（`truncate_phase_report` を挙動不変で O(n²) → O(S log n) に: 該当テスト 45.9 s → 0.02 s、並列 gate 69 s → 58 s）完了 2026-09-28（worktree）

各 Phase の詳細・証跡・申し送りは上記の分割ファイルを参照。既存の `docs/PROGRESS.md` 参照はこの目次を入口として維持する。

## F5-fix8: クラスタの ssh master の維持と切断の記録（ADR-0078）

2026-09-28 実装完了（未昇格、schema 30）。`ControlPersist=yes` の明示・鍵認証の再接続の抑制・切断の通知と回数（`cluster_connection_log`、
`GET /clusters` の `stats`）。[記録](progress/phase-F.md#f5-fix8-pegasus-の-ssh-master-を長く保ち無駄な再接続をやめ切断を数えて知らせるadr-00782026-09-28)。

## Phase F5-1 dogfood（再レビュー対応）

worker・review の完了を JoinHandle で明示同期し、実時間の待機回数に依存しない検証へ変更。
[実装と検証の記録](progress/phase-F.md#f5-1-review-repair)を参照。
