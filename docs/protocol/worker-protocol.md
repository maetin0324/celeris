# celeris ワーカープロトコル

- 正本: 型は `crates/task-worker/src/protocol.rs`（`RunRequest` / `RunContext` / `WorkerMessage`）、JSON Schema は
  隣の `worker-protocol.schema.json`（その型から `schemars` で生成。`task-worker` のテスト
  `committed_schema_matches_generated` が一致を検証し、`UPDATE_SCHEMA=1 cargo test -p task-worker` で再生成する）。
  本文書と食い違うときは型と schema が正
- 版: `PROTOCOL_VERSION = 4`（`run.protocol` に載る）。欄・メッセージの**追加のみ**では上げない（受信側は未知の
  欄を無視し、新しい欄を出さない・読まないワーカーはそのまま動く）。上げるのは既存の形を変えるときだけ。
  ワーカーはこの値を検査しなくてよい
- 経緯（どの Phase で何を足したか）は各 ADR を参照: 基本形 [ADR-0003](../../agent-docs/adr/0003-worker-protocol.md)、
  CLI 系アダプタの結果ファイル [ADR-0006](../../agent-docs/adr/0006-phase4-claude-code-adapter.md)、kind 別出力
  [ADR-0007](../../agent-docs/adr/0007-phase5-planner-and-reviewer.md)、役割と委譲（v2）
  [ADR-0016](../../agent-docs/adr/0016-roles-and-delegation.md)、分野（v3）
  [ADR-0027](../../agent-docs/adr/0027-task-genres-and-research-harness.md)、組織・記憶・対話（v4）
  [ADR-0033](../../agent-docs/adr/0033-organization-projects-and-reports.md)、成果物の置き場
  [ADR-0036](../../agent-docs/adr/0036-per-task-artifacts.md)、作業場所・worktree・リポジトリ
  [ADR-0039](../../agent-docs/adr/0039-project-workspace.md) / [ADR-0041](../../agent-docs/adr/0041-self-improvement-loop-hardening.md) /
  [ADR-0043](../../agent-docs/adr/0043-workspaces.md)、コメント [ADR-0044](../../agent-docs/adr/0044-task-management.md)、
  構造化 progress [ADR-0048](../../agent-docs/adr/0048-console.md)、yield・予算・checkpoint・WorkUnit
  [ADR-0072](../../agent-docs/adr/0072-task-execution-decomposition.md)、クラスタ job の wait
  [ADR-0090](../../agent-docs/adr/0090-durable-wait-for-cluster-jobs.md)
- §1〜§8 は JSON Lines を stdin/stdout で直接話すワーカー（`fake` 等）の規約。§9 は CLI エージェント系アダプタ
  （`claude-code` / `codex` / `acp` / `aider`）が使う結果ファイル（`result.json`）の規約、§10 は kind 別の出力ファイル

## 1. 概要

オーケストレータ（celeris）はワーカーを **サブプロセス** として起動し、stdin に `run` を 1 行書いて閉じる。
ワーカーは stdout に JSON Lines で `progress` / `artifact` / `comment` / `delegate` を任意回、最後に終端メッセージ
（`done` / `error` / `question` / `yielded` / `budget_exhausted` / `wait`）のいずれか 1 つを書いて exit する。
全アダプタ（`fake` / `claude-code` / `codex` / `acp` / `aider` / `paperqa` / `local-deep-research` など）の結果はこの形に正規化される。

```
celeris ──stdin──▶ {"type":"run", ...}\n  (EOF)
celeris ◀─stdout── {"type":"progress", ...}\n
                 {"type":"artifact", ...}\n
                 ...
                 {"type":"done", ...}\n   | {"type":"error", ...}\n | {"type":"question", ...}\n
                 | {"type":"yielded", ...}\n | {"type":"budget_exhausted", ...}\n | {"type":"wait", ...}\n
       ◀─exit──
```

## 2. 符号化の規則

| 規則 | 内容 |
|---|---|
| 形式 | 1 行 1 JSON オブジェクト。UTF-8、`\n` 終端。行内に改行を含めない（文字列内は `\n` エスケープ） |
| 識別 | 全メッセージに `type`（文字列）が必須 |
| 未知フィールド | 受信側は無視する（前方互換） |
| 未知 `type` | JSON として妥当だが `WorkerMessage` に読めない行（未知の `type`・必須欄の欠落・型違い）はプロトコル違反。run を `error{retryable:false, message:"protocol violation: …"}` で打ち切り、プロセスグループを止める |
| 非 JSON 行・空行 | 破棄し警告ログ（ワーカーの stderr は別途 `runs/<run_id>/stderr.log` へ） |
| 行長 | 1 MiB 以下。超過はプロトコル違反（`error{retryable:false}`） |
| バージョン | `run.protocol = 4`（`PROTOCOL_VERSION`）。既存の形を変えるときだけ上げる（追加のみでは上げない） |

## 3. celeris → ワーカー

### 3.1 `run`

```json
{"type":"run",
 "protocol":4,
 "task":{ "...": "task-core::Task を serde でそのまま直列化したもの（role・aggregate を含む）" },
 "workspace":"/abs/path/to/workspace/<task_id>",
 "work_dir":"/abs/path/to/workspace/<task_id>/tree",
 "artifacts_dir":"/abs/path/to/workspace/<task_id>/artifacts",
 "context":{
   "prior_review":[{"criterion":0,"pass":false,"reason":"cargo test exit 101: ..."}],
   "inputs":[{"name":"spec.md","path":"inputs/spec.md","sha256":"…","kind":"doc"}],
   "answers":[{"question":"which crate version?","answer":"1.0"}],
   "role":{"id":"lead","instructions":"You coordinate the work of others."},
   "children":[{"id":"01J9…","title":"implement parser","role":"implementer","status":"done",
                "outcome":"done","artifacts":[{"name":"parser.rs","path":"artifacts/parser.rs","sha256":"…","kind":"rs"}],
                "workspace":"/abs/path/to/workspace/<child_task_id>","branch":"celeris/<child_task_id>"}]
 }}
```

| フィールド | 型 | 必須 | 説明 |
|---|---|---|---|
| `protocol` | integer | ✓ | `PROTOCOL_VERSION`（現在 `4`）。ワーカーはこの値を検査する必要はない |
| `task` | object | ✓ | `Task`（id, kind, title, objective, acceptance[], inputs[], depends_on[], status, priority, worker_hint, workspace, budget, attempts, `role`, `aggregate`, …）。`task.role`（`Option<string>`）はタスクの役割名、`task.aggregate`（`bool`。既定 false）は集約 run の親かどうか（ADR-0016 D1/D3） |
| `workspace` | string | ✓ | 絶対パス。`artifact.path` の基準で、`runs/` `inputs/` `artifacts/` の親。`work_dir` が無ければワーカーの cwd でもある |
| `work_dir` | string | –（省略可。Phase 49, ADR-0041 D1） | 絶対パス。ワーカーの cwd。タスクごとの `git worktree`（`<workspace>/tree`。ブランチ `celeris/<task_id>`）を切った run にだけ載る。このとき成果物は作業ツリーの**外**にあるので、前置きの成果物のパスは絶対パスになる |
| `artifacts_dir` | string | ✓（Phase 35, ADR-0036 D1） | 絶対パス。**この run の成果物と結果ファイル（`result.json` / `delegate.json` / `plan.json` / `review.json` / `summary.md`）の置き場**。workspace を自分で所有するタスクは `<workspace>/artifacts`、**workspace を親から継いだタスク（plan / delegate の子）は `<workspace>/.taskd/artifacts/<task_id>`**。決めるのは celeris（ディスパッチャ）で、ワーカーはここに書く。`artifact` メッセージの `path` は従来どおり **workspace 相対**（`.taskd/artifacts/<task_id>/report.md` の形）|
| `cargo_target_dir` | string | –（省略可） | この run に daemon が与えた `CARGO_TARGET_DIR` の監査用の写し（環境変数は別に渡る）。ワーカーは読まなくてよい |
| `context.prior_review` | array | ✓（空可） | 直前のレビュー結果。`{criterion: usize, pass: bool, reason: string}` |
| `context.inputs` | array | ✓（空可） | 依存成果物の `ArtifactRef`。`prepare()` で `workspace/inputs/` に配置済み |
| `context.answers` | array | –（省略可、空なら省略） | `celerisctl answer` で記録された `question` → 人間の回答の履歴（時系列、`{question: string, answer: string}`）。ADR-0010 D3, P-10。前方互換のため未知のワーカーは無視してよい |
| `context.role` | object | –（省略可。v2, ADR-0016 D1/M3） | タスクに役割があるときだけ `Some`。`{id: string, instructions: string}`（`instructions` は `[[roles]]` に指示文が無ければ空文字列）。`claude-code`/`codex` はプロンプトの前置きにする（`## Role: <id>`） |
| `context.children` | array | –（省略可。空なら省略。v2, ADR-0016 D3/M4） | 集約 run（`task.aggregate == true` の親の、子が全て終端になった後の run）でのみ非空。`ChildSummary`: `{id, title, role?, status, outcome?, artifacts: ArtifactRef[], workspace?, branch?}`。`branch` はその子が worktree で作業したときのブランチ（`celeris/<child_id>`。Phase 49, ADR-0041 D1）で、親はこれを merge して子の成果を統合する |
| `context.node` | object | –（省略可。v4, ADR-0033 D4） | `task.assignee` の組織ノード（担当が決まっている run だけ）。`{id, name, brief?}`。プロンプトの一番前に「あなたは誰で、何の担当か」として置かれる |
| `context.memory` | object | –（省略可。v4, ADR-0033 D6） | `[memory]` を設定し、担当が決まっている run だけ。`{notes?: string, project?: string}`（`<memory_dir>/<node_id>/notes.md` と `projects/<project_id>.md` の中身。それぞれ 8,000 字で切る） |
| `context.conversation` | array | –（省略可。空なら省略。v4, ADR-0033 D4） | その案件でのこのノードと人の**直近のやり取り**（既定 20 件、古い順）。`{role: "user"\|"node", text: string}` |
| `context.standing_rules` | array | –（省略可。空なら省略。v4, ADR-0033 D5） | 「今後ずっと」の認可（担当宛て + 全員向け。ADR-0033 D5）。前置きの「永続の認可」節になる |
| `context.organization` | array | –（省略可。空なら省略。v4, ADR-0033 D4） | 分解・委譲できる run（`context.available_genres` を渡す run と同じ条件）に渡す組織図。`{id, name, kind, parent_id?, brief?, genre?, skills?, harnesses?, tools?}`。「どの課に何を振るか」を `assignee` で決めさせる |
| `context.workspace_note` | string | –（省略可。Phase 43, ADR-0039 D3） | **案件が作業場所を決めている run** にだけ載る 1 行（そのコードがどこにあるか）。前置きに `## 作業場所` として出て、「編集は手元の作業ディレクトリで、別のホストの作業ツリーへ `ssh` で直接書くな」が続く。作業場所を決めていない案件・案件に属さないタスク・対話 run では省略（プロンプトは Phase 42 までとバイト単位で同じ） |
| `context.conversation_addressee` | string | –（省略可。Phase 28, ADR-0033 D4 追記） | 対話用タスクの run だけ `"secretary"` / `"other"`。委譲・`Question` は使えない（`delegate` は子を作らず理由を `progress` で返し、`Question` はそのまま `done` の返事になる） |
| `context.work_genre` | object | –（省略可。Phase 30, ADR-0033 D4 追記） | 対話 run で、担当ノードが自分の仕事の分野（`node.genre`）を持つときだけ。`GenreContext`（`context.available_genres[]` と同じ形）。対話そのものは常にこの分野ではなく対話用分野（`task.genre`）で走る。前置きに「仕事で使う道具」として 1 行渡すためだけの情報 |
| `context.milestone_review` | object | –（省略可。Phase 41, ADR-0038 D1） | **途中目標レビューの対話 run**（対話の印 + `task.milestone_id`）にだけ。`{milestone: {id, title, description?, status}, tasks: [{title, status, outcome?, artifacts_excerpt?}]}`。`tasks` はその途中目標に属する仕事（裏方は除く。作られた順、最大 20 件）、`outcome` は `context.recent_work[].outcome` と同じ終端の要約、`artifacts_excerpt` は `answer.md` / `report.md` の先頭 4,000 字（決定的に切る）。集めるのはストアとファイルの読み取りだけ（LLM は使わない） |
| `context.comments` | array | –（省略可。空なら省略。Phase 53, ADR-0044 D2） | そのタスクのコメントの**最新 20 件**（古い順）。`{author_kind: "human"｜"node"｜"system", author?, body, at}`。前置きの**先頭**に「コメント」節として出る |
| `context.interrupt` | string | –（省略可。Phase 53, ADR-0044 D2） | **直前の run を人のコメントで止めた**とき、そのコメントの本文。前置きのいちばん先に「**人からの割り込み**: …」として出る。割り込みの直後の run にだけ載る（その run が終わったら消える） |
| `context.comments_enabled` | bool | –（省略可。既定 false。Phase 53, ADR-0044 D2） | この run は `comment` を書ける。true のときだけ前置きに「短い進捗や判断の記録はコメントに書け」が出る。**仕事の run は true、対話 run（Phase 28 の「返事だけをする」run）とレビュー run は false** |
| `context.recent_work` | array | –（省略可。空なら省略。Phase 33, ADR-0033 D4 追記） | 対話 run にだけ、担当の直近の仕事（最大 10 件、更新の新しい順。案件を選んでいればその案件のものを先に。対話・まとめ・承認・レビューは除く）。`RecentWork`: `{task_id, title, project_title?, status, finished_at?, outcome?, artifacts: string[]}`。`outcome` は終端の要約（`done` なら summary の 1 行目、`failed` なら理由、`blocked` なら質問）で、ストアのタスクとイベントから決定的に組む（LLM は使わない） |
| `context.review` | object | –（省略可） | Review run だけ。`ReviewRequest`: `{summary, evidence, criteria, decisions?, answers?, checks?}`（§10.2） |
| `context.available_genres` | array | –（省略可。空なら省略。v3） | 分解・委譲できる run に渡す分野の一覧。`GenreContext`: `{id, description, capabilities?, input_artifacts?, output_artifacts?, roles?, harness?}` |
| `context.subject_genre` | object | –（省略可） | レビュー対象の仕事の分野（`GenreContext`） |
| `context.clusters` | array | –（省略可。空なら省略） | 使えるクラスタ。`{id, connected, work_dir?}` |
| `context.profile` | object | –（省略可） | 担当の実効 profile（`task_core::EffectiveProfile`） |
| `context.mode` | string | –（省略可） | タスクの mode（`task_core::TaskMode`） |
| `context.knowledge` | object | –（省略可） | 読める知識のマウントと索引。`{mounts?, index?}` |
| `context.active_projects` | array | –（省略可。空なら省略） | 進行中の案件。`{id, title, status, repos?, milestones?: [{id, title, status}]}` |
| `context.session` | object | –（省略可） | 継続セッションの手がかり。`{adapter, session_id, resume}`。`adapter` が自分と違えば無視する |
| `context.cos_chat` | object | –（省略可） | CoS chat run の入力（ADR 2026-10-06 cos-chat-run-dispatch）。`{thread_id, run_id, inputs[{id,seq,text,interrupt,attachment_ids}], summary, summary_through_seq, unsummarized{from_seq,through_seq,messages[]}, delivered_through_seq?, attachments[{id,name,media_type,size_bytes,sha256,path,delivery=image\|file}], skills[], credential_env, api_base_url}`。ある run の `task` はディスパッチャが保存しない一時の値。`delivered_through_seq` は resume した session が既に持つ seq の水位で、ある時は `summary` を省き `unsummarized` はその後の差分だけ（ADR 2026-10-05 cos-chat-home D2 付記）。credential は `credential_env` が名指す環境変数でだけ渡り、値はここに載らない |
| `context.session_diff` | array | –（省略可。空なら省略） | 継続セッションの前回からの差分（文字列の配列） |
| `context.skills` | array | –（省略可。空なら省略） | mount された skill。`{name, path, description?}`（`path` は `SKILL.md` を含むディレクトリの絶対パス） |
| `context.continuation` | object | –（省略可） | 続きの run（yield・予算切れ・wait の後）だけ。`{run_seq, previous_end, checkpoint, prior_runs?, cluster_jobs?}` |
| `context.work_unit` | object | –（省略可） | WorkUnit の run だけ。`{key, title, objective, done_when?, task_objective_excerpt, dependency_summaries?, plan_overview?, branch?, parallel_siblings?, human_decisions?, previous_check_failures?}` |
| `context.execution_planner` | object | –（省略可） | 実行計画を書く planner run だけ。上限・gate の結果・replan の情報（`ExecutionPlannerContext`。欄は schema を参照） |
| `context.decision_requests` | bool | –（省略可。既定 false） | true なら結果ファイルに人への決定の要求（`decisions`）を書ける |
| `context.browser` / `context.browser_policy` | object | –（省略可） | browser capability を使う run だけ（`crate::browser::BrowserContext` / `task_core::BrowserTaskPolicy`） |

`context.answers` は、このタスクの `Event::Answered` を時系列に並べたもの（`question` は直前の
`WorkerFinished.outcome` の `"question: "` 接頭辞から取ったもの、無ければ空文字列）。`claude-code`/`codex`
アダプタは、空でなければプロンプトに「以前の質問への人間の回答」節として反映する（Execute/Plan のみ。
Review プロンプトには含めない）。JSON Lines プロトコルを直接話す `fake` 等のワーカーは、この配列を読んで
自由に扱ってよい（celeris 側は解釈を強制しない）。

`context.role` / `context.children` も同様に、JSON Lines を直接話すワーカーは自由に解釈してよい
（celeris 側は解釈を強制しない）。`claude-code`/`codex` の反映のしかたは §9 M8 を参照。

**v4 の前置き（ADR-0033 D4/D6, Phase 24。Phase 28 で末尾に対話専用の指示を追加。Phase 30 で役職と brief の
直後に「仕事で使う道具」を追加。Phase 33 で記憶の直後に「あなたの直近の仕事」を追加）**: `context.node` /
`context.work_genre` / `context.standing_rules` / `context.memory` / `context.recent_work` /
`context.conversation` / `context.role` は、CLI エージェント系アダプタでは `task_worker::preamble::render` が
**この順**で 1 か所に組む（役職と brief → 仕事で使う道具（あれば） → 永続の認可 → 記憶 →
あなたの直近の仕事（対話 run にだけ） → 直近のやり取り → 役割の指示文 → 記憶の書き方 → 対話専用の指示）。
`context.conversation_addressee` が `None`（対話でない run）ならこの末尾の節は出ず、`context.work_genre` /
`context.recent_work` も渡らないので、前置きは Phase 23 までの出力と 1 バイトも変わらない。
Phase 41（ADR-0038 D1）: `context.milestone_review` があるときだけ、「あなたの直近の仕事」の直後に
**「途中目標『X』のここまで」**（各仕事の status / 終端の要約 / 成果物の抜粋）が入り、いちばん最後に
**「途中目標の判定をお願いする返事です」**（(a)〜(d) と `milestone_proposal` の書き方）が足される
（対話専用の指示は消えない）。
Phase 53（ADR-0044 D2）: `context.comments` か `context.interrupt` があるときだけ、**いちばん先頭**に
「コメント」節が入る（`interrupt` があればその本文が節の先頭に「**人からの割り込み**: …」として置かれ、
続けてコメントの糸が古い順に並ぶ）。`context.comments_enabled` が true なら、記憶の書き方の直後に
「コメントの書き方」が入る。どちらも無い run の前置きは Phase 52 までと 1 バイトも変わらない。
`local-deep-research` だけは役割の指示文を載せない（ADR-0029 / Phase 19: 検索エンジンに渡す問いを
役割の文面で濁さないため）。

## 4. ワーカー → celeris

`WorkerMessage` の variant は次の 10 種（`#[serde(tag = "type", rename_all = "snake_case")]`）。非終端は何度出してもよく、
終端は run につき 1 つ（§5）。

| `type` | 終端 | 例 |
|---|---|---|
| `progress` | – | `{"type":"progress","msg":"running cargo test"}` |
| `comment` | – | `{"type":"comment","body":"ベンチは 12% 速くなった"}` |
| `delegate` | – | `{"type":"delegate","tasks":[{"title":"…","objective":"…","acceptance":[…]}]}` |
| `artifact` | – | `{"type":"artifact","name":"bench.json","path":"artifacts/bench.json","kind":"json"}` |
| `question` | ✓ | `{"type":"question","text":"Which Python version?"}` |
| `done` | ✓ | `{"type":"done","summary":"…","evidence":[]}` |
| `error` | ✓ | `{"type":"error","message":"…","retryable":true}` |
| `yielded` | ✓ | `{"type":"yielded","checkpoint":{"remaining":["…"]}}` |
| `budget_exhausted` | ✓ | `{"type":"budget_exhausted","kind":"turns","message":"error_max_turns"}` |
| `wait` | ✓ | `{"type":"wait","kind":"cluster_job","cluster":"sirius","jobs":["42634"]}` |

### 4.1 `progress`（任意回）

```json
{"type":"progress","msg":"running cargo test"}
{"type":"progress","msg":"tool_use: Bash {\"command\":\"cargo test --workspace\"}","kind":"tool_use","tool":"Bash","summary":"cargo test --workspace","detail":"{\"command\":\"cargo test --workspace\"}"}
```

| フィールド | 型 | 必須 | 説明 |
|---|---|---|---|
| `msg` | string | ✓ | 人が読む 1 行。**従来どおり**（この行だけのワーカーはそのまま動く） |
| `kind` | string | – | **Phase 60a（ADR-0048 D2）**。`tool_use` / `tool_result` / `text` / `thinking` / `status` のどれか。知らない語は**その行ごと**読み捨てになるので、迷ったら付けない |
| `tool` | string | – | `tool_use` / `tool_result` のときの道具の名前（`Bash` / `Read` …） |
| `summary` | string | – | 折り畳んだ Console の見出しに出る 1 行（`tool_use` は入力の要約、`tool_result` は出力の先頭、`text` / `thinking` は本文の先頭） |
| `detail` | string | – | 本文（**4 KiB まで**。超えたら切って `truncated: true`）。`thinking` には付けない |
| `truncated` | bool | – | `detail` を切った（既定 `false`） |
| `error` | bool | – | `tool_result` が失敗だった（既定 `false`） |

`WorkerProgress{run_id, msg, kind?, tool?, summary?, detail?, truncated?, error?}` として記録される
（`docs/api/v1/event.schema.json`。**追加のみ**なので導入前のイベントもそのまま読める）。無出力タイムアウトの
カウンタをリセットする。

**`PROTOCOL_VERSION` は 4 のまま**（追加のみ。この 6 つを出さないワーカーも、読まない celeris も従来どおり動く）。

**アダプタごとの写像**（ここだけがアダプタ固有。ADR-0048 D2）:

| アダプタ | 出どころ | `kind` |
|---|---|---|
| `claude-code` | stream-json の `assistant.content[]` | `tool_use`（`tool` + 入力の 1 行要約: `Bash` はコマンド、`Read`/`Write`/`Edit` はパス、`Grep`/`Glob` は模様、他は入力の先頭 120 文字）、`text`（1 メッセージ分の本文をまとめて 1 件）、`thinking`（要約だけ） |
| `claude-code` | stream-json の `user.content[].tool_result` | `tool_result`（先頭 200 文字、`is_error` → `error`） |
| `codex` | `item.*`（`command_execution` / `mcp_tool_call` / `web_search` / `file_change` / `patch_apply`） | 開始・更新は `tool_use`、`item.completed` は `tool_result`（`exit_code != 0` → `error`） |
| `codex` | `item.*`（`agent_message` / `reasoning`） | `text` / `thinking` |
| `codex` | それ以外の `item.*`（`todo_list` など） | `status` |
| `acp` | `session/update` の `tool_call` / `tool_call_update` | `tool_use`（`completed` / `failed` は `tool_result`） |
| `acp` | `agent_message_chunk` / `agent_thought_chunk` | `text` / `thinking`（細切れは改行か 400 文字で束ねてから 1 件） |
| `paperqa` / `local-deep-research` / `fake` | 節目の行 | `status` だけ |

### 4.2 `artifact`（任意回）

```json
{"type":"artifact","name":"bench.json","path":"artifacts/bench.json","kind":"json"}
```

| フィールド | 型 | 必須 | 説明 |
|---|---|---|---|
| `name` | string | ✓ | 一意名。`ArtifactExists{name}` の照合キー |
| `path` | string | ✓ | **ワークスペース相対**。絶対パス・`..`・ワークスペース外へのシンボリックリンクは拒否 |
| `kind` | string | – | `log` / `diff` / `json` / `md` / その他自由文字列。省略時は拡張子から推定 |

受信時に celeris が sha256 を計算し `ArtifactProduced{run_id, artifact: ArtifactRef{name,path,sha256,kind}}` を記録する。ファイルが無ければ警告のみ。

### 4.3 `question`（終端）

```json
{"type":"question","text":"Which Python version should the benchmark target?"}
```

| フィールド | 型 | 必須 |
|---|---|---|
| `text` | string | ✓ |

タスクは `blocked` になる。ワーカーはこの行を書いたら exit する。人間の回答は `celerisctl answer` で記録され、次回の `run` の `context.answers` に渡る（§3.1・§9）。

### 4.4 `done`（終端）

```json
{"type":"done",
 "summary":"Added CLI parsing with clap; all tests pass.",
 "evidence":[
   {"criterion":0,"command":"cargo test","exit":0,"stdout_tail":"test result: ok. 12 passed"},
   {"criterion":1,"command":"test -f README.md","exit":0,"stdout_tail":""},
   {"criterion":2}
 ],
 "usage":{"input_tokens":12345,"output_tokens":678}}
```

| フィールド | 型 | 必須 | 説明 |
|---|---|---|---|
| `summary` | string | ✓ | 人間向け要約 |
| `evidence` | array | ✓（空可） | 受け入れ条件ごとの証拠 |
| `evidence[].criterion` | integer | ✓ | `task.acceptance` の添字 |
| `evidence[].command` | string | – | 実行したコマンド。`ArtifactExists` / `Reviewer` / `Human` の条件では省略してよい（ADR-0012 D3, P-12） |
| `evidence[].exit` | integer | – | 終了コード（同上） |
| `evidence[].stdout_tail` | string | – | 出力末尾（同上）。4 KiB を目安に切り詰める |
| `usage` | object | – | `input_tokens`, `output_tokens`（integer）。取れないアダプタは省略 |

`done` は完了ではない。タスクは `reviewing` に入り、Reviewer が `Command` を再実行し `ArtifactExists` を検査する（完了はレビュアーか決定的な検査が決める。`docs/SPEC.md`）。

### 4.5 `error`（終端）

```json
{"type":"error","message":"claude exited with error_max_turns","retryable":true}
```

```json
{"type":"error","message":"429 rate limit exceeded","retryable":true,
 "provider_failure":{"kind":"throttled","retry_after_secs":60}}
```

| フィールド | 型 | 必須 | 説明 |
|---|---|---|---|
| `message` | string | ✓ | |
| `retryable` | boolean | ✓ | `true` → `attempts+1` の上で `max_retries` 内なら `ready`、超過で `failed`。`false` → `failed` |
| `provider_failure` | object | – | 供給側の失敗の種別（ADR-0010 D5, P-21）。付いていれば `retryable` の値に関わらずディスパッチャは `attempts` を消費せず `requeue` し、そのプロバイダを cooldown にする |

`provider_failure.kind` は次の 3 種のいずれか（`#[serde(tag="kind", rename_all="snake_case")]`。判定・分類は
アダプタが行い、遷移の判断（requeue するかどうか）はディスパッチャの責務。原則: 協調判断に LLM を使わず、
ここも決定的な規則だけで完結する）:

| `kind` | 追加フィールド | 意味 |
|---|---|---|
| `throttled` | `retry_after_secs`（integer） | レート制限。しばらく待てば復帰しうる |
| `auth_failed` | – | 認証切れ・未ログイン。人間の対応が要る |
| `exhausted` | – | 利用上限・クレジット枯渇 |

JSON Lines プロトコルを直接話す `fake` 等のワーカーは `provider_failure` を任意で付けてよい。
`claude-code`/`codex` はこのプロトコルを話さないので、代わりにエラー文面を決定的な文字列規則で分類する
（§9 参照）。

### 4.5b `yielded`（終端。ADR-0072 D9/D10, Phase E1）

```json
{"type":"yielded",
 "checkpoint":{"completed":["store に execution.rs を追加"],"remaining":["dispatcher の配線"],
   "next_action":"dispatcher.rs の on_worker_finished を直す"},
 "usage":{"input_tokens":12345,"output_tokens":678}}
```

| フィールド | 型 | 必須 | 説明 |
|---|---|---|---|
| `checkpoint` | object | – | checkpoint の意味の欄（下記 §4.5d と同じ形。省略可だが、省略すると daemon は mechanical だけで checkpoint を作る） |
| `usage` | object | – | `done` と同じ形 |

自分から「ここで区切る」と判断した run（graceful yield。§9 の予算の予告を参照）。予算切れではないが、
**完了でもない**。`Trigger::Continue` で `ready` に戻り、`checkpoint` を引き継いで**新しい session**が
続く（`Task` は `failed` にも `blocked` にもならない。ADR-0072 D9）。`[execution] continuation = false`
のときは従来どおり `error(retryable=true)` と同じ扱いになる。

### 4.5c `budget_exhausted`（終端。ADR-0072 D7, Phase E1）

```json
{"type":"budget_exhausted","kind":"turns","message":"error_max_turns","usage":{"input_tokens":12345,"output_tokens":678}}
```

| フィールド | 型 | 必須 | 説明 |
|---|---|---|---|
| `kind` | string | ✓ | `turns` \| `wall_clock` \| `context` |
| `message` | string | ✓ | 人間向けの理由 |
| `usage` | object | – | `done` と同じ形（予算切れでも usage は運ぶ。従来は捨てていた） |

JSON Lines プロトコルを直接話す `fake` 等のワーカーが自己申告することもできるが、通常は
`claude-code`/`codex`/`acp` アダプタが `--max-turns` の上限や wall-clock の打ち切りから構造化して作る
（§9）。`yielded` と同じく `Trigger::Continue` で続く（checkpoint は `<artifacts_dir>/checkpoint.json`
（§4.5d）と mechanical な git の読み取りから daemon が合成する）。

### 4.5d rolling checkpoint（`<artifacts_dir>/checkpoint.json`。任意、何度でも上書き。ADR-0072 D8/D10）

予算が尽きる前に区切りよく止まれるよう、全 harness の前置き（§9）でワーカーに
「意味のある区切りのたびに `<artifacts_dir>/checkpoint.json` を上書きせよ」と指示する。
`result.json` のプロトコルの外（run の終端を待たずに、途中で何度でも書き直せるファイル）。
schema は `celeris.checkpoint/1`（`docs/protocol/checkpoint.schema.json`。daemon が生成）:

```json
{"completed":["A を実装した"],"remaining":["B のテスト"],
 "decisions":[{"what":"WU は直列実行","why":"worktree 共有のため"}],
 "files_changed":[{"path":"src/lib.rs","change":"modified","note":"A の実装"}],
 "tests_run":[{"command":"cargo test -p foo","exit":0,"summary":"12 passed"}],
 "known_failures":[{"what":"clippy の警告 1 件","detail":"lib.rs:42"}],
 "artifact_refs":[{"path":"artifacts/notes.md","kind":"doc"}],
 "next_action":"B のテストを書く",
 "open_questions":[],"plan_issue":null}
```

ここに書けるのは**意味の欄だけ**（`completed`/`remaining`/`decisions`/`files_changed`/`tests_run`/
`known_failures`/`artifact_refs`/`next_action`/`open_questions`/`plan_issue`）。`schema`/`task_id`/
`run_id`/`run_seq`/`end`/`source`/`repo_state`/`recent_activity`/`created_at` は daemon が run 終了後に
埋める（mechanical。git の読み取りと `WorkerProgress` の tool_use から決定的に作る）。ワーカー側は
寛容に読まれる（`deny_unknown_fields` ではない。未知の欄は捨てる）。schema 違反（型が合わない・JSON が
壊れている）や、このファイルを書かなかった run は、daemon が mechanical な事実だけで checkpoint を作る。
全体は 16 KiB、配列はそれぞれ 30 件、文字列はそれぞれ 500 文字で決定的に切り詰められる。

### 4.6 `delegate`（任意回、非終端。v2, ADR-0016 D2）

```json
{"type":"delegate","tasks":[
  {"title":"implement the parser","objective":"...","acceptance":[{"text":"cargo test passes","check":{"type":"command","cmd":"cargo test","expect_exit":0}}],"role":"implementer","depends_on":[]},
  {"title":"write docs","objective":"...","acceptance":[{"text":"README updated","check":{"type":"artifact_exists","name":"README.md"}}],"depends_on":[0]}
]}
```

| フィールド | 型 | 必須 | 説明 |
|---|---|---|---|
| `tasks` | array | ✓（空も可だが意味がない） | `DelegateTask` の配列。1 回の `delegate` メッセージにつき複数件でよく、同じ run が複数回 `delegate` を送ってもよい |

`DelegateTask`（`task-core::DelegateTask`）:

| フィールド | 型 | 必須 | 説明 |
|---|---|---|---|
| `title` | string | ✓ | 空文字列は不合格 |
| `objective` | string | ✓ | 空文字列は不合格 |
| `acceptance` | array | ✓（1 件以上） | `task-core::Criterion` の配列（`text` + `check`）。空配列は不合格 |
| `role` | string | – | 役割名（`[[roles]]` にあれば既定と指示文が効く。無くても自由記述として許される） |
| `depends_on` | array | –（省略可、空なら省略） | 各要素は整数（同じ `tasks` 配列内のインデックス）か、既存タスクの ID（文字列）。混在可 |
| `tier` | string | – | `frontier` / `standard` / `cheap`。省略時は役割の既定 → 親の tier |
| `workspace` | object | –（省略可。Phase 43, ADR-0039 D2） | その子の作業場所。`{"kind":"local","path":"..."}` か `{"kind":"remote","cluster":"<[[clusters]] の id>","path":"<クラスタ側のパス>"}`。**省略時は案件の作業場所 → 親の workspace** を継ぐので、別のリポジトリ・別のクラスタで作業させたいときにだけ書く（`local` の `~` は celeris の `$HOME` で展開される） |

未知フィールドは拒否する（`deny_unknown_fields`。綴り間違いの検出のため。§2 の「未知フィールドは無視する」
という一般規則とは意図的に逆。`Plan` の `NewTask` と同じ方針）。

celeris 側の扱い（ADR-0016 D2, 実装メモ M2/M6/M7）:

- ディスパッチャは受け取った提案を、ストアを見ない検証（空欄、`depends_on` の範囲・自己参照・閉路、ID の
  書式）と、ストアを見る検証（既存 ID の依存が存在し `failed`/`cancelled` でないこと、依頼元の祖先や自分
  自身に依存していないこと、木の深さ・件数・run 数の上限）の両方を通ったものだけを子タスクとして挿入する。
  挿入は `draft` → 同じトランザクションで `Accept`（`ready`）、`Event::Delegated{run_id, task_ids}` を記録する
  （`plan.auto_accept` は見ない）。
- 上限（設定 `[delegation]`。既定値）: `max_delegate_per_run`（8。同じ run の複数の `delegate` をまたいで数える）、
  `max_tree_depth`（5。根 = 1）、`max_tree_runs`（100。根から全ての子孫の `WorkerStarted` の合計）。
- 拒否した提案は挿入せず、理由を `WorkerProgress{msg: "delegate rejected: tasks[i] \"<title>\": <reason>"}` として
  残す。**run 自体は失敗しない**（拒否は run の terminal に影響しない）。
- 親は run を続けてよい。親が `done` を返しても、委譲した子が全て終端になるまで親は `reviewing` のまま
  （`task.aggregate == true` なら M4 の集約 run、`false` なら子の成否を問わず `done` になる）。
- 自分の親（祖先を含む）や自分自身を `depends_on` に指定することはできない。

### 4.7 `comment`（任意回、非終端。Phase 53, ADR-0044 D2）

```json
{"type":"comment","body":"ベンチは 12% 速くなった。次は書き込み側を見る"}
```

| フィールド | 型 | 必須 |
|---|---|---|
| `body` | string | ✓ |

`progress` と違って**残る**記録。celeris は `task_comments` に `author_kind = "node"`、
`author = task.assignee`（担当が無ければ `null`）、`run_id` = この run で 1 行足す
（`GET /tasks/{id}/comments` と `GET /tasks/{id}/timeline`、次の run の前置きに出る）。

- **run は止まらない**（非終端）。状態機械も動かない。
- **人を起こさない**（通知は ADR-0037 の 5 種のまま）。人が書いたコメントだけが
  `POST /tasks/{id}/comments` 経由で担当を起こす（[gui-api.md](../api/v1/gui-api.md) の `POST /tasks/{id}/comments`）。
- 空白だけの本文・20,000 文字超は捨てる（警告だけ。run は続く）。
- 前置きには「短い進捗や判断の記録はコメントに書け」と書いてある（`context.comments_enabled`）。
  長い成果は成果物（`artifact`）に書くこと。
- **追加のみ**なので `PROTOCOL_VERSION` は 4 のまま。この行を出さない既存のワーカーは何も変わらない。

### 4.8 `wait`（終端。ADR-0090 D1）

```json
{"type":"wait","kind":"cluster_job","cluster":"sirius","scheduler":"pbs","jobs":["42634","42635"],
 "poll_secs":300,"timeout_secs":86400,
 "checkpoint":{"completed":["job を投入した"],"next_action":"結果を集計する"},
 "summary":"IOR を 2 本投入した"}
```

| フィールド | 型 | 必須 | 説明 |
|---|---|---|---|
| `kind` | string | ✓ | 常に `"cluster_job"`（それ以外は不正） |
| `cluster` | string | – | `[[clusters]] id` |
| `scheduler` | string | – | `pbs` \| `slurm`。省略時は `pbs` |
| `jobs` | array | ✓ | 待つ job id（文字列） |
| `poll_secs` / `timeout_secs` | integer | – | 確かめる間隔と待つ上限 |
| `checkpoint` | object | – | `yielded` と同じ checkpoint の意味の欄 |
| `summary` | string | – | 待つ前の run の要約 |
| `usage` | object | – | `done` と同じ形 |

クラスタ job の終了を待って続きを走らせる。検証は `result.json` の wait と同じ
（`task_core::cluster_job::parse_wait_request`）で、不正なら `error{retryable:true}` になる。daemon が job を
見て、終わったら（または `timeout_secs`・cancel で）次の run を始め、`context.continuation.cluster_jobs` に
job の最終状態を渡す。`yielded` と違い、continuation の回数・進捗なしの窓・attempts には数えない。

## 5. 終了規則

1. 終端メッセージ（`done` / `error` / `question` / `yielded` / `budget_exhausted` / `wait`）は run につき 1 つ。最初の終端で読むのをやめ、後の行は捨てる。
2. 終端メッセージ無しで exit した場合、exit code に関わらず `error{retryable:true, message:"worker exited without terminal message (exit=N)"}` と等価に扱う。
3. exit code は `WorkerFinished{run_id, outcome, usage}` に記録するが、状態遷移には使わない。
4. 打ち切りはどの経路でも **SIGTERM → 猶予（`kill_grace_secs`、既定 10 s）→ SIGKILL を
   プロセスグループごと**送る（孫プロセスまで届く。コンテナ実行の run はコンテナにも同じ 2 段を送る）。
   アダプタ自身が打ち切る場合（wall-clock 超過・無出力タイムアウト・プロトコル違反）も、
   celeris 側から打ち切る場合（`cancel`、人のコメントによる割り込み、リース喪失、drain タイムアウト。
   `task_dispatch` の `stop_run`）も同じ。割り込みで止めた run は
   `WorkerFinished{outcome: "interrupted: comment"}` として記録し、**失敗には数えない**
   （報告も `error_cooldown` も作らない）。worktree はそのまま残すので、次の run が続きから直せる。

## 6. タイムアウト

| 上限 | 出所 | 既定 | 超過時 |
|---|---|---|---|
| wall-clock | `task.budget.max_wall_secs` | タスクごと | `error{retryable:true,"wall clock exceeded"}` |
| 無出力 | 設定 `idle_timeout_secs`（アダプタごとに上書き可） | 300 | `error{retryable:true,"idle timeout"}` |

`progress` / `artifact` / `comment` / `delegate` の受信で無出力カウンタをリセットする（stdout の 1 行ごと）。

### 6.1 heartbeat（リースの延長。プロトコルのメッセージではない）

`heartbeat` は本文書が定義する JSON Lines メッセージ（§4 の `WorkerMessage`）の
1 つではない。ワーカーの stdout から **1 行読むたび**（`progress`/`artifact` 等の既知メッセージだけでなく、
破棄される非 JSON 行や行長超過も含む）に、アダプタ内部で `EventSink::heartbeat()` を呼ぶだけの生存通知
である（ADR-0010 D7, P-7）。ワーカー自身がこれを送出するのではなく、アダプタが「まだ子プロセスが出力して
いる」という事実からこの通知を合成する。

ディスパッチャ（`StoreSink::heartbeat`）はこれを使って DB 上のリース（`expires_at`）を延長する
（`renew_lease`。状態遷移ではないので `events` には記録しない）。取得時のリース ttl は
`max_wall_secs + lease_grace` のままだが、無出力タイムアウト（`idle_timeout`）より wall-clock の方が
大幅に長い場合でも、生きているワーカーのリースが `idle_timeout` 到達前に切れることはない
（延長間隔を `lease_grace / 2` 以下に保つので、延長後の期限は「直前の出力時刻 + `idle_timeout` + `lease_grace / 2`」以降になる）。
ただしこれは、無出力で強制終了された run の結果が `kill_grace`（SIGKILL までの猶予）と `tick_ms`（次 tick での取り込み）の
分だけ遅れて処理されることを含めて `kill_grace + tick_ms < lease_grace / 2` のときに成り立つ。`celeris` は設定検証でこれを要求する
（ADR-0010 D7）。

### WorkUnit の計画 check の書き方

`checks` の範囲検査は `scope: true` とし、unit 開始時の基点からの差分を使う。
ローカル git の直列実行・専用 worktree のどちらでも、daemon は worker と事後 check に
`CELERIS_WU_BASE`（先行 unit の未 commit の追跡済み成果を含む一時 commit）、
`CELERIS_WU_BASE_UNTRACKED`（開始時の未追跡 path 一覧）、`CELERIS_WU_SCOPE_PATHS`（全 file の差分補助）を渡す。
retry と daemon の検査引き継ぎでは初回の snapshot を再利用する。

```sh
paths=$(if [ -n "${CELERIS_WU_SCOPE_PATHS:-}" ]; then sh "$CELERIS_WU_SCOPE_PATHS"; else git diff --name-only "${CELERIS_WU_BASE:-HEAD}" && git ls-files --others --exclude-standard; fi) || exit 1
out=$(printf '%s\n' "$paths" | sort -u | grep -vE '^(<許可 path の正規表現>)'); [ -z "$out" ] || { echo "out of scope:"; echo "$out"; exit 1; }
```

開始時の未追跡 file を単に `git ls-files --others` で足すと先行成果を誤検知する。
補助は開始時の未追跡 file の編集・削除も検出するため、設定されていれば必ず使う。
詳細は [WU checks の指針](../../agent-docs/guides/work-unit-checks.md) と
[開始時 snapshot の ADR](../../agent-docs/adr/2026-10-08-work-unit-scope-snapshot.md)。

## 7. JSON Schema

正は隣の [`worker-protocol.schema.json`](worker-protocol.schema.json)（`task_worker::protocol::schema_value()` が
`ProtocolSchema{run, message, review_output}` から生成する）。本文書には手書きの抜粋を置かない。
checkpoint は [`checkpoint.schema.json`](checkpoint.schema.json)、計画は [`plan-output.schema.json`](plan-output.schema.json) /
[`execution-plan.schema.json`](execution-plan.schema.json) が正。

## 8. 例: fake ワーカーの 1 run

stdin:

```json
{"type":"run","protocol":4,"task":{"id":"01J8…","kind":"execute","title":"add README example","acceptance":[{"text":"cargo test exits 0","check":{"type":"command","cmd":"cargo test","expect_exit":0}}}],"budget":{"max_turns":20,"max_wall_secs":600,"max_retries":1},"attempts":0},"workspace":"/srv/ws/01J8…","artifacts_dir":"/srv/ws/01J8…/artifacts","context":{"prior_review":[],"inputs":[]}}
```

stdout:

```json
{"type":"progress","msg":"editing README.md"}
{"type":"artifact","name":"readme.diff","path":"artifacts/readme.diff","kind":"diff"}
{"type":"delegate","tasks":[{"title":"double-check the wording","objective":"proofread the new README section","acceptance":[{"text":"a human approves","check":{"type":"human"}}],"role":"reviewer"}]}
{"type":"progress","msg":"running cargo test"}
{"type":"done","summary":"Added usage example to README","evidence":[{"criterion":0,"command":"cargo test","exit":0,"stdout_tail":"test result: ok. 3 passed"}]}
```

→ celeris: `WorkerProgress` ×2, `ArtifactProduced`, `Event::Delegated{run_id, task_ids}`（§4.6 の検証を通ればそれだけ）,
`WorkerFinished{outcome: done}`, `Transitioned{running→reviewing, reason:"worker_done"}`。

## 9. CLI エージェント系アダプタの結果ファイル規約（Phase 4、ADR-0006 で確定）

`claude-code`・`codex`・`acp`・`aider` は本文書 §1〜§8 の JSON Lines プロトコルを**話さない**。
`claude` CLI は独自の `stream-json` イベント（`system`/`assistant`/`user`/`result`）を吐くだけであり、
celeris はこれを直接パースできない。そこでこれらのアダプタはプロンプトでワーカー（Claude Code 自身）に
次を指示し、アダプタが成果物ディレクトリ（`request.artifacts_dir`。以下この節では `artifacts/` と書くが、
共有 workspace のタスクでは `.taskd/artifacts/<task_id>/`。ADR-0036 D2/D3。プロンプトにはその相対パスが
そのまま出る）の `result.json` を読んで本文書の `done`/`question` に
相当する終端を合成する（旧 P-13。ADR-0006 D3 で確定。旧 P-11 の `run_id`/`attempt` はスキーマ変更せず
プロンプト文面にのみ埋め込む。旧 P-12 の「evidence を任意化」は ADR-0012 D3 で採用し、`command` / `exit` /
`stdout_tail` を任意にした）:

```json
{"summary": "...", "evidence": []}
```
```json
{"question": "..."}
```

**`yield`（ADR-0072 D9/D10, Phase E1）**: `summary`/`question` の代わりに `yield` を書くと、
`Terminal::Yielded` になる（§4.5b と同じ意味）。予算の予告（§9 末尾）を見て、区切りのよい所で
自分から止まりたいときに使う。

```json
{"yield": {"completed": ["A を実装した"], "remaining": ["B のテスト"], "next_action": "B のテストを書く"}}
```

`yield` の中身は `<artifacts_dir>/checkpoint.json`（§4.5d）と同じ意味の欄（省略した欄は
`checkpoint.json` の最後の内容、または mechanical な事実で埋める。優先順位は `yield` >
`checkpoint.json` > mechanical。ADR-0072 D9）。`ResultFile` は寛容に読むが、celeris が見るのは
`question` > `wait` > `summary` > `yield` の優先順（複数書かれていたら `question` が勝つ）。

**`wait`（ADR-0090 D1）**: クラスタ job の終了を待つときは §4.8 と同じ欄を、最上位
（`{"type":"wait","kind":"cluster_job","jobs":[…],…}`）か入れ子（`{"summary":"…","wait":{"kind":"cluster_job","jobs":[…]}}`）
で書く。不正な wait は `error{retryable:true}` になる。

**置き場は必ず成果物ディレクトリの絶対パス（Phase 115、ADR-0006 追記）**: プロンプトの結果ファイル
指示（`result_json_instructions`）は `RunRequest::artifacts_rel()` を使うので、`work_dir`（§3.1）が
ある run（部署のリポジトリの `git worktree` で cwd がそこになる run）では常に絶対パスになる。
`work_dir != workspace` の run のプロンプト冒頭にも「cwd は `<work_dir>`。成果物ディレクトリは
`<artifacts_dir>`。相対 `artifacts/` はリポジトリの中を指すので使わない」の 2 行が出る
（`claude_code::work_dir_note`。`codex` も同じ関数を再利用）。それでもワーカーが cwd 相対の
`artifacts/result.json`（= `<work_dir>/artifacts/result.json`）に書いてしまった場合、アダプタは
run 終了時にそのファイルへフォールバックし、正しい置き場へ**移して**採用する（worktree の中には
残さない。`subprocess::adopt_result_json_written_under_work_dir`）。本番障害
01M3915FARENW8M0JM11XVF6W0 / 01M38T8N17MEWPTJQXGX1TNYJD の再発防止。

**`memory`（v4, ADR-0033 D6, Phase 24）**: 結果ファイルに `memory` があれば、celeris はその中身を担当ノードの
長期記憶に**日付付きの箇条書きで追記する**（`- 2026-09-17: …` の 1 項目 1 行）。

```json
{"summary": "...", "evidence": [], "memory": {"notes": ["pegasus は pjsub で投げる"], "project": ["Pluvio は非同期ランタイム基盤"]}}
```

- `notes[]` は `<memory_dir>/<node_id>/notes.md`（**案件をまたぐ**記憶: クラスタの使い方、人の好み、直近の相談）、
  `project[]` は `<memory_dir>/<node_id>/projects/<project_id>.md`（この案件だけの事）。
- `memory` が無い・空・形が違う・そもそも `[memory]` を設定していない・タスクに `assignee` が無いときは
  **何もしない**（run は失敗させない）。案件に属さない run の `project[]` は行き先が無いので捨てる。
- 追記は決定的なファイル操作だけで、何を覚えるかを決めるのはワーカー（**LLM に書かせるのはここだけ**）。
  プロンプトの前置きの末尾にその指示が入る。
- 検索ハーネス（`local-deep-research`）はこの前置きを出さないので、記憶の追記も起きない（ADR-0033 D6）。

**`milestone_proposal`（Phase 41, ADR-0038 D1）**: 途中目標レビューの対話 run（および `discuss` / `ng` で
起きた対話 run）が、**次の途中目標**を宣言できる（任意。`report` と同じ宣言的フィールド）。

```json
{"summary": "...", "evidence": [], "milestone_proposal": {"title": "候補の比較実験", "description": "3 本を同じ条件で比べる"}}
```

- celeris は `done` のときだけこれを読み、その案件に `status = proposed` の途中目標を 1 件作る（`seq` は末尾）。
  既にあった `proposed`（判定中の途中目標自身は除く）は `redesigned` にして**差し替える**（`proposed` は常に 1 件）。
- `title` が空・`milestone_proposal` が無い・形が違う・案件に属さない run では**何もしない**（run は失敗させない）。
- 達成にするのも、この提案を承認するのも**人**（`POST /milestones/{id}/decide` の `ok`）。celeris は行を作るだけ。

**`report`（v4 で追加、ADR-0034 D7, Phase 27）**: `done` の結果が「提案」なのか「ただの結果」なのかを
ワーカーが**自分で宣言**できる（任意。書かなければ従来どおり）。

```json
{"summary": "...", "evidence": [], "report": {"kind": "proposal"}}
```

- `kind` は `"result"` / `"proposal"` / `"bad_news"` / `"question"`。celeris は**固定表で写すだけ**で、
  判断はしない（協調判断に LLM を使わない。`docs/SPEC.md`）: `"proposal"` → 報告の `kind = proposal`、**それ以外・未知の値・欠落は
  `result`**。`bad_news` / `question` の報告は従来どおりタスクの終端遷移から作られる（ADR-0034 D2）ので、
  `done` を返しながら `"bad_news"` を名乗っても悪い知らせにはならない。
- 効くのは「レビューを通って `Status::Done` になった」ときの報告 1 件だけ。`assignee` の無いタスク・
  対話用タスクは報告を作らないので、宣言も無視される。
- `PROTOCOL_VERSION` は 4 のまま（追加のみで、既存のワーカーは何も変えなくてよい）。

判定順序（ADR-0006 D4）: stream-json の最後の `{"type":"result",...}` が `is_error:true` か
`subtype != "success"` なら、結果ファイルの内容によらず `error{retryable:true}` とする（自己申告の
`done` は信用しない）。`success` の場合のみ結果ファイルを読み、無い／不正なら `error{retryable:true}`。

`context.answers`（P-10、`question` → `blocked` → `celerisctl answer` の回答をワーカーへ渡す経路）は
Phase 7（ADR-0010 D3）で実装した。`claude-code`/`codex` のプロンプトは、空でなければ「以前の質問への
人間の回答」節（`## Answers from a human to your earlier questions`、各回答を `- Q: ...` / `  A: ...`）を
`prior_review` の節の近くに載せる（Execute/Plan プロンプトのみ。Review プロンプトには載せない）。

**`runs/<run_id>/result.json`（P-26, ADR-0010 D10）**: `claude-code`/`codex` も、run の終端（`done`/
`question`/`error`。供給側失敗として分類された `error` の場合は `provider_failure` 付き）を本文書 §4 の
`WorkerMessage` に正規化し、1 行 JSON として `runs/<run_id>/result.json` に書いてから終了する
（`fake`/`run_subprocess` がワーカーから受信した生の行をそのまま書くのと同じ役割）。これは celeris の
再起動後にレビュー対象の `done` 内容を復元するために使われる（ADR-0007 D5）ので、CLI 系アダプタも
同じファイルを同じ形式で書く必要がある。

**`artifacts/delegate.json`（ADR-0016 M8, v2）**: `claude-code`/`codex` は §4.6 の `delegate` メッセージの
プロトコルを話さないので、代わりに作業ディレクトリ直下 `artifacts/delegate.json` を使う。形式は
`{"tasks":[…]}`（`tasks` は §4.6 の `DelegateTask` と同じ形。v4 から `tasks[].assignee`（組織ノードの id）を
書ける。`role` を書かなければそのノードの分野から tier / アダプタ / 予算が決まる。**自分と別の部の課へ
委譲しようとした提案は子を作らず、親の run が秘書への `question` で終わる**。SPEC §3.1 / ADR-0033 D4。
Phase 27 から、同じバッチの**同じ部宛ての提案はその場で子になり**、部またぎの提案だけが
`approvals` の 1 行（`cross-department: <from> -> <to>: <理由>`）になる。人が「今回だけ」/「今後ずっと」で
認めた後に同じ提案を出せば、その run では子が作られる。ワーカーには
`WorkerProgress{msg:"delegated N child task(s)（M 件は秘書の認可待ち）: …"}` として見える）。run 開始時（`artifacts/result.json` を消す
のと同じタイミング）に前回の run が残したファイルを消し、run の終わり（終端を決めた直後、`result.json` を
書く前）に存在すれば読んで、§4.6 と同じ検証・挿入の経路に渡す。ファイルが無ければ何もしない。JSON として
読めない場合は run を失敗させず、`WorkerProgress{msg:"delegate.json ignored: <error>"}` を残して無視する。
プロンプトには役割の指示文（`## Role: <id>`）、`artifacts/delegate.json` の書き方の指示、集約 run
（`task.aggregate == true` で子が全て終端になった後の run）なら `## Delegated child tasks` 節（各子を
`title` / `role` / `status` / 直近 run の `outcome` / `workspace` / `artifacts` で列挙し、`artifacts/summary.md`
を書くよう指示）を足す（`task_worker::claude_code::build_prompt` / `codex::run_codex` が共通で使う）。

**エラー文面の分類（供給側失敗。ADR-0010 D5）**: `claude-code`/`codex` はワーカープロトコルの
`provider_failure` フィールドを直接受け取れない（stream-json/JSON Lines の形式が異なるため）。代わりに
エラー文面を決定的な文字列規則（`task_worker::provider::classify_provider_failure`。大文字小文字を無視した
部分一致、LLM を呼ばない）で分類し、`AdapterError::{Throttled, AuthFailed, Exhausted}` として返す
（`run()` は `result.json` を書いた後にこの `Err` を返す。遷移の判断はディスパッチャが行う）:

| 分類 | 判定順 | 一致パターン（部分一致・大小無視） |
|---|---|---|
| `exhausted` | 1 | `usage limit`, `quota`, `credit balance` |
| `throttled`（`retry_after_secs:60` 固定） | 2 | `rate limit`, `rate_limit`, `overloaded`、独立トークンの `429` / `529` |
| `auth_failed` | 3 | `invalid api key`, `authentication`, `not logged in`, `/login`、独立トークンの `401` |

「独立トークン」は前後の文字が英数字・`.` でなく、`:` を挟んで数字が続く位置情報（`:17`、`12:`）の一部でもないこと
（`HTTP 429`、`status=429`、`HTTP 529: too many requests` は一致し、stderr のスタックトレースに含まれる `cli.js:4291:17` や
`file.js:429:17` のような位置情報は一致しない）。プロトコルの `provider_failure.retry_after_secs` は最低 1 秒に切り上げる。

どれにも当たらなければ分類せず、従来どおり `Terminal::Error{retryable:true}`（例:
`The 'gpt-5.4' model is not supported when using Codex with a ChatGPT account.` は分類対象外）。
分類に使う文面は、`claude-code` は `result` メッセージの `result`（文字列。無ければ `subtype`）、
`codex` は `turn.failed.error` のメッセージ。どちらも「`result`/`turn.*` を一度も観測できずに exit した
場合」は代わりに `runs/<run_id>/stderr.log` の末尾（最大 4 KiB）を分類する（`codex` はさらに
`{"type":"error","message":...}` 行を観測していればそれを優先する）。**wall-clock・無出力タイムアウトは
分類しない**（供給側の問題ではなくワーカー側の停止・暴走のため）。

### 9.1 `codex` アダプタ（Phase 6、ADR-0008 D3）

`codex exec --json` も同じ結果ファイル規約（`artifacts/result.json`）を使うが、正常終了の判定に使う
JSON Lines のイベント形が `claude-code` と異なる: `claude-code` の `{"type":"result",...}` の代わりに
`{"type":"turn.completed",...}` / `{"type":"turn.failed","error":...}` を見る。`turn.completed` を一度でも
観測できれば結果ファイルを読み、`turn.failed` はそのまま `error{retryable:true}`（供給側失敗として分類
できればディスパッチャへの `requeue` 経路。上記参照）にする。
`turn.completed`/`turn.failed` のどちらも一度も観測できずに exit した場合はクラッシュとして扱い、
`artifacts/result.json` を一切信用しない（§9 の判定順序と同じ考え方）。`item.*`（`item.started`/
`item.completed` 等）は進捗としてのみ扱い、内容の構造には依存しない（実機の codex-cli 0.154.0 で
`turn.failed.error` がオブジェクト（`{"message":"..."}`）で返ることを確認済み。将来この形が変わっても
読めるよう文字列・オブジェクトの両方を受け付ける）。プロンプトは `claude-code` と共通（`build_prompt`
を再利用）。

## 10. kind 別の出力ファイル（Phase 5、ADR-0007 D1/D5/D7）

§1〜§9 の `run`/`done`/`error`/`question` プロトコル自体は kind によらず同じ（`task.kind` に応じて
`RunRequest.task`/`context` の内容が変わるだけで、メッセージ形式は変更しない）。ただし `Plan` run と
`Review` run では、ワーカーは終端メッセージ（あるいは CLI エージェント系アダプタなら §9 の
`artifacts/result.json`）に加えて、成果物ディレクトリ（`request.artifacts_dir`。共有 workspace では
`.taskd/artifacts/<task_id>/`。ADR-0036 D2）に追加のファイルを書く。ディスパッチャ側の
Reviewer（決定的コード。LLM 呼び出しはここには書かない）がそれを読んで判定する。

### 10.1 `Plan` run — `artifacts/plan.json`

`task.kind == "plan"` の run では、ワーカーは分解結果を作業ディレクトリ直下 `artifacts/plan.json` に
`PlanOutput` として書く（正の JSON Schema は隣の `plan-output.schema.json`。`task-core::plan::schema_value()`
から生成し `task-core` のテストで一致を検証する）。概形:

```json
{"tasks":[
  {"title":"...", "objective":"...",
   "acceptance":[{"text":"...", "check":{"type":"command","cmd":"...","expect_exit":0}}],
   "depends_on":[0],
   "kind":"execute",
   "tier":"standard"}
]}
```

検証規則の要約（`task-core::plan::validate`、全て決定的）:

- `tasks` は 1〜20 件（既定の上限）
- 各 `title`/`objective` は空でない。`acceptance` は 1 件以上、各 `text` も空でない
- `check` は `command` / `artifact_exists` / `reviewer` / `human` のいずれか（`task-core::Check` の 4 種）
- `depends_on` は同じ `tasks` 配列内のインデックスで、範囲内・自己参照無し・DAG（閉路無し）
- `kind:"plan"` の子は、その Plan 自身を含む祖先 `Plan` の数（`plan_depth`）が `MAX_PLAN_DEPTH`（3）を
  超えない場合のみ許される
- 未知フィールドは拒否（`#[serde(deny_unknown_fields)]`。綴り間違いの検出のため。§2 の「未知フィールドは
  無視する」という一般規則とは意図的に逆）
- `workspace`（任意。Phase 43 / ADR-0039 D2）: その子の作業場所。`{"kind":"local","path":"..."}` か
  `{"kind":"remote","cluster":"<[[clusters]] の id>","path":"<クラスタ側のパス>"}`。**省略時は案件の
  作業場所 → 親の workspace** を継ぐので、別のリポジトリ・別のクラスタで作業させたい子にだけ書く
  （`local` の `~` は celeris の `$HOME` で展開される）。案件が作業場所を持つ計画 run のプロンプトには
  「## 子タスクの作業場所」節が出る
- `repos`（任意。Phase 52 / ADR-0043 D2）: その子が使う案件のリポジトリを**名前で**並べた配列
  （`["benchfs", "benchfs-paper"]`）。名前は計画 run の前置きに出る「この案件のリポジトリ」の `name`。
  **省略時は 親 → 案件の primary** を継ぐ。`repos[0]` がその子のワーカーのカレントディレクトリになる。
  **前置きに無い名前を書くと計画は差し戻される**（他の検証の失敗と同じ扱い。エラー文言は
  `tasks[0].repos[1] = "nope" is not one of this project's repositories (benchfs, benchfs-paper)`）

検証に失敗した場合、`Plan` タスクの `reviewing` は `ReviewFail` になり（`criterion_idx =
task.acceptance.len()`。`celerisctl plan` が作る Plan は `acceptance = []` なので常に `criterion 0`）、
次回の `context.prior_review[criterion_idx].reason` に検証エラー文言（例:
`tasks[2].depends_on[0] = 7 is out of range (0..3)`）が載ってリトライされる（`max_retries` 内）。全 pass なら子タスクの挿入と親のトランザクションが原子的に行われる
（`TaskStore::complete_plan`。ADR-0007 D3）。

### 10.2 `Review` run — `artifacts/review.json`

`Check::Reviewer` 条件を判定する際、ディスパッチャは対象タスクとは別の run を起動する。
`RunRequest.task` は永続化されない合成タスク（`kind = "review"`、`title = "Review: <対象 title>"`、
`objective`/`acceptance`/`workspace`/`budget` は対象タスクと同じ）。`RunRequest.context.review` に
`ReviewRequest{summary, evidence, criteria, decisions?, answers?, checks?}` が入る（`criteria` は判定すべき `task.acceptance` の
インデックス一覧、`summary`/`evidence` は対象 run の `done` の内容、`decisions` / `answers` は人の決定と回答、
`checks` は daemon が走らせた決定的な検査の結果）。`context.inputs` には対象 run の
`ArtifactProduced` が入る。run は対象タスクと同じ作業ディレクトリで実行される（成果物を直接読めるように
するため。ワーカーには読み取り専用で振る舞うよう指示するが強制はしない。既知の制約）。

ワーカーは判定結果を作業ディレクトリ直下 `artifacts/review.json` に `ReviewOutput` として書く
（`worker-protocol.schema.json` の `review_output` 定義。正）:

```json
{"verdicts":[{"criterion":0,"pass":true,"reason":"cargo test passes and README was updated"}]}
```

不合格の verdict には局所修復のヒント `repair: {scope, class, hint?}` を任意で添えられる（`scope = "local"`
だけが認識され、`class` は `format` / `lint` / `test` / `doc` / `other`。ADR-0072 D16）。

`criteria` に列挙された各インデックスについて `verdicts` に必ず 1 件対応するエントリが必要。次はすべて
該当する `Reviewer` 条件を `pass=false`（理由に原因を記録）として扱う: run の終端が `done` 以外
（`error`/`question`/クラッシュ/タイムアウト）、`artifacts/review.json` が無い、JSON として不正、
`criteria` のいずれかのインデックスに対応する `verdicts` エントリが欠落している。

### 10.3 run 開始前のクリーンアップ

リトライで前回 run の出力ファイルを今回の結果と誤読しないよう、ディスパッチャは run 開始前に
（§9 の `artifacts/result.json` と同様に）該当ファイルを削除してから起動する: `Plan` run なら
`artifacts/plan.json`、`Review` run なら `artifacts/review.json`。

### 10.4 例: `fake` アダプタでの kind 分岐

`fake` アダプタ（sh スクリプト。§1〜§8 の JSON Lines を stdin/stdout で読み書きする）は、stdin に来る
`run` 行の `"task":{"kind":"plan", ...}` / `"kind":"review"` を見て分岐できる。例（`sh`, `jq` 前提）:

```sh
#!/bin/sh
line=$(cat)
kind=$(printf '%s' "$line" | jq -r '.task.kind')
case "$kind" in
  plan)
    mkdir -p artifacts
    printf '%s' '{"tasks":[{"title":"a","objective":"do a","acceptance":[{"text":"c","check":{"type":"command","cmd":"true","expect_exit":0}}]}]}' \
      > artifacts/plan.json
    ;;
  review)
    mkdir -p artifacts
    printf '%s' '{"verdicts":[{"criterion":0,"pass":true,"reason":"looks fine"}]}' > artifacts/review.json
    ;;
esac
echo '{"type":"done","summary":"ok","evidence":[]}'
```

（`claude-code` アダプタでの kind 別プロンプトは §9 と同じ「翻訳層」であり、`task-worker::claude_code::
build_prompt` が `task.kind` で分岐する。ADR-0007 D7）

### Adapter の context compaction 観測（ADR-0079 2026-10-04 付記）

`EventSink::context_compacted()` は完了した圧縮・context rollover 1 回を報告する adapter 内部の口。
既存 progress の `kind: "status"`, `tool: "context_compaction"` として記録する。
Claude Code は stream-json の `type: "system", subtype: "compact_boundary"` を使い、
`status: "compacting"`（開始中）やモデルの本文の「compaction」は数えない。
深さ上限の自動 leaf は回数超過で adapter を中断し checkpoint を保存して人の決定を待つ。
この印が取れない adapter は `BudgetExhausted { kind: context }` と session rollover で代用する。
session rollover は dispatcher が `kind: "status", tool: "context_rollover"` で記録する。これは前の run の context を閉じた印なので、新しい run の context 消費と区別して数える。
