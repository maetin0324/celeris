# WorkUnit の並列・途中確認・案件計画・quota 指標（ADR-0074）Phase F: 最終報告

- 日付: 2026-09-28
- 対象: ADR-0074（`docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md`）Phase F0〜F5、F5-fix〜F5-fix5。
  関連として Phase G の G3-fix1・SD-1・SD-2（dogfood の配送と昇格に効いたもの）と Phase K-1（同じ期間の昇格）
- 前の報告: `docs/execution-decomposition-report-2026-09-25.md`（ADR-0072 E6。以下「E6 報告」）
- このレポートはコードを変えていない（`docs/` 配下だけ）。数値の出どころは次の 3 つで、本番には読み取りしかしていない。
  - `GET /api/v1/tasks/{id}/execution` と `GET /api/v1/tasks/{id}/events`（2026-09-28 14:4xZ に取得。コピーは
    scratchpad の `f5/*.json`）
  - `sqlite3 "file:/var/lib/celeris/celeris.sqlite3?mode=ro"` での events の集計
  - `~/.local/celeris/backups/promote-*.log` と `~/.local/celeris/releases/<sha>/promoted.json`
- 推測には「推測」と書く。値が取れないものは「未計測」と書き、理由を添える。

---

## 1. 目的と人の決定

E6 報告で分かったこと（WU が依存上は独立でも直列に走り 4h06m かかった、worker の lane が全部 standard だった、
費用を定価 USD でしか見られず codex の分が抜けていた、人が工程の途中で止めて見る手段が無い、案件の途中目標が直列の鎖しか持てない）
を受けて、人が 2026-09-25 に次の 4 点を決めた（ADR-0074 §1.2）。

| # | 人の決定（2026-09-25） | ADR-0074 の決定 |
|---|---|---|
| 1 | WU の並列は「WU ごとの worktree + 工程（phase）末尾の統合 WU」。並列数は `max_parallel_work_units`（既定 3）と provider の枠。統合は scheduler が決定的に `git merge`、衝突は repair WU（LLM）。lease と running の鍵は (task, WU) | D1 |
| 2 | 途中確認は `pause_after`（工程）。人と CoS が指定できる。止まったら `blocked(awaiting_human)` と途中報告、操作は「続ける / replan（指示つき）/ 取り下げ」。既定は全工程自動 | D2 |
| 3 | 案件レベルの計画は、案件直下の「マイルストーン Task」の DAG を CoS が起こし人が承認する（HUMAN GATE）。3 層固定（案件 → マイルストーン Task → ExecutionPlan → WU → Run）。大きすぎる WU は子 Task への分割を提案して既存の委譲に乗せる | D3 |
| 4 | 費用の主指標は quota の消費（定価 USD ではない）。Task / WU / run ごとに「どのアカウントの quota をどれだけ使ったか」を推定して記録する。定価 USD は参考値（`cost_usd_complete` 付き） | D4 |

これに E6 の個別の問題への対処（D5: WU ごとの lane と planner のコスト、D6: repair の分類と成果物の登録）を足したのが ADR-0074。

---

## 2. 決定 → 実装 Phase → main のコミット → release → 本番昇格

### 2.1 決定ごとの対応

| 決定 | 実装 Phase（main のコミット） | release | 実装の状態 | 残り（未実装・逸脱） |
|---|---|---|---|---|
| D1.1 schema `execution-plan/2`（phases） | F2(a)（worktree-agent-a5ad8285d9e9540a2 → main、`a770bcb` に含む） | a770bcb5b7b5 | 実装 | `max_work_units_v2` を別欄にした（逸脱 F2(a)-1）。`parallel = false` でも v2 の計画は採用される（§4.4 観察 1） |
| D1.2 WU ごとの worktree・完了時の commit | F2b（`e6cac7b`・`b42fccb`、merge `ddd6a5d`） | e3465764475c | 実装 | WU ごとの `CARGO_TARGET_DIR` は F5-fix で追加（ba2134a9fcc2） |
| D1.3 工程の中の並列・上限・公平性 | F2(c 下ごしらえ)・F2b | e3465764475c | **部分** | 並列数を Task（`NewTaskSpec.execution`）・CoS・profile で狭める経路が無い（`[execution] max_parallel_work_units` だけ。F2b 逸脱 12） |
| D1.4 統合 WU（決定的 merge・衝突 repair・検査の再実行） | F2b（`integration.rs`） | e3465764475c | 実装 | `PhaseIntegrated.checks[]` は `{cmd, pass, summary}`（exit code 無し。F2b 逸脱 14） |
| D1.5 鍵 (task, WU)・WU の lease・工程の lease | F2b（`cdcaa68`・`021512e`）、F5-fix2（検査・統合を in-flight に数える） | e3465764475c、af65cfb6592d | 実装 | — |
| D1.6 並列時の Task の遷移・D1.7 再起動の照合 | F2b、F5-fix2（lease 失効 run の result.json からの確定）、F5-fix3（`runs` 行を閉じる） | e3465764475c、af65cfb6592d、e50a8768c0dd | 実装 | Ready の v2 Task の Cancel では WU 行が cancelled にならない（F2b 申し送り） |
| D1.8 最終レビューは Task 単位 1 回 | F2b（変更なし） | e3465764475c | 実装 | — |
| D2.1 `PausePolicy`（人・CoS） | F3(pause) 区切り 1（`7b926b8`、merge `90d418e`） | 90d418e2036a | **部分** | `PUT /tasks/{id}/execution/pause-after` は未実装（`PATCH /tasks/{id}` で代用） |
| D2.2 `PhaseGate` / `PhaseResume` | F3(pause) 区切り 2（`8b39f8b`） | 90d418e2036a | 実装 | — |
| D2.3 途中報告（決定的） | F3(pause) 区切り 2 | 90d418e2036a | 実装 | `truncate_phase_report` が O(n²)（P-SD2-1。テストで 49 s） |
| D2.4 受信箱・操作・通知 | F3(pause) 区切り 3・4（`0ee84c1`・`7845d1a`） | 90d418e2036a | 実装（逸脱あり） | withdraw の reason は `cancel`（ADR は `withdrawn`。F3 逸脱 7）。replan 上限到達時は人の指示を `answers` に残して黙って進む（F3 逸脱 6） |
| D3.1 マイルストーン Task の述語と 1:1 | F4a checkpoint 1（merge `2035d34`） | 90d418e2036a | 実装 | — |
| D3.2 DAG と Go（reached まで待つ）・`auto_advance` | F4b checkpoint 1（migration 0028、merge `c518374`） | c51837427ac5 | 実装 | Go は `milestones.plan_key` のある途中目標だけ（F4b 逸脱 1） |
| D3.3 CoS の `project-plan/1`・人の承認 | F4a checkpoint 2・3（`c118844`） | 90d418e2036a | 実装 | `propose` は 1 トランザクションではない（F4a/F4b 申し送り） |
| D3.4 案件 replan（差分・同じ承認） | F4b checkpoint 2 | c51837427ac5 | **部分** | 起点 (c)「マイルストーン Task の failed / 取り下げで自動 replan」が未実装 |
| D3.5 GUI の案件ページの DAG | F4b checkpoint 5（`3c623ce`・`4ae0425`） | c51837427ac5 | 実装 | 辺は線でなく「← 依存先」（F4b 逸脱 7） |
| D3.6 途中目標の達成の意味 | F4b checkpoint 1・4 | c51837427ac5 | 実装 | dispatch で `in_progress` に上げる部分は dogfood 4 回目の成果（`ebe7008`）で追加 → 57efebe4fb7d |
| D3.7 planner の children → 委譲 | F4b checkpoint 3 | c51837427ac5 | 実装 | replan で新しい子を足さない。部またぎの質問は単体テストだけ |
| D3.8 互換 | F4a checkpoint 1、F4b checkpoint 4 | 90d418e2036a、c51837427ac5 | **部分** | 「CoS の `create_task` が案件直下に作る Task を draft にする」は実装を確認できなかった（`crates/task-ops/src/actions.rs::create_task_action` に draft 化の分岐が無い。U-F6 のまま）。**未確認**として扱う |
| D4.1〜D4.3 quota の観測・推定・記録・集計・GUI | F3(quota)（`36ee4be`・`da95565`・`2924e1a`、merge `b009c88`）、planner / reviewer の quota は dogfood 4 回目の成果（`ebe7008`） | a770bcb5b7b5、57efebe4fb7d | 実装（限界あり） | 較正と重なりの追跡はプロセス内メモリだけ（F3 逸脱 3）。`estimated` は 1 件も出ていない（§6） |
| D5.1〜D5.3 WU ごとの features・lane の上限・planner の lane / サイズ・replan の差分 | F1（`e4e922f`）、F5-fix3（上限を planner に渡す・拒否理由を再試行に渡す） | a770bcb5b7b5、e50a8768c0dd | 実装 | 差分は `phases` を足せない（F5-fix3 提案）。`WorkUnitPatch` は欄を消せない（F1 逸脱 4） |
| D6.1 `review_timeout`・D6.2 Task 内部の `merge_base` | F1（`7b3821c`） | a770bcb5b7b5 | **部分** | `RepairOrigin::Delivery` の `RepairScheduled` は発行していない（配送の repair は題名の接頭辞で分類。F1 逸脱 6） |
| D6.3 成果物の run 後の走査・D6.4 reviewer の `runs` 索引 | F1（`7b3821c`） | a770bcb5b7b5 | 実装 | 暗黙 WU の worker run の `runs.seq` が +1 ずれる別の不具合（F1 逸脱 7）は直したか本報告では**未確認** |

→ 決定のうち D1.3・D2.1・D3.4・D3.8・D6.2 が部分。**ADR-0074 の状態は「Partially implemented」**にした（§8 と ADR の状態行）。

### 2.2 release と本番昇格（2026-09-26〜28）

昇格の時刻は `~/.local/celeris/backups/promote-<日時>.log` の最終行（昇格の完了）。`promoted.json` が残っているのは現存する
4 つ（7667410c23d5・6ad50d1e7e7d・0633d1c91b96・181939898ec3）だけで、4 つとも log と一致した。それより古い release のディレクトリは
掃除で消えている（ログと phase-F.md / phase-G.md / phase-K.md の記録で確かめた）。

| release（main） | 中身（Phase） | schema | 昇格の完了（UTC） | 方式 | from |
|---|---|---|---|---|---|
| a770bcb5b7b5（a770bcb） | F1 + F2(a)(b) + F3(quota) | 27 | 2026-09-26 08:26:45 | stop-start | 5cc1610938f5 |
| e3465764475c（e346576） | F2b（WU 並列の本体）+ F5-1 dogfood 1 回目の 3 成果（ディスク残量ゲート、codex cache usage、API 文書）+ 配送準備の修復 | 27 | 2026-09-26 12:37:38 | live | a770bcb5b7b5 |
| 90d418e2036a（90d418e） | F3(pause)（shadow でも人の明示 compound を採用、を含む）+ F4a | 27 | 2026-09-27 13:17:22 | live | e3465764475c |
| c51837427ac5（c518374） | F4b + planner の `permission_mode` 既定を `bypassPermissions` に | 28 | 2026-09-27 15:31:12 | stop-start | 90d418e2036a |
| 353d32fbe0ea（353d32f） | F5-1 dogfood 2 回目の成果（EVENT_TYPES、フレーク 5 件の決定化、PROGRESS.md の分割） | 28 | 2026-09-27 15:52:14 | stop-start | c51837427ac5 |
| ba2134a9fcc2（ba2134a） | F5-fix（replan で daemon の統合 WU を不変条件から外す、WU ごとの `CARGO_TARGET_DIR`） | 28 | 2026-09-28 02:26:06 | live | 353d32fbe0ea |
| （参考）10bb975a731a / 89854b08d3b1 / b8ca4eefda28 | G1 / G2 / G3（dogfood 4 回目の最中のライブ切替） | 28 | 03:42:16 / 05:39:00 / 06:54:33 | live | — |
| af65cfb6592d（af65cfb） | F5-fix2（WU の checks と統合を in-flight に数える、lease 失効 run を result.json から確定） | 28 | 2026-09-28 09:24:03 | live | b8ca4eefda28 |
| e50a8768c0dd（e50a876） | F5-fix3（planner に計画の上限と前回の拒否理由、`WorkerFinished` で `runs` 行を閉じる）+ F5-fix4（codex に worktree の gitdir / common dir を `--add-dir`） | 28 | 2026-09-28 10:32:54 | live | af65cfb6592d |
| 57efebe4fb7d（57efebe） | G3-fix1（継いだ `RUSTC_WRAPPER` / `SCCACHE_*` を外す）+ dogfood 4 回目の成果の人による統合（`ebe7008`: API 文書、途中目標の `in_progress`、planner / reviewer の quota） | 28 | 2026-09-28 11:02:23 | live | e50a8768c0dd |
| 7667410c23d5（7667410） | F5-fix5（headless の claude-code の background task 対策） | 28 | 2026-09-28 12:06:01 | live | 57efebe4fb7d |
| 6ad50d1e7e7d（6ad50d1） | SD-1（release / verify の短縮） | 28 | 2026-09-28 13:07:33 | live | 7667410c23d5 |
| 0633d1c91b96（0633d1c） | K-1（知識の置き場のガード、案件の slug、migration 0029） | 29 | 2026-09-28 13:26:44 | stop-start | 6ad50d1e7e7d |
| 181939898ec3（1819398） | SD-2（release gate の `cargo-test` を nextest でバイナリ並列に） | 29 | 2026-09-28 14:33:36 | live | 0633d1c91b96 |

- K-1 は Phase F の dogfood から出たものではない（ChatGPT の MCP client が案件 ID の下に知識を置いた人の報告が起点。
  `docs/progress/phase-K.md`）。同じ期間の昇格として並べただけ。
- SD-1 / SD-2 の効果: release 開始から昇格完了まで SD-1 前の約 18〜21 分 → 181939898ec3 で 8 分 49 秒（phase-G.md SD-2）。
  dogfood 4 回目の最中の 1 日 5 周の修正と昇格の往復が、これで短くなった。

---

## 3. 受け入れ条件ごとの証拠と判定

判定: **満たす** = テストで確かめ、本番で反する観測が無い / **部分** = 一部だけ / **未** = 行っていない。
テスト名は phase-F.md の各節の記録から。「本番」はこの報告で events を数えて確かめたもの。

### F1（ADR-0074 §7 F1）

| 条件 | 証拠 | 判定 |
|---|---|---|
| (a) planner プロンプトに WU ごとの `features` | `claude_code::tests::execution_plan_prompt_asks_for_per_unit_features` ほか 2 passed | 満たす |
| (b) 読めない `features` は検証エラー → 1 回再試行 → atomic | `features_must_parse_as_task_feature_hints`、`plan_with_unreadable_features_retries_once_then_falls_back_to_atomic` | 満たす |
| (c) 機械的な WU は cheap、上限で丸めて `clamped_by` | `model_policy::tests::*` 6 passed。本番: dogfood 4 回目の worker 19 run のうち cheap 11（`cheap/mechanical-verifiable-reversible`）、dogfood 2・3 回目で `clamped_by: "work-unit lane cap (task lane cheap): frontier -> standard"` を 3 run で観測 | 満たす |
| (d) planner は standard（`planner/system-standard`）・24 turn / 900 s | `planner_run_uses_the_standard_lane_by_default` ほか。本番: dogfood 2〜4 回目の planner 11 run すべて `rule_id = planner/system-standard`（うち 2 run は quota 層で cheap に降格） | 満たす |
| (e) サイズ上限の超過 → 拒否 → 再試行 → atomic | `plan_size_limits_*` 3 件、`oversized_plan_retries_once_then_falls_back_to_atomic`。本番: dogfood 4 回目で `too many checks: 8 > 6` が 2 回 → replan なので blocked（ADR どおり）。再試行が同じ計画を再提出した件は F5-fix3 で修正 | 満たす |
| (f) replan の差分出力、reason に件数 | `replan_delta_carries_done_units_without_restating_them` ほか。本番: dogfood 4 回目の v2 `(added=0, changed=5, removed=1)`、v3 `(added=0, changed=2, removed=0)`、v4 は全体形式 `(added=1, changed=0, removed=0)` | 満たす |
| (g) `review_timeout` の 2 倍再実行 → repair | `review_timeout_reruns_once_with_double_timeout_before_repair` ほか 4 件。本番では未発生 | 満たす |
| (h) `merge-base --is-ancestor` → 決定的 merge / repair | `merge_base_failure_merges_base_deterministically` ほか 3 件。本番: dogfood 2 回目の配送で `repairs_by_class: {merge_base: 1}` | 満たす |
| (i) `RepairScheduled` が残り `unknown` が出ない | `repair_scheduled_event_names_the_class` ほか 5 件。本番: 2026-09-26 以降の `repair_scheduled` は 1 件（class `planner`、origin `planner`）。dogfood 1〜4 回目の `repairs_by_class` に `unknown` は 0 | 満たす |
| (j) git worktree の Task の `report.md` を `ArtifactProduced{declared:false}` に | `git_worktree_task_registers_report_md_as_an_artifact` ほか 4 件。本番: dogfood 4 回目 `artifact_produced` 4 件 | 満たす |
| (k) reviewer run の `runs` 索引の欠落 | `reviewer_run_usage_is_recorded_in_the_runs_index_and_survives_replay_check`。別経路（lease 失効の requeue）の `running` 残りは F5-fix3 で修正、本番の 7 行は人の判断で UPDATE、現在 `runs.status = 'running'` は 0 行 | 満たす |

### F2（§7 F2）

| 条件 | 証拠 | 判定 |
|---|---|---|
| (a) execution-plan/2 の検証、v1 不変 | `execution_plan::` 37 passed（新規 20） | 満たす |
| (b) migration 0027 | `migration_27_adds_work_unit_lease_columns`。本番 schema 27 → 28 → 29 | 満たす |
| (c) 独立 WU 3 本の並列、WU ブランチで commit、統合で `PhaseIntegrated` | `three_independent_units_run_in_parallel_and_integrate` ほか 5 passed。本番: dogfood 3・4 回目で worker の最大同時 3、`work_unit_committed` 9 件、`phase_integrated` 4 件（2 Task） | 満たす |
| (d) 積み上げ | `stacked_unit_branches_from_its_dependency` | 満たす |
| (e) 衝突 → `merge-<phase>-<key>` の repair → 続きから再開 | `integration_conflict_creates_a_merge_repair_and_resumes`、`merge_is_idempotent_after_restart`。本番: 衝突 0 件（未発生）。統合の冪等なやり直し（`skipped: true`）は dogfood 3・4 回目で観測 | 満たす |
| (f) 統合後の検査の失敗 → repair / replan | `integration_check_failure_is_repaired_when_classified`、`..._replans_when_not_classified` | 満たす |
| (g) 兄弟の in-flight を待ってから遷移 | `sibling_failure_waits_for_in_flight_units_before_replan` ほか | 満たす |
| (h) 再起動の照合 | `parallel_units_survive_dispatcher_restart`、`an_interrupted_integration_is_redone_idempotently_after_restart`。本番で「検査中のライブ切替で完了を失う」穴が見つかり F5-fix2 で修正（`draining_dispatcher_keeps_work_unit_checks_in_flight_until_the_completion_is_recorded` ほか 5 件） | 満たす |
| (i) Cancel で全 WU の run が止まる | `cancel_stops_every_work_unit_run` | 満たす |
| (j) 公平性 | `ready_task_first_run_beats_a_second_parallel_unit` | 満たす |
| (k) remote / dir / Shared は並列 1 | `shared_workspace_falls_back_to_serial`、`remote_workspace_falls_back_to_serial`。本番で `work_units_serialized` は 0 件（該当 Task なし） | 満たす |
| (l) GUI の工程の列・同時 run・ブランチ | `gen:types` 差分 0、`mobile-audit` 違反 0、GUI test 1099 passed | 満たす |

### F3（§7 F3。途中確認 (a)〜(f) と quota (g)〜(k)）

| 条件 | 証拠 | 判定 |
|---|---|---|
| (a) `PausePolicy` を人と CoS が書け、採用時に `PausePointsResolved` | 区切り 1 の 7 テスト + `pause.rs` 4 テスト。本番: `pause_points_resolved` 5 件（2 Task）、**すべて `phases` 空**（どの dogfood も `pause_after` を指定していない） | 満たす（本番で停止点のある解決は未観測） |
| (b) `PhaseGate`・`PhaseReported`・`phase-reports/<n>-<phase>.md` | `pause_after_design_blocks_with_awaiting_human`。本番: `awaiting_human` の遷移 0 件、`phase_reported` 0 件 | 満たす（本番未観測） |
| (c) 受信箱の `PhaseCheckpoint`、通知 1 回、`QuestionBlocked` なし | `notify::tests::phase_checkpoint_is_not_a_question`、`inbox_phase_checkpoint_is_attention_not_a_question` | 満たす（本番未観測） |
| (d) continue / replan（note 必須）/ withdraw、409 / 422 | `phase_gate_continue_resumes_the_next_phase`、`phase_gate_replan_requires_a_note`、`task-api --test execution phase_gate` 3 件 | 満たす（本番未観測） |
| (e) v1 / atomic で無害 | `adopt_plan_resolves_no_pause_points_for_v1_plans` | 満たす |
| (f) GUI の途中報告と 3 ボタン、受信箱 | GUI test 1106 passed、`mobile-audit` 違反 0 | 満たす |
| (g) `QuotaEstimated` の 5 方式を決定的に | `quota::` 21 passed。本番: 2026-09-26 以降 `quota_estimated` 51 件（17 Task）。method は measured / apportioned / unknown / free が出て、estimated は 0 件 | 満たす |
| (h) `before` の有効性 | `before_is_valid_table`、`accounts::quota_activity_tests::` 9 passed | 満たす |
| (i) `ExecutionMetrics.quota`・`cost_usd_complete`・WU ごと・`accounts_now` | `execution_metrics::` 18 passed。本番 API の `metrics.quota` と `cost_usd_complete: false` を 4 dogfood で確認 | 満たす |
| (j) GUI: quota が主、定価は参考、unknown は「不明」 | `task-execution.test.ts`、`mobile-audit` 違反 0 | 満たす |
| (k) quota で dispatch・lane・アカウントの選択が変わらない | `quota_bookkeeping_does_not_change_account_or_lane_selection` | 満たす |

### F4（§7 F4）

| 条件 | 証拠 | 判定 |
|---|---|---|
| (a) `is_milestone_task` と 1:1 | `is_milestone_task_requires_project_root_execute_position` ほか 3 件 | 満たす |
| (b) `mode: milestones` の秘書 run → 提案・途中目標・draft・受信箱 | `milestones_project_plan_task_proposes_milestones_and_top_level_draft_tasks` ほか。本番: `project_plan_proposed` 0 件（案件計画は本番で一度も起こしていない） | 満たす（本番未観測） |
| (c) approve / reject | `approve_readies_the_whole_dag_in_one_transaction` ほか、API 202 / 422 / 404 / 409 | 満たす（本番未観測） |
| (d) reached まで待つ、`ok` で Go、`auto_advance` | `dependent_milestone_waits_for_reached_not_done` | 満たす（本番未観測） |
| (e) 案件 replan の差分と同じ承認 | `project_replan_delta_cannot_modify_a_started_milestone` ほか | 満たす（本番未観測） |
| (f) planner の children → 子 Task、`child:<key>` | `planner_children_become_delegated_child_tasks`、`plan_children_runs_the_delegation_checks_and_asks_before_crossing_departments`。本番: `delegated` 0 件 | 満たす（本番未観測） |
| (g) 案件計画の無い案件の互換 | `legacy_project_keeps_linear_milestones` | 満たす |
| (h) GUI の DAG | `project_detail_carries_the_plan_dag_and_the_pending_proposal`、GUI test 1113 passed、`mobile-audit` 違反 0 | 満たす |

### F5（§7 F5。dogfood）

| 条件 | 証拠 | 判定 |
|---|---|---|
| F5a: F1 の後、E6 と同規模の自己改善 Task を流し、lane の分布・planner の時間と出力・replan の大きさ・repair の class・成果物・attempts・壁時計・定価を E6 と並べる | 4 回流した（§4）。ただし本番は `gate = "shadow"` のまま（`gate = on` への切り替えは人の判断で行っていない）で、人の明示 compound で走らせた。done まで compound で完走した回は無い（1・2 回目は atomic、3 回目 cancelled、4 回目 failed で成果は人が統合） | 部分 |
| F5b(1): `parallel = true` で工程の並列・統合の衝突・壁時計を測る | 3・4 回目で並列（最大同時 3）と統合を観測。ただし本番の `[execution] parallel` は既定の `false` のまま（§4.4 観察 1）。統合の衝突は 0 件、並列 WU の中のビルド待ち（U-F2）は測っていない | 部分 |
| F5b(2): 案件計画（2〜3 マイルストーン、独立 / 直列を混ぜる） | 行っていない（本番の `project_plan_proposed` 0 件） | 未 |
| F5b(3): `pause_after` を 1 か所で流し途中確認を往復 | 行っていない（本番の `awaiting_human` 0 件） | 未 |

### 集計

| 区分 | 条件の数 | 満たす | 部分 | 未 |
|---|---|---|---|---|
| F1 | 11 | 11 | 0 | 0 |
| F2 | 12 | 12 | 0 | 0 |
| F3 | 11 | 11 | 0 | 0 |
| F4 | 8 | 8 | 0 | 0 |
| F5 | 4 | 0 | 2 | 2 |
| **計** | **46** | **42** | **2** | **2** |

F3 (a)〜(d) と F4 (b)〜(f) の 9 件は「テストでは満たすが本番では一度も通っていない」。F5b(2)(3) の未と同じ原因。

---

## 4. dogfood の比較（E6 と F5-1 の 1〜4 回目）

### 4.1 対象

| 回 | Task | 期間（UTC） | 開始時の release | 内容 |
|---|---|---|---|---|
| E6 | 01M3C33KW8YH336QDD0QAV45H8 | 2026-09-25 10:52〜14:58 | （E6 報告） | 配送の局所修復・単価表・metrics の集計性能 |
| 1 | 01M3EDF3JEHRQCG6A2EJDRQMXJ | 2026-09-26 08:31〜12:26 | a770bcb5b7b5 | ディスク残量チェック・codex cache usage・API 文書 |
| 2 | 01M3HG7VV6A2HRHTNXWPDS9051 | 2026-09-27 13:18〜15:50 | 90d418e2036a | EVENT_TYPES・フレーク 5 件の決定化・PROGRESS 分割 |
| 3 | 01M3HS2E19BRC021ZXMDZANP5B | 2026-09-27 15:52〜20:18（16:32 から blocked） | 353d32fbe0ea | planner / reviewer の quota・途中目標 `in_progress`・API 文書 |
| 4 | 01M3JXB3DHVBWKWKPW04DTG6SJ | 2026-09-28 02:26〜10:24 | ba2134a9fcc2（途中で 5 回ライブ切替） | 3 回目と同じ 3 成果 |

- 2 回目の Task id: phase-F.md の見出しの `01M3HJYF9ZN09GE0ZFT9VT4V8J` は `GET /tasks/{id}` が 404 で、実際には
  01M3HG7VV6A2HRHTNXWPDS9051 の worker **run** の id だった（その Task の `quota_estimated.run_id`）。Task は 01M3HG7VV6… を使う。
- E6 の値は E6 報告の表（2026-09-25 14:58 の done の時点）を使う。E6 の Task はその後 reopen・質問を経て 2026-09-27 16:27Z に
  cancelled になっており、いまの `GET /tasks/01M3C33K…/execution` は wall 192,905,257 ms・planner 6 run など報告後の分を含むので使わない。
- 4 回とも `execution_gated{mode: compound, source: human, rule_id: human/explicit}`（人の明示）。規則表の判定は一度も起きていない。

### 4.2 指標

| 指標 | E6 | 1 回目 | 2 回目 | 3 回目 | 4 回目 |
|---|---|---|---|---|---|
| 実行方式 | compound（v1、8 WU、3 版） | **atomic**（shadow が人の明示を捨てた） | **atomic** に倒れた（planner 2 回失敗 → `atomic/planner-invalid`）。配送の repair で WU 2 | compound（v2、3 工程、WU 9〈統合 3〉、1 版） | compound（v2、3〜4 工程、WU 行 12〈統合 5〉、4 版） |
| 終了状態 | done | done | done | cancelled（blocked から人が取り消し） | failed（成果は人が main に統合） |
| 壁時計（`wall_ms`） | 4h06m04s | 3h54m40s | 2h32m43s | 4h25m46s（動いていたのは 40m16s、残りは blocked） | 7h57m53s |
| run 数（計） | 14 | 7 | 14 | 10 | 26 |
| 　planner / worker / reviewer | 3 / 8 / 3 | 0 / 4 / 3 | 2 / 6 / 6 | 3 / 7 / 0 | 6 / 19 / 1 |
| run の時間の和: worker | 3h20m16s | 2h46m15s | 1h47m33s | 47m41s | 6h27m11s（うち lease 失効で捨てた 2 run が 4h05m05s） |
| run の時間の和: planner | 26m53s | — | 9m22s（2 run とも失敗） | 7m07s | 8m36s |
| planner の出力トークン | 約 95k（44k / 37k / 14k） | — | 未計測（2 run とも usage 無し） | 26,665 | 37,094（6 run、平均 6,182） |
| planner 1 run の平均時間 | 538 s | — | 281 s | 142 s | 86 s |
| lane: planner（frontier / standard / cheap） | 3 / 0 / 0 | — | 0 / 1 / 1（cheap は quota 層の降格） | 0 / 2 / 1（同） | 0 / 6 / 0 |
| lane: worker（frontier / standard / cheap） | 0 / 8 / 0 | 0 / 4 / 0 | 1 / 2 / 3（frontier は retry のエスカレーション） | 0 / 6 / 1 | 0 / 8 / 11 |
| lane: reviewer | 3（claude-opus-5-5。lane は報告に記載なし） | standard 3 | standard 6 | — | standard 1 |
| worker の `rule_id` | 全部 `standard/default` | 全部 `standard/default` | `cheap/mechanical-verifiable-reversible` 5、`frontier/judgment-under-uncertainty` 1（上限で standard） | `standard/default` 5、`frontier/judgment-under-uncertainty` 2（上限で standard、1 つは quota で cheap） | `cheap/mechanical-verifiable-reversible` 11、`standard/default` 8 |
| WU の並列 | 無（直列。依存上は独立 3 系列） | 無 | 無 | **有**（最大同時 3） | **有**（最大同時 3） |
| 並列の効き目（工程の壁時計 / run の時間の和） | — | — | — | investigate: 8m36s / 11m57s | build: 18m22s / 45m21s |
| 統合（`phase_integrated`） | — | — | — | 1（investigate） | 3（build・gate・report） |
| replan（採用された数） | 2 | 0 | 0 | 0（起動 1、planner の拒否 2 → blocked） | 3 |
| retry（WU） | 0 | 0 | 0 | 2 | 6 |
| continuation | 0 | 1 | 1 | 0 | 0 |
| repair（class） | 1（unknown） | 0 | 1（merge_base、配送） | 0 | 1（planner） |
| Task の attempts | 推測 2（E6 報告 注2） | 0 | 0 | 0 | 2 |
| 入力 / cache read / 出力トークン | 65.12M / 5.30M / 252k | 54.77M / 3.19M / 98.6k | 63.01M / 61.81M / 122.6k | 3.66M / 13.74M / 92.1k | 10.70M / 21.13M / 104.9k |
| 定価 `cost_usd`（参考） | $11.21 | $4.63 | 未計測（metrics に `cost_usd` 欄が無い: 単価のある run に usage が無かった） | $4.54 | $5.94 |
| `cost_usd_complete` | （欄なし。pre-F3） | false | false | false | false |
| quota（アカウント × 窓の合計と method） | 未計測（F3 より前） | codex 7d 0.0pt（measured 4）、5h 不明 4 | codex 7d 3.0pt（measured 6）、5h 不明 6 | claude 5h 4.0pt / 7d 1.0pt（measured 1・apportioned 2）、codex 7d 1.0pt（measured 4）、5h 不明 4 | claude 5h 6.0pt（apportioned 2・不明 1）/ 7d 7.0pt（measured 1・apportioned 2）、codex 7d 1.0pt（measured 14）、5h 不明 14 |
| 人の介入 | 報告に記載なし | answer 0（reopen 1。配送の再試行、誰の操作かは events から判別できない） | answer 0（reopen 2。2 回目は `delivery_repair` の計画を伴う） | cancel 1（phase-F.md は「人の操作と思われる」） | answer 2（計画の上限の提示、昇格実行の指示）、成果の手動統合 1、journald の復旧 1 |
| 人を待った時間 | — | — | — | 3h45m29s（16:32:36 → 20:18:05、そのまま cancel） | 1h20m42s（08:19:42 → 09:25:15、09:34:54 → 09:50:03） |

出どころ: 1〜4 回目は `GET /tasks/{id}/execution` の `metrics` と `runs`、lane と `rule_id` は各 run の `routing_decided`、
並列と工程の時間は `runs[].started_at / finished_at`、人の介入と待ち時間は `transitioned`（reason `answer` / `cancel` / `reopen`）
と `answered`、手動統合と journald の復旧は phase-F.md の記録。

### 4.3 見出しの数字

- **lane の分布が変わった**: worker の cheap の割合は E6 0/8 → 4 回目 11/19（58%）。planner は frontier 3 run（E6）→ standard 6 run（4 回目）。
- **planner は軽くなった**: 1 run の平均 538 s → 86 s、1 run の出力は E6 平均約 32k → 4 回目平均 6.2k トークン（replan の差分形式と
  「計画 run でコードを調べない」指示、D5.3）。
- **並列は効いた**: 4 回目の build 工程は 3 本並列で 18m22s（run の時間の和 45m21s。約 2.5 倍）。3 回目の investigate 工程は約 1.4 倍。
- **それでも壁時計は縮まなかった**: 4 回目は 7h57m53s（E6 4h06m04s）。内訳の大きいもの: F5-fix2 の不具合で捨てた gate run 2 本
  4h05m05s、人を待った 1h20m42s。この 2 つを除くと約 2h32m（推測。単純な差し引きで、捨てた run と並行して別の作業は走っていない）。
- **完走率は下がった**: E6 は done。F5-1 は compound で done まで行った回が無い（1・2 回目は atomic で done、3 回目 cancelled、4 回目 failed）。
  止めたのはすべて基盤の不具合（§5）で、4 回目の reviewer は内容の基準 4 を合格にしている。

### 4.4 観察（指標の読み方に効くもの）

1. **本番は `[execution] parallel` が既定の `false` のまま**（`~/.config/celeris/config.toml` の `[execution]` は `gate = "shadow"` だけ）。
   それでも 3・4 回目で v2 の計画が採用され並列に走った。Task の objective が「execution-plan/2（phases）で書き」と頼み、検証は
   `parallel` の値に関わらず v2 を受け付けるため。ADR D1.1 の「planner に v2 を出させるのは `parallel = true` のときだけ」は
   プロンプトだけに効いていて、採用の関門にはなっていない。既定を変えるかの判断（F5 の人の判断）をするなら、先にこの扱いを決める必要がある（§7 P-F-1）。
2. gate の規則表は 4 回とも評価されていない（人の明示）。2026-09-26 以降の本番の `execution_gated` は 5 件で、4 件が `human/explicit`、1 件が
   `atomic/planner-invalid`。E6 報告 §5 の「shadow の記録が溜まらない」は変わっていない。
3. 3 回目の `replans` は metrics で 0 だが、phase-F.md は「replan 2」と書いている。metrics は採用された版の数、phase-F.md は planner の
   試行の数（2 回とも拒否）を数えている。
4. 2 回目の worker は phase-F.md では 4 run だが API では 6 run（reopen の後の配送の repair と再作業の 2 run を含む）。表は API の値。

---

## 5. 見つかった不具合と修正

dogfood 3 回目で 2 件、4 回目で 5 件（F5-fix2・F5-fix3 の 2 件・F5-fix4・G3-fix1）、4 回目の run の解析で F5-fix5 を 1 件。
1・2 回目の発見（shadow が人の明示を捨てる、planner が Plan Mode で成果物を書けない）も同じ形で並べる。

| # | 見つけた回 | 症状 | 原因 | 修正 | release（昇格） |
|---|---|---|---|---|---|
| 1 | 1 回目 | 人の明示 compound が atomic で走った | `gate = "shadow"` が source = human の判定も一律に記録だけにしていた | shadow でも `source = human` の compound は採用（F3(pause) 区切り 0、`shadow_gate_adopts_a_human_explicit_compound_decision`） | 90d418e2036a（09-27 13:17） |
| 2 | 1 回目 | 終端した workspace に `repos/*/target` 25G | `CARGO_TARGET_DIR` を受け取らない cargo の経路がある（P-F5-1） | WU ごとの target（F5-fix）、scratch pool と semantic GC（G1）で置き場を一本化。`repos/*/target` の prune そのものは未確認 | ba2134a9fcc2 / 10bb975a731a |
| 3 | 2 回目 | planner 2 run が `result.json` を書けず atomic に倒れた | `[execution.planner].permission_mode` の既定 `plan` で claude-code が Plan Mode に入り Write できない | 既定を `bypassPermissions` に | c51837427ac5（09-27 15:31） |
| 4 | 3 回目 | 並列 WU の検査で偽のコンパイルエラー（E0609）→ retry 2 → failed | 兄弟 WU が同じ `CARGO_TARGET_DIR` を共有 | WU ごとの `<build_cache_dir>/cargo/<repo-key>/wu-<id>`、終端で削除、daemon の検査にも同じ値（F5-fix、`parallel_work_units_get_their_own_cargo_target_dir_and_it_is_removed_when_done`） | ba2134a9fcc2（09-28 02:26） |
| 5 | 3 回目 | replan の差分が「done work unit integrate-investigate must not change」で 2 回拒否 → blocked | dispatcher 側の検証が daemon の足した統合 WU を done の不変条件に入れていた | `is_daemon_added_work_unit` / `replan_done_work_units` を共有（F5-fix、`replan_delta_after_an_integrated_phase_keeps_the_daemon_integration_unit`） | ba2134a9fcc2 |
| 6 | 4 回目 | gate WU の run が result.json を残したのに完了が記録されず、111 分後に lease 失効で捨てられた（2 回、1.59M トークン × 2、4h05m） | WU の checks と工程の統合が `in_flight()` に数えられず、ライブ切替で draining の旧デーモンが検査の途中で exit | `checking` / `integrating` を in-flight に、lease 失効 run は result.json から確定、確定の失敗を記録（F5-fix2、テスト 5 本） | af65cfb6592d（09-28 09:24） |
| 7 | 4 回目 | replan の planner が checks 8 本（上限 6）を 2 回出して blocked | planner に上限が渡っていない、再試行に拒否理由が渡らない、拒否した `execution-plan.json` が残り再提出された | 上限を 1 か所（`ExecutionConfig.limits`）からプロンプトへ、`previous_attempt_errors`、`execution-plan.rejected.json` へ退避（F5-fix3） | e50a8768c0dd（09-28 10:32） |
| 8 | 4 回目 | lease 失効で requeue した run の `runs` 行が `running` のまま | requeue の経路が `run_index_finish` を呼ばない | `WorkerFinished` を書く同じトランザクションで行を閉じる、止めた run に `WorkerFinished{interrupted}`（F5-fix3）。本番の既存 7 行は人の判断で UPDATE（backup `20260928-112219-pre-runs-update.sqlite3`） | e50a8768c0dd |
| 9 | 4 回目 | repair WU `sync-main` の `git merge main` が `ORIG_HEAD` read-only で 2 回失敗 → blocked | codex の `workspace-write` が cwd と `--add-dir <artifacts>` しか書けず、worktree の gitdir / common dir に書けない | fresh `exec` に gitdir と common dir を `--add-dir`（F5-fix4、`codex sandbox` で修正前 exit 128 / 修正後 exit 0 を確認） | e50a8768c0dd |
| 10 | 4 回目 | `sync-main` の checks の `cargo test --workspace` が cache-server-down の 2 テストで必ず落ちる → Task failed の直接原因 | daemon から継いだ `RUSTC_WRAPPER` が run と checks の子に漏れる | sccache を配線しないとき族を `env_remove`（G3-fix1、`inherited_sccache_env_does_not_leak`） | 57efebe4fb7d（09-28 11:02） |
| 11 | 4 回目（run の解析） | Sonnet の gate run が 55 秒で result.json 無しに終わった | headless の `claude -p` が `cargo test` を background task にして turn を終え、session 終了で殺された | `--append-system-prompt` と `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`、殺された background task は continuation へ（F5-fix5） | 7667410c23d5（09-28 12:06） |

- 11 は当初 P-F5-1-4d で「制限中の Sonnet に振った router の誤り」と書いたが誤りで、人の決定（2026-09-28）で訂正した
  （その run の `rate_limit_event` は `allowed`、five_hour 13%・seven_day 11%）。
- 6〜11 はすべて同じ日のうちに本番で再現 → 修正 → 昇格した。
- dogfood 4 回目の最初の `survey` WU（investigate）が retry 2 回の後 failed になり v2 で外された件は、原因が events に残っておらず
  本報告では**未調査**。

### E6 報告の問題の行方

| E6 報告の問題 | Phase F での扱い | 状態 |
|---|---|---|
| 1 review の不合格が repair でなく replan + ReviewFail になる | D6.1 `review_timeout`、D6.2 `merge_base`（F1） | 実装。本番では merge_base の repair を 1 件観測（2 回目の配送）、review_timeout は未発生 |
| 2 `GET /tasks/{id}/artifacts` が空 | D6.3 run 後の走査（F1） | 実装。本番で `artifact_produced` を観測 |
| 3 reviewer run の `runs` 索引の欠落 | D6.4（F1）、別経路は F5-fix3 | 実装。本番の `running` 残り 0 行 |
| 4 入力トークンの大半は codex の非キャッシュ入力 | codex の `cached_input_tokens` を `cache_read_tokens` に（F5-1 の 1 回目の成果） | 実装。2 回目以降は cache read が観測される（2 回目 61.81M） |
| 5 `cost_usd` が codex の作業を除外 | D4 quota を主指標、`cost_usd_complete` | 実装。ただし codex の quota は §6 のとおり実質見えていない |
| 6 planner が 1 回 10〜14 分 | D5.3 standard・24 turn・900 s・差分 | 実装。平均 538 s → 86 s（4 回目） |
| 7 WU の直列が壁時計を押し上げる | D1 並列 | 実装。工程の中は約 2.5 倍。ただし Task の壁時計は不具合と人待ちで伸びた |
| 8 shadow の計測が機能していない | 本 ADR の範囲外（U10） | 変わらず（§4.4 観察 2） |

---

## 6. 費用: quota 消費（人の決定 4）の観測はどこまで取れたか

4 回の dogfood の `quota_estimated`（run ごとに最後の event）を窓ごとに数えた。

| 供給元 | 対象 run | 5 時間窓 | 7 日窓 | 重み付きトークン W の合計 | 窓の消費の合計 |
|---|---|---|---|---|---|
| codex-oauth（chatgpt_plus_personal） | 28（1 回目 4・2 回目 6・3 回目 4・4 回目 14） | **unknown 28 / 28（100%）** | measured 28 / 28。ただし **before == after が 25 / 28（89%）** | 137.8M（55.2M・67.1M・4.1M・11.4M） | 7d 5.0pt（0.0・3.0・1.0・1.0） |
| claude-oauth（claude_max_lab） | 6（3 回目 3・4 回目 3） | measured 1、apportioned 4、unknown 1 | measured 2、apportioned 4 | 2.4M | 5h 10.0pt、7d 8.0pt |
| （対象外）planner / reviewer run | 21（1 回目 3・2 回目 8・3 回目 3・4 回目 7） | `QuotaEstimated` 無し | 同左 | — | 未計測 |
| （欠落）lease 失効で捨てた run | 2（4 回目） | 無し | 無し | — | 未計測 |

- **codex は実質見えていない**。5 時間窓は 28 run すべて `unknown`（観測が一度も来ていない。原因は本報告では未調査、U-F5）。
  7 日窓は形式上 `measured` だが、観測値が小数 2 桁（1pt 刻み）で、28 run 中 25 run は前後の値が同じ → `used_pct = 0.0`。
  1 回目は 55.2M の W を使って 0.0pt、4 回目は 14 run で 1.0pt。**codex の `measured` は「1pt 未満だった」以上のことを言っていない**。
  phase-F.md の 4 回目の節は「codex は 14 run すべて unknown」と書いているが、正確には「5 時間窓は 14 run すべて unknown、7 日窓は measured だが 13 run が前後同値」。
- claude は 6 run と少ないが、measured と apportioned が取れている（窓の粒度は同じく 1pt）。
- `estimated` は 0 件。較正には同じ供給元の measured が 3 件要るが、プロセス内メモリだけで保持しており（F3 逸脱 3）、4 回目の間だけで
  ライブ切替が 5 回あった。codex の measured は 0.0 が多く、較正に使っても比が 0 に近くなる（推測）。
- planner / reviewer の quota は F3 の範囲外（逸脱 4）で、dogfood 4 回目の成果（`ebe7008`、57efebe4fb7d）で追加された。
  4 回の dogfood のデータには入っていない。
- 定価 USD（参考）は 4 回とも `cost_usd_complete = false`。単価の無い gpt-6-sol / luna / astra の run を含むため。2 回目は単価のある run に
  usage が無く、`cost_usd` 自体が出ていない。
- まとめ: 人の決定 4 の「どのアカウントの quota をどれだけ使ったか」は、**claude では粗く取れ、codex では取れていない**。
  4 回の worker run 36 のうち 30（83%）が codex（うち `QuotaEstimated` が残ったのは 28）なので、費用の主指標としてはまだ使えない。

---

## 7. 未解決・提案の集約

phase-F.md（F0〜F5-fix5）と phase-G.md（G1〜SD-2）の未解決・P-* を、重複を除いて優先度つきでまとめる。
優先度: **高** = 次の dogfood か費用の判断を止める / **中** = 誤りや手戻りの元 / **低** = 見た目・性能・将来。

### 高

| # | 内容 | 出どころ |
|---|---|---|
| P-F-1 | **決定（2026-09-28、人）**: `[execution] parallel = true` にして実態に合わせる（v2 の採用は変えない）。`gate` の既定は据え置き。費用指標（決定 4）はデータが揃ってから改めて判断する | 本報告 §4.4 観察 1、ADR-0074 §7 F5 |
| P-F-2 | F5b の残り（案件計画 2〜3 マイルストーン、`pause_after` 1 か所の往復）を流す。F3 (a)〜(d) と F4 (b)〜(f) は本番で一度も通っていない | 本報告 §3、F3 未解決 5、F4b 申し送り |
| P-F-3 | codex の quota: 5 時間窓の観測が来ない理由（U-F5）と 7 日窓の 1pt の粒度。取れないなら GUI の quota を「codex は 1pt 未満 / 不明」と明示する | 本報告 §6、U-F5 |
| P-F5-3 | draining の旧デーモンが持つ run を、新しい active が lease 失効より前に引き継ぐ（F5-fix2 で本番の経路は閉じたが、旧デーモンがクラッシュしたときは最大 `review_timeout × (2n+1)` 待つ） | phase-F.md F5-1 4 回目・F5-fix2 |
| P-F5-1-4c | failed の Task でも成果のブランチがゲートを通っていれば「人が統合できる」と GUI に出す | phase-F.md 4 回目 |

### 中

| # | 内容 | 出どころ |
|---|---|---|
| P-F-4 | `max_parallel_work_units` を Task・CoS・profile で狭める経路（D1.3） | F2b 逸脱 12 |
| P-F-5 | replan の差分（`execution-plan-delta/1`）に `add_phases` を足すか、「工程を足すなら全体形式」とプロンプトに書く | F5-fix3 |
| P-F-6 | 途中確認の replan で `max_replans` を使い切っていたら、黙って進まず人に見える形（質問）にする | F3 逸脱 6 |
| P-F-7 | D3.4 起点 (c): マイルストーン Task の failed / 取り下げで案件 replan を自動で起こす | F4b 申し送り |
| P-F-8 | `propose` / `propose_delta` を 1 トランザクションに（途中で止まると proposed / draft が残る） | F4a / F4b 申し送り |
| P-F-9 | 統合の repair WU・最終レビューの repair WU の spec を `RepairScheduled` に載せ、replay で行を作り直せるようにする | F2b 申し送り |
| P-F-10 | 暗黙 WU の worker run の `runs.seq` が +1 ずれる不具合（F1 逸脱 7）を直したか確かめる（本報告では未確認） | F1 未解決 |
| P-F-11 | quota の較正（`QuotaCalibrationBook`）と重なりの追跡を events から起動時に温める | F3(quota) 未解決 |
| P-F5-fix5-b | headless で turn が終わると殺される形が codex / acp に無いか確かめる | F5-fix5 |
| P-F5-fix5-a | `ScheduleWakeup` / `Cron*` など headless で無意味な道具を `--disallowedTools` で外す | F5-fix5 |
| P-F5-2 | ルート LVM（252G）の拡張か build-cache の別ボリューム化（人の判断）。並列数 × target の容量（ADR-0074 §4 の注意）もこれに効く | phase-F.md 1 回目 |
| P-G1-1 | 設定に新しい節を足すのは、それを知る release の昇格の後。`release.sh` に「現行 config を N-1 で parse できるか」を足す | phase-G.md G1 |
| P-F-12 | 別 Task 同士（同じリポジトリの Task 単位の run）の `<repo-key>` の共有で同じ偽のコンパイルエラーが出るか見る | F5-fix 未解決 |
| P-F-13 | lease 失効 2 回目の run の `worker_progress` が stdout より先に止まっていた件（draining 中の progress の書き込み） | F5-fix2 未解決 |

### 低

| # | 内容 | 出どころ |
|---|---|---|
| P-SD2-1 | `truncate_phase_report` の O(n²)（途中報告の切り詰め。テストで 49 s。release gate の律速） | phase-G.md SD-2 |
| P-F-14 | `GET /metrics/execution` の性能: `has_quota_events` でほぼ全 Task が events に落ちる。実データで再計測 | F3(quota) 未解決 |
| P-F-15 | `RepairOrigin::Delivery` の `RepairScheduled` の発行 | F1 逸脱 6 |
| P-F-16 | `WorkUnitPatch` で欄を消せない（`double_option`） | F1 逸脱 4 |
| P-F-17 | `PUT /tasks/{id}/execution/pause-after`、withdraw の reason `withdrawn` | F3 未解決 2・3 |
| P-F-18 | Ready の v2 Task の Cancel で WU 行を cancelled に | F2b 申し送り |
| P-F-19 | replan で新しい children を足す、部またぎの質問の dispatcher e2e | F4b 申し送り |
| P-F5-fix5-c | `headless_background_task` を `HarnessErrorClass` として構造化（再発が観測されたら） | F5-fix5 |
| P-F-20 | 上限を設定可能にするなら、人の `PUT /tasks/{id}/execution-plan` と celerisctl にも `ExecutionConfig.limits` を渡す | F5-fix3 |
| P-F-21 | `mode = shared` の実リポジトリで codex の `<cwd>/.git` の read-only entry と `--add-dir` のどちらが勝つか（celeris 既定の worktree には影響なし） | F5-fix4 |
| P-G2-1 | release gate でも `celerisctl scratch env` の sccache 系を使う | phase-G.md G2 |
| P-G3fix1-1 / -2 | paperqa / langmem / LDR の `with_env_removed`、legacy（scratch 無効）でも継いだ `RUSTC_WRAPPER` を外すか | phase-G.md G3-fix1 |
| P-SD1-1 / -3 / -4 | 共有 target の P1 化、`GET /releases` に skipped の段を出す、梱包の残り 12 s | phase-G.md SD-1 |
| P-SD2-2 | `free_port()` の競合が nextest で出たら test-group に入れる | phase-G.md SD-2 |

### 済み・取り下げ

| # | 内容 | 状態 |
|---|---|---|
| P-F5-1-4a | task checkout を clone（`--reference`）にして worktree の共有 `.git` をやめる（F5-fix4 の提案も同じ） | **取り下げ**（人の決定 2026-09-28: worktree のまま、codex は F5-fix4 の `--add-dir` で足りる、他ブランチへの書き込みは reviewer とブランチ規約で受け止める） |
| P-F5-1-4d | 制限中の Sonnet に cheap lane が振られた → quota の `resets_at` で provider を避ける | **取り下げ**（誤り。原因は headless の background task、F5-fix5 で修正。router の不具合ではない） |
| P-F5-1-4b | 本番 DB の `running` のまま残った `runs` 行 | 済み（人の指示で 7 行 UPDATE、以後は F5-fix3 の store hook） |
| P-F5-1 | worker / reviewer / checks の cargo に `CARGO_TARGET_DIR` を渡す | 大部分は済み（F5-fix の WU ごとの target、G1 の scratch pool）。終端 Task の `repos/*/target` の prune は未確認 |
| P-SD1-2 | `cargo nextest` でテストバイナリを並列に | 済み（SD-2、181939898ec3） |
| F3(quota) 未解決 | planner / reviewer run の quota、`docs/celeris-api-v1.md` の追記 | 済み（dogfood 4 回目の成果 `ebe7008`、57efebe4fb7d） |
| F4b 申し送り | 途中目標を dispatch で `in_progress` に | 済み（同上） |

---

## 8. 次の一歩（提案）

ADR-0074 の仕組みは F1〜F4 でテストどおりに入り、並列と統合・WU ごとの lane・軽い planner は本番で効いた。一方、F5 の 4 回は
どれも compound で done まで行けず、止めたのはすべて基盤の不具合（3・4 回目で 7 件、run の解析で 1 件。すべて同日中に修正・昇格済み）だった。案件計画と途中確認は本番で一度も
通っていない。そこで次は新しい機能を足さず、(1) `parallel` を採用の関門にするかを決めて本番の設定と揃え（P-F-1）、(2) 修正がすべて入った
181939898ec3 の上で dogfood 5 回目を F5b の形（案件計画 2〜3 マイルストーン + `pause_after` 1 か所 + 並列）で 1 回流し、(3) その結果と
codex の quota が取れない件（P-F-3）の調査を合わせて、`parallel` / `gate` の既定と「費用の主指標を quota にする」ことが codex で成り立つかを
人が判断する材料にする、のがよいと考える。ADR-0074 の状態は「Partially implemented」（D1.3・D2.1・D3.4・D3.8・D6.2 が部分）にした。
`docs/DESIGN.md` と `docs/SPEC.md` は変えていない。
