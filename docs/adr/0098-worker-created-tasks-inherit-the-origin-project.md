# ADR-0098: worker の run が作る task は、その run の task の案件とリポジトリを継ぐ

- 日付: 2026-10-01
- 状態: **Accepted（Phase R7-10 で実装）**。人の決定「案件の worker が作った task はその案件に結び付ける」。
- 関連: [ADR-0033](0033-organization-projects-and-reports.md) D2（task は案件に属する）、
  [ADR-0043](0043-workspaces.md) D2（`repos` は 明示 > 親 > 案件の primary）、
  [ADR-0095](0095-worker-runs-see-the-db-read-only.md)（worker の run から DB は読み取り専用、celerisctl は migration をしない）、
  [ADR-0016](0016-roles-and-delegation.md) D2 / M8（`delegate.json`）、[ADR-0069](0069-routing-four-layers.md) D1（LLM の書いた担当・tier）、
  [ADR-0044](0044-task-management.md) D1（`PATCH /tasks/{id}`、人が Go を出す）
- 番号: main の `docs/adr/` は 0095 まで、0096 は別ブランチ（web-parallel-operation）で使用済み、0097 は並行中の R7-9 に空ける。

## 文脈

task 01M3SPF94RDWTPWHNDEQD68VB9（案件 01M2WTS3DKNZBSZ2JMVB4CZMBW、repo `agent-platform`）の run が
`celerisctl add --db /var/lib/celeris/celeris.sqlite3 …` で後続 task（01M3SPN8HP…、01M3SPN8H05…、01M3SPN8HE6A…、後の run で 01M3T7FRCW…）を
作った。`celerisctl add` は `project_id` を書かない（ADR-0033 D2「案件は GUI から付ける」）ので、どれも案件・リポジトリ無しになり、
planner が「子の repos が親の `[]` の部分集合でない」で落ちた。後から案件を付ける API も無い。

R7-6（ADR-0095）以降、run の中から本番 DB には書けない（`celerisctl add --db` は `SQLITE_READONLY`）。worker が後続を起票する口は
現在どこにも無い。

### worker の run から task が作られる経路（調査、HEAD 291f1701）

| 経路 | 現状 | 案件・リポジトリ |
|---|---|---|
| 委譲 `artifacts/delegate.json`（`WorkerMessage::Delegate`）→ `StoreSink::delegate_impl` → `task_core::delegate` | 動く。子は親の完了を止める | **継ぐ**（`delegate.rs` の `project_id: parent.project_id`、`child_repos` = 明示 > 親 > primary） |
| 木の子 task（plan/3 の kind task の unit） | 動く（daemon が作る） | 継ぐ（ADR-0079） |
| 人の承認の子（`create_human_approval_child`） | 動く | 継ぐ（`project_id: task.project_id`） |
| CoS の対話 run の `actions.create_task` | 秘書（`OrgKind::Secretary`）の対話 run だけ | `project` は明示。CoS は案件をまたぐ 1 本の対話（`node_sessions.project_id = None`）で、人の依頼の代筆 |
| `celerisctl add --db <本番>` | **R7-6 以降は失敗**（EROFS / `SQLITE_READONLY`） | 継がない（今回の事故） |
| HTTP API `POST /tasks` | run は admin token を受け取らない（`token_file`） | 人の経路 |
| MCP（`celeris-mcp`） | task を作る tool は無い（`task_retry` は複製で案件を保つ）。run には MCP server を渡していない（acp は `mcpServers: []`） | — |

つまり「親の完了を止めない、独立した後続 task」を worker が作る正規の口が無い。

## 決定

### D1. 後続 task は run が宣言し、daemon が作る（`<artifacts_dir>/followups.json`）

- run は自分の成果物ディレクトリ（`RunRequest.artifacts_dir`。WU の run は WU の置き場）に
  `{"tasks": [<NewTaskSpec>, …]}` を書く。各要素は `POST /tasks` の本文（`task_ops::add::NewTaskSpec`、`deny_unknown_fields`）と同じ形。
- daemon（worker の run を起こした future、`task-dispatch::dispatcher::worker_task`）は run が返った直後に
  （終わり方に依らず。`absorb_memory` と同じ）このファイルを読み、**その run がまだ task の lease を持っているときだけ**
  （`run_holds_lease` と同じ規則。v2 の工程の lease を含む）1 件ずつ作る。読んだファイルは
  `followups.<run_id>.applied.json` に改名する（記録として残し、二度目の適用を防ぐ）。run の開始時に古い `followups.json` は消す
  （`delegate.json` / `plan.json` と同じ扱い）。
- 1 件の失敗（検証エラー）は他を止めず、run も失敗させない。理由は元の task の `WorkerProgress` に残す（委譲と同じ）。
  1 run が作れるのは先頭の 20 件まで（`MAX_FOLLOWUPS_PER_RUN`。残りは作らずに 1 行）。
- 対象は worker の run だけ。対話 run（CoS は `actions`）と planner run（計画が出力）は読まず、env も渡さない。
- 終わり方に依らない理由: 後続は「見つけた次の仕事」で、この run の成否とは独立。retry で同じものが再宣言される重複は D4 で潰す。

### D2. 出自は daemon が決める（クライアントを信用しない）

元の task X と run R は、**daemon が自分で R に割り当てた成果物ディレクトリ**から決まる。ファイルの中に task id / project id を
書かせて信じることはしない。run に渡す env（D6）は celerisctl が書き先を知るためだけのもので、daemon は読まない。

### D3. 案件・リポジトリの継承規則（X の案件を P、X の repos を Rs とする）

1. `project_id`: 省略 → **P**（X が案件に属さなければ無し）。**P と違う値を書いたら、その 1 件を拒否する**
   （X が案件に属さないのに案件を書いた場合も拒否）。
   - 理由: 案件は報告・リポジトリ・作業場所・担当の単位（ADR-0033 D2 / ADR-0043）。案件 P の worker が別の案件 Q に仕事を
     書き込めると、Q の持ち主（人）の Go を経ずに Q の木と worktree を動かせる。人の決定は「案件の worker が作った task は
     その案件に結び付ける」で、例外を作る理由が無い。別案件に頼みたいことは結果に書き、人か CoS が起票する。
     案件の無い task の worker も案件を選べない（人が D7 の PATCH で付ける）。
2. `repos`: 明示 → P の中で名前解決（`resolve_task_repos`。知らない名前は拒否）。省略 → **Rs の名前**（P の中で解決できる
   ものだけ）、Rs が空（または解決できない）なら **P の primary**（ADR-0043 D2 の「明示 > 親 > primary」の「親」を X に読み替える）。
3. `milestone_id`: P のものだけ（既存の `validate_project_refs`）。
4. 使わない欄（落として進行に 1 行残す）: `parent`（子を作るなら `delegate.json`。後続は X の子ではない独立の task）、
   `assignee`（ADR-0069 D1）、`adapter`、`workspace` / `cluster` / `workspace_mode`（作業場所は案件のリポジトリから決まる）。
5. `status` は常に `draft`（書いた値は無視して 1 行残す）。後続は X の承認済みの範囲の外の新しい仕事なので、人が Go を出す
   （ADR-0044 D1）。委譲の子（X の範囲内）とは違う。
6. `tier` / `execution` / `pause_after` は `provenance.origin = Agent`（ヒント扱い。ADR-0069 D1 / ADR-0072 D13 / ADR-0074 D2.1）。

### D4. 重複の抑止

同じ案件（X が案件に属さなければ案件無しの task の中）に**終端でない同じ題名の task**が既にあれば作らない（進行に 1 行）。
retry・review 差し戻しの再 run が同じ後続を再宣言しても 1 件のまま。

### D5. 出自の記録

- 作った task の `Event::Created.origin` = `{"worker_run": {"task_id": X, "run_id": R}}`（`CreatedOrigin::WorkerRun`。
  既存の `"plan_unit"` と同じ欄。migration は無い）。
- X の events に `WorkerProgress`「follow-up created: <新しい id> <題名>」を残す（X の timeline から辿れる）。

### D6. run の中で daemon の DB に向けた `celerisctl add` は後続の宣言になる

- daemon は worker の run に env `CELERIS_TASK_ID` / `CELERIS_RUN_ID` / `CELERIS_FOLLOWUPS_FILE`（= `<artifacts_dir>/followups.json`）と、
  ADR-0095 の `db_guard` が入っていれば `CELERIS_RUN_DB`（守っている DB のパス）を渡す
  （`with_env` を持つ adapter: claude-code / codex / acp / aider。コンテナは同じパスで mount されるのでそのまま使える）。
- celerisctl の `--db` の既定は `--db` > `CELERIS_DB` > **`CELERIS_RUN_DB`** > `./celeris.sqlite3`（run の中で `celerisctl ls` / `show` が
  `--db` 無しで本番を読める）。
- `celerisctl add` は、`CELERIS_FOLLOWUPS_FILE` と `CELERIS_RUN_DB` があり、**書き先の DB（上の解決の結果、canonicalize して比較）が
  `CELERIS_RUN_DB` と同じ**ときだけ、DB を開かずに spec をそのファイルに追記して「queued follow-up #n …（run の終わりに celeris が作る。
  案件は run の task のもの）」と出す。事故の形（`--db /var/lib/celeris/celeris.sqlite3` の明示）も `--db` 無しも、この扱いになる。
  - 比較を入れる理由: worker が run の中で `cargo nextest` を回すと、試験が一時 DB（`--db <tmp>`）に `celerisctl add` を打つ。env だけで
    判断すると、それが本番の後続の宣言に化ける。一時 DB に向けた `add` は従来どおりその DB に書く。
  - `--parent` / `--workspace` / `--cluster` はこのモードではエラー（D3-4 で落ちるものを早く言う）。`--role` / `--genre` は名前のまま渡し、
    daemon が自分の `[[roles]]` / `[[genres]]` で解決する（`--config` は読まない）。
- 環境変数が無い（人が run の外で打つ）ときは従来どおり DB に書く。`[db] worker_read_only = false`（ADR-0095 D5 の opt-out）では
  `CELERIS_RUN_DB` を渡さないので、run の中の `add --db <本番>` は従来どおり直接書く（継承しない。opt-out の既知の制限）。
- 読み取り専用の書き込み失敗の案内（ADR-0095 D6 の `READ_ONLY_HINT`）に「run の中では `--db` 無しの `celerisctl add`」を足す。
- worker への指示文（`claude_code::prompt` の委譲の段落の後）に「独立した後続は `followups.json` か `celerisctl add`」を 1 段落足す。
  対話 run（CoS）には出さない（`actions` で作る）。

### D7. 案件の無い task に後から案件を付ける（`PATCH /tasks/{id}` の `project_id`）

- `TaskEdit.project_id`。受け付けるのは: task が**案件を持たない**、`parent_id` が無い（子は親に従う）、`draft` / `ready`、lease 無し、
  `attempts == 0`、`WorkerStarted` の event が無い（まだ一度も run していない）とき。それ以外は 409 / 422。
- 案件を変える・外す（既に持つ task）は 422（task の木・worktree・報告が案件に結び付いた後で動かさない。やり直すなら retry/新規）。
- 同じ PATCH に `repos` が無ければ案件の primary を付ける（ADR-0043 D2）。primary がリモートのリポジトリなら 422
  （作業場所の種類が変わるので作り直す）。`milestone_id` は同じ PATCH で付けてよい（案件を付けた後で検証）。

### D8. 変えないもの

- 人の経路: `POST /tasks`（admin token）、run の外で人が打つ `celerisctl add`、GUI。
- CoS の `actions.create_task`（人の依頼の代筆。案件は明示）、MCP、委譲・木・承認の子（既に継ぐ。回帰試験だけ足す）。

## 残る穴（受け入れる）

- **別の task の成果物ディレクトリに書く**: worker は同じ uid で、`workspaces/` は書ける（ADR-0095 D1 で意図的に残した）。
  task Y の run が走っている間に Y の `followups.json` を書けば、Y の案件で作られる。Y の worktree そのものを書き換えられるのと同じ
  信頼の水準で、D2 の「どの run のファイルか」は daemon が決めている。
- **admin token を持つ worker**: HTTP API で人として作れる（ADR-0095 の「残る穴」と同じ）。
- **remote（クラスタ）の run**: 成果物は手元の写しに戻るが、クラスタには celerisctl も env も無い。手で `followups.json` を書けば拾う。

## 却下した案

- **celerisctl に HTTP API モードを足し、run ごとの token で認証する**: token の発行・失効・保存（run の寿命との同期）と、API に
  「run の主体」を足す変更が要る。daemon は既に run と成果物ディレクトリの対応を持っているので、ファイルで十分に server 側で強制できる。
- **env の `CELERIS_TASK_ID` を API に送らせて信じる**: クライアントの自己申告で、偽装できる（D2）。
- **MCP に `create_task` を足す**: MCP は外部のエージェント（人の道具）向けで、run には渡していない。
- **後続を X の子にする（委譲）**: 子は X の完了を止める（ADR-0016 D2）。後続は X の後に人が Go を出す独立の仕事。
- **別案件の指定を許す**: D3-1 の理由。

## 昇格時に人がすること

- 設定の変更は不要。昇格後、案件に属す task の run で `celerisctl add --title … --objective … --accept …` が
  「queued follow-up」を出し、run の後に `draft` の task が同じ案件・リポジトリで作られ、`celerisctl show <id>` の events の
  `created` に `worker_run` が載ることを確かめられる。
- 既存の案件の無い後続（01M3SPN8H05… など）は全て終端なので救済は不要。今後同じものができたら
  `PATCH /tasks/{id}` に `{"project_id": "<id>"}`（D7）。
