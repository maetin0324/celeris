---
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
---
# ADR 2026-11-23: 案件の概要の読み取り口・chat の案の人の採否・publish manifest と人だけの署名を、既存の成果物・決定・chat message の上に置く

- 日付: 2026-11-23
- 状態: Accepted
- 関係: 戦略層 v1 子 7（task 01M7XQ9R1EHA8GQ0TZVRM4FBVX）。子 0 の設計 brief（[design-brief.md](../progress/2026-11-21-strategy-v1-0/design-brief.md)）の
  §1（状態と権限の分離）・§3（observability）・§4（publish の署名）・§7（脅威 T1・T9・T10）。
  子 2 [ADR 2026-11-21-strategy-evidence-contract](2026-11-21-strategy-evidence-contract.md)（`celeris.strategy/1`・D2.2 の承認・D3.4 の 1.65 秒の走査）、
  子 3 [ADR 2026-11-21-chat-discussion-mode](2026-11-21-chat-discussion-mode.md)（mode・D3 の案の添付・D5 の編集と撤回・D6 の引用の失効）、
  子 4 [ADR 2026-11-22-external-llm-discussion-exchange](2026-11-22-external-llm-discussion-exchange.md)（D3 の MCP の propose・D4 の人の取り込み）、
  子 5 [ADR 2026-11-22-strategy-review-critic](2026-11-22-strategy-review-critic.md)（候補の `state`・`experiment`・X(v)）、
  子 6 [ADR 2026-11-22-parent-checkpoint-evaluator](2026-11-22-parent-checkpoint-evaluator.md)（`celeris.parent-evaluation-record/1`・D1.3・D7）、
  [ADR 2026-10-27-api-principals](2026-10-27-api-principals.md)（`Principal`）、
  [ADR 2026-10-09-cos-operations-all-mutations](2026-10-09-cos-operations-all-mutations.md)（書き込みの route の登録）、
  [ADR-0079](0079-recursive-task-decomposition.md) D7（人への決定）。
- この ADR は設計を固定した。source の変更は下の「WorkUnit への割り当て」の 6 つの WorkUnit が行った。設計どおりに実装していない点は
  本文のその場所に「未実装」と書き、「未確認」と末尾の「付記（close-audit、2026-11-24）」にまとめた。

## 背景

- 人は publish の責任者として、案件の目的・承認した戦略・反証・最新の知見・費用・判断待ち・危険・公開の準備度を、1 つの画面で読みたい。
  今は同じ情報が別々の場所にある: 目的は `GET /projects/{id}` の `project.request`、Roadmap は `GET /projects/{id}/roadmap`、controller の要約は
  `GET /projects/{id}/controller`、戦略の文書は task の成果物、承認は `decisions`、評価は root の成果物 `parent-evaluation/<判断 id>.json`、議論は chat の message。
  確認済みか推定か、人の判断が要るかの区別も画面に無い。
- 子 4 で、ChatGPT・Muse の案は案件 thread の message（`author = mcp:<client_id>`、`metadata_json.exchange`）として、人が選んだ要約は取り込みの message として
  Celeris に入る。子 3 で、CoS の案は返事の添付 `strategy/<id>/v<n>.json` になる。どれも人が Web で採る・却下する口が無い（子 3 D3 の末尾が子 7 に送った）。
- publish の manifest と人の署名は無い（子 0 の X6、脅威 T9）。既存の拒否 `external_publication_requires_human`（`task_api::cos::goal_guard`）は案件 CoS の
  operation に効くだけで、何を公開するかの記録と、それに対する人の署名を持たない。
- 既存の決定の回答 `POST /decisions/{id}/answer` は、HTTP（人・operator の token）・CoS の operation `decision.answer`・MCP の `decision_answer`（scope
  `tasks:decide`）の 3 経路が同じ `task_ops::decision::plan_answer` を呼ぶ。決定の key で主体を分ける規則は無いので、今は CoS と operator と MCP の client も
  どの決定にも答えられる。

## 決定

### D1: 読み取り専用の `GET /api/v1/projects/{id}/overview`

#### D1.1 形と範囲

- 応答の schema は `celeris.project-overview/1`。`{schema, project_id, generated_at, request_sha256, items[14], warnings[], truncated[]}`。
- 読むだけで、どの表にも行を書かない。読める主体は `GET /projects/{id}` と同じ（同じ認可の層を通す）。MCP の道具は足さない。
  理由: 概要は機密の分類（confidential）の文書の要約と未承認の案を含み、子 4 D2 の「人の共有だけを外へ出す」範囲と意味が違う。
- 既存の GET にある欄は写さない。概要は「既存の GET に無い導出」と「既存の行への参照（id）」だけを返し、Web は同じ画面で既存の GET の値と並べる。

| 既存の GET | 既にある欄（概要は写さない） | 概要が代わりに返すもの |
|---|---|---|
| `GET /projects/{id}` | `project.request` の本文、`tasks[]` の status・題、`root_totals`（token・定価・run の数） | `request_sha256`、task の id だけ |
| `GET /projects/{id}/roadmap`・`/roadmap/history` | 承認済み Roadmap の版・Outcome・acceptance | 戦略の文書の `source_revision.roadmap_version` との一致の判定だけ |
| `GET /projects/{id}/controller` | `outcomes[].reached`・`waiting`・`authority`・`next_wake`・`budget_limits`・`trace` | 無し（Web は controller の panel をそのまま出す） |
| `GET /projects/{id}/controller/decisions` | 判断の履歴 | 評価の判断の id だけ |
| `GET /decisions?root_id=` | 決定の本文・選択肢 | 案件の未回答の決定の id と種類（D1.3 の O12） |
| `GET /chat/threads/{t}/messages` | message の本文・添付・引用の状態 | message の id・revision・sha256 と導いた印 |

#### D1.2 印・強い警告・記録無し

- 印 `mark` は 4 語だけ: `confirmed`（確認済み）・`estimated`（推定）・`needs_decision`（要判断）・`contradicted`（矛盾・反証・失効）。子 2 の仮説の
  `confidence` と同じ語で、項目ごとの導き方は D1.3 の表で固定する。LLM の申告・外部の文の自己申告から `confirmed` を付けない。
- 記録の無い項目は `recorded: false` で、印を付けない（Web は「記録無し」と出す）。記録の無いことを `needs_decision` や `confirmed` で表さない。
- 強い警告 `strong: true` は次のどれか。
  1. 印が `contradicted` か `needs_decision`（子 0 brief §3「強い警告は needs_decision と contradicted」）。
  2. D1.3 の表の「強い警告」の列の code（`strategy_stale`・`refute_unapplied`・`evaluation_unsupported`・`evaluation_off_track`・`evaluation_invalid`・
     `citation_stale`・`budget_exceeded`・`cost_unknown`・`signoff_stale`・`manifest_invalid`・`manifest_blocking_question`・`manifest_confidential`）。
- `warnings[]` は全項目の強い警告と通常の警告を 1 列にしたもの `{code, item, strong, entry?, links[]}`。文は Web が code から組む（応答に文を持たない）。
- 1 項目の `entries` は 50 件まで。超えたら新しい順に 50 件を返し、`truncated[]` に項目の key を入れる。

#### D1.3 14 項目の正本と印の規則

受け入れ条件 0 の列挙を 14 項目に数える（「仮説と検証状況」を O4 仮説と O5 検証の状況に分ける）。`items` はこの順で、key は固定である。

| # | key | 表示 | 正本（既存） | 概要が返すもの | 印の規則 | 強い警告の code | drill-down |
|---|---|---|---|---|---|---|---|
| O1 | `original_goal` | 原目的 | `projects.request` | `request_sha256`（`task_core::project_proposal::sha256_hex`） | 常に `confirmed`（人の元の目的の正本） | 無し | task（Web は project の画面の本文） |
| O2 | `approved_strategy` | 承認戦略 | 決定 `strategy-approve-v<n>`（子 2 D2.2）と成果物 `strategy/<id>/v<n>.json` | 戦略の系列ごとに承認済みの最新の版 `{task_id, name, sha256, version, approved_by, approved_at}` と、回答待ちの版 | 承認が有効で鮮度も合う: `confirmed`。回答待ちだけ: `needs_decision`。request の sha256・Roadmap の版の不一致、より新しい承認、file の上書き: `contradicted` | `strategy_stale` | artifact・task |
| O3 | `candidates` | 候補の比較 | O2 の版の文書の `candidates[]`（子 5 D1.1） | 候補ごとの `{id, title, state, selected, cost, experiment.budget, rejected_by}` と X(v) の task の id | 承認済みの版の `selected`: `confirmed`。`open`: `estimated`。`rejected`: `contradicted`（`rejected_by` の反証を持つ）。回答待ちの版の候補: `needs_decision` | 無し（却下は反証の反映で、危険ではない） | artifact・task |
| O4 | `hypotheses` | 仮説 | 同じ文書の `hypotheses[]` | `{id, statement, confidence}` | 文書の `confidence` をそのまま使う（子 2 D2.3 で `confirmed` は内部の参照を要し、支持と反証の両方があれば `confirmed` にできない） | 無し（`contradicted`・`needs_decision` は規則 1 で強い警告） | artifact |
| O5 | `verification` | 検証の状況 | 候補の `experiment.discriminates`、X(v) の task の status、`evidence[]` | 仮説ごとに識別実験の task の id と、stance ごとの証拠の数 | 実験の task が `done` で、その task を `task`・`artifact` で引く証拠がある: `confirmed`。実験の task が走っている: `estimated`。実験の task が無い・`failed`・`cancelled`: `needs_decision` | 無し | task・notebook・log（識別実験の task ごと。D1.4） |
| O6 | `refutations` | 反証 | 文書の `stance: refute` の証拠（子 2 D2.3 で版を進めても残る）、評価の記録の `refuted[]` | `{evidence_id, hypothesis, statement, references}` | 内部の参照（`commit`・`artifact`・`task`）を持つ: `confirmed`。外部の参照だけ: `estimated`。反証された仮説が `confirmed` のまま、または `selected` の候補の仮説のまま: `contradicted` | `refute_unapplied` | artifact・git・literature |
| O7 | `latest_findings` | 最新に得た知見 | strategy root の最新の評価の記録 `parent-evaluation/<判断 id>.json`（子 6 D3.4。判断の行 `project_controller_decisions` から name を引く） | `effective.{verdict, next, axes, negative_result, unsupported}` と `measured[]`・`learned[]` | supported の項目（子 6 D4 R2）: `confirmed`。unsupported の項目: `estimated`。`status: invalid`・verdict `unknown`: `needs_decision`。verdict `off_track`: `contradicted`。3 軸（execution・epistemic・goal）は分けて返す（子 6 D7） | `evaluation_unsupported`・`evaluation_off_track`・`evaluation_invalid` | artifact・task（評価の記録を作った run の `log` は未実装。D1.4） |
| O8 | `discussion_sources` | 議論の出典 | 案件 thread の message の `metadata_json.exchange`（子 4）・`citations`（子 3 D6）、案の添付 | 出典ごとに `{message_id, revision, sha256, source_kind, source_label, author, review_state}` | 議論は検証済みの主張でないので `estimated`（子 4 D1.4 と同じ）。引用の状態が `current` でない: `contradicted` | `citation_stale` | chat・artifact |
| O9 | `experiments_and_tasks` | 現在の実験と Task | `cos_operations` の strategy 付きの `task.create`（`task_core::strategy_reads::strategy_task_creates`） | strategy root と実験の task の `{task_id, strategy name, version, candidate}`（status は D1.1 の表のとおり `GET /projects/{id}` の `tasks[]` から Web が引く） | 記録の事実なので `confirmed` | 無し | task・log・git（`git` は取り込まれた task だけ。D1.4） |
| O10 | `budget_spent` | 予算と spent | 候補の `experiment.budget`・`cost`（子 5）と、X(v) の task の subtree の消費（`root_totals` を作る既存の集計 `task_ops::tree_view::project_root_totals` と同じ関数） | 版ごとの `{usd_max, wall_hours_max}` と `{tokens, list_price_usd}`（`wall_secs` は未実装: `RollupMetrics` に壁時計の合計が無い） | token（実測）: `confirmed`。USD（定価の換算）: `estimated`。消費が予算を超えた: `contradicted`。単価の無い model の run がある: `needs_decision` | `budget_exceeded`・`cost_unknown` | task・log（X(v) の task ごと。D1.4） |
| O11 | `next_verification` | 次の検証 | X(v) にまだ task の無い `open` の候補の `experiment`、最新の評価の `next`・`uncertainty[].resolve_by`、文書の `held_assumptions[].revisit_when` | `{source, statement}` の列 | 計画なので `estimated`。評価の `next` が `ask_human`: `needs_decision` | 無し | artifact・task |
| O12 | `pending_human_decisions` | 人の判断待ち | 案件の task の `decisions`（`status = 'open'`）、採否の無い chat の案（D2.4） | 決定の `{decision_id, task_id, key の種類}`（`strategy-approve`・`publish-signoff`・その他）と案の message の id | 1 件以上なら `needs_decision` | 無し（規則 1 で強い警告） | task・chat |
| O13 | `risks` | 危険 | O2〜O12・O14 の警告と、controller の phase が `blocked`（`GET /projects/{id}/controller` の行を参照する） | `warnings[]` の要約（code ごとの件数） | 強い警告が 1 件以上: `contradicted`。通常の警告だけ: `estimated`。警告が 0 件で記録がある: `confirmed` | 各項目の code | 各項目の link |
| O14 | `publish_readiness` | 公開の準備度 | 決定 `publish-signoff-v<n>` と成果物 `publish/<manifest_id>/v<n>.json`（D3） | manifest ごとに最新の版の `{task_id, name, sha256, version, signoff, violations[]}` | 最新の版が人の署名済みで file が同じ: `confirmed`。検査を通り署名待ち: `needs_decision`。検査の違反・署名後の file の変化・より新しい版: `contradicted` | `manifest_invalid`・`signoff_stale`・`manifest_blocking_question`・`manifest_confidential` | artifact・task |

- O7 の goal の軸: 子 6 の評価器は `goal_reached` を `false` に固定して入力している（子 6 の「未確認」）。概要は goal の軸を評価の値のまま出し、
  案件の Outcome の到達は controller の `outcomes[].reached` を Web が並べて出す。評価の goal の軸から到達を導かない。
- O2 の stale の判定は子 2 D3.3 の照合の関数を使い、概要で同じ照合を書き直さない（規約 2）。O8 の引用の状態は子 3 D6 の `task_core::chat_citation::citation_state`、
  O3・O5 の X(v) は子 5 D2.3 の集合の関数を使う。

#### D1.4 drill-down の link の種類

link は `{kind, …}` の 7 種だけである。Web は kind ごとに既存の画面へ移る。外部の URL は Celeris が取りに行かない。

| kind | 欄 | Web の移り先（既存） | 備考 |
|---|---|---|---|
| `task` | `task_id` | `/tasks/{task_id}` | 案件の task だけ（他の案件の task は概要に出さない） |
| `artifact` | `task_id`・`name`・`sha256` | `/tasks/{task_id}?tab=artifacts` と成果物の viewer | 未実装: 概要の panel は link の sha256 と今の file を照合せず、「上書きされた」を出さない（成果物の viewer の `sha256_matches` で読める）。戦略の文書の link の `sha256` は空文字（承認済みの版は O2 の link が sha256 を持つ） |
| `notebook` | `task_id`・`name` | `artifact` と同じ viewer と `/tasks/{task_id}/files` | O5 の識別実験の task（X(v)）の `ArtifactProduced` のうち name が `.ipynb` で終わるもの（同じ name は 1 件。link は既存の `artifact_link` が作る）。X(v) でない task の成果物は出さない。Celeris に Notebook の正本は無く、成果物の file を指すだけ |
| `log` | `task_id`・`run_id` | `/tasks/{task_id}/runs/{run_id}` | X(v) の実験の task（O5・O9・O10）ごとに、`runs` の索引の `started_at` の順で最後の run（`SqliteStore::runs_for_task`。worker・reviewer・planner の役割を問わない）。run の無い task には出さない。未実装: O7 の評価の記録を作った run の `log` は出さない |
| `git` | `repo`・`sha`・`task_id?` | `task_id` があれば `/tasks/{task_id}/changes`、無ければ repo 名と 40 桁の sha を写せる文字で出す | O9 は実験の task の最後の `IntegrationMerged`（delivery と sweep が取り込みを書く event。A9 D1 が取り込み済みと数える event と同じ）から、repo（task の `repos` の名前）・head の sha・`task_id` を出す。取り込みの記録の無い task には出さない。子 2 の `commit` の参照（O6・O7）は、repo と sha が実験の task の取り込みと一致すれば `task_id` を付ける。X(v) でない task の取り込みとは照合しない（概要は案件の全 task の event を読まない。D1.5）ので、その参照は `task_id` を持たず、Web は repo 名と sha を文字で出す。commit の存在は reviewer が確かめる（gate は形だけ） |
| `literature` | `url` か `path`・`sha256` | `url` は新しい tab（`rel="noopener noreferrer"`）。`path` は写せる文字で出し、`/knowledge` の検索の画面へ移す | `url` は `https://` だけ（子 2 の参照の規則）。外部の資料は「外部・untrusted」と出す。knowledge の page へ path で直接移る口は未実装（「未確認」） |
| `chat` | `thread_id`・`message_id`・`revision`・`sha256` | `/?thread={thread_id}`（概要と manifest の画面の link は `message` を付けない。chat の route の `/?thread=&message=` の強調は D4 で実装したが、概要の link からは使っていない） | 引用の状態（current・edited・retracted・missing）は O8 の引用の link だけが添える。案の message と戦略の文書の chat の参照の link は状態を持たない |

#### D1.5 戦略の版の一覧の速さ

- 子 2 D3.4 は、案件の全 task の `ArtifactProduced` を走査して戦略の版を集めると、本番の案件 01M2WTS3DKNZBSZ2JMVB4CZMBW で 1.65 秒かかったと測った。
  概要はこの走査をしない。
- 版の一覧は `decisions` から引く: 案件の task（`tasks.project_id`）の決定で key が `strategy-approve-v` で始まるもの。決定の question に書かれた name と sha256 から
  成果物を `events_for(task_id)` で 1 task ずつ引く。`decisions` の全行の走査は本番で 6 ms（473 行、子 2 D3.4、2026-11-21）。manifest の一覧（O14）も同じく
  key が `publish-signoff-v` で始まる決定から引く。
- したがって、決定を出す前の下書きの版（承認を求めていない版）は概要に出ない。下書きは task の成果物の画面で読める。この扱いで足りないと分かったら、
  索引か表を足すかをそのときの測定で決める（子 2 D3.4 と同じ条件）。
- 評価の記録（O7）は `project_controller_decisions` の `evaluate` の判断の行から name を作り、root の `events_for` を 1 回読む。議論の出典（O8）は案件 thread の
  message を seq の新しい順に 50 件まで読む。概要全体で読む file は、戦略の系列ごとの最新の版 1 件・評価の記録 1 件・manifest ごとの最新の版 1 件だけである。
- drill-down（D1.4）のために、X(v) の task ごとに `runs_for_task` と `events_for` を 1 回ずつ読む（`events_for` は task ごとに 1 回だけ読む cache を
  O2・O7 と共有する）。file は読まない。

### D2: chat の案を人が採る・却下する・コメントする口

#### D2.1 route と主体

- `POST /api/v1/chat/threads/{t}/messages/{m}/review`、本文 `{verdict, expected_revision, sha256, attachment?, note?, client_message_id?}`（`deny_unknown_fields`）。
  - `verdict` は `adopt`・`reject`・`comment` の 3 語。
  - `expected_revision` と `sha256` は人が画面で見た案の版。`attachment` は CoS の案の添付の name（`strategy/<id>/v<n>.json`）で、そのときの `sha256` は添付の
    sha256。`note` は 2,000 文字まで（`comment` では 1 文字以上が必須）。
- 呼べるのは `Principal::Human` だけ。`Principal::Operator`（relay を含む）・`Principal::Mcp`・`Principal::Cos`・`Principal::Unauthenticated` は
  403、problem の code `proposal_verdict_requires_human`。判定は roadmap の decide と同じ形（`requires_human`）で handler の先頭に置く。
  - 実装（WU proposal-api）: MCP の token と CoS の run は handler より前の層で止まる。MCP の token は middleware の「MCP の token は読むだけ」の guard で
    403 `forbidden`。CoS の run の直接の呼び出しは 422 `cos_audit_context_required`、`/cos/operations` 経由は 422 `cos_operation_not_allowed`。
    handler の 403 `proposal_verdict_requires_human` は operator（relay を含む）と token の無い構成に返り、前の層が変わっても MCP・CoS を止める
    （handler の unit 試験で 4 主体を確かめる）。worker の run は API の主体を持たず DB を読むだけなので、採否の message を書けない。採否の状態の導出（D2.4）も
    author が `human:` の採否の message だけを数える。
- CoS の operation の allowlist に足さない（`crates/task-api/src/cos/ops/surface.rs` の `EXCLUDED` と
  [ADR 2026-10-09-cos-operations-all-mutations](2026-10-09-cos-operations-all-mutations.md) の表に登録する）。MCP の道具も足さない。

#### D2.2 対象の案

次の 3 種の message だけを対象にする。それ以外は 422 `proposal_review_not_a_proposal`。

| 種類 | 見分け方 | 案の版（照合する revision・sha256） |
|---|---|---|
| 外部 LLM の案（子 4 D3） | `role='user'`・`author` が `mcp:` で始まる・`metadata_json.exchange` がある | message の revision と本文の sha256（`task_core::chat_citation::body_sha256`。子 3 D6 の引用と同じ値） |
| 人が取り込んだ要約・引用（子 4 D4） | `metadata_json.exchange` があり `source.kind` が `human_paste` か `markdown_import` | 同上 |
| CoS の案の添付（子 3 D3） | `role='assistant'` で添付の name が `strategy/<id>/v<n>.json` | message の revision と添付の sha256 |

- 統括 CoS の §A4 の `cos_proposal` は対象にしない（controller の入力の経路で、意味が違う。規約 8）。
- 照合: `expected_revision` が今の revision と違う、`sha256` が今の本文・添付と違う: 409 `proposal_review_stale`。撤回された message: 409 `proposal_review_stale`。

#### D2.3 記録は既存の chat message

- 採否は同じ thread の新しい message 1 件として、既存の人の送信の tx（`task_core::chat::human_post` の `chat_message_post_as_also`）で書く。`role='user'`、`author` は呼んだ人の `caller.actor()`。
  `metadata_json.proposal_review = {verdict, target: {thread_id, message_id, revision, sha256}, attachment?}`。新しい表・列・migration・`chat_events` の type は足さない。
- 本文は Rust の固定の文で組み、案の本文を写さない（外部の文の中の指示を人の指示の本文に入れない）。`note` は人の文としてその後に付ける。
- mode（子 3 D1）は verdict で決める。
  - `adopt`: mode を書かない（execute）。本文は「案 <message_id>（rev <n>、sha256:<hex>）を採る。task の成果物 `strategy/<strategy_id>/v<n>.json` にし、決定
    `strategy-approve-v<n>` で人の承認を求めよ」の形。これは承認ではない。CoS は execute の run で、子 2 の経路（task を起票し、その run が成果物を作り、決定に人が答える）
    を進める。`task.create` は `instructed_by` でこの message を引けるが、goal_guard・strategy_gate・authority_gate は今のまま掛かる。
  - `reject`・`comment`: `discussion_mode: "discuss"`。CoS の run は議論だけで、子 3 D2 によりすべての operation が拒否される。
- idempotency は既存の `client_message_id`（`idx_chat_messages_client`）。同じ key で同じ本文は既存の message を返し、違えば 409（既存の規則）。

#### D2.4 採否の状態は導く（保存しない）

- 案の message ごとの状態は、読むたびに同じ thread の `proposal_review` の message から導く: その案を target に持つ最後の採否の message の verdict
  （`adopted`・`rejected`、`comment` だけなら `commented`、無ければ `pending`）。採否の target の revision・sha256 が今の案と違えば `stale`（人が見た版の後に
  案が編集・撤回された）。導出は子 3 D6 の引用の状態の関数 `task_core::chat_citation::citation_state` を使い、照合を書き直さない。
- 状態の列・表・状態機械は足さない。採否の正本は人の message だけである（規約 4）。
- wire の `ChatMessage` に 2 つの任意の欄を足す。
  - 案の message に `proposal: {kind, review_state, review_message_id?, reviewed_by?, reviewed_at?}`。
  - 外部の文の印 `external: {source_kind, source_label, untrusted: true}`。`metadata_json.exchange` の `source.kind` が `mcp_client`・`human_paste`・`markdown_import`
    のときに出す。`untrusted` は常に `true` で、Web は「外部の文（未検証）」と出す。
- 案の本文が「採用する」「approve」などの語を含んでも状態は変わらない（状態は人の採否の message だけから導く。脅威 T1・T2）。

### D3: publish manifest `celeris.publish-manifest/1` と人だけの署名

#### D3.1 置き場所

- manifest は task の成果物 `publish/<manifest_id>/v<n>.json`。記録は既存の `ArtifactProduced`（sha256 は dispatcher が計算した値。自己申告ではない）。
  1 つの task が持つ `manifest_id` は 1 つだけ（決定の key が版の番号だけを持つため。2 つ目は `publish_manifest_id_conflict`）。
- 版を上げるときは新しい name の file を作る。同じ name で違う sha256 は衝突（`publish_manifest_idempotency_conflict`。子 2 D1.4 と同じ規則）。

#### D3.2 文書の欄（claims → evidence → 再現条件 → limitations・未解決 → human signoff）

| 欄 | 型・値 | 規則 |
|---|---|---|
| `schema` | `"celeris.publish-manifest/1"` | 固定 |
| `project_id`・`manifest_id`・`version`・`supersedes`・`change_reason`・`idempotency_key` | 子 2 D1.1 と同じ形。`manifest_id` は `[a-z0-9-]{1,40}`、`version` は 1〜64、`idempotency_key` は `publish:<manifest_id>:v<version>` | 版の鎖は子 2 と同じ |
| `source`・`actor`・`source_revision`・`timestamp` | 子 2 D1.1 と同じ（`source.kind = celeris_task_run`、`actor.id` は run の id） | 作った run と一致 |
| `target` | `{kind, venue}`。`kind` は `paper`・`release_note`・`dataset`・`other`、`venue` は 200 文字まで | 公開する先の説明。Celeris はここへ送らない |
| `acl` | `{classification, outbound}`（子 2 と同じ） | `confidential` は概要で強い警告 `manifest_confidential` |
| `claims[]` | 1〜32 件 `{id, statement, confidence, evidence[]}` | `evidence` は証拠の id を 1 件以上。`confidence` は 4 語 |
| `evidence[]` | 1〜256 件 `{id, stance, statement, references[]}` | `references` は子 2 の `task_core::strategy::Reference`（6 種）をそのまま使い、同じ解決の関数で案件を照合する |
| `reproduction[]` | 0〜16 件 `{id, claims[], commands[], environment{repo, commit, host_class}, data[], expected}` | `commands` は 16 件・各 2,000 文字まで。`commit` は 40 桁の小文字 hex |
| `limitations[]` | 0〜32 件 `{id, statement, claims[]}` | |
| `open_questions[]` | 0〜32 件 `{id, statement, blocking}` | `blocking: true` は概要で強い警告 `manifest_blocking_question` |
| `decision_state` | `draft`・`proposed`・`withdrawn` | `signed` などの署名の状態は書けない（D3.4） |

検査の純関数 `task_core::publish::validate_manifest(doc, chain)` は、最初の違反で止まらず違反の code の集合を返す（子 2 D6 と同じ形）。

- M1 形と上限（上の表、子 2 D1.3 の文字数の上限と同じ値）: `publish_manifest_invalid`。
- M2 claim の証拠が 1 件以上で、すべて同じ文書にある。
- M3 `confirmed` の claim は、`stance: support` で内部の参照（`commit`・`artifact`・`task`）を持つ証拠を 1 件以上要する（子 2 D2.3 と同じ規則）。
- M4 `support` と `refute` の両方の証拠を持つ claim は `confirmed` にできない。
- M5 `confirmed` の claim は `reproduction[]` のどれかの `claims` に入る（再現条件の無い確認済みの主張を書かせない）。
- M6 `confirmed` でない claim と、`refute` の証拠を持つ claim は、`limitations[]` か `open_questions[]` のどれかで id を引かれる（未確認を隠さない）。
- M7 前の版の `refute` の証拠は次の版で消せず、書き換えられない（子 2 D2.3 と同じ）: `publish_manifest_refute_removed`。
- M8 秘密に見える文字列（子 2 の `task_core::strategy::contains_secret`）: `publish_manifest_secret`。
- M9 `project_id` と参照が別の案件: `publish_manifest_other_project`。
- M10 文書に署名の状態（`signoff`・`signed`・`approved` の欄、`decision_state` の他の語）を書いた: `publish_manifest_invalid`。

#### D3.3 署名は sha256 に対する人だけの回答

- 署名は既存の決定（ADR-0079 D7）で行う。manifest を作った task の run が `result.json` の `decisions` で次の決定を出す。
  未実装: run に manifest と署名の決定を出させる prompt・計画は作っていない。試験は成果物と決定の行を直接作って確かめた（「未確認」）。
  run が `task_core::publish::signoff_question` と違う文で出した決定は、署名に数えない（`signoff_state` が `unsigned`）。
  - `key`: `publish-signoff-v<version>`（決定の key の上限 32 文字に収まる。`task_core::publish::signoff_decision_key` の 1 か所で組む）。
  - `question`: `publish/<manifest_id>/v<version>.json`（`sha256:<64 桁 hex>`）に署名するかを問う文。sha256 は 1 回だけ書く。
  - `options`: `sign`（署名する）と `reject`（署名しない）。
- 署名とみなす条件（`task_core::publish::signoff_state` の純関数。子 2 D2.2 と同じ形）:
  1. 決定の行が manifest を作った task の `decisions` にある。
  2. `key` と `question` の中の name・sha256 が manifest の `ArtifactProduced` と一致する。
  3. 状態が `answered` で、最後の `DecisionAnswered.option` が `sign`。
  4. 最後の `DecisionAnswered.by` が署名できる主体（`task_core::publish::is_signoff_actor`: `task_core::is_human_principal` が真で、`human:local` でない）。
     `human:local` は token の無い構成の `Principal::Unauthenticated` の actor で、人の token・Web の login を経ていないので数えない。接頭辞の無い古い `human` も数えない。
  5. 決定が `withdrawn` でない。
- 拒否する主体: 決定の key が `publish-signoff-v` で始まる決定への回答と回答の改め（`decision.revise`）は、`by` が `is_signoff_actor` でなければ拒否する。
  置く所は 3 経路（HTTP・CoS の operation・MCP）が共有する owner の `task_ops::decision::plan_answer` と `plan_revise` で、決定の行も event も書かない。
  - HTTP: 403、problem の code `publication_requires_human`。
  - CoS の operation `decision.answer`・`decision.revise`: 同じ 403 と code で、既存の `OperationAudit::reject` が `cos_operations` に `rejected` の行を残す。
    `instructed_by` で人の発言を引いても外れない（署名は人が自分で行う）。
  - MCP の `decision_answer`: 既存の権限の拒否の error（JSON-RPC）で、message に `publication_requires_human` を含める。
- 署名の前の内容の検査: 人の HTTP の回答で `option = sign` のとき、task-api の decisions の handler が `task_api::publish::manifest_status` で manifest を
  検査し、違反があれば 422 `publish_manifest_invalid`、file の今の byte 列の sha256 が question と違えば 409 `publish_manifest_stale` にする。
  この検査を HTTP の handler に置くのは、署名できる経路が人の HTTP だけだからである（CoS と MCP は上の主体の拒否で先に止まる）。
  回答の改め（`POST /decisions/{id}/revise`）で `sign` に変えるときも同じ検査を通す。token の無い構成の要求は管理系の入口（`require_admin`）で 401 になり、
  共有の owner も `human:local` を拒む。manifest の GET は `?version=` で版を選べる（省けば最新の版）。
  概要（O14）と manifest の画面は同じ `manifest_status` を使う。

#### D3.4 失効と状態（保存しない）

- 署名の状態は文書に書かず、読むたびに決定と file から導く（子 2 D2.1 と同じ理由: 正本を 2 つにしない）。値は `unsigned`・`signed`・`rejected`・`stale`・`superseded`。
  - `stale`: 署名した後に file の byte 列が変わった（sha256 が question と違う）。
  - `superseded`: 同じ `manifest_id` のより新しい版の決定がある。署名は版に付くので、新しい版には署名し直しが要る。
- 本文が変われば署名は失効する。新しい版の manifest は新しい決定（`publish-signoff-v<n+1>`）で署名し直す。

#### D3.5 Celeris は外へ何も送らない

- 署名は記録だけである。Celeris は署名を受けて外部へ送信・公開・通知（Discord の送信を含む）をしない。署名を起点にする task の起票・controller の行動も無い。
- 実際の公開（論文の投稿・データの公開など）は、署名した後に人が Celeris の外で行う。会話・LLM・CoS・controller からの自動の publish は作らない。
- 既存の `external_publication_requires_human`（案件 CoS の operation の拒否）は変えずに残す。

### D4: Web の画面（既存の画面の延長）

| 画面 | 既存の file | 足すもの |
|---|---|---|
| 案件の詳細 | `web/features/projects/project-detail-view.tsx` | 概要の panel（14 項目・印・強い警告・link）。既存の controller・roadmap・authority の panel は並べたまま |
| chat | `web/features/chat/messages/message-item.tsx`・`parts.tsx` | 案の message に採用・却下・コメントの操作と状態、外部の文の印。`/?thread=&message=` で message へ移る |
| manifest | 新しい route `/tasks/$id/publish/$manifestId` | claims → evidence → 再現条件 → limitations・未解決 → 人の署名 の順に出す。署名の操作は人の login のときだけ出し、`sign` は既存の決定の回答を呼ぶ |

- 画面は daemon の API を通して読み、状態を自分で持たない（既存の方針）。強い警告は色だけでなく文字で出す（a11y）。
- 実装（WU web-publish）: 「人の login」は gateway の session（`GET /api/session` の `authenticated`）で判定する。Web は BFF の token の daemon の主体
  （`human:web` か、token の無い構成の `human:local`）を読めないので、`human:local` の拒否は daemon だけが行う。gateway（`web/server/relay.js`）は daemon の
  401・403 を 502 `daemon_auth` に置き換えるので、本物の gateway を通すと画面は `publication_requires_human`・`proposal_verdict_requires_human` の理由を出さず
  `daemon_auth` の文を出す（「未確認」）。gateway の SPA の path（`web/server/spa-routes.js`）に manifest の route を足した。

### D5: 足さないもの

- 新しい DB 表・列・migration・event の種類・`chat_events` の type は足さない。置き場所は、概要は読むだけ、採否は `chat_messages.metadata_json`、manifest は
  `ArtifactProduced`、署名は `decisions` と `DecisionAnswered`。理由: どれも既存の行で正本を持て、照会も既存の索引で速く引ける（D1.5）。
- 新しい Manager・trait・状態機械・Agent サービスは足さない。採否の状態・署名の状態・印は、既存の行から読むたびに導く純関数で、遷移の表を持たない。
- Project.request と承認済み Roadmap は書かない（概要は読むだけ。採否は chat の message を足すだけ）。dispatcher に LLM の呼び出しを入れない。

## 保証の表

| 守る不変条件 | 試験（正式名。close-audit の 74 件と WU drilldown-api が足した 4 件の計 78 件すべてが crates に `fn` として実在することを確かめた） | 変更前に落ちるか |
|---|---|---|
| 非人の主体は publish manifest に署名できない（operator・CoS・MCP・token の無い構成） | `publish_signoff_operator_answer_is_403_and_records_nothing`・`publish_signoff_cos_operation_decision_answer_is_403_and_audited`・`publish_signoff_mcp_decision_answer_is_refused`（celeris-mcp）・`publish_signoff_unauthenticated_local_is_refused`（HTTP は管理系の入口で 401、共有の owner も `human:local` を拒む） | 落ちる（base はどの主体の回答も受ける） |
| 人の発言の引用でも CoS は署名できない | `publish_signoff_cos_instructed_by_human_message_cannot_sign` | 落ちる |
| 回答の改めでも非人は署名できない | `publish_signoff_revise_by_non_human_is_403` | 落ちる |
| worker の run（API の主体を持たない。run の id・`worker:` を名乗る回答）も署名できない | `publish_signoff_worker_run_cannot_sign` | 落ちる |
| 外部 LLM・会話の文（「承認する」「publish して」）は署名にならない | `publish_signoff_external_llm_text_cannot_sign` | 落ちる |
| 別の案件の参照を持つ manifest には署名できない | `publish_signoff_other_project_reference_is_422` | 落ちる |
| 署名の状態は sha256・版・最後の回答から導く | `publish_signoff_state_follows_sha_version_and_answer` | 落ちる（関数が無い） |
| 人の HTTP の回答だけが今の sha256 に署名する | `publish_signoff_human_answer_signs_current_sha` | 落ちる（署名の状態の導出が無い） |
| 本文が変われば署名は失効し、新しい版は署名し直す | `publish_signoff_after_file_change_is_stale`・`publish_signoff_newer_version_marks_previous_signoff_superseded` | 落ちる |
| 検査に通らない manifest には署名できない | `publish_signoff_invalid_manifest_is_422` | 落ちる |
| 接頭辞の無い古い主体は署名に数えない | `publish_signoff_legacy_unprefixed_human_is_not_signoff` | 落ちる |
| 署名は記録だけで、外へ送らず起票もしない | `publish_signoff_records_only_decision_events` | 落ちる（署名の口が無い） |
| 他の決定の回答は今と同じ | `publish_signoff_other_decision_keys_unchanged` | 通る（回帰） |
| manifest は claims → evidence → 再現条件 → limitations・未解決 → 署名 の順で読める | `publish_manifest_view_orders_claims_evidence_reproduction_limitations_signoff` | 落ちる |
| 基準の manifest は違反が無い | `publish_manifest_valid_baseline_has_no_violations` | 落ちる（検査の関数が無い） |
| 再現条件の command の長さと commit の形、文書の大きさの上限 | `publish_manifest_limits_reject_long_commands_and_bad_commit` | 落ちる（検査の関数が無い） |
| 証拠の無い claim・内部の参照の無い確認済みの claim・再現条件の無い確認済みの claim を拒む | `publish_manifest_claim_without_evidence_is_invalid`・`publish_manifest_confirmed_claim_needs_internal_reference`・`publish_manifest_confirmed_claim_needs_reproduction` | 落ちる |
| 支持と反証の両方を持つ claim は確認済みにできず、未確認の claim は limitations か未解決に載る | `publish_manifest_supported_and_refuted_claim_cannot_be_confirmed`・`publish_manifest_unconfirmed_claim_must_be_listed_in_limitations` | 落ちる |
| 反証は版を進めても消えない | `publish_manifest_refute_evidence_cannot_be_removed_in_next_version` | 落ちる |
| 秘密・署名の自己申告・別の案件の参照・同じ版の 2 つの sha256・2 つ目の manifest の id を拒む | `publish_manifest_secret_in_body_is_invalid`・`publish_manifest_signed_state_in_document_is_invalid`・`publish_manifest_other_project_reference_is_invalid`・`publish_manifest_same_version_other_sha_is_conflict`・`publish_manifest_second_id_in_task_is_conflict` | 落ちる |
| 上書きされた file を検査済みと出さない | `publish_manifest_view_reports_overwritten_file_as_stale` | 落ちる |
| 案の採否は人だけ | `proposal_review_operator_is_403_proposal_verdict_requires_human`・`proposal_review_mcp_and_cos_principals_are_refused`・`proposal_review_unauthenticated_local_is_403`・`proposal_review_non_human_principals_are_403_at_handler`（task-api の handler の unit） | 落ちる（base に route が無い。`proposal_review_mcp_and_cos_principals_are_refused` の MCP の 403 と CoS の 422 は base の guard でも同じで、base では wire の `proposal` が無いことで落ちる） |
| 採否の route は CoS の operation に無い | `proposal_review_route_not_in_cos_operations` | 落ちる |
| 採用は execute の人の指示の message で、承認ではない | `proposal_review_human_adopt_posts_execute_instruction_message`・`proposal_review_adopt_is_not_strategy_approval`・`proposal_review_text_is_fixed_and_adopt_is_not_approval`（task-core の unit） | 落ちる |
| 却下・コメントは discuss の message で副作用が無い | `proposal_review_reject_and_comment_post_discuss_messages` | 落ちる |
| 人が見た版と違う案・撤回された案・案でない message は採否できない | `proposal_review_stale_revision_is_409`・`proposal_review_sha_mismatch_is_409`・`proposal_review_retracted_target_is_409`・`proposal_review_non_proposal_target_is_422` | 落ちる |
| 別の案件の thread の案は、この thread の path から採否できない（404） | `proposal_review_other_project_thread_is_refused` | 落ちる |
| CoS の案の添付は添付の sha256 で照合し、添付に無い path と本文の sha256 を拒む | `proposal_review_cos_strategy_draft_reviewed_by_attachment_sha` | 落ちる |
| 採否の状態は保存せず導き、案の編集・撤回で失効する | `proposal_review_state_is_derived_not_stored`・`proposal_review_state_stale_after_target_edit`・`proposal_review_state_stale_after_target_retract`・`proposal_review_state_follows_last_verdict_of_a_person`（task-core の unit） | 落ちる |
| 案の種類と外部の印は記録（role・author・`metadata_json.exchange`）から導き、本文から導かない | `proposal_review_kind_and_mark_come_from_the_record`（task-core の unit） | 落ちる（関数が無い） |
| 外部の文は wire で untrusted と印が付き、文の中の指示で状態が変わらない | `proposal_review_external_text_marked_untrusted_on_wire`・`proposal_review_injected_text_cannot_adopt` | 落ちる |
| 同じ client_message_id の再送は二重にならない | `proposal_review_idempotent_client_message_id` | 落ちる |
| 概要は 14 項目を決めた順で返し、既存の GET の欄を写さない | `project_overview_lists_fourteen_items_in_order`・`project_overview_does_not_copy_existing_get_fields` | 落ちる（base に route が無い） |
| 概要は読むだけ | `project_overview_is_read_only` | 落ちる |
| 原目的の sha256 は Project.request と一致する | `project_overview_request_sha_matches_project_request` | 落ちる |
| 戦略の版は decisions から引き、成果物の全走査をしない | `project_overview_strategy_versions_from_decisions_not_artifact_scan` | 落ちる |
| 人でない回答・回答待ちの戦略を承認済みと出さない | `project_overview_unapproved_strategy_is_not_marked_approved`・`project_overview_operator_answered_strategy_is_not_approved` | 落ちる |
| 古い戦略・矛盾した仮説・未反映の反証は強い警告（失効した引用の `citation_stale` は実装したが試験は無い。下の「未確認」） | `project_overview_stale_strategy_is_contradicted_with_strong_warning`・`project_overview_contradicted_hypothesis_is_strong_warning`・`project_overview_unapplied_refutation_is_strong_warning` | 落ちる |
| 根拠の無い評価の主張は推定で、3 軸を分けて出す | `project_overview_unsupported_evaluation_claim_is_estimated_with_warning`・`project_overview_evaluation_axes_shown_separately` | 落ちる |
| 外部の出典は推定 | `project_overview_external_source_is_estimated` | 落ちる |
| 予算の超過と単価の不明は強い警告（`budget_exceeded`・`cost_unknown` は実装したが試験は無い。下の「未確認」） | 無し | — |
| 判断待ちは要判断、公開の準備度は署名の決定から導き、未署名・失効した manifest は強い警告 | `project_overview_open_decisions_are_needs_decision`・`project_overview_publish_readiness_from_signoff_decisions`・`project_overview_unsigned_or_stale_manifest_is_strong_warning` | 落ちる |
| link の URL は https だけ（7 種の型は wire の enum `ProjectOverviewLink` で固定） | `project_overview_url_link_is_https_only` | 落ちる |
| 実験の task の最後の run を `log` にする（O5・O9・O10。run の無い task には出さない。挿入の順でなく `started_at` の順） | `project_overview_drilldown_log_link_points_to_latest_run_of_experiment_task` | 落ちる（概要が `log` を出さない） |
| 実験の task の `.ipynb` の成果物を O5 の `notebook` にする（X(v) でない task の成果物と `.ipynb` でない成果物は出さない） | `project_overview_drilldown_notebook_link_from_experiment_ipynb_artifact` | 落ちる（O5 が task の link だけ） |
| 取り込まれた実験の task の O9 の `git` は `task_id` を持ち、戦略の文書の `commit` の参照も取り込みの repo と sha に一致すれば `task_id` を持つ（一致しない参照は持たない） | `project_overview_drilldown_git_link_carries_task_id_from_integration` | 落ちる（`git` がいつも `task_id` 無しで、O9 に `git` が無い） |
| 本物の route の drill-down の形を fixture `crates/task-api/tests/fixtures/project_overview_drilldown.json` に固定する（`log`・`notebook`・`task_id` 付きの `git` を含む。id は初出順の記号、`generated_at`・`approved_at` は固定。`UPDATE_FIXTURE=1` で書き直す） | `project_overview_drilldown_fixture_matches_generated` | 落ちる（fixture が無く、link も出ない） |
| 別の案件の記録を出さず、記録の無い項目に印を付けない | `project_overview_other_project_data_excluded`・`project_overview_empty_project_items_absent` | 落ちる |
| 人が概要で 14 項目・印・強い警告を読み、各 link で移れる（drill-down の試験は、偽 daemon が上の golden をそのまま返す構成で golden の link を押す。WU drilldown-e2e） | `web/e2e/work/project-overview.spec.ts` | 落ちる（画面が無い） |
| 人が chat の案を採用・却下・コメントでき、外部の文に印が付く | `web/e2e/chat/proposal-review.spec.ts` | 落ちる |
| 人が manifest を順に読み、人の login のときだけ署名の操作が出る | `web/e2e/work/publish-manifest.spec.ts` | 落ちる |

## 実装前 brief

1. 根本原因: 案件の重要な情報（戦略・反証・評価・議論・判断待ち・公開）の正本が別々の行と file にあり、確認済み・推定・要判断を区別して 1 か所に導く読み取りの口が無い。
   chat の案に人が採否を記録する口が無い。publish の manifest と、決定の key で主体を分ける規則が無く、CoS・operator・MCP も人の決定に答えられる。
2. 責務 owner: 概要の導出は `task_ops::project_overview`（新しい module。`task_ops::project_controller_view` と同じ層）と handler `task_api::handlers::project_overview`。
   manifest の検査・署名の状態・署名できる主体は `task_core::publish`（純関数）。主体の拒否は決定の回答の共有の owner `task_ops::decision::plan_answer`・`plan_revise`。
   署名の前の内容の検査と manifest の GET は `task_api::publish`。採否の導出は `task_core::proposal_review`（chat の型を一方向に使う。`chat` は使わない）、
   route は `task_api::chat`。
3. 既存の再利用先: `ArtifactProduced`・`files::resolve_artifact`、`decisions`・`DecisionAnswered`、`task_core::is_human_principal`、`task_core::strategy` の `Reference`・
   参照の解決・`contains_secret`・承認の導出、`task_core::strategy_reads::strategy_task_creates`、子 5 の X(v)、子 6 の評価の記録と `checkpoint_states`、
   子 3 の `task_core::chat_citation` の `citation_state`・`body_sha256`、`task_core::chat::human_post`、`client_message_id`、`task_ops::tree_view::project_root_totals`、`OperationAudit::reject`、roadmap の `requires_human` の形。
4. 選択肢: (a) 概要用の表か materialized view を足す → 依頼が禁じ、D1.5 の照会で足りるので採らない。(b) 概要に既存の GET の欄を写す → 正本が 2 つの応答に分かれ、
   不一致が起きるので採らない。(c) 採否の表か `chat_events` の新しい type → message の metadata で足り、人の発言として履歴・検索・SSE に乗るので message を採る。
   (d) 署名の専用の route と表 → 既存の決定で承認と同じ形にでき、主体の規則を 1 か所に置けるので決定を採る。(e) 主体の拒否を HTTP の handler だけに置く → CoS と MCP の経路が
   素通りするので、3 経路の共有の owner に置く。
5. 新しい状態・分岐・fallback・port: 状態は無し（採否・署名・印はすべて導く）。分岐は、`plan_answer`・`plan_revise` の「key が `publish-signoff-v` で始まり主体が人でなければ拒否」の 1 つと、
   decisions の handler の「`publish-signoff` の `sign` の前に manifest を検査」の 1 つ。wire の任意の欄 `ChatMessage.proposal`・`ChatMessage.external`。fallback と新旧切替の flag は無し。
6. 撤去対象: 無し（置き換える仕組みが無い）。子 3 D3 の末尾「Web で chat の案を直接採用・却下する口は子 7 が決める」は D2 で決まった。
7. 不変条件: 上の「保証の表」。

## WorkUnit への割り当て

| WorkUnit | 書く場所 | 内容 |
|---|---|---|
| publish-api | `crates/task-core/src/publish.rs`（と試験、`lib.rs` の `mod` 1 行）、`crates/task-ops/src/decision.rs`（`plan_answer`・`plan_revise` の主体の拒否）、`crates/task-api/src/publish.rs`（新しい file: `manifest_status` と `GET /api/v1/tasks/{id}/publish/{manifest_id}`）、`crates/task-api/src/decisions.rs`（`sign` の前の検査と 403 の写し）、`crates/task-api/src/problem.rs`、`crates/celeris-mcp/src/tools/decisions.rs`（error の写しだけ）、`crates/task-api/src/schema.rs`・`docs/api/v1`・`web/api/generated`、`crates/task-api/tests/publish_manifest.rs` | D3 と `publish_manifest_`・`publish_signoff_` の試験 |
| proposal-api | `crates/task-core/src/proposal_review.rs`（新しい crate 直下の module。`chat` の本体は 8,000 行の上限に近いので置かない。子 3 の付記と同じ理由）、`crates/task-core/src/chat/`（wire の 2 欄と metadata の読み書き）、`crates/task-api/src/chat/`（route）、`crates/task-api/src/cos/ops/surface.rs`（`EXCLUDED`）と [ADR 2026-10-09-cos-operations-all-mutations](2026-10-09-cos-operations-all-mutations.md) の表、schema・`docs/api/v1`・`web/api/generated`、`crates/task-api/tests/proposal_review.rs` | D2 と `proposal_review_` の試験 |
| overview-api | `crates/task-ops/src/project_overview.rs`（と試験）、`crates/task-api/src/handlers/project_overview.rs`（route）、schema・`docs/api/v1`・`web/api/generated`、`crates/task-api/tests/project_overview.rs` | D1 と `project_overview_` の試験 |
| web-overview | `web/features/projects/project-overview-panel.tsx`（と unit 試験）、`project-detail-view.tsx`、`project-queries.ts`、`web/e2e/work/project-overview.spec.ts` | D1 の画面 |
| web-proposals | `web/features/chat/messages/`、`web/features/chat/data/client.ts`、`web/routes/index.tsx`（`message` の search の欄）、`web/e2e/chat/proposal-review.spec.ts` | D2 の画面 |
| web-publish | `web/routes/tasks.$id.publish.$manifestId.tsx`、`web/features/publish/`、`web/routeTree.gen.ts`、`web/e2e/work/publish-manifest.spec.ts` | D3 の画面 |
| close（close-audit・close-verify） | この ADR の文と末尾の付記、進捗 | close-audit: 保証の表の試験名の実在の確認（`git grep -qE 'fn <名前>\('`）・ADR の文の監査・LoC の測定・負の試験の再現（結果は付記）。close-verify: 提出時の HEAD での試験の再実行 |

- 結果: 6 つの実装の WU はすべて完了した（各 WU の進捗は [2026-11-23-strategy-v1-7](../progress/2026-11-23-strategy-v1-7/)）。上の表の外に書いた file:
  publish-api は `crates/task-api/src/files.rs`（成果物の読み方 `produced_artifacts`・`artifact_bytes` を移した）、proposal-api は
  `crates/task-core/src/proposal_review_wire.rs`（採否の状態と外部の印の導出。`chat` の wire の型がこれを使う）・`crates/task-core/src/strategy_ref.rs`
  （循環を広げないため `artifact_name`・`strategy_artifact` を移した）・`crates/task-core/src/chat/store.rs` などの `Box` 化、web-proposals は
  `web/lib/sha256.ts`・`web/features/chat/data/problem.ts`・`data/proposal.ts`、web-publish は `web/server/spa-routes.js` と `project-overview-panel.tsx`
  （O14 から manifest への link）、web の 3 つの WU は `web/e2e/support/fake-daemon.mjs` の fixture。

- 3 つの backend の WU は `crates/task-api/src/schema.rs`・`docs/api/v1`・`web/api/generated` を同じ段で変える。生成物の衝突は段の統合で再生成して解く
  （各 WU は自分の型の登録の行だけを足す）。
- 高 risk の path class: 上の書く場所は class に当たらない（`task_ops::decision` は `transaction` の `lifecycle.rs` と別の file、task-core の `store/` は変えない）。
  `crates/task-core/src/store/` を変える必要が出た WU は、設計 review の対象になることを計画に書く。
- deps.cycle: 新しい module は既存の module を一方向に使う（`publish` → `strategy`、`proposal_review` → `chat`・`chat_citation`、`project_overview` → `project_controller_view` 等）。
  逆向きの use を足さない。

## 未確認

- 概要の応答の時間を本番の大きさ（task 1,715 件・decisions 473 行・案件 thread の message 数千件）で測っていない。目標は 1 秒未満。
  fixture の案件（戦略 1 件・決定 1 件）では 2.60 ms と 2.65 ms（WU overview-api）。close の WU も本番の DB を読まないので測っていない。
  測るなら人が読み取り専用で行う。
- O7 の goal の軸: 子 6 の評価器の入力の欠け（`goal.outcomes`・`children[].criteria` が空、`goal_reached` が `false` 固定）は直っていない。直すのは子 8 の測定の前
  （子 6 の引き継ぎ）。それまで概要の goal の軸は Outcome の到達を表さない。
- knowledge の page へ path で直接移る Web の口（`/knowledge` の search の欄）は今の route に無い。WU web-overview の結果: literature の `path` の link は
  path を写せる文字で出し、`/knowledge` の検索の画面へ移す。page へ直接移る口は未実装。
- `notebook` の link は成果物の file を指すだけで、`.ipynb` を描画する viewer は無い。描画が要るかは人の判断待ち。
- 採用（`adopt`）の message を受けた CoS の execute の run は、外部の案の本文を thread で読む。本文の中の指示が CoS の行動を誘う余地は prompt の段に残る。
  backend の保証は、どの operation も goal_guard・strategy_gate・authority_gate を通ること、戦略の実行には人の承認が要ることまでである。
- `human:local`（token の無い構成）を署名に数えない規則は、戦略の承認（子 2 D2.2 は `is_human_principal` だけを見る）より厳しい。2 つを揃えるかは子 9 で人が決める。
- 人の token の名前が `local` のとき、その人の署名も数えない。本番の token の名前は確かめていない。
- 本番の config の変更は無い。本番への昇格・publish・外部公開は人が行う。
- overview-api の WU（2026-11-23）: 失効した引用（`citation_stale`）・予算の超過（`budget_exceeded`）・単価の不明（`cost_unknown`）は導出を実装したが
  試験が無い。`log` の link（run の id）は出さなかった（WU drilldown-api が X(v) の task に足した。下）。O10 の `wall_secs` は返さない（`RollupMetrics` に壁時計の合計が無い）。
  O3 の却下の候補（`contradicted`）は規則 1 の強い警告から外した（D1.3 の表の「無し」に合わせた）。
- close-audit（2026-11-24）が本文と WU の進捗から足した未確認と未実装:
  - 概要と manifest の画面の chat の link は `/?thread=` だけで、`message` を付けない（D1.4）。
  - 概要の panel は artifact の link の sha256 と今の file を照合しない（D1.4）。
  - run に `publish/<manifest_id>/v<n>.json` と決定 `publish-signoff-v<n>` を出させる prompt・計画は無い（D3.3）。署名の流れは試験の fixture でしか通っていない。
  - 本物の gateway は daemon の 403 を 502 `daemon_auth` に置き換え、画面に拒否の code が届かない（D4）。gateway を変えるかは人が決める。
  - Web の e2e は偽 daemon（`web/e2e/support/fake-daemon.mjs`）に対して流した。本物の daemon・本物のデータに対する画面の確認はしていない。password 付きの gateway の e2e も無い。
  - CoS の案が戦略の添付を 2 つ以上持つとき、Web は最初の 1 件で採否する。daemon がどの添付を照合するかは確かめていない。
- WU drilldown-api（2026-11-24）が残した未実装と未確認:
  - `git` の `task_id` は X(v) の task の取り込みとだけ照合する。X(v) でない案件の task が取り込んだ commit を戦略の文書が引いても `task_id` は付かない。
  - O7 の評価の記録を作った run の `log` は出さない。`log` は最後の run で、役割（worker・reviewer・planner）で選ばない。
  - 概要の drill-down の e2e は、この WU の時点では偽 daemon の fixture の link を押す。本物の route が出した fixture の JSON で流すのは WU drilldown-e2e の仕事
    （WU drilldown-e2e が直した。下の「付記（reclose-audit、2026-11-24）」）。
  - X(v) の task ごとに増えた読み取り（`runs_for_task`・`events_for`）の時間を本番の大きさで測っていない。

## 付記（close-audit、2026-11-24）

- 保証の表の試験名: 節を `awk '/^## 保証の表/{p=1;next} /^## /{p=0} p'` で切り出し、4 つの接頭辞の backtick の名前 74 件（重複を除く）を
  `git grep -qE "fn <名前>\(" -- crates` で 1 件ずつ確かめた。74 件すべてが crates の 1 file に定義されていた。略記（`…`・`...`）は 0 件。
  3 つの spec（`web/e2e/work/project-overview.spec.ts`・`web/e2e/chat/proposal-review.spec.ts`・`web/e2e/work/publish-manifest.spec.ts`）は表にあり、file も在る。
  crates にあって表に無い同じ接頭辞の `fn` は `proposal_review_state`（試験でない導出の関数）だけ。
- 本文の直し: 実装していない挙動を実装済みと書いていた所を「未実装」に直した（D1.3 の O5・O7 の `log`、O10 の `wall_secs`、D1.4 の `artifact` の上書きの表示・
  `log`・`literature` の `path`・`chat` の `message`、D3.3 の run が manifest と決定を出す流れ）。予定の文（冒頭の「後の WorkUnit が行う」、保証の表の「予定の試験」、
  未確認の「close の WU か人が測る」「web-overview の WU が確かめる」、WorkUnit の表の close の行）を結果か未確認に直した。
- 負の試験の再現: `task_ops::decision` の `plan_answer`・`plan_revise` の `signoff_actor(&row, by)?;` 2 行と、`task_api::chat::proposal_review::reviewer` の
  `Principal::Human` の照合を一時的に外し、`cargo nextest run -p task-api -p celeris-mcp -E 'test(/publish_signoff_|proposal_review_/)' --no-fail-fast` を流すと
  35 件のうち 11 件が落ちた（`publish_signoff_` 8 件・`proposal_review_` 3 件。名前は task の進捗の「試験」）。`git checkout --` で戻すと 35 件が通った。
- LoC と試験の結果は task の進捗 [2026-11-23-strategy-v1-7](../progress/2026-11-23-strategy-v1-7.md) に書いた。

## 付記（drilldown-api、2026-11-24）

- final review（v3）の不合格（本物の backend が実験の task の `log`・`notebook`・task 付きの `git` を出さない）を直した。`task_ops::project_overview` の
  `experiment_drills` が X(v) の task ごとに最後の run・`.ipynb` の成果物・最後の `IntegrationMerged` を読み、O5・O9・O10 の link と O6・O7 の `commit` の
  参照の `task_id` に使う。新しい表・event の種類・状態は足していない。
- 本文の直し: D1.3 の O5・O9・O10 の drill-down の列、O7 の未実装の範囲、D1.4 の `notebook`・`log`・`git` の備考、D1.5 の読み取り、保証の表の 4 行。
  O7 の `log` は未実装のまま残した。
- 変更前の再現と試験の結果は WU の進捗 [drilldown-api](../progress/2026-11-23-strategy-v1-7/drilldown-api.md) に書いた。

## 付記（reclose-audit、2026-11-24）

- final review（v3）の不合格の 2 点は修正の段で直した。(1) 本物の backend が drill-down の `log`・`notebook`・task 付きの `git` を出さず、e2e が偽 daemon の
  手書きの link を押していた → WU drilldown-api（backend と golden。上の付記）と WU drilldown-e2e（e2e）。(2) `docs/api/v1/api.md` が子 7 の口を書いていなかった
  → WU api-doc。助言（規約 2 `duplicate_implementation`）の 3 件は WU dedup-api が直した。
- WU drilldown-e2e: 偽 daemon の構成 `projectOverview: "drilldown"` が golden `crates/task-api/tests/fixtures/project_overview_drilldown.json` を書き換えずに返し、
  `web/e2e/work/project-overview.spec.ts` の drill-down の試験が golden の link で log・Notebook・作業ツリー・task 付きの Git・文献・chat の thread・task の画面へ移る。
  手書きの fixture（14 項目・印・強い警告の試験に使う）の link は golden にある (項目, kind) だけにした。この 2 つの整合は vitest
  `project_overview_fixture_links_are_kinds_the_golden_has_for_the_same_item`・`project_overview_drilldown_daemon_serves_the_golden_and_every_link_target` が守る。
  golden に知識の path の `literature` は無く、その link の e2e は手書きの fixture に対するもののまま残る。
- WU dedup-api: 本文の型と関数の名前の読み替え。挙動は同じである。
  - 決定の行と最後の回答の型は `task_core::decision::DecisionRecord` の 1 つ（子 2 の `strategy::ApprovalRecord` と D3.3 の署名の型 `publish::SignoffRecord` を消した。
    子 4 の ADR [2026-11-22-external-llm-discussion-exchange](2026-11-22-external-llm-discussion-exchange.md) の `ApprovalRecord` も同じ型を指す）。
  - D3.3 の 3・4 の「最後の `DecisionAnswered`」は `task_core::decision::DecisionRow::last_answer` で読む（store が回答と同じトランザクションで行に写した値。正本は event のまま）。
  - O2 の承認の照合（option が `approve` で主体が人）は子 2 D2.2 と同じ `task_core::strategy::approving_human` を呼ぶ。
  - manifest の検査の違反の wire の型は `task_core::publish::ManifestViolation`（`task_api::publish::ManifestViolationView` を消した）。
  - D3.3 の 4 の署名できる主体の判定 `task_core::publish::is_signoff_actor` はそのまま残る。
- 保証の表の試験名: 節を `awk '/^## 保証の表/{p=1;next} /^## /{p=0} p'` で切り出し、4 つの接頭辞の backtick の名前 78 件（`publish_manifest_` 15・`publish_signoff_` 17・
  `proposal_review_` 23・`project_overview_` 23。重複を除く）を `git grep -qE "fn <名前>\(" -- crates` で 1 件ずつ確かめた。78 件すべてが実在した（missing 0）。
  略記（`…`・`...`）は 0 件。3 つの spec は表にあり、file も在る（修正後の HEAD で実行）。
- 修正後の HEAD で `cargo nextest run -p task-core -p task-ops -p task-api -p celeris-mcp -E 'test(/publish_manifest_|publish_signoff_|proposal_review_|project_overview_|cos_ops_registry_/)' --no-tests=fail`
  → exit 0、83 passed。LoC と結果は task の進捗 [2026-11-23-strategy-v1-7](../progress/2026-11-23-strategy-v1-7.md) と WU の進捗
  [reclose-audit](../progress/2026-11-23-strategy-v1-7/reclose-audit.md) に書いた。
