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
