# ADR: task の「なぜ今走らないか」（待ちの理由）を dispatcher と同じ根拠から出す

- Date: 2026-10-11
- Status: 実装済み（D4 の DaemonSnapshot 旧 3 欄の削除を除く。付記「検証葉 verify」を参照）。実装は task 01M4NAKDCCS5XYAAJ518MVZ134 の葉 core・api・web-list・web-tree
- 決定者: 人の依頼（2026-10-11 運用セッション）を受けた計画。方式の細部はこの ADR が決める
- 関連: ADR-0044 D6（案件・途中目標の停止）、ADR-0079 D5/D13（子待ち・subtree の一時停止）、ADR-0074 D1.3（工程の scheduler）、
  ADR-0130（write-set）、ADR 2026-10-08-account-concurrency-limits（枠）、[破壊的変更を既定とする](2026-10-11-breaking-changes-by-default.md)

## Context

2026-10-11 11:05 UTC の本番で `status = ready` の task が 5 件あったが、dispatch できるものは 0 件だった。

| task | 実際の理由 | どこで見送られたか |
|---|---|---|
| 01M4M8D4AN・01M4M8D4AP・01M4MAYXS5 | 子 task（計画の kind task の unit）の完了待ち。自分の leaf は無い | `wu_dispatch_gate` → `resolve_child_dependencies` の `waiting_on_children` → `WuDispatchGate::Skip` |
| 01M4M8D4AQ | `depends_on` の 01M4M8D4AP が未完 | `ready_tasks_impl` の depends_on の検査（`continue`） |
| 01M3PAZ4XG（BenchFS） | 人が 10/02 に一時停止（`paused_at`） | `ready_tasks_impl` → `halted_by_ancestry_locked` |

人には「ready が多いのに走る数が少ない」としか見えず、DB を読んで調べた。詳細の木でも WorkUnit の key・段・depends_on が読めず、
01M4MEEKX4 のように段 edges→remove→close が直列で、dispatch-prune が cos-route を、integrate-edges が 4 つの unit を待っていることが画面から分からない。

さらに現状は「子待ち」の判定が 2 つある（`task-ops/view.rs` の `awaits_only_children`〈ready/running の kind task unit があり leaf が無い〉と、
dispatcher の `resolve_child_dependencies`〈`child:` 依存を持つ pending unit がある〉）。画面用に別の推測を持っている状態で、食い違いうる。

### 見送りの箇所の棚卸し（2026-10-11 の main `4ef38473`）

**DB だけで決まるもの**

| # | 理由 | 現在の判定箇所 |
|---|---|---|
| D1 | 自分が一時停止（`paused_at`） | `task-core store/projects.rs` `halted_by_ancestry_locked`（`ready_tasks_impl` から） |
| D2 | 祖先が一時停止／祖先が止まった案件に属す | 同上（`parent_id`、無ければ `tree.parent_unit.task_id` を辿る。上限 32） |
| D3 | 案件が paused / cancelled / archived | `ready_tasks_impl` + `halted_projects_locked` |
| D4 | 途中目標が paused / cancelled | `ready_tasks_impl` + `halted_milestones_locked` |
| D5 | `depends_on` の task が未完（対話は終端なら可） | `ready_tasks_impl` |
| D6 | 親が未完の Approval | `ready_tasks_impl` |
| D7 | 子 task 待ち（計画の kind task の unit が未完、または `child:<key>` 依存の pending unit） | dispatcher `work_units.rs` `resolve_child_dependencies` → `Skip`、表示は `view.rs` `awaits_only_children` |
| D8 | WU の depends_on 待ち（現在の段で runnable な unit が無く、pending の unit が未完の key を待つ） | `task-core execution_plan/scheduling.rs` `newly_ready` / `runnable_work_units`、dispatcher `settle_phase` → `PhaseSettle::Wait` → `Skip` |
| D9 | 段待ち（前の段の行〈統合 WU を含む〉が全部 done でない） | `scheduling.rs` `earlier_phases_done` |
| D10 | 段の中に failed / blocked(decision・infra・cluster_jobs 以外) があり新しい unit を起こさない | `runnable_work_units` の `has_failed_or_blocked` |
| D11 | 人の決定待ち（`needed_before: self` の未回答・`blocked(decision)` の unit・木の上限の決定） | dispatcher `decision_self_hold` / `tree_run_limit_hold`、`PhaseSettle::Question` |
| D12 | 人の gate（計画の承認・段の `review: human`・途中確認） | `human_gate_hold`（task は blocked のことも多い。`plan_gate` / `phase_gate`） |
| D13 | unit が `blocked(infra)`／`blocked(cluster_jobs)` | `runnable_work_units`（兄弟は止めない）・ADR-0090 |

**dispatcher のメモリにしか無いもの**（tick ごとに変わる。DB に書かない）

| # | 理由 | 現在の判定箇所 |
|---|---|---|
| M1 | 枠が満杯（全体 `max_concurrency`・CoS 枠・account の上限・暗黙アカウント〈LLM source〉の上限） | `dispatch_ready` の `run_load().any_slot()`／`admits`、`provider_select` の `full`・`implicit_account_full`・`ExcludedReason::AtCapacity` |
| M2 | unroutable（設定に合う provider／経路が無い） | `provider_select`・`dispatch_run` の `self.unroutable` |
| M3 | cluster 待ち（cooldown・ControlMaster 不通・auto-connect 失敗） | `dispatch_run` の `self.cluster_waiting` |
| M4 | write-set（同じ repo の走っている run と expected write-set が重なる） | `write_set_gate`（`write_set_waits`） |
| M5 | バックオフ（`attempts` の再試行・infra の再試行） | `dispatch_one` の `retry_delay`・`infra_backoff` |
| M6 | disk critical | `held_by_disk_critical` |
| M7 | reviewing の親が委譲した子を待つ（ADR-0016 M5） | `self.awaiting_children` |
| M8 | reviewing のまま人の承認を待つ（ADR-0010 D8） | `self.awaiting_human` |
| M9 | 窓・容量で今 tick 照合されなかった | `dispatch_ready` の `capacity_cut`（M1 の global として出す） |

`verify` モードの `is_eligible`・`just_aborted`（1 tick だけ）は人に出す意味が無いので理由にしない。

## Decision

### D1. status は足さない。待ちは導出欄 `waits` で出す

状態機械（`ready`）は変えない。理由:
- 待ちの理由のうち M1〜M9 は tick ごとに変わる。status にすると遷移の event（追記専用）が tick ごとに積まれ、lease・受信箱・orphan 回収・集計など status を鍵にする全経路に影響する。
- 1 つの task が同時に複数の理由で待つ（例: 子待ちかつ祖先停止）。1 つの status に潰すと情報が落ちる。
- 「走れるか」は status ではなく `ready && waits が空` という導出で十分に区別できる（下の `runnable`）。

### D2. 理由の型（task-core に置く。API・snapshot・web で同じ型）

新しい module `crates/task-core/src/wait.rs`（`pub use` で `task_core::TaskWait` 等）。`Serialize`/`Deserialize`/`JsonSchema`、`#[serde(tag = "kind", rename_all = "snake_case")]`。

```rust
pub enum TaskWait {
    // ---- DB だけで決まる（task-core の純粋関数 `db_waits` / `plan_waits`）----
    Paused { by: TaskId },                         // D1/D2。by == 自分なら自分の一時停止
    AncestorProjectHalted { task_id: TaskId, project_id: ProjectId }, // D2 の「祖先が止まった案件」
    ProjectHalted { project_id: ProjectId, state: HaltState },        // D3。HaltState = paused|cancelled|archived
    MilestoneHalted { milestone_id: MilestoneId, state: HaltState },  // D4
    DependsOn { task_ids: Vec<TaskId> },           // D5。未完の先行 task だけ（id 順）
    ApprovalParent { task_id: TaskId },            // D6
    Children { task_ids: Vec<TaskId>, unit_keys: Vec<String> },  // D7。未完の子と、その代理 unit の key
    WorkUnits { units: Vec<UnitWait> },            // D8/D9。runnable な unit が無く、待っている unit がある
    StageHalted { stage: String, unit_keys: Vec<String> },        // D10。段に failed/blocked がある
    HumanDecision { decision_keys: Vec<String> },  // D11
    HumanGate { gate: HumanGateKind, stage: Option<String> },     // D12。plan_approval|stage_review|phase_checkpoint
    UnitsBlocked { reason: WorkUnitBlockedReason, unit_keys: Vec<String> }, // D13（infra / cluster_jobs）
    // ---- dispatcher のメモリ（DaemonSnapshot.task_waits から合わせる）----
    Capacity { scope: CapacityScope, id: Option<String> },        // M1/M9。scope = global|cos|account|source
    Unroutable { detail: String },                 // M2
    Cluster { cluster_id: String, detail: String },// M3
    WriteSet { blocking_task_ids: Vec<TaskId> },   // M4
    Backoff { until: String, cause: BackoffCause },// M5。cause = retry|infra。until は RFC 3339
    DiskCritical { path: String },                 // M6
    DelegatedChildren { task_ids: Vec<TaskId> },   // M7
    ReviewAwaitingHuman,                           // M8
}

pub struct UnitWait {
    pub key: String,
    pub stage: Option<String>,
    /// depends_on のうち done でない key（宣言順）。`child:<key>` 依存もそのまま出す。
    pub waiting_on: Vec<String>,
    /// 前の段が終わっていないなら、その段の key（D9）。
    pub waiting_stage: Option<String>,
}
```

並びは上の宣言順（DB の理由 → メモリの理由）。web の「短い理由」は先頭の 1 件、全件は詳細に出す。

### D3. 判定する関数（DB の分は task-core の純粋関数で、`ready_tasks_impl` と dispatcher が共有）

`crates/task-core/src/wait.rs`:

- `pub struct WaitFacts<'a> { task: &'a Task, ancestors: &'a [Task] /* 親から root へ。上限 32・循環で打ち切り */, deps: &'a [(TaskId, Option<Status>)], parent: Option<&'a Task>, halted_projects: &'a HaltedSet, halted_milestones: &'a HaltedSet }`
- `pub fn db_waits(f: &WaitFacts) -> Vec<TaskWait>` — D1〜D6。対話（`is_conversation`）の例外（停止を無視・depends_on は終端で可）もここに入れる。
- `pub fn plan_waits(units: &[WorkUnitRow], child_status: &dyn Fn(TaskId) -> Option<Status>, open_decision_keys: &[String]) -> Vec<TaskWait>` — D7〜D11・D13。
  `Children` は「未完の kind task の unit（`child_task_id` があるもの）」と「`child:<key>` 依存を持つ pending unit の key」の和。
  今の `view.rs::awaits_only_children` と dispatcher の `resolve_child_dependencies` の `waiting` を **この 1 つに置き換える**。
  `WorkUnits` は `runnable_work_units(units, 0, usize::MAX)` が空で、走っている unit も無いときだけ出す（`unit_waits` の結果）。
- `pub fn unit_waits(units: &[WorkUnitRow]) -> Vec<UnitWait>` — pending の各 unit について、`newly_ready` と同じ規則（done の key 集合・`phase_ranks`・`earlier_phases_done`）で未完の依存と前の段を返す。
  `newly_ready` は内部で `unit_waits` を使って「待ちが空の pending」を返す形に書き換え、規則を 1 か所にする。
- 人の gate（D12）は `task-ops` の `plan_gate::latest_plan_approval` / `phase_gate::latest_phase_checkpoint` と dispatcher の `human_gate_hold` が events から決める。
  これは events を読む決定的な関数なので task-ops に置いたまま、`task_ops::wait::gate_waits(task, events) -> Vec<TaskWait>` にまとめ、dispatcher の `human_gate_hold` もそれを呼ぶ。

呼び出し側:
- `ready_tasks_impl`: 各行について `ancestors_locked`（`halted_by_ancestry_locked` を「祖先の行を集める」関数に置き換え）・deps・parent を読み、
  `db_waits(..).is_empty()` の task だけ返す。**`Children`・`WorkUnits` で ready_tasks から外すことはしない**（dispatcher の `wu_dispatch_gate` は
  子の完了で unit を ready に上げる副作用〈`resolve_child_dependencies`・`promote_newly_ready`〉を持つので、今どおり候補として受け取る）。
- dispatcher `wu_dispatch_gate`: `waiting_on_children` の代わりに `plan_waits` の `Children` を使う。`settle_phase` の Wait/Question は `plan_waits` と同じ入力。
- 一覧・木の API: `TaskStore::task_waits(ids: &[TaskId]) -> Result<HashMap<TaskId, Vec<TaskWait>>, StoreError>`（task-core store）。
  halted 集合は 1 回、祖先・deps は一覧で既に読んだ行を使い、計画は非終端の task の `work_units_for` だけ読む。終端の task は空。

### D4. メモリの理由は DaemonSnapshot に task ごとに載せる

- dispatcher に `dispatch_waits: HashMap<TaskId, TaskWait>` を持ち、`dispatch_ready` の先頭で clear、`dispatch_one` が `Ok(false)` で見送る各箇所（M1〜M6、M9）で
  理由を 1 件入れる（同じ task の 2 回目は上書きしない）。M7・M8 は既存の `awaiting_children`・`awaiting_human` から組む。
- `DaemonSnapshot.task_waits: Vec<SnapshotTaskWait { task_id: TaskId, wait: TaskWait }>`（task_id 順）。
- **消す**: `DaemonSnapshot.unroutable`・`DaemonSnapshot.awaiting_children`・`DaemonSnapshot.awaiting_human`（同じ情報が `task_waits` の
  `Unroutable` / `DelegatedChildren` / `ReviewAwaitingHuman` に入る）。dispatcher 内部の集合（`is_idle` などが使う）はそのまま。
  gui の `daemon.tsx`・`graph.tsx`・`graph-layout.ts` は `task_waits` を kind で絞って読むよう同じ変更で直す。
- API は応答を組むとき、snapshot が古すぎない（`last_tick_at` から `tick_ms × 10` 以内）なら task の `waits` の末尾に足す。古ければ足さない（推測しない）。

### D5. API の欄

- `TaskSummary`（`GET /tasks`・ボード）: `waits: Vec<TaskWait>` と `runnable: bool`（`status == ready && waits.is_empty()`。サーバで決め、web は再計算しない）。
- `TaskDetail`: 同じ `waits`・`runnable`。
- `TaskTreeNode`: `waits: Vec<TaskWait>`、`stages: Vec<TreeStageView { key, order: u32, state: StageState /* done|current|pending */, integrate_key: String }>`（`phase_ranks` の順）。
- `TreeUnitView`: 既存の `key`・`stage`・`kind`・`status`・`blocked_reason`・`child_task_id` に加え、`seq: u32`、`depends_on: Vec<String>`（宣言どおり）、
  `waiting_on: Vec<String>`（未完のもの。`unit_waits` の値）、`waiting_stage: Option<String>`。
- `GET /tasks` に query `run_state=runnable|waiting`（`runnable` = `runnable == true`、`waiting` = 非終端で `waits` が空でない）。GET なので CoS ops registry の対象外。

### D6. 置き換えて消す旧い欄（同じ変更で消す。互換の分岐は残さない）

| 消すもの | 置き換え先 |
|---|---|
| `TaskSummary.pending_children`（と `view.rs` の `child_counts` の pending 側・`task_ops::delegate::pending_children` の表示用途） | `waits` の `Children`／`DelegatedChildren`。`children`（子の数）は待ちではないので残す |
| `TreeNodePhase::AwaitingChildren`・`TreeNodePhase::HeldOnDecision`・`TreeNodePhase::BlockedInfra` | `TaskTreeNode.waits` の `Children`・`HumanDecision`・`UnitsBlocked`。`TreeNodePhase` は進み具合（Planning/Executing/Repairing/Verifying/AwaitingHuman/AwaitingPlanApproval）だけにする |
| `ExecutionPhase::AwaitingChildren`・`ExecutionView.awaiting_children`・`AwaitedChildView` | `TaskDetail.waits` の `Children`。子を待つ ready の計画持ち task の `ExecutionPhase` は `Executing` |
| `task-ops view.rs` の `awaits_only_children`、dispatcher `resolve_child_dependencies` の戻り値の `waiting` | task-core `plan_waits` |
| `DaemonSnapshot.unroutable`・`.awaiting_children`・`.awaiting_human` | `DaemonSnapshot.task_waits` |
| web `execution-panel.tsx`・gui `tree.ts`／`task-execution.ts` の `awaiting_children` ラベル、web e2e の `pending_children` の fixture | `waits` の表示 |

`backoff_until`（一覧の既存欄）は DB から決まる再試行の時刻なので残す（`Backoff` は dispatcher が実際に見送った事実）。

### D7. web の表示

**ボード（`web/features/projects/board-screen.tsx`）と一覧（`web/features/tasks/task-list-view.tsx`）**
- `runnable` の task に「走れる」印。`waits` がある task に短い理由 1 つ（先頭の理由）と、2 件以上なら「ほか N」。
  例: `子 2 件待ち`・`依存待ち: <先行 task の題名>`・`一時停止中`／`祖先が一時停止`・`WU 待ち: dispatch-prune ← cos-route`・`段待ち: remove`・`枠満杯（account x）`・`経路なし`・`cluster 待ち`・`人の決定待ち`。
- 辿り先: `Children`・`DependsOn`・`DelegatedChildren`・`ApprovalParent`・`Paused{by≠自分}` は task の詳細へのリンク（子が複数なら最初の未完と「ほか N」→ 詳細の木）。
  `WorkUnits`・`StageHalted` は詳細の木（`#task-tree`）へ。`HumanDecision`・`HumanGate` は決定・受信箱へ。
- 絞り込み: 「すべて／走れる／待ち」（一覧は `run_state` query、ボードは ready 列の中で同じ 3 つ）。ready 列の中は「走れる」を先に並べる。
- 表示文言は `web/features/tasks/wait-reason.ts`（kind → 短い文・リンク先の純粋関数）1 か所に置き、ボード・一覧・木で共有する。

**詳細の木（`web/features/tasks/overview-view.tsx` の `#task-tree`）**
- 段の帯: `stages` を順に `edges（済）→ remove（今）→ close` の形で出し、直列であることを矢印で示す。
- unit の行: `key`（等幅）・`kind` の印・`status` の badge・段、未完の依存があれば `待ち: cos-route` の chip（`waiting_on` の key ごと。押すとその unit の行へ scroll）、
  前の段待ちなら `段待ち: edges`、kind task の unit は子 task へのリンク。統合 WU（`integrate-<stage>`）は段の最後に置き、待っている unit の key を出す。
- 子 task の節点の行にも `waits` の短い理由を出す。
- スマホ幅（390×844）: 行は 2 段（1 段目 key・status、2 段目 kind・段・待ち chip）で折り返し、横溢れを起こさない。

### D8. 例（2026-10-11 の 5 件と 01M4MEEKX4 の見え方）

| task | `waits`（先頭） | 一覧の短い理由 | 辿り先 |
|---|---|---|---|
| 01M4M8D4AN／AP／MAYXS5 | `Children { task_ids: [未完の子…] }` | `子 N 件待ち` | 最初の未完の子 |
| 01M4M8D4AQ | `DependsOn { task_ids: [01M4M8D4AP…] }` | `依存待ち: <AP の題名>` | 01M4M8D4AP |
| 01M3PAZ4XG | `Paused { by: 01M3PAZ4XG }` | `一時停止中` | （自分。再開の操作） |
| 01M4MEEKX4（木） | unit `dispatch-prune` の `waiting_on: [cos-route]`、`integrate-edges` は 4 key、`stages` = edges→remove→close | `WU 待ち: dispatch-prune ← cos-route` | 詳細の木 |

この 5 件はどれも `runnable = false` になり、「走れる」の絞り込みに入らない。

### D9. 試験

- task-core・task-dispatch: `task_wait_` 接頭辞。`db_waits` の各理由（対話の例外を含む）、`plan_waits`／`unit_waits` が `newly_ready`・`runnable_work_units` と食い違わないこと
  （同じ入力で「待ちが空 ⇔ newly_ready に入る」）、`ready_tasks_impl` が `db_waits` 非空の行を返さないこと、dispatcher の見送りが snapshot の `task_waits` に載ること。
- task-ops・task-api: `task_wait_api_` 接頭辞。上の 5 件を再現した store で一覧・木・`run_state` の応答。
- web: vitest（`web/features/tasks/wait-reason*.test.ts(x)`）、e2e `web/e2e/work/wait-reasons.spec.ts`・`web/e2e/work/task-tree-waits.spec.ts`（両方に 390×844 の横溢れ無しの assert）。

## Consequences

- 「ready なのに走らない」の理由が画面で読め、運用が DB を読まなくてよくなる。dispatcher と画面が同じ関数を使うので、表示と挙動が食い違わない。
- 一覧の応答で非終端 task の計画を読む分だけ重くなる（終端は読まない）。計測して遅ければ api 葉で `work_units` の一括読みを足す。
- メモリの理由は最後の tick の観測で、古い snapshot は出さない。API プロセスと dispatcher が別のときは snapshot の同期間隔だけ遅れる。
- gui（旧 GUI）の daemon 画面・graph は `task_waits` に読み替える（旧欄は消す）。

## 実装時の付記

（後の葉が実装との食い違いをここに書く）

### 葉 core（2026-10-11）
- `wait.rs` の関数は ADR の名前どおり。加えて `halt_waits`（D1〜D4 だけ。`halted_by_pause` が使う）、`resolve_children`（`ChildFacts{task_id,status,labels}` から
  `child:<key>` の done/failed と待つ unit を返す。dispatcher の `resolve_child_dependencies` と `plan_waits` が共有）、`parent_of`・`MAX_ANCESTRY` を公開した。
  `plan_waits` の第 2 引数は ADR の `child_status: &dyn Fn` ではなく `&[ChildFacts]`（label で `child:<key>` と結ぶのに要る）。
- `HaltedSet` は id → `HaltState` の `HashMap`。`halted_projects_locked` はアーカイブを `Archived`、それ以外を status で分ける。
- D3 の `TaskStore::task_waits` は trait には足さず、`SqliteStore::db_waits_for(&[Task])`（DB の D1〜D6 だけ）を置いた。計画の待ちは api 葉が
  `work_units_for` と `plan_waits` で足す。
- D4: provider が選べなかった見送りは、`unroutable` に入っていれば `Unroutable`、そうでなければ `Capacity{scope: source}`（満杯と cooldown を区別しない）。
  クラスタの並列度の満杯は `Capacity{account}` ではなく `Cluster{detail: "concurrency full"}`。全部の枠が埋まって `dispatch_ready` が早期 return する tick も、
  窓の ready に `Capacity{global}` を付ける（ready_tasks を 1 回読む）。
- D4 の旧 3 欄（`unroutable`・`awaiting_children`・`awaiting_human`）の削除は未了（gui の読み手が core・api 葉の範囲外）。進捗 core.md の「未解決」。

### 葉 api（2026-10-11）
- `TaskStore::db_waits_for` を trait に足した（task-ops が `&dyn TaskStore` で呼ぶため。実体は `SqliteStore::db_waits_for_impl`）。
- 合わせる関数は `task_ops::wait::task_waits`（DB → 計画 → 人の gate → メモリの順）。`gate_waits` は plan approval と途中確認だけ
  （`StageReview` はまだ出さない。dispatcher の `human_gate_hold` は共有していない）。
- `run_state` の頁は SQL の cursor ではなく、絞った後の位置（`rs:<n>`）で切る。`total` は絞った後の件数。
- `TreeStageView.order` は段の位置（0 から）。`TaskExecutionView`（`GET /tasks/{id}/execution`）にも `waits` を出し、`awaiting_children` を消した。
- D4 の旧 3 欄（`DaemonSnapshot.unroutable` 等）の削除は api 葉でも未了（読み手が gui・web の daemon 画面）。

### 検証葉 verify（2026-10-11）
- 実装と食い違う点: D4 の `DaemonSnapshot.unroutable`・`.awaiting_children`・`.awaiting_human` は削除されていない（`crates/task-ops/src/daemon.rs` に残り、dispatcher の内部集合も同名で残る）。
  `task_waits` には同じ情報が入っているが、旧 3 欄を消す作業（D4・D6 の最後の行）は未了。core・api 葉の範囲外として残した。
- 対応: 旧 3 欄の削除を後続の葉か task に分ける（進捗 `agent-docs/progress/2026-10-11-task-wait-reasons.md` の「未解決」）。
- 検証の結果（test-parallel・clippy・fmt・web typecheck/lint/vitest/e2e）は進捗の証拠節を参照。

### 葉 snapshot-old（2026-10-11、final review の差し戻し修正）
- D4・D6 の最後の行を実装した: `DaemonSnapshot.unroutable`・`.awaiting_children`・`.awaiting_human` を `crates/task-ops/src/daemon.rs` から消し、
  `crates/task-dispatch/src/dispatcher/snapshot.rs` の組み立てからも消した。互換の分岐・deprecated は残さない。API schema（`docs/api/v1/api-v1.schema.json`）と
  web の生成型（`web/api/generated/`）からも消えた。
- dispatcher の内部集合（`Dispatcher.unroutable`・`awaiting_children`・`awaiting_human`。`is_idle`・レビュー延期が使う）は D4 どおり残し、
  `snapshot_task_waits` が `Unroutable`（`dispatch_waits` 経由）・`DelegatedChildren`・`ReviewAwaitingHuman` に写す。
- 読み手の移し先: 受信箱の `attention[].unroutable`（`task_ops::inbox`）は `task_waits` の `Unroutable` から組む。web の daemon 画面の
  「人の待ち」「担当なし」は `task_waits` を kind（`review_awaiting_human`・`unroutable`）で数える。`docs/api/v1/api.md` §3.20 を書き換えた。

### 葉 stage-gate（2026-10-11、final review の差し戻し修正）
- D12 の人の gate を 1 つの関数にまとめた: `task_ops::wait::human_gate_waits(task, plan: Option<&ExecutionPlanRow>, events) -> Vec<TaskWait>`。
  旧 `gate_waits` は消した（互換の別名なし）。dispatcher の `human_gate_hold`（`crates/task-dispatch/src/dispatcher/tree_units.rs`）はこの結果が
  空でないかで止め、理由の文字列も結果の gate から作る（`PlanApproval` → `plan_gate_pending`、`StageReview` → `stage_review`、
  `PhaseCheckpoint` → `human_gate`）。API の `task_waits` は `store.execution_plan_active(task.id)` を渡して同じ関数を呼ぶ。
- 判定（どれも既存の事実だけ。推測しない）:
  1. 有効な版が PlanGate を通っていない（`plan_gate::plan_gate_state == Pending`）か `plan_gate::is_awaiting_plan_approval` → `PlanApproval{stage: None}`。
  2. `phase_gate::is_awaiting_human` → 直近の `PhaseReported.phase` が /3 の計画の `stages` で `review: human` の段なら `StageReview{stage: 段 key}`、
     そうでなければ（/2 の `pause_after`、該当する段が無い）`PhaseCheckpoint{stage: phase}`。`PhaseReported` が無ければ `stage: None`。
- 以前の `gate_waits` との違い: Blocked 限定の条件は関数側の判定（`is_awaiting_*` が Blocked を見る）に寄せた。その結果、PlanGate を通っていない版
  （承認待ちのまま人が replan を求め `ready` に戻った task など）は、dispatcher が止めているとおり `waits` にも `PlanApproval` が出る。
