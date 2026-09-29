# ADR-0079: task の分解を再帰にする（task / 計画 / 段階 / unit = leaf | 子 task）。案件は方向と常設の文脈だけを持つ

- 日付: 2026-09-28
- 状態: **Accepted（R0 = 設計）**。人の決定（2026-09-28、§1.2）に基づく。実装は §7 の R1a〜R5b
- 関連:
  - ADR-0072（Task / ExecutionPlan / WorkUnit / Run、Complexity Gate、planner、repair、replan、上限）。本 ADR は D2・D13・D14・D18 の
    `max_planning_depth = 1（固定 2 層）`・D22・D24 を改める
  - ADR-0074（WU の並列・工程末尾の統合 WU・途中確認・案件計画・quota）。本 ADR は §2（3 層固定）と **D3 全体（案件計画）を廃止**し、
    D1（工程と統合）と D2（途中確認）を再帰の各段で使う
  - ADR-0044（タスク管理。D1 の `milestone_id`、D6 の途中目標の 3 階層）、ADR-0048（Console。CoS の `create_task` / `add_milestone`）、
    ADR-0062 D5（`create_task` の remote 検証。子 task にも同じ規則を当てる）、ADR-0038（途中目標の判定）、ADR-0077（途中目標の自動 reached）
  - ADR-0016 / 0021（委譲と子の失敗）、ADR-0043（worktree と取り込み）、ADR-0051（部署レビューから取り込み・配送）、ADR-0069（routing の 4 層）、
    ADR-0037 / 0070（通知・失敗の可視化）、ADR-0078（browser capability。Phase 1〜4 の分解が今日の事象の発端）
- 人の決定: 2026-09-28（§1.2。`docs/progress/phase-R.md` の R0 に原文を残す）

## 1. 文脈

### 1.1 今日（2026-09-28）起きたこと

| # | 事実 | 根拠 |
|---|---|---|
| T1 | ChatGPT（RDC、MCP `console_instruct`）からの「browser capability を足す。Phase 1〜4」という依頼を、CoS が **1 つの task**（01M3MBV3AKXZGEG5RXR60XC62J → retry 01M3MFS5T52FXA63W4V10XGC4S、題名「…の設計と Phase 1 MVP 実装」）にした。CoS は `create_task` と `add_milestone` の両方を持つが、複数 Phase の依頼をどう表すかの規則が無い（`crates/task-worker/src/preamble.rs` の `actions_instructions`。目安は「1 時間以内・承認不要なら `create_task` 1 つ」「人が儀式を求めれば `add_milestone`」だけ） | 本番 DB（read-only）、preamble.rs L983-1021 |
| T2 | その task の ExecutionPlan の「工程」（adopt / harden / ship）は**実行の段取り**であって製品の Phase ではない。Phase 2〜4 の依存付きの小タスク（P2-A〜P4-C）と人の決定点 H1〜H7 は、WU `design-gap` の成果物 `phases-and-decisions.md` に表として書かれ、**行き場が無い**（起票されず、受信箱にも出ない） | `/var/lib/celeris/workspaces/01M3MFS5T52FXA63W4V10XGC4S/wu/design-gap/artifacts/phases-and-decisions.md` |
| T3 | 案件に付く「案件計画」（ADR-0074 D3、`POST /projects/{id}/plan {mode: milestones}`、`celeris.project-plan/1`）は本番で一度も採用されていない（`project_plan_proposed` 0 件）。F6 で GUI に入口を足した後、17:51Z の案件の計画 run（01M3MJ94BXAJXW2XEX11PEB345、kind plan）は Qwen の API 不達で `failed` になった（基盤の失敗が「計画の失敗」として残った） | 本番 DB、`docs/execution-parallel-report-2026-09-28.md` §3 F4 |
| T4 | 案件「agent-platform の自己改善」は root の task が 109 件、途中目標が 21 行（`task_ops::add` が案件直下の task ごとに自動で作った 1:1 の行。ADR-0074 D3.8）。実態は粗い入れ物で、途中目標は「task の写し」にすぎない。BenchFS は `kind = plan` の分解 task の子（Phase0 / Phase1）が走り、framing 候補（HUMAN GATE 1 の決定材料）が done のまま、次の判断が記録されずに止まっている | 本番 DB |
| T5 | browser task の最終レビュー中に main が進み、merge-base のずれを直すための replan（`remerge` / `reship`）が要った。`remerge` は Task ブランチに直接 commit し、依存する `reship` の WU 準備が 20 分間無音で失敗し続けた（F5-fix7 で修正） | `docs/progress/phase-F.md` F5-fix7・本番確認節 |

人（2026-09-28）の結論: 案件と task の責務がずれた。直し方は**新しい中間層を足すことではなく、task の構造を再帰にすること**。

### 1.2 人の決定（2026-09-28。固定。これに合わせて設計する）

1. **節点の種類は task だけ**。task は目的・受け入れ条件・決定点を持ち、任意で計画 = 順序付きの**段階**を持つ。段階の unit は **leaf**（今の
   WorkUnit: 1 回の run で終わる）か **子 task**（自分の計画を持つ）。製品の「Phase N」は root task の段階の名前にすぎない。
   「取り組み / 途中目標 / 案件計画」はすべてこの構造で表し、専用の中間層は作らない。
2. **抽象度は自動で管理する**: Complexity Gate はすべての節点で走る（子 task は自分で gate をやり直す。atomic → 1 run、compound → 自分の計画）。
   planner は深さと残りの深さを受け取り、明示的な leaf の基準（1 run で終わる: 有界の turn、1 つの領域・リポジトリ、機械的な検査）に照らして
   各 unit を leaf / task と宣言する。最終判断は gate で、食い違いは記録する。上限: **max_depth = 3**（root → 子 task → leaf）、段階あたりの
   unit 数、木あたりの leaf の総数と費用。深いほど atomic に寄せる。上限に収まらない unit は**人への決定の要求**にし、黙って leaf に押し込まない。
3. root の計画の人の承認は、その計画が**決定点を含むか上限に近いとき**だけ。それ以外は通知して進める。決定の要求はどの深さからでも出せ、
   木の中の位置（path）を持ち、受信箱と通知（cluster_login_needed で使っている Discord の経路が既にある）に出る。答えの無い決定は、
   それに依存しない unit を止めない。子の失敗は親の replan（repair）で上限の中で吸収する。人に届くのは上限の超過だけ。
   **基盤の失敗は「質問」として人に届けない**（自動で回復し、回復できなければ障害通知）。
4. 子 task は**親のブランチに**成果を取り込む。main に取り込むのは root だけ（これで今日の「task の最終レビュー中に main が進んで
   merge_base がずれる」も消える）。
5. 案件（project）が持つのは**ごく粗い方向**（「BenchFS に取り組む」「celeris を良くする」「hoge クラスタの維持管理」）と常設の文脈
   （組織の既定、リポジトリ、知識の置き場 `projects/<slug>`、quota の枠、常設の認可、報告の集約）だけ。**案件に付く計画（ADR-0074 D3、
   `POST /projects/{id}/plan`、project-plan/1、途中目標の `auto_advance`、F6 の GUI「案件計画を提案させる」）は廃止**する。案件には依存の無い
   独立した root task が並列に付くだけ。依存は root task の木の中にだけある。

### 1.3 既にあるもの（設計の前提として確かめたこと）

- `celeris.execution-plan/2`（`crates/task-core/src/execution_plan.rs`）は `phases`（工程）・`work_units`・`children`（ADR-0074 D3.7。
  子 Task の提案を既存の委譲 `delegate::materialize` に通し、WU は `depends_on: ["child:<key>"]` で子の `done` を待つ）を持つ。本番の `delegated` は 5 件。
  **再帰の芽は既にある**: 足りないのは (a) 子が自分で gate / 計画をやり直すこと、(b) 子の成果が親のブランチに入ること、(c) 深さと木の上限、
  (d) 決定点の置き場。
- 工程末尾の統合 WU（ADR-0074 D1.4）は `celeris-wu/<task>/<key>` を Task ブランチへ決定的に `git merge --no-ff` し、検査を再実行する。
  WU ブランチを持たない WU の基点の規則（F5-fix7 の `integration::dependency_base`）もある。**子 task のブランチも同じ統合で入れられる**。
- 途中確認（ADR-0074 D2）: `Trigger::PhaseGate` / `PhaseResume`、`blocked(awaiting_human)`、`POST /tasks/{id}/execution/phase-gate`、
  `NotificationKind::PhaseCheckpoint`。本番では未通過（`awaiting_human` 0 件）だがテスト済み。
- 委譲の上限 `DelegationLimits { max_delegate_per_run: 8, max_tree_depth: 5, max_tree_runs: 100 }`（ADR-0016 D2）。
- 子待ちの親の扱いは 2 通りある: 委譲の親は `Reviewing` + `awaiting_children`（メモリ）、plan/2 の `child:<key>` 待ちは「Task は ready のまま待つ」
  （`WuDispatchGate::Skip`）。後者は GUI に理由が出ない（F5-fix7 と同じ「ready に見えて動かない」の形）。
- 通知の種類（`task_core::notify::NotificationKind`）: `QuestionBlocked`・`ClusterLoginNeeded`・`TaskFailed`・`PhaseCheckpoint` ほか。
  Discord の webhook（ADR-0037）は全種類で共通。
- 最新の migration は 0029（K-1 の `projects.slug`）、`SCHEMA_VERSION = 29`。

## 2. 用語と構造

```
Project（案件）… 粗い方向と常設の文脈だけ。計画を持たない。root task が並列に付く（互いに依存しない）
└─ root task（depth 1）… 人の依頼 1 件 = 1 つ。main に取り込む唯一の節点
   └─ 計画 plan/3（任意。gate が compound のとき）
      ├─ 段階 "phase-1"（stage。順序付き。末尾に統合 WU）
      │  ├─ unit p1   kind=task → 子 task（depth 2）… 自分で gate。compound なら自分の計画（leaf だけ）
      │  │                          └─ 段階 … leaf（depth 3 = 最下段。今の WorkUnit）→ Run
      │  ├─ unit doc  leaf       → WorkUnit → Run（continuation / retry / repair は今どおり）
      │  └─ integrate-phase-1（system WU。leaf のブランチと子 task のブランチを親ブランチへ merge）
      ├─ 段階 "phase-2" …（review: human なら段階の後で人の確認）
      └─ 決定 decisions[]（H1…。needed_before で依存する unit / 段階を指す）
```

| 語 | 意味 | 実体 |
|---|---|---|
| **task** | 唯一の節点。目的・受け入れ条件・決定点・（任意で）計画。担当・worktree・ブランチ・最終レビューを持つ | `tasks`（従来どおり） |
| **root task** | 親を持たない task。案件に直接付く。人の依頼の単位で、main に取り込む | `tasks.parent_id IS NULL` |
| **子 task** | 親の計画の unit（kind task）から daemon が作った task。親のブランチに取り込む | `tasks.parent_id` + `Task.tree.parent_unit` |
| **計画** | task の中の段階と unit と決定の集合。版を持つ（replan） | `execution_plans`（従来どおり） |
| **段階（stage）** | unit の束。配列の順に進む。末尾で統合する。製品の「Phase N」も実行の「調査 / 実装」もこれ | plan/3 の `stages`（= plan/2 の `phases`。DB の列名は `work_units.phase` のまま） |
| **unit** | 段階の中の仕事の単位。**leaf** か **task** | `work_units` の行（kind `task` は子 task の代理の行） |
| **leaf** | 1 回の run で終わる unit（今の WorkUnit。continuation は許す） | `work_units`（kind が task 以外） |
| **決定の要求** | 人に選んでもらう点。構造化され、木の中の位置を持つ | `decisions`（新設、§3 D7） |
| **深さ（depth）** | root = 1、子 task = 2、leaf はその task の深さ + 1 | `Task.tree.depth` |

---

## 3. 決定

### D1. 再帰のモデルと語彙

- 節点は task だけ。計画の unit は **leaf か子 task** のどちらかで、leaf は計画を持たない（ADR-0072 の「WU は計画を持たない」はそのまま）。
  **再帰は子 task を通してだけ起きる**。WorkUnit の中で再分割はしない（ADR-0074 R15 の考え方を保つ）。
- 「取り組み」「途中目標」「案件計画」「Phase N」は語として残してよいが、**実体はすべて task / 段階**:
  - 取り組み = root task。途中目標 = root task（または子 task）の段階で、人が確認したいものは `review: human`（D5）。
  - 案件計画 = 無い（案件には root task が並ぶだけ。D13）。
- **ADR-0072 D2（WorkUnit を子 Task にしない）は leaf について保つ**。子 task にするのは「別に受け入れ・レビュー・判断されるまとまり」か
  「1 run に収まらないまとまり」だけで、それは gate と planner が決める（D4）。context の都合の分割は今どおり leaf の continuation。
- task に付くもの（worktree・ブランチ・受け入れ条件・最終レビュー・担当・events）は子 task にもすべて付く。**違うのは取り込み先だけ**（D6）。

### D2. schema `celeris.execution-plan/3`

```json
{
  "schema": "celeris.execution-plan/3",
  "rationale": "製品の Phase 1〜4 を段階にし、各 Phase は独立に受け入れられるので子 task にする",
  "stages": [
    {"key": "phase-1", "kind": "implement", "title": "Phase 1 MVP"},
    {"key": "phase-2", "kind": "implement", "title": "Phase 2 policy・credential・durable wait", "review": "human"},
    {"key": "phase-3", "kind": "implement", "title": "Phase 3 identity・live proxy・takeover"},
    {"key": "phase-4", "kind": "implement", "title": "Phase 4 isolated runtime・注入・backend routing"}
  ],
  "units": [
    {"key": "p1", "stage": "phase-1", "kind": "task", "title": "…", "objective": "…",
     "acceptance": [{"text": "…", "check": {"type": "reviewer"}}],
     "depends_on": [], "needs_decisions": [], "genre": "coding", "skills": ["rust"],
     "features": {"judgment": "medium"}, "repos": ["agent-platform"]},
    {"key": "p2-a", "stage": "phase-2", "kind": "task", "title": "policy 契約", "objective": "…",
     "acceptance": [{"text": "…"}], "depends_on": ["p1"], "needs_decisions": ["h2", "h6"]},
    {"key": "p2-note", "stage": "phase-2", "kind": "design", "title": "…", "objective": "…",
     "done_when": ["…"], "checks": [{"cmd": "test -s artifacts/p2-note.md", "expect_exit": 0}],
     "context": {"paths": ["docs/"], "repo": "agent-platform"}, "budget": {"max_turns": 30}}
  ],
  "decisions": [
    {"key": "h1", "question": "credential backend をどれにするか",
     "options": [{"key": "org-vault", "label": "既存の組織 vault"}, {"key": "1password", "label": "専用 1Password vault"},
                 {"key": "manual", "label": "専用の手動登録（試験用）"}],
     "recommended": "org-vault", "cost_of_reversal": "medium",
     "cost_note": "項目 ID / policy の移行と権限の再承認が要る", "needed_before": ["p2-b"]}
  ]
}
```

- **unit**: `kind` の語彙に **`task`** を足す（`WorkUnitKind::Task`）。`kind = task` の unit は `acceptance`（1 件以上、`Criterion`）を必須にし、
  `genre` / `skills` / `features` / `repos`（親の repos の部分集合）/ `needs_decisions` を持てる。`checks` / `budget` / `harness` / `context.paths` は
  持たない（子 task が自分で決める。書けば検証で拒否）。kind が task 以外の unit が **leaf** で、今の WorkUnit の欄（`done_when` / `checks` /
  `context` / `harness` / `features` / `budget` / `outputs`）に加えて `needs_decisions` を持てる。unit 側の `decisions`（その unit が持ち込む決定）は
  書いてもよく、計画の `decisions` に `needed_before: [<その unit>]` を付けて移したものとして読む（同じ意味の糖衣）。
  kind task の unit の `adopt: <task_id>`（既存の task をその unit の子として採用する。D15）は**人の計画（origin human）にだけ**書ける
  （planner の出力にあれば拒否）。
- **stages**: plan/2 の `phases` と同じ形に `review: "none" | "human"`（既定 none）を足す。`review: human` は ADR-0074 D2 の `pause_after` の
  その段階への指定と同じ意味（D5）。planner が書いてよい（人の決定 3 の「決定点」の一種として扱い、D8 の承認の条件に数える）。
- **依存**: `depends_on` は unit の key（leaf・task を問わない）。plan/2 の規則（同じ段階か前の段階、同じ段階の中は高々 1 つ）はそのまま。
  plan/2 の `child:<key>` 接頭辞と `children` 配列は **plan/3 では書けない**（kind task の unit が置き換える）。
- **decisions**: `{key, question, options: [{key, label, consequence?}] (2..=5), recommended (options の key), cost_of_reversal: low|medium|high,
  cost_note?, needed_before: [unit key | "stage:<key>"]}`。`needed_before` が空の決定は許さない（何を止めるかを書かせる）。
- `deny_unknown_fields`。`assignee` / `tier` / `model` / `lane` は持たない（ADR-0069 D1 のまま）。
- **型**: `ExecutionPlanSpec` は `schema` の値で /1・/2・/3 を分ける 1 つの型のまま（ADR-0074 D1.1 の方針）。/3 の `stages` / `units` は
  内部で /2 の `phases` / `work_units` と同じ行に写す（DB の列は増やさず `work_units.phase` に段階の key）。`docs/protocol/execution-plan.schema.json`
  を再生成する。
- **後方互換**: /1・/2 の計画は受け付け、今と 1 バイトも変えずに走る（/2 の `children` も ADR-0074 D3.7 のまま）。planner に /3 を出させるのは
  `[execution.tree] enabled = true` のときだけ。ADR-0074 §4.4 観察 1（`parallel` がプロンプトにしか効かず採用の関門でなかった）を繰り返さない
  ため、**/3 の採用も同じ設定で関門にする**（`enabled = false` なら /3 は検証で拒否、人の `PUT` も 422）。

### D3. 深さと上限

`[execution.tree]`（すべて任意。組織の profile の `budget` で狭められるのは `max_tree_runs` / `max_tree_tokens` だけ。最も厳しい値。ADR-0069 D2）:

| 設定 | 既定 | 意味 | 超えたとき |
|---|---|---|---|
| `enabled` | `false`（R5b で人が `true` に） | plan/3 と子 task の生成を有効にする | — |
| `max_depth` | **3**（1..=3 だけ。広げるには ADR） | root = 1。深さ d の task が子 task を持てるのは `d + 1 < max_depth` のとき（既定では root だけ）。leaf は常に最下段 | 検証で拒否 → D4 の決定の要求 |
| `max_units_per_stage` | 6（leaf + task、統合 WU と repair は数えない） | 1 段階の unit 数 | 検証で拒否 |
| `max_stages` | 5（= `max_phases`） | 1 計画の段階の数 | 検証で拒否 |
| `max_child_tasks_per_plan` | 6 | 1 計画の kind task の unit 数 | 検証で拒否 |
| `max_parallel_child_tasks` | 2 | 1 つの親で同時に非終端の子 task の数 | 作らずに待つ（`pending` のまま） |
| `max_tree_leaves` | 40 | 木の生涯で作る leaf（repair・統合を除く） | 決定の要求（limit） |
| `max_tree_runs` | 120 | 木全体の worker / planner / repair の run（reviewer を除く。ADR-0016 の 100 を置き換える） | 決定の要求（limit） |
| `max_tree_replans` | 10 | 木全体で採用した replan の版 | 決定の要求（limit） |
| `max_tree_tokens` | 無し | 木全体の input + output（設定したときだけ） | 決定の要求（limit） |
| `max_open_decisions` | 12 / 木、8 / 計画 | 未回答の決定の数 | 計画の検証で拒否 / 新しい要求を束ねる（D7） |
| `gate_depth_step` | 2 | 深さ d の gate の閾値 = `5 + step × (d − 1)`（depth 2 は 7） | — |
| `approval_near_limit_ratio` | 0.8 | D8 の「上限に近い」 | — |

- 節点ごとの上限（`max_runs_per_task` 24、`max_replans` 3、`max_repairs` 3 など、ADR-0072 D18 / ADR-0074 §4）は**そのまま各 task に効く**。
  木の上限はその上に重なる（どちらか先に当たった方）。
- **費用の上限は「止めて聞く」上限だけ**（run 数とトークン）。金額・quota の配分や dispatch の選択には使わない（CLAUDE.md の「予算管理は
  別プロジェクト」、ADR-0074 R12）。
- **超えたとき**: 黙って打ち切らず、黙って leaf に押し込まず、黙って atomic に倒さない。超えた節点に決定の要求（`kind: limit`、D7）を出し、
  その節点の新しい run だけを止める（兄弟・他の subtree は続く）。選択肢は「今回だけ上限を上げて続ける / この subtree を replan で小さくする /
  この subtree を取り下げる」。

### D4. 節点ごとの gate と、計画の unit から子 task を作ること

**(1) 節点ごとの gate**: 子 task も最初の dispatch で Complexity Gate（ADR-0072 D13）を通る。違いは 2 つ:
- 閾値を深さで上げる（`gate_depth_step`。深いほど atomic に寄る）。`ExecutionGateDecision` に `depth` と `threshold` を残す。
- 木の節点では gate の判定を**常に採用する**（`gate = "shadow"` でも子 task の compound は planner に進む）。root が compound で採用された時点で
  人の明示（F3 の例外）か `gate = "on"` を通っているので、その下で shadow にすると「分けると決めた木が途中で 1 run に潰れる」ことになる。
  root の gate は従来どおり `[execution] gate` に従う。

**(2) planner の入力と leaf の基準**: planner run（ADR-0072 D14 / ADR-0074 D5.3）に `depth`・`remaining_depth`（= `max_depth − depth − 1`、
子 task を書けるのは 1 以上のとき）・木の残りの上限（leaf・run・未回答の決定）・祖先の path と各祖先の目的の先頭 300 文字を渡す。プロンプトに
**leaf の基準**を明記し、planner は unit ごとに leaf / task を宣言する:
- leaf の基準（3 つすべて）: (a) 1 回の run で終わる（`budget.max_turns ≤ 80`、壁時計 ≤ 3,600 秒。continuation は保険であって前提にしない）、
  (b) 1 つの領域・1 つのリポジトリ（`context.repo` は高々 1、`context.paths` は 1 つの部分木が目安）、(c) 機械的な検査がある（`checks` が 1 本以上）。
- task にする理由: leaf の基準のどれかを満たさない、別に受け入れ・レビューされるまとまり、人の確認（human の acceptance・`needs_decisions`）を持つ、
  別の部署の skill が要る。
- 決定的な検証: leaf で `checks` が空、`context.repo` が 2 以上、`budget` が上限超過 → 拒否（「task にするか検査を足せ」の文言）。

**(3) gate の最終判断（unit の gate）**: 計画の採用のとき、daemon は各 unit の **view**（ADR-0072 D21 と同じく Task を複製し目的・受け入れ・予算・genre を
unit の spec に差し替えたもの）に深さ `d + 1` の閾値で gate をかける（純粋関数 `task_core::tree::unit_gate(view, depth, limits) -> UnitGate`）:

| planner の宣言 | unit の gate | 結果 | 記録 |
|---|---|---|---|
| leaf | atomic | leaf | — |
| leaf | compound、子 task を持てる深さ | **task に上げる** | `UnitGateOverridden{declared: leaf, gate: compound, action: promoted}` |
| leaf | compound、持てない深さ | **決定の要求**（`kind: leaf_too_large`）。その unit は `blocked(decision)`、他は進む | 同上（action: decision） |
| task | compound | task | — |
| task | atomic、構造上の理由（human の acceptance・`needs_decisions`・親と違う repo / genre / 部署）がある | task（子は自分の gate で atomic → 1 run） | `UnitGateOverridden{…, action: kept_task}` |
| task | atomic、構造上の理由なし、leaf の基準を満たす | **leaf に下げる** | `UnitGateOverridden{…, action: demoted}` |
| task | 子 task を持てない深さ | 検証で拒否 → 再試行 → それでも同じなら決定の要求 | — |

食い違いは `Event::UnitGateOverridden` で残し、`GET /metrics/execution` に「planner と gate の不一致」の件数として出す（閾値の調整の材料。ADR-0072 U10）。

**(4) 子 task の生成**（daemon、決定的、LLM なし）: kind task の unit が **ready になったとき**（依存がすべて done、`needs_decisions` がすべて回答済み、
段階が現在の段階、`max_parallel_child_tasks` に空き）に、1 トランザクションで:
- 子 task を作る: `parent_id` = 親、`project_id` = 親の案件、`kind = execute`、`title` / `objective` / `acceptance` = unit の spec、`objective` の末尾に
  回答済みの関係する決定（D7）と祖先の path（題名の列）を固定の書式で足す、`genre` / `skills` = unit（無ければ親）、`repos` = unit（親の部分集合。
  無ければ親と同じ）、`workspace` = 親（ADR-0039 D2 / ADR-0062 D5 の検証を同じく通す。`cluster:<id>` を持たない担当への remote は落とす）、
  `budget` = 親の `budget`、`status = ready`（親の計画が承認・採用済みなので draft を挟まない）、`labels` に `child-<key>`（F4b と同じ引き方）。
- 担当は matching が決める（ADR-0069 D1。unit は担当を書けない）。部署をまたぐ子は ADR-0074 F4b の規則のまま（未認可なら計画を採用せず
  approvals で聞く。SPEC §3.1）。
- `TaskFeatures` は子の目的・受け入れから `infer_with_hints` で推定し、unit の `features` をヒント（出自 `planner`）として重ねる。
- `Task.tree = {root_id, depth, parent_unit: {task_id, plan_id, unit_key, stage}, base_commit}`（Task の JSON）と列 `tasks.root_id`（索引。migration 0031）。
- 親の unit の行を `running`、`child_task_id` を書き、親の events に `Event::ChildTaskCreated{unit_key, child_task_id, depth}`、子の `Created` に
  `origin: plan_unit`。
- 子の中からの委譲（`delegate.json`）は使えない（木の節点の run には `available_genres` を渡さない。子を作る入口は計画の unit だけ）。

**(5) 子の状態の写し**: 親の unit の状態は子 task の状態から決定的に決まる（tick の照合と子の遷移の同じトランザクション）:
子が非終端 → unit `running`、子が `done` → unit `done`（統合待ち）、子が `failed`（work の失敗）→ unit `failed`（D9）、子が `cancelled` → unit
`cancelled`（人が子だけを止めたときは親の replan を起こす）、子が `blocked` → unit はそのまま `running`（子の質問・決定は子の path で受信箱に出る）。

### D5. 段階の完了

- **段階 S が完了した** ⇔ S の unit（superseded・取り下げを除く）がすべて次を満たし、かつ `integrate-S` が done（ADR-0074 D1.4 の検査の再実行まで通った）:
  leaf は `done`、子 task は **`done` で、かつ `integrate-S` がそのブランチを親ブランチに merge した**（`PhaseIntegrated.merged` に子の key と commit がある）。
- 統合の順: 段階の葉（同じ段階の他の unit に依存されていない unit）を `seq` 順に merge する規則はそのまま。子 task は `celeris/<child_id>` を merge する
  （WU ブランチ `celeris-wu/…` と同じ扱い）。子が `workspace_mode = shared` か remote でブランチを持たなければ merge しない（今の「並列 1 に倒す条件」と同じ）。
- `blocked(decision)` の unit がある段階は完了しない（次の段階は pending のまま）。**同じ段階の他の unit は止めない**（人の決定 3）。
- `review: human` の段階は、統合の後で `Trigger::PhaseGate`（ADR-0074 D2.2）を当て、途中報告（D2.3。子 task の要約を 1 行ずつ足す）を出す。
  操作は D2.4 の「続ける / replan（指示つき）/ 取り下げ」。これが SPEC §7「途中目標の達成ごとに判定」の新しい置き場になる（ADR-0038 の ok = 続ける、
  議論・ng = replan）。
- **親の待ち方**: 段階の中で走れる自分の leaf が無く、子 task だけを待っている親は `Ready` のまま dispatch されない（`WuDispatchGate::Skip`、今の plan/2 と同じ）。
  ただし表示用の導出値 `ExecutionPhase::AwaitingChildren`（待っている子の題名と状態）を足し、GUI と `GET /tasks/{id}/tree` に理由を出す。親は lease を持たない
  （子が何日走っても親の lease は要らない）。子が終わったら次の tick の照合で unit を done にし、段階が揃えば統合のために親を dispatch する。

### D6. 親のブランチへの取り込み（各段の「配送」の意味）

| 段 | 成果の置き場 | 取り込み先 | 誰が取り込むか |
|---|---|---|---|
| leaf | WU ブランチ `celeris-wu/<task>/<key>`（または Task の worktree） | その task のブランチ | 段階末尾の統合 WU（ADR-0074 D1.4。変えない） |
| 子 task | `celeris/<child_id>`（基点は親の段階の基点） | **親のブランチ `celeris/<parent_id>`** | 親の段階末尾の統合 WU（D5） |
| root task | `celeris/<root_id>` | main（既定ブランチ） | ADR-0051 の部署レビュー後の取り込み（selfdeploy 案件）か ADR-0043 D5 の人の取り込み（merge / PR / 破棄）。変えない |

- **子の基点**: 子の worktree は `Task.tree.base_commit` から切る。値は unit の `base_commit`（段階の最初の unit は「段階が始まった時点の親ブランチの HEAD」、
  同じ段階の依存を持つ unit は依存先の HEAD。F5-fix7 の `dependency_base` を子 task のブランチにも広げる）。`Dispatcher::worktree_base` は木の子について
  既定ブランチの代わりにこの値を返す。
- **子の最終レビュー**は子の単位で今どおり走る（受け入れ条件・reviewer・決定的な検査）。差分の基点は `base_commit`、merge-base の検査の相手は**親のブランチ**。
  親のブランチは段階の途中では動かない（動くのは段階末尾の統合だけ）ので、**子の最終レビュー中に基点がずれることは起きない**（人の決定 4）。
- 子 task には ADR-0051 の取り込み（`deliveries`）も ADR-0043 D5 の取り込みボタンも無い。子の `done` は「親の段階で取り込まれる準備ができた」を意味し、
  `TaskReady` 通知は鳴らさない（root の `done` だけが鳴る。D11）。
- root だけが main と比べる。root の最終レビューの merge-base のずれは今どおり ADR-0072 D16 / ADR-0074 D6.2 の `merge_base` repair で直す。
- 後片付け: 子の worktree は親の統合が済んだら消す。ブランチは root の終端まで残す（監査のため）。root の中止・取り込みで木のブランチをまとめて消す
  （ADR-0043 D2 の規則を木に広げる）。
- **GUI の語**: 「配送」を**「成果の取り込み」**に改める。root は「成果の取り込み（main へ）」、子は「成果の取り込み（親『<題名>』の段階『<段階>』へ）」と
  取り込み済みの commit。ADR-0051 の release / 昇格の語（「配送済み（release …）」）は「main に取り込み済み（release …）」にする。API の欄名（`delivered_release`
  など）は変えない（互換）。

### D7. 決定の要求

**形**（`task_core::decision::DecisionRequest`、`docs/protocol/decision.schema.json`）:

```json
{
  "id": "01M…",                        // daemon が振る ULID（木の中で一意）
  "key": "h1",                         // 出した者が付けた key（計画・run の中で一意）
  "kind": "choice",                    // choice | leaf_too_large | limit | plan_invalid
  "question": "credential backend をどれにするか",
  "options": [{"key": "org-vault", "label": "既存の組織 vault", "consequence": "…"}, …],
  "recommended": "org-vault",
  "cost_of_reversal": "medium",       // low | medium | high
  "cost_note": "項目 ID / policy の移行と権限の再承認",
  "needed_before": ["p2-b"],           // <unit key> | stage:<key> | self（self は worker が出すときだけ）
  "path": [{"task_id": "01M…root", "title": "browser capability", "stage": "phase-2"},
           {"task_id": "01M…p2b", "title": "broker / provider 1 種", "unit": "p2-b"}],
  "raised_by": {"task_id": "…", "run_id": "…", "origin": "planner | worker | daemon"},
  "status": "open"                     // open | answered | withdrawn
}
```

- **出どころ**: (a) planner の計画の `decisions`（D2）、(b) worker の `result.json` / `checkpoint.json` の `decisions`（同じ形。`needed_before: self` は
  その unit を `blocked(decision)` にする。それ以外は run の完了を妨げない）、(c) daemon（`leaf_too_large` / `limit` / `plan_invalid`。D3・D4・D9）。
  `path` と `id` は daemon が付ける（LLM に書かせない）。
- **置き場**: `Event::DecisionRequested{decision}` / `DecisionAnswered{id, option, note, by}` / `DecisionWithdrawn{id, reason}` を出した節点の events に積み、
  派生の表 `decisions(id, root_id, task_id, key, kind, status, needed_before_json, json, created_at, answered_at)`（migration 0031）に同じトランザクションで書く。
  `approvals`（SPEC §3.6 の「今回だけ / 今後ずっと」の認可）とも `QuestionRaised`（task を止める自由文の質問）とも別物にする。
- **待つもの・待たないもの**: `needs_decisions` を持つ unit と、`needed_before` が指す unit / 段階だけが pending のまま待つ。他の unit・兄弟の subtree は進む。
  `needed_before` に指された子 task はまだ作られない（D4 (4)）。
- **答えの流れ**: `POST /decisions/{id}/answer {option, note?}`（管理系）/ MCP `decision_answer`（scope `tasks:interact`、`task_answer` と同じ重さ）/ GUI。
  1 トランザクションで `DecisionAnswered`、表の更新、待っていた unit の再評価（ready にする）。回答は依存する unit の入力に**固定の書式**で入る:
  leaf は前置きの「人の決定」節、子 task は生成時の `objective` の末尾（`## 人の決定（ADR-0079 D7）` の下に `- <key> <question>: <label>（推奨どおり / 推奨と異なる）— <note>`）。
  子孫は祖先の objective を通して回答を継ぐ。回答済みの決定を変えたいときは `POST /decisions/{id}/revise`（新しい `DecisionAnswered`。既に作られた子には
  人のコメント〈ADR-0044 D2〉として届け、作り直しはしない）。
- **通知**: `NotificationKind::DecisionRequested`（key = `decision:<id>`。計画の採用で同時に出た複数は key = `plan:<plan_id>:decisions` の 1 通に束ねる）。
  Discord の経路は既存（ADR-0037。`ClusterLoginNeeded` と同じ webhook）。本文は「『<root>』› <段階> › <unit>: <question>（推奨: <label>、後戻り: 中）→ <link>」。
  未回答のまま 24 時間たったら 1 回だけ再通知（SPEC §3.5 の「数時間単位」）。受信箱に `AttentionItem::Decision{id, path, question, recommended, needed_before, age}`。
- **上限**: 未回答の決定は計画あたり 8、木あたり 12（D3）。超える出力は計画なら検証で拒否、worker の出力なら超えた分を 1 件の「決定が多すぎる」決定に束ねる。

### D8. root 計画の承認の規則

- 計画の採用（`ExecutionPlanned`）は今どおり daemon の検証で決まる。**root の計画**（depth 1、/3）について、採用と同じトランザクションで
  `approval_required = has_decisions || has_review_human_stage || near_limits` を決定的に計算する。`near_limits` は、段階数・unit 数・子 task 数・
  見込みの leaf 数（leaf + 子 task × 既定 4）・木の run の見込みのどれかが上限 × `approval_near_limit_ratio` 以上。
- `approval_required` なら `Trigger::PlanGate`（Running → Blocked、reason `awaiting_plan_approval`、attempts を変えない）と
  `Event::PlanApprovalRequested{plan_id, reasons}`。受信箱の `AttentionItem::PlanApproval`（計画の木の見取り図・決定の一覧・上限の使用率）と通知
  （`NotificationKind::PlanApproval`）。操作は `POST /tasks/{id}/execution/plan-gate {action: approve | replan | withdraw, note}`（D2.4 と同じ形。replan は note 必須）。
  承認までは unit を 1 つも起こさない。**決定への回答は承認と別**（承認した後も、答えの無い決定に依存しない unit は進む）。
- そうでなければ承認を挟まずに進め、報告の流れ（ADR-0034）に「計画を採用して進めます: <段階の一覧>」を 1 件残す（Discord は鳴らさない）。
- 子 task の計画は承認を求めない（決定の要求は出せる）。root の replan の版にも同じ規則を当てる（replan で決定や上限の近さが生じたら承認を求める）。
- ADR-0074 D2 の途中確認（`pause_after` / `review: human`）は承認とは別に今どおり効く。

### D9. replan / repair の再帰と、人に届くもの・届かないもの

- **子の中の失敗は子が先に吸収する**: continuation・retry・repair・replan は各 task の中で今どおり（ADR-0072 D11 / D16 / D17）。
- **子が `failed` になったら親が吸収する**: 親の unit は `failed` → 親の replan（ADR-0072 D17 の起点 1。起こした理由は「子 task <題名> が失敗: <分類と理由>」、
  planner の入力に子の最後の checkpoint の要約と最終レビューの不合格の理由を足す）。親の planner は子をやり直す（新しい key の task unit、目的を直して）・
  分ける・落とす、のどれかを新しい版で出す。子の done の成果（ブランチ）は捨てない。
- **予算は subtree ごと**: 節点の `max_replans` 等に加えて木の `max_tree_replans` / `max_tree_runs` / `max_tree_leaves`。**人に届くのは上限を超えたときだけ**で、
  そのときは超えた節点の path を持つ `kind: limit` の決定の要求（D3）。`WorkerQuestion` による「分割し直す / 中止」の自由文の質問（ADR-0072 D12）は、
  木の節点では構造化した決定の要求に置き換える。
- **plan/3 の計画が 2 回不正だったとき**は atomic に倒さない（ADR-0072 D14 の倒し方を /3 では採らない。倒すと「分けると決めた仕事が黙って 1 run に潰れる」）。
  `kind: plan_invalid` の決定の要求（選択肢: 人が計画を書く〈`PUT /tasks/{id}/execution-plan`〉/ atomic で試す / 取り下げ）。/1・/2 は今どおり。
- **基盤の失敗は質問にしない**: supply / infra / lease の失効は `Requeue` / `InfraRequeue`（ADR-0010 / 0070）で自動回復する。子 task が基盤の分類
  （`classify_task_failure` の class が infra / supply）で `failed` になったら、親は replan ではなく**子を 1 回だけ複製して再試行**（`task_ops::retry`、
  `max_child_infra_retries = 1`）。それでも失敗したら**障害通知**（`TaskFailed` を基盤の分類で、報告は悪い知らせ `BadNews`）を出し、unit は `blocked(infra)`
  （GUI の「再試行」だけを出す。決定の要求にはしない）。T3 の「Qwen 不達が計画の失敗に見える」はこの経路になる。

### D10. 黙って止まらない（木の生存確認）

- 木の非終端の節点は、常に次のどれか 1 つに分類できなければならない: **走っている**（run・検査・統合が in-flight）/ **走れる**（次の tick で dispatch される）/
  **名指しの待ち**（子 task・決定・人の確認・承認・依存先・基盤の回復のどれかを、対象の id 付きで待つ）。純粋関数 `task_core::tree::liveness(&TreeSnapshot) -> Vec<NodeLiveness>`。
- どれにも当たらない節点が `stall_secs`（既定 600 秒）続いたら `Event::StallDetected{task_id, detail}` と障害通知（`BadNews`、基盤の扱い）を出し、GUI の
  節点に「理由なく止まっています」を出す。F5-fix7 の 20 分の無音（準備の失敗が ready に見えた）、plan/2 の「子待ちの ready」、T3 の計画 run の失敗の後に
  何も起きない、を同じ網で捕まえる。判定は決定的（store の読み取りだけ）、LLM を使わない。

### D11. 報告と metrics の roll-up

- `task_core::tree_metrics::rollup(&TreeSnapshot) -> Vec<SubtreeMetrics>`（純粋関数、`runs` の索引と events から）。節点ごとに**自分と subtree の合計**:
  run 数（role ごと）、トークン、定価 `cost_usd` と `cost_usd_complete`、quota（アカウント × 窓の point と method。ADR-0074 D4）、壁時計（subtree の最初の
  dispatch から最後の終端まで、と実働時間）、leaf の done / total、子 task の done / total、未回答の決定、人を待った時間。
- API: `GET /tasks/{id}/tree`（節点・段階・unit・子を再帰で、roll-up 付き）。`GET /tasks/{id}/execution` の `metrics` は自分の分のまま（互換）。
  `GET /projects/{id}` は root task ごとの roll-up の和。`GET /metrics/execution?group_by=depth` を足す。
- **報告**（SPEC §3.5 の圧縮）: leaf の checkpoint → 段階の途中報告（ADR-0074 D2.3、機械的な束ね）→ 子 task の完了の要約（決定的な 1 段落。LLM なし）が
  親の段階の途中報告に入る → root の完了の報告が案件に上がる。通知は root の `done` / `failed`・決定・承認・障害だけ（子の完了・子の失敗〈親が吸収するもの〉は鳴らさない）。

### D12. CoS の指針

- **1 つの依頼は 1 つの `create_task`**（root task）。大きさ・段階の数の判断を CoS はしない（gate と planner の仕事）。依頼の範囲を狭めない:
  人が「Phase 1〜4」と言ったら、目的と受け入れ条件に Phase 1〜4 のすべてを書く。人が段階を名指ししたときだけ、その名前と範囲を任意の
  `stages_hint: [{"title": "Phase 1", "scope": "…"}]` にそのまま写す（planner への入力。構造の強制ではない）。
- 互いに独立な依頼（「A を直して、ついでに無関係な B も」）は root task を 2 つ（依存は書けない）。一方が他方に依存するなら 1 つの root task にまとめる
  （依存は木の中にだけある。人の決定 5）。
- `execution: compound` のヒントは CoS の指示から消す（CoS は大きさを判断しない。欄は互換のため読むが、プロンプトでは求めない）。`pause_after` は
  人が段階ごとの確認を頼んだときだけ写す。
- `project` は既存の案件（方向）から選ぶ。`propose_project` は人が新しい方向を名指ししたときだけ。**`add_milestone` は廃止**（action の検証で
  「途中目標は root task の段階で表す（ADR-0079）」として落とし、人に理由が見える。ADR-0048 D3 の他の action は変えない）。
- RDC / console の経路（MCP `console_instruct` → CoS の対話 run → `actions`）は変えない。変わるのは preamble の文面（`actions_instructions` と秘書の対話の節）だけ。

### D13. 案件モデルの変更

**案件が持ち続けるもの**: `title`（方向）、`request`（方向の説明。F6 の編集のまま）、`slug`（K-1。知識の置き場 `projects/<slug>` と文書リポジトリ）、
`status` と中止・一時停止・アーカイブ（ADR-0044 D6。中止は root task の木ごと連鎖）、`project_repos`、`workspace`、組織の既定（担当の既定・profile の継承）、
常設の認可（`standing_rules`）、quota の枠（観測と表示だけ）、報告の集約、対話（秘書との案件の会話）。

**案件が失うもの**（API・GUI・CLI から外す。行は消さない）:

| 対象 | 扱い |
|---|---|
| `POST /projects/{id}/plan`（`mode: decompose` / `milestones` とも） | **410 Gone**（`{"detail": "ADR-0079: 案件は計画を持たない。root task を作る"}`）。`task_ops::project_plan::{start, start_milestones, propose, propose_delta}` を外す |
| `POST /projects/{id}/project-plan/{version}/decide` と `celerisctl projects plan approve / reject` | 410 / サブコマンド削除 |
| `POST /projects/{id}/milestones`、`PATCH /milestones/{id}`、`POST /milestones/{id}/{cancel,pause,resume}` | 410（`GET` は履歴として残す） |
| `PATCH /projects/{id} {auto_advance}` | 422（列 `projects.auto_advance` は残し読まない） |
| `task_ops::add` の案件直下の task への途中目標の自動作成（ADR-0074 D3.8）と `is_milestone_task` | やめる。述語は `is_root_task`（案件直下・execute・対話でも support でもない）に改名 |
| 途中目標の Go（`milestones_awaiting_go_locked`、ADR-0074 D3.2）・dispatch での `in_progress` / 自動 `reached`（ADR-0077）・途中目標の判定 run（ADR-0038 の `milestone_review::schedule`）・`MilestoneReady` 通知 | やめる（本番に `plan_key` のある途中目標は 0 行なので Go の判定が効いている行は無い）。root の `done` は `TaskReady` で鳴る |
| GUI: 案件ページの DAG・「案件計画を提案させる」「計画を見直す」「この方針で進める」・途中目標カードの作成 / 判定 | 外す。案件ページは root task の一覧（並列、roll-up 付き）と「root task を作る」（`POST /tasks` + `project_id`）と秘書との対話 |
| `kind = plan` の案件の分解 task（ADR-0028 / ADR-0033 D4） | 新しく作らない。既存の行・子はそのまま読める |

**既存の行の扱い**（migration は行を書き換えない。凍結）:
- 途中目標の行（本番 25 行）は状態のまま凍結し、GUI の案件ページの下に「以前の途中目標（読み取り専用）」として出す。`tasks.milestone_id` は履歴として残す。
  paused の途中目標に属する task の dispatch 抑止（ADR-0044 D6）は既存の行に対してだけ残す（新しく paused にする入口は無い）。
- **既存の「マイルストーン Task」（案件直下の task）は普通の root task になる**（合成の root は作らない。互いに依存しないものが大半で、並列に付く root task の
  定義にそのまま合う）。例外は人が木にまとめたいもの（browser・BenchFS）で、それは R5b で root task を作って**採用**（adopt、D15）する。
- ADR-0074 D3.7 の plan/2 の `children` で作られた子（本番 5 件）はそのまま（親の plan/2 の `child:<key>` 待ちで動く）。

### D14. GUI

- **task の木**（task 詳細の新しいタブ「木」と、root task の概要）: 節点 = task（root / 子）、段階ごとにまとめ、leaf は折りたたんだ行。節点に状態・ExecutionPhase
  （`awaiting_children` / `awaiting_plan_approval` / `awaiting_human` を含む）・leaf の done / total・未回答の決定・roll-up（run・quota・定価・壁時計）・
  止まっている理由（D10 の分類）。子を選ぶとその task の詳細へ。モバイル幅は深さで字下げした縦の一覧（ADR-0055、`mobile-audit` 違反 0）。
- **決定の受信箱**: 受信箱に「決定」の節。path のパンくず（「browser capability › Phase 2 › P2-B」）、問い、選択肢（推奨に印）、後戻りの大きさ、待っている unit、
  経過時間。その場で答える（note 任意）。root 計画の承認（D8）も同じ受信箱に「計画の承認」として出す。
- **案件ページ**: root task の一覧（状態・roll-up・未回答の決定の数）、「root task を作る」、秘書との対話、以前の途中目標（読み取り専用）。DAG は出さない。
- **語**: 「配送」→「成果の取り込み」（D6）。「途中目標」は新しい画面では使わず「段階」。
- 表示の判定は GUI の純粋関数（`~/lib/tree.ts`）、押せるかどうかは celeris が決める（409 / 422 の文言を出す。F6 の P7 と同じ流儀）。

### D15. 後方互換

- `tree` を持たない既存の task は深さ 1 の節点として扱う（`root_id` は NULL のまま。埋め戻さない）。/1・/2 の計画・既存の WU（すべて leaf）・委譲の子・
  plan/2 の children は今と同じに走る。`[execution.tree] enabled = false`（既定）では挙動が 1 バイトも変わらない（新しい API は 404 ではなく空を返す）。
- **採用（adopt）**: 既存の task を木の子として取り込む入口 `POST /tasks/{root}/tree/adopt {task_id, stage, unit_key}`（管理系）と `celerisctl tree adopt`。
  条件: 対象は同じ案件、祖先ではない、他の木に属さない、root の計画にその unit が kind task で `adopt: <task_id>` として書かれている。対象が終端（`done`）なら
  unit は `done`、成果が既に main か親ブランチに入っていれば（`merge-base --is-ancestor`）統合は飛ばす（ADR-0074 D1.4 の冪等の規則）。対象の `parent_id` が
  NULL なら親を書き、NULL でなければ（BenchFS の `kind = plan` の子）`parent_id` は書き換えずに unit の `child_task_id` だけで結ぶ（履歴を変えない）。
  `Event::ChildAdopted` を root に残す。
- API / GUI は追加（`Task.tree`、`TaskDetail.tree`、`GET /tasks/{id}/tree`、`/decisions`、`plan-gate`）と D13 の削除（410 / 422）。`api-v1.schema.json` /
  `event.schema.json` / `execution-plan.schema.json` を再生成し、`decision.schema.json` を足す。
- migration 0031（`SCHEMA_VERSION` 30 → 31）: `ALTER TABLE tasks ADD COLUMN root_id TEXT`（+ 索引）、`ALTER TABLE work_units ADD COLUMN child_task_id TEXT`、
  `ALTER TABLE work_units ADD COLUMN needs_decisions_json TEXT NOT NULL DEFAULT '[]'`、`CREATE TABLE decisions …`。すべて events の派生（replay で作り直せる）。
  `SchemaTooNew` の規則どおり旧いバイナリは 30 を開けない。ロールバックは ADR-0040 D2。

### D16. 安全と上限

- **daemon と store に LLM を入れない**（DESIGN 原則 1、CLAUDE.md）: gate・unit の gate・計画の検証・子 task の生成・状態の写し・統合・承認の要否・決定の待ち・
  生存確認・roll-up はすべて決定的な関数と store の読み書き。LLM は planner / worker / reviewer / 秘書の対話 run だけ。
- **循環と暴走の拒否**: unit の依存はトポロジカルソートで循環を拒否。子を作れるのは親の計画の unit からだけで、子は祖先を unit に持てない（採用の検証も同じ）。
  深さは作るときに `root_id` の鎖で数えて `max_depth` を超えたら作らない。同じ unit key の子の作り直しは replan の新しい key を通す（`max_tree_replans` に数える）。
- **費用の上限は木ごと**（`max_tree_runs` / `max_tree_tokens`、D3）。超えたら止めて聞く。dispatch の選択には使わない。
- 子 task の remote / cluster の検証（ADR-0062 D5）、部をまたぐ認可（ADR-0033 / ADR-0074 F4b）、human の acceptance の成果物の参照（ADR-0067 D2）は、
  CoS の `create_task` と同じ検証を子の生成にも通す（抜け道を作らない）。

### D17. SPEC / DESIGN との関係（SPEC.md・DESIGN.md は編集しない）

- SPEC §3.3「案件と、仕事の木（DAG）」: 仕事の木 = **root task の木**。案件の中の依存は root task の中にだけあり、案件の最上段は並列の root task になる。
  §2.3「種々の仕事に分解され、実行される」「どの粒度にも口を出す」= 木のどの節点にもコメント（ADR-0044 D2）・決定・replan ができる。
- SPEC §7「途中目標は予め大まかに決めておいて、適宜再設計する」「途中目標の達成ごとに判定」= root task の段階（`review: human`）と replan。
- SPEC §3.5 / §5「通知は数時間単位」: 鳴るのは root の完了・失敗、決定、承認、障害だけ（D11）。
- **SPEC への提案**（人が判断する。本 ADR は SPEC を変えない）: §3.3 に「案件はごく粗い方向と常設の文脈。仕事の木は案件の中の root task ごとにあり、
  依存はその中にだけある」を、§7 に「途中目標 = root task の段階。人の確認を求める段階だけ止まる」を 1 文ずつ足す。
- DESIGN §5.6（計画が不正なら 1 回だけ再試行、それでも不正なら failed）: /3 では failed でも atomic でもなく決定の要求（D9）。PROGRESS の「提案」に注記を置く。

## 4. 置き換える既存の決定

| ADR | 決定 | 本 ADR での扱い |
|---|---|---|
| ADR-0072 | D14「入れ子の計画は禁止（固定 2 層）」、D18 `max_planning_depth = 1（固定）` | **置き換え**: 子 task を通した再帰、`max_depth = 3`（D1・D3） |
| ADR-0072 | D14「不正な計画は 1 回再試行、それでも不正なら atomic に倒す」 | /3 では**置き換え**（決定の要求、D9）。/1・/2 は維持 |
| ADR-0072 | D13「Task の最初の dispatch で 1 回」 | 各節点で 1 回（子も）、深さで閾値を上げる、木では shadow でも採用（D4）。規則表は維持 |
| ADR-0072 | D22（子 Task は Plan / 委譲だけ、委譲を使う Task は atomic）、D24 | **置き換え**: 子を作る入口は計画の kind task の unit。木の節点では委譲を使わない（D4） |
| ADR-0072 | D2（WorkUnit を子 Task にしない） | leaf について**維持**。子 task にする条件を D4 で明示 |
| ADR-0072 | D12 の「分割し直す / 中止」の自由文の質問 | 木の節点では構造化した決定の要求（D9） |
| ADR-0074 | §2「3 層に固定」、D3.1〜D3.8（案件計画・マイルストーン Task・Go・`auto_advance`・案件 replan・案件ページの DAG・children・互換）、§4 の案件計画の行、§6 R15 の「大きすぎれば children」 | **廃止・置き換え**（D1・D2・D13）。D3.7 の children は /2 の互換としてだけ残る |
| ADR-0074 | D1（工程・統合 WU・鍵 (task, WU)）、D2（途中確認） | **維持し再帰の各段で使う**（統合は子のブランチも merge、D5・D6） |
| ADR-0077 | 途中目標の dispatch での `in_progress`、`auto_advance` の自動 `reached` | **廃止**（D13） |
| ADR-0044 | D1 の `milestone_id` の指定・「途中目標カードにタスクを追加」、D6 の途中目標の中止・一時停止（3 階層） | 新しい途中目標は作らない。中止・一時停止は task の subtree と案件の 2 階層に（D13、R5a） |
| ADR-0048 | D3 の `add_milestone`、`create_task` の目安（1 時間以内なら 1 つ、儀式なら途中目標） | **置き換え**（D12） |
| ADR-0038 | 途中目標の判定の対話（ok / 議論 / ng） | 新しい仕事では段階の `review: human`（続ける / replan / 取り下げ）に移る。既存の行は凍結（D5・D13） |
| ADR-0062 | D5（`create_task` と plan.json の remote の検証） | **維持**し、子 task の生成にも当てる（D4・D16） |
| ADR-0016 | D2 の `max_tree_depth = 5` / `max_tree_runs = 100` | 木の節点では D3 の `max_depth` / `max_tree_runs` が効く（委譲の上限は木でない task の委譲にだけ残る） |

## 5. 今日の事象との対応

| 起きたこと | 何が防ぐか |
|---|---|
| CoS が「Phase 1〜4」を「設計 + Phase 1 MVP」の 1 task に狭めた（T1） | D12: 1 依頼 = 1 root task、CoS は範囲を狭めず大きさを判断しない、人が名指しした段階は `stages_hint` に写す。D4: 大きさは root の gate と planner が決め、Phase は root の段階になる。`add_milestone` という第 2 の選択肢を消す |
| 実行の工程（adopt / harden / ship）と製品の Phase が同じ「phase」に混ざった（T2） | D1・D2: 段階は 1 種類で、製品の Phase は root の段階、実行の工程は子 task の中の段階。深さが違うので混ざらない |
| Phase 2〜4 と H1〜H7 が設計の成果物の表に落ちて行き場が無かった（T2） | D2: planner は kind task の unit と `decisions` を計画に書ける。D7: 決定は path 付きで受信箱と Discord に出て、答えが依存する unit の目的に入る。D8: 決定を含む root 計画は人が承認する |
| 案件計画が一度も使われず、案件が粗い入れ物になっていた（T3・T4） | D13: 案件計画を廃止し、案件は方向と常設の文脈だけと定義し直す（実態に定義を合わせる）。途中目標の 1:1 の自動作成をやめる |
| 最終レビュー中に main が進んで merge_base の repair と replan が要った（T5） | D6: 子は親のブランチに取り込み、親のブランチは段階の途中で動かない。main と比べるのは root だけ |
| 依存 WU の準備の失敗が 20 分無音、案件計画 run の基盤の失敗の後に何も起きない（T3・T5） | D10: 木の生存確認（走っている / 走れる / 名指しの待ち のどれでもなければ障害通知）。D9: 基盤の失敗は自動回復し、回復できなければ障害通知で、質問にしない。D5: 子待ちの親に `awaiting_children` の理由を出す |

## 6. 採らない（代替案と却下の理由）

- **R1: 案件と task の間に「取り組み」「計画」などの専用の中間層を足す**。人の決定 1。層ごとに状態・承認・報告・GUI が要り、今日の「案件計画が使われない」の再演になる。
- **R2: WorkUnit に計画を持たせて再帰する（leaf の再分割）**。worktree・レビュー・取り込みを WU ごとに持たせることになり ADR-0072 R1 に戻る。再帰は task で行う。
- **R3: 子 task も main に直接取り込む**。子ごとに main と比べて merge-base がずれ、兄弟の成果が main 経由でしか見えない。人の決定 4。
- **R4: 上限を超えた unit を leaf として走らせる / atomic に倒す**。黙って範囲が縮む（T1 の形）。人の決定 2。
- **R5: 決定点を `approvals` に入れる**。approvals は「今回だけ / 今後ずっと」の認可（SPEC §3.6）で、選択肢・推奨・後戻りの大きさ・待つ unit を持たない。
- **R6: 決定を `QuestionRaised`（自由文の質問）で出す**。task 全体を止める。答えの無い決定が関係の無い unit を止めないこと（人の決定 3）が満たせない。
- **R7: CoS に大きさを判断させて複数の task / 途中目標に分けさせる**。T1 の原因そのもの。判断は決定的な gate と task-local の planner に置く（ADR-0072 R5 と同じ理由）。
- **R8: 既存のマイルストーン Task を合成の root の下にまとめる**。互いに独立なものが大半で、合成の root は目的も受け入れ条件も持てない。木にしたいものだけを人が採用する（D13・D15）。
- **R9: `max_depth` を大きくする（委譲の 5 に揃える）**。深い木は人が見て方針の異常に気づけない（SPEC §3.3）。3 で足りない依頼は決定の要求で人に見せる。
- **R10: 子待ちの親を `Blocked` にする**。受信箱が「人が何かすべき」で埋まる。`Ready` + 導出の `awaiting_children` と生存確認で足りる（D5・D10）。
- **R11: 費用の上限を金額・quota で持ち dispatch を変える**。予算管理は別プロジェクト（CLAUDE.md、ADR-0074 R12）。

## 7. 実装計画（R1a〜R5b）

各 Phase の完了時に `cargo fmt --all -- --check` / `cargo test --workspace` / `cargo clippy --workspace --all-targets -- -D warnings` を通す。GUI を触る Phase は
`pnpm typecheck` / `lint` / `test` / `gen:types` の差分ゼロ / `mobile-audit` 違反 0 / `pnpm build`。テストは偽のアダプタ・一時ディレクトリの git・スタブの CLI
だけで、外部ネットワークに出ない。**1 Phase = 1 セッション**に収まる大きさにする（人の常設の規則）。`[execution.tree] enabled` は R5b まで既定 `false`。

| Phase | 範囲 | 大きさ | 依存 |
|---|---|---|---|
| R1a | plan/3 の型と検証（純粋関数）、`Task.tree`、migration 0031、新しい Event、`[execution.tree]` の設定 | M | — |
| R1b | daemon: unit から子 task を作る、子の状態の写し、段階の完了（子を含む）、`awaiting_children`、subtree の中止の連鎖、木での委譲の禁止 | M | R1a |
| R1c | 親ブランチへの取り込み: 子の基点、統合 WU が子のブランチを merge、子の最終レビューの基点、子の取り込み（配送）の抑止、root だけ main | M | R1b |
| R2a | 再帰の gate: 深さの閾値、木では shadow でも採用、unit の gate（上げる / 下げる / 決定）と不一致の記録、木の上限と超過の決定の要求 | M | R1b |
| R2b | planner のプロンプト（深さ・残りの深さ・leaf の基準・/3・木の残りの上限）、/3 の不正 2 回で決定の要求、子の失敗 → 親の replan、子の基盤の失敗の再試行と障害通知 | M | R2a |
| R3a | 決定の要求: 回答 API・MCP・revise、答え → 待つ unit と目的への注入、worker の `decisions`、受信箱、Discord 通知（束ね・再通知） | M | R1a、R2a |
| R3b | root 計画の承認の規則（`PlanGate`・plan-gate API・通知）、木の生存確認（`StallDetected`・障害通知）、子の完了・失敗の通知の抑止 | M | R3a |
| R4a | API: `GET /tasks/{id}/tree`・roll-up の純粋関数・`GET /projects/{id}` の和・`metrics/execution?group_by=depth`・報告の束ね（子の要約） | S〜M | R1c、R3a |
| R4b | GUI: 木のタブ、決定と承認の受信箱、案件ページ（root の一覧）、語（配送 → 成果の取り込み）、モバイル | M | R4a、R3b |
| R5a | 案件モデル: project-plan API・GUI・celerisctl の削除（410 / 422）、途中目標の自動作成と Go / ADR-0077 / 判定 run の停止、`is_root_task`、subtree の一時停止、CoS の preamble（D12）と `add_milestone` の廃止 | M | R1b（CoS 文面は R2b の後） |
| R5b | 本番の移行と dogfood: `tree adopt`、browser の root（Phase 1 = 既存の done task を採用、Phase 2〜4 = 子 task、H1〜H7 = 決定）、BenchFS の root（「国際会議フルペーパー化」、止まっている framing を決定に）、`enabled = true`、Phase 2 の子を 1 本通す | M（実機） | すべて |

### R1a: plan/3 とデータモデル（migration 0031）

- **範囲**: `task_core::execution_plan` に `celeris.execution-plan/3`（`stages` / `units` / `decisions`、`WorkUnitKind::Task`、leaf の基準の検証、kind task の
  必須欄と禁止欄、`child:` / `children` の拒否、決定の形と上限）。`task_core::tree`（`TreeInfo`、`TreeLimits`、深さの計算）、`task_core::decision`（型と検証）。
  `Task.tree`（serde(default)）。Event: `ChildTaskCreated` / `ChildAdopted` / `UnitGateOverridden` / `DecisionRequested` / `DecisionAnswered` / `DecisionWithdrawn` /
  `PlanApprovalRequested` / `StallDetected`（型と replay の読みだけ。発行は後の Phase）。migration 0031。`[execution.tree]` の設定（R5b まで `enabled = false`）。
  schema の再生成。
- **受け入れ条件**:
  - (a) /3 の fixture が通り、/1・/2 の既存 fixture の検証結果と出力 JSON が変わらない（`plan_v1_and_v2_fixtures_are_byte_identical`）。
  - (b) 拒否: leaf に `checks` が無い / `context.repo` が 2 / kind task に `checks` / kind task に `acceptance` 無し / `child:` 依存 / `children` / 循環 / 未知の
    `needed_before` / `needs_decisions` が未知の key / 段階あたり 7 unit / 決定 9 件（`rejects_*` のテスト 10 本）。
  - (c) `enabled = false` で /3 は `TreeDisabled` で拒否される（`v3_is_rejected_when_tree_is_disabled`）。
  - (d) migration 0031 が 30 の DB に当たり、`rebuild_work_units_and_runs` と `decisions` の再構築で events から同じ行ができる（`replay_rebuilds_decisions_and_child_links`）。
  - (e) `event.schema.json` / `execution-plan.schema.json` / `decision.schema.json` の一致テスト。
- **テスト**: 上記。**触るファイル**: `crates/task-core/src/{execution_plan.rs, tree.rs(新), decision.rs(新), model.rs, event.rs, store.rs}`、`crates/task-core/migrations/0031_task_tree.sql`、
  `crates/celeris/src/config.rs`、`docs/protocol/*.schema.json`。

### R1b: 子 task の生成と段階の完了

- **範囲**: dispatcher の WU の段で kind task の unit を扱う: ready になったら子 task を作る（D4 (4) の 1 トランザクション）、子の状態を unit に写す（D4 (5)）、
  段階の完了の条件に子を含める（統合は R1c まで「子の done で完了」の仮の規則、R1c で merge に置き換える）、親の `Skip` と `ExecutionPhase::AwaitingChildren`、
  `max_parallel_child_tasks`、root の中止で subtree を連鎖して中止（worktree とブランチの削除は ADR-0043 D2）、木の節点の run に `available_genres` を渡さない。
- **受け入れ条件**:
  - (a) 偽の planner が /3（leaf 1 + task 1）を出すと、leaf が走り、task の unit が ready で子 task（`parent_id`・`tree.depth = 2`・`root_id`・`labels: child-<key>`・
    `status: ready`・親の repos / workspace を継ぐ）が 1 トランザクションで作られる（`v3_task_unit_creates_child_when_ready`）。
  - (b) 依存・`needs_decisions` が満たされるまで子は作られない（`child_waits_for_dependencies_and_decisions`）。
  - (c) 子が done → unit done、子が failed → unit failed、子が cancelled → unit cancelled（`unit_mirrors_child_status`）。
  - (d) 親が子だけを待つ間は `Ready` のまま dispatch されず、`GET /tasks/{id}/execution` の phase が `awaiting_children`（`parent_waits_without_lease`）。
  - (e) root の Cancel で子・孫の非終端 task が `cancelled`（reason `parent_cancelled`）になり、走っている run が止まる（`cancel_cascades_to_subtree`）。
  - (f) 木の節点の run の `RunContext.available_genres` が空（`tree_runs_cannot_delegate`）。
  - (g) `enabled = false` と /2 の children の既存テストが変わらない。
- **触るファイル**: `crates/task-dispatch/src/dispatcher.rs`（WU の段）、`crates/task-ops/src/{tree.rs(新), add.rs, cancel.rs}`、`crates/task-core/src/execution.rs`（phase）。

### R1c: 親ブランチへの取り込み

- **範囲**: 子の `base_commit` の決定（段階の基点、同じ段階の依存は依存先の HEAD。`dependency_base` を子のブランチに広げる）、`worktree_base` の木の子の分岐、
  統合 WU が `celeris/<child_id>` を merge（冪等、既に入っていれば飛ばす）、子の最終レビューの差分の基点と merge-base の相手を親のブランチに、子の `deliveries`・
  intake の抑止、子の worktree の後片付け、`TaskReady` を root だけに。
- **受け入れ条件**:
  - (a) 一時 git で: 段階 S に leaf a と子 c。c のブランチは S の基点から切られ、`integrate-S` が a と c を親ブランチに merge し、`PhaseIntegrated.merged` に c と commit が残る（`integration_merges_child_task_branch`）。
  - (b) 同じ段階で c に依存する leaf は c の HEAD から切られる（`same_stage_dependency_on_child_bases_on_child_head`）。
  - (c) 子の最終レビュー中に main が進んでも子の merge-base の検査は落ちない（親ブランチと比べる）（`child_review_ignores_main_moving`）。
  - (d) 子の done で `deliveries` の行も取り込みの通知も作られず、root の done だけが ADR-0051 の取り込みに進む（`only_root_delivers_to_main`）。
  - (e) 採用した done の子の成果が既に main にあるとき統合は merge を飛ばす（`adopted_child_already_in_base_is_skipped`）。
- **触るファイル**: `crates/task-dispatch/src/{integration.rs, dispatcher.rs}`、`crates/celeris/src/delivery.rs`、`crates/task-worker/src/review.rs`（基点）、`crates/celeris/src/notify.rs`。

### R2a: 再帰の gate と木の上限

- **範囲**: `execution_gate::decide` に深さの閾値、木の節点で shadow を採用に、`task_core::tree::unit_gate` と採用時の適用（上げる / 下げる / 決定）、
  `UnitGateOverridden`、木の上限（`max_tree_leaves` / `max_tree_runs` / `max_tree_replans` / `max_tree_tokens`）の決定的な数え上げ、超過で `kind: limit` の決定の要求と
  その節点の新しい run の停止（兄弟は続く）。
- **受け入れ条件**:
  - (a) 深さ 2 の子の閾値が 7 になり、score 6 の子は atomic、score 7 は compound（`gate_threshold_rises_with_depth`）。
  - (b) `gate = "shadow"` でも木の子の compound は planner に進み、root の compound（規則表）は shadow のまま記録だけ（`tree_nodes_adopt_gate_even_in_shadow`）。
  - (c) unit の gate の表（D4 (3)）の 7 行がそれぞれ期待どおり（`unit_gate_table`）。深さ 2 の compound の leaf は決定の要求になり、他の unit は進む（`leaf_too_large_at_max_depth_raises_decision`）。
  - (d) `max_tree_runs` を超えた subtree は新しい run を起こさず `kind: limit` の決定が 1 件出る。兄弟の subtree は走る（`tree_limit_breach_stops_only_that_subtree`）。
  - (e) 回答「今回だけ上げる」で同じ subtree が続きから走る（`limit_raise_answer_resumes_subtree`）。
- **触るファイル**: `crates/task-core/src/{execution_gate.rs, tree.rs}`、`crates/task-dispatch/src/dispatcher.rs`。

### R2b: planner と replan の再帰

- **範囲**: planner のプロンプト（/3 の形、深さ・残りの深さ・leaf の基準・木の残りの上限・祖先の path・`stages_hint`）と検証の上限の共有（F5-fix3 と同じ 1 か所）、
  /3 の計画が 2 回不正なら `kind: plan_invalid` の決定、子の failed（work）→ 親の replan（理由と子の要約を planner の入力に）、子の failed（infra / supply）→ 子を 1 回複製して再試行、
  それでも失敗なら障害通知と unit `blocked(infra)`。
- **受け入れ条件**:
  - (a) プロンプトのスナップショットに深さ・残りの深さ・leaf の基準・上限が出て、`remaining_depth = 0` では kind task を書くなと出る（`planner_prompt_carries_depth_and_leaf_criteria`）。
  - (b) 偽の planner が 2 回不正な /3 を出すと atomic に倒れず `plan_invalid` の決定が出る。/2 は従来どおり atomic に倒れる（`v3_invalid_plan_asks_instead_of_atomic`）。
  - (c) 子が work で failed → 親の planner（replan）が起き、その入力に子の失敗の要約がある。新しい版の子が作られ、元の子のブランチは残る（`child_failure_triggers_parent_replan`）。
  - (d) 子が infra で failed → 1 回だけ複製され、2 回目の失敗で `TaskFailed`（基盤の分類）と unit `blocked(infra)`、決定の要求は作られない（`child_infra_failure_is_not_a_question`）。
- **触るファイル**: `crates/task-worker/src/claude_code.rs`（planner のプロンプト）、`crates/task-dispatch/src/dispatcher.rs`、`crates/task-ops/src/retry.rs`。

### R3a: 決定の要求の流れ

- **範囲**: `POST /decisions/{id}/answer`・`revise`・`GET /decisions?status=&root=`、MCP `decision_answer`（`tasks:interact`）、worker の `result.json` / `checkpoint.json` の
  `decisions`（`needed_before: self` はその unit を blocked）、回答 → 待つ unit の ready と objective / 前置きへの固定の書式での注入、受信箱の `AttentionItem::Decision`、
  `NotificationKind::DecisionRequested`（束ね・24 時間後の再通知 1 回）。
- **受け入れ条件**:
  - (a) 計画の決定 2 件（h1 は p2-b、h2 は stage:phase-2）で、決定に依存しない unit は走り、p2-b と phase-2 は待つ（`unanswered_decision_blocks_only_dependents`）。
  - (b) 回答で p2-b が ready になり、作られた子の objective の末尾に固定の書式で回答が入る（`answer_flows_into_child_objective`）。leaf では前置きの「人の決定」節に入る。
  - (c) worker の `decisions` が path 付きで記録され、`self` だけがその unit を止める（`worker_declared_decisions`）。
  - (d) 計画の採用で出た 3 件は通知 1 通に束ねられ、Discord への送信本文に path と推奨がある（偽の webhook。`decisions_are_notified_once_per_plan`）。
  - (e) API: トークン無し 401、無い id 404、回答済み 409、未知の option 422。MCP の scope 違反 403。
- **触るファイル**: `crates/task-api/src/{decisions.rs(新), inbox.rs}`、`crates/celeris-mcp/src/*`、`crates/task-ops/src/decision.rs(新)`、`crates/task-worker/src/{result_report.rs, preamble.rs}`、
  `crates/task-core/src/notify.rs`、`crates/celeris/src/notify.rs`。

### R3b: root 計画の承認と生存確認

- **範囲**: `approval_required` の計算（D8）、`Trigger::PlanGate` と `PhaseResume` の再利用、`POST /tasks/{id}/execution/plan-gate`、`AttentionItem::PlanApproval`、
  `NotificationKind::PlanApproval`、承認不要のときの報告 1 件、`task_core::tree::liveness` と `StallDetected`・障害通知、子の完了・子の失敗（親が吸収するもの）の通知の抑止。
- **受け入れ条件**:
  - (a) 決定を含む root の /3 は `awaiting_plan_approval` で止まり、approve で unit が起き、replan（note）で planner、withdraw で Cancel（`root_plan_with_decisions_needs_approval`）。
  - (b) 決定も `review: human` も無く上限の 0.8 未満の root の /3 は承認なしで進み、報告が 1 件だけ残る（`small_root_plan_proceeds_with_notice`）。子の計画は決定を含んでも承認を求めない。
  - (c) 生存確認: 準備の失敗を繰り返す unit・子待ちで子が消えた親・計画 run が基盤の失敗で終わった後に何も起きない task を、偽の時計で 600 秒後に `StallDetected` と
    障害通知 1 件にする。名指しの待ち（決定・承認・子）は検出しない（`liveness_flags_only_unexplained_stalls`）。
  - (d) 子の done・親が replan で吸収した子の failed では通知が鳴らない（`child_events_do_not_notify`）。
- **触るファイル**: `crates/task-core/src/{transition.rs, tree.rs, notify.rs}`、`crates/task-dispatch/src/dispatcher.rs`、`crates/task-api/src/execution.rs`、`crates/celeris/src/notify.rs`。

### R4a: 木と roll-up の API

- **範囲**: `task_core::tree_metrics::rollup`、`GET /tasks/{id}/tree`、`TaskDetail.tree`、`GET /projects/{id}` の root ごとの和、`GET /metrics/execution?group_by=depth`、
  段階の途中報告への子の要約の行。
- **受け入れ条件**:
  - (a) 3 段の fixture で、各節点の subtree の run・トークン・定価（`cost_usd_complete` の伝播）・quota・壁時計・leaf の done / total・未回答の決定が手計算と一致（`rollup_matches_hand_computed_fixture`）。
  - (b) `GET /tasks/{id}/tree` が木を返し、`enabled = false` の旧い task は 1 節点の木（`legacy_task_is_a_single_node_tree`）。
  - (c) 途中報告に子の要約の行があり、16 KiB の切り詰め（SD-3 の実装）を通る。
  - (d) `api-v1.schema.json` の差分が追加だけ。
- **触るファイル**: `crates/task-core/src/tree_metrics.rs(新)`、`crates/task-api/src/{execution.rs, types.rs, projects.rs}`、`crates/task-ops/src/phase_report.rs`。

### R4b: GUI

- **範囲**: task 詳細の「木」タブ、root の概要の木の要約、受信箱の「決定」「計画の承認」、案件ページ（root の一覧・「root task を作る」・以前の途中目標）、
  「配送」→「成果の取り込み」、`~/lib/tree.ts`、モバイル。
- **受け入れ条件**:
  - (a) `tree.ts` の単体テスト（止まっている理由の文言、パンくず、押せるボタンの出し分け）。
  - (b) 画面のテスト: 決定に答えると API に `option` と `note` が送られ、409 / 422 の文言が出る。計画の承認の 3 ボタン。
  - (c) `gen:types` の差分ゼロ（2 回）、`mobile-audit` 違反 0、`pnpm build`。
  - (d) GUI に「配送」の語が残らない（`help.tsx` を含む。テストで grep）。
- **触るファイル**: `gui/app/routes/{tasks.$id.tsx, projects.$id.tsx, inbox*.tsx, help.tsx}`、`gui/app/lib/tree.ts(新)`、`gui/app/components/*`。

### R5a: 案件モデルの変更と CoS の指針

- **範囲**: D13 の削除（410 / 422、`task_ops::project_plan` の撤去、celerisctl の `projects plan`、GUI の案件計画の入口と DAG）、`task_ops::add` の途中目標の自動作成の停止と
  `is_root_task`、途中目標の Go・ADR-0077・判定 run・`MilestoneReady` の停止、subtree の `POST /tasks/{id}/pause|resume`（root の木ごと、`ready_tasks` が `root_id` の一時停止を見る）、
  CoS の preamble（D12）、`add_milestone` を検証で落とす、`stages_hint` を `create_task` に足す。
- **受け入れ条件**:
  - (a) 消した API がすべて 410 / 422 を返し、`GET /projects/{id}` と `GET /milestones` は読める（`project_plan_endpoints_are_gone`）。
  - (b) 案件直下に作った task に途中目標の行ができない。既存の途中目標の行は状態が変わらない（`no_milestone_rows_for_new_root_tasks`）。
  - (c) root の pause で子孫が dispatch されず、resume で戻る（`subtree_pause_stops_descendants`）。
  - (d) CoS の preamble のスナップショット: 1 依頼 = 1 `create_task`、範囲を狭めない、`stages_hint`、`add_milestone` と `execution: compound` の記述が無い。`add_milestone` の action は
    理由付きで落ち、人に見える（`add_milestone_is_retired`）。
  - (e) 既存の案件（Pluvio の 2 件）の GUI が壊れない（以前の途中目標が読み取り専用で出る）。
- **触るファイル**: `crates/task-api/src/{project_plan.rs, handlers.rs, lifecycle.rs}`、`crates/task-ops/src/{project_plan.rs, add.rs, actions.rs, milestone_review.rs}`、
  `crates/task-core/src/{store.rs(ready_tasks), console_action.rs}`、`crates/task-worker/src/preamble.rs`、`crates/celerisctl/src/*`、`gui/app/routes/projects.$id.tsx`。

### R5b: 本番の移行と dogfood

- **範囲**（本番の DB と設定を変えるので、**各手順の実行は人の Go の後**。エージェントは手順・計画の JSON・確認の SQL を用意し、人の指示で実行して証跡を残す）:
  1. release を昇格し、`[execution.tree] enabled = true`（idle のときに再起動。P-F-1 と同じ手順）。
  2. **browser**: root task「browser capability（Phase 1〜4）」を案件 agent-platform に作り、人（origin human）の /3 の計画を `PUT` する: 段階 phase-1〜4。phase-1 の unit p1 は
     `adopt: 01M3MFS5T52FXA63W4V10XGC4S`（done、成果は main 09a6b0c に取り込み済み → 統合は飛ばす）。phase-2〜4 はそれぞれ**1 つの子 task**（p2 / p3 / p4）にし、
     その目的に `phases-and-decisions.md` の P2-A〜C・P3-A〜C・P4-A〜C の表（依存・検査・現在の扱い）をそのまま写す。子の中の P2-A〜C は子の planner が
     leaf として作る（`max_depth = 3` なので孫 task は無い。leaf に収まらないものは D4 の決定の要求になる）。
     決定 h1〜h7 を `phases-and-decisions.md` の表から写す（`needed_before`: h1 → p2、h2 → p2、h3 → p2、h4 → p3、h5 → p3、h6 → p2、h7 → p4）。`review: human` は phase-2 の後。
     決定を含むので D8 の承認に出る。人が承認し、決定のうち答えられるものに答える。
  3. **BenchFS**: root task「国際会議フルペーパー化」を案件 BenchFS に作り、/3 の計画: 段階「棚卸しと関連研究」（既存の done の Phase0 / Phase1 の task を採用。`parent_id` は
     `kind = plan` の 01M35X04345ZNDM09VE6FT168Z のまま）、段階「framing の選択」（決定 `framing`: 選択肢は採用する task 01M35X86XTK84F97QW0CN5PGMR の framing 候補の成果物から、
     推奨はその成果物の推奨）、以後の段階（実験計画 → 実装 → 実験 → 執筆）は子 task（決定 `framing` に依存）。
  4. dogfood: browser の phase-2 の子を 1 本 done まで通す（子の gate・子の計画・leaf・親ブランチへの取り込み・決定の回答の注入・roll-up を実機で観測）。
  5. 途中目標の行・案件計画の GUI が本番で見えないこと、`POST /projects/{id}/plan` が 410 であることを確かめる。
- **受け入れ条件**:
  - (a) 本番の `GET /tasks/<browser root>/tree` に 4 段階、phase-1 が done（採用、統合は skip）、未回答の決定が受信箱と Discord に path 付きで出た証跡。
  - (b) phase-2 の子が親ブランチ `celeris/<browser root>` に取り込まれ（`phase_integrated.merged` に子）、子の `deliveries` が無く、main は動いていない。
  - (c) roll-up の run・quota・壁時計が子の値の和と一致（SQL の照会の出力）。
  - (d) 人に届いたものの内訳（決定・承認・障害）と、基盤の失敗が質問として届かなかったことの events の照会。
  - (e) BenchFS の root の決定 `framing` が受信箱に出ている。
- **大きさ**: M（実機の待ち時間を含む。コードの変更は採用の入口の不具合修正だけの想定）。

## 8. 未解決事項（R0 時点。既定を選んで記録する）

- **U-R1（max_depth の数え方）**: 「max_depth = 3（root → 子 task → leaf）」を、task の段が 2（root と子）で leaf が 3 段目と読んだ。したがって既定では**子 task は子 task を持てない**
  （孫 task は無い）。browser の P2-A〜C は Phase 2 の子の leaf になる。「task の段が 3（孫 task まで）」の意味なら `max_depth` の定義を 1 つずらす（R1a の前に確認したい）。
- **U-R2（部をまたぐ子の認可）**: 子 task が別の部署に当たるとき、ADR-0074 F4b の approvals（人に聞く）を残すか、root 計画の承認（D8）に含めて一度で済ませるか。既定は前者（SPEC §3.1）。
- **U-R3（承認不要の計画の通知）**: 承認を挟まない root の計画を報告の流れに残すだけ（Discord は鳴らさない）で良いか。
- **U-R4（上限の既定値）**: `max_tree_leaves = 40`・`max_tree_runs = 120`・`max_units_per_stage = 6`・`max_parallel_child_tasks = 2` は推測の初期値。R5b の dogfood の後に見直す。
- **U-R5（`gate` の既定）**: 木の子は shadow でも gate を採用する（D4）が、root は `[execution] gate` のまま。本番は 17:06Z から `gate = "on"`。root を常に gate するかは人の判断。
- **U-R6（`POST /plans` と `kind = plan`）**: ADR-0028 の `POST /plans`（Plan kind の分解）は本 ADR では消さない（案件に付かない経路のため）。一本化するなら R5a で 410 にする。
- **U-R7（子の最終レビューの費用）**: 子ごとに reviewer run が 1 本増える。子の数 × reviewer の費用が大きければ、子の最終レビューを決定的な検査だけにする選択肢がある（既定は reviewer あり）。
- **U-R8（既存の途中目標の非終端の行）**: agent-platform の approved / in_progress の行は凍結のまま表示する。人が一括で `cancelled` にしたいなら R5b の手順に入れる。

## 付記: R0 の未決点への人の決定（2026-09-28）

- **U-R1 深さの数え方**: `max_depth` は **task の層数**で数える（根 = 1、子 = 2、孫 = 3。葉はどの層にもぶら下がり、深さに数えない）。D3 の「根 → 子 task → 葉」は「根 → 子 → 孫 → 各層に葉」に改める。
- **U-R2**: 部署をまたぐ子 task の認可は当面今のまま（根の計画承認に畳まない）。
- **U-R3**: 承認不要の計画は通知しない（report stream への記録のみ）。
- **U-R4**: 上限の既定値は草案どおり（葉 40 / run 120 / 段階あたり単位 6 / 同時実行の子 2）。dogfood（R5b）で調整する。
- **U-R5**: 根の gate は `[execution] gate` の設定に従う。
- **U-R6**: ADR-0028 の `POST /plans` も 410 にする（R5a）。
- **U-R7**: 子 task ごとの reviewer run は当面許容。ただし review 数が増えすぎる恐れがあるので、**深さ別・部分木別の review run 数と費用を指標として出す**（R4a の集約に含める）。本格的な Prometheus metrics の導入は将来課題として棚上げ。
- **U-R8**: agent-platform の未終了の途中目標行は凍結し、将来的に GUI から見えないようにする（R5a で凍結 + 既定非表示）。
- **追加: R6 回収フェーズ**: 仕様変更に伴い、既存の案件・task の中身（案件の方向性文、途中目標、親子関係、知識の置き場）を新モデルの実情に合わせて整える回収フェーズを R5b の後に置く。

## 付記: R1a 実装時の逸脱・明確化（2026-09-28）

R1a（plan/3 の型と検証、`Task.tree`、migration 0031、Event、`[execution.tree]`）で決めたこと。本文の決定は変えていない。

1. **U-R1 の数え方に合わせた式の読み替え**: `max_depth` は task の層数（root 1 / 子 2 / 孫 3）。深さ `d` の task の計画が kind task の unit を
   持てるのは **`d < max_depth`**（D3 の表の「`d + 1 < max_depth`」を置き換える）。planner に渡す `remaining_depth` は **`max_depth − d`**
   （D4 (2) の「`max_depth − depth − 1`」を置き換える。1 以上なら kind task を書ける）。gate の閾値 `5 + step × (d − 1)` は変えない。
   実装は `task_core::tree::{can_have_child_tasks, remaining_depth, gate_threshold}`。検証は `validate_with(.., PlanContext{origin, depth})` の
   `depth`（`tree::depth_of(task)`、`tree` の無い task は 1）で見る（`ChildTaskTooDeep`）。
2. **`tasks.root_id` は埋め戻さない**: migration 0031 は列・表・索引を足すだけで既存の行を書き換えない（D13「凍結」、D15「root_id は NULL のまま」）。
   列は `Task.tree.root_id` の写しで、`tree` を持つ task（R1b で作る子と、その root）にだけ入る。root の `tree` を誰がいつ書くか（root 自身の
   `root_id = id`）は R1b が子を作るときに決める。
3. **型**: /3 は `ExecutionPlanSpec` の同じ型に `stages` / `units` / `decisions` を足した（空なら出力しない。/1・/2 の JSON は 1 バイトも変わらない）。
   `work_units` は `serde(default)` にした（/3 は書かない）。そのため /1・/2 で `work_units` を省いた JSON は、parse の失敗ではなく検証の
   `NoWorkUnits` で拒否される（拒否されることは変わらない）。/3 を直列化すると空の `phases` / `work_units` / `children` が付く（読み戻しは同じ値）。
   unit は /2 の `WorkUnitSpec` とは別の型 `PlanUnitSpec`（`stage`・kind task の欄・leaf の欄・`needs_decisions`・`decisions`・`adopt`）で、
   `internal_view` が /2 の形（`phases` / `work_units`、`work_units.phase` = 段階の key）に写す。統合 WU・工程の障壁・replay はこの写しを使う。
   leaf の `context.repo` は文字列か配列（`RepoSelector`。2 つ以上なら `LeafMultipleRepos`）。
4. **検証の細部**: kind task の unit は `checks` / `budget` / `harness` / `context.paths` を拒否（D2 の列挙どおり。`done_when` / `outputs` は拒否しない）。
   leaf は kind task 専用の欄（`acceptance` / `genre` / `skills` / `repos` / `adopt`）を拒否。/3 の leaf の予算は **丸めずに拒否**（D4 (2) の
   「上限超過 → 拒否」。/1・/2 は従来どおり丸める）。段階の kind に `task` / `integrate` は使えない。unit の `decisions` は `needed_before` に
   その unit を足して計画の決定に並べる（`normalized_decisions`）。決定の上限（計画あたり 8）は unit 側を含めた総数で数える。`needed_before` に
   `self` は計画では書けない（worker 専用。R3a）。`repos` が親の repos の部分集合かは R1b（子の生成で親を知るとき）に検査する。
5. **`[execution.tree]` の値の持ち方**: `ExecutionLimits.tree: TreeLimits`（/3 の検証だけが見る）。`approval_near_limit_ratio`（設定は 0 < r ≤ 1）は
   `ExecutionLimits` を `Eq` のまま保つため千分率 `approval_near_limit_permille` で持つ。`max_open_decisions`（木あたり）と
   `max_open_decisions_per_plan` を別の欄にした（D3 の「12 / 木、8 / 計画」）。`max_depth` は `1..=3` 以外なら設定エラー。
   daemon（dispatcher の planner 検証）は設定の値を使うが、**API の `PUT/POST /tasks/{id}/execution-plan` と `celerisctl execution` は従来どおり
   `ExecutionLimits::default()`（tree 無効）**で検証する。R5b で人が /3 を `PUT` する前に、この 2 つの入口へ設定を配線する（R1b〜R5b のどこかで。
   `enabled = false` の間は挙動が同じなので R1a では変えない）。
6. **Event の欄**: `ChildTaskCreated` / `ChildAdopted` に `plan_id` を足した（`work_units` の行を events だけから結び付けるため）。
   `UnitGateOverridden` は `{plan_id, unit_key, declared, gate, action, depth, threshold}`。`DecisionRequest` に `answer` / `withdrawn_reason`
   （表の `json` に回答・取り下げを残すため。どちらも省略可）と `DecisionOrigin::Human`（人の計画の決定）を足した。`decisions.root_id` は
   `path` の先頭（空なら出した節点）。`Created{origin: plan_unit}`（D4 (4)）は子の生成と一緒に R1b で足す。
7. **派生の書き込みの場所**: `work_units.child_task_id` と `decisions` は、store が Event を追記するのと同じトランザクションで書く
   （`append_event_tx` と遷移の `extra_events` の両方から `apply_tree_event_tx`。F5-fix3 の `close_run_row_for_event_tx` と同じ形）。畳み込みは
   `task_core::DecisionRow::{from_request, apply_answer, apply_withdrawal}` を store と `task_ops::replay::rebuild_decisions` が共有する。
   `celerisctl replay --check/--apply` は `decisions` も突き合わせる（`DECISION_MISMATCH`）。`work_units.needs_decisions_json` は採用時に
   `effective_needs_decisions`（`needs_decisions` と、その unit か `stage:<その段階>` を `needed_before` に持つ決定）で決まる。
8. **daemon の挙動は変えない**: `enabled = false`（既定）では /3 は `TreeDisabled` の 1 件だけで拒否され（planner の出力・人の PUT とも）、他は
   何も変わらない。保険として、scheduler（`runnable_work_units`）は kind task の行を LLM run の候補にしない（子の生成は R1b）。
   `review: human` の段階を途中確認（`pause_after`）に解決するのは R1b 以降（R1a の採用では `PausePointsResolved` は段階を拾わない）。

## 付記: R1b 実装時の逸脱・明確化（2026-09-28）

R1b（kind task の unit からの子 task の生成、状態の写し、段階の完了、`awaiting_children`、subtree の中止の連鎖、木での委譲の禁止、
`review: human` の段階の途中確認）で決めたこと。migration は足していない（schema 31 のまま）。

1. **照合の場所**: 子の生成と状態の写しは dispatcher の tick の照合 `reconcile_tree_units`（`drain_completions` の後、dispatch の前）で行う。
   終わっていない kind task の unit を持つ task だけを `TaskStore::tasks_with_open_task_units` で引く（木が無ければ空で何もしない）。
   子の生成は `TaskStore::tree_child_create` の 1 トランザクション（子の挿入 + `Created{origin: plan_unit}`、unit の行を `running`・
   `child_task_id` に、親の events に `WorkUnitTransitioned{reason: child_created}` と `ChildTaskCreated`。unit が今も `ready` で子を持たない
   ことを同じトランザクションで確かめる〈冪等〉）。写しは子の遷移と同じトランザクションではなく次の tick の照合で、unit の遷移 1 件ごとに
   `WorkUnitTransitioned`（reason `child_done` / `child_failed` / `child_cancelled`）を積む（replay は events だけから同じ行を作る）。
2. **子の `cancelled` は unit を `failed` にする**（D4 (5)・§7 R1b (c) の「unit `cancelled`」からの逸脱）。`cancelled` の unit は計画の実行から
   外れる（`WorkUnitStatus::is_active` でない）ので、段階の完了の判定（`settle_phase`）から消え、**段階が黙って完了してしまう**。D4 (5) の
   「人が子だけを止めたときは親の replan を起こす」を満たすため、unit は `failed`（reason `child_cancelled`）にして既存の失敗の経路
   （`PhaseSettle::Failure` → replan）に乗せる。親を中止したときの unit は従来どおり `cancelled`（下の 5.）。
3. **子が `blocked`（質問・決定・人の確認）なら unit は `running` のまま**（D4 (5) のとおり）。その段階は完了しないが、同じ段階の他の unit・
   兄弟の子は止めない。
4. **段階の完了と統合 WU（R1c まで）**: 段階は leaf がすべて `done`・kind task の unit がすべて `done`（= 子が `done`）で揃い、統合 WU
   `integrate-<stage>` が done になったら完了する。R1b の統合 WU は**子のブランチを merge しない**（kind task の unit は WU ブランチを持たないので
   `phase_leaves` の merge の対象から外れ、検査にも `checks` を足さない）。leaf のブランチの merge と検査の再実行は今どおり。
   `PhaseIntegrated.merged` に子の key はまだ出ない。子の成果を親ブランチに入れるのは R1c（D5・D6）。
5. **subtree の中止の連鎖**: 親が `cancelled` **または `failed`** で終端になったら、store の遷移と同じトランザクションで、親の計画の unit から
   作った子（`parent_id = 親` かつ `root_id` を持つ task）を新しい `Trigger::ParentCancelled`（遷移は `Cancel` と同じ、reason
   `parent_cancelled`）で中止する。子の遷移がさらに孫へ連鎖する。親の失敗も含めたのは、親が終わった後に子の subtree が誰にも取り込まれずに
   走り続けないため（D10）。走っている run は既存の `abort_stale_runs` が止める。子だけを待っていた親（run を持たない Ready）の unit は tick の照合が
   `cancel_open_work_units`（reason `cancel`）で閉じ、`done` / `failed` の親に残った kind task の unit は reason `parent_terminal` で閉じる。
   `TransitionResult.cascaded` は `parent_cancelled` も拾う。一時停止（subtree の pause）は R5a。
6. **root の `tree` は書かない**: 子の `tree.root_id` は `task_core::tree::root_id_of(親)`（`tree` の無い root なら親の id）。root 自身は
   `tree = None`・`tasks.root_id = NULL` のまま（R1a 付記 2. の未決を「書かない」で閉じる。D15「埋め戻さない」と同じ考え方で、既存の root の行を
   書き換えない）。したがって `tasks.root_id = X` は X の子孫だけを返し、X 自身は含まない。
7. **子の組み立て**（`task_ops::tree::build_child_task`）: 委譲の子と同じ `task_core::materialize_delegated_logging`（ADR-0062 B2 / D5: 担当が
   `cluster:<id>` を持たなければ継いだ remote を local に落とす）を 1 件で通し、その上で `status = ready`・`budget = 親`・`labels = [child-<key>]`・
   `skills`（unit、無ければ親）・`genre`（unit、無ければ親）・`repos`（unit の名前で親の repos を選ぶ。無ければ親と同じ）・`routing.features`
   （unit の `features` をヒントに）・`tree = {root_id, depth + 1, parent_unit}` を書く。**workspace は親**（案件の workspace で上書きしない。D4 (4)）。
   `execution_hint` は持たせない（子は最初の dispatch で自分の Complexity Gate を通る。深さの閾値と shadow の採用は R2a）。
   `objective` の末尾に固定の書式で「## 木の中の位置（ADR-0079 D4）」（深さごとの祖先の題名と段階、この task の unit）と、回答済みの決定が
   あれば「## 人の決定（ADR-0079 D7）」（`- <key> <question>: <label>（推奨どおり | 推奨と異なる） — <note>`）を足す。委譲の上限
   （ADR-0016 D2 の `max_tree_depth` / `max_tree_runs`）は木の子には当てない（木の上限は R2a）。
8. **採用前の検査**（planner の /3、`tree_plan_checks`）: kind task の unit の `repos` ⊆ 親の repos（親が持たなければ案件の primary）を検証し、
   外れれば不正な試行（理由は進行に残り、planner の再試行へ）。部をまたぐ子は、組み立てた子で matching を引いて ADR-0074 F4b と同じ
   `cross_department_questions`（`plan_children` から切り出した共通の関数）に通し、未認可なら計画を採用せず approvals で聞く（U-R2 のまま）。
   子の生成の時点でも repos を見直し、外れていれば unit を `failed`（reason `child_create_failed`）にして理由を進行に残す。
9. **`needs_decisions` の待ち**: unit の `needs_decisions` の決定が、親の節点が出した `decisions` の行（`task_id = 親`、`key`）で `answered` に
   なるまで子を作らない（兄弟は止めない）。**計画の採用時に `DecisionRequested` を出すのは R3a**（R1b では出さない）ので、R1b の時点で
   `needs_decisions` を持つ unit は R3a の回答の入口ができるまで待ち続ける。leaf の `needs_decisions` の待ちも R3a。
10. **`max_parallel_child_tasks`**: 同じ親で `running` の kind task の unit の数が上限に達していれば、`ready` の unit は子を作らずに待つ
    （`seq` 順に空いた分だけ作る）。
11. **子だけを待つ親**: `settle_phase` と `reconcile_parallel_tasks` は kind task の unit の `running` を in-flight に数えない（子の写しであって
    親の run ではない）。親の leaf の run が終わって残りが子だけなら `Continue{advance}` で `ready` に戻り、gate は `Skip`（lease なし）。
    表示用の導出値 `ExecutionPhase::AwaitingChildren`（`ready` で、leaf に走れる・走っているものが無く、`ready` / `running` の kind task の
    unit がある）と `ExecutionView.awaiting_children` / `TaskExecutionView.awaiting_children`（unit の key・子の id・題名・状態）を足した。
    `WorkUnitView` / `ExecutionWorkUnitView` に `child_task_id`。GUI は `EXECUTION_PHASE_LABEL` に「子 task の完了待ち」を足しただけ（木のタブは R4b）。
12. **/3 の scheduler**: dispatcher の `wu_dispatch_gate` と `parallel_mode` の「/2 か」の判定を `is_phased_schema`（/2・/3）に広げた
    （/3 の段階は `internal_view` で /2 の工程と同じ行）。`running_tasks_with_runnable_work_units` は kind task の行を拾わない。/1・/2 は変わらない。
13. **`review: human` の段階**: 採用（新規・replan）で `task_core::resolve_plan_pause_points`（/1・/2 は `resolve_pause_points` と同じ結果。/3 は
    `pause_after` の解決に `review: human` の段階を足す）で `PausePointsResolved` に解決し、ADR-0074 D2.2 の `PhaseGate` にそのまま乗る。
    **最後の段階の `review: human` は止めない**（統合の後は最終レビュー。ADR-0074 D1.6 の表と同じ）。途中報告の工程の題名は `internal_view` で
    段階の題名を引く（子の要約の行は R4a）。
14. **木での委譲の禁止**: `Task.tree` を持つ task の run には `available_genres` を渡さず、`delegate` の受け口も「木の節点では委譲できない」で拒む。
    root（`tree` を持たない）の WU の run は ADR-0072 D22 のまま元から委譲できない。
15. **`Event::Created.origin`**（`CreatedOrigin::PlanUnit`）を足した（省略可、`None` は出力しない。既存の JSON は 1 バイトも変わらない）。
    `event.schema.json` / `api-v1.schema.json` / `gui/app/celeris/types.ts` を再生成。
16. **replay**: 子の結び付き（`ChildTaskCreated` → `work_units.child_task_id`、R1a）と unit の写し（`WorkUnitTransitioned`）は events だけから
    作り直せる（`celerisctl replay --check` の `work_units` 差分 0。壊した索引は `--apply` で戻る）。ただし `runs` の表は、偽のアダプタで走らせた
    /2・/3 の計画で planner run の role と並列 WU の run の `work_unit_id` / `seq` が events から復元しきれない差が **R1b 以前から**ある
    （木と無関係。R1b では直していない）。

## 付記: R1c 実装時の逸脱・明確化（2026-09-28）

R1c（子のブランチの基点、統合 WU での子のブランチの merge、子の最終レビューの基点、子の取り込みの抑止、root だけが main）で決めたこと。
本文の決定は変えていない。migration は足していない（schema 31 のまま）。

1. **子の作業場所は task の作業場所の規則のまま**: 子は親の作業場所の下に入れ子にせず、普通の task と同じ
   `<workspace_root>/<child_id>/repos/<name>`（1 リポジトリの旧い形は `<workspace_root>/<child_id>/tree`）に worktree を持ち、ブランチは
   `celeris/<child_id>`（`worktree_branch_prefix`）。成果物・`runs/`・目印・レビューの錠・中止の後片付け（ADR-0043 D2）・生成物の刈り取り
   （ADR-0066 D2）など task id で引く経路がすべてそのまま効くため。違うのは worktree の base だけ（下の 2.）。子が compound なら、子の WU は
   今どおり `<child_dir>/wu/<key>`・`celeris-wu/<child_id>/<key>` で、子のブランチに対して統合される（孫も同じ）。
2. **base の決め方**（`Dispatcher::worktree_base_for`）: 木の子（`tree.parent_unit` を持つ task）の worktree は `BaseKind::Parent`
   （前置きの base の出どころは `parent`）で、`tree.base_commit` から切る。`base_commit` は**親の先頭の git リポジトリの sha 1 つ**
   （`TreeInfo` の形を変えないため）。子の repos の 2 つ目以降のように、その sha がリポジトリで解決できなければ、そのリポジトリの親のブランチ
   `celeris/<parent_id>` の HEAD（親のブランチは段階の途中では動かない。D6）。どちらも無ければ WARN を出して従来の規則（main）に倒す。
   木でない task・root は今どおり（1 バイトも変わらない）。
3. **`base_commit` を決める時点**: 子を作るとき（unit が ready になった tick、`Dispatcher::child_base_commit`）に、葉と同じ規則で決めて子の
   `tree.base_commit` に書く（`Created` の Task の JSON に入るので replay は同じ値）: 同じ段階の依存先があれば `integration::dependency_base`、
   無ければ親の task ブランチの HEAD（= 段階の基点。段階の unit が子だけのときは親の worktree を先に用意する）。親が並列 1 に倒れている
   （remote / `shared` / git でない。`parallel_mode` の fallback）なら `None`（子もブランチを持たず、統合は子を merge しない。D5）。基点が
   決まらなければ unit を `failed`（reason `child_create_failed`、理由は進行に残す）にし、既存の失敗の経路に乗せる（黙って待たない。D10）。
4. **`dependency_base` を子に広げた**: 引数に `branch_prefix` を足し、依存先が kind task の unit なら子のブランチ `<prefix><child_id>` の HEAD →
   記録した `head_commit` → 親の task ブランチの HEAD の順（子がブランチを持たなかったとき）。葉が子に依存しても、子が子に依存しても同じ関数。
5. **子の done の記録**: 子が `done` になり unit を `done` に写す tick で、daemon が子の worktree に残った変更を決定的に commit し（WU の完了時と
   同じ `integration::commit_all`、メッセージ `task/<child_id>: <title>`、変更が無ければ commit しない）、unit の行に `head_commit`（子のブランチの
   HEAD）と `base_commit`（子の基点）を書き、`WorkUnitTransitioned{child_done}` と `Event::WorkUnitCommitted{branch: celeris/<child_id>, base, commit}`
   を 1 トランザクションで積む（`work_units_apply`）。unit の行の `branch` は書かない（「WU の worktree を持たない」の印のまま）。replay は
   `WorkUnitCommitted` の `branch` が `celeris-wu/` でなく unit が kind task なら `base_commit` も写す（`head_commit` は従来どおり）。
6. **統合**: 段階の統合 WU が merge するのは、葉の WU のブランチ（`phase_leaves`、従来どおり）に、その段階の `done` の kind task の unit の子の
   ブランチを足したもの（**葉かどうかに関わらず**入れる。子に依存する同じ段階の葉があっても子の commit は 1 度だけ入り、`merged` には子と葉の
   両方が残る）。順は `seq`。子のブランチは `MergeItem::child_task`（**リポジトリに無ければ飛ばす**。子の repos は親の部分集合なので）で、
   2 つ目以降のリポジトリの `merged` も和を取る。子が `base_commit` を持つ（= ブランチを切った）のにどのリポジトリにもブランチが無ければ
   統合の失敗（`integration_gives_up`、replan）。既に入っている子は `skipped`（冪等。採用した done の子の成果が既に親ブランチにある場合も同じ）。
   衝突は既存の `merge-<stage>-<key>` の repair、検査の再実行と失敗は既存の repair / replan のまま。統合が済んだら子の worktree を消す
   （ブランチは残す）。
7. **子の最終レビュー**（`review::tree_child_review_view`、レビューに渡す task の view だけを変え、保存した task は変えない）: `Check::Command` の
   `merge-base --is-ancestor <ref>` の `<ref>` が既定のブランチ（`main` / `master` / `origin/main` / `origin/master` / `refs/heads/main` /
   `refs/heads/master` / `origin/HEAD`）なら親のブランチに置き換える（条件の数と順は変えない）。したがって merge-base の不成立で daemon が
   決定的に merge するのも親のブランチになり、main の commit が子に入らない。reviewer run の前置き（目的）の末尾に「## 取り込み先（ADR-0079 D6）」
   （親のブランチ、差分の基点 `base_commit`、main が進んでいても不合格にしない）を固定の書式で足す。checkpoint の差分の基点も `base_commit`。
8. **取り込みの抑止**: 子には ADR-0051 の部署の取り込み判定（`task_ops::delivery::begin` が `None`。`deliveries` の行・merge の条件・release を
   作らない）も、ADR-0043 D5 の人の取り込み（`POST /tasks/{id}/changes/{repo}/integrate` の merge / pr / discard はすべて 409 `tree_child`、
   文言は「成果の取り込み」）も無い。`celeris` の delivery の tick も木の子の行は進めない（保険）。`TaskReady` は木の子では鳴らさない（root の done
   だけ）。**子の `TaskFailed` は R3b まで今どおり鳴る**（D11 の「子の失敗は鳴らさない」は R3b の範囲）。
9. **後片付けの範囲**: 子の worktree は親の統合の後で消す。中止の連鎖で `cancelled` になった子は既存の `cleanup_cancelled_worktrees` が
   worktree とブランチを消す（ADR-0043 D2 のまま）。**root の取り込み・中止で木の done の子のブランチをまとめて消すのは R1c ではやっていない**
   （done の子のブランチは root の終端の後も残る。R4b 以降で木の後片付けとして扱う）。
10. **受け入れ条件 (e)**（採用した done の子の成果が既に main にあるとき統合は飛ばす）は、採用（adopt）が R5b なので、統合の冪等性
    （`integration::tests::child_task_branches_are_optional_and_idempotent`: 既に入っている子のブランチは `skipped`）で確かめた。採用の入口の
    試験は R5b。

## 付記: R2a 実装時の逸脱・明確化（2026-09-29）

R2a（深さの gate の閾値、木の子は shadow でも採用、計画の採用時の unit の gate、木の上限の数え上げと超過の決定の要求）で決めたこと。
migration は足していない（schema 31 のまま。`work_units.blocked_reason` は制約の無い TEXT なので新しい値 `decision` を足すだけ）。

1. **閾値と記録**: `task_core::execution_gate::decide_at(.., GateThreshold)`（`decide` は `GateThreshold::ROOT` = 閾値 5・深さなしで呼ぶ同じ関数）。
   閾値は `task_core::tree::gate_threshold(depth, [execution.tree] gate_depth_step)`（`5 + step × (d − 1)`。設定は 0..=10、既定 2）。
   `ExecutionGateDecision` に `depth: Option<u32>` を足した（木の子だけ `Some`。root・木でない task は `None` で出力しない = 既存の JSON は 1 バイトも
   変わらない）。**木の子**は `tree.parent_unit` を持つ task（`task_core::tree::is_tree_child`）。root の gate は `[execution] gate` のまま（U-R5）。
2. **木の子は shadow でも採用**（D4 (1)）: 木の子の判定は `shadow = false`・`depth = Some(d)` で記録し、compound なら `[execution] gate` に関わらず
   planner に進む（`dispatch_one` の `tree_child_compound`）。**`gate = "off"` でも木の子は判定して採用する**（D4 の理由〈分けると決めた木を途中で
   1 run に潰さない〉は off でも同じ。root は off なら従来どおり判定しない）。GUI は `depth` で見分けて「木の子: 深さ d・閾値 t、常に採用」を出す
   （`~/lib/execution-mode.ts` の `gateDecisionLine` / `~/lib/task-execution.ts` の `gateModeLabel`）。
3. **unit の view**（D4 (3)、`task_core::tree::unit_view`）: 親の Task を複製し、題名・目的・受け入れ（leaf は `done_when` を reviewer・`checks` を
   command の条件に写す）・予算（leaf は unit の `budget`、無ければ D18 の既定 `max(親, 30 turns / 1,800 秒)`、kind task は親の予算 = 子が継ぐ予算）・
   genre（leaf は `harness`、kind task は unit の `genre`、無ければ親）を差し替える。**`routing` は unit の `features` のヒントだけ**（親のヒント・人の
   明示・CoS のヒントは継がない。継ぐと root の compound の理由がすべての unit に写る）、印（labels）は持たない。判定は深さ `d + 1` の閾値、
   `shadow = false`。
4. **表の読み方**（`task_core::tree::unit_gate` / `apply_unit_gates`）:
   - 構造上の理由（表の 5 行目、`structural_reasons`）= human の acceptance・実効の `needs_decisions`（unit か `stage:<段階>` を `needed_before` に持つ
     決定を含む）・`adopt`・親と違う `genre`・親の実効の repos と違う `repos` の集合・**親の skills に無い skill（「別の部署の skill が要る」の決定的な
     代わり。matching を引かない）**。
   - 下げる（表の 6 行目）の leaf の基準は kind task の unit について (a) 子が継ぐ予算（親の予算）が leaf の 1 run の上限（`work_unit_max_turns` 80 /
     `work_unit_max_wall_secs` 3,600）以内、(b) `repos` が高々 1、(c) `Check::Command` の acceptance が 1 本以上（gate の atomic は 1 run の見込み）。
     **基準を満たさなければ task のまま（`kept_task`、理由に「leaf criteria not met」）**。表は「構造上の理由なし・基準を満たす → 下げる」だけを
     書いているので、満たさないときは宣言どおりにした（黙って leaf に押し込まない）。
   - 上げた leaf（`promote_to_task`）: `checks` を command の acceptance に、`done_when` と `context.paths` は目的の末尾に固定の書式
     （`## 完了の条件（計画の leaf から引き継ぎ。ADR-0079 D4 (3)）`・`対象のパス（計画の leaf から引き継ぎ）:`）で移す。**`done_when` を reviewer の
     acceptance にはしない**（子の最終レビューに reviewer run を足さない。U-R7）。`context.repo` → `repos`。
   - 下げた task（`demote_to_leaf`）: kind は段階の kind、acceptance の文 → `done_when`、command の条件 → `checks`、`repos`（高々 1）→ `context.repo`。
   - 上げる・下げるを当てた spec を**採用する計画の spec**にする（`ExecutionPlanned` に入るので replay は同じ行を作る）。当てた spec が上限以外の
     理由で検証に落ちたら（通常起きない）planner の宣言どおりに採用し、上げる・下げるは当てない（警告）。replan で持ち越す done の unit には
     gate をかけない（D17 の不変条件）。
   - **人の計画（`PUT`、origin human）には unit の gate をかけない**（入口の API は R1a 付記 5. のとおりまだ tree 無効で検証する。R5b で配線するとき
     に決める）。
5. **`UnitGateOverridden` の欄**: `score`（view の gate のスコア）と `reason`（規則 id・閾値・構造上の理由・基準の不足の 1 行）を足した
   （`serde(default)`。R1a の Event は本番で未発行）。宣言と食い違った unit（`promoted` / `demoted` / `kept_task` / `decision`）ごとに 1 件、
   採用の**直後の 1 トランザクション**（`Dispatcher::apply_tree_plan_holds`、`work_units_apply`）で決定の要求・unit の止めと一緒に積む
   （採用〈`ExecutionPlanned`〉と同じトランザクションではない。子の生成〈R1b 付記 1.〉と同じく tick の中の続きの書き込み）。
6. **止め方 = `blocked(decision)`**: `WorkUnitBlockedReason::Decision`（`"decision"`）を足した。止めるのは `pending` / `ready` の行だけ
   （`WorkUnitTransitioned{reason: "decision"}`。replay は reason から同じ値を作る）。scheduler は `blocked(decision)` を**工程の失敗にも質問にも
   数えず**（`settle_phase`）、**同じ段階の他の unit を止めない**（`runnable_work_units` の「失敗・blocked があれば ready を起こさない」から外した。
   /1・/2 にこの理由は無いので従来どおり）。段階はその unit が done にならないので完了しない（D5）。親は `ready` のまま run を起こさない
   （`WuDispatchGate::Skip`、子待ちと同じ）。回答で `ready` に戻すのは R3a。
7. **計画の上限（段階の数・段階あたりの unit・計画あたりの子 task・`max_depth`）**: D3 の「検証で拒否」と D4 (3) の表の 7 行目「検証で拒否 → 再試行 →
   それでも同じなら決定の要求」を合わせて、**1 回目の試行は従来どおり不正な試行**（planner に理由を渡して再試行）、**最後の試行**（ADR-0072 D14 の
   2 回目）で違反がこの 4 つだけなら、その 4 つを外した上限（`relaxed_tree_plan_limits`）で採用し、超えた分の unit を `kind: limit` の決定で止める
   （atomic に倒さない・黙って切らない。計画の unit は 1 つも消さない）。どれを止めるかは `task_core::tree::plan_limit_holds`（純粋関数、計画の順で
   上限の内に入るものを残す）: 段階の数 → `max_stages` 番目より後の段階の unit、段階あたり → 段階ごとに先頭から上限個より後、子 task → 先頭から
   上限個より後、`max_depth` → 子 task を持てない深さの kind task の unit すべて。1 つの unit は 1 件の決定で止まる。unit の gate の上げる
   （子 task の数を増やしうる）も同じ規則で止める。他の不正（形・依存・予算…）が混じれば従来の経路（/3 の 2 回不正の `plan_invalid` は R2b）。
8. **木の leaf**（`max_tree_leaves`）: 計画の採用のとき、木（root とその子孫）の生涯の leaf の数（`work_units` の行のうち kind task・統合・repair と
   `blocked(decision)` を除く）に、この計画で新しく作る leaf（既存の key に無い leaf）を足して超える分を計画の順で止める（`limit:max_tree_leaves`）。
9. **木の run・トークン・replan**（`max_tree_runs` / `max_tree_tokens` / `max_tree_replans`）: dispatch が run（worker・planner・repair・atomic の
   子の run。統合と「何もしない」は数えない）を起こす直前に `Dispatcher::tree_run_limit_hold` が木の数（`task_ops::tree::tree_counters`）を
   `task_core::tree::run_limit_breach` に照らす（runs → tokens → replan の planner run の replans の順。reviewer の run は数えない）。超えるなら
   **run を起こさず**（Task の状態は変えない。`ready` のまま、並列の 2 本目以降なら走っている兄弟はそのまま）、`kind: limit` の決定を**木に 1 件だけ**
   出す（同じ key〈`limit:max_tree_runs` など〉の未回答の決定が木にあれば出さない = tick ごとに増やさない）。決定は超えようとした節点の events に
   積み、`needed_before: ["self"]`（D7 の `self` を daemon の節点の止めにも使う）、`path` は root からその節点まで。木全体の数なので、超えた後は
   木のどの節点も新しい run を起こさない（走っている run・判定・統合は止めない）。§7 R2a (d) の「兄弟の subtree は走る」は、先に走り出した兄弟と
   in-flight の仕事が続くことで満たす（受け入れの試験 `tree_limit_breach_stops_only_that_subtree`）。木の節点 = 木の子、または /3 の計画を持つ root。
   `[execution.tree] enabled = false` なら何もしない。
10. **`max_parallel_child_tasks` は決定の要求にしない**: D3 の表のとおり「作らずに待つ（pending / ready のまま）」（R1b の実装のまま）。上限は
    同時の数で、仕事は落ちず、空きができれば作られる（人に聞くことが無い）。依頼の「同時の子も limit の決定」は D3 と食い違うので D3 に従った
    （`concurrent_children_limit_waits_without_a_decision` で待つだけで決定・質問が出ないことを確かめた）。
11. **数え上げ**（`task_core::tree::{TreeCounters, DepthCounters, TreeNodeFacts, tree_counters}`、store の読み取りは `task_ops::tree::tree_counters` と
    新しい `TaskStore::tree_tasks(root_id)`〈`id = root OR root_id = root`〉）: 節点数、leaf、run（reviewer を除く）、reviewer の run、replan（節点ごとの
    `計画の版 − 1` の和）、トークン（input + output、cache は含めない）、定価（`cost_usd_complete`）と、**深さごとの run（role ごと）・reviewer の run と
    reviewer の定価・トークン・定価**（U-R7 の指標の元。API に出すのは R4a）。
12. **決定の形**（`task_core::tree::{limit_decision, leaf_too_large_decision}`）: id は daemon の ULID、key は daemon が振る `limit:<設定名>[:<段階>]` /
    `leaf_too_large:<unit>`（計画の key の書式 `[a-z0-9-]` とは別。計画の決定と衝突しない）、`origin: daemon`、`raised_by.run_id` は planner run
    （計画の採用のとき）か無し（dispatch のとき）。選択肢は limit が「今回だけ上限を上げて続ける（`raise-once`）/ この subtree を replan で小さく
    する（`replan`）/ この subtree を取り下げる（`withdraw`）」、leaf_too_large が「leaf のまま 1 run で試す / replan で小さな leaf に分ける /
    取り下げる」。推奨はどちらも `replan`。通知・受信箱・回答の API と回答の効き目（止めた unit を `ready` に戻す、`raise-once` で上限を足す）は R3a。
13. **既存の試験の fixture**: R1b / R1c の試験の kind task の unit は小さく atomic なので、R2a の unit の gate で leaf に下げられる。子 task のまま
    残す構造上の理由として fixture に `skills: ["tree-fixture"]`（親に無い skill）を足した（孫の unit は別の skill）。試験の意図は変えていない。
14. **idle**: 決定を待って止めた仕事を持つ木は `ready` の task を残すので、`--until-idle` の idle にならない（子待ちの親〈R1b〉と同じ扱い。本番の
    daemon は `until_idle` を使わない）。黙って止まっていないことの検出（D10）は R3b。

## 付記: R2b 実装時の逸脱・明確化（2026-09-29）

R2b（planner への深さ・leaf の基準・木の残りの上限、/3 の 2 回不正 → `plan_invalid`、子の失敗 → 親の replan、子の基盤の失敗の
再試行と障害通知）で決めたこと。migration は足していない（schema 31 のまま。`work_units.blocked_reason` に値 `infra` を足すだけ）。

1. **/3 を書かせる planner**: `[execution.tree] enabled` で、計画が無いか今の計画が /3 の task（`Dispatcher::is_tree_planner`）。
   root も木の子も同じ（root も /3 を使える）。/1・/2 の計画の replan は木が有効でも従来どおり（/2 の形と差分）。
   planner の文脈は `ExecutionPlannerContext.tree: Option<TreePlannerContext>`（深さ・`max_depth`・残りの深さ・計画の上限〈段階・
   段階あたり・子 task・決定〉・同時の子・木の残り〈leaf・run・replan・節点の replan・トークン・未回答の決定〉・祖先の題名と段階と
   目的の先頭 300 文字・`stages_hint`）。値は検証と同じ `ExecutionLimits.tree` と `task_ops::tree::tree_counters` から埋める
   （F5-fix3 と同じ「1 か所」。プロンプトの `### Plan limits` も /3 のときは同じ値で段階・unit・決定の上限と「leaf の予算は丸めずに
   拒否」を出す）。`tree = None` のプロンプトは 1 バイトも変わらない。
2. **`stages_hint` の置き場**: `Task.routing.stages_hint: [{title, scope}]`（`pause_after` と同じ理由で `TaskRouting` に置く。空なら
   出力しない）。書く入口（CoS の `create_task.stages_hint`、API）は R5a。planner は root の値だけを受け取る（子は継がない）。
3. **/3 の 2 回不正の止め方**: `blocked(decision)` を Task の状態には持ち込まず、R2a の木の上限と同じく **Task は `ready` に戻し
   （`Continue{planned}`）、未回答の `kind: plan_invalid` の決定（key `plan_invalid`、`needed_before: ["self"]`、選択肢
   「人が計画を書く（推奨）/ atomic で試す / 取り下げる」、`cost_note` に試行ごとの検証の理由）がある間は run を起こさない**
   （`Dispatcher::plan_invalid_hold`）。`blocked` にしないのは、`blocked` が質問（`QuestionBlocked` の通知・`Answer` で再開）の意味を
   持つため。「2 回」は /3 を書かせた planner の試行の窓（ADR-0072 D14）で数え、計画を書かなかった・/3 でない形を書いた試行も
   不正な試行に含む。**replan の planner（/3 の計画を持つ節点）の 2 回不正も、D12 の自由文の質問の代わりに同じ決定**にした
   （D9「木の節点では構造化した決定の要求に置き換える」）。回答の効き目は R3a。
4. **/3 の replan は計画の全体**: 差分（`execution-plan-delta/1`）は /2 の形しか持たないので /3 の計画の replan では拒否する（理由は
   planner に返る）。planner が done の unit を省いた・書き写し損ねたときは、daemon が今の版の（unit の gate を当てた）spec のまま
   持ち越す（`task_core::execution_plan::carry_done_units_v3`。段階・待つ決定も）。あわせて `task_ops::execution::replan` を /3 に
   対応させた（**R1b〜R2a では /3 の replan は `validated.spec.work_units` が空のため未完了の unit をすべて superseded にし、新しい行を
   作らなかった**。`internal_view` で /2 と同じ行に写し、統合 WU・工程の障壁・`needs_decisions` も同じ規則）。
5. **子の work の失敗 → 親の replan**（D9）: 子の `failed`（work）・`cancelled` は R1b のとおり unit `failed` → 段階の失敗 → replan。
   replan の planner の `replan_reason` は `child task "<題名>" (unit <key>, attempt n, <id>) failed (<分類>): <理由>; the child's last
   checkpoint: …`（理由は `classify_task_failure`。最終レビューの不合格の理由を含む）、unit の要約の行は kind task の unit について
   子の id・題名・状態・分類と理由・checkpoint の要約。**やり直しは同じ key でよい**（D9 の「新しい key の task unit」からの逸脱。
   新しい key も使える）: 失敗した子の unit を新しい版に残すと、replan が unit の子の結び付き（`child_task_id`）を外し、次の照合で
   同じ unit から新しい子（`Task.tree.parent_unit.attempt = n + 1`。`attempt` は 1 なら出力しない）を作る。元の子の task とブランチは
   残る。**子が走っている kind task の unit は replan で `running` のまま持ち越す**（R1b では兄弟の失敗の replan で走っている子の unit が
   `ready` に戻り、子を持ったまま二度と写されなかった）。やり直しの回数の上限は unit ごとには置かず、replan の上限
   （節点の `max_replans`、木の `max_tree_replans`、D16）で抑える。
6. **節点の replan の上限**: 木の節点で replan が要るのに `[execution] max_replans` を使い切っていたら、従来の `Skip`（黙って止まる）の
   代わりに `kind: limit`（key `limit:max_replans`、`TreeLimitKind::NodeReplans`、節点ごとに 1 件、`needed_before: ["self"]`）の決定の
   要求を出す。**leaf の失敗で replan を使い切ったときの ADR-0072 D12 の質問（run の終わりの経路）は変えていない**（R3a 以降）。
7. **子の基盤の失敗**（D9）: 基盤の分類は `classify_task_failure` の infra（`InfraRequeue` を使い切った harness_error・lease の失効〈orphan
   の引き取りを含む〉・準備の失敗、供給側の requeue の上限。ディスク不足）。`max_child_infra_retries` は設定にせず定数
   `task_core::tree::MAX_CHILD_INFRA_RETRIES = 1`。作り直しは `task_ops::retry`（失敗した task の複製）ではなく**同じ unit の spec から
   `build_child_task` で新しい子**（attempt + 1、基点は元の子と同じ `base_commit`）を作る（R1b の子の生成と同じ組み立て・検証を通す）。
   unit は `running` のまま `WorkUnitTransitioned{running → running, child_infra_retry}` と `ChildTaskCreated` を 1 トランザクション
   （`tree_child_create(.., replaces: Some(<元の子>))`）。回数の窓は直近の `ExecutionPlanned` から（replan で作り直す）。
   2 回目も基盤の失敗なら unit を `blocked(infra)`（`WorkUnitBlockedReason::Infra`、reason `child_infra_failed`）にし、
   **障害通知を dispatcher が `NotificationKind::TaskFailed`（key `tree-infra:<unit id>:<子 id>`、本文に「障害（infra）」・子の題名・
   理由・止まった段階）で 1 件**出す（決定の要求・質問にはしない）。`blocked(infra)` は `blocked(decision)` と同じく工程の失敗に数えず、
   同じ段階の他の unit・兄弟の子を止めず、段階は完了しない（親は `ready` のまま待つ）。子自身の `TaskFailed`（`scan_task_failed`）も
   今どおり鳴る（子の失敗の通知の抑止は R3b）。人の「再試行」の入口（`blocked(infra)` の unit から新しい子）は R3b / R4b。
8. **replay**: `rebuild_work_units_and_runs` は版ごとの `ExecutionPlanned` で「done でない行を新しい版の `plan_id` / `seq` にする」を
   畳み込む（`replan` が done の行に触れないのと同じ。**/2 の replan で done の WU があると `plan_id` が食い違っていた既存の差**も
   これで消える）。`replan v<n>` の遷移で `ready` / `pending` に戻った kind task の unit は子の結び付きを外し、`child_infra_failed` は
   `blocked(infra)` に写す。

## 付記: R3a 実装時の逸脱・明確化（2026-09-29）

R3a（決定の要求の回答 API / MCP、回答の効き目と入力への注入、worker の `decisions`、受信箱、Discord 通知）で決めたこと。
migration は足していない（schema 31 のまま。`decisions` の表・`work_units.blocked_reason` の値 `decision` は R1a / R2a のまま）。
Event の形も変えていない（`DecisionRequested` / `DecisionAnswered` / `DecisionWithdrawn` は R1a の定義のまま発行する）。

1. **置き場**: 判断と書き込みは `task_ops::decision`（`list` / `for_subtree` / `answer` / `withdraw` / `revise` / `inbox_items` /
   `open_count` / `leaf_decision_lines` / `limit_allowances`）。API（`crates/task-api/src/decisions.rs`）と MCP（`decision_list` /
   `decision_answer`、scope `tasks:interact`）は同じ関数を呼ぶ。回答の主体（`DecisionAnswered.by`）は API が `human`、MCP が
   `mcp:<client_id>`。純粋な部分（回答の検証 `validate_answer`、選択肢 → 効き目 `answer_effect`、注入の書式 `answer_line`、worker の
   決定の検証と束ね `prepare_worker_decisions`）は `task_core::decision`。
2. **1 トランザクション**: 新しい `TaskStore::decision_resolve_apply(task_id, decision_id, expect, rows, events)` が、決定の行が今も
   `expect`（回答・取り下げは `open`、revise は `answered`）であることを同じトランザクションで確かめてから、`DecisionAnswered` /
   `DecisionWithdrawn`・unit の遷移・効き目の event を積む（競合した 2 つ目の回答は何も書かず 409）。**節点の中止**（下の表の
   `self` の取り下げ）だけは続けて既存の `gate::cancel`（別の書き込み）。決定を出した節点が終端なら回答は 409。
3. **計画の決定の発行**（R1b 付記 9. の「R3a で出す」）: /3 の採用（新規・replan）の直後の書き込み（`apply_tree_plan_holds`、R2a の
   止めと同じトランザクション）で、`normalized_decisions` のうちこの節点にまだ同じ key の行（取り下げ済みを除く）が無いものを
   `DecisionRequested`（origin `planner`、`raised_by.run_id` = planner run、path = root からこの節点まで）にする。replan で計画から
   消えた planner / human の未回答の決定は `DecisionWithdrawn`（reason `replan v<n>: …`）。**人の計画（`PUT`）の決定はまだ発行しない**
   （その入口は R1a 付記 5. のとおり tree 無効で検証する。R5b で配線するときに同じ関数を通す）。
4. **待ち方**: 答えの無い決定を待つ leaf（実効の `needs_decisions` = `needs_decisions` と `needed_before` の unit / `stage:<段階>`）は
   採用の直後に `blocked(decision)`（R2a の止め方。同じ段階の他の unit・兄弟を止めない）。kind task の unit は R1b のまま `ready` で
   子を作らずに待つ（照合 `reconcile_tree_units` が `answered_decisions` を見る）。回答の再評価（`release_rows`）: `blocked(decision)` の
   unit を、(a) それを名指しする決定（`needed_before` に key か `stage:<段階>`）が「まだ止めている」ものが無く、(b) `needs_decisions` が
   すべて回答済みなら `pending` に戻し、依存と段階の障壁が満たされていれば同じ書き込みで `ready`（reason `decision_answered`）。
   「まだ止めている」= 未回答、または `replan` と答えて、その回答の後に計画がまだ採用されていないもの。`needed_before: [self]` の
   決定のうち `plan_invalid` と atomic の run の worker の `self` は、R2b の `plan_invalid_hold` を一般化した `decision_self_hold`（その節点が
   出した未回答の `self` の決定がある間は run を起こさない）で待つ。`kind: limit` の `self`（run 時の木の上限・節点の replan の上限）は
   従来どおり run を起こすときに `tree_run_limit_hold` / `replan_gate` が回答の余裕込みで見直す（R2a 付記 9. の「走っている run・判定・
   統合は止めない」を保つ）。
5. **選択肢 → 効き目の表**（`task_core::decision::answer_effect`。決定の種類と選択肢の key だけで決まる。決定的、LLM なし）:

   | kind | option | 効き目 | 具体的に起きること |
   |---|---|---|---|
   | choice（計画・worker・束ね） | どれでも（自由記述 `other` を含む） | resume | 待っていた unit を 4. の規則で戻す。子は生成時の objective の末尾、leaf は前置きの「人の決定」節に答えが入る。atomic の run の `self` なら節点の止めが外れ、答えは次の run の `answers` に入る（`Event::Answered`、問いの接頭辞 `人の決定（ADR-0079 D7）: `） |
   | leaf_too_large | `run-as-leaf` | resume | 止めた leaf を戻す（leaf のまま 1 run で走る） |
   | leaf_too_large / limit | `replan` | replan | 節点が計画を持てば `ExecutionHintSet{replan: true, source: "<by> (decision <id>)", note}` を積む（次の dispatch で replan の planner run。`replan_reason` に「決定 <key>「…」への回答で replan: <note>」）。止めた unit は replan が置き換えるまで止めたまま。run 時の上限なら余裕も 1 回分足す（下の 6.） |
   | leaf_too_large / limit | `withdraw` | withdraw | 止めた unit と、それに（推移的に）依存する未着手の unit を `cancelled`（reason `decision_withdrawn`。段階はそれ抜きで完了する）。`needed_before: [self]` なら節点を中止 |
   | limit | `raise-once` | raise_once | 計画の採用時の上限（段階の数・段階あたり・子 task・`max_depth`・木の leaf）は止めた unit を戻すだけ。run 時の上限（`max_tree_runs` / `max_tree_tokens` / `max_tree_replans` / 節点の `max_replans`）は上限に余裕を足す（6.） |
   | plan_invalid | `replan`（R2b の `human-plan` も同じ） | replan | 止めが外れ、planner の試行の窓（`planner_attempts_in_window`）を回答の位置から数え直す（もう 2 回試せる）。人の note は planner の `previous_attempt_errors` の末尾に「人の指示（…）」の 1 行で渡る。計画を持つ節点（replan の 2 回不正）なら `ExecutionHintSet{replan: true}` も積み、`replan_reason` にも note が入る |
   | plan_invalid | `atomic`（初回の計画のときだけ選択肢に出る） | atomic | 次の dispatch で daemon が gate の判定を `atomic`（規則 id `atomic/decision`、信号 `plan_invalid_answered_atomic`）に書き換え（`ExecutionGated`）、計画を作らずに 1 run で走らせる |
   | plan_invalid | `cancel`（R2b の `withdraw` も同じ） | withdraw | 節点を中止 |

   人の取り下げ（`POST /decisions/{id}/withdraw`、`DecisionWithdrawn`）の効き目は `withdraw` と同じ。**節点の中止**は、木の子なら先に
   親の unit を `cancelled`（reason `decision_withdrawn`）にしてから子を `Cancel` する（子の中止を R1b 付記 2. の「子の cancelled → unit
   failed → 親の replan」に乗せない。取り下げは「その仕事をしない」）。root なら root の `Cancel`（subtree に連鎖）。
6. **`raise-once` の余裕**（run 時の上限）: 木の上限は、木の中の回答済みの `limit:<名前>` の決定のうち `raise-once` / `replan` の件数 ×
   `task_core::tree::limit_allowance_step`（設定の上限の半分、切り上げ・最低 1）を上限に足す（`limits_with_allowances`。`tree_run_limit_hold` と
   planner の「木の残り」が同じ値を使う）。節点の `max_replans` は、その節点の `limit:max_replans` への回答 1 件ごとに +1（`effective_max_replans`。
   replan の要否・`replan_gate`・統合の失敗の replan の判定で同じ値）。run 時の上限の `replan` にも余裕を足すのは、replan 自体が run を要するため
   （余裕が無いと replan の planner run がまた上限で止まる）。計画を持たない節点（atomic の子）への `replan` は余裕を足すだけ（replan する計画が無い）。
   「今回だけ」は回答 1 件 = 1 回分で、使い切ればまた `kind: limit` の決定が出る。
7. **`plan_invalid` の選択肢を改めた**（R2b の「人が計画を書く / atomic / 取り下げる」→「note を添えて replan（推奨）/ atomic / cancel」）。
   人が計画を書く道（`PUT`）は `replan` の選択肢の説明に残す。atomic は計画がまだ無いときだけ出す（採用済みの計画の WU を宙に浮かせない。
   `task_ops::regate` の「計画を持つ Task を atomic に戻さない」と同じ）。本番の DB に `plan_invalid` の行は無い（木が無効）。
8. **自由記述**: `option` を省いた回答は `kind = choice` で `note` が空でないときだけ受け付け、`option = "other"` と記録する（注入の書式の
   ラベルは「自由記述」、推奨と異なる扱い）。daemon の決定は効き目が選択肢で決まるので `option` 必須（422）。`note` は 2,000 文字まで。
9. **revise**（`POST /decisions/{id}/revise`）: 回答済みの `choice` だけ（daemon の決定は回答の時点で効き目を当てたので 409）。新しい
   `DecisionAnswered` を積むだけで unit の状態は変えない。これから作る子・これから走る leaf は最後の回答を読む。既に作られた非終端の子には
   node のコメント（人を起こさない。見出し「人の決定（ADR-0079 D7）」と新しい 1 行）で届け、作り直さない（D7 のとおり）。
10. **注入の書式**: 子の objective の末尾（R1b）と leaf の前置きは同じ `task_core::decision::answer_line`（`- <key> <question>: <label>（推奨どおり |
    推奨と異なる） — <note>`）と同じ見出し `## 人の決定（ADR-0079 D7）`。leaf は `WorkUnitPromptContext.human_decisions`（その leaf の
    `needs_decisions` と、その leaf を名指しした決定〈worker の `self` を含む〉の回答）。
11. **worker の決定**（D7 (b)）: 読むのは `result.json` の `decisions` だけ（**`checkpoint.json` は読まない**。yield の続きの checkpoint に決定を
    書く経路は使われていないため。必要になれば同じ関数を足す）。木が有効で木の節点の worker の run だけ（`RunContext.decision_requests = true`
    のとき前置きに書き方の節が出る。無効なら 1 バイトも変わらない）。検証（`prepare_worker_decisions`）: D7 の形、`needed_before` は `self`・
    計画の unit の key・`stage:<key>` だけ（atomic の run は `self` だけ）、この節点で使われている key は捨てる（同じ `result.json` を読み直しても
    二重に出ない）。捨てた要素は進行の 1 行に理由を残し、run は失敗させない。
    - leaf の run: `self` はその leaf の key に書き換える。`self` の決定があれば leaf を done にせず `blocked(decision)`（reason `decision`。
      WU の commit はしない。worktree は残るので答えの後の run が続きをする）、それを待つ unit を ready にしない。他の unit を指した決定は
      その unit（`pending` / `ready`）を `blocked(decision)` にし、この run の完了は妨げない（§7 R3a (c)）。path は節点の path の最後の段に
      leaf の段階を書き、leaf の段（題名・unit key）を足す（パンくず「root › 段階 › leaf」）。
    - atomic の run（木の子）: `self` のまま節点の止めになり、run の完了を最終レビューに進めず `advance` で `ready` に戻す（答えの後の run で
      続きをする。答えは `answers`）。
    - **上限と束ね**: 新しく開ける数 = min(`max_open_decisions_per_plan` − この節点の未回答, `max_open_decisions_per_tree` − 木の未回答)。
      超えれば先頭 `残り − 1` 件を残し、残りを 1 件の `choice`（key `bundle`〈衝突すれば `bundle-2` …〉、選択肢「すべて推奨どおりに進める
      〈`all-recommended`、推奨〉/ note に書いた答えで進める〈`see-note`〉」、問いに元の問い・選択肢・推奨を列挙、`needed_before` は和、
      後戻りは最大）に束ねる。**新しい kind は足さない**（`bundle` は choice。残りが 0 でも 1 件には束ねる）。
12. **受信箱**: D7 の `AttentionItem::Decision` の代わりに、受信箱に独立の節 `decisions: DecisionInboxItem[]` と `counts.decisions` を足した
    （`attention` は task に紐づく「注意」の節で、決定は path・選択肢・推奨を持つ別の形のため。D14 の GUI の「決定」の節にそのまま対応する）。
    未回答で、決定を出した節点が終端でないものだけ（節点の中止で決定を自動で取り下げることはまだしない。終端の節点の決定は受信箱・通知・
    `DaemonSnapshot.decisions_open` から外すだけ）。
13. **通知**（`NotificationKind::DecisionRequested`、`celeris::notify::scan_decisions`）: 同じ run で出た決定を 1 通に束ねる。key は
    `plan:<plan_id>:decisions`（その run が planner run として採用した計画）/ `run:<run_id>:decisions`（worker の run・採用に至らなかった
    run）/ `decision:<id>`（run を持たない daemon の決定）。本文は 1 件なら「人の決定が必要: 『<root>』› <段階> › … : <question>（推奨: <label>、
    後戻り: 小|中|大）→ <link>」、複数なら件数と 1 行ずつ。初回は走査の下限（backfill 禁止）より後に出た決定を含む束だけ。束の最古の決定が
    24 時間未回答なら `reminder:<key>` で 1 回だけ再通知（重複排除は `notifications` の `(kind, key)`）。回答・取り下げ・終端の節点の決定は
    鳴らさない。GUI の通知の種類の語は「人の決定を待っている」。
14. **replay**: 回答・取り下げ・revise は既存の `DecisionRow::{apply_answer, apply_withdrawal}` の畳み込みのまま（revise は最後の回答で上書き）。
    unit の戻し（`decision_answered`）・取り下げ（`decision_withdrawn`）は普通の `WorkUnitTransitioned` なので replay は同じ行を作る。
    **R3a より前からある replay の差**: /3 の replan で計画から消えた unit（superseded の行）は最後の版の計画から作り直せない（`work_units` の
    presence の差。R3a の試験 `plan_invalid_replan_feeds_the_note_and_adopts_plan_v2` で見つけた。直していない）。
15. **R3a の試験で見つけた既存の挙動**（直していない）: 人の replan の依頼（`ExecutionHintSet{replan: true}`）で起きた replan の planner run が
    1 回目に不正だと、依頼は `Transitioned{to: running}` で消費済みなので 2 回目の試行が起きない（失敗が起点の replan は起点が残るので
    再試行される）。R3b 以降の生存確認（D10）で「理由なく止まっている」として見つかる形。
