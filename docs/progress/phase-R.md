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
