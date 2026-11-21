---
tasks: [01M7XQ9Q9EB3CH59TSNYM9HBC8]
---
# ADR 2026-11-21: 戦略・仮説・証拠の契約 celeris.strategy/1 を既存の成果物・決定・CoS operations の上に置き、Project.request の全文を実 prompt に出す

- 日付: 2026-11-21
- 状態: Accepted
- 関係: 戦略層 v1 子 2（task 01M7XQ9Q9EB3CH59TSNYM9HBC8）。子 0 の設計 brief §5 の envelope と規則 R1〜R13
  （`agent-docs/progress/2026-11-21-strategy-v1-0/design-brief.md`、検査器 `fixtures/validate.py`）。子 1 の prompt-audit の欠け 5
  （`agent-docs/progress/2026-11-21-strategy-v1-1/prompt-audit.md`）と
  [ADR 2026-11-21-ancestor-objective-in-prompts](2026-11-21-ancestor-objective-in-prompts.md)（「欠け 5 を扱わない理由」と D4 をこの ADR が
  一部置き換える。下の D5.6）。[ADR 2026-11-04-project-controller](2026-11-04-project-controller.md) 付記 A5（Roadmap と `request_sha256`）、
  [ADR-0079](0079-recursive-task-decomposition.md) D7（人への決定）、[ADR 2026-10-27-event-principals](2026-10-27-event-principals.md)（`by` の主体）。

## 背景

- 人の依頼は、戦略候補・仮説・支持/反証/未確認の証拠・元の研究質問・判定方法・費用とリスク・根拠 refs・再評価条件を、版と引用つきで
  つなぐ契約を求める。同時に、新しい DB 表・Goal の正本・Manager・trait・Agent サービスを作らないことを求める。
- アーキv4 の親は、子 A〜H が done でも親の目標を満たさなかった。親は未達を認識したが、代わりの戦略を記録し採る場所が無く、
  固定した計画をほぼ最後まで進めた。この契約は、戦略と反証を版つきで残し、承認されていない戦略を実行の入口で止める最小の部品である。
- 子 1 の prompt-audit は、Project.request（人の元の目的の正本）が実 prompt のどこにも入らないことを見つけた（15 run で 0/15）。人は
  この欠けを子 2 で扱うと決めた（2026-11-21 のコメント）。戦略の文書は Project.request の sha256 を持つので、run の prompt にも同じ全文と
  sha256 を出す。

## 決定

### D1: schema `celeris.strategy/1`

1 つの戦略の 1 版を 1 つの JSON 文書にする。文書は task の成果物（D3）として置く。

#### D1.1 文書の欄

| 欄 | 型・値 | 意味 |
|---|---|---|
| `schema` | `"celeris.strategy/1"` 固定 | 版の違う文書を誤って読まない |
| `project_id` | 案件の id | 文書の案件。作った task の案件と一致する（D3.4） |
| `strategy_id` | `[a-z0-9-]{1,40}` | 戦略の系列の名前。版をまたいで同じ |
| `version` | 1〜64 の整数 | 系列の中の版の番号 |
| `supersedes` | `null`（`version` = 1 のときだけ）か、前の版の成果物の参照 `{task_id, name, sha256}` | 版の鎖。前の版は `version - 1` で同じ `strategy_id`・`project_id` |
| `change_reason` | `version` > 1 で必須の文 | 前の版から何を変えたか |
| `idempotency_key` | `strategy:<strategy_id>:v<version>` と完全に一致する文字列 | D1.4 |
| `source` | `{kind, label, thread_id?}` | 文書を作った場所。`kind` は brief の 4 種（`celeris_cos_thread`・`human_paste`・`mcp_client`・`markdown_import`）と `celeris_task_run`（この案件の task の run が成果物として作った） |
| `actor` | `{kind, id}` | 作った主体。`celeris_task_run` では `kind: "worker"`、`id` は run の id で、成果物の `ArtifactProduced.run_id` と一致する（D3.4） |
| `source_revision` | `{request_sha256, roadmap_version}` | 文書が前提にした Project.request の sha256（64 桁の小文字 hex）と、承認済み Roadmap の版（承認済みの版が無い案件では `null`） |
| `timestamp` | RFC 3339 | 文書を作った時刻 |
| `consent` | `{granted_by, scope, granted_at}` | 外部の文（ChatGPT の貼り付け・MCP・Markdown の import）を含むとき、人が与えた許可。`granted_by` は `human:` の主体、`scope` は `selected_excerpt` か `summary`（brief R8） |
| `acl` | `{classification, outbound}` | `classification` は `internal` か `confidential`。`outbound` は既定 `false`。`confidential` を外へ出すには人の承認が要る（brief R13。外へ出す経路は子 4） |
| `decision_state` | `discussion`・`proposed`・`withdrawn` のどれか | 作った側の状態。`approved`・`rejected` は文書に書けない（D2.1） |
| `research_question` | `{text, references[]}` | 元の研究質問。references は 1 件以上 |
| `candidates[]` | 1〜5 件 | 戦略候補（下の D1.2） |
| `hypotheses[]` | 1〜32 件 | 仮説（D1.2） |
| `evidence[]` | 0〜256 件 | 証拠（D1.2） |

文書の `content_hash` は文書の中に持たない。成果物の file の byte 列の SHA-256 で、dispatcher が成果物の申告のときに計算して
`ArtifactProduced.artifact.sha256` に記録した値を使う（自己申告ではない）。人・API・決定の文が文書を指すときは `sha256:<64 桁 hex>` と書く。
文書が自分自身の hash を含むと hash が決まらないので、brief §5 の `content_hash` の欄はこの外部の値で満たす。

#### D1.2 候補・仮説・証拠・参照

- 候補 `{id, title, approach, decision_method, cost{usd_max, wall_hours_max, note}, risks[], reevaluate_when[], hypotheses[], selected}`。
  `decision_method` は候補を採るか捨てるかを決める判定方法、`reevaluate_when` は再評価の条件（1〜8 件）、`hypotheses` は仮説の id。
  `selected` が `true` の候補は高々 1 件。
- 仮説 `{id, statement, confidence, falsified_by, evidence[]}`。`confidence` は `confirmed`・`estimated`・`contradicted`・`needs_decision`
  （確認済み・推定・矛盾・要判断）。`falsified_by` は反証の条件。`evidence` は証拠の id。
- 証拠 `{id, stance, hypothesis, statement, observed_at, source, references[]}`。`stance` は `support`・`refute`・`unknown`
  （支持・反証・未確認）。`source` は D1.1 と同じ 5 種で、外部の 3 種（`human_paste`・`mcp_client`・`markdown_import`）は untrusted として扱う。
- 参照 `references[]` の 1 件は次の 6 種のどれか。

| `kind` | 欄 | gate が確かめること（D3.4） |
|---|---|---|
| `commit` | `repo`・`sha`（40 桁の小文字 hex） | 形と、`repo` が案件に登録されたリポジトリであること。commit の存在は reviewer が確かめる（gate は git を起こさない） |
| `url` | `url`（論文など） | `https://` で始まり、userinfo を持たず、2,048 文字以内。中身は untrusted |
| `task` | `task_id` | task があり、案件が同じ |
| `knowledge` | `path`・`sha256` | page があり、front matter の scope が `project:<この案件の slug>` か案件でない scope（`user`・`environment` など） |
| `chat` | `thread_id`・`message_id` | message が残っている（撤回・編集での失効は子 3 が決める）。thread の案件がこの案件か、案件の無い統括の thread |
| `artifact` | `task_id`・`name`・`sha256` | 同じ task に一致する `ArtifactProduced` があり、task の案件が同じ |

#### D1.3 入力長の上限（数）

| 対象 | 上限 |
|---|---|
| 文書の file | 262,144 byte（UTF-8） |
| `research_question.text` | 4,000 文字 |
| `title`・`approach`・`decision_method`・`statement`・`falsified_by`・`change_reason`・`cost.note` | 各 2,000 文字（Roadmap の `ROADMAP_TEXT_MAX` と同じ） |
| `risks[]`・`reevaluate_when[]` の 1 件 | 500 文字。`risks` は 0〜16 件、`reevaluate_when` は 1〜8 件 |
| `source.label` | 200 文字 |
| id（`strategy_id` と候補・仮説・証拠の id） | `[a-z0-9-]{1,40}`、種類ごとに文書の中で一意 |
| 参照 | 1 件の証拠・研究質問につき 1〜16 件、文書の全体で 1,024 件 |
| `url` | 2,048 文字 |
| 版 | 1〜64（gate が鎖を辿る回数の上限も 64） |

秘密に見える文字列（brief R9 の token・cookie・webhook の形）を含む文書は拒否する。

#### D1.4 idempotency と版

- `idempotency_key` は `strategy:<strategy_id>:v<version>` と一致しなければならない（純粋関数で検査する）。
- 同じ key（同じ系列の同じ版）を持つ文書の sha256 が 2 つあれば衝突として拒否する。gate は次の 2 か所で見る。
  (a) 成果物の name を `strategy/<strategy_id>/v<version>.json` に固定するので、同じ task の `ArtifactProduced` に同じ name で違う sha256 が
  あれば上書き（衝突）。(b) 承認の決定（D2.2）は文に `strategy/<strategy_id>/v<version>.json` と sha256 を書くので、`decisions` の表に
  同じ版の名前で違う sha256 の承認があれば衝突。
- 同じ key で同じ sha256 の成果物が 2 回記録されたとき（run の再試行）は同じ文書として扱い、二重に数えない。
- CoS の operation の `idempotency_key`（既存。D4）は operation の要求全体（`strategy` の欄を含む）の hash に掛かる。同じ key で同じ要求は
  同じ task を返し、違えば既存どおり 409 にする。

### D2: discussion note と approved strategy の区別、承認、反証の保存

#### D2.1 状態は文書に書かず、決定から導く

- 文書の `decision_state` は作った側の状態（`discussion`・`proposed`・`withdrawn`）だけを持つ。`approved`・`rejected` を書いた文書は
  契約違反として拒否する。承認を文書に書くと、承認の正本が文書と決定の 2 つになるからである。
- 実効の状態は gate が導く。`proposed` の文書に D2.2 の人の承認があれば `approved`、人の却下があれば `rejected`、それ以外は文書の状態のまま。
- discussion note（議論の切り出し）は同じ schema の `decision_state: discussion` の文書で、置き場所は成果物か、子 3 の CoS thread の message・
  Knowledge inbox の候補である。discussion note は実行の入口（D4）を通らない。

#### D2.2 承認に要る人の決定

承認は既存の決定（ADR-0079 D7）で行う。文書を作った task の run が、`result.json` の `decisions` で次の決定を出す。

- `key`: `strategy-approve-v<version>`（決定の key の上限 32 文字に収まる）
- `question`: `strategy/<strategy_id>/v<version>.json`（`sha256:<64 桁 hex>`）を承認するかを問う文。sha256 は 1 回だけ書く
- `options`: `approve`（承認する）と `reject`（却下する）

gate は次のすべてを満たすときだけ承認とみなす。

1. 決定の行が文書を作った task の `decisions` にある（`idx_decisions_task` で引く）。
2. `key` と `question` の中の名前・sha256 が参照した文書と一致する。
3. 状態が `answered` で、最後の `DecisionAnswered.option` が `approve`。
4. 最後の `DecisionAnswered.by` が `human:` で始まる主体（名前が空でない）。`operator:`・`mcp:`・`cos`・`cos:` の主体と、主体の規則より前の
   `human`（接頭辞の無い古い記録）は承認に数えない。外部の LLM や operator の発言を人の承認と取り違えないためである。
   本番の `decision_answered` の `by` は `human` 375 件・`operator:claude-code` 40 件・`cos` 11 件・`human:web` 7 件・`cos:<thread>` 6 件
   （2026-11-21 に読み取り専用で数えた）。戦略の承認は、人が Web の受信箱か人の token で答える。
5. 決定が `withdrawn` でない。

#### D2.3 反証を次の版で消せない

- 版 N−1 の `stance: refute` の証拠は、版 N に同じ id・同じ内容（JSON として等しい）で残っていなければならない。消した・書き換えた版は拒否する。
  鎖のすべての隣り合う 2 版で確かめる。
- `support`・`unknown` の証拠は次の版で外してよい（理由を `change_reason` に書く）。
- 同じ仮説に `support` と `refute` の両方の証拠があるとき、その仮説の `confidence` は `confirmed` にできない（`contradicted` か `needs_decision`）。
- `confirmed` の仮説は、外部でない参照（`commit`・`artifact`・`task`）を持つ証拠を 1 件以上要する（brief R12 を強めた規則）。
- 外部の source の証拠は consent（D1.1）が要る（brief R8）。外部の文の中の指示は状態を進めない（brief R11。状態は D2.2 の決定だけが進める）。

### D3: 置き場所・承認・照合（新しい DB 表・event の種類・migration を足さない）

#### D3.1 置き場所

- 文書は task の成果物として置く。name は `strategy/<strategy_id>/v<version>.json`、記録は既存の `ArtifactProduced{run_id, artifact{name, path, sha256, kind, declared}}`。
- 版を上げるときは新しい name の file を作る。前の版の file を上書きしない（上書きは D1.4 (a) の衝突として拒否される）。
- 長く残す知見は既存の Knowledge inbox（`POST /knowledge/inbox`）に候補として出し、文書からは `knowledge` の参照で指す。

#### D3.2 承認

既存の決定の event（`DecisionRequested`・`DecisionAnswered`・`DecisionWithdrawn`）と `decisions` の表を使う（D2.2）。

#### D3.3 照合

- Project.request: 今の `Project.request` の sha256 を `task_core::project_proposal::sha256_hex` で計算し、文書の `source_revision.request_sha256` と比べる。
  Roadmap の `RoadmapVersion.request_sha256` と同じ関数・同じ値である。
- 承認済み Roadmap: store の今の承認済みの版の番号（`GET /projects/{id}/roadmap` と同じ読み出し）と `source_revision.roadmap_version` を比べる。
  承認済みの版が無い案件は `null` どうしで一致とする。
- 新しい承認済みの版: `decisions` の表で、同じ `strategy/<strategy_id>/` の、参照した版より大きい版の承認（D2.2 の 3〜5 を満たすもの）があれば
  参照した版は古い（stale）。

#### D3.4 欄と照会を既存の何で引くか

| 欄・照会 | 既存の引き口 | 測定・新設しない理由 |
|---|---|---|
| 文書の本体 | 作った task の `ArtifactProduced` と、その task の成果物の file（`files::resolve_artifact` で workspace の外を拒否して読む） | `events_for(task_id)` は `events` の `UNIQUE(task_id, seq)` の索引で 1 task 分だけ読む |
| `content_hash` | `ArtifactRef.sha256`（dispatcher が計算）。gate は file の今の byte 列の sha256 とも比べ、上書きされた file を拒否する | 文書に hash の欄を足さない |
| `supersedes`（版の鎖） | 前の版の `{task_id, name, sha256}` → その task の `ArtifactProduced` | 版は 64 まで。`events_for` を最大 64 回 |
| `project_id` | 文書の欄と、作った task の `tasks.project_id` | 一致を見る |
| `actor` | `ArtifactProduced.run_id` | `celeris_task_run` の `actor.id` と一致を見る |
| `request_sha256` | `projects.request` → `sha256_hex` | 既存の Roadmap と同じ値 |
| `roadmap_version` | Roadmap の承認済みの版（既存の roadmap の store） | 既存の読み出し |
| 承認 | `decisions` の行（`idx_decisions_task`）と `DecisionAnswered{option, by}` | 本番で `decisions` は 473 行 |
| より新しい承認済みの版の有無 | `decisions` の全行の走査（`status = 'answered'` と文の照合） | 本番で 6 ms（473 行、2026-11-21） |
| ある task がどの戦略を実行するか | `cos_operations`（`action = 'task.create'`、`target_id` = 作った task、`payload_json.strategy`） | 本番で 3,911 行、照会 9 ms（2026-11-21） |
| 参照 `task`・`artifact` | `tasks.project_id`・`ArtifactProduced` | 索引で引く |
| 参照 `chat` | chat の message と thread の `project_id`（`chat_threads` の `project_id` 列） | 既存 |
| 参照 `knowledge` | page の front matter の scope と `Project.slug` | 既存 |
| 参照 `commit` | 案件に登録されたリポジトリと 40 桁 hex の形 | 存在は reviewer |
| discussion note | 同じ schema の `discussion` の成果物、CoS thread の message、Knowledge inbox の候補 | 子 3 が使う |

既存で速く引けない照会は 1 つだけ測れた。案件のすべての戦略の版の一覧は、案件の task の `ArtifactProduced` を全部読む必要があり、本番の
案件 01M2WTS3DKNZBSZ2JMVB4CZMBW（task 1,715 件、`artifact_produced` 5,442 件、events 全体 984,449 行）で `json_extract` による走査が 1.65 秒かかった
（2026-11-21、読み取り専用）。この契約の gate はこの一覧を使わない（参照は `{task_id, name, sha256}` を明示し、新しい版の有無は `decisions` で見る）。
子 7 の画面が案件の全版の一覧を 1 秒より速く出す必要が生じたときに、索引か表を足すかを、そのときの測定で改めて決める。

### D4: 実行の接続点（CoS の `task.create` の任意の欄 `strategy`）

- `/cos/operations` の要求に、`instructed_by`・`basis` と同じ層の任意の欄 `strategy: {task_id, name, sha256}` を足す。`task.create` の操作だけが
  受け付け、他の操作に付けたら 422 `strategy_not_applicable` にする。
- `NewTaskBody`（`POST /tasks` の本文）には足さない。本文は人の REST と派生の作成（integrate task）も使い、`deny_unknown_fields` で守っている。
  そこに足すと、保存する列（migration）が要るか、黙って捨てることになる。
- 欄を持つ要求は、`instructed_by` の有無によらず D3 の検査をすべて通す（人の発言を中継するときも、古い・別の案件の・出典の欠けた戦略は使えない）。
  検査は `goal_guard::check` の後、`authority_gate` の前に置く。断った要求は既存の `audit.reject` で `rejected` の監査を残す。
- 通った要求は、`audit.payload` に `strategy` を入れる（`basis` と同じ）。`cos_operations.payload_json` がその task と戦略の対応の記録になる。
- 拒否の code（すべて 422）: `strategy_source_missing`（成果物・前の版・参照が引けない、または file が上書きされた）、`strategy_stale`
  （request の sha256・Roadmap の版が違う、または新しい承認済みの版がある）、`strategy_other_project`（文書・作った task・参照のどれかが別の案件）、
  `strategy_unapproved`（D2.2 を満たさない）、`strategy_invalid`（schema・上限・形）、`strategy_refute_removed`（D2.3）、
  `strategy_idempotency_conflict`（D1.4）。
- `strategy` の無い `task.create` は、検査も応答も監査も今と 1 byte も変えない。通常のバグ修正の task に戦略を求めない。

### D5: Project.request を worker・planner・final review・WU の実 prompt に出す（U3）

- D5.1 運ぶ欄: `RunContext` の欄は増やさない（repo health の型の欄の上限 40 に base で達している。ADR 2026-11-21-ancestor-objective-in-prompts D1）。
  `TaskLineageContext` に `project: Option<ProjectRequestContext>` を足す。`ProjectRequestContext` は `{project_id, request, request_sha256}` で、
  `request` は `Project.request` の全文（切り詰めない。長さは API の `PROJECT_REQUEST_MAX_CHARS` = 20,000 文字が上限）、`request_sha256` は
  `task_core::project_proposal::sha256_hex(request)`。wire は `context.lineage.project` で、`None` のとき出さない（`skip_serializing_if`）。
  `TaskLineageContext::is_empty` は `project` も見る。欄を足すだけで既存の欄の形を変えないので、`PROTOCOL_VERSION`（5）は上げない。
  `docs/protocol/worker-protocol.md` と schema に欄を書く。
- D5.2 組む場所: `task_dispatch::prompt_ancestors` に案件の request を読む関数を 1 つ置く。案件は対象 task の `project_id`、無ければ
  `chain_to_root` の列で最初に `project_id` を持つ祖先のもの。worker・planner・WU の run（`run_extras` → `prompt_ancestors::lineage`）、
  final review（`review_human_inputs` → `ReviewHumanInputs` → `RunContext.lineage`）、設計 review（`start_design_review`）の 3 経路が同じ関数を使う。
- D5.3 失敗の扱い: 案件を持つ task で案件の行を読めない（store の失敗・行が無い）ときは、祖先の読み取りの失敗と同じく run を起こさない。
  人の元の目的の無い prompt で走らせない。案件の無い task では `None` で、wire にも prompt にも出ない。
- D5.4 prompt: `prompt_header` の 1 か所に、祖先の節の直前（案件は root の上にあるので先頭）に見出し「案件の依頼（Project.request の全文。
  人の元の目的の正本。sha256: <64 桁 hex>）」の節を置く。`prompt_header` を全 builder（worker・WU・plan・案件の計画と replan・木と木でない
  execution planner・reviewer）と全 harness が使うので、harness ごとの分岐は足さない。
- D5.5 範囲の外: CoS の対話の run（既に `active_projects` を持つ）、`celerisctl worker run`（手動）、eval の trial は変えない
  （ADR 2026-11-21-ancestor-objective-in-prompts D1 と同じ方針）。
- D5.6 置き換えるもの: ADR 2026-11-21-ancestor-objective-in-prompts の「欠け 5 を扱わない理由」と D4（祖先の無い task の prompt は 1 byte も変えない）を、
  「案件の無い task の prompt は 1 byte も変えない」に狭める。案件に属する root の task の prompt は変わる。扱わない理由の 2 点目（この案件の
  request は 345 字の説明で依頼の原文ではない）は今も事実だが、人は欠け 5 を直すと決めた。また戦略の文書は request の sha256 を前提にするので、
  run は文書と同じ text と sha256 を見る必要がある。
- D5.7 回帰の条件: この変更を含む release の昇格後に、子 1 の prompt-audit の script（collect と render）を走らせ、`project_request` の欠けが
  0 になることを確かめる。

### D6: 試験の名前と ablation の形

- 試験の名前の prefix:
  - `strategy_contract_`: task-core の純粋関数（schema・上限・idempotency の key・状態を書けない規則・反証の保存・矛盾した仮説・外部の証拠の consent）。
  - `strategy_gate_`: task-api の `/cos/operations` の `task.create` の検査（D3・D4。store に task・成果物の file・決定・案件・Roadmap を置く）。
  - `prompt_project_request_`: task-worker の builder と task-dispatch の run の経路（D5）。
- ablation の形（4 規則）: 4 規則は R-src（出典の欠損）・R-stale（stale revision）・R-proj（別の案件の資料）・R-appr（未承認の戦略の実行）。
  1 つの基準の状態 B（承認済み・最新・同じ案件・出典のそろった文書）を作り、B から 1 か所だけ変えた負の状態 N-src・N-stale・N-proj・N-appr を作る。
  - N-src: `evidence[0].references` の `artifact` が存在しない name を指す。
  - N-stale: 文書を作った後に Project.request が変わり、`request_sha256` が今と違う（Roadmap の版の違いは別の試験）。
  - N-proj: `evidence[0].references` の `chat` が別の案件の thread の message を指す。
  - N-appr: 承認の決定の最後の回答の `by` が `operator:claude-code`（人でない）。
  - 検査は最初の違反で止まらず、違反の code の集合を返す。試験は B で集合が空、N-x で集合がちょうど `{R-x の code}` であることを確かめる
    （その規則だけが拒否することの証明）。さらに `strategy_gate_` の試験で、N-x の `task.create` が 422 とその code になり、task が作られないことを確かめる。
  - 変更前に落ちること: base の `/cos/operations` は `strategy` の欄を知らないので、N-x の要求は 422 とその code にならない。
- 追加の負の試験: 文書の `decision_state: approved`、反証の削除、同じ版の 2 つの sha256、上書きされた file、`strategy` を `task.create` 以外に付けた要求、
  `instructed_by` 付きの N-stale（中継でも拒否）。
- 回帰の試験: `strategy` の無い `task.create` の応答と監査が変わらないこと。案件の無い task の prompt が 1 byte も変わらないこと。

## WorkUnit への割り当て（計画 v1）

| WorkUnit | 書く場所 | 内容 |
|---|---|---|
| contract-core | `crates/task-core/src/strategy.rs`（と試験、`lib.rs` の `mod` 1 行）、`task_core::is_human_principal`（`is_principal_actor` の隣） | 型・上限の定数・`validate_document(doc, chain)`（違反の code の集合を返す純粋関数）・承認の決定の文の形の 1 か所の定義・`strategy_contract_` の試験と fixture。依存は serde・sha2 と `project_proposal::sha256_hex` だけで、既存の循環に新しい依存の組を足さない |
| request-prompt | `crates/task-worker/src/protocol.rs`・`claude_code/prompt.rs`、`crates/task-dispatch/src/prompt_ancestors.rs` と review・設計 review の経路、`docs/protocol/worker-protocol.md` と schema | D5 と `prompt_project_request_` の試験 |
| strategy-gate | `crates/task-api/src/cos/operations.rs`（欄と呼び出しと監査）、`crates/task-api/src/cos/` の検査の関数（新しい file 1 つ）、CoS の operations の説明 | D3・D4 と `strategy_gate_` の試験 |

`crates/task-api/src/cos/operations.rs` は高 risk の path class `authz` に入るので、strategy-gate の差分は設計 review の対象になる。

## 実装前 brief

1. 根本原因: 戦略・仮説・反証を版つきで置く契約が無く、実行の入口（CoS の `task.create`）が戦略の承認・鮮度・案件・出典を確かめない。
   Project.request を run に運ぶ欄が無い（`RunContext`・`TaskLineageContext` に案件の欄が無い）。
2. 責務 owner: 契約の検査は task-core の純粋関数（新しい module `strategy`）。入口の検査は CoS の operations（`cos/operations.rs` が呼ぶ検査の関数）。
   prompt の文脈は `prompt_ancestors`、prompt の文は `prompt_header`。
3. 既存の再利用先: `ArtifactProduced`・`files::resolve_artifact`、決定（`decisions`・`DecisionAnswered`）、`project_proposal::sha256_hex`、
   Roadmap の store、`cos_operations` の監査、`goal_guard` の並び、`prompt_ancestors::chain_to_root`、`prompt_header`。
4. 選択肢: (a) 新しい表 `strategies` と event の種類 → 依頼が禁じ、既存で引ける（D3.4）ので採らない。(b) `NewTaskBody` に `strategy` を足す → 保存に
   migration が要るので採らない。(c) 承認を文書に書く → 正本が 2 つになるので採らない。(d) `RunContext` に欄を足す → 欄の上限 40 なので採らない。
   採用: 成果物＋決定＋operation の欄＋`TaskLineageContext.project`。
5. 新しい状態・分岐・fallback・port: 新しい状態機械・表・event の種類・migration・Manager・trait・Agent サービスは無し。足すのは、
   operation の任意の欄 `strategy` 1 つ（無ければ従来の経路）と `TaskLineageContext.project` 1 つ。fallback と新旧切替の flag は無し。
6. 撤去対象: ADR 2026-11-21-ancestor-objective-in-prompts の「欠け 5 を扱わない理由」と D4 の文（D5.6 で狭める。request-prompt の WU が
   その ADR に付記を書く）。消すコード経路は無い（新しい欄と検査を足すだけ）。
7. 不変条件: 下の節。

## 守る不変条件

- 承認されていない（人の `human:` 主体の `approve` の回答が無い）、古い、別の案件の、出典の欠けた戦略を参照する `task.create` は、task を作らずに 422 になる。
- 反証の証拠は、版を進めても消えず、書き換わらない。
- `strategy` の無い `task.create` は今と同じに動く。
- 案件に属する task の worker・planner・WU・final review・設計 review の prompt に、`Project.request` の全文と sha256 がちょうど 1 回載る。案件の無い task の
  prompt は変わらない。
- Project.request と承認済み Roadmap を書き換えない（gate は読むだけ）。dispatcher と gate に LLM の呼び出しを入れない。events は追記だけ。

## 未確認

- 案件の全版の一覧を画面に出す速さ（D3.4 の 1.65 秒）が子 7 で問題になるか。
- `commit` の参照の存在（gate は形だけを見る）。reviewer が確かめる手順は子 5・6 の Critic と Evaluator で決める。
- operator が人の代わりに答える運用（この task の計画の承認・prod-release の答え）は、戦略の承認（D2.2 の 4）には使えない。人が Web で答える
  必要がある。この運用の影響は子 3・5 で確かめる。
