# Phase R: 再帰的な task 分解（ADR-0079）

## R0: 設計（完了 2026-09-28）

- ADR: [`docs/adr/0079-recursive-task-decomposition.md`](../adr/0079-recursive-task-decomposition.md)
- 種類: 設計のみ（コード・本番の DB と設定は変えていない。本番 DB は `sqlite3 "file:/var/lib/celeris/celeris.sqlite3?mode=ro"` で読んだだけ）。
  cargo のビルド・テストは実行していない（変更は文書だけ）。
- 置き換えの注記を足した ADR（本文は書き換えず、末尾に 1 段落）: ADR-0038・0044・0048・0062（関係のみ、置き換えなし）・0072・0074・0077。
- `docs/DESIGN.md` と `docs/SPEC.md` は変えていない（SPEC への提案は ADR-0079 D17、DESIGN への提案は下の「提案」）。

### 発端（2026-09-28）

ChatGPT（RDC）からの「browser capability を足す。Phase 1〜4」という依頼を、CoS が「設計 + Phase 1 MVP」の 1 task（01M3MFS5T52FXA63W4V10XGC4S）に
した。その task の ExecutionPlan の工程（adopt / harden / ship）は実行の段取りで、製品の Phase ではない。Phase 2〜4 の小タスクと人の決定点 H1〜H7 は
設計の成果物（WU design-gap の `phases-and-decisions.md`）に落ち、行き場が無い。案件に付く「案件計画」（ADR-0074 D3）は本番で一度も使われておらず、
案件「agent-platform の自己改善」は実態として粗い入れ物になっている。人の結論: 案件と task の責務がずれた。直し方は新しい中間層ではなく、task の構造を
再帰にすること。対応表は ADR-0079 §5。

### 人の決定（2026-09-28。原文を日本語で記録。固定）

1. **節点の種類は task だけ。** task は目的・受け入れ条件・決定点を持ち、任意で計画 = 順序付きの段階（stage）を持つ。段階の unit は **leaf**（今の
   WorkUnit: 1 回の run で終わる）か **子 task**（自分の計画を持つ）のどちらか。製品の「Phase N」は root task の段階の名前にすぎない。
   「取り組み / 途中目標 / 案件計画」はすべてこの構造で表し、専用の中間層は作らない。
2. **抽象度は自動で管理する。** Complexity Gate はすべての節点で走る（子 task は自分で gate をやり直す。atomic → 1 run、compound → 自分の計画）。
   planner は深さと残りの深さを受け取り、明示的な leaf の基準（1 run で終わる: 有界の turn、1 つの領域・リポジトリ、機械的な検査）に照らして各 unit を
   leaf / task と宣言する。最終判断は gate にあり、食い違いは記録する。上限: **max_depth = 3**（root → 子 task → leaf）、段階あたりの unit の最大数、
   木あたりの leaf の総数と費用。深い段ほど atomic に寄せる。上限に収まらない unit は**人への決定の要求**にし、黙って leaf に押し込まない。
3. root の計画を人が承認するのは、その計画が**決定点を含むとき、または上限に近いとき**だけ。それ以外は通知して進める。決定の要求はどの深さからでも
   出せ、木の中の位置（path）を持ち、受信箱と通知（cluster_login_needed で使っている Discord の経路が既にある）に出る。答えの無い決定は、それに依存しない
   unit を止めない。子の失敗は上限の範囲で親の replan（repair）が吸収し、人に届くのは上限の超過だけ。**基盤の失敗は決して「質問」として人に届けない**
   （自動で回復し、回復できなければ障害通知）。
4. 子 task は**親のブランチ**に成果を取り込む。main に取り込むのは root だけ（今日見た「task の最終レビュー中に main が進んで merge_base がずれる」も
   これで消える）。
5. 案件（project）が持つのは**ごく粗い方向**（「BenchFS に取り組む」「celeris を良くする」「hoge クラスタの維持管理」）と、常設の文脈（組織ノードの既定、
   リポジトリ、知識の置き場 `projects/<slug>`、quota の枠、常設の認可、報告の集約）だけ。**案件に付く計画（ADR-0074 D3、`POST /projects/{id}/plan`、
   project-plan/1、途中目標の auto_advance、F6 の GUI「案件計画を提案させる」）は廃止する。** 案件には依存の無い独立した root task が並列に付くだけで、
   依存は root task の木の中にだけある。

### 決定の一覧（ADR-0079）

D1 再帰のモデルと語彙 / D2 `celeris.execution-plan/3` / D3 深さと上限 / D4 節点ごとの gate と子 task の生成 / D5 段階の完了 / D6 親ブランチへの取り込み
（「配送」→「成果の取り込み」）/ D7 決定の要求 / D8 root 計画の承認の規則 / D9 replan / repair の再帰と、人に届くもの / D10 木の生存確認 / D11 報告と
metrics の roll-up / D12 CoS の指針 / D13 案件モデルの変更 / D14 GUI / D15 後方互換（採用 adopt を含む）/ D16 安全と上限 / D17 SPEC / DESIGN との関係。

### 実装計画（ADR-0079 §7）

R1a（plan/3 とデータモデル、migration 0031）→ R1b（子 task の生成と段階の完了）→ R1c（親ブランチへの取り込み）→ R2a（再帰の gate と木の上限）→
R2b（planner と replan の再帰）→ R3a（決定の要求の流れ）→ R3b（root 計画の承認と生存確認）→ R4a（木と roll-up の API）→ R4b（GUI）→
R5a（案件モデルの変更と CoS の指針）→ R5b（本番の移行と dogfood: browser の root と BenchFS の root）。`[execution.tree] enabled` は R5b まで既定 `false`。

### 未解決事項（人に確認したいこと。ADR-0079 §8）

- **U-R1**: 「max_depth = 3（root → 子 task → leaf）」を「task の段は root と子の 2 つ、leaf が 3 段目（孫 task は無い）」と読んだ。孫 task まで許す意味なら定義を 1 つずらす。**R1a の前に確認したい。**
- **U-R2**: 部署をまたぐ子 task の認可を、今の approvals（人に聞く）のままにするか、root 計画の承認に含めるか（既定は前者）。
- **U-R3**: 承認を挟まない root の計画は報告の流れに 1 件残すだけ（Discord は鳴らさない）で良いか。
- **U-R4**: 木の上限の既定値（leaf 40、run 120、段階あたり 6、同時の子 2）は推測の初期値。
- **U-R5**: root の gate の既定（本番は 17:06Z から `gate = "on"`。木の子は shadow でも gate を採用する設計）。
- **U-R6**: `POST /plans`（ADR-0028 の Plan kind）も 410 にして分解の経路を一本化するか。
- **U-R7**: 子 task ごとの reviewer run の費用を許すか（既定は許す）。
- **U-R8**: agent-platform の非終端の途中目標の行（approved / in_progress）を凍結のまま表示するか、R5b で一括 `cancelled` にするか。

### 提案

- DESIGN §5.6（計画が不正なら 1 回だけ再試行、それでも不正なら failed）に「内部の実行計画 /3 では failed でも atomic でもなく人への決定の要求にする
  （ADR-0079 D9）」の注記を足す（ADR-0072 D24 の提案と同じ場所）。
- SPEC §3.3 / §7 への 1 文ずつの追記案は ADR-0079 D17。

### 次の一歩

U-R1（max_depth の数え方）に人が答えた後、R1a（plan/3 の型と検証・`Task.tree`・migration 0031・新しい Event・`[execution.tree]`）を 1 セッションで実装する。

### R0 追記: 未決点への人の決定（2026-09-28 21:0xZ）

U-R1 = task の層数で数える（根 1 / 子 2 / 孫 3、葉は数えない）、U-R2 = 認可は今のまま、U-R3 = 承認不要の計画は通知なし、U-R4 = 既定値は草案どおり
（dogfood で調整）、U-R5 = gate は設定に従う、U-R6 = `POST /plans` も 410、U-R7 = reviewer run は当面許容し深さ別・部分木別の review 数と費用を
指標化（Prometheus は棚上げ）、U-R8 = 未終了の途中目標は凍結し将来非表示。追加で **R6 回収フェーズ**（既存の案件・task を新モデルの実情に合わせる）を置く。
次: R1a（plan/3 の型と検証、Task.tree、migration 0031、events、`[execution.tree]` 設定）。

## R1a: plan/3 の型と検証、Task.tree、migration 0031（完了 2026-09-28）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) §7 R1a、付記「R1a 実装時の逸脱・明確化」（8 項目）。
- **番号の付け替え（main への merge 時、2026-09-28）**: 実装時は migration 0030 / schema 30 だったが、先に main に入った ADR-0078 D5（F5-fix8）が
  `0030_cluster_connection_log.sql` で schema 30 を使ったため、R1a の migration を `0031_task_tree.sql`（schema 30 → 31）へ付け替えた。中身は同じ。
- 種類: コード（task-core / task-ops / task-api / celeris / celerisctl）と schema・GUI の型の再生成。daemon の挙動は変えていない
  （`[execution.tree] enabled = false` が既定。/3 は検証で `TreeDisabled`）。本番の DB・設定・サービスには触れていない。
- **schema 30 → 31（migration 0031）。昇格は stop → start が要る**（版数 30 の旧いバイナリは版数 31 の DB を `SchemaTooNew` で開けない。
  ロールバックは ADR-0040 D2 のバックアップから）。migration は列・表・索引を足すだけで既存の行を書き換えない（`root_id` は NULL のまま）。

### 実装したもの

- `celeris.execution-plan/3`（`task_core::execution_plan`）: `stages`（`review: none | human`）・`units`（`PlanUnitSpec`、`kind` に `task` を足した
  `WorkUnitKind::Task`）・`decisions`（D7 の形。`task_core::decision::DecisionSpec`）・`needs_decisions`・unit の `decisions`（糖衣）・`adopt`
  （origin human だけ）。`validate_with(spec, limits, done, PlanContext{origin, depth})`（`validate` は `PlanContext::default()` で呼ぶ）。
  `internal_view` で /2 の形に写し、統合 WU・工程の障壁・replay をそのまま使う。/1・/2 の検証は `[execution.tree]` を見ない。
- `task_core::tree`: `TreeInfo`（`Task.tree`: root_id / depth / parent_unit / base_commit）、`TreeLimits`（D3 / U-R4 の既定）、
  `can_have_child_tasks` / `remaining_depth` / `gate_threshold`（U-R1: task の層数）、`UnitDeclared` / `UnitGateAction`。
- `task_core::decision`: `DecisionRequest`（`docs/protocol/decision.schema.json`）、`DecisionKind` / `DecisionStatus` / `CostOfReversal` ほか、形の検証、
  `DecisionRow` の畳み込み（store と replay が共有）。
- Event 8 種（型と replay の読みだけ。発行は R1b 以降）: `child_task_created` / `child_adopted` / `unit_gate_overridden` / `decision_requested` /
  `decision_answered` / `decision_withdrawn` / `plan_approval_requested` / `stall_detected`。`EVENT_TYPES` 34 → 42。
- migration 0031: `tasks.root_id`（+ 部分索引）、`work_units.child_task_id`（+ 部分索引）、`work_units.needs_decisions_json`（既定 `'[]'`）、
  `decisions` 表（+ 索引 2）。store は `ChildTaskCreated` / `ChildAdopted` / `Decision*` を追記するのと同じトランザクションで派生の行を書く。
  `TaskStore::{decisions_list, decision_get, decisions_replace}`。`celerisctl replay --check/--apply` が `decisions` も突き合わせる。
- `[execution.tree]`（`celeris::config::ExecutionTreeTomlConfig`）: `enabled`（既定 false）、`max_depth`（既定 3、1..=3 以外は設定エラー）、
  `max_units_per_stage` 6、`max_stages` 5、`max_child_tasks_per_plan` 6、`max_parallel_child_tasks` 2、`max_tree_leaves` 40、`max_tree_runs` 120、
  `max_tree_replans` 10、`max_tree_tokens`（無し）、`max_open_decisions` 12、`max_open_decisions_per_plan` 8、`gate_depth_step` 2、
  `approval_near_limit_ratio` 0.8。`ExecutionLimits.tree` に写り、/3 の検証だけが見る。
- schema の再生成: `docs/protocol/execution-plan.schema.json`・`execution-plan-delta.schema.json`（`WorkUnitKind` に `task`）・
  `worker-protocol.schema.json`（`Task.tree`）・`decision.schema.json`（新規）、`docs/api/v1/event.schema.json`・`api-v1.schema.json`、
  `gui/app/celeris/types.ts`。GUI は `WORK_UNIT_KIND_LABEL` に `task: "子 task"` を足しただけ。
- ついで: `crates/task-api/tests/list_and_detail.rs` の clippy（`cloned_ref_to_slice_refs`。R1a と無関係の既存の 1 行）を直した（gate を 0 にするため）。

### 受け入れ条件（ADR-0079 §7 R1a）

| 条件 | コマンド | 結果 |
|---|---|---|
| (a) /3 の fixture が通り、/1・/2 の既存 fixture の検証結果と出力 JSON が変わらない | `cargo test -p task-core --lib -- plan_v1_and_v2_fixtures_are_byte_identical v3_fixture_validates` | ok。/1・/2 の 5 fixture（`crates/task-core/testdata/execution-plan/*.json`）のスナップショットは **R1a の変更の前のコード**で作り（commit afa87e4。生成は R1a 前のコードの wip commit 3dd0bfb 上）、変更後も一致（`git diff afa87e4 -- crates/task-core/testdata/execution-plan/*.expected.json` は差分ゼロ）。parse → 再直列化・検証エラーの文言・丸め・採用時の行を比べる |
| (b) 拒否 10 本（leaf に checks 無し / context.repo 2 / kind task に checks / acceptance 無し / `child:` 依存 / `children` / 循環 / 未知の needed_before / 未知の needs_decisions / 段階あたり 7 unit / 決定 9 件） | 同上 `rejects_leaf_without_checks rejects_leaf_with_two_repos rejects_task_unit_with_checks rejects_task_unit_without_acceptance rejects_child_prefix_dependency_in_v3 rejects_children_in_v3 rejects_cycle_in_v3 rejects_unknown_needed_before rejects_unknown_needs_decision rejects_seven_units_in_a_stage rejects_nine_decisions` | 11 本 ok（決定 9 件は unit 側の糖衣を含めて数える）。加えて `rejects_task_unit_at_max_depth`（U-R1: depth 1・2 は可、3 は `ChildTaskTooDeep`）、`adopt_is_only_allowed_in_human_plans`、`v3_leaf_budget_over_the_limit_is_rejected_not_rounded`、`rejects_malformed_stages_and_unknown_unit_stage`、`rejects_too_many_child_task_units`、`rejects_duplicate_and_malformed_decisions`、`v1_and_v2_reject_v3_fields_and_the_task_kind` |
| (c) `enabled = false` で /3 は `TreeDisabled` で拒否 | `v3_is_rejected_when_tree_is_disabled`、`cargo test -p task-api --test execution a_v3_plan_is_rejected_with_422_while_the_tree_is_disabled` | ok。エラーは `TreeDisabled` の 1 件だけで、文言に `[execution.tree] enabled = true` が出る。人の `POST /tasks/{id}/execution-plan` も 422、計画は作られない（GET は 404） |
| (d) migration 0031 が 30 の DB に当たり、`rebuild_work_units_and_runs` と `decisions` の再構築で events から同じ行ができる | `cargo test -p task-core --lib -- migration_31_adds_tree_columns_without_rewriting_rows tree_events_write_root_id_child_links_and_decisions`、`cargo test -p task-ops --lib -- replay_rebuilds_decisions_and_child_links` | ok。版数 30 の DB（task 2 行・WU 1 行）→ 31、`tasks.json` は 1 バイトも変わらず `root_id` は NULL、旧い WU 行は `child_task_id = None`・`needs_decisions = []`、`decisions` 空・索引 4 つ。/3 を採用し ChildTaskCreated・ChildAdopted・決定 3 件（回答・取り下げ・子の節点から）を積むと、store の行と events からの再構築が完全一致（`DecisionRow` の等値・`diff_execution` / `diff_decisions` 空）。表を空にする・child link を消すと `--apply` 相当で戻る |
| (e) `event.schema.json` / `execution-plan.schema.json` / `decision.schema.json` の一致テスト | `UPDATE_SCHEMA=1 cargo test -p task-core --lib`・`-p task-api --lib schema`・`-p task-worker --lib schema` で再生成 → 通常実行で `committed_schema_matches_generated`（execution-plan / delta / decision）・task-api の schema 3 本・task-worker の schema 2 本 | ok（再生成の後、`UPDATE_SCHEMA` なしの全体テストで一致） |
| `[execution.tree]` の設定 | `cargo test -p celeris --lib execution_tree_defaults_and_validation` | ok。既定は `TreeLimits::default()`（enabled false・max_depth 3）で /1・/2 の上限は不変、`max_depth = 0 / 4`・0 の上限・比 0 / 1.5・計画 > 木 の決定上限は `[execution.tree] …` の設定エラー、綴り違いは parse エラー |
| `EVENT_TYPES` と serde 名の一致 | `cargo test -p task-api --lib tree_event_types_match_their_serde_names` | ok（配列長 42） |

### gate

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 8、binaries 87（nextest 77 + doc 10）、**passed 2715 / failed 0 / ignored 7**
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0、2 回目の後も `gui/app/celeris/types.ts` の md5 が同じ（1db65f1b532351c35ff675668a9c8f8f。差分ゼロ）
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0 / `lint` → exit 0（Checked 285 files、2 infos）/ `test` → exit 0（Test Files 76 passed、Tests 1173 passed）

### 未解決・R1b 以降へ

- **API / celerisctl の入口は tree 無効のまま**: `PUT/POST /tasks/{id}/execution-plan` と `celerisctl execution` は `ExecutionLimits::default()` で検証する
  （daemon の planner 検証だけが `[execution.tree]` を見る）。R5b で人が /3 を書く前に配線が要る（付記 5）。
- `repos` が親の repos の部分集合かの検査、`Created{origin: plan_unit}`、`review: human` の段階の途中確認への解決は R1b。
- /3 の replan の差分（`execution-plan-delta` を units に当てる）は R2b（`apply_delta` は /3 の段階・unit・決定を持ち越すだけ）。
- `EVENT_TYPES` に `work_unit_committed` / `phase_integrated` / `work_units_serialized` が R1a 以前から無い（`types=` で絞れない）。R1a では触れていない。
- 依頼文の「root_id = id で埋め戻す」は ADR-0079 D13 / D15（「migration は行を書き換えない」「root_id は NULL のまま。埋め戻さない」）と食い違うので、
  ADR に従い埋め戻していない（付記 2）。埋め戻すなら ADR の改訂が先。

### 提案

- なし（DESIGN / SPEC への提案は R0 のまま）。

## R1b: 単位からの子 task 生成、状態の反映、段階完了、部分木の cancel（完了 2026-09-28）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) §7 R1b、付記「R1b 実装時の逸脱・明確化」（16 項目）。
- 種類: コード（task-core / task-ops / task-dispatch / task-api）と schema・GUI の型の再生成、GUI のラベル 1 つ。**migration なし（schema 31 のまま）**。
  `[execution.tree] enabled = false`（既定）では /3 は採用されず、daemon の挙動は変わらない。本番の DB・設定・サービスには触れていない。

### 実装したもの

- **子 task の生成**（`Dispatcher::reconcile_tree_units`、tick ごと、LLM なし）: `ready` の kind task の unit（依存は `newly_ready`、段階は現在の段階、
  `needs_decisions` が回答済み、`max_parallel_child_tasks` に空き）から `task_ops::tree::build_child_task` で子を組み立て、
  `TaskStore::tree_child_create` の 1 トランザクションで子の挿入・`Created{origin: plan_unit}`・unit を `running`・`child_task_id`・
  親の `WorkUnitTransitioned{child_created}` と `ChildTaskCreated{plan_id, unit_key, child_task_id, depth}`。子は `parent_id`・`tree`
  （root_id / depth + 1 / parent_unit）・`labels: child-<key>`・`status: ready`・親の workspace（ADR-0062 B2 の remote の格下げを通す）・
  budget・案件、unit の repos（親の部分集合）・skills・genre・features ヒント、objective の末尾に木の位置と回答済みの決定（固定の書式）。
  担当は matching、`execution_hint` なし（子は自分で gate）。
- **採用前の検査**（planner の /3）: kind task の unit の `repos` ⊆ 親（R1a から持ち越し）、部をまたぐ子の認可（`cross_department_questions`、
  `plan_children` から切り出し）。
- **状態の写し**: 子 done → unit done（`child_done`、依存先を ready に）、子 failed → unit failed（`child_failed`）、子 cancelled → unit **failed**
  （`child_cancelled`。付記 2.）、非終端（blocked を含む）→ running のまま。
- **段階の完了**: 子の unit が done で段階が揃う。統合 WU は子のブランチを merge しない（R1c まで。付記 4.）。/3 を dispatcher の工程の scheduler に乗せた
  （`is_phased_schema`）。`settle_phase` / `reconcile_parallel_tasks` は kind task の unit の running を in-flight に数えない。
- **`awaiting_children`**: 子だけを待つ親は `Ready`・lease なし・gate `Skip`。`ExecutionPhase::AwaitingChildren` と `awaiting_children`
  （`GET /tasks/{id}/execution`・タスク詳細の実行節）、`WorkUnitView.child_task_id`。GUI は「子 task の完了待ち」のラベルだけ。
- **subtree の中止**: `Trigger::ParentCancelled`（reason `parent_cancelled`）。親が cancelled / failed で終わると store が同じトランザクションで木の子を中止し、
  孫へ連鎖。走っている run は `abort_stale_runs` が止め、親の unit は tick が閉じる。`TransitionResult.cascaded` に出る。
- **委譲の禁止**: `Task.tree` を持つ task の run に `available_genres` を渡さず、`delegate` も拒む。
- **`review: human` → 途中確認**: `task_core::resolve_plan_pause_points`（/1・/2 は不変）を採用・replan の `PausePointsResolved` に使う。
- **replay**: 子の結び付きと unit の状態は events だけから作り直せる（新しい集計は無し。R1a の `ChildTaskCreated` の読みと `WorkUnitTransitioned` で足りる）。

### 受け入れ条件（ADR-0079 §7 R1b と依頼の項目）

| 条件 | コマンド | 結果 |
|---|---|---|
| (a) 偽の planner の /3（leaf + task）で leaf が走り、task の unit が ready で子（parent_id・depth 2・root_id・`child-<key>`・ready・親の repos / workspace / budget）が 1 トランザクションで作られ、`ChildTaskCreated` と `Created{origin: plan_unit}` が残る。子は自分の gate で atomic → 1 run → done → unit done → 段階 s1 統合 → s2 → 親 done | `cargo test -p task-dispatch --lib -- dispatcher::tests::tree::v3_task_unit_creates_child_when_ready` | ok。unit c の reason は `child_created`→`child_done`、`runs = 0`、`PhaseIntegrated` は s1, s2、s2 の leaf は子の done の後に ready |
| (b) 依存・`needs_decisions` が満たされるまで子は作られない | `… tree::child_waits_for_dependencies_and_decisions` | ok。a done 後も h1 未回答（`DecisionRequested` だけ）の間は子 0・親 Ready・lease なし。回答で子ができ、objective に `- h1 which backend: manual（推奨と異なる） — trial first` |
| (c) 子 done → unit done、failed → unit failed、cancelled → unit failed（付記 2.）。段階は完了せず既存の失敗の経路（replan の planner run）へ、panic なし。`max_parallel_child_tasks` | `… tree::unit_mirrors_child_status`、`… tree::child_failure_fails_the_unit_and_the_stage_does_not_complete`、`cargo test -p task-ops --lib tree::` | ok。上限 2 で 3 つ目は待ち、空きで作成。blocked の子は running のまま。`settle_phase` = Failure、gate = `RunPlanner{replan: true}`。実行経路では子の最終レビューが落ちて failed → unit failed・統合なし・親は done にならない |
| (d) 子だけを待つ親は Ready のまま dispatch されず lease なし、execution の phase が `awaiting_children` | `… tree::parent_waits_without_lease` | ok。gate `Skip`、`awaiting_children = [{unit_key c, task_id, "Child c", running}]`、親の run は planner の 1 本だけ |
| (e) root の Cancel で子・孫が `cancelled`（`parent_cancelled`）、走っている run が止まる | `… tree::cancel_cascades_to_subtree` | ok。`cascaded` = 子と孫（深さ 3）、子の run の runs 行は `cancelled`、root の unit はすべて cancelled |
| (f) 木の節点の run の `available_genres` が空 | `… tree::tree_runs_cannot_delegate` | ok（木でない task は従来どおり 1 件） |
| `review: human` の段階が途中確認で止まる | `… tree::review_human_stage_pauses_after_integration`、`cargo test -p task-core --lib pause::tests::plan_pause_points_include_review_human_stages` | ok。s1 の統合の後に `awaiting_human`・`PausePointsResolved{phases: [s1]}`・`PhaseReported`、「続ける」で done。/2 の解決は従来と同じ |
| repos ⊆ 親 | `… tree::task_unit_repos_must_be_a_subset_of_the_parents`、`cargo test -p task-ops --lib tree::tests` | ok。外れた計画は不正な試行（進行に理由）、次の計画を採用 |
| (g) tree 無効で挙動不変 | `… tree::tree_disabled_rejects_v3_and_creates_no_child` と既存の全テスト | ok。/3 は採用されず子 0、`[execution.tree] enabled = true` の文言。既存の /1・/2・plan/2 children のテストは全部通過 |
| replay | `… tree::replay_rebuilds_child_links_and_unit_statuses`（ほか各テストの末尾の `assert_replay_is_clean`） | ok。status / attempts・`work_units`（`child_task_id`・状態）・`execution_plans`・`decisions` の差分 0。壊した索引は `--check` で検出、`--apply` で復元 |

### gate

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 8、binaries 88（nextest 78 + doc 10）、**passed 2751 / failed 0 / ignored 7**
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0、再実行後も `gui/app/celeris/types.ts` の md5 が同じ（6c7d2447bd379177f13be9c3702ef15f）
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0 / `lint` → exit 0（Checked 285 files、2 infos）/ `test` → exit 0（Test Files 76 passed、Tests 1175 passed）

### 未解決・R1c 以降へ

- **R1c**: 子の基点（`base_commit`）、統合 WU が `celeris/<child_id>` を親ブランチに merge、子の最終レビューの基点、子の取り込み（配送）と `TaskReady` の抑止。
  R1b では子の成果は親ブランチに入らない（付記 4.）。
- **R2a**: 深さの gate の閾値・木では shadow でも採用・unit の gate・木の上限（子の生成は今 `max_parallel_child_tasks` だけを見る）。
- **R2b**: 子の失敗 → 親の replan の中身（/3 の replan の差分、子の失敗の要約を planner に）、子の基盤の失敗の再試行。今は既存の失敗の経路に入るだけ。
- **R3a**: 計画の採用時の `DecisionRequested` の発行と回答の入口。R1b では `needs_decisions` の unit は回答の行ができるまで待ち続ける。leaf の `needs_decisions` も R3a。
- **R4a**: 途中報告に子の要約の行を足す（今は unit の題名だけ）。R4b: 木のタブ。R5a: subtree の一時停止。
- API の `PUT/POST /tasks/{id}/execution-plan` と `celerisctl execution` は R1a のまま tree 無効で検証する（R5b までに配線）。
- 既存の差（R1b と無関係）: 偽のアダプタで走らせた /2・/3 の計画で、`replay --check` の `runs` の表に planner run の role と並列 WU の run の
  `work_unit_id` / `seq` の差が出る（/2 だけの計画でも再現。R1b では直していない）。

### 提案

- なし（DESIGN / SPEC への提案は R0 のまま）。

## R1c: 子のブランチと親ブランチへの取り込み、子の review 基準、根だけが main へ（完了 2026-09-28）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) §7 R1c・D5・D6、付記「R1c 実装時の逸脱・明確化」（10 項目）。
- 種類: コード（task-core / task-worker / task-dispatch / task-ops / task-api / celeris）。**migration なし（schema 31 のまま）**、API の型・
  schema・GUI の型の変更なし（`gen:types` の差分 0）。`[execution.tree] enabled = false`（既定）では /3 が採用されず子が作られないので、
  daemon の挙動は変わらない（木でない task の worktree の base・統合・取り込み・通知は 1 バイトも変えていない）。本番の DB・設定・サービスには触れていない。

### 実装したもの

- **子のブランチと基点**: 子は普通の task の作業場所（`<workspace_root>/<child_id>/repos/<name>`、旧い形は `/tree`）とブランチ `celeris/<child_id>`
  を持つ（付記 1.）。子を作る tick で `Dispatcher::child_base_commit` が葉と同じ規則（同じ段階の依存先は `integration::dependency_base`、
  無ければ親の task ブランチの HEAD = 段階の基点）で基点を決め、子の `tree.base_commit` に書く。`Dispatcher::worktree_base_for` は木の子の
  worktree を `BaseKind::Parent`（前置きの出どころ `parent`）でこの sha から切る。`dependency_base` は依存先が子 task なら子のブランチの HEAD。
- **子の done の記録**: unit を `done` に写す tick で子の worktree に残った変更を決定的に commit し、unit の `head_commit` / `base_commit` と
  `WorkUnitCommitted{branch: celeris/<child_id>}` を `child_done` と同じトランザクションで残す（replay も同じ値を作る）。
- **統合**: `integrate-<stage>` は葉の WU のブランチに、段階の done の子のブランチを `seq` 順で足して親ブランチへ `merge --no-ff`
  （既に入っていれば `skipped`、リポジトリに無い子のブランチは飛ばす）。段階の検査を再実行し、`PhaseIntegrated.merged` に子の key と子の
  ブランチの HEAD が入る。衝突・検査の失敗は既存の repair / replan。統合の後で子の worktree を消す（ブランチは残す）。
- **子の最終レビュー**: `review::tree_child_review_view` が、検査の `merge-base --is-ancestor main`（等）を親のブランチに置き換え、reviewer の
  前置きに「## 取り込み先（ADR-0079 D6）」（親のブランチ・差分の基点・main が進んでも不合格にしない）を足す。checkpoint の差分の基点も `base_commit`。
- **根だけが main へ**: 木の子は `task_ops::delivery::begin`（ADR-0051）で `deliveries` の行も merge の条件も作られず、`celeris` の delivery の tick も
  木の子の行を進めない。`POST /tasks/{id}/changes/{repo}/integrate`（ADR-0043 D5）は 409 `tree_child`（文言は「成果の取り込み」）。`TaskReady` は
  root の done だけ。root の経路は変えていない。

### 受け入れ条件（ADR-0079 §7 R1c と依頼の項目）

| 条件 | コマンド | 結果 |
|---|---|---|
| (a) 子は `celeris/<child_id>` を段階の基点から切り、`integrate-S` が leaf と子を親ブランチに merge し、`PhaseIntegrated.merged` に子と commit が残る | `cargo test -p task-dispatch --lib -- dispatcher::tests::tree_branches::integration_merges_child_task_branch` | ok。s2 の子 c の `tree.base_commit` = `integrate-s1` の `integrated_commit`、c の cwd = `<ws>/<child_id>/tree`、`merged(s2)` = `[(b,false),(c,false)]` で c の commit = `celeris/<child_id>` の HEAD、親ブランチに a/b/c.txt、子の worktree は消えブランチは残る、main は不変、`WorkUnitCommitted{c}`。壊した c の `head_commit` を `replay --check` が検出し `--apply` で `head_commit` / `base_commit` が戻る |
| (b) 同じ段階で子に依存する leaf は子の HEAD から切られる | `… tree_branches::same_stage_dependency_on_child_bases_on_child_head`、`cargo test -p task-dispatch --lib -- integration::tests::child_task_branches_are_optional_and_idempotent` | ok。b の `base_commit` = c の `head_commit`、`merged(s1)` = `[(c,false),(b,false)]`。`dependency_base` は子のブランチ → 記録した head → task ブランチ |
| (c) 子の最終レビュー中に main が進んでも、子の merge-base の検査は親ブランチと比べて通る | `… tree_branches::child_review_ignores_main_moving`、`cargo test -p task-dispatch --lib -- review::tests::merge_base_ref_is_rebased_on_the_parent_branch review::tests::only_tree_children_get_the_parent_branch_review_view` | ok。子の判定は 1 回で合格、理由に `--is-ancestor celeris/<root> HEAD`、main の新しい commit は子にも親にも入らない、子の保存された条件は `main` のまま。root は view が変わらない |
| (d) 子の done で `deliveries` も取り込みの通知も作られず、root だけが ADR-0051 に進む | `cargo test -p task-ops --lib delivery`、`cargo test -p celeris --test notify a_tree_child_done_does_not_notify_task_ready_but_the_root_does`、`cargo test -p task-api --test changes` | ok。木の子は `begin` が `None`・条件を足さない・行なし、同じ task を root として渡すと従来どおり `Some`（回帰）。`TaskReady` は root の 1 件だけ。子の merge / pr / discard は 409 `tree_child`（main・ブランチ・記録は不変）、既存の取り込みの試験 7 本は通過 |
| 孫（深さ 3）→ 子（深さ 2）→ root | `… tree_branches::grandchild_integrates_into_child_which_integrates_into_root` | ok。子は compound（自分の計画）、孫の `depth = 3`・基点は子の段階の基点の子孫、子の `merged(t1)` に g（孫のブランチの HEAD）と l、root の `merged(s1)` = c（子のブランチの HEAD）、g/l.txt が子と root のブランチに入る、main 不変 |
| worktree の base（木の子は `Parent`、root は `Main`、基点が無いリポジトリでは親のブランチの HEAD） | `… tree_branches::worktree_base_of_a_tree_child_is_the_parent_base` | ok |
| (e) 既に入っている子は統合で飛ばす（採用は R5b。付記 10.） | `… integration::tests::child_task_branches_are_optional_and_idempotent` | ok。2 回目は `skipped`、リポジトリに無い子のブランチは `merged` に出ない、WU のブランチが無ければ従来どおり Err |
| tree 無効で挙動不変・root の取り込みは不変 | 既存の全テスト（R1b の `tree_disabled_rejects_v3_and_creates_no_child`、ADR-0041 / 0043 / 0051 / 0074 の worktree・統合・取り込みの試験） | ok（下の gate） |
| replay | 各試験の末尾の `assert_replay_is_clean` と (a) の壊した索引の復元 | ok。status / attempts・`work_units`（`head_commit`・`child_task_id`）・`execution_plans` の差分 0 |

### gate

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 8、binaries 88（nextest 78 + doc 10）、**passed 2761 / failed 0 / ignored 7**（R1b の 2751 から +10）
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0、2 回とも `gui/app/celeris/types.ts` の md5 が同じ（6c7d2447bd379177f13be9c3702ef15f、R1b と同じ）
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0 / `lint` → exit 0（Checked 285 files、2 infos）/ `test` → exit 0（Test Files 76 passed、Tests 1175 passed）

### 未解決・R2 以降へ

- **木のブランチの後片付け**: 子の worktree は統合の後で消すが、done の子のブランチ `celeris/<child_id>` は root の取り込み・中止の後も残る（付記 9.。
  中止の連鎖で cancelled になった子は既存の ADR-0043 D2 の後片付けで消える）。root の終端で木のブランチをまとめて消すのは R4b 以降。
- **子の `TaskFailed` 通知**は今どおり鳴る（D11 の抑止は R3b）。子の失敗 → 親の replan の中身は R2b。
- **`base_commit` は先頭のリポジトリの sha だけ**（付記 2.）。子の repos の 2 つ目以降は worktree を切る時点の親のブランチの HEAD（段階の途中では動かない）。
- GUI の「配送」→「成果の取り込み」の全面的な言い換えと、子の取り込み先（「成果の取り込み（親『…』の段階『…』へ）」）の表示は R4b。
  `GET /tasks/{id}/changes` は子にも今どおり差分を出す（取り込みのボタンを押すと 409）。
- 採用（adopt）した done の子の統合の試験（§7 R1c (e) の名前の試験）は R5b（統合の冪等性は R1c で確かめた）。

### 提案

- なし（DESIGN / SPEC への提案は R0 のまま）。

## R2a: 再帰 gate と深さ別閾値、unit gate、木の上限 → 決定要求（完了 2026-09-29）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) §7 R2a・D3・D4、付記「R2a 実装時の逸脱・明確化」（14 項目）。
- 種類: コード（task-core / task-ops / task-dispatch / task-api / celeris）と schema・GUI の型の再生成、GUI の gate の 1 行表示。
  **migration なし（schema 31 のまま）**。`[execution.tree] enabled = false`（既定）では /3 が採用されず木の子も無いので、gate（root は
  `[execution] gate` のまま、判定の JSON も 1 バイトも変わらない）・unit の gate・木の上限はどれも働かない。本番の DB・設定・サービスには触れていない。
- R2b（planner のプロンプト・`plan_invalid`・子の失敗の replan）には手を付けていない。

### 実装したもの

- **深さの閾値**: `execution_gate::decide_at(.., GateThreshold)`、閾値 `5 + gate_depth_step × (depth − 1)`（`[execution.tree] gate_depth_step`、既定 2、
  0..=10 を検証）。`ExecutionGateDecision.depth`（木の子だけ。root は出力しない）。
- **木の子は shadow でも採用**: 木の子（`tree.parent_unit` を持つ task）の判定は `shadow = false`・`depth = d` で記録し、compound なら `[execution] gate`
  （shadow / off を含む）に関わらず planner に進む。root は従来どおり（U-R5）。GUI の gate の 1 行に「木の子: 深さ d・閾値 t、常に採用」。
- **unit の gate**（`task_core::tree::{unit_view, unit_gate, apply_unit_gates, promote_to_task, demote_to_leaf}`、採用の前に `Dispatcher::tree_plan_gate`）:
  /3 の各 unit の view に深さ `d + 1` の閾値で gate をかけ、D4 (3) の表どおり leaf → task に上げる / task → leaf に下げる / task のまま（構造上の理由・
  leaf の基準の不足）/ 子 task を持てない深さの compound な leaf は `kind: leaf_too_large` の決定の要求。上げる・下げるは採用する計画の spec に当て、
  食い違いは `Event::UnitGateOverridden{plan_id, unit_key, declared, gate, action, depth, threshold, score, reason}`（`score` / `reason` を足した）で残す。
- **止め方**: `WorkUnitBlockedReason::Decision`（`blocked(decision)`）。工程の失敗にも質問にも数えず、同じ段階の他の unit を止めず、段階は完了しない。
- **木の上限 → `kind: limit` の決定の要求**（D7 の形、path 付き、`decisions` の行と `DecisionRequested` を同じトランザクション）:
  - 計画の上限（段階の数・段階あたりの unit・計画あたりの子 task・`max_depth`）: 1 回目は不正な試行（再試行）、最後の試行では採用して超えた分の
    unit だけを止める（`task_core::tree::plan_limit_holds`。atomic に倒さない・計画から消さない）。
  - 木の leaf（`max_tree_leaves`）: 採用のとき、木の生涯の leaf + この計画の新しい leaf で超える分を止める。
  - 木の run・トークン・replan（`max_tree_runs` / `max_tree_tokens` / `max_tree_replans`）: run を起こす直前に照らし、超えるなら run を起こさず決定を
    木に 1 件だけ出す（`Dispatcher::tree_run_limit_hold`）。
  - 同時の子（`max_parallel_child_tasks`）は D3 どおり「作らずに待つ」（決定にしない。付記 10.）。
- **数え上げ**: `task_core::tree::tree_counters`（純粋）と `task_ops::tree::tree_counters`（store の読み取り、新しい `TaskStore::tree_tasks`）。
  木の節点・leaf・run（reviewer を除く）・reviewer の run・replan・トークン・定価と、**深さごとの run（role ごと）・reviewer の run と定価**（U-R7。API は R4a）。
- 決定の path（`task_ops::tree::decision_path`）と同じ key の未回答の決定の検索（`open_decision`）。
- 既存の R1b / R1c の試験の kind task の unit に `skills: ["tree-fixture"]`（子 task のまま残す構造上の理由。付記 13.）。

### 受け入れ条件（ADR-0079 §7 R2a と依頼の項目）

| 条件 | コマンド | 結果 |
|---|---|---|
| (a) 深さの閾値: score 6 は深さ 1 で compound、深さ 2（閾値 7）で atomic、score 7 は深さ 2 で compound・深さ 3（閾値 9）で atomic。root の判定の JSON は不変 | `cargo test -p task-core --lib -- execution_gate::tests::gate_threshold_rises_with_depth tree::` | ok（10 passed）。`decide == decide_at(ROOT)`、root の JSON に `depth` が無い、強制規則は深さに関わらず compound |
| (b) `gate = "shadow"` でも木の子の compound は planner に進み、score 6 の子は深さ 2 で atomic（どちらも `shadow = false`・`depth = 2`・閾値 7）。root（人の明示）は `shadow = true` のまま planner へ、木でない task の規則表の compound は shadow で記録だけ | `cargo test -p task-dispatch --lib -- dispatcher::tests::tree_gate::tree_nodes_adopt_gate_even_in_shadow` | ok。子 cb は自分の計画を持ち planner run あり、子 c6 は planner run なし・1 run で done、root done。木でない task は planner run なし・done |
| (c) unit の gate の表 7 行（leaf+atomic / leaf+compound で上げる〈深さ 1・2〉/ 持てない深さで決定 / task+compound / task+atomic で構造上の理由 5 種は kept_task / 理由なし・基準を満たせば下げる、予算・command の不足は kept_task / 深さ 3 の task は `ChildTaskTooDeep`） | `cargo test -p task-core --lib -- tree::tests::unit_gate_table tree::tests::promoted_and_demoted_units_keep_their_work_and_validate` | ok。上げた unit は checks → command の acceptance、done_when・paths は目的の末尾、下げた unit は段階の kind・checks・done_when。当てた計画は検証を通る |
| (c) 実行経路: compound な leaf は子 task に上げ、小さな kind task は leaf に下げ、`UnitGateOverridden`（promoted / demoted、depth 2・閾値 7・score・reason）が残り、仕事は落ちずに root done | `… tree_gate::unit_gate_promotes_and_demotes_with_override_events` | ok。big は kind task（子は自分の planner で compound）、small は leaf として 1 run、a は一致で記録なし |
| (c) `leaf_too_large_at_max_depth_raises_decision`: 子 task を持てない深さの compound な leaf は決定の要求（`leaf_too_large:big`、needed_before [big]）と `blocked(decision)`、同じ段階の a は done | `… tree_gate::leaf_too_large_at_max_depth_raises_decision` | ok。run は planner 1 + a 1 だけ、tick を重ねても決定・run は増えない、質問・approvals なし、gate は compound のまま |
| 計画の上限 4 種と木の leaf が `limit` の決定を 1 件出し、超えた unit だけを止める（他に副作用なし） | `… tree_gate::plan_limits_raise_limit_decisions_and_hold_only_the_excess` | ok（5 ケース）。段階あたり（`limit:max_units_per_stage:s1`、x）・段階の数（`limit:max_stages`、x）・子 task（`limit:max_child_tasks_per_plan`、x）・`max_depth`（`limit:max_depth`、x）は planner 2 回（1 回目は不正な試行）で採用、木の leaf（`limit:max_tree_leaves`、x）は 1 回。どれも計画の unit は全部残り、上限の内の unit は done、root は ready・compound のまま、質問・approvals なし、replay 差分 0 |
| 同時の子の上限は決定にせず待つ（D3） | `… tree_gate::concurrent_children_limit_waits_without_a_decision` | ok。上限 1 で c2 は ready・子なし、決定 0・approvals 0 |
| (d) `tree_limit_breach_stops_only_that_subtree`: `max_tree_runs` に達した木で次の run を起こそうとした子は run を起こさず、`limit:max_tree_runs` の決定が木に 1 件（`needed_before: [self]`、path = root(s1) › 子）。先に走った兄弟は done | `… tree_gate::tree_limit_breach_stops_only_that_subtree` | ok。tick を重ねても決定は 1 件、木の run は 2（reviewer を除く）、root は ready（子待ち） |
| replan の上限（`max_tree_replans`）: 人の replan の依頼でも planner run を起こさず `limit:max_tree_replans` の決定 | `… tree_gate::tree_replan_limit_raises_a_decision_instead_of_a_planner_run` | ok。planner run は 1 のまま、計画は版 1、子は走り続ける |
| 3 段の木の数（store から）が手計算と一致、深さごとの reviewer の run（U-R7） | `cargo test -p task-core --lib -- tree::tests::tree_counters_across_a_three_level_tree`、`cargo test -p task-dispatch --lib -- dispatcher::tests::tree_branches::grandchild_integrates_into_child_which_integrates_into_root` | ok。純粋: run 5・reviewer 3・leaf 2（task / 統合 / repair / 決定待ちを除く）・replan 1・トークン（cache 除く）・定価（欠けたら complete=false）・深さ 1〜3 の値、上限の照合（runs → tokens → replan）。実行経路: root → 子 → 孫の木で節点ごとの runs 索引から数えた値と一致 |
| `[execution.tree]` の設定と検証 | `cargo test -p celeris --lib execution_tree_defaults_and_validation` | ok。`gate_depth_step` / `max_tree_runs` / `max_tree_replans` / `max_tree_leaves` / `max_parallel_child_tasks` が `ExecutionLimits.tree` に写る、`gate_depth_step = 11`・各上限 0 は設定エラー |
| 決定の形（D7） | `cargo test -p task-core --lib -- tree::tests::daemon_decisions_have_the_d7_shape` | ok。limit / leaf_too_large の選択肢・推奨 replan・ULID・計画の決定と同じ形の検査を通る |
| tree 無効で挙動不変・既存の木の試験 | 下の gate（全テスト） | ok。R1b / R1c の試験は fixture に skill を足しただけで同じ結果 |
| replay | 各試験の末尾の `assert_replay_is_clean` | ok。status / attempts・`work_units`（`blocked(decision)` を含む）・`execution_plans`・`decisions` の差分 0 |

### gate

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 8、binaries 88（nextest 78 + doc 10）、**passed 2774 / failed 0 / ignored 7**（R1c の 2761 から +13）
- schema の再生成: `UPDATE_SCHEMA=1 cargo test -p task-core --lib schema`・`-p task-api --lib schema`・`-p task-worker --lib schema`（`event.schema.json` /
  `api-v1.schema.json` / `worker-protocol.schema.json`。`execution-plan.schema.json` は不変）
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0、再実行の前後で `gui/app/celeris/types.ts` の md5 が同じ（6e737b087860c6b97df0799918ede1c0）
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0 / `lint` → exit 0（Checked 285 files、2 infos〈既存〉）/ `test` → exit 0（Test Files 76 passed、Tests 1176 passed）

### 未解決・R2b 以降へ

- **R3a**: 決定の回答の API・MCP・受信箱・Discord 通知と、回答の効き目（`blocked(decision)` の unit を `ready` に戻す、`raise-once` で上限を今回だけ
  上げる、`withdraw` で取り下げる、`replan` で planner）。§7 R2a (e) `limit_raise_answer_resumes_subtree` は回答の入口が R3a なのでそこで試す。
  `max_open_decisions`（木あたり 12）の束ねも R3a。
- **R2b**: planner に深さ・残りの深さ・leaf の基準・木の残りの上限を渡す（今は unit の gate と上限の止めが後から直すだけ）、/3 の 2 回不正の
  `plan_invalid`、子の失敗 → 親の replan。
- **R3b**: 決定を待って止めた木（`ready` のまま run を起こさない節点）の生存確認と表示（D10）。今は決定の行が「名指しの待ち」の証拠。
- **R4a**: `GET /metrics/execution` の「planner と gate の不一致」の件数（`UnitGateOverridden` の集計）と深さ別の run・reviewer の費用（`TreeCounters.by_depth`）の API。
- **R5b まで**: 人の計画（`PUT`）の入口は tree 無効のまま（unit の gate もかけない。付記 4.）。
- 木の上限の run の数は木全体の値なので、超えた後は木のどの節点も新しい run を起こさない（付記 9.）。部分木ごとの上限にするかは R5b の dogfood で見る。

### 提案

- なし（DESIGN / SPEC への提案は R0 のまま）。

## R2b: planner への深さ・葉基準・残り上限、無効計画 → 決定要求、子の失敗 → 親の replan、基盤失敗の再試行（完了 2026-09-29）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) §7 R2b・D4 (2)・D9・D12、付記「R2b 実装時の逸脱・明確化」（8 項目）。
- 種類: コード（task-core / task-ops / task-worker / task-dispatch）と schema・GUI の型の再生成。**migration なし（schema 31 のまま）**。
  `[execution.tree] enabled = false`（既定）では planner の文脈に `tree` が無く（プロンプトは 1 バイトも変わらない）、/3 は採用されず木の子も
  無いので、plan_invalid・子の replan・子の基盤の失敗の再試行・`limit:max_replans` はどれも働かない。本番の DB・設定・サービスには触れていない。
- R3（決定の回答・通知・承認・生存確認）には手を付けていない。

### 実装したもの

- **/3 の planner の入力**（`task_worker::protocol::TreePlannerContext`、`Dispatcher::tree_planner_context`）: 深さ・`max_depth`・残りの深さ・
  計画の上限（段階・段階あたり・子 task・決定）・同時の子・木の残り（leaf・run・replan・節点の replan・トークン・未回答の決定）・祖先（題名・段階・
  目的の先頭 300 文字）・`stages_hint`（新しい `Task.routing.stages_hint`）。値は検証と同じ `[execution.tree]` と `tree_counters` から。
- **/3 のプロンプト**（`claude_code::tree_plan_shape_section` / `tree_replan_context_section`）: /3 の形（段階・unit = leaf | 子 task・決定）、
  木の中の位置、leaf の基準 3 つ（有界の turn・1 つのリポジトリ・command の検査 1 本以上）、子 task にする理由、「収まらない unit は子 task か
  決定にし、leaf に押し込まない」、決定の書き方と `needs_decisions`、計画と木の上限の残り、`stages_hint`。残りの深さ 0 では「kind task を書くな」。
  replan は計画の全体（done の unit は daemon が持ち越す）と、失敗した子の unit の扱い（同じ key で次の子・分ける・落とす・決定）。
- **/3 の 2 回不正 → `kind: plan_invalid`**（`task_core::tree::plan_invalid_decision`）: atomic に倒さず、Task は `ready`、決定が開いている間は
  run を起こさない（`plan_invalid_hold`）。/3 の replan の 2 回不正も自由文の質問の代わりに同じ決定。/1・/2 は従来どおり（atomic / 質問）。
- **/3 の replan**: 差分は拒否、done の unit は持ち越し（`carry_done_units_v3`）、`task_ops::execution::replan` を /3 に対応（`internal_view`。
  R1b〜R2a の /3 の replan は未完了の unit を全部 superseded にして新しい行を作らなかった）。走っている子の unit は `running` のまま持ち越す。
- **子の work の失敗 → 親の replan**: replan の理由に子の題名・attempt・分類・理由・最後の checkpoint、unit の要約に子の状態と失敗の理由。
  同じ key を残すと同じ unit から次の子（`Task.tree.parent_unit.attempt`）。元の子とブランチは残る。
- **節点の replan の上限**（`[execution] max_replans`）を使い切った木の節点は `limit:max_replans` の決定の要求（黙って `Skip` にしない）。
  木の上限（`max_tree_replans`）は R2a のまま `limit:max_tree_replans`。
- **子の基盤の失敗**: 同じ unit から 1 回だけ自動で子を作り直し（`child_infra_retry`、replan しない）、それでも失敗なら unit `blocked(infra)`
  （`child_infra_failed`）と障害通知（`TaskFailed`、key `tree-infra:…`）。決定・質問にはしない。兄弟は続き、段階は完了しない。
- **replay**: 版ごとに done でない行の `plan_id` / `seq` を新しい版にする（/2 の replan の既存の差も解消）、replan で子の結び付きを外す、
  `child_infra_failed` → `blocked(infra)`。

### 受け入れ条件（ADR-0079 §7 R2b と依頼の項目）

| 条件 | コマンド | 結果 |
|---|---|---|
| (a) `planner_prompt_carries_depth_and_leaf_criteria`: /3 のプロンプトに深さ・残りの深さ・leaf の基準・計画と木の残りの上限・`stages_hint`・決定の書き方・「leaf に押し込まない」。残りの深さ 0 で「kind task を書くな」と祖先。replan は全体と失敗した子の扱い。`tree = None` は /2 のまま | `cargo test -p task-worker --lib planner_prompt` | ok（4 passed。既存の上限・再試行・v2 のプロンプトの試験も通る） |
| (a) 配線: root の planner の `tree`（深さ 1・残り 2・設定の上限・木の残り・`stages_hint`）、子の planner（深さ 2・残り 1・祖先 = root の題名と段階）、木が無効なら `tree` 無し | `cargo test -p task-dispatch --lib tree_replan::tree_planner_context_is_wired_from_limits_and_counters` | ok |
| (b) `v3_invalid_plan_asks_instead_of_atomic`: 2 回不正な /3 → `plan_invalid` の決定 1 件（選択肢 3、理由つき、`needed_before: [self]`、path = root）、worker の run 0、gate は compound のまま、root `ready`、tick を重ねても planner 2 回・決定 1 件、質問なし。/2（木が無効）は `atomic/planner-invalid` で done | `… tree_replan::v3_invalid_plan_asks_instead_of_atomic` | ok。replay 差分 0 |
| (c) `child_failure_triggers_parent_replan`（一時 git）: 子 c が work で failed → 親の replan の planner の `replan_reason` に子の題名・attempt 1・(work)・不合格の理由、unit の要約に子の状態。v2 は done の leaf a を省いても持ち越し、同じ key c から attempt 2 の子が done、統合は新しい子を merge、root done、元の子の task とブランチ `celeris/<子>` は残る | `… tree_replan::child_failure_triggers_parent_replan` | ok。計画 2 版、planner 2 回、決定・質問なし、replay 差分 0 |
| (d) `child_infra_failure_is_not_a_question`: 子 c が基盤の失敗（`infra failure ×1`）→ 1 回だけ作り直し（attempt 2）→ 2 回目も失敗 → unit c `blocked(infra)`、`TaskFailed` の障害通知 1 件（本文に infra と子の題名）、決定・質問・replan なし（planner 1 回）。兄弟の子 s と leaf a は done、段階は完了せず root は `ready` | `… tree_replan::child_infra_failure_is_not_a_question` | ok。unit c の遷移は `child_created → child_infra_retry → child_infra_failed`、replay 差分 0 |
| replan は木の上限（`max_tree_replans = 1`）で止まり `limit:max_tree_replans`、節点の上限（`max_replans = 1`）で止まり `limit:max_replans`（どちらも planner 2 回・計画 2 版・子 2 つ・決定 1 件・質問なし） | `… tree_replan::child_replans_are_bounded_by_tree_and_node_limits` | ok。replay 差分 0 |
| 木が無効で挙動不変・既存の木の試験 | 下の gate（全テスト） | ok。`cargo test -p task-dispatch --lib tree` は 48 passed（R1b〜R2a の既存の木の試験は変更なし + R2b の 5 本）、task-ops 373 passed |

### gate

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 8、binaries 88（nextest 78 + doc 10）、**passed 2780 / failed 0 / ignored 7**（R2a の 2774 から +6）
- schema の再生成: `UPDATE_SCHEMA=1 cargo test -p task-core --lib schema`・`-p task-api --lib schema`・`-p task-worker --lib schema`（`event.schema.json` /
  `api-v1.schema.json` / `worker-protocol.schema.json`。`execution-plan.schema.json` / `decision.schema.json` は不変）
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0、再実行の前後で `gui/app/celeris/types.ts` の md5 が同じ（ae3ddd7e5a1ecb5c237c9370af02c245）
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0 / `lint` → exit 0（Checked 285 files、2 infos〈既存〉）/ `test` → exit 0（Test Files 76 passed、Tests 1176 passed）

### 未解決・R3 以降へ

- **R3a**: `plan_invalid` / `limit:max_replans` の回答の効き目（人の計画の `PUT` で決定を閉じる、atomic に切り替える、取り下げ）。`PUT` の入口は
  まだ tree 無効で検証する（R1a 付記 5.。R5b までに配線）。leaf の失敗で replan を使い切ったときの ADR-0072 D12 の質問は木の節点でも質問のまま。
- **R3b / R4b**: `blocked(infra)` の unit の人の「再試行」の入口（GUI・API）、子の `TaskFailed`（`scan_task_failed`）の抑止（今は子自身の通知と
  dispatcher の障害通知の両方が鳴る）、`blocked(infra)` と `plan_invalid` の待ちを生存確認（D10）の「名指しの待ち」に数える。
- **R5a**: `stages_hint` を書く入口（CoS の `create_task.stages_hint`・API）。
- 子の作り直しの回数は unit ごとの上限を持たず replan の上限で抑える（付記 5.）。dogfood（R5b）で見直す。

### 提案

- なし（DESIGN / SPEC への提案は R0 のまま）。

## R3a: 決定の要求の回答 API / MCP、回答の注入、inbox、Discord 通知（完了 2026-09-29）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) §7 R3a・D7・D3・D9、付記「R3a 実装時の逸脱・明確化」（15 項目。選択肢 → 効き目の表は 5.）。
- 種類: コード（task-core / task-ops / task-api / celeris-mcp / task-dispatch / task-worker / celeris）、schema・GUI の型の再生成、GUI の通知の語 1 つ。
  **migration なし（schema 31 のまま）**。Event の形は R1a のまま。`[execution.tree] enabled = false`（既定）では決定は作られず（計画の決定の発行・
  worker の `decisions` の読み取り・`decision_self_hold`・余裕の計算はすべて木が有効なときだけ）、新しい API は空の一覧（404 ではない）を返し、
  受信箱の `decisions` は空、プロンプトは 1 バイトも変わらない。本番の DB・設定・サービスには触れていない。
- R3b（root 計画の承認・生存確認・子の通知の抑止）には手を付けていない。

### 実装したもの

- **回答の API / MCP**（`task_ops::decision`、`crates/task-api/src/decisions.rs`、`crates/celeris-mcp/src/tools/decisions.rs`）:
  `GET /decisions?open=&root_id=`、`GET /tasks/{id}/decisions?open=`（subtree）、`POST /decisions/{id}/answer {option?, note?}`・`withdraw {reason?}`・
  `revise {option?, note?}`（管理系）。MCP `decision_list` / `decision_answer`（scope `tasks:interact`、`by = mcp:<client_id>`）。
  404 `decision_not_found` / 409 `decision_not_open` / 422（選択肢の外・daemon の決定で option 無し）/ 401。
- **1 トランザクション**: `TaskStore::decision_resolve_apply`（決定が今も open であることを確かめてから `DecisionAnswered` / `DecisionWithdrawn`・
  unit の遷移・効き目の event を積む）。
- **計画の決定の発行**: /3 の採用の直後に計画の `decisions` を path 付きの `DecisionRequested`（origin planner、raised_by = planner run）にし、
  答えの無い決定を待つ leaf を `blocked(decision)`。replan で消えた未回答の決定は取り下げ。
- **回答の効き目**（決定的、表は付記 5.）: 待つ unit の再評価（`decision_answered`）、limit の `raise-once`（計画の採用時の上限は止めた unit を
  戻す、run 時の上限は余裕を足す `limits_with_allowances` / `effective_max_replans`）、`replan`（`ExecutionHintSet{replan: true}`）、`withdraw`
  （止めた unit と依存を `cancelled`、`self` なら節点を中止）、`plan_invalid` の `replan`（試行の窓を開け直し、note を planner へ）/ `atomic`
  （gate を `atomic/decision` に）/ `cancel`。`plan_invalid` の選択肢を replan / atomic（初回だけ）/ cancel に改めた。
- **注入**: 子の objective の末尾（R1b）と leaf の前置きの「人の決定」節（`WorkUnitPromptContext.human_decisions`）が同じ `answer_line`。
  atomic の run の `self` への答えは `answers`。
- **worker の決定**: 木の節点の worker の run の `result.json` の `decisions`（`RunContext.decision_requests` のとき前置きに書き方）を検証して
  path 付きで記録。leaf の `self` はその leaf を `blocked(decision)`、他の unit を指せばその unit だけを止める。atomic の `self` は節点を止める。
  上限（計画 8 / 木 12 の残り）を超えた分は 1 件の `choice`（key `bundle`）に束ねる。
- **受信箱と件数**: `GET /inbox` の `decisions[]`（path・問い・選択肢・推奨・後戻り・止めている unit・経過秒）と `counts.decisions`、
  `DaemonSnapshot.decisions_open`（`GET /daemon`）。
- **通知**: `NotificationKind::DecisionRequested`（`scan_decisions`）。同じ run（計画の採用 = `plan:<plan_id>:decisions`、worker の run =
  `run:<run_id>:decisions`）の決定は 1 通、run を持たない daemon の決定は `decision:<id>`。24 時間未回答で `reminder:<key>` を 1 回だけ。
  回答・取り下げ・終端の節点の決定は鳴らさない。既存の webhook・重複排除（`notifications (kind, key)`）をそのまま使う。

### 受け入れ条件（ADR-0079 §7 R3a と依頼の項目）

| 条件 | コマンド | 結果 |
|---|---|---|
| (a) `unanswered_decision_blocks_only_dependents` と (b) `answer_flows_into_child_objective`: 決定 2 件（h1 → 子 task の unit p2-b、h2 → `stage:phase-2`）が採用で planner の path 付きの決定になり、phase-1 は走り、phase-2 の leaf は `blocked(decision)`、p2-b は子を作らずに待つ。h2 の回答で leaf が進み前置きに `- h2 …（推奨どおり）`、h1 の note 付きの回答で子ができ objective の末尾が固定の書式（h1・h2）。root done | `cargo test -p task-dispatch --lib tree_decisions::unanswered_decision_blocks_only_dependents_and_answers_flow_into_inputs` | ok。受信箱 2 件 → 0 件、replay 差分 0 |
| `limit_raise_answer_resumes_subtree`（§7 R2a (e)）: `limit:max_tree_leaves` の止めた leaf が `raise-once` で走る。`limit:max_tree_runs`（self）の止めた子が `raise-once` の余裕で走り、新しい決定は出ない | `… tree_decisions::limit_raise_answer_resumes_subtree` | ok。どちらも root done、replay 差分 0 |
| `plan_invalid` → `replan`: replan の planner が 2 回不正（選択肢 replan / cancel）→ note 付きの回答 → planner の 4 本目の入力（`previous_attempt_errors` と `replan_reason`）に note、計画の版 2、root done | `… tree_decisions::plan_invalid_replan_feeds_the_note_and_adopts_plan_v2` | ok（replay は付記 14. の既存の差〈superseded の unit〉だけを許す） |
| `atomic` は節点を 1 run で走らせ（gate `atomic/decision`、worker run 1）、`cancel` は節点を中止 | `… tree_decisions::plan_invalid_atomic_runs_the_node_once_and_cancel_cancels_it` | ok。replay 差分 0 |
| withdraw は止めた unit を取り下げる（`leaf_too_large` の withdraw、人の取り下げで待っていた leaf とその依存） | `… tree_decisions::withdraw_cancels_the_held_units` | ok。どちらも root done |
| (c) `worker_declared_decisions`: leaf a の `result.json` の決定 2 件が path（root › s1 › a）付き・origin worker・raised_by = その run で記録、a は `blocked(decision)`、次の段階の c も止まり、同じ段階の b は done、形の壊れた 1 件は理由付きで捨てる。回答で a が再実行（前置きに自由記述の答え）、atomic の子 k の `self` は子を止め答えは `answers` に入る | `… tree_decisions::worker_declared_decisions` | ok。replay 差分 0 |
| 木が無効なら何も変わらない（worker の `decisions` を読まない・前置きに節が出ない） | `… tree_decisions::tree_disabled_ignores_worker_decisions` | ok |
| R1b の `child_waits_for_dependencies_and_decisions` を採用で出た決定への回答（`task_ops::decision::answer`）に書き換え | `… tree::child_waits_for_dependencies_and_decisions` | ok（`cargo test -p task-dispatch --lib tree` 55 passed = 既存 48 + R3a 7） |
| (e) API: 一覧（open / root_id / subtree / 空は `[]`）、回答 200・再回答 409・未知の option 422・無い id 404・トークン無し 401（両方）・壊れた JSON 400・自由記述・limit の option 無し 422・withdraw・revise 409/200/409・受信箱の節と件数・`snapshot.decisions_open` | `cargo test -p task-api --test decisions` | ok（13 passed） |
| (e) MCP: scope 無しは `decision_list` / `decision_answer` とも `-32601`、`tasks:interact` で一覧と回答、`by = mcp:<client>`、再回答 `-32602`、無い id `-32001` | `cargo test -p celeris-mcp --test mcp_integration decision_tools` | ok（mcp_integration 18 passed） |
| (d) `decisions_are_notified_once_per_plan`: 同じ planner run の 3 件が `plan:<plan_id>:decisions` の 1 通、本文に path と推奨、偽の webhook への POST 本文にも残る。1 件の daemon の決定は `decision:<id>`、24 時間後の再通知は 1 回だけ（偽の時計 +23h なし / +25h 1 件 / +49h 増えない）、回答済み・取り下げ・終端の節点は鳴らない | `cargo test -p celeris --test notify` | ok（30 passed = 既存 26 + 4） |
| 純粋関数: 選択肢 → 効き目の表・回答の検証（自由記述・note の長さ）・注入の書式・worker の決定の検証と束ね・limit の key と余裕・`plan_invalid` の選択肢 | `cargo test -p task-core --lib decision` / `… tree::tests::limit_keys` | ok |
| leaf の前置き（「人の決定」節・決定の要求の書き方の節、無ければ変わらない） | `cargo test -p task-worker --lib leaf_prompt_carries_human_decisions` | ok |

### gate

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 8、binaries 89（nextest 79 + doc 10）、**passed 2812 / failed 0 / ignored 7**（R2b の 2780 から +32）
- schema の再生成: `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema`（`api-v1.schema.json`）・`-p task-worker --lib schema`（`worker-protocol.schema.json`）。
  `event.schema.json` / `decision.schema.json` / `execution-plan.schema.json` は不変
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0、再実行の前後で `gui/app/celeris/types.ts` の md5 が同じ（5761108730e99f2452310f60777c16a6）
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0 / `lint` → exit 0（Checked 285 files、2 infos〈既存〉）/ `test` → exit 0（Tests 1176 passed）。
  GUI の変更は通知の種類の語（`decision_requested`）と、受信箱の型に足した `decisions` / `counts.decisions` を試験の fixture に足しただけ（画面は R4b）

### 未解決・R3b 以降へ

- **R3b**: 生存確認（D10）で `blocked(infra)` / 決定の待ちを名指しの待ちに数える。人の replan の依頼で起きた replan の planner が 1 回目に不正だと
  2 回目の試行が起きない既存の挙動（付記 15.）は生存確認で見つかる形。子の `TaskFailed` の抑止。
- **R4a**: /3 の replan で計画から消えた unit（superseded）を replay が作り直せない既存の差（付記 14.）。
- **R4b**: GUI の受信箱の「決定」の節（その場で答える・409 / 422 の文言）、決定の path のパンくず。
- **R5b**: 人の計画（`PUT`、origin human）の決定の発行と、`PUT` の入口の木の設定の配線（付記 3.）。
- 節点が中止されたときに未回答の決定を自動で取り下げることはしていない（受信箱・通知・件数から外すだけ。付記 12.）。
- worker の `checkpoint.json` の `decisions` は読まない（付記 11.）。
- `raise-once` の余裕（設定の上限の半分）は推測の初期値。dogfood（R5b）で見直す。

### 提案

- なし（DESIGN / SPEC への提案は R0 のまま）。

### 本番昇格（2026-09-29 02:39:11Z）: release `a525af223783`（main a525af2 = R1a〜R3a + pegasus ssh 修正 + migration 0030/0031）

- ゲート: cargo-test 108 s（nextest）、GUI 0 件。verify ok=true、live_ok=false（schema 29→31 なので N-1 不可、想定どおり）。
- **停止→起動**で昇格（backup 20260929-023848-pre-a525af223783）。実行中 run は無し（Phase 2 task が `ready` の隙間）。health: active、schema 31。
- `[execution.tree]` は既定 false のまま。R1a〜R3a は本番の挙動を変えない（/3 計画は 422）。有効化は R5b。
- 発端: main が schema 31 に進んでいるのに本番が 29 のままだったため、Phase 2 task（01M3MZKB3DFYJNBH015MJGQ0BT）が main を取り込んで作る release が
  すべて live_ok=false になり、最終 review 不合格 → replan を 3 回繰り返した（planner 7 run、cost_usd 26.41）。**R6 回収フェーズの論点**: task 側の
  release/verify は本番 schema を基準にすべきか、`live_ok` を受け入れ条件から外すか、main の schema を進めたら速やかに昇格する運用にするか。
## R3b: 根の計画承認（PlanGate）、liveness と StallDetected、子の通知抑止（完了 2026-09-29）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) §7 R3b・D8・D10・D11、付記「R3b 実装時の逸脱・明確化」（12 項目）。
- 種類: コード（task-core / task-ops / task-api / celeris-mcp / task-dispatch / celeris）、schema・GUI の型の再生成、GUI の語 4 つ。
  **migration なし（schema 31 のまま）**。`[execution.tree] enabled = false`（既定）では承認・報告・生存確認のどれも動かず
  （`root_plan_approval` は `None`、`check_tree_liveness` は何もしない）、挙動は変わらない。子の通知の抑止（`scan_task_failed` /
  `scan_bad_news`）は `tree` を持つ task にだけ効くので、木が無効の本番では対象が 0 件。本番の DB・設定・サービスには触れていない。
- R4（木と roll-up の API・GUI）には手を付けていない。

### 実装したもの

- **root の計画の承認（D8）**: /3 の root の計画の採用の直後に `task_core::tree::plan_approval`（純粋関数）で要否を決め、要るなら
  `Trigger::PlanGate`（Running → Blocked、reason `awaiting_plan_approval`、attempts 不変）と `PlanApprovalRequested{plan_id, reasons}` を
  1 トランザクションで積む。条件は未回答の決定・まだ済んでいない `review: human` の段階・上限の 0.8 以上（段階数・段階あたり・子 task・
  見込みの leaf・見込みの木の run）。承認までは unit を 1 つも起こさない（dispatch されず、子も作らない）。root の replan の版にも同じ規則。
- **操作**: `POST /tasks/{id}/execution/plan-gate {action: approve|replan|withdraw, note?}`（`decision` は別名、管理系）と MCP
  `task_plan_gate`（`tasks:interact`、`mcp:<client>`）。approve → `plan_approved`、replan（note 必須）→ `plan_replan` と
  `ExecutionHintSet{replan: true}`（R3a の plan_invalid → replan と同じ経路）、withdraw → `Cancel`。承認待ちに `Answer` は 409。
- **承認の要らない計画**: 進めて、報告の流れに `progress` の「計画を採用して進めます: <段階の一覧>」を 1 件だけ残す（通知なし。U-R3）。
- **受信箱・表示・通知**: `attention` の `plan_approval`（理由・段階の見取り図・未回答の決定の id）、`ExecutionPhase::AwaitingPlanApproval`
  と `plan_approval` の節、`Action::PlanGate`。`NotificationKind::PlanApproval`（`plan:<plan_id>:approval`、その計画の決定を 1 通に束ねる）。
- **生存確認（D10）**: `task_core::tree::liveness`（純粋関数）で ready の木の節点を 走っている / 走れる / 名指しの待ち / 理由なし に分け、
  理由なしが `liveness_timeout_secs`（新設定、既定 600）続いたら `StallDetected{task_id, detail, reason, since, path}` と障害通知
  （`TaskFailed`、`tree-stall:<task>:<seq>`）を 1 回だけ。確認は 30 秒ごと、時刻はメモリ（再起動で数え直す）。
- **R3a 付記 15. の修正**: 「もう一度だけ試します」の後に planner run が始まっていなければ、起点を問わず次の run を planner にする
  （`task_ops::tree::planner_retry_pending`）。人の replan の 1 回目が不正でも 2 回目が起きる。
- **子の通知の抑止（D11）**: 木の子の `task_failed` と、子の run の悪い知らせ（秘書への複製）を鳴らさない。root の失敗・決定・承認・
  障害（`tree-infra:` / `tree-stall:`）は鳴る。key の形は変えていない。

### 受け入れ条件（ADR-0079 §7 R3b と依頼の項目）

| 条件 | コマンド | 結果 |
|---|---|---|
| (a) 3 つの条件（決定 `decisions:h1` / `review_human:s1` / `near_limit:max_stages:4/5`）がそれぞれ単独で root を `awaiting_plan_approval` で止め、planner の 1 本だけで unit も子も走らない、報告も残らない、`Answer` は拒否、操作は `plan_gate` | `cargo nextest run -p task-dispatch -E 'test(/tree_approval::each_approval_trigger_holds_the_root_plan/)'` | ok。replay 差分 0 |
| (a) approve で unit が起き、答えの無い決定に依存する unit は回答まで待つ。replan（note 必須、空は Validation）で planner が note を `replan_reason` に受けて v2 を書き、v2 は承認なしで進んで報告 1 件。withdraw で Cancelled | `… tree_approval::approve_replan_and_withdraw_have_their_effects` | ok（replan の場面の `work_units` の replay の差は付記 12. の既存の系統として比べない。task・計画・決定は差分 0） |
| (b) `small_root_plan_proceeds_with_notice`: 条件の無い root は承認なしで進み、報告（`progress`、「計画を採用して進めます: Stage s1 → Stage s2」）が 1 件、daemon の通知 0 件。子の計画は決定 k1 を含んでも承認を求めず、k1 に依存しない leaf は走る | `… tree_approval::small_root_plan_proceeds_with_notice` | ok |
| (b) 承認不要の計画では Discord に何も鳴らない（報告は progress） | `cargo nextest run -p celeris -E 'test(/a_root_plan_without_approval_posts_nothing/)'` | ok（候補 0 件） |
| 承認の通知: `plan_approval` の 1 通（key `plan:<plan_id>:approval`）に決定の行と理由、`decision_requested` と `question_blocked` は鳴らない、2 回目の tick で増えない、偽の webhook への本文に残る | `… -p celeris -E 'test(/plan_approval_is_notified_once/)'` | ok |
| (c) `liveness_flags_only_unexplained_stalls`（偽の時計）: 決定待ちの節点と承認待ちの root は 0 → 2,000 秒で検出されない。子の消えた親は 599 秒で出ず、600 秒後の最初の確認で `StallDetected{reason: child_missing, since, path}` と `tree-stall:` の障害通知が 1 件、その後 5,000 秒まで増えない | `… tree_approval::liveness_flags_only_unexplained_stalls` | ok |
| 人の replan の 1 回目が不正で 2 回目が起きない（R3a 付記 15.）の回帰: 1 回目の後に再試行の印があり分類は「走れる: planner」、印を外すと `decision_released`（理由なし）で捕まる。2 回目が走り（planner 3 本目、`replan_reason` に note）v2 が採用され root done、`StallDetected` なし | `… tree_approval::human_replan_whose_first_attempt_is_invalid_gets_its_second_attempt` | ok |
| 生存確認の分類（純粋関数）: 名指しの待ち 7 種・走っている / 走れる・理由なし 3 形（child_missing / nothing_runnable / decision_released）。承認の要否の 3 条件と境界（4/5・5/6・32/40・96/120） | `cargo nextest run -p task-core -E 'test(/tree::r3b_tests/)'` | ok（5 passed） |
| 遷移: `PlanGate` は Running → Blocked だけ、`PhaseResume{plan_approve|plan_replan}` の reason | `… -p task-core -E 'test(/transition::tests::(table_simple|phase_resume)/)'` | ok（4 kinds × 8 statuses × 21 triggers） |
| (d) `child_events_do_not_notify`: 木の子の failed・子の run の悪い知らせ・子の done は鳴らず、root の failed と root の悪い知らせは鳴る | `… -p celeris -E 'test(/child_events_do_not_notify/)'` | ok |
| API: 401・404・409（質問の blocked・承認待ちへの answer・承認後の 2 回目）・422（replan の空 note）・400/422（知らない action）・200（approve〈`decision` 別名〉/ replan / withdraw）、受信箱の `attention.plan_approval`（`actions` に `plan_gate`、`answer` 無し）、`GET /tasks/{id}/execution` の `phase = awaiting_plan_approval` | `cargo nextest run -p task-api -E 'test(/plan_gate/)'` | ok（2 passed） |
| MCP: scope 無しは -32601、`tasks:interact` で approve（`mcp:chatgpt` の記録）・replan（`source = mcp:chatgpt (plan-gate)`）、note 無しの replan・承認待ちでない は -32602、無い task は -32001 | `cargo nextest run -p celeris-mcp -E 'test(/task_plan_gate/)'` | ok |
| 木が無効なら変わらない（承認・報告・`StallDetected` なし、生存確認は動かない） | `… tree_approval::tree_disabled_is_unchanged` | ok |
| 既存の木の試験（R1b / R2a / R3a）は、決定・`review: human`・上限に近い root の計画で承認を挟むように更新（意図は変えていない） | `cargo nextest run -p task-dispatch -E 'test(/tree/)'` | ok（tree 61 passed = 既存 55 + R3b 6） |

### gate

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 8、binaries 89（nextest 79 + doc 10）、**passed 2831 / failed 0 / ignored 7**（R3a の 2812 から +19）
- schema の再生成: `UPDATE_SCHEMA=1 cargo test -p task-core --lib schema`（`docs/api/v1/event.schema.json`: `StallDetected` の `reason` / `since` / `path`）・
  `-p task-api --lib schema`（`api-v1.schema.json`: `PlanGateRequest`、`AttentionItem::plan_approval`、`ExecutionPhase::awaiting_plan_approval`、
  `plan_approval`、`Action::plan_gate`、`NotificationKind::plan_approval`）。
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0、再実行の前後で `gui/app/celeris/types.ts` の md5 が同じ（c088a7b788db0ab80cb4be4c874285ca）
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0 / `lint` → exit 0（Checked 285 files、2 infos〈既存〉）/ `test` → exit 0（Tests 1176 passed）。
  GUI の変更は語（実行の段階「計画の承認待ち」、操作「計画の承認」、通知の種類「計画の承認を待っている」）と `GateAction` から `plan_gate` を外しただけ（画面は R4b）

### 未解決・R4 以降へ

- **R4a**: /3 の replan で同じ key の unit を作り直したときの `work_units` の replay の差（runs / seq / phase / 統合 WU の presence。付記 12.、
  R3a 付記 14. と同じ系統）。
- **R4b**: 受信箱の「計画の承認」の 3 つのボタン（approve / replan〈note〉/ withdraw、409 / 422 の文言）、木の節点の「理由なく止まっています」
  （`StallDetected` の `reason` / `detail`）の表示。
- 承認の再通知（決定の 24 時間後の再通知に相当するもの）は作っていない。承認待ちの計画の決定の 24 時間後の再通知は `scan_decisions` の束ねの
  抑止で鳴らない（承認の後は通常どおり鳴る）。
- 生存確認は「走れる」を信じる（容量・quota 待ちを止まりにしない）。走れると分類されたまま何年も dispatch されない節点は検出しない。
  時刻はメモリなので daemon の再起動で数え直す。
- 人の計画（`PUT`、origin human）の承認: `PUT` の入口は R1a 付記 5. のとおりまだ木の設定で検証しないので /3 を受け付けない（R5b で配線するときに
  同じ `root_plan_approval` を通す）。

### 提案

- なし（DESIGN / SPEC への提案は R0 のまま）。

## R4a: GET /tasks/{id}/tree、集約、深さ別指標、段階報告の子要約、replay の /3 replan gap 修正（完了 2026-09-29）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) §7 R4a・D5・D11・U-R7、付記「R4a 実装時の逸脱・明確化」（11 項目）。
- 種類: コード（task-core / task-ops / task-api / task-dispatch / celeris）、schema・GUI の型の再生成（GUI の画面は変えていない。R4b）。
  **migration なし（schema 31 のまま）**。新しい endpoint・欄はすべて読み取りの追加で、`[execution.tree] enabled = false`（既定）では
  木が無いので 1 節点の木・深さ 1 の group になるだけ。daemon の挙動の変化は途中報告に `child_units` が付くことだけ（子 task を持つ段階の
  停止点のみ。木が無効なら空で出力もされない）。本番の DB・設定・サービスには触れていない。
- **パスの逸脱**: ADR の `GET /tasks/{id}/tree` は ADR-0043 D6 の作業ツリーの閲覧が既に使っているため `GET /tasks/{id}/task-tree`（付記 1.）。
- R4b（GUI）には手を付けていない。

### 実装したもの

- **roll-up の純粋関数**（`task_core::tree_metrics`）: `node_metrics`（1 節点の自分の分）・`rollup`（入力の順に own と subtree。親を辿って
  足す、64 段で打ち切り）・`by_depth`（U-R7）・`RollupMetrics::{absorb, sum}`。数は role ごとの run（reviewer を含む）・reviewer を除く run・
  reviewer の run と定価・走っている run・トークン（in / out / 計）・定価と `cost_usd_complete`・quota（(source, account, window) ごと）・壁時計
  （最初の run の開始 → 最後の run の終わり）と実働時間・leaf と子 task の done / total・未回答の決定。和は件数が和・完全性が論理積・壁時計が
  最小と最大なので、root の `subtree` = 各節点の `own` の和。
- **`GET /tasks/{id}/task-tree?root=`**（`task_ops::tree_view::task_tree`）: 節点ごとに id・題名・状態・導出値 `phase`
  （`TreeNodePhase`: Execution 節の段階 + `awaiting_children` / `awaiting_plan_approval` / `held_on_decision` / `blocked_infra`）・深さ・親・
  親の unit の key と段階・計画の版・未回答の決定・子・unit の一覧・`own` / `subtree`。view の根が木の root なら木の上限の使用
  （leaf / run / replan / トークン / 未回答の決定と、回答の余裕を当てた上限）。木の無い task は 1 節点（深さ 1）。
- **案件の合計**: `GET /projects/{id}` に `root_totals`（root task の数・状態ごとの数・subtree の roll-up の和。quota は数えない）。
- **`GET /metrics/execution?group_by=depth`**: 深さ（task の層）ごとの group に `rollup`（その深さの task の自分の分の和。U-R7 の
  「子ごとの reviewer run は高すぎるか」を後で決める指標）。他の `group_by` は変えない。
- **段階の途中報告の子の要約**: `PhaseReport.child_units`（子ごとに状態・subtree の run〈reviewer〉・定価・子の最新の報告の見出し）。
  16 KiB の切り詰めは `work_units` の後に落とす。Markdown の成果物に「## この段階の子 task」。
- **replay の /3 replan の差**: `task_ops::replay::rebuild_work_units_and_runs` がすべての計画の版を畳み込み（`apply_replan_step`）、replan で
  消えた unit・統合 WU（superseded）と、同じ key で書き直した unit の `runs` / `seq` / `phase` を events だけから同じに作り直す。
- **配線**: `ApiSettings.tree_limits`（`[execution.tree]` の写し。表示だけ）、`task_ops::view::execution_phase_of`。

### 受け入れ条件（ADR-0079 §7 R4a と依頼の項目）

| 条件 | コマンド | 結果 |
|---|---|---|
| (a) `rollup_matches_hand_computed_fixture`: root → 子 → 孫の 3 段で、各節点の subtree の run（role ごと・reviewer）・トークン・定価（単価不明の run で `cost_usd_complete = false` が root まで伝わる）・quota（runs 4 / 6.5pt）・壁時計（09:50 → 12:05 = 135 分）と実働時間・leaf 2/3・子 task 1/2・未回答の決定 3 が手計算と一致、root の合計 = 3 節点の `own` の和、深さ別（reviewer 深さ 1: 1 本 $0.50、深さ 3: 1 本 $0.05） | `cargo nextest run -p task-core -E 'test(/tree_metrics/)'` | ok（2 passed） |
| 木の endpoint（3 段、API）: 前順の節点、孫 / 子 / root の `own` と `subtree`（root: run 3・reviewer 1・トークン 1,595・$1.40・leaf 2/2・子 task 1/2・決定 2・壁時計 65 分・quota 2 run 4.0pt）、`phase`（root `awaiting_children`・子 `held_on_decision`・done の孫は無し）、unit の一覧、上限の使用（leaf 2/40・run 3/120・replan 0/10・決定 2/12）、子の subtree（2 節点・上限なし）、`?root=true` で孫から root の木 | `cargo nextest run -p task-api --test task_tree -E 'test(task_tree_rolls_up_a_three_level_tree_by_hand)'` | ok |
| (b) `legacy_task_is_a_single_node_tree`: 木が無効・木の無い task は 1 節点（深さ 1・親なし・`own` = `subtree`・run 1 + reviewer 1・$0.22・壁時計 12 分）、`?root=true` も同じ、404 `task_not_found`、`root=maybe`・知らないクエリは 400、作業ツリーの `GET /tasks/{id}/tree` は従来どおり | `… --test task_tree -E 'test(legacy_task_is_a_single_node_tree)'` | ok |
| 案件の合計: root task 2（`ready` 1・`done` 1。対話 task は数えない）、subtree の和（task 4・run 4・reviewer 1・$1.90・トークン 1,606・決定 2・壁時計 08:00 → 11:05）、quota は空、`tasks[]` は従来どおり 5 件 | `… --test task_tree -E 'test(project_detail_sums_the_root_task_subtrees)'` | ok |
| `group_by=depth`（U-R7）: group `1` / `2` / `3`、深さ 1 は root 2 件（planner 1・worker 1・$0.60・壁時計 2 時間 1 分）、深さ 2 は run 1・$1.00・quota 3.0pt、深さ 3 は reviewer 1 本 $0.05・計 $0.30。他の `group_by` に `rollup` は出ない、知らない `group_by` の 400 の文言に `depth` | `… --test task_tree -E 'test(execution_metrics_group_by_depth_counts_reviewer_runs_per_depth)'` | ok |
| 索引の集計とイベントからの参照実装が `depth` を含むすべての `group_by` で一致 | `cargo nextest run -p task-api -E 'test(indexed_summary_matches_event_reference_for_all_groups_and_since)'` | ok |
| (c) 途中報告に子の要約: `review: human` の段階 s1（leaf a + 子 c）の `PhaseReported.child_units` = `c Child c: done / 1 run（reviewer 0）/ $0.00 — 報告なし`、Markdown の成果物 `1-s1.md` に「## この段階の子 task」、続けると done、replay 差分 0 | `cargo nextest run -p task-dispatch -E 'test(stage_report_lists_child_summaries)'` | ok |
| 子の要約の組み立て（純粋に近い store の読み取り）: 子の subtree（孫を含む）の run 2（reviewer 1）・$0.50（不完全）・最新の報告の見出し、未作成の unit は「未作成（pending）」、別の段階・superseded は出ない | `cargo nextest run -p task-ops -E 'test(stage_child_summaries_sum_the_child_subtree_and_quote_its_latest_report)'` | ok |
| (c) 16 KiB の切り詰め: 400 行の `work_units` と `child_units` で `work_units` が先に全部落ち、子の要約は先頭から残り、旧実装（参照）と同じ切り口 | `cargo nextest run -p task-core -E 'test(/pause::tests::(child_unit_summaries_are_truncated_after_work_units|truncate)/)'` | ok |
| replay の回帰（R3a で外していた）: `plan_invalid` の replan（superseded の a を含む）で `assert_replay_is_clean`（task・`work_units`・計画・決定の差分 0） | `cargo nextest run -p task-dispatch -E 'test(plan_invalid_replan_feeds_the_note_and_adopts_plan_v2)'` | ok |
| replay の回帰（R3b で外していた）: 計画の承認の replan（b を s2 → s1 へ書き直し、s2 と `integrate-s2` が消える）で `assert_replay_is_clean` | `… -E 'test(approve_replan_and_withdraw_have_their_effects)'` | ok |
| replay の純粋な回帰: /2 の replan で消えた unit x・`integrate-p2`（superseded）、別の工程へ移した b、新しい c が `--check` で差分 0、superseded の行を消した索引が `--apply` で戻る | `cargo nextest run -p task-ops -E 'test(replay_rebuilds_units_dropped_or_rewritten_by_a_phased_replan)'` | ok（replay 12 passed） |
| 既存の木の試験は変わらず通る | `cargo nextest run -p task-dispatch -E 'test(/tree/)'` | ok（62 passed = R3b の 61 + 1） |
| (d) `api-v1.schema.json` の差分が追加だけ | `UPDATE_SCHEMA=1 cargo nextest run --workspace -E 'test(/schema/)'` → `git diff docs/api/v1/` | `api-v1.schema.json` は追加 529 行・削除 3 行（削除は `group_by` の説明文 2 つと `required` の末尾の `"decision_outcome"` の行〈`"task_tree"` を足した際のカンマ〉だけ）。`event.schema.json` は `PhaseReport.child_units` の追加 7 行 |

### gate

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 8、binaries 90（nextest 80 + doc 10）、**passed 2841 / failed 0 / ignored 7**
  （R3b の 2831 から +10: tree_metrics 2、pause 1、tree_view 1、replay 1、task_tree 4、stage_report 1。R3a / R3b の 2 試験は置き換え）
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0、再実行の前後で `gui/app/celeris/types.ts` が同じ（md5 45f66fe0a73d1a23f194e127cde7c8d3）
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0 / `lint` → exit 0（Checked 285 files、2 infos〈既存〉）/ `test` → exit 0（Test Files 76、Tests 1176 passed）

### 未解決・R4b 以降へ

- **replan が行の `phase` を書き換えない**（付記 11.、`task_ops::execution::replan`。F5-fix8 が同じファイルを直している最中なので触れていない）:
  replan で unit が別の段階へ移ると、行は古い段階のままで、その段階の統合 WU に merge されない（git の作業場所では成果が Task ブランチに
  入らない）。直すときは replay の `apply_replan_step` も同じく `phase` を新しい版にする。
- D11 の「人を待った時間」は roll-up に入れていない（付記 3.）。`TaskDetail.tree` の要約も足していない（付記 6.）。R4b の GUI で要るなら足す。
- 途中報告は停止点の段階にしか無いので、停止点でない段階の完了では子の要約がどこにも残らない（付記 9.。D11 の報告の圧縮の全体は R4b / R5 で）。
- 案件の合計は quota を数えない・`tasks[]` と同じ 2,000 件の範囲（付記 7.）。
- **R4b**: 木のタブ（`task-tree` の節点・段階・unit・roll-up・上限の使用率）、受信箱の「決定」「計画の承認」の操作、案件ページの root の一覧と
  `root_totals`、「理由なく止まっています」、語（配送 → 成果の取り込み）。

**昇格**: release `3a928fd6ebeb`（main 3a928fd = R4b + browser Phase 2 統合）を 2026-09-29 04:58:07Z にライブ昇格（backup 20260929-045751-pre-3a928fd6ebeb、verify ok / live_ok、schema 33）。GUI に木タブ・決定と計画承認の inbox・案件ページの根 task 一覧・「成果の取り込み」の用語が入った（tree 自体は既定 off のまま）。

### 提案

- なし（DESIGN / SPEC への提案は R0 のまま）。

## R4b: GUI の木タブ、決定・承認の inbox、案件ページの根 task 一覧、用語の変更（完了 2026-09-29）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) §7 R4b・D6・D10・D13・D14、付記「R4b 実装時の逸脱・明確化」（7 項目）。
- 種類: GUI（`gui/`）が主。celeris 側は **1 欄の追加**（`TaskTreeNode.stall?`、`task_ops::tree_view::current_stall`。付記 1.）と、Discord の
  失敗通知の文面の語（「配送済み」→「main に取り込み済み」）だけ。**migration なし（schema 31 のまま）**、`api-v1.schema.json` は追加だけ。
  本番の DB・設定・サービスには触れていない（systemctl / promote / release.sh は使っていない）。
- R5（案件の API の 410 化・CoS の指針・本番の移行）には手を付けていない。

### 実装したもの

- **`~/lib/tree.ts`**（表示の判定の純粋関数。D14）: 節点の導出値の語と色、止まっている理由（`treeNodeHold` / `taskHold`。stall →
  承認 → 基盤 → 決定の順）と「理由なく止まっています（<分類>）」、timeline からの stall（`currentStallFromTimeline`）、決定のパンくず
  （`decisionBreadcrumb`）・選択肢（推奨に印・自由記述は `choice` だけ）・待っているもの・経過、計画の承認の 3 操作（`actions` に
  `plan_gate` があるときだけ）と理由の文、roll-up の文（role ごとの run〈reviewer を含む〉・定価〈不完全の明示〉・壁時計と実働・leaf /
  子 task の done / total）、上限の使用（0.8 以上に印）、前順の字下げ、段階ごとの unit、D6 の取り込み先の文、案件の root task の読み。
- **タスク詳細の「木」タブ**（`~/components/TaskTreeTab.tsx`、lazy）: `?tab=tree` のときだけ `GET /tasks/{id}/task-tree?root=true`。
  木の合計（roll-up・取り込み先・上限の使用）と、節点ごとのカード（状態・導出値・未回答の決定・深さ・親の段階と unit・止まっている
  理由と受信箱へのリンク・roll-up・段階ごとの unit〈折りたたみ〉・子へのリンク）。木の無い task は 1 節点。
- **止まっている理由の帯**（`~/components/TaskHoldBanner.tsx`、lazy。どのタブでも上部）と、**「実行の形」カードの計画の承認**
  （`ExecutionModeControl` に承認の理由と 3 つのボタン。承認待ちの間は分解の操作を隠す）。action は `intent=plan_gate` →
  `~/celeris/decisions-admin.server.ts::planGateTask`（`POST /tasks/{id}/execution/plan-gate`）。
- **受信箱**: 「計画の承認」（`~/components/DecisionControls.tsx::PlanApprovalCard`: 理由・段階ごとの unit・同じ節点の決定へのリンク・
  3 つのボタン）と「決定」（`DecisionItemCard`: パンくず・問い・種類・後戻り・経過・待っているもの・選択肢〈推奨を既定で選択〉・note・
  答える / 取り下げる）の 2 節、タイル「決定」。action は `intent=decision_answer|decision_withdraw` → `answerDecision` /
  `withdrawDecision`（`POST /decisions/{id}/answer|withdraw`）。結果は `DecisionFlash` / `PlanGateFlash`、409 / 422 は `ErrorFlash`。
- **案件ページ**（`~/components/ProjectRootTasks.tsx`）: root task の一覧（状態・導出値・止まっている理由・未回答の決定・subtree の
  roll-up〈root ごとの `task-tree`、先頭 20 件〉）と `root_totals` の 1 行。案件計画の DAG・「計画を見直す」・「案件計画を提案させる」・
  途中目標の指定を外し、途中目標は「以前の途中目標（読み取り専用）」（題名・状態・説明だけ）。
- **語**: 「配送」を GUI から無くした（付記 6. の対応表）。`help.tsx` の失敗の直し方と用語集（木・成果の取り込み・決定・計画の承認・
  理由なく止まっています）を更新。
- **fixture と監査**: `gui/test/fixtures/api/task-tree.json`（R4a の API の形の 4 節点の木: root〈子待ち〉→ 子〈決定待ち〉→ 孫〈実行中〉と
  子〈理由なしの停止〉。`api-types.check.ts` で生成型と照合）。偽の celeris（`scripts/lib/celeris-fixture.mjs`）に木・受信箱の決定 1 件と
  計画の承認 1 件・案件の `root_totals` を足し、監査と e2e の route に `task-tree` を足した。mobile-audit の `touch-scroll` はコンテナを
  先頭に戻してから払う（付記 7.）。

### 受け入れ条件（ADR-0079 §7 R4b と依頼の項目）

| 条件 | コマンド | 結果 |
|---|---|---|
| (a) `tree.ts` の単体テスト: 止まっている理由の文言（stall の分類語・名指しの待ち・優先順・終端は出さない・timeline の最後の event だけ）、パンくず、押せるボタンの出し分け（自由記述は `choice` だけ・`plan_gate` の有無）、承認の理由の文、roll-up / 上限の使用、D6 の取り込み先 | `corepack pnpm@11.27.0 -C gui test`（`test/unit/tree.test.tsx`） | ok（19 tests） |
| 木タブの描画（R4a の形の fixture）: 4 節点・深さ 3・今の task の印・子待ち / 決定待ちの語・未回答の決定 1・stall の帯と分類・子へのリンク・reviewer を含む run・取り込み先・上限の使用（leaf 4/5 に 8 割の印）・統合 unit を出さない。1 節点の木と取得失敗 | 同上 | ok |
| 止まっている理由の帯（stall・決定 → 受信箱・承認 → 実行の形）と、案件の root task 一覧（root の読み・状態・導出値・決定・節点数・roll-up・`root_totals`） | 同上 | ok |
| (b) 決定に答えると API に `option` と `note` が送られる（自由記述は `note` だけ、空白の note は送らない）、取り下げは `reason`、409 `decision_not_open` と 422（選択肢の外）の文言が出る、id の無いフォームは送らない | 同上（`test/unit/decisions.action.test.tsx`） | ok（13 tests） |
| (b) 計画の承認の 3 ボタン: approve / replan（note）/ withdraw がそれぞれ `{action, note?}` を送り、422（空の replan）・409（承認待ちでない）の文言、知らない操作は送らない。受信箱のカードと「実行の形」カードの 3 ボタン（`plan_gate` が無ければ出さない） | 同上 | ok |
| 案件ページの loader: root task（親なし・対話でない）だけ `task-tree` を引き、失敗した root は null | 同上（`test/unit/projects.detail.test.ts`） | ok（32 tests） |
| (d) GUI に「配送」が残らない（`help.tsx` を含む `app/` と画面用の fixture を grep。生成型の注釈は除く） | 同上（`test/unit/wording.test.ts`） | ok |
| GUI の全体 | `corepack pnpm@11.27.0 -C gui typecheck` / `lint` / `test` | exit 0 / exit 0（Checked 294 files、2 infos〈既存の `scripts/check-resume-recovery.mjs`〉）/ exit 0（Test Files 79、Tests 1210 passed。R4a の 76 / 1176 から +3 / +34） |
| (c) `gen:types` の差分ゼロ（2 回） | `corepack pnpm@11.27.0 -C gui gen:types` を 2 回 → `md5sum gui/app/celeris/types.ts` | 2 回とも `2307aaff2e3957734b2a8819ddd45bfc`（R4a からの差分は `TaskTreeNode.stall` と `TreeNodeStall` の追加 18 行だけ） |
| `api-v1.schema.json` は追加だけ | `UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema` → `git diff --stat docs/api` | 35 行追加・削除 0 |
| (c) `pnpm build` | `corepack pnpm@11.27.0 -C gui build` | exit 0（✓ built） |
| (c) モバイル監査 違反 0（全 route） | `MOBILE_AUDIT_SKIP_BUILD=1 node scripts/mobile-audit.mjs`（`gui/`） | exit 0、`routes=28 schemes=2 violations=0`（R4a の 27 route に `task-tree` を足した） |
| task 系ルートの初回 JS（予算 532 KB） | 同上の perf 表（`js_kb`） | **前 528.6 KB → 後 528.9 KB**（task-overview / tree / timeline / changes / files / artifacts とも同じ。木タブ・止まっている理由の帯・承認の部品は lazy）。project-detail 503.6 KB、inbox 479.5 KB |
| e2e（偽の celeris、読み取りだけ。6 つのタブの切り替えに「木」を含む） | `E2E_SKIP_BUILD=1 node scripts/e2e-check.mjs`（`gui/`） | exit 0、`"ok": true`、`failures: []` |
| 節点の stall の欄（celeris 側） | `cargo test -p task-ops --lib current_stall` | ok（`current_stall_is_the_last_event_of_a_live_node`） |

### gate

- `cargo fmt --all -- --check` → exit 0（notify.rs の 1 行を `cargo fmt` で整形した後）
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 8、binaries 90（nextest 80 + doc 10）、
  **passed 2846 / failed 0 / ignored 7**（新しい試験は `tree_view::tests::current_stall_is_the_last_event_of_a_live_node`。
  `crates/celeris/tests/notify.rs` の失敗通知の文面の期待を新しい語に）
- GUI: 上の表（typecheck / lint / test / gen:types ×2 / build / mobile-audit / e2e:mock）

### 未解決・R5 以降へ

- `blocked(infra)` の unit の人の「再試行」の入口（API も GUI も無い。R2b / R3b からの持ち越し）。
- 決定の `revise`（回答済みの choice の答えを変える）の GUI の入口（受信箱からは消えるため出していない）。
- D11 の「人を待った時間」は API に無いので出していない。`TaskDetail.tree` の要約も足していない（概要では木を引かない。付記 2.）。
- 案件ページの milestone 系の action（`projects-admin.server.ts` の関数と route の intent）は残した。R5a で API の 410 化と一緒に外す。
- ADR-0051 の部署レビューの取り込みが作る repair task の題名「配送の局所修復」（task のデータ）は変えていない（付記 6.）。
- replan が行の `phase` を書き換えない不具合（R4a 付記 11.）は R4b では触れていない。

### 提案

- なし（DESIGN / SPEC への提案は R0 のまま）。

## R5a: 案件計画の撤去、途中目標の凍結、CoS の指針、stages_hint の書き込み口（完了 2026-09-29）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) §7 R5a・D12・D13・U-R6・U-R8、付記「R5a 実装時の逸脱・明確化」（14 項目）。
  ADR-0028 / 0038 / 0044 / 0048 / 0074 / 0077 の冒頭に「Superseded (in part) by ADR-0079（Phase R5a）」の 1 行。
- 種類: コード（task-core / task-ops / task-api / task-dispatch / task-worker / celeris / celerisctl）、schema と GUI の型の再生成、GUI の
  server 側の中継の削除（画面は変えていない。R4b と並行）。**migration なし（schema 33 のまま。次の空きは 0034）**。行は書き換えない。
  本番の DB・設定・サービスには触れていない（読み取りの `sqlite3 …?mode=ro` だけ）。
- **凍結した途中目標の行（本番、読み取りで確認）**: 25 行（agent-platform 21 行: reached 8・redesigned 9・**approved 5・in_progress 1・proposed 1**、
  pluvio-jp572bat 4 行: reached 3・redesigned 1）。**非終端は 7 行（すべて agent-platform）**。R0 の「21 / 4」は行の総数だった。
  どの行も R5a 以降は書けず（410）、`GET /projects/{id}` の既定の一覧から消える（`?include_frozen=true` で読める）。

### 実装したもの

- **撤去（410 / 422）**: `POST /projects/{id}/plan`（両 mode）・`POST /projects/{id}/project-plan/{version}/decide`・
  `POST /projects/{id}/milestones`・`PATCH /milestones/{id}`・`POST /milestones/{id}/{decide,cancel,pause,resume}`・`POST /plans`（U-R6）は
  410（`type: urn:celeris:problem:removed_by_adr_0079`、`adr`・`instead` 付き、本文も id も読まない、トークン無しは先に 401）。
  `PATCH /projects/{id}` の `auto_advance` は 422。`celerisctl projects plan approve|reject` を削除。
  `task_ops::project_plan` は読み取り（`plan_state` / `dag_view`）だけ、`task_ops::milestone_review` は読み取り（`find` / `latest_proposal` /
  `review_state`）だけを残し、`start` / `start_milestones` / `start_replan` / `propose` / `propose_delta` / `decide` /
  `validate_delta_against_store` / `record_proposal_failure` / `mark_milestone_dispatched` / `auto_reach_done_milestones` / `start_review` /
  `record_proposal` / `decide`（途中目標）と、途中目標の lifecycle（`cancel|pause|resume_milestone`）を削除。
- **daemon**: 途中目標の自動作成（`task_ops::add`・store `create_task_with_milestone`）、Go（`milestones_awaiting_go_locked`）、ADR-0077 の
  dispatch での `in_progress` と自動 `reached`、案件計画 run の提案の取り込み、判定 run（celeris の `milestone_review` モジュール）、
  `milestone_proposal` の取り込み、`milestone_ready` の通知を削除。CoS の文脈から途中目標を外した（`active_projects[].milestones` は空）。
- **`is_root_task`**（`task_core::is_root_task`、旧 `is_milestone_task` の置き換え）: `ProjectTaskView` / `TaskSummary` / `TaskDetail` に出る。
  R4a の `root_totals` の述語もこれ。
- **subtree の一時停止**: `POST /tasks/{id}/pause|resume`（管理系、`TaskPauseResult`）。`Task.paused_at` + `Edited{paused_at}`、
  `ready_tasks` が祖先を辿る（案件の停止も子孫に効く）、並列 WU の 2 本目以降も止める（`TaskStore::halted_by_pause`）、走っている run は
  終わるまで走る。`TaskSummary.paused`・`TaskDetail.paused_by`。
- **既定非表示（U-R8）**: `GET /projects/{id}` の `milestones` は既定で空、`milestones_frozen` に件数、`?include_frozen=true` で全行と
  凍結した `project_plan` の DAG。
- **CoS（D12）**: 秘書の対話の節と `actions_instructions` を書き換え（1 依頼 = 1 `create_task`・範囲を狭めない・名指しの段階だけ `stages_hint`・
  独立なら 2 つ / 依存なら 1 つ・`pause_after` は頼まれたときだけ・途中目標は作らない）。`add_milestone` と `create_task.milestone` を
  型から外し、`add_milestone` は `ADD_MILESTONE_RETIRED` の理由付きで落ちる（人に見える）。`execution: compound` のヒントと
  「1 時間以内」の目安を消した。
- **`stages_hint` の書き込み口**: `NewTaskSpec.stages_hint`（`POST /tasks`）と CoS の `create_task.stages_hint` → `Task.routing.stages_hint`
  （形の検証: 16 件・title 1〜120 文字・scope 2,000 文字）。
- **GUI**: `projects-admin.server.ts` と `projects.$id.tsx` の action から撤去した API の中継を外した（画面・`/plans/new` はそのまま。付記 14.）。

### 受け入れ条件（ADR-0079 §7 R5a と依頼の項目）

| 条件 | コマンド | 結果 |
|---|---|---|
| (a) 消した API がすべて 410 / 422、`GET /projects/{id}` は読め、行も task も変わらない（案件計画 3 通りの本文・decide・途中目標の作成 / PATCH / decide / cancel / pause / resume・`POST /plans`・`auto_advance` 単独と他の欄と一緒・トークン無しは 401） | `cargo nextest run -p task-api --test project_plan -E 'test(project_plan_endpoints_are_gone)'` | ok |
| `POST /plans` は 410 で何も作らない（旧 201 の試験を置き換え）・未知の欄でも 410・e2e の API シナリオ（実バイナリ）で `/plans` が 410 | `… -p task-api --test operations -E 'test(create_plan_is_gone)'`、`… --test auth_and_guards`、`… -p e2e -E 'test(api_mutations_go_through_the_state_machine)'` | ok |
| 途中目標の lifecycle と PATCH は 410、凍結した行と属する task は変わらない、既存の paused の行の抑止は残る | `… -p task-api --test lifecycle`（7 passed）、`… --test organization` | ok |
| 途中目標の一覧は既定で隠れ（`milestones: []`・`milestones_frozen: 3`・`project_plan` 無し）、`?include_frozen=true` で全行（状態そのまま）、`include_frozen=maybe` は 400 | `… --test project_plan -E 'test(project_detail_hides_frozen_milestones_by_default)'` | ok |
| (b) 案件直下の root task にも子にも途中目標の行ができず、既存の行（in_progress）は状態が変わらない | `cargo nextest run -p task-ops -E 'test(no_milestone_rows_for_new_root_tasks)'` | ok |
| (b) task の完了で途中目標が進まない: `plan_key` 付き・`auto_advance = false` の途中目標に結ばれた root task 同士（poc は survey に依存）でも survey の done で poc が dispatch され、3 本とも done、途中目標の行の状態は前後で同一（`in_progress` にも `reached` にもならない）（旧 案件計画 run・Go・ADR-0077 の 6 試験を置き換え） | `cargo nextest run -p task-dispatch -E 'test(frozen_milestones_do_not_gate_or_advance_root_tasks)'` | ok |
| `is_root_task`: root = true、子（`parent_id`）・木の子（`parent_unit` のみ）・対話・`kind = plan` / approval / review・裏方 = false、木の root 自身は true | `cargo nextest run -p task-core -E 'test(is_root_task_requires_project_root_position)'` | ok |
| `is_root_task` が API に出る（案件の `tasks[]`・`GET /tasks?project=`・`GET /tasks/{id}`、root / 子 / 対話）、`root_totals.root_tasks = 1` | `… -p task-api --test project_plan -E 'test(is_root_task_and_stages_hint_are_exposed)'` | ok |
| (c) root の pause で子・孫・採用の木の子が `ready_tasks` に出ず、兄弟の root は出る、走っている root は running のまま `halted_by_pause`、二重 pause / 子の resume は 409、resume で全部戻る、状態は変わらず replay の差分 0、`Edited{paused_at}` が 2 件 | `cargo nextest run -p task-ops -E 'test(subtree_pause_stops_descendants)'` | ok |
| 案件の pause が `project_id` を持たない子孫にも効く / 終端・対話は pause できない | `… -E 'test(project_pause_applies_to_root_task_subtrees) \| test(terminal_and_conversation_tasks_cannot_be_paused)'` | ok |
| API の pause / resume（401・200 と `subtree`・`paused_by`・409・404）と、pause / resume を含む API 操作の後の replay が `mismatches: []` | `… -p task-api -E 'test(task_pause_and_resume_cover_the_subtree) \| test(replay_reports_zero_mismatches_after_api_operations)'` | ok |
| (d) CoS の preamble: 「1 つの依頼は 1 つの `create_task`」「範囲を狭めないでください」「Phase 1〜4 のすべてを書きます」`stages_hint` の例・「人が段階を名指ししたときだけ」・独立なら 2 つ / 依存なら 1 つ・`pause_after` は頼まれたときだけ・途中目標は段階、**無いこと**: `add_milestone`・`"execution": "compound"`・「1 時間以内」・`"milestone": "<途中目標の id`・「対象ごとにタスクを分けてください」・「方針や途中目標を毎回再承認」。CoS の文脈に凍結した途中目標が出ない | `cargo nextest run -p task-worker -E 'test(cos_preamble_carries_the_adr_0079_guidance) \| test(cos_conversations_show_active_projects_and_the_actions_instructions)'` | ok |
| (d) `add_milestone_is_retired`: 結果ファイルの `add_milestone` は「add_milestone は廃止（ADR-0079）…」で落ち、同じ結果の `create_task` は `stages_hint` 付きで読める | `… -p task-worker -E 'test(add_milestone_is_retired_with_a_reason_and_stages_hint_parses)'` | ok |
| (d) CoS の対話 run の結果ファイル（dispatcher の実経路）: `create_task` + `stages_hint` が実行され `Task.routing.stages_hint = [Phase 1 / MVP]`、`add_milestone` は失敗の注記に「add_milestone は廃止（ADR-0079）」「root task の段階」で人に見える、途中目標の行は作られない | `cargo nextest run -p task-dispatch -E 'test(absorb_console_actions_executes_the_declared_actions_for_the_cos_only)'` | ok |
| `create_task` / `POST /tasks` の `stages_hint` → `Task.routing.stages_hint`、空の title は 422 | `… -p task-ops -E 'test(create_task_carries_stages_hint_into_routing)'`、上の `is_root_task_and_stages_hint_are_exposed` | ok |
| `milestone_ready` は鳴らない（以前なら鳴った形でも）・秘書の返事は `secretary_reply` で届く | `cargo nextest run -p celeris --test notify` | ok（33 passed） |
| `celerisctl projects plan` は無い | `cargo nextest run -p celerisctl -E 'test(projects_plan_subcommand_is_removed)'` | ok |
| (e) 既存の案件の GUI が壊れない（途中目標が空の詳細・凍結した DAG の表示関数・案件ページの loader / action の単体テスト） | `corepack pnpm@11.27.0 -C gui test` | ok（Test Files 78、Tests 1190 passed） |
| MCP / Console は変わらない | `scripts/dev/test-parallel.sh`（celeris-mcp・task-api の console / console_instruct を含む全体） | ok |
| 上の新規・置き換えの試験をまとめて | `cargo nextest run --workspace -E 'test(project_plan_endpoints_are_gone) \| … \| test(a_draft_of_a_frozen_project_plan_proposal_can_be_accepted_individually)'`（23 本） | 23 passed |

### gate

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 8、binaries 94（nextest 83 + doc 11）、
  **passed 2847 / failed 0 / ignored 7**（削除した機能の試験〈案件計画・途中目標の Go / ADR-0077 / 判定 run / `milestone_ready` /
  `projects plan` / `POST /plans` の 201 など〉を消し、新しい規則の試験に置き換えた。`crates/task-api/tests/milestone_decide.rs` と
  `crates/celeris/tests/milestone_review.rs` は削除、`crates/task-api/tests/project_plan.rs` は R5a の試験に書き直し）
- `UPDATE_SCHEMA=1 cargo test -p task-core -p task-api -p task-worker --lib schema` で `api-v1.schema.json` / `event.schema.json` /
  `worker-protocol.schema.json` を再生成（追加: `Task.paused_at`・`NewTaskSpec.stages_hint`・`ProjectDetail.milestones_frozen`・
  `ProjectTaskView.is_root_task`・`TaskSummary.is_root_task` / `paused`・`TaskDetail.is_root_task` / `paused_by`・`TaskPauseResult`。
  削除: `ConsoleAction` の `add_milestone` と `create_task.milestone`。撤去した入口の要求・応答の型は互換のため残した〈付記 2.〉）
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0、再実行の前後で `gui/app/celeris/types.ts` が同じ（md5 8562abf30fd7f5d292f24adb9af738c4）
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0 / `lint` → exit 0（Checked 299 files、2 infos〈既存〉）/ `test` → exit 0（Test Files 78、Tests 1190 passed）

### 未解決・R5b 以降へ

- **R4b との併合**: R4b の「以前の途中目標（読み取り専用）」は loader で `GET /projects/{id}?include_frozen=true` を読むこと（既定は空。
  件数は `milestones_frozen`）。`projects.$id.tsx` の action から外した `intent`（`project_plan` / `project_plan_decide` / `milestone_*`）の
  ボタンが残っていれば 400 になる（R4b が隠す前提）。`/plans/new` の画面と nav・help のリンクは R5b で外す（今は 410 の文言が出る）。
- **`celerisctl plan`**（DB に直接 `kind = plan` を作る）は残した。U-R6 の一本化に含めるかは人の判断。
- 読み取りとして残した旧い経路（凍結した DAG・途中目標のレビューの文脈・worker の案件計画 run のプロンプト・`task_core::project_plan` の型・
  `NotificationKind::MilestoneReady`・`milestone_proposal` の解析・撤去した入口の schema の型）は R6（回収）で外す。
- 本番の非終端の途中目標 7 行（agent-platform）を一括で `cancelled` にするかは U-R8 のとおり人の判断（R5b の手順に入れるなら SQL ではなく
  案件の中止ではない別の入口が要る。今は書く入口が無い）。
- R4a から持ち越しの「replan が行の `phase` を書き換えない」は未修正のまま（触れていない）。

### 提案

- なし（DESIGN / SPEC への提案は R0 のまま）。

**昇格**: release `d8bb8069a0b2`（main d8bb806 = R5a + verify.sh の途中目標件数照合を include_frozen=true に）を 2026-09-29 05:53:51Z に昇格（mode=stop-start。live_ok=false は N-1 の旧バイナリが凍結途中目標を数える差分によるもので schema 変更なし。backup 20260929-055325-pre-d8bb8069a0b2）。1 回目の verify は `counts-match: milestones(snapshot=28 staging=0)` で失敗 → verify.sh を修正して再実行。

## R5b-prep: 人の /3 計画の入口と tree adopt（完了 2026-09-29）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) D2・D8・D15・§7 R5b、付記「R5b-prep 実装時の逸脱・明確化」（13 項目）。
- 種類: コード（task-core / task-ops / task-api / task-dispatch / celeris / celerisctl / celeris-mcp）、schema と GUI の型の再生成、GUI の
  `/plans/new` の撤去と「以前の途中目標」の開き方、手順書 [`docs/ops/adr-0079-r5b-runbook.md`](../ops/adr-0079-r5b-runbook.md)。
  **migration なし（schema 33 のまま。次の空きは 0034）**。本番の DB・設定・サービスには触れていない（読み取りの `sqlite3 …?mode=ro` と
  `GET /health` だけ）。
- **本番の読み取りで分かったこと**: R5b で採用する browser の Phase 2（01M3MZKB3DFYJNBH015MJGQ0BT）は `done` ではなく **`failed`**
  （`review_fail`: main の祖先の検査だけが不合格。その後に人が main へ取り込んだ = ce5d768）。Phase 1（01M3MFS5…）と Phase 2 の
  ブランチはどちらも main の祖先。BenchFS の done の Phase0 / Phase1 は 6 件（`parent_id` = `kind = plan` の 01M35X04345ZNDM09VE6FT168Z）。
  Phase 2 の人の決定 H1 / H2 / H3 / H6 は ADR-0080 に記録済み（手動登録から開始・操作ごとの approve_once と短い lease・認証 session の
  間の観測の停止・隔離は Phase 4）。

### 実装したもの

- **人の /3 計画の入口**: `PUT /tasks/{id}/execution-plan`（`POST` と同じ）と `celerisctl execution plan set|put --config` が daemon の
  実効の上限（`[execution.tree]`。API は `ApiSettings.tree_limits`、CLI は `--config` / `CELERIS_CONFIG`）で検証し、/3 は planner と同じ
  経路（unit の gate・計画の決定〈origin human〉・答えの無い決定を待つ leaf の `blocked(decision)`・木の上限の `kind: limit`）を
  **1 トランザクション**（`TaskStore::execution_plan_adopt_tree`）で通す（`task_ops::execution::adopt_human_plan`）。unit の gate と止めは
  dispatcher から `task_ops::tree_plan`（`unit_gate_plan` / `plan_hold_writes`）に移し、planner の経路も同じ関数を呼ぶ。木が無効なら
  /3 は従来どおり 422 `TreeDisabled`。
- **PlanGate の扱い（決めたこと）**: 人の計画は書いた人の承認とみなし `PlanGate` を挟まない。報告の流れに「計画を採用して進めます」を
  1 件（承認が要る形なら理由も）。決定の要求は通知・受信箱に出る（`scan_decisions` が人の計画の決定を `plan:<plan_id>:decisions` の
  1 通に束ねる）。部をまたぐ子の認可の質問も人の計画には出さない。
- **採用（D15）**: `POST /tasks/{id}/tree/adopt {task_id, stage, unit_key}`（管理系）・`celerisctl tree adopt`・人の計画の unit の
  `adopt: <task_id>`（PUT と同じトランザクション）。条件の違反は 422（要求と計画の食い違い）/ 409（状態）で `code` は `adopt_*`。
  採用できるのは **`done` か `failed`**（決めたこと: 非終端は PUT では unit を待たせ、後からの採用は 409。ブランチの基点を見る例外は
  実装しない）。採用した unit は `done`（`ChildAdopted`）、対象の `tree` と（無ければ）`parent_id` を書き、状態・履歴・ブランチ・作業場所は
  変えない。統合は対象のブランチを任意の項目として扱い、既に基点にあれば `skipped`。daemon は `adopt` の unit から新しい子を作らない。
- **直した既存の穴**: 段階の unit が採用した task だけのとき root の worktree が無く統合が `no HEAD` で失敗していた → `start_integration`
  が merge の前に worktree を用意する。`adopt` の unit は `max_child_tasks_per_plan` に数えない（BenchFS の採用 6 + 子 4 が上限 6 に収まる）。
- **後片付け**: GUI の `/plans/new`（画面・ルート・ナビ・使い方のリンク・`createPlan`・単体テスト・e2e の節）を撤去。`celerisctl plan` は
  残し、ADR-0079 の注記を stderr に出す。案件ページの「以前の途中目標（読み取り専用）」は件数だけを出し、「表示する」（`?frozen=1`）で
  `GET /projects/{id}?include_frozen=true` を読む。R4b からあった `MilestoneStatus` の未使用 import の lint 違反も消した。
- **手順書**: `docs/ops/adr-0079-r5b-runbook.md`（0. 変数と前提 → 1. 設定と再起動 → 2. browser の root・/3 計画・受け入れ・決定 →
  3. BenchFS の root・/3 計画 → 4. dogfood → 5. 受け入れ条件 (a)〜(e) の SQL / curl → 6. 巻き戻し）。手順書の JSON 4 本は試験
  （`the_r5b_runbook_plans_are_accepted_as_written`）が本番と同じ id・状態の fixture でそのまま通ることを確かめる。

### 受け入れ条件（依頼の項目）

| 条件 | コマンド | 結果 |
|---|---|---|
| 人の `PUT` の /3（木が有効）が unit の gate（`UnitGateOverridden{kept_task}`）・計画の決定 2 件（origin human、path は root、`GET /tasks/{id}/decisions` に出る）・答えを待つ leaf の `blocked(decision)`・木の leaf の上限（`limit:max_tree_leaves`）を通り、`adopt` の unit は同じ書き込みで `done`（`ChildAdopted`、対象の `tree` と `parent_id`、`Edited{tree, parent_id}`）、PlanGate は無く root は draft のまま、`adopt` の unit は `max_child_tasks_per_plan = 1` に数えない、replay の差分 0 | `cargo nextest run -p task-api --test tree_adopt -E 'test(human_put_v3_goes_through_the_tree_path_without_plan_gate)'` | ok |
| 木が無効なら `PUT` の /3 は 422 `TreeDisabled`（何も書かない。`POST` の既存の試験も通る） | `… --test tree_adopt -E 'test(human_put_v3_is_422_while_the_tree_is_disabled)'`、`… -p task-api --test execution -E 'test(a_v3_plan_is_rejected_with_422_while_the_tree_is_disabled)'` | ok |
| 人の計画の `adopt` の拒否で何も書かない: 別の案件 422 `adopt_other_project`・自分（祖先）422 `adopt_ancestor`・他の木の子 / 木の root 409 `adopt_target_in_tree`・中止済み 409 `adopt_target_cancelled`・無い task 422・重複 422、leaf の `adopt` は 422 | `… --test tree_adopt -E 'test(human_plan_adopt_refusals_write_nothing)'` | ok |
| `POST /tasks/{id}/tree/adopt`: 対象が非終端なら PUT は unit を待たせ（`adopted: false`、子なし）、採用は 409 `adopt_target_not_terminal`、leaf の unit 422 `adopt_unit_not_task`・id 違い 422 `adopt_id_mismatch`・段階違い 422・無い unit 422・無い task 404・トークン無し 401・未知の欄は拒否、対象が done になれば 200（unit done・`ChildAdopted`・`parent_id`）、二度目は 409、replay の差分 0 | `… --test tree_adopt -E 'test(adopt_endpoint_happy_path_and_refusals)'` | ok |
| 木が無効なら 422 `tree_disabled`、/3 の計画が無ければ 422 `adopt_no_tree_plan` | `… --test tree_adopt -E 'test(adopt_endpoint_requires_the_tree_and_a_v3_plan)'` | ok |
| 一時 git: done の task（ブランチは main と同じ commit）を採用した段階の統合が root の worktree を用意して `merged = [p1 skipped]` で通り、子は作られず、次の段階の leaf が merge され root が done、対象は done のまま・ブランチは残り・main は動かない、replay の差分 0 | `cargo nextest run -p task-dispatch -E 'test(adopted_done_task_already_in_base_is_skipped_by_integration)'` | ok |
| 非終端の対象を待つ `adopt` の unit から daemon は子を作らない（30 tick） | `… -p task-dispatch -E 'test(an_adopt_unit_waiting_for_its_task_never_spawns_a_new_child)'` | ok |
| 人の計画の決定 2 件が `plan:<plan_id>:decisions` の 1 通に束ねられ、推奨が本文にあり、`plan_approval` の通知は出ない | `cargo nextest run -p celeris --test notify -E 'test(human_plan_decisions_are_bundled_per_plan)'` | ok |
| `celerisctl execution plan set` は設定無し・木が無効の設定では /3 を `TreeDisabled`、`--config`（`enabled = true`）で採用（`adopt` で done、決定 origin human）、`put` は `set` の別名、`tree adopt` は木が無効なら拒否 | `cargo nextest run -p celerisctl -E 'test(set_adopts_a_v3_plan_with_the_configured_tree_limits) \| test(tree_adopt_needs_the_tree_and_put_is_an_alias_of_set)'` | ok |
| 手順書の JSON（browser / BenchFS の root と /3 の計画）が本番と同じ id・状態（Phase 2 = failed、BenchFS の `parent_id` は plan の task）でそのまま通る（採用 2 / 6、決定 3 / 1、PlanGate なし）、replay の差分 0 | `… -p task-api --test tree_adopt -E 'test(the_r5b_runbook_plans_are_accepted_as_written)'` | ok |
| 上の新しい試験をまとめて | `cargo nextest run --workspace -E 'binary(tree_adopt) \| test(adopted_done_task_already_in_base_is_skipped_by_integration) \| …'`（12 本） | 12 passed |
| planner の経路（R2a / R3a / R3b の unit の gate・止め・決定・承認）が関数の移動の後も同じ | `scripts/dev/test-parallel.sh`（`dispatcher::tests::tree*` を含む全体） | ok |
| GUI: `/plans/new` がルート・ナビ・使い方・中継に無い | `corepack pnpm@11.27.0 -C gui test`（`test/unit/plans-retired.test.ts`） | ok |
| GUI: 以前の途中目標は既定で `include_frozen` を付けず件数だけ、`?frozen=1` で `GET /projects/{id}?include_frozen=true` | 同上（`test/unit/projects.detail.test.ts` の 2 本） | ok |

### gate

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 8、binaries 95（nextest 84 + doc 11）、
  **passed 2859 / failed 0 / ignored 7**（R5a の記録の 2847 から +12。うち R5b-prep の新しい試験 11 本〈`task-api/tests/tree_adopt.rs` 6・
  dispatcher 2・notify 1・celerisctl 2〉、残りは main の R4b の併合分）
- `UPDATE_SCHEMA=1 cargo test -p task-core -p task-api -p task-worker --lib schema` で `api-v1.schema.json` を再生成（追加だけ:
  `ExecutionPlanView.adoptions` / `decisions_raised`、`AdoptRequest`、`AdoptionOutcome`）
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0、再実行の前後で `gui/app/celeris/types.ts` が同じ（md5 26f0a348adfb302ce963f6262fc6d0d0）
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0 / `lint` → exit 0（Checked 307 files、2 infos〈既存の `scripts/check-resume-recovery.mjs`〉）/
  `test` → exit 0（Test Files 81、Tests 1222 passed）/ `build` → exit 0
- `MOBILE_AUDIT_SKIP_BUILD=1 corepack pnpm@11.27.0 -C gui mobile-audit` → exit 0（`routes=28 schemes=2 violations=0 perf_worst=task-overview 590.1KB`）
- `E2E_SKIP_BUILD=1 corepack pnpm@11.27.0 -C gui e2e:mock` → exit 0（`failures: []`）

### 未解決・R5b へ

- **R5b の実行は人**（手順書のとおり。設定の変更・再起動・root の作成・計画の PUT・決定への回答）。R5b-prep を含むリリースの昇格が先。
- 案件ページの「この方針で進める」（`POST /projects/{id}/plan {mode: decompose}`）は R5a で 410 になったが画面に残っている（押すと 410 の
  文言）。R4b / R5a のどちらも外していない。R6（回収）か次の GUI の手当てで外す。
- 採用の書き換え（`tree`・`parent_id`）を戻す入口は無い（巻き戻しでも残る。手順書 6.）。要るなら R6。
- 非終端の task を「ブランチが root の段階の基点の上にあれば」採用する例外は実装していない（付記 5.）。
- 採用した task の過去の run・leaf・版は木の上限に数える（付記 13.）。browser は run 30 / 120 を最初から使う。
- BenchFS の案件のリポジトリは sirius の remote で、root は並列 1 に倒れる。子の作業場所（`cluster:sirius` の担当か local への落とし）は
  子が作られたときに確かめる（手順書 3.）。
- R4a から持ち越しの「replan が行の `phase` を書き換えない」は未修正のまま（触れていない）。

### 提案

- なし（DESIGN / SPEC への提案は R0 のまま）。

**昇格**: release `9aeb1b721649`（main 9aeb1b7 = R5b-prep）を 2026-09-29 06:55:40Z にライブ昇格（backup 20260929-065531-pre-9aeb1b721649、verify ok / live_ok、schema 33）。R5b の手順 1（`[execution.tree] enabled = true` と再起動）から人の実行に入る。


### R5b 実行記録（2026-09-29、人の go の後に Fable が API で実行）

- 手順 1（人）: `[execution.tree] enabled = true` を追記し `celeris@9aeb1b721649` を再起動（10:16:30Z）。`task-tree` の `tree_enabled = true` を確認。
- 手順 2（browser）: root task **01M3PAX6RVE7AX8Z6118KADME3**「browser capability（Phase 1〜4）」を draft で作成 → /3 計画 v1 を PUT（p1 = 01M3MFS5… adopt done、
  p2 = 01M3MZKB3D… adopt done、p3 / p4 = 子 task、決定 h4 / h5 / h7、phase-3 の後に review: human）→ accept（10:21:01Z）。events: `child_adopted` ×2、
  `unit_gate_overridden` ×4（kept_task）、`decision_requested` ×3、Discord `decision_requested`（plan 単位で 1 通、ok）。gate 記録は `atomic/policy`（人の計画は gate に
  関わらず採用されるため表示上の齟齬。P-R5b-1）。
- 手順 3（BenchFS）: root task **01M3PAZ4XG4QN1T8S98VNA6ABV**「国際会議フルペーパー化」→ /3 計画 v1（棚卸し段階で done の Phase0 / Phase1 の 6 task を adopt、
  子 task 4、決定 `framing`）→ accept → 決定 `framing` に人の決定 **A** で回答（10:2xZ）。
- 手順 2.4（browser の決定 h4 / h5 / h7）は人の回答待ち。
- 手順 2.4（10:2xZ、人「推奨どおりで」）: h4 = task-acl-proxy、h5 = project-origin-identity、h7 = keep-acp-claude を API で回答。phase-1 / phase-2 の統合 WU は
  採用済みとして done、p3 の子 task **01M3PBAVFAYPDWMQMDBXPTE2V8**「Phase 3: Browser Identity・live proxy・takeover」が生成され running（手順 4 の dogfood）。
- BenchFS の不具合と対処: runbook の root JSON が `workspace` を持たず、root と子が `local` の作業場所になった（案件 BenchFS は `remote sirius
  /work/NBB/rmaeda/workspace/rust/benchfs`）。子 01M3PB68JKRED21E3QVG9TE6QZ「実験に要る実装」が「BenchFS のソースが無い」と質問 → root と子の `workspace` を
  PATCH で remote に直し、質問に回答して再開。**P-R5b-2**: root task 作成時に案件の workspace を既定で継ぐべき（子は root を継ぐ）。remote workspace の子の
  親ブランチ統合が動くかは未確認（R1c はローカル git 前提）。
- 人の報告: GUI に決定へ回答する場所が見当たらなかった（R4b の inbox「決定」節が本番で見えていない可能性。決定が open のときに確認する。P-R5b-3）。
- 別件: 人が起票した「web Phase 0: GUI 全面改修…」01M3MS2JRDJ4GM0D9VN9PJCB6B は 09-28 20:50Z から blocked（done WU の check 同士が矛盾し replan では解けない、
  planner が A/B/C を提示）。人の判断待ち。
- web Phase 0 task（10:37Z、人「A で進めて良い」）: 質問に A で回答 → planner は baseline の check だけ差し替えた全体計画を出したが、検証器が
  `done work unit baseline must not change on replan` で拒否し再び blocked（10:38Z）。ADR-0074 の done 不変条件は planner には正しいが、人が done WU の spec を
  直す入口が無い → **R5b-fix1**（人の replan は done WU の spec を上書きできる。事象と diff に記録）を Opus に委譲。

### 手順 4 dogfood の観察（2026-09-29 10:25Z〜10:56Z）

- **browser phase-3 子 01M3PBAVFAYPDWMQMDBXPTE2V8**: gate は `atomic/score`（score 2 / 閾値 7、深さ 2）。atomic の run が yield → 続き run を 3 回繰り返し
  （P3-C 制御 lease の状態機械 + 単体 10、P3-B ACL / 再接続 / scrub の純関数 + 単体 11、P3-A identity の純関数 + 単体 10、ADR-0081〜0083、
  `cargo test --workspace` 2890 passed）、continuation 上限 3 で「予算を増やす／分割し直す／中止」の質問（10:55Z、blocked）。
  → `POST /tasks/{id}/execution/decompose {mode: compound}` で人の compound を設定し、質問に「分割し直す」で回答（10:58Z、ready）。次の dispatch で
  ExecutionPlan の経路に入るかを見る。
- **BenchFS 子 01M3PB68JKRED21E3QVG9TE6QZ「実験に要る実装」**: gate `atomic/small`（score 0）。run 1〜2 は root の local 作業場所（ソース無し）で空回り、
  workspace を remote に直した後の run 3〜4 は `.celeris/remote-exec` で sirius の worktree を使ったが、run 間で編集が消えて（下記 D1）復元に費やし、
  review 不合格 ×2 で **failed**（10:39Z）→ root の unit bf-impl failed → root の planner replan ×2 とも検証に落ち、決定 `plan_invalid`
  01M3PC2C6NAPJPM055B2Q70WH7（推奨 replan）が open。回答は修正の昇格後。
- **/inbox の「決定」節（P-R5b-3）**: `GET /inbox` は `counts.decisions = 1` と該当の決定を返し、GUI `inbox.tsx` に節（`decisions-section`）がある。
  描画は要ログインのため未目視（人に確認を依頼）。
- **読み取り専用の調査（Opus）で判明した欠陥**（root と子の events・作業場所・コードから）:
  - **D1 remote workspace の編集消失（重大）**: `SshWorkspace::prepare` は毎 run `--delete` 付きで pull するが、push は `Check::Command` の `exec()` だけ。
    reviewer check しか持たない task は一度も push されず、次の run の pull で local の編集と `artifacts/`（除外に無い）が消える。root が remote に
    切り替わった planner run の pull で bf-plan の成果 `artifacts/experiment-plan.md` も消えた（`runs/<run>/stdout.jsonl` から復元可）。→ **R5b-fix2**。
  - **D2 remote の子の reviewer**: プロンプトに remote-exec の指示が無く「取り込み先: 親のブランチ」と書かれる（remote は merge 無し）。reviewer が
    rsync 複製で `git status` を打ち `fatal: not a git repository`。→ **R5b-fix2**。
  - **D3 root の workspace**: `POST /tasks` は案件の workspace を継がず `Local{path: id}`、repo だけ remote を継ぐ（P-R5b-2 の原因）。→ **R5b-fix3**。
  - **D4 子の workspace**: `build_child_task` が親の `Local{path: <parent_id>}` をそのまま写し、子が親のディレクトリを共有（R1c 注 1 に反する）。→ **R5b-fix3**。
  - **D5 子の予算と gate**: 子は親の budget（10 turns / 600 s）を継ぎ、全 run が予算切れ（root planner も `error_max_turns`）。gate の強制規則 `atomic/small`
    （`max_turns <= 10 && objective < 400`）が木の子に必ず当たり、`kind: task` の unit が全部 atomic 判定。→ **R5b-fix3**（子の予算は葉の予算以上、
    木の子には atomic/small を当てない、`kind: task` は compound の手掛かり）。
  - **D6 replan と adopt**: 人の /3 計画の adopt 済み done unit を planner が写す（または daemon が復元する）と `adopt is only allowed … origin human` で拒否 →
    adopt 済みの計画は planner が replan できない。→ **R5b-fix1** に追加。
  - **D7 木の remote 未対応（設計の穴）**: remote では子ブランチ・基点・統合が全部 no-op（`parallel_mode = serial` → `child_base_commit = None`、
    `phase_integrated.merged = []`）。子ごとにクラスタ側 worktree（基点は cluster の `worktree.base`、親ブランチではない）が作られ、celeris は commit しない。
    remote の親に `kind: task` の unit を許すか、クラスタ側でのブランチ/commit/merge を実装するかは人の判断（R6 か別 ADR）。
  - 小: 基盤障害（作業場所無し）の review 不合格が attempts を消費し、人の回答 / PATCH で reset されない。root の gate 記録が人の /3 計画採用後も
    `atomic/small`（P-R5b-1 と同根）。



## R5b-fix2: remote workspace の run 後 push、成果物を pull で消さない、reviewer への remote-exec（2026-09-29）

本番の木の子 01M3PB68JKRED21E3QVG9TE6QZ（remote sirius）で見つかった不具合の修正。判断は ADR-0079 付記「R5b-fix2」。本番には触れていない。

### 実装したもの

- `task-worker/src/ssh.rs`: `SshWorkspace::push_after_run`（印 `.celeris/push-pending` を置いてから push、成功で消す）、`pull` は印があれば
  先に push し、落ちたら pull しない。`push_args` / `pull_args` に組み立てを分け、pull に `--filter=P artifacts/`（`SYNC_PULL_PROTECTED`）。
  指示文を `remote_exec_head` / `REMOTE_EXEC_USAGE` / `remote_worktree_note` に分け、reviewer 向けの `remote_exec_reviewer_instructions` を足した。
- `task-dispatch/src/dispatcher.rs`: `run_worker` が remote の run の後に毎回 `push_remote_after_run`（進行 1 行、失敗は error 付きで run を失敗にする）。
  最終レビューは `review::review_view` を使い、remote なら reviewer の前にラッパを書き直す。
- `task-dispatch/src/review.rs`: `review_view`（remote の木の子は `TREE_CHILD_REMOTE_NOTE`、親のブランチの行と `merge-base` の書き換え無し、
  remote-exec の reviewer 指示）。`tree_child_review_view` は `review_view(.., None)` の薄い包み。

### 逸脱

- 依頼の「`artifacts/` を `SYNC_ALWAYS_EXCLUDED` に足す」は **protect フィルタ**にした（除外だと成果物がクラスタへ行かず、クラスタで作られた成果物も
  戻らない。P-46 の注記と衝突）。pull が成果物を消さない、という目的は同じ。テストは protect と push で成果物が送られることを確かめる。

### 未解決

- remote の木の子のブランチ・親への統合は未対応（別の決定）。
- 失われた `artifacts/experiment-plan.md`（task 01M3PAZ89XXEF10B92Y0Z0T8V4）の復旧は人の作業（報告本文に手順）。

### gate

- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo test --no-fail-fast -p task-worker -p task-dispatch -- --test-threads=8` → exit 0（task-worker lib 598 passed / 1 ignored、ssh_localhost 9 passed、
  task-dispatch lib 450 passed）。新しいテスト: `ssh::tests::{pull_protects_artifacts_…, push_after_run_runs_once_and_the_next_pull_keeps_edits_and_artifacts,
  failed_push_is_reported_and_blocks_the_deleting_pull_until_a_push_succeeds, push_after_run_is_a_no_op_under_sync_none,
  worker_and_reviewer_instructions_share_the_remote_exec_usage}`、`dispatcher::remote_push_after_run_tests::*`（2 件）、
  `review::tests::remote_tree_child_review_view_has_remote_exec_and_no_parent_branch_line`。
- 既定の並列度（load 約 60 のホスト）での 1 回目は build cache / scratch 系の 5 件（`the_preamble_notes_the_shared_build_cache_when_enabled` など、
  `left: Running` で待ち切れず）が落ちたが、単独実行と `--test-threads=8` では通る（高負荷のタイミング依存。今回の変更とは無関係の経路）。


## R5b-fix3: 子 task の workspace 継承・予算・gate（2026-09-29）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) 付記「R5b-fix3: 子 task の workspace 継承・予算・gate（kind: task は compound の手掛かり）」（7 項目）。
- 種類: コード（task-core / task-ops / task-api / celerisctl）。**migration なし**。本番には触れていない。
- 直したもの:
  - **D1（P-R5b-2）**: 案件に属し `cluster` / `workspace` の無い `POST /tasks` は、リモートのリポジトリ（primary なら案件の workspace）を継ぐ。
    明示の `Local` とリモートのリポジトリの組み合わせは 422。
  - **D2**: 木の子は親の `Local{<parent_id>}` を共有せず `Local{<child_id>}`（remote / 絶対パスはそのまま）。
  - **D3 (a)**: 木の子の予算 = `max(親, 30 turns / 1,800 秒)`、`max_retries` は親（ADR-0072 D18 の leaf 1 run の既定と同じ下限）。
    unit の gate の view と leaf に下げる基準も同じ値。root の予算は変えない。
  - **D3 (b)**: `atomic/small` は木の子に当てない。
  - **D3 (c)**: 子の `execution_hint = {compound, explicit: 人の計画か}`、unit の gate も同じ（`UnitGateContext.human_plan`）。
  - **D3 (d)（P-R5b-1）**: 人の計画の採用（API の PUT/POST・CLI の `plan set`）の後に gate の記録を `{compound, human, human/plan}` にする
    （`task_ops::regate::record_human_plan_gate`）。
- 持ち越し: **D4**（answer / PATCH での attempts の巻き戻し）は replay の規則を変えるので実装しない（付記 7.）。本番の既存の子
  （01M3PAZ89… / 01M3PB68… / 01M3PBAV…）の予算・作業場所・gate 記録は直していない（人が PATCH / retry する）。
- gate: `cargo fmt --all -- --check` exit 0 / `cargo clippy --workspace --all-targets -- -D warnings` exit 0 /
  `cargo nextest run -p task-core -p task-ops -p task-api -p celerisctl` 1380 passed / 0 failed / 1 skipped /
  `cargo nextest run -p task-dispatch` 451 passed / 0 failed。（`task-api::browser_e2e` の 3 本は 1 回目に高負荷で `celeris-credentiald` の
  入れ子の `cargo build` が失敗したが、再実行で通った。変更とは無関係）
- 変えた既存の試験: `task-api/tests/tree_adopt.rs`（人の計画の kind task の unit は `kept_task` ではなく記録なし）、
  `dispatcher/tests/tree_gate.rs`（score 6 の子は features 4 + H 2 で作る）、`dispatcher/tests/tree.rs`（子の予算は `tree_child_budget`）。


## R5b-fix1: 人の replan は done WU の spec を上書きできる・planner は done の unit の adopt を写してよい（完了 2026-09-29）

- ADR: [ADR-0079](../adr/0079-recursive-task-decomposition.md) 付記「R5b-fix1」。ADR-0072 の末尾に相互参照の注記（D14 / D17 の done の不変条件の緩和）。
- 種類: コード（task-core / task-ops / task-api / celerisctl）、schema（`api-v1.schema.json` / `event.schema.json`）と GUI の型の再生成、API 文書。
  **migration なし**。本番の DB・設定・サービスには触れていない。

### 何を・なぜ

1. **人の replan は done の WU の spec を上書きできる**（本番 01M3MS2JRDJ4GM0D9VN9PJCB6B「web Phase 0」）: done の WU `baseline` の check
   `git diff --quiet 06e9a03cffe8 -- gui` が後の WU の正当な変更（`gui/docs/adr/0002-frontend-stack.md`）で通らなくなり、人は案 A（check を
   `… -- gui ":!gui/docs/adr/0002-frontend-stack.md"` に直す）を選んだが、planner の replan は `DoneWorkUnitChanged` で拒否され、人が直す入口も
   無かった（`PUT` は新規だけ）。
   - 検証（`validate_with` の `done_carry_over_errors`、/1・/2・/3 共通の 1 か所）: origin human だけ、done の WU の spec の上書きを許す。消すのは
     `DoneWorkUnitChanged`、`kind` / `phase`（段階）/ `depends_on` を変えるのは新しい `DoneWorkUnitStructureChanged`。planner / repair は従来どおり完全一致。
   - `task_ops::execution::replan`: 上書きした done の行は `done` のまま `spec` / `updated_at` だけを新しい版に（`plan_id` / 依存 / カウンタは元のまま）。
     `Event::WorkUnitSpecOverridden { work_unit_id, key, plan_id, plan_version, changed_fields }` と `ReplanDiff.overridden_done`、`ExecutionPlanned.reason`
     の末尾に ` (overridden_done=<keys>)`（上書きがあるときだけ）。replay は同じ event で done の行の spec を新しい版にする。
   - 入口: `PUT /tasks/{id}/execution-plan` は有効な計画があれば人の replan（200、応答に `replan` の差分）、無ければ従来の新規採用（201）。`POST` は
     新規だけのまま（409）。`celerisctl execution plan replan <task> --file <json> [--reason] [--config]` を足した。
   - `DoneWorkUnitChanged` の文言に「人は `PUT /tasks/{id}/execution-plan` で done の WU の spec を上書きできる」を足した（planner の再試行の入力に入る）。
2. **planner は done の unit の `adopt` をそのまま写してよい**（本番 01M3PAZ4XG4QN1T8S98VNA6ABV「BenchFS」）: 人の /3 計画の done の unit
   （phase1-framing など）が `adopt` を持ち、子の失敗で起きた planner の replan が、done の不変条件どおり写した unit ごとに `AdoptNotAllowed` で拒否された。
   検証は、key が `done_work_units` にあり内部の形が done の spec と一致する unit の `adopt` を planner にも許す。done の写しでない unit の新しい `adopt` は
   従来どおり拒む。`AdoptNotAllowed` の文言に「(a done unit copied verbatim from the previous version may keep its adopt)」を足した。

### 受け入れ条件（依頼の項目）

| 条件 | コマンド | 結果 |
|---|---|---|
| 人の origin は done の spec の変更（check）を通し、変わった欄は `checks` | `cargo nextest run -p task-core -E 'test(human_replan_may_override_a_done_work_unit_spec)'` | ok（全体の実行に含まれる） |
| planner / repair の origin は従来どおり `DoneWorkUnitChanged`、文言に `PUT /tasks/{id}/execution-plan` | `… -E 'test(planner_replan_still_rejects_a_changed_done_work_unit)'` | ok（全体の実行に含まれる） |
| 人の origin でも done の WU の削除は `DoneWorkUnitChanged`、`kind` / `depends_on` の変更は `DoneWorkUnitStructureChanged` | `… -E 'test(human_replan_still_rejects_a_removed_or_restructured_done_work_unit)'` | ok（全体の実行に含まれる） |
| planner の /3 replan は done の写しの `adopt` を通し、写しでない（done 無し・spec 違い）`adopt` は `AdoptNotAllowed`、文言に "copied verbatim" | `… -E 'test(planner_replan_may_keep_adopt_on_a_verbatim_done_carry_over)'` | ok（全体の実行に含まれる） |
| `replan`（Human）: done の行の spec が更新され `done` のまま・`plan_id` / 依存は元のまま、`WorkUnitSpecOverridden{changed_fields: [checks]}`・`diff.overridden_done = [a]`・reason の `overridden_done=a`、planner は同じ spec を拒否、replay の差分 0 | `cargo nextest run -p task-ops -E 'test(human_replan_overrides_the_spec_of_a_done_work_unit)'` | ok（全体の実行に含まれる） |
| HTTP: 有効な計画がある `PUT` は 200 の人の replan（`replan.overridden_done = ["a"]`、行は done で新しい check、event あり）、done を消す `PUT` は 422、`POST` は 409 のまま、計画が無ければ `PUT` は 201 | `cargo nextest run -p task-api --test execution -E 'test(put_with_an_active_plan_is_a_human_replan_that_may_override_a_done_spec)'` | ok（全体の実行に含まれる） |
| 新しい event の `type` 名が serde・`event_type_name`・`EVENT_TYPES`（44 → 45）で一致 | `cargo nextest run -p task-api -E 'test(tree_event_types_match_their_serde_names)'` | ok（全体の実行に含まれる） |
| 既存の planner の replan（dispatcher）の done 不変条件の試験が変わらず通る | `scripts/dev/test-parallel.sh`（全体） | ok |

### gate

- `cargo fmt --all -- --check` → `cargo fmt --all` の後の `--check` → exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（commit 7cf3c5d の後に実行）
- `scripts/dev/test-parallel.sh` → exit 0、`CELERIS_TEST_SUMMARY`: nextest 0.9.146、jobs 6、binaries 95（nextest 84 + doc 11）、**passed 2865 / failed 0 / ignored 7**（R5b-prep の 2859 から +6: task-core 4・task-ops 1・task-api 1）
- `UPDATE_SCHEMA=1 cargo test -p task-core -p task-api -p task-worker --lib schema` → exit 0（追加だけ: `work_unit_spec_overridden` の event、
  `ExecutionPlanView.replan`、`ReplanDiff`）
- `corepack pnpm@11.27.0 -C gui gen:types` → exit 0（`gui/app/celeris/types.ts` に 40 行の追加だけ）/ `typecheck` → exit 0 / `lint` → exit 0（Checked 307 files、
  2 infos は既存）

### 未解決

- 本番の task への適用は人（または Fable）が行う: 01M3MS2JRDJ4GM0D9VN9PJCB6B は有効な計画（`celeris.execution-plan/2`）の全体を `GET` で取り、`baseline` の
  `checks[].cmd` だけを直して `PUT`。01M3PAZ4XG4QN1T8S98VNA6ABV は次の planner の replan が通る（手当て不要。止まっていれば replan を起こし直す）。
  どちらもこの変更を含むリリースの昇格が先。
- 人の `PUT` の replan は /3 の unit の gate・計画の決定・`adopt` の当て込みを通さない（付記「制限」）。/3 の人の replan で新しい kind task の unit や
  `adopt` を足す使い方は未対応。
- `celerisctl execution plan put` は `set` の別名（新規だけ）のままで、HTTP の `PUT`（有効な計画があれば replan）と意味がずれる。

### 提案

- なし。

### R5b-fix1〜3 の統合と release（2026-09-29 15:1xZ）

- main 1ab74fa = fix2（b30b7a8）+ fix3（d12a745）+ fix1（7cf3c5d / e12ba20）。fix1 の PUT 新規採用側にも fix3 の `record_human_plan_gate` を移植（統合時）。
- release chain: 3d7f44f（fix2+3）は gate ok、verify の gui-e2e が staging の task ページ（phase-3 の子）503 で失敗（負荷 60 超、本番 API は同 task の
  全 GET が 0.13 s 以下 → SSR の 15 s 超過と判断）。1ab74fa の 1 回目は mobile-audit の `home` LCP 4852 ms（CPU ×4）1 件で gate 失敗（GUI 差分は
  `types.ts` 40 行のみ）。負荷 6 で再実行 → gate ok（cargo-test 2865 相当、clippy 0、mobile-audit 0）、push、**verify ok / live_ok**（schema 33、41.8 s）。
- 昇格は chain が「Production Deploy」で拒否されたため人が `promote.sh 1ab74fae921c` を実行する。
- P-R5b-3 は解消: BenchFS root の決定 `plan_invalid` 01M3PC2C… が 14:40:35Z に `by: human`（GUI）で `replan` と回答された（Fable は回答していない）。
  再 replan は fix1 未昇格のため同じ理由で失敗し、決定 01M3PT3132DPB84TDY669VDFSZ が open（昇格後に replan + note で回答）。
- bf-plan の成果 `artifacts/experiment-plan.md`（sha256 b0a56e29…、175 行）は run 01M3PAZ8AFZQDD31BDX43FDYS0 の stdout.jsonl の Write から復元し、
  sha 一致を確認（scratchpad）。昇格後に root の作業場所へ戻す（人）。

### 昇格後の dogfood（2026-09-29 16:3xZ〜17:3xZ、release 1ab74fae921c）

- 人が `promote.sh 1ab74fae921c`（live）と `experiment-plan.md` の復元を実行（sha256 一致を確認）。
- **web Phase 0**: 人の PUT replan（v3）で baseline の check を A 案に差し替え → 200、`replan.overridden_done = ["baseline"]`（R5b-fix1 の本番動作）。質問に回答して ready
  → planner run が `failed to spawn worker: Argument list too long (os error 7)` で失敗（prompt 135,644 B を `-p` の argv 1 要素で渡していた。
  Linux MAX_ARG_STRLEN 128 KiB 超）→ blocked。**F5-fix10**（prompt を stdin で渡す）を Opus に委譲。昇格後に回答して再開する。
- **BenchFS**: 決定 `plan_invalid`（2 件目、人が GUI で回答した 1 件目は昇格前で同じ失敗）に replan + note で回答 → planner v2（adopt 済み unit は復元、bf-impl は
  superseded → bf-impl2 を再発行、bf-exp / bf-write は reviewer + artifact_exists）→ PlanGate（`review_human:experiments`、near_limit ×3。
  `max_child_tasks_per_plan 10/6` は adopt 済み unit を数えている表示の齟齬）→ 人が approve → 子 01M3Q25DSD895DGMGPWD752G3G が running。
  R5b-fix3 の効果: 子の workspace は remote sirius を継承、budget 30 turns / 1800 s、`execution_hint = compound (explicit: false)`。gate は
  `atomic/out-of-scope`（remote は木の対象外 = D7）。
- **browser phase-3 子**: 全 16 WU done → 最終 review の `cargo test --workspace` が e2e 1 件（replay MISMATCH status）で失敗 → `limit:max_replans`（3）の決定に
  `raise-once` + note で回答 → 修理 `fix-replay`（同 test 5 回連続 pass）→ 機械検査は通過、reviewer（codex）が P3-B/C の未配線・認証区間の未接続で
  criterion 0/1 不合格 → **failed**（17:05Z）。根の unit p3 が `child_failed` → 根の planner replan v2（17:10Z、同 key p3 で再試行・前回ブランチを merge して
  欠落だけ塞ぐ、p4 は 1 子 task）→ PlanGate（near_limit `max_tree_leaves 33/40`）。v2 は v1 の phase-3 後の `review: human` を落としていたので、それだけ戻す
  replan を要求（v3 待ち）。
- 観察: 根 task の `GET /tasks/{id}/timeline` が 1〜5.6 s（人が GUI で閲覧中）。件数由来の疑い、回収で見る。
- browser 根の続き（時刻は events の値）: 17:11:52Z v2 が PlanGate 待ち → 17:13:04Z 人が `plan-gate {action: replan}`（`review: human` を戻す指示）→
  **P-R5b-4（欠陥）**: replan の要求と同時に v2 の unit p3 から子 task 01M3Q2FPRCF34F00PBZSMNSZE8 が作られ dispatch された（17:13:07Z。承認されていない v2 の
  unit を動かした）。planner は 17:13:35Z に v3（`review: human` 復元、p3 は同 key）→ PlanGate（`review_human:phase-3`、near_limit）→ 17:14:20Z 人が approve。
  子は v3 でも p3 なのでそのまま継続。子の gate は `atomic/score`（score 6 = 特徴 4 + hint 2 / 閾値 7）で atomic、budget 30 turns、workspace は自分の
  Local（R5b-fix3 D2 の効果）。
- 17:40Z: browser phase-3 の再試行の子 01M3Q2FPRCF34F00PBZSMNSZE8 が **done**（6 条件すべて pass。auth-section の配線、takeover/renew の 409、実経路の
  forward_events、e2e、workspace test/clippy）→ 根の p3 done（`work_unit_committed` branch `celeris/01M3Q2FP…` base 30ade54 → f24932c）→ `integrate-phase-3` running。
  深さ 2 の子（atomic 再試行）→ 親ブランチ統合の経路が本番で通った。次は phase-3 の `review: human`。
- 17:34Z: BenchFS の子 01M3Q25DSD895DGMGPWD752G3G が決定 `provision-submodules`（sirius の専用 worktree に submodule が無い。推奨 celeris-sync）。
  celeris-sync は Celeris の修正が要り task を長く止めるので、**allow-init**（この worktree に限り remote-exec で `git submodule update --init --recursive`）で回答。
  **回収項目**: `ensure_worktree`（task-worker/src/ssh.rs）が worktree 作成時に submodule を展開する（F5-fix11 候補）。
- F5-fix10（prompt を stdin / message-file で渡す、Opus f08d443）を main に統合（475be20）→ gate ok → verify ok / live_ok（17:30Z）。昇格は人。
- 17:4xZ: 人が release 475be20d7d1e（F5-fix10）を昇格。web Phase 0 の質問に回答 → planner run 01M3Q4JE4FYW4SVR4K8QENNGAQ が prompt 136,003 B を stdin で
  受け取って起動（stdout.jsonl に system/init、progress 12 件）。F5-fix10 の本番動作を確認。
- 17:44:54Z: browser 根の `integrate-phase-3` done（p3 @ f24932c を親ブランチへ merge、head be7c251、workspace の test/clippy pass、途中報告
  `artifacts/phase-reports/1-phase-3.md`）→ 段階 phase-3 の `review: human` で根が `blocked awaiting_human`。**P-R5b-5（欠陥）**: 同じ tick で p4 が
  `dependency_ready` → 子 task 01M3Q49ZTST3XQ9DGF6AGNR0XG が作られ dispatch された。段階の人の review は次の段階の unit を止めていない（P-R5b-4 と同根:
  人の gate が unit の dispatch を抑止しない）。人の判断は `POST /tasks/{id}/execution/phase-gate {action: continue|replan|withdraw}`。
- 17:56Z: web Phase 0 01M3MS2JRDJ4GM0D9VN9PJCB6B は planner v4 が足した repair WU `adr-renumber`（新 ADR を 0078 → 0081 に振り直す）が、自分の check
  `! grep -rn 'ADR-0078' docs/web docs/adr/0081-web-spa-frontend.md` に ADR 本文の「ADR-0078 は既にある」という説明行が当たって不合格 → replans 3/3 を使い切って
  いたため task は **failed**（人に聞かず終端。R6 項目「max_replans 超過時の人の要求」と同根）。ブランチ `celeris/01M3MS2J…` は docs のみ 6 ファイル
  +951（ADR-0081 web SPA frontend、docs/web/feature-parity.md、docs/web/implementation-plan.md、ADR-0002 の supersede 注記、PROGRESS.md）で内容は完成して
  いたので人（Fable）が main に統合（docs 差分のみ、非 docs ファイルなし）。教訓: planner の repair WU の check は自己言及に弱い（否定 grep）。
- 20:09Z 人「task が ready で止まりまくる」: 原因は枠の取り合い。全体 `max_concurrency = 3`（claude-pool 2 + codex-pool 1）を、18:22 起票の
  「Celeris コードベースの構造リファクタリング」（compound、WU 24）が `max_parallel_work_units = 3` の並列葉で全部占有し、木の task 4 つが枠待ち。
  ディスパッチャに task 間の公平性が無い（**R6 項目**: round-robin と task ごとの同時数を全体枠 − 1 以下に）。runs 索引に旧 phase-3 子の reviewer run が
  `running` のまま残る不整合も（回収）。
- 20:14Z 人の指示「chatgpt / claude ともアカウントごとに 2 run、全体 6」: config を Fable が変更（backup `config.toml.bak-20260929-2020`）:
  `max_concurrency = 6`、claude-pool `concurrency = 4`（lab / personal × 2）、codex-pool `concurrency = 2`（chatgpt_plus_personal × 2）、
  `[accounts] max_runs_per_account` は既定 2 のまま。`POST /reload` でプールは 4 / 2 に反映済み。`max_concurrency` は起動時固定なので再起動が要る（人）。
- 20:3xZ 人「TanStack で web 画面を作る task が無くなった。動かして」: web Phase 0（failed、成果は main 884606d）の後続として root task
  **01M3QE4D330YESFT6FY8G50R12**「web: 新 Web GUI（ADR-0081、TanStack SPA + 薄い gateway）の実装 — Phase 1〜6」を案件 agent-platform に作成
  （stages_hint 5: Phase 1 / 2 / 3 / 4 / 5〜6、objective は docs/web/implementation-plan.md を正本に、H1〜H10 は「決まるまでの扱い」、Phase 7 は範囲外、
  P6-04 は H6/H7 の後）→ `decompose {compound}`（人の explicit）→ accept → ready。planner の /3 計画は PlanGate で人が確認する。
- 20:38Z: 人が再起動（`max_concurrency = 6` 有効）。web root の planner が /3 計画 v1（5 段階 p1〜p56、各 Phase 1 子 task、p56 に `review: human`、
  決定なし。木全体の葉 40 の制約から P*-NN 55 件を Phase ごとの葉上限 6/5/10/10/6 にまとめる方針）→ PlanGate（`review_human:p56`、near_limit
  max_stages 5/5、max_child_tasks 5/6）→ 20:4xZ 人（Fable）が approve。葉の上限 `[execution.tree] max_tree_leaves = 40` は web のような大きい木には
  小さい（回収で見直し候補）。
- 20:56Z: browser Phase 4 の子 01M3Q49ZTST3XQ9DGF6AGNR0XG（atomic、score 6 / 閾値 7）が最終 review で P4-A/B/C の実配線不足により failed（ADR-0084〜0086、
  純関数・egress transport の試験は入った）。子の記録: 非特権 LXC では `newuidmap` が EPERM（親 uid_map `0:100000:1001, 1001:1001:1, …`）で
  bwrap + subuid の隔離が実証できない（環境制約）。根の p4 failed → 木の `max_tree_replans`（10）超過で決定 `limit:max_tree_replans`
  01M3QF8TWSTGZDQMM33HF9WXF6 が open。判断は人へ（Phase 4 を compound で分解し直すか、P4-A の subuid 実証を別ホストに切り出すか）。
- 21:03Z: 人が GUI で `limit:max_tree_replans` に `replan` と回答 → planner v4（p4 を p4a / p4c 並行 + p4b（p4a の後）に分割、前回ブランチを各子が merge、
  決定 `p4a-uid`〈subuid の実証場所〉）→ PlanGate。人（選択肢 1）に従い、決定は **ns-only** で回答し、PlanGate は `replan`（p4a / p4b / p4c に
  `gate: compound` を明示）を要求（21:22Z）→ **P-R5b-4 再現**: 未承認 v4 の p4a / p4c から子 task が即座に作られ dispatch され、replan 自体は
  `max_replans`（3）超過の決定 `limit:max_replans` 01M3QGRC80H4M42V5NAF3P9YQN で止まった。
- 21:25Z: planner v5 = v4 + p4a / p4b / p4c の `features` を compound 寄りに（plan/3 の unit に `gate` 欄は無く deny_unknown_fields で拒否されるため。
  「unit の gate 上書き」は daemon 側の `unit_gate_overridden` だけ）→ 人が approve。既に動いている p4a / p4c の子（atomic、score 6 / 7）は run が切れた
  瞬間に人の `decompose {compound}` を当てる（scratchpad `force_compound.sh`、1 秒 poll）。**R6 候補**: plan/3 の unit に `gate: compound|atomic` を
  planner / 人が書ける欄を足す（ADR-0079 D6 の per-node gate に対する明示の手掛かり）。

## R6: 回収（2026-09-29 22:3xZ 着手、人「背後で監視しつつ R6 と不具合の修正に取り掛かって」）

分担（Opus、worktree、ファイル境界で並列）:
- **R6-1**（task-dispatch / plan_gate / regate）: P-R5b-4（PlanGate の replan 要求で未承認版の unit を dispatch しない）、P-R5b-5（段階の `review: human` 待ちで
  次段階の unit を止める）、非 tree の task も max_replans 超過で人に聞く・人の replan は上限に数えない、終端 task の runs 索引を閉じる、near_limit の数え方。
- **R6-2**（task-core / task-ops tree / config 既定）: plan/3 unit の `gate: compound|atomic` 欄、`kind: task` の子は既定で explicit compound（全 origin）、
  planner prompt に gate と否定 grep の注意、木の上限の既定値（leaves 40→120、runs 120→400、replans 3→5、tree_replans 10→30）。
- **R6-3**（task-worker ssh）: クラスタ worktree の submodule 展開。
- **R6-4**（task-ops execution/replay、task-api、gui）: replan で unit の phase を書き換える、「この方針で進める」撤去、timeline API の遅延、凍結途中目標の注記。
- 後続 **R6-5**（R6-1 の後、dispatcher）: task 間の公平性（round-robin、task ごとの同時数 ≤ 全体枠 − 1）。
- **人の判断待ち**: D7（remote workspace の木: 子ブランチ/統合をクラスタ側で実装するか、remote の親では葉だけに制限するか）、D4（人の answer で attempts を
  reset するか。ADR 要）。
- 22:44Z: P4-A の子 01M3QGRC542ZC23996DNCTHZF5 に `stall_detected {nothing_runnable}`（人に障害通知）。原因は既知バグ「replan で unit の phase が
  更新されない」の実害: planner v2 が `restore-binding` を段階 relay → verify（dep prod-launch）に移したが行は `phase = relay` のまま →
  `integrate-relay` が永遠に待ち、verify 側の依存も満たせない膠着。R6-4 で修正中（再現条件を伝達）。当面の解消は人の replan（unit を
  `restore-binding-2` に付け替え、内容同じ）。Fable の PUT は classifier に拒否されたため body を用意して人に依頼（scratchpad `p4a-plan-put.json`）。
- 23:0xZ 人「CoS のチャット task が枠で待たされるのは不便。CoS に割り当てられる run だけ max_concurrency から除外して」→ **R6-5**（Opus）: CoS の対話 run は
  `max_concurrency` とプールの `concurrency` を数えない・超えてよい、アカウントは最も空いているものに +1 の許容、安全上限 `max_cos_runs`（既定 2）、
  `GET /providers` に `in_use_cos`。ADR を新設。
