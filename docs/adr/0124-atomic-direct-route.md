# ADR-0124: atomic coding task の planner なし直行経路（決定的 routing）

---
tasks: [01M3Y2KXY3R6YXD6DEDDNFG97T]
---

- 日付: 2026-10-02
- 状態: 提案（Phase 4。実装は後続の葉 core-route / api-route / dispatch-route / gui-route）
- 関連: [ADR-0072 D13/D14](0072-task-execution-decomposition.md)（Complexity Gate・planner run）、[ADR-0074](0074-parallel-work-units-checkpoints-milestones-quota.md)（WU・RepairScope）、[ADR-0079](0079-recursive-task-decomposition.md)（再帰の木・段階統合）、[ADR-0043](0043-workspaces.md)（worktree・`[commands] check`）、[ADR-0117](0117-review-human-decisions-and-check-results.md)（決定的 check 結果を reviewer に渡す）、[ADR-0118](0118-review-target-sync-and-merge-candidate.md) / [ADR-0120](0120-pre-review-sync-integration-repair.md)（review 前同期・IntegrationRepair）、ADR-0121（root delivery。別ブランチで進行中で、この時点の main には未取り込み）
- 番号: main と全 `celeris/*`・`celeris-wu/*` ブランチ、および稼働中の worktree の `docs/adr` を走査し、0122 までが使用済み。0116 は browser launcher 用に避け、0123 は同時に走っている Phase 3（session reuse）の ADR が取る見込みなので避けた。

## 背景

本番は `[execution] gate = "on"`・`[execution.tree] enabled = true`。いまの経路は 2 本ある。

- **atomic 経路**: `execution_gate_if_needed`（`crates/task-dispatch/src/dispatcher/work_units.rs`）が `ExecutionMode::Atomic` を記録し、`wu_dispatch_gate` が `WuDispatchGate::Atomic`（計画なし）を返す。`dispatch_run.rs` の `is_planner_dispatch` は偽で、Task の worktree で 1 本の worker run が走る。終わると `Reviewing` に入り、`review_spawn.rs::spawn_review` が review 前同期（ADR-0118/0120）→ command checks（Task の `Check::Command` と、無ければ repo の `[commands] check`。ADR-0043 D4）→ reviewer を回す。不合格は `review_verdict.rs` の `try_review_repair`（ReviewRepair）か `ReviewFail` の再試行。
- **compound 経路**: gate が `Compound` を記録し、`is_planner_dispatch` が真になると planner run（`[execution.planner]`、/3 なら木の planner）が計画を書き、WU・子 task・段階統合（ADR-0079 D6）・最終レビュー（`start_final_review_from_ready` → 同じ `spawn_review`）を通る。

問題は、具体的な repo・objective・command check を持つ実装 task でも、規則表のスコア（F1 context_size・F2 expected_length・F4 cross_cutting・S1 工程語・S5 objective の長さ）で `compound/score` になりやすいことである。計画の文が長いだけで planner run（最大 24 turn / 900 秒）・ADR WU・統合 WU が挟まり、実行時間と並列開発時の統合失敗の機会が増える。逆に「分けるべき」task（cross-repo・cross-department・人の承認が要る）を見分ける構造的な信号は gate のスコアに混ざっていて、経路の理由として表に出ていない。

## 決定

### D1. 直行経路の適格条件を純関数で決める（LLM に可否を判断させない）

新しいモジュール `crates/task-core/src/direct_route.rs` に次を置く（I/O・LLM なし。ADR-0001 D2）。

```rust
pub const DIRECT_ROUTE_POLICY_VERSION: &str = "direct-route/1";

pub enum Route { Direct, Planned }            // serde: "direct" / "planned"

pub struct RouteReason {
    pub rule_id: String,   // 例 "direct/single-repo"
    pub ok: bool,          // この条件を満たしたか
    pub detail: String,    // 例 "repos=2 (agent-platform, benchfs)"
}

pub struct RouteDecision {
    pub route: Route,
    pub reasons: Vec<RouteReason>,   // 評価した全条件（満たしたものも残す）
    pub gate_rule_id: String,        // 入力にした ExecutionGateDecision.rule_id
    pub overrode_gate: bool,         // gate の compound/score を直行で上書きしたら true
    pub shadow: bool,                // gate = "shadow" の記録だけの判定なら true
    pub policy_version: String,
}

/// dispatcher/store が決定的に計算して渡す信号（純関数のままにするため）。
pub struct DirectRouteInputs {
    pub coding_harness: bool,        // 担当の実効 harness が coding（受けられるハーネスの既定が coding）
    pub cross_department: bool,      // 担当部署以外の skill/genre を要する、または cross-department の認可/委譲が記録済み
    pub pending_approval: bool,      // 未決の承認（approvals 表の pending 行）がある
}

pub fn evaluate(task: &Task, gate: &ExecutionGateDecision, inputs: DirectRouteInputs) -> RouteDecision;
```

`route = Direct` は次の条件を**すべて**満たしたときだけ。どれか 1 つでも外れれば `Planned`。全条件を `reasons` に残す（満たさなかったものは `ok = false`）。

| rule_id | 条件 |
|---|---|
| `direct/in-scope` | `execution_gate::out_of_scope_rule(task)` が `None`（kind = Execute・対話でない・support-task でない・routing あり・固定パイプラインの genre でない・`workspace_mode != Shared`） |
| `direct/coding` | `inputs.coding_harness` |
| `direct/single-repo` | `task.repos.len() == 1`。remote workspace は repo が 1 つでも local repos と混在しない（gate S4 と同じ定義で `multi_environment` でない） |
| `direct/single-department` | `task.assignee` が決まっていて `!inputs.cross_department` |
| `direct/no-human-approval` | acceptance に `Check::Human` が無い、かつ `!inputs.pending_approval` |
| `direct/command-check` | acceptance に `Check::Command` が 1 つ以上ある（repo の `[commands] check` だけでは足りない。直行の合否を決定的に固定するため） |
| `direct/no-plan` | `task.aggregate == false`（委譲の子を集める run でない）。計画（`execution_plan_active`）を持つ Task には evaluate を呼ばない（呼び出し側の前提） |
| `direct/gate` | 下の gate 条件 |

`direct/gate` は gate の判定を次のように扱う。

- `mode = Atomic`: 満たす。
- `mode = Compound` で `source = Policy` かつ `rule_id = "compound/score"`、かつ root（`gate.depth` が `None`・`tree::is_tree_child` でない）: 満たし、`overrode_gate = true`。スコアの compound は文の長さ・語に引きずられるので、構造の条件（上の 7 行）が全部満たされるならそちらを優先する。
- それ以外の compound は満たさない（**従来の分解経路を維持**）: `source = Human`（`human/explicit`。`POST /tasks/{id}/execution/decompose` を含む）、`source = Hint`（CoS のヒント）、強制規則 `compound/long-and-broad`、木の子の compound（親の planner が kind task と決め、深さ付きの閾値で判定したもの。ADR-0079 D4 (1)）。

`Planned` の意味は「直行経路に乗せない＝従来どおり gate の判定に従う」である。gate が atomic で直行の条件を満たさない task（例: genre が docs の非 coding task、command check の無い task）は、これまでどおり計画なしの 1 run で走る（planner を新たに強制しない。挙動は 1 バイトも変えない）。gate が compound なら従来どおり planner run。

### D2. 経路の選択を event と Task.routing に記録する

- `Event::ExecutionRouted { decision: Box<RouteDecision> }` を `crates/task-core/src/model.rs` の `Event` に足す。状態は変えない（`replay` は無視する。`ExecutionGated` と同じ扱い）。
- `TaskRouting.route: Option<RouteDecision>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`。既存の JSON は変わらない）。
- いつ: `execution_gate_if_needed` が gate の判定を書いた直後、同じ dispatch で 1 回だけ。`routing.route` が `Some` の Task は評価し直さない（再試行・ReviewRepair・continuation でも同じ経路）。`Event::ExecutionRouted` と `Task.routing.route` は同じ `update_task` の transaction で書く。
- 人が後から経路を変える: `regate.rs`（decompose / retry の `execution`）は `routing.execution` を消すのと同時に `routing.route` も消す。次の dispatch で gate（`human/explicit`）→ evaluate（`direct/gate` が偽で `Planned`）の順に記録し直す。
- `gate = "off"` は gate の判定が無いので evaluate しない（記録なし・従来どおり）。`gate = "shadow"` は記録だけ（`shadow = true`）で、D3 の配線・D4 の prompt 節は効かせない。木の子は gate と同じく shadow でも採用する。

### D3. 直行時の流れと API の route 欄

直行（`route = Direct` かつ `shadow = false`）の流れ:

1. **1 implementation run**: `dispatch_run.rs` の `is_planner_dispatch` に `&& route != Direct` を足す（`replan_dispatch` は計画を持つ Task だけなので触らない）。WU も planner も作らず、既存の atomic run と同じ worktree・同じ予算（Task の `budget`）で 1 本走る。
2. **deterministic checks**: 既存の `spawn_review` が review 前同期（ADR-0118/0120。衝突は IntegrationRepair）→ Task の command checks を回す。新しい check 経路は作らない。
3. **final reviewer**: 同じ `spawn_review` の reviewer。ADR-0117 の決定的 check 結果を渡す。不合格は既存の ReviewRepair / `ReviewFail` の再試行（route は sticky なので再試行も直行）。

API: `task_ops::view::ExecutionView` に `route: Option<task_core::RouteDecision>`（skip if none）を足す。`TaskDetail.execution.route` として出す（gate と並ぶ。`execution.plan` が `None` の「直接実行」の行に理由を付ける）。`docs/api/v1/api-v1.schema.json` と型生成（`UPDATE_SCHEMA=1` の 3 箇所 + gui/web の `gen:types`）を更新する。GUI は task 詳細の Execution 節に「経路: 直行 / 分解（従来）」と `reasons`（`ok = false` の行を先に、`overrode_gate` なら「gate の compound/score を上書き」）を出す。

### D4. worker への指示（`crates/task-worker/src/claude_code/prompt.rs`）

- `RunContext` に `direct_route: Option<DirectRouteContext>`（`reasons` の要約だけ）を足し、dispatcher は D3 の直行 run にだけ入れる。
- `build_execute_prompt` の `## Acceptance criteria` の直後に、`context.direct_route` が `Some` のときだけ次の節を出す。`None` の run（従来の atomic・WU・planner・reviewer・対話）の prompt は 1 バイトも変えない（既存の snapshot 試験で固定）。

```
## 直行経路（planner なし）
この task は決定的な条件で分解不要と判定され、planner を挟まずこの 1 run で実装する。
- 調査 → 編集 → テスト → 局所修正を、この run の中で完結させる。
- 上の acceptance の command check を自分で実行し、落ちたら同じ run の中で直して再実行する。
- run が終わると celeris が同じ command check を決定的に再実行し、その後に最終レビューが入る。
- 範囲が想定より大きいと分かったら、無理に広げず result.json の summary にそう書く
  （分解は人が decompose で指示する。委譲の書式は従来どおり使える）。
```

- 他の harness（codex の AGENTS.md・acp の前置き）の prompt 組み立てが `build_execute_prompt` を共有しない場合も、同じ文面の節を `context.direct_route` で出す（文面は 1 箇所の関数に置く）。

### D5. 依存しないもの・触れないもの

- **Phase 3 session reuse に依存しない・触れない**: `preamble::continuation_section`・continuation の判定・session の再利用の差分には手を入れない。直行節は continuation 節と独立に出す（continuation があればその後ろに直行節が並ぶだけ）。
- **ReviewRepair**（`review_verdict.rs::try_review_repair`・RepairScope）: 直行 task は計画を持たないので、従来の atomic task と同じ扱い。変えない。
- **ADR-0043**（worktree・`[commands] check`・衝突の解消）: worktree の準備・後片付けは atomic run と同じ。変えない。
- **ADR-0079 の再帰統合**: 木の子の compound は上書きしない（D1）。木の子が直行でも、done 後の親ブランチへの統合（段階統合・merge candidate）は従来の atomic 子と同じ。
- **root delivery**（`crates/celeris/src/delivery.rs`・ADR-0121）と selfdeploy: 直行 root task の delivery は従来の atomic root と同じ。変えない。
- **remote worktree**: remote workspace の準備・同期（手元で編集してリモートで検証）は変えない。`direct/single-repo` の判定に使うだけ。
- dispatcher・store に LLM 呼び出しを入れない（CLAUDE.md 禁止事項）。

### D6. 実装の葉割りと試験名の規約

| 葉 | 範囲 | 主な内容 |
|---|---|---|
| core-route | `crates/task-core` | `direct_route.rs`（D1）、`Event::ExecutionRouted`、`TaskRouting.route`、`lib.rs` の re-export、task-core の schema 断片 |
| api-route | `crates/task-ops`・`crates/task-api`・`docs/api` | `ExecutionView.route`、`regate.rs` の route 消去、schema 再生成 |
| dispatch-route | `crates/task-dispatch`・`crates/task-worker` | `execution_gate_if_needed` 直後の evaluate と記録、`DirectRouteInputs` の計算、`is_planner_dispatch` の条件、`RunContext.direct_route`、prompt.rs の節 |
| gui-route | `gui/`（必要なら `web/`） | task 詳細 Execution 節の経路と理由の表示 |

試験名の規約:

- task-core の純関数の単体試験は名前に `direct_route` を含める（例 `direct_route_atomic_single_repo_is_direct`、`direct_route_human_explicit_compound_stays_planned`、`direct_route_tree_child_compound_not_overridden`）。
- dispatcher の回帰試験（`crates/task-dispatch/src/dispatcher/tests/`）は名前に `fast_path` と `direct_route` の両方を含める（例 `direct_route_fast_path_atomic_coding_task_skips_planner`〈planner run なしで 1 run → checks → reviewer を通る〉、`direct_route_fast_path_cross_repo_uses_planner`、`direct_route_fast_path_cross_department_uses_planner`、`direct_route_fast_path_human_approval_uses_planner`、`direct_route_fast_path_human_explicit_compound_uses_planner`）。
- prompt の試験は `direct_route_prompt_*`（節あり）と、`direct_route` が `None` の既存 snapshot が変わらないことを確かめる試験を置く。
- 試験は外部ネットワークに出ず、実 claude/systemd を叩かない（既存の fake adapter と in-memory store を使う）。

## 帰結

- スコアだけで compound になっていた具体的な実装 task が planner run なしで走り、計画・統合 WU の分だけ短くなる。分けるべき task の理由（cross-repo 等）が `reasons` として表に出る。
- 上書きは root の `compound/score` だけに限るので、人の明示・CoS のヒント・強制規則・木の子の判定は従来どおり。上書きが強すぎれば `direct/gate` の規則（D1）を絞るだけで戻せる（`policy_version` を上げる）。
- 直行 run が想定より大きかった場合の戻り道は人の decompose（D2）。自動の昇格（run の途中から planner に移る）は本 ADR では作らない。
