# ADR 2026-10-07: CoS 実機確認 live2 の不具合 D1〜D4 を直す（作成時 pin・KB 候補 operation・triage の confidence・終端 run の takeover 除外）

- 日付: 2026-10-07
- 状態: 承認済み・未実装（task 01M4APB5FP8T3TAAE51Z20E3M1）
- 関連: ADR 2026-10-05-cos-chat-home D2（chat API）/ D3（CoS credential と監査付き操作）/ D4（添付の pin）、
  ADR 2026-10-06-cos-inbox-triage（受信箱の一次対応）、ADR 2026-10-06-cos-chat-run-dispatch（CoS run の起動）、
  ADR 2026-10-03-ownerless-running-runs（lease を持たない running run の回収）、ADR-0047 D1（KB の scope）、
  ADR-0095 付記 D-d（本番の操作は人）

## 1. 文脈

live-check2（`agent-docs/progress/2026-10-05-cos-chat-home/live-check.md` の「実機で見つかった点 D1〜D4」「提案（live-check2）」。
branch tree `5664ef5fb20e`、2026-10-07）で、(d) screenshot の task への引き継ぎと (e) PDF の KB 取り込みが FAIL だった。
運用セッションの判定は次の 4 点。

- **D1**: skill cos-operator §3a の「起票してから pin」は dispatcher と競合する。CoS が `/cos/operations` で起票した task は
  作成の transaction で ready になり、pin の operation が届く前（実機で約 4 秒前）に worker run が始まった。
  最初の run の `prompt.txt` に「## 入力の添付」が無く、stage も空だった。
- **D2**: CoS が監査つきで KB 候補を作る経路が無い。`celerisctl knowledge record` は CoS credential を拒否し、
  `POST /api/v1/knowledge/inbox` はそもそも無く、`/cos/operations` の許可表 `ALLOWED` にも無い（422 `cos_operation_not_allowed`）。
  scope の書式も skill（`projects/agent-platform`）と CLI（`project:agent-platform`）で違う。
- **D3**: skill cos-inbox-triage §4・§5 は `confidence` を必須とするが、`crates/task-api/src/cos/inbox.rs` の `ResolveBody`
  （`deny_unknown_fields`）に欄が無い。実機の triage は confidence を `reason` の文に書いて回避した。
- **D4**: triage thread で、終わった run が `orphan takeover; continuing in a new run` で interrupted にされ、
  続きの run が重複して起きた（`crates/task-dispatch/src/dispatcher/cos_chat/control.rs` の takeover 判定）。

## 2. 決定

### D1. task 作成時に `attachment_ids` を受け、作成と pin を同じ transaction で行う

- `POST /api/v1/tasks` の本文に任意の `attachment_ids: [<chat 添付 id>]` を足す（省略・空配列は従来どおり）。
  CoS operation `task.create`（`/cos/operations` に包んだ同じ request）も同じ欄を受ける。
- **task の行の作成・ready への遷移と、各添付の `chat_attachment_refs`（`owner_kind: "task"`）の pin は 1 つの
  transaction**で書く。commit されるまで task は dispatcher から見えないので、起票と pin の間に run は始まらない。
  最初の run の入力 manifest（stage と前置きの「## 入力の添付」。ADR cos-chat-home D4 / attach-handoff）に添付が載る。
- 検証（添付が存在しない、削除済み、同じ id の重複、件数上限超過、CoS 経由なら credential の thread に属さない添付）に
  1 件でも失敗したら 422（`invalid_attachment` の系）を返し、**task も作らない**。部分的な pin は残さない。
- 監査: CoS 経由では `cos_operations` の 1 行（action `task.create`）の `result` に task id と pin した添付 id を載せ、
  pin ごとの event（既存の `attachment.reference` と同じ形）も同じ transaction で積む。人の API 呼び出しでも pin の event は積む。
- 既存の `POST /api/v1/chat/attachments/{id}/references`（action `attachment.reference`）は残す（既存の owner への後からの pin、
  KB 候補への pin）。task への添付の引き渡しは作成時の `attachment_ids` を正規の経路にする。
- skill `config/skills/cos-operator/SKILL.md` §3a を新しい形に直す: 画像を task へ渡すときは **起票の request に
  `attachment_ids` を入れる 1 回の operation**にし、「起票してから pin」の手順を消す。応答の確かめ方（operation が
  `applied`、`result` に task id と添付 id）と、§3 の操作例に POST /tasks の最小例（live2 で 422 が 4 回出た欄名の違いを避ける）を置く。

却下した案: (a) draft で作って pin してから ready にする — operation が 3 つになり、途中で落ちると draft が残る。
(b) run の開始時に pin を stage し直す — 最初の run は pin の前に始まりうるので競合が消えない。

### D2. `POST /api/v1/knowledge/inbox`（KB 候補の作成）を足し、CoS operation `knowledge.record` として登録する

- `POST /api/v1/knowledge/inbox` を足す。本文は `title`・`scope`・`body`（Markdown）・`sources[]`（`message:<id>` /
  `task:<id>` …）・任意の `tags[]`・`confidence`・`path`・`attachment_ids[]`。KB の `_inbox/<id>.md` に候補を書き、
  201 で候補 id（`GET /api/v1/knowledge/inbox/{id}` と同じ id）を返す。書き込みは `celerisctl knowledge record` と同じ
  task-core の関数を使う（候補ファイルの形・id の規則を 1 つに保つ）。
- `attachment_ids` があれば、候補ファイルの作成と `chat_attachment_refs`（`owner_kind: "knowledge_inbox"`）の pin を
  同じ処理で行い、どちらかが失敗したら候補ファイルも残さない（順序は付記 D2-a）。
  `GET /knowledge/inbox/{id}` の provenance（attach-handoff の kb-provenance）にその添付が出る。
- `crates/task-api/src/cos/operations.rs` の `ALLOWED` に `("POST", "/api/v1/knowledge/inbox", "knowledge.record")` を登録する。
  CoS からは `/cos/operations` に包んだ時だけ通り、`cos_operations` の行と監査 event が付く（直接叩けば
  422 `cos_audit_context_required`、他の operation と同じ）。人の管理系 token でも呼べる。
- `celerisctl knowledge record` は、CoS run credential（`celeris-cos-run.<credential>`）のときは KB を直接書かず、
  この API を `/cos/operations` 経由で呼ぶ（`reason` は `--reason`、無ければ既定の文、`idempotency_key` は本文と title の
  hash）。CoS credential で KB の根（`CELERIS_KNOWLEDGE_ROOT` 等）へ直接書く経路は作らない（live-check の「新たに見つけた点 6」
  の穴を開けない）。CoS でない worker の `record` は従来どおり（直書き）。
- **scope の書式は `project:<slug>` に 1 つに決める**（`user` / `environment` / `experience` / `project:<slug>`。
  ADR-0047 D1 と task-core `knowledge/layout.rs` の正規形）。`projects/<slug>` は KB の置き場（ディレクトリ）の名前で、
  scope の値としては書かない。API と CLI は互換のため `projects/<slug>` も受けて `project:<slug>` に正規化して記録するが、
  skill（cos-operator §3・§3a）・文書・例は `project:<slug>` だけを書く。理由: 候補の front matter・検索の `--scope`・
  layout の検証がすでにこの形を正としており、置き場の名前は accept 時に scope から決まる。

### D3. `ResolveBody` に任意の `confidence`（0..=1）を足し、記録する

- `crates/task-api/src/cos/inbox.rs` の `ResolveBody` に `confidence: Option<f64>`（`#[serde(default)]`）を足す。
  範囲外（0 未満・1 超）・非有限値は 422。`deny_unknown_fields` は保つ。
- 値は `cos_operations` の行（resolve の operation の request / result の JSON）と、resolve の event に残す。
  migration は足さない（既存の JSON 列に入れる）。
- 欠落は「低確信」と扱う（skill §4 のとおり、answer に `min_confidence` 未満または欠落なら escalate を求める判定は skill 側の
  規則のままで、API は欠落を拒否しない。古い worker の本文を壊さないため）。
- skill `config/skills/cos-inbox-triage/SKILL.md` §5 の本文例（`"confidence":0.97`）とこの形を一致させる。

### D4. 終端済みの CoS chat / triage run を orphan takeover の対象から外す

- `cos_chat/control.rs` の takeover は、**store 上の run の状態が終端（completed / failed / stopped / interrupted）なら
  何もしない**。thread の `active_run_id` が終端の run を指していても、続きの message（`cos-recover:<run>`）を積まない。
- 判定と回収を分けない: 回収（続きの message の投入と run の interrupted 化）は、run が非終端であることを確かめる
  **同じ store transaction**の中で行う。いまは `chat_message_post` の後に `chat_run_finish` を呼ぶので、run が
  その間に終端になっても続きの message が先に入り、run が重複する。終端なら transaction ごと何もしない。
- 順序: run の process を抱える側は、**終端の記録（`chat_run_finish`）を commit してから** in-process の handle・
  account / provider の枠・lease を手放す。handle が終わったのに終端が未記録の run（記録の途中・記録の失敗）は、
  この daemon instance が持ち主である限り orphan とみなさず、同じ instance の sink が記録するのを待つ
  （`running` から handle を外すのは終端の記録の後。triage の run も同じ扱い）。takeover は持ち主の instance が
  居なくなった run（ADR 2026-10-03-ownerless-running-runs の判定）だけにする。
- 実装の葉は、終端記録と handle の解放の間に試験専用の遅延フックを置くか `tokio::time::pause` で、決定的に再現する試験
  （`cos_live_fix_d4_*`）を足し、重複 run が起きないことを確かめる。

### D5. live 台本を `scripts/dev/cos-chat-live.sh` として repo に入れる

- live-check2 の WU 成果物 `live2-ops.sh` を `scripts/dev/cos-chat-live.sh` として入れる。
  引数は `<repo> <bin dir> <data dir> dry|full`。`dry` は LLM を呼ばずに設定・起動・API の疎通・fixture の投入までを確かめ、
  `full` は claude_oauth の既存ログインで (a)〜(e) を流す（試験・CI からは呼ばない。外部への通信は LLM 呼び出しだけ）。
- 本番の設定・DB・KB を読まない・書かない: `<data dir>` に専用の config・DB・KB を作り、`CELERIS_CONFIG` 等を明示し、
  本番の port を使わない。本番 host の操作（systemd・releases）はしない。
- turn の `run_id` は `POST …/messages` の応答に頼らず（queued では `null`）、`GET /chat/threads/{t}/messages` を
  `client_message_id` で引いて待つ。待ちは出来事待ち（応答の状態）で、上限つきの保険だけ時間で持つ。
- provider の `concurrency = 4`（1 では CoS の turn が task worker と受信箱の triage の後ろで止まった。attempt 2 の知見）。
- (d) は起票の request の `attachment_ids`（D1）で渡し、最初の run の `prompt.txt` の「## 入力の添付」と stage を確かめる。
  (e) は `POST /api/v1/knowledge/inbox`（D2）の operation で候補を作り、`GET /knowledge/inbox/{id}` の provenance を確かめる。
- 証拠は `<data dir>/evidence/`（`steps.log`・`db.txt`・`kb-inbox*.json`・`prompt-files.txt`・`staged.txt`）。
  `docs/ops/cos-chat.md` の再実行手順はこの script の呼び方に直す。

## 3. 帰結

- CoS の添付の引き渡しは、task へは作成時の 1 operation、KB へは候補作成の 1 operation になり、どちらも監査が付く。
- `celerisctl knowledge record` は呼び手によって書き込みの経路が変わる（CoS は API、それ以外は直書き）。
- `ResolveBody` に欄が増えるので API schema（`UPDATE_SCHEMA=1`）と gui・web の生成型を再生成する。
- 試験は決定的（偽ハーネス・fixture・`tokio::time::pause`・試験専用フック）で、名前の接頭辞は `cos_live_fix_d1_`〜`_d4_`。
- `docs/api/v1/gui-api.md` の CoS の節（§3.128〜§3.130）に D1〜D3 の欄と route を足す。

## 付記 D2-a（2026-10-07、d2-kb-inbox の実装）

- 当初は「候補を一時名で書き、DB の commit の後に rename する」としたが、そのためには `crates/task-ops` の
  `record_in` を書き込みと commit に割る必要があり、葉の範囲（task-api・celerisctl・task-core …）の外になる。
- 実装は `task_ops::knowledge::record_in`（`celerisctl knowledge record` と同じ関数。候補の形・id の規則は 1 つのまま）で
  `_inbox/<id>.md` を書いて git commit し、その後に pin（と CoS の監査）を SQLite に commit する。pin か監査が
  落ちれば `inbox_reject` で候補を消す commit をする。「どちらかが失敗したら候補ファイルを残さない」は保つ
  （git の履歴には書いて消した 2 commit が残る）。
- 残る隙: 候補の commit と pin の commit の間に process が落ちると、pin の無い候補が `_inbox` に残る。人の受信箱で
  見える（provenance が空）ので、捨てるか取り込むかを人が決める。
