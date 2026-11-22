---
tasks: [01M7XQ9QQXY1DY56EXE0TT8X6W]
---
# ADR 2026-11-22: Strategy Review — 既存の CoS が戦略候補と識別実験を設計し、既存の Reviewer が final review で Critic を兼ね、CoS の task.create が実験段階・Critic 未了・却下への逆戻り・失敗の未読を止める

- 日付: 2026-11-22
- 状態: Accepted
- 関係: 戦略層 v1 子 5（task 01M7XQ9QQXY1DY56EXE0TT8X6W）。子 2 の [ADR 2026-11-21-strategy-evidence-contract](2026-11-21-strategy-evidence-contract.md)
  （`celeris.strategy/1`・承認・`strategy_gate`）、子 3 の [ADR 2026-11-21-chat-discussion-mode](2026-11-21-chat-discussion-mode.md)（discuss・propose・
  execute、chat の案の添付）、子 0 の replay（`agent-docs/progress/2026-11-21-strategy-v1-0/replay/replay.md`、E-mid）、
  [ADR-0007](0007-phase5-planner-and-reviewer.md) D5（reviewer run は決定的な条件が全 pass のときだけ）、[ADR-0117](0117-review-human-decisions-and-check-results.md) D1（reviewer に人の決定を渡す）、
  [ADR 2026-11-17-boundary-change-risk-review](2026-11-17-boundary-change-risk-review.md) D3（final review に任意の節 `diff_design` を足して判定を求める前例）、
  [ADR 2026-10-27-review-resync-halt-visible-and-reviewer-reuse](2026-10-27-review-resync-halt-visible-and-reviewer-reuse.md) D2（reviewer の合格の持ち越し）。

## 背景

- アーキv4 の親は、子 A〜H を最初に全部起票し、途中で未達を認識しても代わりの戦略を作って採る前に固定した計画をほぼ最後まで進めた。
  子 0 の replay では、最初に未達を検知できた時点は E-mid（2026-11-18T20:36:35Z、子 A〜C が done、費用 171.06 USD）である。
- 子 2 は戦略の文書・承認・実行の入口の検査を作った。だが文書は「候補を比べる前の識別実験」「却下した候補」「停止条件」を持たず、
  入口は承認済みの版なら大きな計画（compound）もすぐ通す。Critic（候補の欠陥を探す独立の目）も無い。
- 依頼は、既存の CoS・Reviewer・Task だけでこれを足し、LLM を Dispatcher・TS 決定器に入れず、新しい Agent サービス・Manager・trait・状態機械・
  DB 表・event の種類・migration を足さず、strategy の無い task.create と通常の task の reviewer の prompt・run の数を変えないことを求める。

## 子 2・子 3 の ADR から使うもの・変えるもの

| ADR と決定 | この ADR での扱い |
|---|---|
| 子 2 D1.1・D1.2（schema `celeris.strategy/1`、欄、候補・仮説・証拠・参照） | 使う。schema 名は変えず欄を足す（D1）。候補の数の上限 1〜5 を「全 2〜8・open 3 まで」に**変える**（D1.3） |
| 子 2 D1.3（上限） | 新しい欄の上限を足す（D1.3）。既存の上限は変えない（候補の数だけ変える） |
| 子 2 D1.4（idempotency、同じ版の 2 つの sha256 は衝突） | 使う。Critic の対象（D3.1）も同じ規則で衝突を不合格にする |
| 子 2 D2.1（状態は文書に書かず決定から導く） | 使う。候補の `state`（open・rejected）は**候補**の採否で、文書の承認の状態ではない。文書の `approved`・`rejected` を書けない規則は変えない |
| 子 2 D2.2（承認は文書を作った task の決定 `strategy-approve-v<n>`、`human:` の回答だけ） | 使う。変えない。Critic との順を D3.7 で決める（人の承認 → Critic） |
| 子 2 D2.3（`refute` の証拠は次の版で消せない） | 使う。却下の根拠 `rejected_by` はこの証拠を指すので、却下の根拠も消えない（D4.1） |
| 子 2 D3（成果物・決定・`cos_operations` に置く。新しい表を足さない） | 使う。実験 task の集合も `cos_operations.payload_json.strategy` から導く（D2.3） |
| 子 2 D4（CoS の `task.create` の任意の欄 `strategy`、検査の位置、7 つの 422 の code、strategy の無い要求は 1 byte も変えない） | 使う。`StrategyRef` に任意の欄 `candidate` を足し、検査に 4 つの code を足す（D5）。位置（`goal_guard::check` の後・`authority_gate` の前）と、strategy の無い要求の扱いは変えない |
| 子 2 D6（`strategy_contract_`・`strategy_gate_` の試験、ablation の形） | 使う。同じ形（基準 B から 1 か所だけ変えた N で、ちょうどその code）で新しい規則の ablation を書く（D7）。既存の試験の基準の状態は D7.4 のとおり直す |
| 子 2 の未確認「`commit` の参照の存在は Critic と Evaluator で決める」 | Critic の基準 C1 で reviewer が確かめる（D3.4）。gate は今までどおり形だけ |
| 子 3 D1・D2（mode、discuss・propose の run は operation を全部拒否） | 使う。変えない。Strategy Review の起票は execute の run だけができる |
| 子 3 D3（propose の案は返事の添付 `strategy/<id>/v<n>.json`、検査は `task_core::strategy`） | 使う。案にも新しい欄が要る（D1 は Record・Execute の両方に掛かる）。chat の案は実行の入口を通らない（子 2 D4 の `strategy_source_missing`）ことは変えない |
| 子 3 D6（`ChatCitation`、引用の失効） | 使う。変えない |
| 子 3 D8.3（既存の試験を変えずに通す） | 文書の fixture（`crates/task-core/src/strategy/fixtures/baseline-v1.json`）は新しい欄を足すだけ直す。主張は変えない（D7.4） |

## 決定

### D1: `celeris.strategy/1` に足す欄

本番に `celeris.strategy/1` の文書は無い（2026-11-22T09:00:56Z に本番 DB を `mode=ro` で開いて数えた: `artifact_produced` の name が `strategy/%` 0 件、
`cos_operations` の `payload_json.strategy` 0 件、決定の key `strategy-approve-%` 0 件、`chat_attachments` の `relative_path` が `strategy/` を含む 0 件。
`celeris.strategy/1` の文字列を含む events 105 件はすべて計画・進捗・worker の文〈`worker_progress` 90・`execution_planned` 6・`worker_finished` 4・ほか 5〉で文書ではない）。
したがって schema 名を `celeris.strategy/1` のまま、欄を**必須**で足す。旧い形を読む互換の分岐は作らない。

#### D1.1 候補（`candidates[]` の 1 件）に足す欄

| 欄 | 型・値 | 意味と規則 |
|---|---|---|
| `state` | `open` か `rejected` | 候補の採否。文書の承認の状態（子 2 D2.1）とは別 |
| `rejected_by` | 証拠の id の列 | `rejected` のとき 1〜16 件で必須、`open` のとき空。各 id は同じ文書の `stance: refute` の証拠で、その証拠の `hypothesis` が候補の `hypotheses` にある（反証の無い却下をさせない） |
| `experiment` | `{content, discriminates[], budget{usd_max, wall_hours_max}}` | 最小の識別実験。`content` は実験の内容、`discriminates` は区別する仮説の id（1〜8 件、文書にあり、1 件以上が候補の `hypotheses` にある）、`budget` は候補の `cost` 以下（`usd_max ≤ cost.usd_max`、`0 < wall_hours_max ≤ cost.wall_hours_max`） |
| `expected_information_value` | 文 | 期待情報価値。実験の結果がどの候補の採否をどう変えるか |
| `stop_when` | 文の列 | 停止条件（1〜8 件）。この候補を止める・捨てる条件 |

既存の `selected` は残す。`selected: true` の候補は高々 1 件（子 2 のまま）で、`state` は `open` でなければならない。

#### D1.2 文書に足す欄

| 欄 | 型・値 | 意味 |
|---|---|---|
| `held_assumptions` | `{id, statement, revisit_when}` の列（0〜16 件） | 保留する前提。今は確かめずに置く前提と、確かめに戻る条件 |

#### D1.3 上限と数

| 対象 | 上限 |
|---|---|
| 候補 | 全体で 2〜8 件（却下した候補を次の版に残すため、子 2 の 1〜5 から変える）。`open` は 3 件まで。`selected` の無い版は `open` が 2〜3 件（比べる代替案） |
| `experiment.content`・`expected_information_value`・`held_assumptions[].statement` | 各 2,000 文字（子 2 の `TEXT_MAX_CHARS`） |
| `stop_when[]`・`held_assumptions[].revisit_when` の 1 件 | 500 文字（子 2 の `LIST_ITEM_MAX_CHARS`） |
| `held_assumptions[].id` | `[a-z0-9-]{1,40}`、文書の中で一意（子 2 の id と同じ規則） |

違反の code は子 2 の `strategy_invalid`。ただし `selected` の候補が `rejected` のときは `strategy_revisit_rejected`（D4.1）。
D1 の規則は `validate_document`（子 2）に入れ、Record（chat の案）と Execute（`task.create`）の両方に掛かる。

### D2: 実験段階（selected の無い承認済みの版は、識別実験だけを候補の数まで許す）

#### D2.1 段階は文書から導き、保存しない

参照した版に `selected` の候補が無ければ**実験段階**、あれば**実行段階**。段階の列・表・状態機械は足さない（規約 4）。

#### D2.2 `StrategyRef` に任意の欄 `candidate`

`/cos/operations` の欄 `strategy` を `{task_id, name, sha256, candidate?}` にする（`candidate` は `[a-z0-9-]{1,40}`、省略は `None`）。
`StrategyRef` は `deny_unknown_fields` のままで、欄を足すだけである。`cos_operations.payload_json.strategy.candidate` が「どの候補の実験か」の記録になる。

#### D2.3 実験 task の集合 X(v)

版 v（name と sha256）の実験 task の集合 X(v) は、`cos_operations` の `action = 'task.create'`・`state = 'applied'`・`payload_json.strategy.name` と
`.sha256` が v のものの `target_id` の集合である。保存しない。子 2 D3.4 の同じ照会（本番 3,911 行で 9 ms、2026-11-21）と同じ引き方で、表も索引も足さない。

#### D2.4 規則（violation の code は `strategy_experiment_phase`）

実験段階の版を参照する `task.create` は、次のすべてを満たすときだけ通る。

1. 本文の `execution` が `atomic`（既存の `NewTaskSpec.execution`・`task_core::ExecutionMode::Atomic`）。省略・`compound` は拒否。
2. `candidate` があり、参照した版の `open` の候補の id である（`rejected` の候補は D4.1 の `strategy_revisit_rejected`）。
3. X(v) に同じ `candidate` の task がまだ無い。したがって 1 つの版の実験 task は `open` の候補の数（2〜3）までである。
4. 本文に `max_wall_secs` があれば `experiment.budget.wall_hours_max × 3600` 以下（省略なら既存の既定 600 秒が掛かる）。

実行段階の版を参照する `task.create` は、`execution` を問わない（`compound` の本実装の計画を起票できる）。ただし次を満たす。

5. `candidate` は省くか、`selected` の候補の id（`selected` でない `open` の候補は `strategy_experiment_phase`、`rejected` の候補は `strategy_revisit_rejected`）。
6. 版の鎖（子 2 の `supersedes`）の前の版の X の合併が 1 件以上あり、そのすべてが終端（`done`・`failed`・`cancelled`）である。v1 に `selected` を書いた版・
   実験の結果を待たずに選んだ版は `strategy_experiment_phase`（結果を見てから選ぶ）。

### D3: Critic（既存の reviewer の 1 回の run が final review で判定する）

#### D3.1 Critic の対象（決定的に導く）

`task_core::strategy_critic::critic_subjects(events)` は、対象 task の events の `ArtifactProduced` のうち name が `strategy/<strategy_id>/v<n>.json` の形に
一致するものを、task のすべての run（WorkUnit の run を含む。review する run だけではない）から集め、`strategy_id` ごとに最大の版を返す。
承認の決定の後の継続の run は文書を作り直さないので、文書を作った run が review する run と違っても対象に入る。同じ name に 2 つの sha256 があれば
その対象は衝突として印を付け、Critic の条件を不合格にする（子 2 D1.4 と同じ規則）。対象が空の task は Critic と無関係である。

#### D3.2 reviewer run を起こす条件

`review_spawn` の条件を `needs_reviewer_run(&task) || delivery_requires_reviewer_run || !critic_subjects.is_empty()` にする。対象が空なら条件は base と同じで、
reviewer run の数は変わらない。対象がある task では、`Check::Reviewer` の条件が無くても reviewer run を 1 回起こす（`delivery_requires_reviewer_run` と同じ前例）。
対象がある task では `reviewer_reuse`（持ち越し）を使わない。Critic は常に今の文書に対して判定させる。

#### D3.3 暗黙の条件 `strategy_critic`

`review_task` は、対象があるとき暗黙の条件 `strategy_critic` を `research_evidence` の次の番号に足す（plan・aggregate・repo_check・research と同じ置き方）。
この条件の合否は reviewer の出力の `critic` の節（D3.5）だけで決まる。決定的な条件が 1 つでも落ちれば、既存の規則（ADR-0007 D5）どおり reviewer は判定せず
`strategy_critic` も不合格になる。reviewer run が無い・節が無い・形が崩れている・対象と食い違う・どれか 1 つでも `ok: false` なら不合格である。
したがって **Critic を判定しない review は合格にならない**。

#### D3.4 reviewer への入力 `ReviewRequest.strategy_critic`

`ReviewRequest` に `strategy_critic: Option<Box<StrategyCriticRequest>>`（`skip_serializing_if = "Option::is_none"`）を足す。対象が無い task では `None` で、
`request.json` も reviewer の prompt も base と 1 byte も変わらない（`diff_design` と同じ形）。`PROTOCOL_VERSION` は上げない（欄を足すだけ。子 2 D5.1 と同じ理由）。

- `criterion_idx`: D3.3 の番号。
- `subjects[]`: `{name, path, sha256, strategy_id, version}`（D3.1）。reviewer は workspace の `path` を読む。
- `experiments[]`: 鎖の前の版の X の task（D2.3）の `{version, candidate, task_id, status, summary_tail}`。`summary_tail` は最後の run の要約の末尾 1,000 文字
  （`CHECK_REASON_TAIL_CHARS` と同じ）。dispatcher が store から集める（反証・失敗の記録）。
- `rubric[]`: 下の固定の基準（`task_core::strategy_critic::CRITIC_RUBRIC` の定数。文書や run が変えられない）。
  - C1 欠陥: 候補ごとに、根本原因の仮説・approach・費用・risk・保留する前提の欠陥を探して挙げる。`commit` の参照が実在するかも確かめる。
  - C2 反証条件: 候補の仮説の `falsified_by` が観測で確かめられる条件か。
  - C3 識別実験: `experiment` が `discriminates` の仮説を区別でき、予算が最小か。
  - C4 比較: 候補を期待情報価値・費用・risk で比べたか。`selected` があれば、`experiments` の結果と証拠から選んだ理由が成り立つか。
  - C5 逆戻り: `open` の候補が、同じ文書に残る `rejected` の候補の approach を名前を変えて出していないか。`experiments` の失敗・反証を証拠が読んでいるか。

reviewer は既存の ADR-0117 D1 の `decisions` で人の承認（`strategy-approve-v<n>`）も見る。承認した sha256 と対象の sha256 が違えば C1 の欠陥として挙げる。

#### D3.5 reviewer の出力 `ReviewOutput.critic`

`ReviewOutput` に `critic: Option<CriticVerdictOut>`（省略で読める）を足す。形は
`{subjects: [{name, sha256, candidates: [{id, defects: [文], falsification: {ok, reason}, experiment: {ok, reason}}], comparison: {ok, reason}, revisit: {ok, reason}}]}`。

dispatcher（`task_dispatch::review::critic`）は形だけを確かめる。LLM は呼ばない。

- `subjects` の `(name, sha256)` の集合が D3.1 の対象と一致する。
- 各対象の `candidates` の id の集合が、文書の `candidates[].id`（dispatcher が file の JSON から id だけを読む。文書の検査は gate の仕事で、ここではしない）と一致し、重複が無い。
- すべての `reason` が空でなく 2,000 文字以内。
- 合格はすべての `ok` が `true` のときだけ。`defects` は空でもよい（欠陥を挙げても致命でなければ `ok` にできる）。

不合格の理由は `strategy_critic: <食い違い・ok=false の項目>` として verdict に残る（人は task の review で読める）。

#### D3.6 reviewer の prompt

`task_worker::change_rules::review` に `critic_section` を足す。`strategy_critic` が `None` なら空文字列を返し、prompt は変わらない。`Some` なら対象・実験の記録・
C1〜C5 と出力の形（`final_example` に `critic` を加える。`diff_design` と同じ置き方）を出す。

#### D3.7 人の承認（子 2 D2.2）と Critic の順: 人の承認 → Critic

1. 文書を作る task の run が `strategy/<id>/v<n>.json` を書き、決定 `strategy-approve-v<n>`（`needed_before: ["self"]`）を出す。task は人の答えを待つ。
2. 人が Web の受信箱か人の token で `approve` か `reject` を答える（子 2 D2.2 のまま）。
3. 継続の run は答えを確かめて要約を書く（文書を作り直さない）。
4. final review が Critic（D3.3）を判定する。合格なら task は `done`。

採る理由: (a) 既存の仕組みの順のままである（`needed_before: self` の決定は継続の run の前に答えが要り、`Check::Human` の承認も reviewer run より先に解決する）。
順を入れ替えるには新しい待ちか状態が要る。(b) Critic は ADR-0117 D1 の `decisions` で人の答えを見て、承認した sha256 と文書の一致を確かめられる。
(c) 承認と Critic の両方が要る。Critic は拒否だけができ、承認の代わりにならない。人の承認も Critic を飛ばせない。
(d) 人は Critic の判定を task の review で読んだ後、既存の `decision.revise` で答えを `reject` に改められる（子 2 の gate は最後の `DecisionAnswered` を読む。
revise が `DecisionAnswered` を追記することは実装の WU が試験で確かめる）。

gate の規則（code `strategy_uncritiqued`）: 参照した文書を作った task（`strategy.task_id`）の status が `done` でない、または参照した `(name, sha256)` が
その task の `critic_subjects` に無い（同じ task の古い版・衝突した版）なら拒否する。task が reopen されれば `done` でなくなり、再び review を通るまで拒否される。

### D4: 再評価（却下した候補に戻らない、失敗の記録を読む）

#### D4.1 却下した候補は消せず、選べない（code `strategy_revisit_rejected`）

- 鎖の隣り合う 2 版（v<sub>n−1</sub> → v<sub>n</sub>）のすべてで、v<sub>n−1</sub> の `rejected` の候補は v<sub>n</sub> に同じ id・同じ内容（JSON として等しい）で残る。
  消した・`open` に戻した・書き換えた版は拒否する（`validate_document` の鎖の検査。子 2 D2.3 の反証の保存と同じ書き方）。
- `selected` の候補が `rejected` なら拒否する。
- `task.create` の `candidate` が `rejected` の候補なら拒否する。
- 名前を変えて同じ approach を出すことは決定的に見分けられないので、Critic の C5 が受け持つ。

#### D4.2 前の版の実験の終端の記録を次の版が読む（code `strategy_failure_unread`）

参照した版 v<sub>n</sub>（n ≥ 2）で、鎖の前の版 v<sub>1</sub>〜v<sub>n−1</sub> の X の合併（D2.3）のうち gate の時点で終端（`done`・`failed`・`cancelled`）の task は、
すべて v<sub>n</sub> の証拠の参照（`task` の `task_id`、または `artifact` の `task_id`）に現れなければならない。現れない task が 1 件でもあれば拒否する。
v<sub>n</sub> を書いた後に前の版の実験が終われば、v<sub>n</sub> は拒否され、その記録を読んだ v<sub>n+1</sub> が要る。Record（chat の案）には掛けない（実験の集合が無い）。

### D5: gate の順と 422 の code

検査は子 2 D4 の位置（`goal_guard::check` の後、`authority_gate` の前）のまま。子 2 の検査に続けて D1〜D4 の規則を当て、最初の違反で止まらず全部を返す
（子 2 の `rules`・`codes`・`violations` の形）。断った要求は既存の `OperationAudit::reject` で `rejected` の行と監査を残す。`instructed_by`（人の発言の中継）でも外れない。

| code（すべて 422） | 規則 | 置く所 |
|---|---|---|
| `strategy_experiment_phase` | D2.4 | `task_core::strategy::review::check_execution`（純関数） |
| `strategy_uncritiqued` | D3.7 | 同上（`done` と `critic_subjects` は gate が引いて事実として渡す） |
| `strategy_revisit_rejected` | D4.1 | 鎖と `selected` の規則は `validate_document`、`candidate` の規則は `check_execution` |
| `strategy_failure_unread` | D4.2 | `check_execution`（X と task の status は gate が引いて渡す） |

子 2 の 7 つの code（`strategy_source_missing`・`strategy_stale`・`strategy_other_project`・`strategy_unapproved`・`strategy_invalid`・`strategy_refute_removed`・
`strategy_idempotency_conflict`）と `strategy_not_applicable` は変えない。`check_execution` は `CheckContext`（子 2）を変えずに、別の引数
`ExecutionFacts {source_done, critiqued, experiments: [{version, task_id, candidate, terminal}], execution, candidate, max_wall_secs}` を受ける。
したがって chat の案の検査（子 3 の `cos_context::discussion_files`）の呼び出しは変わらない。

strategy の無い `task.create` は、検査・応答・監査を今と 1 byte も変えない（子 2 D4 のまま）。

### D6: CoS の手順

- `config/skills/cos-operator/strategy-review.md`（新しい file）と `SKILL.md` の索引の 1 行に書く。
  - いつ使うか: 人の依頼が成果（親の目的）を求め、根本原因か方法が決まっていない抽象的な依頼の開始時。人が戦略の比較を求めたとき。
    明確なバグ修正・人が手順を指定した変更・低抽象度の task には使わない（strategy の無い `task.create` のまま）。迷えば人に聞く。
  - 手順: (1) execute の run で、文書を作る task を `strategy` 無しで起票する。objective に D1 の欄（根本原因の仮説・代替案 2〜3・最小の識別実験・反証条件・
    保留する前提・期待情報価値・予算・停止条件）と、決定 `strategy-approve-v<n>`（`needed_before: ["self"]`）を出すことを書く。
    (2) 人の承認と Critic を待つ（D3.7）。(3) 実験段階の版を参照し、`open` の候補ごとに `execution: atomic` の実験 task を 1 件ずつ起票する（`candidate` を書く）。
    A〜H 型の計画を最初に起票しない。(4) 実験が終わったら、すべての終端の実験 task を証拠で参照し、却下する候補を `rejected_by` 付きで残し、
    `selected` を決めた次の版の task を起票する。(5) 実行段階の版を参照して本実装を起票する。(6) 再評価（子 6 の checkpoint など）も次の版で行う。
  - 拒否の code ごとの次の手: `strategy_experiment_phase` → 実験 task だけを起票するか次の版を作る。`strategy_uncritiqued` → 文書の task の review を待つ。
    Critic の不合格なら指摘を直した次の版を作る。`strategy_revisit_rejected` → 却下した候補を使わない。戻す必要があると考えるなら人に聞く（大きな戦略変更は人だけ）。
    `strategy_failure_unread` → 終端の実験 task をすべて証拠で参照した次の版を作る。
  - 同じ研究質問に新しい `strategy_id` を立てて却下を避けない。新しい系列は人の指示（`instructed_by`）があるときだけ。
- `crates/task-worker/src/cos_chat.rs` の propose の案の説明（`discussion_files_guide`）に D1 の欄を足す（chat の案も `validate_document` を通るため）。
  discuss・propose の run が operation を使えないこと（子 3）は変えない。

### D7: 試験

#### D7.1 prefix と置き場所

| prefix | crate と file | 守る不変条件 |
|---|---|---|
| `strategy_review_contract_` | task-core `strategy/review.rs` の試験・`strategy/tests.rs` | D1〜D4 の純関数の規則。ablation: 基準 B2（実験段階の版・atomic・open の候補・done で対象・失敗を全部参照）から 1 か所だけ変えた N-phase・N-crit・N-revisit・N-unread が、ちょうどその code の集合を返す |
| `strategy_critic_` | task-core `strategy_critic.rs` の試験、task-worker `change_rules/review` の試験・protocol の試験、task-dispatch `review/critic.rs` の試験・`dispatcher/tests/` の新しい file | D3。対象の導出、reviewer run の条件、`strategy_critic` の合否、対象の無い task の `request.json`・prompt・reviewer run の数が変わらないこと |
| `strategy_gate_review_` | task-api `cos/strategy_gate/` の試験（新しい file `review_tests.rs`） | D2・D3.7・D4・D5 を `/cos/operations` で。422 と code、task が作られないこと、`rejected` の監査、`instructed_by` でも外れないこと |
| `strategy_review_shadow_` | task-api `tests/strategy_review_shadow.rs` | 子 0 の E-mid までの記録だけを使う shadow ケース（D7.3） |
| `strategy_review_regression_` | task-api `tests/strategy_review_regression.rs` | strategy の無い `task.create` の応答・`cos_operations` の行・監査が変わらない。案件に実験段階の戦略があっても、strategy の無い起票は影響を受けない |

#### D7.2 主な試験と変更前に落ちること

- `strategy_review_contract_rejected_removed_next_version_is_revisit`・`…_selected_rejected_is_revisit`・`…_rejected_needs_refute_evidence`・
  `…_open_candidates_two_to_three_without_selected`・`…_experiment_budget_within_cost`・`…_execution_compound_in_experiment_phase`・
  `…_second_experiment_same_candidate`・`…_selected_needs_terminal_experiments`・`…_uncritiqued_source_not_done`・`…_uncritiqued_older_version_in_done_task`・
  `…_failure_unread_terminal_experiment`・`…_ablation`。base の型は `state`・`experiment` などを知らず `deny_unknown_fields` で落とすので、期待した code の集合にならず落ちる。
- `strategy_critic_subjects_latest_version_over_all_runs`・`strategy_critic_subjects_empty_without_strategy_artifact`・`strategy_critic_same_name_two_sha_fails`・
  `strategy_critic_spawns_reviewer_without_reviewer_criterion`・`strategy_critic_missing_section_fails_review`・`strategy_critic_subject_sha_mismatch_fails`・
  `strategy_critic_candidate_missing_fails`・`strategy_critic_not_evaluated_when_deterministic_fails`・`strategy_critic_reuse_not_applied`・`strategy_critic_all_ok_passes`。
  base では戦略の成果物を作った task の review に暗黙の条件が無く、Critic の節の無い `review.json` で合格するので、不合格を期待する試験は落ちる。
- 回帰（base でも通る）: `strategy_critic_absent_request_json_unchanged`（`strategy_critic` の key が無い）、`strategy_critic_absent_prompt_unchanged`（Critic の見出しが無く、
  対象の無い task の prompt が同じ入力の base の prompt と等しい）、`strategy_critic_absent_reviewer_run_count_unchanged`（`Check::Reviewer` の無い task は 0 回、ある task は 1 回）、
  `strategy_critic_old_review_json_without_critic_parses`。
- `strategy_gate_review_compound_in_experiment_phase_is_422_and_creates_nothing`・`…_atomic_experiments_up_to_open_candidates`・`…_rejected_candidate_task_is_revisit`・
  `…_source_task_not_done_is_uncritiqued`・`…_failure_unread_is_422`・`…_selected_without_experiments_is_422`・`…_selected_version_allows_compound`・
  `…_refusals_are_audited`・`…_instructed_by_cannot_waive`。base の gate は承認済みの版なら compound も 2 件目の実験も通して task を作るので、422 を期待する試験は落ちる。

#### D7.3 shadow ケース（子 0 の E-mid までの記録だけ）

- 入力: 子 0 の `replay.py build` が作る slice のうち E-mid（cutoff 2026-11-18T20:36:35Z）の `goal.md`・`events.jsonl`・`slice.json` を
  `crates/task-api/tests/fixtures/strategy_review_shadow/` に写す。試験は各 file の sha256 が `replay/manifest.sha256` の行と一致すること、event の ts が
  すべて cutoff 以下であること、`answer_key.json` を読まないこと（fixture の dir に無く、試験が path を開かない）を最初に確かめる。
- 筋書き: 案件の request を `goal.md` にする → v1（候補 3 件。1 件は子 A〜H の固定計画、ほかは識別実験を先に行う案。`selected` 無し）を人が承認し、
  文書の task を done にする → A〜H の compound の起票は `strategy_experiment_phase` → open の候補ごとの atomic の実験 3 件は通り、4 件目は拒否 →
  実験の結果の証拠に E-mid の slice の中の文（D#1446「未達/要判断として書いたもの: handlers ⇄ cos」、C の進捗「17・198・756」）を引く（試験は文の実在を確かめる）→
  v2 が固定計画の候補を `rejected_by` 付きで却下し、別の候補を `selected` にして実験 task を全部参照 → compound の本実装が通る →
  v3 で却下した候補を `selected` にすると `strategy_revisit_rejected`、終端の実験 task を落とすと `strategy_failure_unread`。
- 数える値: 戦略を選ぶまでに起票した task の数（文書の task 2 と実験 3。v4 の実際の初回起票は子 8 件）。
- Critic の LLM の判定はこの試験では動かさない（文書の task を done に置く）。Critic の判定の質は子 6・子 8 の pilot で確かめる（未確認）。

#### D7.4 既存の試験の直し方（弱めない）

- `crates/task-core/src/strategy/fixtures/baseline-v1.json`（子 2・子 3 の試験が `include_str!` で共有）は D1 の欄を足し、候補を 2〜3 件にする。主張は変えない。
- 子 2 の `strategy_gate_` の基準の要求は、実験段階の版に合わせて `execution: atomic` と `candidate` を足し、文書の task を `done` にする。N-x がちょうどその code になる主張は変えない。

### D8: WorkUnit への割り当てと write set

| WorkUnit | 段 | 書く場所（これ以外は書かない） |
|---|---|---|
| contract-review | impl | `crates/task-core/src/strategy/`（`mod.rs` の D1・D4.1、新しい `review.rs` の `check_execution`、試験、`fixtures/baseline-v1.json`）。D7.4 の fixture を使う試験 file は fixture の写しを持たないので触らない |
| critic-review | impl | `crates/task-core/src/strategy_critic.rs`（と試験）・`crates/task-core/src/lib.rs`（`mod` の 1 行）、`crates/task-worker/src/protocol.rs`・`change_rules/review.rs`（と試験）、`ReviewRequest {`・`ReviewOutput {` の literal を持つ試験 file（欄 `None` を足すだけ）、`crates/task-dispatch/src/review.rs`・`review/critic.rs`（新）・`dispatcher/review_spawn.rs`・`dispatcher/tests/`（新しい試験 file と `mod` の 1 行）、`docs/protocol/worker-protocol.md`・`worker-protocol.schema.json` |
| cos-skill | gate | `config/skills/cos-operator/strategy-review.md`（新）・`SKILL.md`、`crates/task-worker/src/cos_chat.rs`・`cos_chat/tests.rs` |
| experiment-gate | gate | `crates/task-api/src/cos/strategy_gate.rs`・`strategy_gate/`（試験）、生成物 `docs/api/v1/api-v1.schema.json`（`StrategyRef` が載る。生成器が web の生成型を変えたときだけその生成 file） |
| shadow-case | shadow | `crates/task-api/tests/strategy_review_shadow.rs`・`strategy_review_regression.rs`・`tests/fixtures/strategy_review_shadow/` |
| close | close | `agent-docs/progress/2026-11-22-strategy-v1-5.md` |

- 各 WU は自分の進捗を `agent-docs/progress/2026-11-22-strategy-v1-5/<key>.md` に書き、この ADR に実装時の改訂を付記してよい。
- contract-review と critic-review は同じ段で並ぶ。critic-review は文書を `StrategyDocument` で読まず（候補の id だけを JSON から読む）、`task_core::strategy` を変えない。
  したがって 2 つの WU は同じ file を書かず、contract-review が文書の型を変えても critic-review の試験の fixture は壊れない。
- `crates/task-dispatch/src/health_review/design_gate.rs`（高 risk の class `review-gate`）は `ReviewRequest` を `..Default::default()` で作るので触らない。
  `crates/task-api/src/cos/operations.rs`（class `authz`）も触らない（`strategy` の欄は `StrategyRef` の中で増え、呼び出しは変わらない）。
  critic-review は final review の合否（`completion-not-self-reported`）に触れるので、計画の設計 brief にこの ADR を挙げる。
- 子 4（外部 LLM との接続、task 01M7XQ9QK1KC34Q2A6PJ960TJB）と重ねない: 子 5 は `crates/celeris-mcp/`・chat の import（`crates/task-api/src/chat/`・
  `crates/task-core/src/chat/`）・`web/` の画面・`docs/api/v1/api.md` を書かない。子 4 が外部の文を `celeris.strategy/1` の文書にする試験を足すなら、
  その fixture は D1 の欄を要する。先に統合した側に後の側が合わせ、子 4 は `crates/task-core/src/strategy/` を書かない（要るなら報告して子 5 の統合を待つ）。
  2026-11-22 の時点で子 4 の branch に子 3 の統合より後の commit は無い。

## 足さないもの

新しい DB 表・migration・event の種類・Agent サービス・Manager・trait・状態機械は足さない。足すのは、文書の欄（D1）、`StrategyRef.candidate` 1 つ、
`ReviewRequest.strategy_critic` と `ReviewOutput.critic` の任意の節 2 つ、暗黙の条件 `strategy_critic` 1 つ、純関数（`check_execution`・`critic_subjects`）、
422 の code 4 つである。段階・Critic の済み・却下は保存せず、文書・`ArtifactProduced`・task の status・`cos_operations` から毎回導く。
Critic は既存の reviewer run（既存の routing・admission・予算）で、dispatcher は reviewer の出力の形を確かめるだけで LLM を呼ばない。TS の決定器は変えない。
Project.request と承認済み Roadmap は読むだけで書かない。

## 実装前 brief

1. 根本原因: 戦略の文書が識別実験・却下・停止条件を持たず、実行の入口が承認済みの版なら大きな計画も通す。候補の欠陥を探す独立の判定が無く、
   前の版の失敗を読んだかを誰も確かめない。症状はアーキv4 の固定計画の完走に出た。
2. 責務 owner: 文書の規則は `task_core::strategy`（純関数）。入口は `task_api::cos::strategy_gate`。Critic の対象は `task_core::strategy_critic`、判定の起動と
   合否は `task_dispatch::review`（`review_task`・`review_spawn`）、reviewer の prompt は `task_worker::change_rules::review`。手順は CoS の skill。
3. 既存の再利用先: 子 2 の `validate_document`・`strategy_gate`・決定の承認、子 3 の mode と案の添付、`cos_operations`、`ArtifactProduced`、
   `review_task` の暗黙の条件の置き方、`diff_design` の任意の節の前例、`delivery_requires_reviewer_run` の前例、ADR-0117 の `decisions`、`OperationAudit::reject`。
4. 選択肢: (a) Critic を新しい task・新しい Agent にする → 依頼が禁じ、reviewer run が既にあるので採らない。(b) Critic を `Check::Reviewer` の条件として
   acceptance に足すよう CoS に頼む → CoS が書き忘れれば Critic が無いまま合格するので採らない。(c) 段階や Critic の済みを列・event に保存する → 正本が 2 つに
   なるので採らない。(d) Critic を人の承認より先にする → 新しい待ちか状態が要るので採らない（D3.7）。(e) strategy の無い起票も案件の戦略で止める →
   通常の task の経路を変えるので採らない（未確認の節）。採用: 文書の欄＋入口の純関数の規則＋final review の暗黙の条件。
5. 新しい状態・分岐・fallback・port: 状態は足さない（導く）。分岐は `review_spawn` の条件に 1 項、`review_task` の暗黙の条件 1 つ、gate の規則 4 つ。
   fallback と新旧切替の flag は無い。旧い形の文書を読む分岐も作らない（本番に 0 件）。
6. 撤去対象: 子 2 の候補の数の上限 1〜5（`CANDIDATES_MAX = 5`）を D1.3 に置き換える。消すコード経路は無い（欄と規則を足すだけ）。
7. 不変条件: 下の節。試験は D7。

## 守る不変条件

- 実験段階の版を参照する `task.create` は、`open` の候補ごとに 1 件の atomic の実験だけが通る。compound の計画は `selected` のある版（実験の結果を見て選んだ版）が要る。
- 戦略の成果物を作った task は、Critic の条件に合格しないと `done` にならず、`done` でない task の文書は実行の入口を通らない。
- 却下した候補は次の版で消えず、選べず、実験・実行の対象にできない。前の版の実験の終端の記録を参照しない版は実行の入口を通らない。
- strategy の無い `task.create` の検査・応答・監査、戦略の成果物の無い task の reviewer の prompt・`request.json`・reviewer run の数は base と同じ。
- dispatcher・gate に LLM を入れない。events は追記だけ。Project.request と承認済み Roadmap を書き換えない。新しい表・event の種類・migration は無い。

## 未確認と残る穴

- strategy の無い `task.create` は止めないので、CoS が `strategy` を付けずに大きな計画を起票すれば実験段階を通らない。通常の task の経路を変えない条件と引き換えで、
  監査（`cos_operations` に strategy の無い行が残る）と子 6 の Evaluator で見つける。人がこの穴を閉じたいなら、案件の状態で起票を止める設計を人が決める。
- 新しい `strategy_id` を立てて却下した候補を出し直すことは gate では見分けない（CoS の skill の禁止と Critic の C5 だけ）。
- Critic（reviewer）の判定の質と、人の承認 → Critic の順で人が Critic の指摘を読んで revise するかは、子 6・子 8 の pilot で確かめる。
- `decision.revise` が `DecisionAnswered` を追記して gate が最後の答えを読むことは、コードを読んだ範囲の推定で、実装の WU が試験で確かめる。
