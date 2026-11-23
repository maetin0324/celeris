---
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
---
# ADR 2026-11-22: 親の途中評価 — Project Controller が strategy の root に checkpoint を導き、既存の reviewer の 1 run が親 Outcome への実測・知見・反証・不確実性を評価し、Rust が結果を既存の行動（continue・replan・escalate）に写す

- 日付: 2026-11-22
- 状態: Accepted
- 関係: 戦略層 v1 子 6（task 01M7XQ9QWK58ESMW42Q1TGT3WZ）。[ADR 2026-11-04-project-controller](2026-11-04-project-controller.md)（D6 進捗・D7 TS/Rust の境界・D9 不変条件・D12 流れ・A6 の `max_no_progress_decisions`）、
  子 2 の [ADR 2026-11-21-strategy-evidence-contract](2026-11-21-strategy-evidence-contract.md)（`celeris.strategy/1`・参照 `Reference`・承認）、
  子 5 の [ADR 2026-11-22-strategy-review-critic](2026-11-22-strategy-review-critic.md)（実験段階・Critic・「未確認と残る穴」）、
  子 0 の replay（`agent-docs/progress/2026-11-21-strategy-v1-0/replay/replay.md`、12 時点と answer key）、
  [ADR-0095](0095-worker-runs-see-the-db-read-only.md)（reviewer は DB を読み取り専用で読む）、`health_review::design_gate` の設計 review（task の status を変えずに reviewer run を起こす前例。D2.3 で共通の起動口に一般化する）。

## 背景

- アーキv4 の親は、子 A〜H がすべて done でも親の目標を満たさなかった。子 0 の replay で最初に未達を検知できた時点は E-mid（2026-11-18T20:36:35Z、子 A〜C が done）で、
  親が最初に評価したのは 2026-11-20T14:12:10Z（約 41.6 時間後）。その間に worker の run が 137 回走り、費用は 171.06 → 435.31 USD（list price）になった。
- 親の評価が無いのは、子の done（reviewer の局所の判定）と親 Outcome の到達（D6）の間に、途中で「親の目的に効いているか」を判定する口が無いからである。
  Project Controller は決定的な状態と予算だけを扱い、研究的な判断を持たない（D7）。子 5 は戦略の版と識別実験の入口を作ったが、走っている root の途中評価は無い。
- 依頼は、既存の reviewer の run を独立の Evaluator として使い、LLM を dispatcher・TS 決定器に入れず、新しい表・event の種類・migration・Manager・trait・状態機械・
  Agent サービスを足さず、strategy の無い低抽象度の task で reviewer run を増やさないことを求める。

## 決定

### D1: checkpoint の発火条件と、発火の状態の導き方（新しい正本を持たない）

#### D1.1 対象の root（strategy root）と、StrategyRef の型・照会を 1 つにする

対象は、案件の root（`task_core::is_root_task`、snapshot の `roots`）のうち、CoS の `task.create` で `strategy` を付けて起票されたものだけ（以下 strategy root）。
判定は `cos_operations` の行（`action = 'task.create'`・`state = 'applied'`・`payload_json.strategy` がある）の `target_id` が root の id であることで行う（子 2 D3.4 の「ある task がどの戦略を実行するか」の引き口）。

- 型: 戦略の文書の参照 `StrategyRef { task_id: String, name: String, sha256: String, candidate: Option<String> }`（`deny_unknown_fields`。`candidate` は省略可）を、task-core の新しい module
  `crates/task-core/src/strategy_ref.rs` に 1 つだけ置く。欄・serde の形・doc comment は子 5 の `task_api::cos::strategy_gate::StrategyRef` のまま移す。`task_id` は `String` のままにする
  （API の入力の id の誤りを、serde の拒否でなく gate の違反として記録する今の挙動を変えない）。この module は task-core の他の module を use しない（serde・schemars だけ。D11 の deps.cycle）。
- 照会: `cos_operations` の strategy 付きの `task.create` の照会は、task-core の crate 直下の新しい module `crates/task-core/src/strategy_reads.rs`（store の外。SELECT だけで書かない）の 1 つの自由関数
  `task_core::strategy_reads::strategy_task_creates(store: &SqliteStore, target: Option<TaskId>) -> Result<Vec<(String, StrategyRef)>, StoreError>` だけにする。
  読み取り専用の接続（`SqliteStore::with_read_conn`）を使う。要素は `(target_id, 参照)`、順は `created_at, id`。
  `None` は全件、`Some(id)` は `target_id = id` の行だけを返す。子 5 の gate は `None` で X(v) の元を読み（版ごとの選別は今までどおり呼び出し側）、controller の snapshot は `Some(root)` の最初の行を root の参照にする。
- 消すもの: `task_api::cos::strategy_gate` の `StrategyRef` の定義・`StrategyOperation`・`strategy_operations`（`write_tx` の中の生の SQL）と、`task_worker::protocol::ParentCheckpointStrategy`。
  gate は `pub(crate) use task_core::strategy_ref::StrategyRef;` で型を読み込み、`cos/operations.rs`（`super::strategy_gate::StrategyRef` を使う）は変えない。`ParentCheckpointRequest.strategy` は同じ型を使う（D3.1）。
- 照会の module は store（`crates/task-core/src/store/`）の外に置くので、高 risk の class `store` に当たらない。store の file は 1 行も変えない。
  置き場を変えた理由は末尾の付記（WU strategy-read）に書く。

strategy root でない root（人・CoS が strategy 無しで起票した root、controller が起票した root）は checkpoint を持たない。したがって低抽象度の task には何も起きない（D9.2）。

#### D1.2 trigger（4 種。すべて events と既存の行から毎回導く）

| trigger | key | 発火する条件 | 既定・設定 |
|---|---|---|---|
| 子の終端の数 | `children:<k>` | root の子（直下の子 task。`store.children(root)`）のうち終端（`done`・`failed`・`cancelled`）の数 c が `k = N × m`（m = 1, 2, …）に達した。c が一度に 2 段以上進んだら最大の k だけ | N = `[project_controller.checkpoint] children_per_checkpoint`、既定 2、1〜8 |
| 子の失敗 | `child_failed:<task id>` | root の子が `Transitioned{to: Failed}`（理由を問わない。controller の wake の `review_failed` と `root_failed` を含む）。`review_fail` で `ready` に戻る再試行は数えない（通常の局所の review で、親の反証ではない） | 無し |
| 反証の証拠 | `refute:<strategy_id>:v<n>:<evidence id>` | root の subtree（root と子孫）の task と、root が参照する文書の task（`strategy.task_id`）の `ArtifactProduced` のうち name が `strategy/<root の strategy_id>/v<n>.json`（n は root が参照した版より大きい）の文書に、root が参照した版に無い `stance: refute` の証拠がある。文書は `task_core::strategy` の型で読み、読めない文書は数えない | 無し |
| 予算の割合 | `budget:<p>` | root の `max_tokens`（task 木の token 予算。既存）に対する、root の subtree の全 run の使った token（既存の `dispatcher::token_budget::tree_tokens_used`。呼ぶだけで file は変えない）の割合が p % 以上 | p = `budget_thresholds_pct`、既定 `[50, 80]`、昇順・1〜99・4 件まで。`max_tokens` の無い root では発火しない |

- 設定の節 `[project_controller.checkpoint]` は `children_per_checkpoint`・`budget_thresholds_pct`・`max_evaluation_attempts`（既定 2、1〜3）の 3 欄だけを持ち、`deny_unknown_fields`。
  節が無いときは既定値。値の誤りは設定エラー。checkpoint を止める flag は置かない（止めるのは案件の controller の mode `off`。D9.2）。
- 新しい版の文書を別の task（CoS が新しく起票した文書の task）が作ったときは、反証の trigger は発火しない。その版は CoS が作ったもので、CoS は既に戦略を改めている（D5）。
  案件の全 task の `ArtifactProduced` の走査（本番で 1.65 秒、子 2 D3.4）を毎評価で行わないための範囲で、「未確認」に書く。

#### D1.3 checkpoint の状態（保存しない。1 つの純関数で導く）

状態は、案件の `evaluate` の判断の行（`id`・`triggers`・`effect_state`。既存の `project_controller_decisions` で読む）と、root の events の `ArtifactProduced` の `artifact.name` だけから、
純関数 `task_core::parent_checkpoint::checkpoint_states`（controller-checkpoint の WU が足す）で導く。時刻・run の数・記録の file の中身は入力にしない。新しい列・表・event は足さない。
判断ごとに、最終の記録の name `parent-evaluation/<判断 id>.json`（D3.4）と、試行の記録の name `parent-evaluation/<判断 id>.attempt-<n>.json`（n = 1, 2, …。D2.3）が root にあるかを見る。

| 状態 | 導き方 |
|---|---|
| 発火済み | `effect_state` が `pending`・`applied`・`suppressed` の判断の `triggers` の和。`suppressed` は shadow の案件の記録で、発火済みに数えて 2 回目を作らない |
| 実行中 | `effect_state` が `pending`・`applied` で、最終の記録の name が root に無い判断 |
| 評価済み | 最終の記録の name が root にある判断。その判断の `triggers` が評価済みの trigger |
| 試行の数 | 判断ごとの、試行の記録の name の数（同じ n は 1 回に数える） |
| due | D1.2 の条件が真で、発火済みに無い trigger |

- 上の 3 つ以外の `effect_state` の判断は発火済みに数えない（次の判断で再び due になる）。
- `parent_checkpoint::triggers` の入力 `TriggerFacts` の `requested`（発火済みのうち評価済みでない trigger）と `evaluated` は、この関数の出力だけから入れる。呼び出し側が別の方法で集めない。
- snapshot には strategy root ごとに due の trigger、実行中の判断の有無、評価済みのうちまだ行動に写していない評価（D5.3）とその許す行動（D5.2）を出す。
  snapshot の組み立て（`task_ops::project_snapshot`）は判断の表を root の `evaluate` について最後まで読むので、判断の履歴を snapshot に入れる件数（64）を超えた古い判断があっても、同じ trigger が 2 回発火しない。

### D2: ProjectAction の新しい種類 `evaluate`（Rust の検証と効果）

#### D2.1 形

```rust
ProjectAction::Evaluate { root_id: String, triggers: Vec<String> }   // kind = "evaluate"
```

TS は 1 回の判断で 1 つの root だけを評価に出し、その root の `due` の trigger をまとめて `triggers` に入れる（1〜8 件）。

#### D2.2 Rust の検証（`task_core::project_proposal::validate`。違反は `invalid_output` で起票も run も起こさない）

- `checkpoint_root_unknown`: `root_id` が snapshot の strategy root に無い。
- `checkpoint_not_due`: `triggers` が空・9 件以上、または D1.3 の due でない key を含む。
- `launch_on_paused`（既存の規則を `evaluate` にも掛ける）: 案件が `active` でない。
- `checkpoint_evaluation_open`: 同じ root に D1.3 の実行中の判断がある（1 root につき同時に 1 回）。`suppressed` の判断と評価済みの判断は見ない。

validate は snapshot の `checkpoints` の値（D1.3 の due と実行中、D5.2 の許す行動）だけを見て、`parent_checkpoint` の関数を呼ばない（D11 の deps.cycle）。

#### D2.3 効果（共通の reviewer run の起動口で 1 回起こす）

- 判断の計画（`project_decider_ts` の効果の組み立て）は `ProjectEffect::RequestEvaluation { root_id, triggers }` を作る。効果の実行（`project_wake` の効果の段）は
  既存の門（mode・phase・案件の状態。shadow では `suppressed`）を通った後、何も書かずに `applied`（`effect_ref` = root の id）にする。
- 共通の起動口（review-launcher の WU）: 設計 review の `start_design_review`・`run_design_reviewer`・`on_design_review_finished` を、`crates/task-dispatch/src/health_review/design_gate.rs`
  の中で 1 つの reviewer run の起動口に一般化し、設計 review と親の途中評価の両方が使う。
  - 起動 `start_side_review(task, section, kind) -> Result<Option<String>, DispatchError>`: `section` は `ReviewRequest` の節（設計 review は `design`、途中評価は `parent_checkpoint`）、
    `kind` は完了の種類（`SideReviewKind`）。今の設計 review の起動と同じ手順（`pick_reviewer`・`token_budget::run_watch`・`prompt_ancestors::lineage`・`WorkerStarted{role: reviewer}`・
    `run_index_start`・`supervisor.register_run(RunKey{task, work_unit: None})`）で run を起こし、起こした run の id を返す。task の dir が無い・reviewer を選べない（routing・admission・quota・予算）ときは `None`。
    `request.json` には `section` の節だけを入れ、`criteria` は空で、task の summary・evidence は渡さない。
  - 実行と終わり: run の実行と終わりの記録（`WorkerFinished`〈role reviewer〉・quota・runs 索引）は共通で、`review.json` の扱いだけを `kind` で分ける。
    設計 review は `design_verdict::settle_design_review`、途中評価（`SideReviewKind::ParentEvaluation { decision_id }`）は `task_dispatch::review::parent_evaluation` の決着（D3.4）。
    完了の channel は既存の `Completion::DesignReview` の variant とその end の struct に `kind` を持たせて使い、新しい channel は作らない。
  - 設計 review の挙動（起こす条件・`request.json`・prompt・判定・再試行）は一般化の前と同じ。
- 起動の条件（dispatcher の tick の段 `dispatcher/parent_evaluation.rs`、新しい file。controller-checkpoint の WU）: D1.3 の実行中の `evaluate` の判断ごとに、次の 3 つがすべて真のときだけ起動口を呼ぶ。
  1. `supervisor.has_task_run(root)` が偽（root の run を手元に持っていない。`register_run` は `RunKey{task: root, work_unit: None}` への insert なので、真のときに起こすと entry を上書きする）。
  2. `tree_limit_hold_for(root, NextRun::Worker)` が偽（木の run・費用の上限。超えていれば起こさず、既存の `kind: limit` の決定の要求に任せる。設計 review と同じ扱い）。
  3. D1.3 の試行の数が `max_evaluation_attempts` 未満。
- 試行の記録: 起動口が run の id を返したら、同じ tick で `parent-evaluation/<判断 id>.attempt-<n>.json`（n = 試行の数 + 1。中身は
  `{"schema": "celeris.parent-evaluation-attempt/1", "decision_id", "attempt": n, "run_id"}`）を root の成果物の dir に書き、既存の
  `ArtifactProduced{run_id, artifact{name, path, sha256（dispatcher が計算）, kind: "parent_evaluation_attempt", declared: false}}` を root に追記する。
  試行は起動の時点で数えるので、run が `WorkerFinished` を書かずに消えても（daemon の再起動など）試行に数える。起動口が `None` を返したときは数えず、次の tick に回す。
- 上限: 試行の数が `max_evaluation_attempts` に達し、最終の記録が無く、条件 1 が真でない（run を手元に持っていない）とき、dispatcher は D3.4 の `status: invalid` の最終の記録を書く（無限の再試行なし）。
- `abort_stale_runs`（`dispatcher/leases.rs`）の例外 `holds_design_review` は `holds_side_review(task, run_id)` に置き換え、leases.rs は呼び出しの 1 行だけ変える。
  `holds_side_review` が真になるのは、`run_id` が (a) 待っている設計 review の走っている run（今の判定）か、(b) root の events で、最終の記録の name が無い判断の最後の試行の記録の
  `ArtifactProduced.run_id` で、その run の `WorkerFinished` がまだ無いとき。どちらも root の events だけで決まる（評価の run も lease を持たない。root は子を待ったまま）。
- 新しい run の種類は作らない。task の status は変えない。run は既存の routing・admission・quota と root の token 予算（`run_watch`）に掛かる。

### D3: 入出力の wire と固定の rubric

#### D3.1 入力 `ReviewRequest.parent_checkpoint`

`ReviewRequest` に `parent_checkpoint: Option<Box<ParentCheckpointRequest>>`（`skip_serializing_if = "Option::is_none"`）を足す。`None` の review は `request.json` も prompt も base と 1 byte も変わらない
（`strategy_critic`・`diff_design` と同じ形）。欄を足すだけなので `PROTOCOL_VERSION` は上げない。`Some` のとき `criteria` は空で、reviewer は task の受け入れ条件を判定しない
（`ReviewVerdict` を書かない。final review と混ざらない）。

`ParentCheckpointRequest` の欄:

| 欄 | 中身 | 出どころ（決定的） |
|---|---|---|
| `decision_id`・`root_id`・`triggers[]` | 判断の id と D1.2 の key | 判断の行 |
| `goal` | `{request_sha256, outcomes: [{id, title, acceptance[], reached}]}`。`reached` は Rust の D6（`recorded_outcomes`・`root_satisfies`）の値 | snapshot |
| `strategy` | root の `task_core::strategy_ref::StrategyRef`（`task_id`・`name`・`sha256`・`candidate`。D1.1 の型をそのまま使う） | D1.1 の `strategy_task_creates(store, Some(root))` |
| `children[]` | `{task_id, title, status, reviewer_passed, criteria: [{text, kind, result}]}` | snapshot の root の行と子の events |
| `metrics` | `{children_terminal, children_total, tokens_used, max_tokens}` | D1.2 と同じ値 |
| `previous[]` | 同じ root の前の評価の成果物 `{name, sha256}` | root の events |
| `rubric[]` | 固定の E1〜E6（`task_core::parent_checkpoint::PARENT_EVALUATION_RUBRIC` の定数。文書や run が変えられない） | 定数 |

Project.request の全文は既存の `context.lineage.project`（子 2 D5）で届く。worker・CoS の報告の文は入力の欄にしない（controller の I11）。reviewer は ADR-0095 の読み取り専用の DB と
workspace の成果物で証拠を読む。子 0 の answer key は入力にも prompt にも入らない（D8.3）。

rubric（固定）:
- E1 実測: 親 Outcome の指標について、子の成果の前後の値を `task`・`artifact`・`commit` の参照つきで挙げる。参照の無い改善は書かない。
- E2 知見と反証: どの仮説が支持され、どれが反証されたかを参照つきで挙げる。子の done を親の進捗と書かない。
- E3 成功の根拠: done の子が Outcome の acceptance のどれにどう効いたか。効いていなければそう書く。
- E4 残る不確実性: 分からないことと、それを確かめる最小の実験か判断。
- E5 次の手: `continue`・`discriminating_experiment`・`pivot`・`ask_human` から 1 つ。実験と pivot は戦略の新しい版（人の承認）でしか行えないことを前提に選ぶ。
- E6 自己申告の禁止: worker・CoS の報告の文を証拠に数えない。Outcome の到達は `goal.outcomes[].reached` だけが決める。

#### D3.2 出力 `ReviewOutput.parent_evaluation`（schema `celeris.parent-evaluation/1`）

```jsonc
{
  "schema": "celeris.parent-evaluation/1",
  "verdict": "on_track | at_risk | off_track | unknown",
  "next": "continue | discriminating_experiment | pivot | ask_human",
  "axes": {"execution": "succeeded | partial | failed | unknown",
           "epistemic": "advanced | none | unknown",
           "goal": "met | not_met | unknown"},
  "measured":    [{"claim": "…", "metric": "…", "before": "…", "after": "…", "references": [Reference]}],
  "learned":     [{"statement": "…", "references": [Reference]}],
  "refuted":     [{"hypothesis": "…", "statement": "…", "references": [Reference]}],
  "uncertainty": [{"statement": "…", "resolve_by": "…"}],
  "reason": "…"
}
```

`Reference` は子 2 の `task_core::strategy::Reference`（6 種）をそのまま使う（同じ意味の型を二重に作らない）。上限: 各列 0〜16 件、文 2,000 文字、`metric`・`before`・`after` 200 文字、
1 件の参照 0〜16 件（0 件は形として受け、D4 で unsupported に数える）。`ReviewOutput` の欄は省略で読める（古い `review.json` を読める）。

#### D3.3 prompt

`task_worker::change_rules::review` に `parent_checkpoint_section` を足す。`None` なら空文字列で prompt は変わらない。`Some` なら目的の節・入力・E1〜E6・出力の形（`final_example` に `parent_evaluation` を加える）を出す。
reviewer の既存の規則（読み取り専用、subagent を起こさない）は変えない。

#### D3.4 結果の写し（既存の `ArtifactProduced`）

evaluation run の終わり（D2.3 の起動口の `SideReviewKind::ParentEvaluation { decision_id }`）で、dispatcher（`task_dispatch::review::parent_evaluation`、新しい module）が `review.json` を読み、D4 の `check_evaluation` に通す。
結果が `checked`、または `invalid` で D1.3 の試行の数が `max_evaluation_attempts` に達しているときだけ、次の最終の記録を root の成果物の dir に `parent-evaluation/<判断 id>.json` として書き、
既存の `ArtifactProduced{run_id, artifact{name, path, sha256（dispatcher が計算）, kind: "parent_evaluation", declared: false}}` を root に追記する。
`invalid` で上限に達していなければ最終の記録を書かない（判断は実行中のままで、次の tick が次の試行を起こす。D2.3）。

```jsonc
{"schema": "celeris.parent-evaluation-record/1", "decision_id": "…", "root_id": "…",
 "status": "checked | invalid", "evaluation": {/* D3.2 のまま */},
 "effective": {"verdict": "…", "next": "…", "axes": {…}, "unsupported": 0, "negative_result": false},
 "violations": ["…"]}
```

最終の記録は trigger の一覧を持たない。どの trigger の評価かは判断の行（`triggers`）が正本で、記録は `decision_id` でその行を指す（D1.3）。
`task_core::parent_checkpoint::ParentEvaluationRecord` の欄 `triggers` は controller-checkpoint の WU が消す。
LLM は呼ばない。形が崩れた・節が無い出力も試行に数え（試行は起動の時点で数えている）、上限に達したら `status: invalid`・`effective.verdict: unknown`・`effective.next: ask_human` で書く。

### D4: 決定的な規則（`task_core::parent_checkpoint::check_evaluation`、純関数）

入力は D3.2 の出力と、dispatcher が集めた事実 `EvaluationFacts { resolved: 参照ごとの解決, goal_reached: bool, children: 終端・合格の数 }`。参照の解決は子 2 の gate と同じ意味
（`task` は同じ案件の task がある、`artifact` は同じ task に同じ sha256 の `ArtifactProduced` がある、`commit` は形と案件のリポジトリ）で、dispatcher が既存の `SqliteStore::get` と `events_for` で引く。

- R1 形と上限（D3.2）。違反は `invalid`。
- R2 `measured[]`・`learned[]`・`refuted[]` の 1 件は、解決した `task`・`artifact`・`commit` の参照が 1 件以上あるときだけ supported。それ以外（参照 0 件・解決しない・`url`・`knowledge`・`chat` だけ）は `unsupported` に 1 を足す。
  外部の文（`url` など）は untrusted で、支持に数えない（子 2 D2.3 の `confirmed` と同じ考え）。
- R3 `verdict = on_track` は、supported の `measured` が 1 件以上で `unsupported = 0` のときだけ有効。それ以外は `effective.verdict = unknown`。**参照の無い改善の主張がある評価は on_track にならない。**
- R4 `axes.goal` は Rust の D6 が決める: `goal_reached` が真なら `met`、偽で評価が `met` と書いたら `unknown` にして `unsupported` に 1 を足す。`not_met`・`unknown` はそのまま。
  **Outcome の到達（`reached`・`achieved`）は評価から書かない。** 到達の記録は既存の `mark_milestone`（D6 の根拠の確かめ直し）だけが書く。
- R5 `axes.execution` は Rust の事実で決める: 終端の子がすべて `done` かつ合格なら `succeeded`、`failed` の子があれば `failed`、それ以外は `partial`（子が 0 件なら `unknown`）。評価の値と違えば違反に記録する（判定は事実の側）。
- R6 `axes.epistemic = advanced` は supported の `learned`・`refuted` が 1 件以上のときだけ。それ以外は `unknown`。
- R7 `effective.next` は評価の `next`。ただし有効な verdict と組み合わせて D5.2 の表に行が無いとき（`off_track`・`unknown` で `continue`）は `ask_human`。

### D5: next を既存の行動に写す

#### D5.1 TS の規則 `pc/checkpoint`（`control-ts/src/domains/project_controller.ts`）

1. `due` の trigger がある strategy root があれば（既存の escalate・待ちの規則より後、launch_root より前）、最も古い trigger の root を 1 つ選び `evaluate` を返す（rule `pc/checkpoint/evaluate`）。
2. まだ行動に写していない評価（D5.3）があれば、下の表で行動を返す（rule `pc/checkpoint/act`、signal `checkpoint` = 判断 id）。
   置き場は paused（`pc/project-not-active`）と `pc/no-progress` の直後で、他の規則より前。replan の行は CoS 入力の日の上限に当たれば wait（`pc/checkpoint/act/daily-limit`）。理由は末尾の付記（WU ts-checkpoint-rule）。

#### D5.2 写しの表（`task_core::parent_checkpoint::allowed_actions`、Rust が検証する）

| 有効な verdict | 有効な next | 許す行動 | 効果 |
|---|---|---|---|
| `on_track`・`at_risk` | `continue` | `continue`・`wait` | なし（既存） |
| 任意 | `discriminating_experiment` | `replan{reason_code: "checkpoint_experiment"}`・`escalate` | 案件 thread に `role=system` の入力（既存）。CoS が子 5 の gate を通る新しい版（`selected` 無し、識別実験の候補）を作る |
| 任意 | `pivot` | `replan{reason_code: "checkpoint_pivot"}`・`escalate` | 同上。CoS が反証を `rejected_by` に引いて候補を却下し、別の候補の版を作る（子 5 D4.1） |
| 任意 | `ask_human` | `escalate`（`question_key = checkpoint:<判断 id>`） | 受信箱の `project_controller` に 1 件（既存） |
| `invalid` の記録 | — | `escalate` | 同上 |

Rust の検証 `checkpoint_action_mismatch`: 写していない評価があるのに、rule が `pc/checkpoint/act` でない、または表に無い行動を返した proposal は `invalid_output`。
`replan` の `context_refs` は `task:<root>`・`artifact:<root>/parent-evaluation/<判断 id>.json`・戦略の文書の参照だけ（id だけ。評価の文を CoS 入力の本文に写さない。I11）。

#### D5.3 「行動に写した」の導き方

評価済み（D1.3）の判断 e は、判断の表に rule が `pc/checkpoint/act` かつ signal `checkpoint` = e の判断 id で、`effect_state` が `pending`・`applied`・`suppressed` の判断があれば写し済み。
signal が判断 id を持つので、時刻は比べない。保存しない。

#### D5.4 人の承認と変えないもの

- 戦略の変更（実験・pivot）は CoS が新しい版の文書の task を起票し、子 2 D2.2 の人の承認（`strategy-approve-v<n>`、`human:` の回答だけ）と子 5 の Critic を通ってから実行の入口に入る。
  controller も評価も、承認なしに戦略を変えられない。大きな戦略変更・高額な実験・外部公開は人の判断のまま。
- Project.request・承認済み Roadmap・Outcome の acceptance・root の objective と acceptance を書かない。評価が書くのは root の成果物 1 件と、既存の判断の行だけ。

### D6: 評価は進捗に数えず、既存の guard で止まる

- `evaluate` の判断と評価の成果物は `progress_epoch` を増やさない（D6 の「進捗に数えないもの」に評価を足す）。
- `runaway_guard` の数える種類に `evaluate` を足す（`replan`・`launch_root` と同じ扱い）。したがって `evaluate → replan → evaluate → …` が進捗なしで `max_no_progress_decisions`（既定 5）続けば、
  既存どおり `escalate`（`no_progress:<epoch>`）で止まり phase は `blocked` になる。新しい guard・上限は足さない。
- 評価の回数は trigger の数で有界である（子の数 / N、子の失敗の数、反証の証拠の数、予算の閾値 2 つ）。1 つの評価の reviewer run は `max_evaluation_attempts` まで。

### D7: 負の研究結果の表し方

軸を分けて書く。負の研究結果は `effective.axes = {execution: succeeded, epistemic: advanced, goal: not_met}` で、`refuted[]` に supported の項目が 1 件以上あるもの。
記録の `effective.negative_result` を真にする（R5・R6 の値から導く。LLM の申告ではない）。verdict は `off_track`、next は `pivot` か `ask_human` になり、root の task の status（done など）は変えない。
CoS は次の版で反証を `stance: refute` の証拠として残す（子 2 D2.3 で消せない）。子 7 の画面は 3 軸を分けて出す（「実行は成功・知見は前進・当初の目標は未達」）。

### D8: 子 0 の replay の時点での発火と answer key との比べ方

#### D8.1 発火する時点（N = 2、アーキv4 の親を strategy root とみなした場合）

アーキv4 の子は A〜H の 8 件。replay の events で子の終端は A 18:45:41・B 21:55:57（11-17）・C 10:48:28・D 20:39:25・E 22:54:25（11-18）・F 04:54:54・G 10:41:53（11-19）、
`failed` の子は無い（`review_fail` で `ready` に戻った A・C は D1.2 で数えない）。アーキv4 の親に `max_tokens` も戦略の文書も無いので、予算と反証の trigger は replay では発火しない（D8.4）。

| 順 | 時点 | 終端の子 | checkpoint | 評価に渡す slice | answer key の理想 |
|---|---|---|---|---|---|
| 1 | A-mid | 0 | 無し | — | unknown・continue |
| 2 | A-done | 1 | 無し | — | at_risk・discriminating_experiment |
| 3 | C-mid | 2 | **`children:2`**（B の done、2026-11-17T21:55:57Z で発火） | C-mid（終端の子が A・B で発火時と同じ） | at_risk・continue |
| 4 | C-done | 3 | 無し | — | at_risk・continue |
| 5 | D-mid | 3 | 無し | — | at_risk・continue |
| 6 | E-mid | 3 | 無し（最初に検知できた時点。live の sample だけがここで評価する。D8.3） | E-mid | off_track・discriminating_experiment |
| 7 | D-done | 4 | **`children:4`**（D の done、2026-11-18T20:39:25Z で発火） | D-done | off_track・pivot |
| 8 | E-done | 5 | 無し | — | off_track・pivot |
| 9 | F-mid | 5 | 無し | — | off_track・ask_human |
| 10 | F-done | 6 | **`children:6`**（F の done、2026-11-19T04:54:54Z。ただし D-done の pivot が承認されれば F は起票されない） | F-done | off_track・ask_human |
| 11 | G-mid | 6 | 無し | — | off_track・ask_human |
| 12 | G-done | 7 | 無し | — | off_track・ask_human |

最初の off_track の checkpoint は D-done（20:39:25Z）で、検知できた E-mid の 2 分 50 秒後、D#1446（19:55:39Z）の約 44 分後である。史実の親の最初の評価（2026-11-20T14:12:10Z）より約 41.5 時間早い。
N = 3 なら C-done（3）と F-done（6）で発火し、最初の off_track の checkpoint は F-done になる（既定を 2 にする理由）。

#### D8.2 比べ方（second-gate-shadow）

- 試験は子 0 の `replay.py build` の slice を写した fixture（各 file の sha256 が `replay/manifest.sha256` と一致すること、event の ts が cutoff 以下であることを最初に確かめる）から、
  時点ごとに snapshot の checkpoint を計算し、上の表の発火・不発火と一致することを確かめる（`parent_checkpoint_shadow_fires_at_c_mid_d_done_f_done`）。
- 発火した時点では、評価器の出力を固定の fixture（スクリプトの評価。LLM ではない）で与え、`check_evaluation` → TS の写し → Rust の検証 → 効果（shadow では `suppressed`、enabled では CoS 入力）が
  D5.2 の表どおりになること、D-done の `pivot` から CoS の新しい版（固定計画の候補を `rejected_by` で却下）が子 5 の gate を通ること、却下した候補に戻る版が `strategy_revisit_rejected` になることを確かめる。
- answer key（`answer_key.json`）は試験の比較の側だけが読む。評価器の入力（`ParentCheckpointRequest` と slice）に answer key の印（`ideal_verdict`・`ideal_action`・`answer_key`）が無いことを
  試験が確かめる（子 0 の否定対照 N3 と同じ考え）。比べる値は `effective.verdict` と `effective.next` の完全一致で、一致しない時点を表に残す（合否は人が rubric で採点する）。

#### D8.3 live の sample（live-evaluator-sample）

実際の LLM の 1 run が E-mid の slice で評価を書く。worker は LLM の CLI を起こせないので、WU の run 自身が評価器になる: 試験 `parent_checkpoint_live_request_renders` が E-mid の fixture から
`ParentCheckpointRequest` と prompt を作って file に出し、WU の run はその prompt と slice の file だけを読んで `parent_evaluation` を書く。WU の objective は `answer_key.json` と
`replay/` の answer の file を開かないことを求め、review は run の道具の記録（`worker_progress` の tool の記録）に answer key の path が無いことを読み取り専用の DB で確かめる。
`parent_checkpoint_live_sample_checks` が sample を `check_evaluation` に通し、answer key（E-mid の off_track・discriminating_experiment）との一致・不一致を出力する。
同じ計画の WU が書くので独立の度合いは本番の reviewer run より低い（「未確認」）。

#### D8.4 replay で試せない trigger

予算・反証・子の失敗の trigger は合成の fixture（`parent_checkpoint_shadow_budget_50_80`・`…_refute_in_subtree`・`…_child_failed`）で試す。

### D9: 回帰（従来の低抽象度 task で reviewer run を増やさない）

#### D9.1 増えない条件

- strategy root でない root と、その子・WU の review は変わらない（checkpoint が無いので `evaluate` が出ない）。
- controller の state の行が無い案件と mode `off` の案件では判断そのものが無い（controller の I13）。mode `shadow` では `evaluate` の効果は `suppressed` で reviewer run を起こさない。
- `parent_checkpoint` の無い review の `request.json`・prompt・`ReviewVerdict` は base と同じ。

#### D9.2 確かめる試験

| 試験 | 確かめること |
|---|---|
| `parent_checkpoint_regression_no_strategy_root_no_reviewer_run`（task-api） | controller が enabled の案件で strategy 無しの root の子が 4 件 done になっても、`evaluate` の判断が 0 件、root の reviewer run が 0 件、snapshot の JSON に `checkpoints` の key が無い |
| `parent_checkpoint_regression_controller_off_no_reviewer_run`（task-api） | strategy root がある案件で controller の行が無い・`off` のとき、判断 0 件・reviewer run 0 件 |
| `parent_checkpoint_regression_shadow_no_reviewer_run`（task-api） | mode `shadow` で `evaluate` は `suppressed` で記録され、reviewer run は 0 件。同じ trigger の 2 回目の判断が無い |
| `parent_checkpoint_regression_final_review_unchanged`（task-api） | strategy root の子の final review の `request.json` に `parent_checkpoint` の key が無く、reviewer run の数が base と同じ |
| `project_controller_checkpoint_snapshot_without_strategy_roots_unchanged`（task-dispatch） | strategy root の無い案件の snapshot の serialize が base の形と同じ（`checkpoints` は空なら出さない） |
| `parent_evaluation_absent_request_json_unchanged`・`parent_evaluation_absent_prompt_unchanged`・`parent_evaluation_absent_old_review_json_parses`（task-worker・task-dispatch） | 節の無い review の wire と prompt が base と同じ、古い `review.json` が読める |

### D10: 子 5 の穴（選ぶ版を結果より先に書ける）を採るか

**子 6 では採らない（見送る）。** 採否は親 01M7XQ9PS0AFKKMRJ7P5ZMKQQE の closeout で人が決める（進捗の提案に書く）。理由:

1. 閉じる場所は子 5 の gate の検査（`task_api::cos::strategy_gate` と `task_core::strategy::review::check_execution`）で、この計画の WU はその検査を変えない
   （strategy-read の WU は gate の照会を D1.1 の関数に置き換えるだけで、X(v) の意味・違反の code・422 を変えない）。checkpoint の側で
   決定的に検知するには、gate の文書の鎖と X(v) の読み出しを dispatcher に写す必要があり、規約 2（二重実装）に当たる。
2. 子 6 は穴の結果を途中で見つける経路を足す: pivot・実験の版で起票した root も strategy root なので、子の終端・反証で checkpoint が起き、評価の R2・R3 は参照の無い選択の根拠を
   unsupported に数え on_track にしない。ただしこれは検知で、先に書いた版の実行を止めない。
3. 閉じる案（版の `ArtifactProduced` の時刻と前の版の実験 task の終端の時刻の比較）は gate が読む events の範囲を広げ、結果を読み直す版を要求する。後戻りの費用は低いが、
   子 5 の gate の契約を変えるので、子 5 の改訂として人が採否を決めるのが筋である。

### D11: WorkUnit ごとの write set・試験の prefix・高 risk の class・deps.cycle

| WorkUnit | 書く場所（これ以外は書かない） | 試験の prefix |
|---|---|---|
| checkpoint-design | この ADR、`agent-docs/progress/2026-11-22-strategy-v1-6/checkpoint-design.md` | — |
| checkpoint-core | `crates/task-core/src/parent_checkpoint.rs`（`triggers`・`check_evaluation`・`allowed_actions`・`PARENT_EVALUATION_RUBRIC`・型）と `crates/task-core/src/parent_checkpoint/`（試験）、`crates/task-core/src/lib.rs`（`mod` の 1 行）、WU の進捗 | `parent_checkpoint_contract_` |
| evaluator-wire | `crates/task-worker/src/protocol.rs`・`crates/task-worker/src/change_rules/review.rs`（と試験）、`ReviewRequest {`・`ReviewOutput {` の literal を持つ試験 file（欄 `None` を足すだけ）、`docs/protocol/worker-protocol.md`（context.review の表と §10.2）・`docs/protocol/worker-protocol.schema.json`、WU の進捗 | `parent_evaluation_wire_`・`parent_evaluation_absent_` |
| adr-revise | この ADR、`agent-docs/progress/2026-11-22-strategy-v1-6/adr-revise.md` | — |
| strategy-read | `crates/task-core/src/strategy_ref.rs`（新）と `crates/task-core/src/lib.rs` の `mod`・`pub use` の行、`crates/task-core/src/strategy_reads.rs`（新、SELECT だけ）と試験 `crates/task-core/src/strategy_reads_tests.rs`（新）、`crates/task-api/src/cos/strategy_gate.rs`（型の定義・`StrategyOperation`・`strategy_operations` を消し、`pub(crate) use` と D1.1 の関数を使う）、`crates/task-worker/src/protocol.rs`・`crates/task-worker/src/protocol/parent_checkpoint.rs` と試験（`ParentCheckpointStrategy` を消す）、`docs/protocol/worker-protocol.md`・`docs/protocol/worker-protocol.schema.json`、型の移動で生成物が変わるときだけ `docs/api/v1/api-v1.schema.json`・`web/api/generated/`（再生成）、WU の進捗 | `strategy_reads_`。既存の子 5 の gate の試験（task-api）が変更なしで通る。`parent_evaluation_wire_` は型の名前の置き換えだけを直して通る |
| review-launcher | `crates/task-dispatch/src/health_review/design_gate.rs`（共通の起動口・`SideReviewKind`・`holds_side_review` の設計 review の半分）、`crates/task-dispatch/src/dispatcher/leases.rs`（呼び出しの 1 行）、`crates/task-dispatch/src/dispatcher.rs`（`Completion::DesignReview` の variant と受け口の行だけ）、`crates/task-dispatch/src/health_review/` の試験、WU の進捗 | `side_review_`。既存の設計 review の試験（`health_review/` の scenario 試験を含む）が変更なしで通る |
| controller-checkpoint | `crates/task-core/src/parent_checkpoint.rs`（`checkpoint_states`・試行の記録の name の関数・`ParentEvaluationRecord.triggers` の削除）と試験、`crates/task-core/src/project_proposal.rs`（`Evaluate`・snapshot の `checkpoints`・検証）、`crates/task-ops/src/project_snapshot.rs`、`crates/task-dispatch/src/project_controller.rs`（`RequestEvaluation`）、`crates/task-dispatch/src/dispatcher/{project_decider_ts,project_wake,parent_evaluation}.rs`（`parent_evaluation.rs` は新）と `dispatcher.rs` の `mod`・tick の 1 行、`crates/task-dispatch/src/health_review/design_gate.rs`（`SideReviewKind::ParentEvaluation` の variant・`parent_checkpoint` の節・終わりの腕・`holds_side_review` の評価の半分だけ）、`crates/task-dispatch/src/review/parent_evaluation.rs`（新）と `review.rs` の `mod` の 1 行、`crates/task-dispatch/src/dispatcher/tests/project_checkpoint.rs`（新）と `tests` の `mod` の 1 行、`crates/celeris/src/config/project_controller.rs`、`docs/ops/project-controller.md`、WU の進捗 | `project_controller_checkpoint_`・`parent_checkpoint_contract_` |
| ts-checkpoint-rule | `control-ts/src/domains/project_controller.ts`・`project_controller.test.ts`、WU の進捗 | `project_controller_checkpoint_`（TS の試験名） |
| cos-skill-checkpoint | `config/skills/cos-operator/strategy-review.md`（checkpoint の replan を受けたときの手順の節）・`SKILL.md`（metadata.version を上げる）、WU の進捗 | 既存の `cos_operator_skill_` と `cargo nextest run -p celerisctl` |
| second-gate-shadow | `crates/task-api/tests/parent_checkpoint_shadow.rs`・`parent_checkpoint_regression.rs`・`tests/fixtures/parent_checkpoint_shadow/`、WU の進捗 | `parent_checkpoint_shadow_`・`parent_checkpoint_regression_` |
| live-evaluator-sample | `crates/task-api/tests/parent_checkpoint_live.rs`・`tests/fixtures/parent_checkpoint_live/`、WU の進捗 | `parent_checkpoint_live_` |
| close-gate | `agent-docs/progress/2026-11-22-strategy-v1-6.md` | — |

- WU の進捗は `agent-docs/progress/2026-11-22-strategy-v1-6/<key>.md`。各 WU はこの ADR に実装時の付記を足してよい。
- 段の順: strategy-read と review-launcher は adr-revise の後に並び（write set は重ならない）、controller-checkpoint は両方の後。controller-checkpoint は起動口と `strategy_task_creates` を呼ぶだけで、写しを作らない。
- 高 risk の class（計画の設計 brief にこの ADR を挙げる）:
  - strategy-read: 照会の module（`crates/task-core/src/strategy_reads.rs`）は store の外なので class `store` に当たらない。store の file は書かない。`crates/task-api/src/cos/strategy_gate.rs` は class の正規表現に当たらないが、
    class `authz` の `cos/operations.rs` が呼ぶ gate なので、この計画では高 risk と同じに扱う（検査の意味を変えないことを既存の gate の試験で確かめる）。
  - review-launcher: `crates/task-dispatch/src/health_review/design_gate.rs`（`review-gate`）、`crates/task-dispatch/src/dispatcher/leases.rs`（`lease`。呼び出しの 1 行）。
  - controller-checkpoint: `crates/task-dispatch/src/health_review/design_gate.rs`（`review-gate`。variant・節・腕・`holds_side_review` の評価の半分だけ）。
  - どの WU も書かない: `crates/task-dispatch/src/dispatcher/token_budget.rs`（`budget`）、`crates/task-dispatch/src/run_supervisor.rs`（`lease`。`has_task_run` は呼ぶだけ）、
    `crates/task-api/src/cos/operations.rs`（`authz`）、`crates/task-core/src/model/events.rs`（`events`）。`tree_limit_hold_for`（`dispatcher/tree_units.rs`）も呼ぶだけ。
- deps.cycle（repo health の単位は Rust では論理 module の第 1 要素 `<pkg>/src/<第 1 要素>`）:
  - 起動口の置き場は `design_gate.rs` の中の一般化に決める。`design_gate.rs` は `dispatcher.rs` の `#[path]` で読む `dispatcher` の子で、起動口は dispatcher の private な `running`・`Completion` を使う。
    新しい file に分けても `dispatcher` の子として読むしかなく、単位は同じ `task-dispatch/src/dispatcher` で、file と `mod` の行が増えるだけである。`dispatcher/parent_evaluation.rs` からの呼び出しも同じ単位の中に閉じ、新しい組は無い。
    終わりの腕が呼ぶ `task_dispatch::review::parent_evaluation`（単位 `review`）への組は、既存の `dispatcher` → `review` と同じ向きである。
  - `strategy_ref.rs` は task-core の他の module を use しない葉なので、`strategy_reads` → `strategy_ref`、task-api・task-worker → `task_core::strategy_ref` の組は循環の外から一方向に入る。
    `strategy_reads` は `store`・`model`・`strategy_ref` に一方向に依存し、task-core の中でこれを use する module は無い（循環に入らない）。
  - `parent_checkpoint` は既に `strategy`（task-core の既存の循環の中）に依存する。`project_proposal`（循環の中）から `parent_checkpoint` への組を足すと `parent_checkpoint` が循環に入るので、
    `project_proposal::validate` は `parent_checkpoint` を呼ばず、snapshot の組み立て（task-ops）が `checkpoint_states`・`triggers`・`allowed_actions` の結果を snapshot の `checkpoints` に入れる（D2.2）。
    `checkpoint_states` は `project_wake`・`project_proposal` の型を use しない（`effect_state` は呼び出し側が `parent_checkpoint` の入力の形に写す）。
  - strategy-read・review-launcher・controller-checkpoint の final review の health の材料で `deps.cycle` が新しい組を出さないことを確かめる。

## 足さないもの

新しい DB 表・migration・event の種類・Agent サービス・Manager・trait・状態機械は足さない。足すのは、`ProjectAction::Evaluate` と `ProjectEffect::RequestEvaluation` の variant 各 1、
snapshot の欄 `checkpoints`、`ReviewRequest.parent_checkpoint` と `ReviewOutput.parent_evaluation` の任意の節 2 つ、設定の節 `[project_controller.checkpoint]`（3 欄）、
読み取り専用の照会の自由関数 1 つ（`task_core::strategy_reads`。gate の生の SQL を置き換える）、純関数（`triggers`・`checkpoint_states`・`check_evaluation`・`allowed_actions`）、検証の code 4 つ（`checkpoint_root_unknown`・`checkpoint_not_due`・
`checkpoint_evaluation_open`・`checkpoint_action_mismatch`）、replan の `reason_code` 2 つ、成果物の name の形 2 つ（最終の記録と試行の記録）である。`StrategyRef` は task-api から task-core に移すだけで、型の数は 2 から 1 に減る。
reviewer run の起動口は設計 review の起動を一般化したもので、起動の写しは作らない。発火済み・実行中・評価済み・試行の数・写し済みは保存せず、判断の表と root の成果物の name から毎回導く。
Evaluator は既存の reviewer run で、dispatcher は出力の形と参照を確かめるだけで LLM を呼ばない。TS は判定の選択だけで LLM を呼ばない。Project.request と承認済み Roadmap は読むだけ。

## 実装前 brief

1. 根本原因: 子の done（局所の review）と親 Outcome の到達（D6）の間に、途中で親の目的への効き目を独立に判定する口が無く、controller は計画の最後の root の終端まで未達に気づかない。
   症状はアーキv4 の固定計画の完走（検知できた時点の後に 137 run・+264.25 USD）に出た。
2. 責務 owner: 発火の条件・状態の導出・評価の決定的な規則は `task_core::parent_checkpoint`（純関数）。戦略の参照の型は `task_core::strategy_ref`、その照会は `task_core::strategy_reads`（crate 直下。store の外）。
   発火の事実の収集と行動の検証は Project Controller（`task_ops::project_snapshot`・`task_core::project_proposal::validate`・`task_dispatch::dispatcher::project_decider_ts`）。
   評価の run の起動と結果の写しは dispatcher の共通の reviewer run の起動口（`health_review::design_gate`）と `review::parent_evaluation`。写しの方針は TS。新しい版は CoS（skill）。
3. 既存の再利用先: controller の wake → snapshot → TS → Rust の検証 → 効果、`runaway_guard`、設計 review の reviewer run の起動・実行・終わり（D2.3 で一般化して両方が使う）、
   子 5 の gate の `cos_operations` の照会（D1.1 で task-core の `strategy_reads` に移して共有する）、`SqliteStore::with_read_conn`、`supervisor.has_task_run`、`tree_limit_hold_for`、子 2 の `Reference` と承認、子 5 の gate と Critic、
   `ArtifactProduced`、判断の表、`token_budget::tree_tokens_used`、ADR-0095 の読み取り専用の DB。
4. 選択肢: (a) 評価を新しい task（評価 task）として起票する → task の数が増え、低抽象度の経路と混ざるので採らない。(b) 評価の結果を新しい event か表に保存する → 依頼が禁じるので採らない。
   (c) 子の終端ごとに毎回評価する → 費用が子の数に比例し、局所の review と重なるので採らない（N で間引く）。(d) TS か dispatcher が評価の文を判定する → LLM 抜きでは研究的な判断ができず、
   LLM を入れることは禁止。(e) final review の暗黙の条件にする（子 5 の Critic の形）→ root が done になる前の途中評価にならない。(f) 評価の run の起動を設計 review の起動の写しで作る →
   規約 2 に当たるので採らない。(g) 起動口を新しい file に置く → 単位も依存も同じで file と `mod` の行が増えるだけなので採らない（D11）。
   採用: controller の新しい行動 `evaluate` と、設計 review と共通の reviewer run の起動口。結果は成果物。
5. 新しい状態・分岐・fallback・port: 状態は足さない（D1.3 で導く）。分岐は `ProjectAction` の腕 1 つ、`ProjectEffect` の腕 1 つ、dispatcher の tick の段 1 つ、review の節 1 つ、
   起動口の完了の種類 2 つ（設計 review・途中評価）。fallback と新旧切替の flag は無い。
6. 撤去対象: `task_api::cos::strategy_gate` の `StrategyRef` の定義・`StrategyOperation`・`strategy_operations`（生の SQL）、`task_worker::protocol::ParentCheckpointStrategy`（と schema・md の記述）、
   設計 review だけの起動・実行・終わりの関数（共通の起動口に置き換え）、`holds_design_review`（`holds_side_review` に置き換え）、`ParentEvaluationRecord.triggers`。
   `runaway_guard` の数える種類を広げるのは既存の規則の拡張で、旧い経路は残らない。
7. 不変条件: 下の節。試験は「保証と実装の対応」の表。

## 守る不変条件

- 評価は strategy root の checkpoint でだけ起き、1 trigger につき判断は 1 回、評価の reviewer run は `max_evaluation_attempts` 回まで（試行は起動の時点で数える）。
- checkpoint の状態は判断の行と root の成果物の name だけから `checkpoint_states` で導き、他の場所で別の方法で導かない。
- `cos_operations` の strategy 付きの `task.create` の照会は `strategy_task_creates` の 1 か所、戦略の参照の型は `task_core::strategy_ref::StrategyRef` の 1 つ。子 5 の gate の判定は変わらない。
- reviewer run の起動は設計 review と途中評価で同じ起動口を通る。途中評価は root の run を手元に持たず、木の上限に掛からないときだけ起こす。走っている評価の run は `abort_stale_runs` で止めない。
- 評価の結果は Outcome の到達を書かない。到達は D6 の Rust の規則だけが決める。参照の無い改善の主張を含む評価は有効な verdict が `on_track` にならない。
- 写していない評価があるとき、controller の行動は D5.2 の表の行動だけで、`off_track`・`unknown` の評価から `continue` は出ない。戦略の変更は人の承認を通った新しい版でだけ実行される。
- 評価は進捗に数えず、`evaluate` を含む進捗の無い判断の連続は `max_no_progress_decisions` で止まる。
- strategy root の無い案件・controller が無効な案件・`parent_checkpoint` の無い review は、snapshot・判断・reviewer run の数・`request.json`・prompt が base と同じ。設計 review の挙動は一般化の前と同じ。
- dispatcher・TS に LLM を入れない。events は追記だけ。Project.request と承認済み Roadmap を書き換えない。新しい表・event の種類・migration は無い。repo health の `deps.cycle` に新しい組を足さない。

## 保証と実装の対応

実装が保証する文だけを書く。実装の WU は、関数名・試験名が変わったらこの表を付記で直す。表に無い主張（評価の質など）は「未確認」に書く。

| 保証の文 | 関数（owner） | 試験 |
|---|---|---|
| strategy root だけが checkpoint を持つ | `task_core::strategy_reads::strategy_task_creates`、`task_ops::project_snapshot::build` | `project_controller_checkpoint_only_strategy_roots`、`parent_checkpoint_regression_no_strategy_root_no_reviewer_run` |
| strategy の照会は 1 か所で、gate の判定は変わらない | `task_core::strategy_reads::strategy_task_creates`、`cos::strategy_gate` | `strategy_reads_all_rows_in_order`・`strategy_reads_by_target`・`strategy_reads_same_created_at_orders_by_id`、既存の子 5 の gate の試験 |
| 子の終端の数が N の倍数に達すると `children:<k>` が `due` になる。再試行の `review_fail → ready` は数えない | `parent_checkpoint::triggers` | `parent_checkpoint_contract_children_every_n`、`…_review_fail_retry_not_counted`、`parent_checkpoint_shadow_fires_at_c_mid_d_done_f_done` |
| 子の `failed`・subtree の反証の証拠・予算の閾値で `due` になる | `parent_checkpoint::triggers` | `parent_checkpoint_contract_child_failed`、`…_refute_new_evidence_only`、`…_budget_thresholds`、`parent_checkpoint_shadow_budget_50_80`・`…_refute_in_subtree`・`…_child_failed` |
| 状態（発火済み・実行中・評価済み・試行の数）は判断の行と成果物の name だけから導く | `parent_checkpoint::checkpoint_states` | `parent_checkpoint_contract_states_from_rows_and_names`、`…_states_suppressed_is_fired`、`…_states_attempts_by_name` |
| 同じ trigger の判断は 2 回作られない（shadow の `suppressed` も発火済み） | `checkpoint_states`、`project_proposal::validate`（`checkpoint_not_due`） | `project_controller_checkpoint_same_trigger_once`、`parent_checkpoint_regression_shadow_no_reviewer_run` |
| `evaluate` の検証の 4 規則（`checkpoint_evaluation_open` は実行中だけを見る） | `project_proposal::validate`（snapshot の値） | `project_controller_checkpoint_validate_codes` |
| `evaluate` の効果は共通の起動口で reviewer run を 1 回起こし、task の status を変えない | `dispatcher::parent_evaluation`、`design_gate` の起動口、`project_wake` の効果の段 | `project_controller_checkpoint_evaluate_spawns_one_reviewer_run`、`…_root_status_unchanged` |
| 起動は `has_task_run(root)` が偽で `tree_limit_hold_for` が止めないときだけ | `dispatcher::parent_evaluation` | `project_controller_checkpoint_no_launch_while_root_run`、`…_tree_limit_holds` |
| 試行は試行の記録の name で数え、`max_evaluation_attempts` まで。run が消えても数え、上限で `invalid` の記録 | `checkpoint_states`、`dispatcher::parent_evaluation`、`review::parent_evaluation` | `project_controller_checkpoint_attempts_capped`、`…_lost_run_counts_attempt` |
| 走っている評価の run と設計 review の run は `abort_stale_runs` で止められない | `holds_side_review` | `side_review_holds_design_review_run`、`project_controller_checkpoint_run_survives_abort_stale` |
| 設計 review の挙動は起動口の一般化の前と同じ | `design_gate` の起動口 | 既存の設計 review の試験（`health_review/` の scenario 試験）が変更なしで通る |
| 参照の無い改善は unsupported で、on_track にならない | `parent_checkpoint::check_evaluation`（R2・R3） | `parent_checkpoint_contract_unsupported_not_on_track`、`…_url_only_is_unsupported` |
| goal の `met` は Rust の D6 だけ | `check_evaluation`（R4） | `parent_checkpoint_contract_goal_met_needs_d6` |
| 負の研究結果は 3 軸で表す | `check_evaluation`（R5・R6・`negative_result`） | `parent_checkpoint_contract_negative_result_axes` |
| 写していない評価に対する行動は D5.2 の表だけ | `parent_checkpoint::allowed_actions`、`project_proposal::validate`（`checkpoint_action_mismatch`、snapshot の値） | `parent_checkpoint_contract_allowed_actions_table`、`project_controller_checkpoint_off_track_continue_rejected` |
| `evaluate` は進捗に数えず、no-progress の guard が止める | `runaway_guard` | `project_controller_checkpoint_no_progress_guard_counts_evaluate` |
| 節の無い review の wire・prompt は変わらない | `protocol::ReviewRequest`・`change_rules::review::parent_checkpoint_section` | `parent_evaluation_absent_request_json_unchanged`・`…_prompt_unchanged`・`…_old_review_json_parses` |
| 節のある review の wire が schema と一致する | `protocol` | `parent_evaluation_wire_roundtrip`、`parent_evaluation_wire_schema_matches_md` |
| 評価器の入力に answer key が入らない | `dispatcher::parent_evaluation`（入力の組み立て） | `parent_checkpoint_shadow_request_has_no_answer_key`、`parent_checkpoint_live_request_renders` |
| D-done の pivot から子 5 の gate を通る新しい版で戦略が変わる | TS `pc/checkpoint/act`、CoS の skill、子 5 の gate | `parent_checkpoint_shadow_d_done_pivot_new_version`（gate の 422 と通過を確かめる） |
| controller が無効な案件で reviewer run が増えない | controller の I13 | `parent_checkpoint_regression_controller_off_no_reviewer_run` |

変更前に落ちる試験: `parent_checkpoint_contract_*`（`checkpoint_states` の試験を含む）・`project_controller_checkpoint_*`・`parent_evaluation_wire_*`・`parent_checkpoint_shadow_*`・`strategy_reads_*`・`side_review_*` は
base に関数・型・variant・欄が無いのでコンパイルか主張で落ちる。`parent_checkpoint_regression_*`・`parent_evaluation_absent_*`・既存の設計 review と子 5 の gate の試験は base でも通る回帰で、
変更の後も同じ値であることを確かめる。

## 未確認

- 評価の質（LLM の reviewer が answer key と同じ verdict・next を出すか）。shadow はスクリプトの評価で写しの経路だけを確かめる。live の sample は E-mid の 1 run だけで、同じ計画の WU の run なので
  本番の独立の reviewer run より独立の度合いが低い。子 8 の pilot と人の rubric の採点で確かめる。
- 反証の trigger の範囲: root の subtree と参照した文書の task の外（CoS が別の task で作った新しい版）の反証では発火しない。CoS がその版を作った時点で戦略を改めている前提で、実測は無い。
- 予算の trigger は root の `max_tokens` だけを見る。`max_tokens` の無い strategy root では発火しない。USD の閾値（戦略の候補の `cost.usd_max` に対する割合）は採っていない。
- evaluation の reviewer run が root の run として `running` に入る間、子の終端の後の root の継続の dispatch は待つ。待ちの長さは本番で測っていない。
- 起動口が run を起こしてから試行の記録を書くまで（同じ tick の中）に daemon が止まると、その run は試行に数えない。そのときは試行が `max_evaluation_attempts` を 1 回超えうる。
- snapshot の組み立てが root の `evaluate` の判断を判断の表から最後まで読む時間は、本番の判断の件数で測っていない。
- 子 5 の穴（D10）は見送った。採否は親の closeout で人が決める。
- 本番の config の変更は無い（節が無ければ既定値）。本番で controller を `enabled` にする案件は人が決める（既存）。

## 付記（2026-11-22、設計 review の不合格に合わせた改訂。WU adr-revise）

計画 v1 の設計 review が blocking にした 3 件に合わせて、D1.1・D1.3・D2.2・D2.3・D3.1・D3.4・D5.3・D10・D11 と、足さないもの・実装前 brief・守る不変条件・保証と実装の対応・未確認を直した。コードは変えていない。

1. 規約 2（`duplicate_implementation`）: 戦略の参照の型と `cos_operations` の照会が子 5 の gate と controller で別々になる設計だった。D1.1 で型を `task_core::strategy_ref::StrategyRef` の 1 つ、
   照会を `strategy_reads.rs` の 1 関数にし、gate の生の SQL・`StrategyOperation`・`ParentCheckpointStrategy` を消す（WU strategy-read）。
2. 規約 2（`duplicate_implementation`）: 評価の run の起動が設計 review の起動の写しになる設計だった。D2.3 で設計 review の起動・実行・終わりを `design_gate.rs` の中の 1 つの起動口に一般化し、
   起動の条件を `has_task_run` と `tree_limit_hold_for`、`abort_stale_runs` の例外を `holds_side_review` にした（WU review-launcher と controller-checkpoint）。
3. 規約 4（`dual_source_of_truth`）: 評価の状態を判断の行と記録の file の中身の 2 か所から導く設計だった。D1.3 で判断の行と成果物の name だけから `checkpoint_states` で導き、
   最終の記録から trigger の一覧を外し、試行の数を試行の記録の name で数えるようにした（D2.3・D3.4）。

この改訂に合わせ、計画に段 unify（adr-revise → strategy-read・review-launcher）が contract と controller の間に入った。

## 付記（2026-11-22、照会の置き場を crate 直下に移した。WU strategy-read）

D1.1・D11（write set の表・高 risk の class・deps.cycle）・足さないもの・実装前 brief・保証と実装の対応の表で、`cos_operations` の strategy の照会の置き場を
`crates/task-core/src/strategy_reads.rs` の自由関数 `task_core::strategy_reads::strategy_task_creates(store, target)` に直した。関数の名前・引数の意味・戻り値・SQL の条件・順は変えていない。

- 理由: 初めは照会を store の module の下に置き、`SqliteStore` の method にした。そうすると repo health の必須 rule `size.responsibility-growth` が fail した（store の module はその上限に達しており、
  1 行の増加でも落ちる）。計画の check 3（`bash scripts/dev/test-affected.sh`）が通らず、strategy-read と controller-checkpoint が止まった。
- 決定: 人への決定 `strategy-read-store-size` の答えは (a)「crate 直下の `task_core::strategy_reads` の自由関数に移し、この ADR と WU の check を再計画で直す」（CoS の代答。人の選択ではない）。
  repo health の profile・rule は変えず、例外も足さない。`cos_inbox` と同じく store の外から `SqliteStore` の `pub(crate)` の `with_read_conn` を使う。
- 結果: 照会の module は高 risk の class `store`（`^crates/task-core/src/store/`）に当たらなくなった。store・migration・events の file の差分は 0 である。
  試験は `crates/task-core/src/strategy_reads_tests.rs`（`strategy_reads_` の 3 件）に置いた。
- controller-checkpoint は `strategy_task_creates(store, Some(root))` を呼ぶ（D1.1・D3.1 の表）。

## 付記（2026-11-23、TS の規則の順。WU ts-checkpoint-rule）

D5.1 の 2（act）の置き場を書き足した。1（evaluate）の置き場は変えていない。

- 理由: Rust の `checkpoint_action_mismatch`（D5.2）は、写していない評価がある間、act 以外の proposal を落とす。act を evaluate の直後（既存の待ちの規則より後）に置くと、
  milestone の記録や起票の判断が先に出て毎回 `invalid_output` になり、act に届かない。そこで act は paused・no-progress の guard の直後に置き、evaluate は人の待ちの規則の後・root の見守りの前に置いた。
  no-progress の streak は `evaluate` を数える（D6、Rust の `runaway_guard` と同じ）。
- 試験: TS の `control-ts/src/domains/project_controller.test.ts` の `project_controller_checkpoint_` の 9 件（順は `project_controller_checkpoint_comes_after_paused_no_progress_and_the_daily_cos_limit`）。
  task-dispatch の `project_controller_checkpoint_` の試験（`dispatcher/tests/project_checkpoint.rs`）は controller-checkpoint の write set にあり、この WU では書かない。
- 未解決（owner は controller-checkpoint の write set。詳細は `agent-docs/progress/2026-11-22-strategy-v1-6/ts-checkpoint-rule.md` の未解決事項）:
  1. 写していない評価がある間、paused の wait・`pc/no-progress` の escalate・CoS 入力の日の上限の wait も `checkpoint_action_mismatch` で落ちる（TS の出力を Rust の検証に通して再現した）。
     no-progress の guard が人に知らせないまま止まる。
  2. D5.3 の「写し済み」は `effect_state` の `none` を数えない。continue の行を continue・wait で写した判断は効果が無く `none` になるので、その評価は写していないまま残る
     （コードを読んで導いた結論。試験での再現は未確認）。
