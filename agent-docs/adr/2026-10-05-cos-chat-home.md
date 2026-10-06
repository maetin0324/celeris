# ADR 2026-10-05: CoS チャットをホームにし、スレッド継続・全道具・受信箱の一次対応を担う特別ワーカーにする

---
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV]
---

- 日付: 2026-10-05（2026-10-06 付の人の追加要望を含む）
- 状態: **設計確定・人の確認待ち、未実装**。人が既に決めた方針を以下の契約に具体化した。design 段の確認は Fable が行い、その後に実装する。**2026-10-06 付記（cos-run 完了）: cos-run 担当の実装（`[cos]` config・CoS 特別 worker としての chat run 起動/継続/停止/queue/SSE・3 harness（claude-code/codex/opencode）の継続と全道具・画像入力の写像・受信箱一次対応と Discord escalation・CoS 不在退避・通知一本化・代答の取消/差し戻し・旧 Console/MCP 入力の legacy facade・`/cos/operations` 監査付き操作層・run credential）は実装済みで、統合後 HEAD `5434785b` で `bash scripts/dev/test-parallel.sh`（4230 passed）・`cargo clippy --workspace -- -D warnings`・文書検査 4 本が exit 0 を確認済み（`agent-docs/progress/2026-10-06-cos-run.md`）。D6 表の cos-run 担当行の `cos_chat_` 試験対応表（130 件）と、実機 1 回（live-check）・replan/pause/resume の allowlist 登録・添付→task manifest と KB provenance の連結を未解決として記録する。**
- 対象: web のホーム、CoS の実行、会話・添付の永続化、受信箱一次対応、外向け通知。実装・実機確認の完了をこの ADR の作成で代替しない。
- 関連: [ADR-0033 D4](0033-organization-projects-and-reports.md)、[ADR-0048](0048-console.md)、[ADR-0054](0054-stateful-sessions-and-streaming-chat.md)、[ADR-0089](0089-cos-runs-bypass-concurrency.md)、[ADR-0140](0140-claude-session-resume.md)、[ADR-0132](0132-provider-llm-source-split-and-cheap-qwen.md)、[ADR-0133](0133-inbox-and-notifications.md)、[ADR-0037](0037-discord-notifications.md)、[ADR-0050](0050-request-completion-and-notifications.md)。

人の決定は、CoS を全道具・全権限を持つ特別ワーカーにし、会話ごとにセッションを継続すること、config で harness・LLM source・model/tier・account を指定すること（既定 claude-code）、ホームをチャット中心にすること、画像・任意ファイルを添付できること。さらに受信箱は CoS が一次対応し、人に必要な判断だけ Discord で依頼する。dispatcher と store は引き続き決定的であり、LLM の呼び出しは worker の責務とする。

## D1 会話モデル

### 正本と保存

SQLite を正本とする。worker のプロセスは run ごとに終了してよく、メモリ常駐を継続性の条件にしない。ハーネスの session cache を失っても、DB の要約・履歴・操作結果から fresh session を構築できる。添付のバイト列だけは D4 の data dir に持ち、DB と一緒に backup する。

migration は **0050_cos_chat.sql** を選ぶ。2026-10-05、HEAD `8dc4c5e48b08415372d747a0d5f91fcab5a5b9d4` で全 1,331 refs（heads/remotes）と登録 worktree の migration を走査した最大値は 0049（routing_shadow_budget）、0050 は未使用だった。SQL の追加は store-api が行う。完了前の再走査でも 1,333 refs・159 worktree で最大 0049 だった。追加直前にも全ブランチ・worktree を再走査し、競合時は最大値の次へ振り直し、本節と schema version の試験を一緒に直す。予約のための空 migration は作らない。

以下は新設表の列契約。`id` は TEXT ULID、時刻は TEXT RFC3339 UTC、`?` は NULL 可、JSON は TEXT、真偽は INTEGER 0/1。既存 store と同様、参照検査は操作層の transaction で行う。UNIQUE/CHECK は判断ではなく整合性制約なので DB でも強制する。

| 表 | 列（PK 以外も省略しない） | 制約・index |
|---|---|---|
| `chat_threads` | `id PK, kind, title, project_id?, status, queue_paused, next_seq INTEGER, revision INTEGER, summary TEXT, summary_through_seq INTEGER, created_at, updated_at, archived_at?` | kind=`human/inbox/legacy`、status=`open/archived`。`(status,updated_at DESC,id DESC)`、`(project_id,updated_at DESC,id DESC)`。kind=inbox の部分 UNIQUE（システム受信箱スレッドは 1 本） |
| `chat_messages` | `id PK, thread_id, seq INTEGER, role, text, state, client_message_id?, reply_to_id?, run_id?, source_event_id INTEGER?, legacy_message_id?, metadata_json, created_at, updated_at` | role=`user/assistant/system`、state=`queued/running/completed/cancelled/interrupted/failed`。UNIQUE `(thread_id,seq)`、`(thread_id,client_message_id)`（非 NULL）、`legacy_message_id`（非 NULL）。`(thread_id,state,seq)`、`(run_id)` |
| `chat_runs` | `run_id PK, thread_id, input_message_id, output_message_id, state, reason?, session_row_id?, resolved_config_json, started_at?, finished_at?` | state=`queued/running/stopping/completed/stopped/failed/interrupted`。`(thread_id,started_at,run_id)`、UNIQUE `(thread_id)` WHERE state IN ('running','stopping')。run 自体の会計・lease は既存 runs を使用 |
| `chat_events` | `id INTEGER PRIMARY KEY AUTOINCREMENT, thread_id, run_id?, message_id?, type, payload_json, created_at` | `(thread_id,id)`。再接続可能な SSE 正本。監査 events とは用途を分ける |
| `chat_attachments` | `id PK, thread_id, original_name, media_type, size_bytes INTEGER, sha256, relative_path, state, created_at, expires_at?` | state=`ready/deleted`、`(thread_id,created_at,id)`、`(sha256)`、`(expires_at)`。同じ hash の別 upload も独立 id |
| `chat_attachment_refs` | `attachment_id, owner_kind, owner_id, created_at` | PK `(attachment_id,owner_kind,owner_id)`、`(owner_kind,owner_id)`。owner_kind=`message/task/knowledge_inbox` |
| `cos_inbox_items` | `id PK, source_kind, source_key, source_revision, source_event_id INTEGER?, thread_id, message_id, state, run_id?, reason?, policy_version, operation_id?, created_at, updated_at` | UNIQUE `(source_kind,source_key,source_revision)`、`(state,created_at,id)`。state=`pending/running/answered/observed/escalated/fallback/resolved` |
| `cos_operations` | `id PK, thread_id, run_id, item_id?, idempotency_key, request_hash, target_kind, target_id, expected_revision, action, payload_json, reason, policy_version, state, result_json?, event_id INTEGER?, supersedes_id?, created_at, updated_at` | UNIQUE `(thread_id,idempotency_key)`、`(item_id,created_at,id)`。state=`pending/applied/rejected/superseded/needs_remediation` |

既存 `node_sessions` に `thread_id TEXT NULL, llm_source TEXT NULL, model TEXT NULL, summary_through_seq INTEGER NOT NULL DEFAULT 0` を追加する。CoS 用 `kind='cos_chat'` は thread_id 必須。`(thread_id) WHERE kind='cos_chat' AND retired_at IS NULL` を部分 UNIQUE にし、retire→create を一つの transaction にする。既存 conversation/lead/continuation の行と ADR-0140 の task/WU key は維持する。`chat_runs.session_row_id` はこの行を参照する。

メッセージ採番・キュー投入・`chat_events` 追加は同じ transaction。検索は thread の題名と message の text を対象とし、FTS5 の `chat_search(thread_id UNINDEXED,message_id UNINDEXED,title,text)` と同期 trigger を 0050 に追加する。検索語は literal として escape し、FTS 構文を直接実行させない。新規導入時に既存会話を backfill する。空検索は一覧、結果は thread 単位に重複排除し、更新日時/id の降順でページ化する。

0050 には冪等受付用 `chat_client_requests`（D2）と容量予約用 `chat_upload_reservations(id PK,thread_id,client_upload_id,reserved_bytes INTEGER,lease_expires_at,created_at)` も含める。後者は UNIQUE `(thread_id,client_upload_id)`、`(lease_expires_at)` の索引を持つ。予約量は実際に受信する長さ（未知なら 1 ファイル上限）とし、既存 blob の合計 byte 長+有効予約量で上限を検査する。

通知の outbox は既存 notifications を使う。`cos_notification_routes(item_id PK,notification_id UNIQUE,route,source_kind,source_key,source_revision,created_at,updated_at)` を足し、UNIQUE `(source_kind,source_key,source_revision)`、route=`escalation/fallback` で二重 claim を防ぐ。source cursor と切替版は既存 feed_cursors を使う。新 kind と actor/理由を含むイベントの型・API schema は store-api と cos-run が同じ契約で追加する。

### 旧 Console と既存データ

- 新しいホームは chat_threads/messages だけを会話の正本にする。ADR-0048 D1 の全案件 Console stream は `/console` の監視用ビューとして残す。task/progress/report 等の正規化（D2）は新チャットのカード・tool 表示にも共用するが、全案件の出来事を全スレッドへ流さない。
- 旧 `messages` の CoS 行を **project_id ごと（NULL は全体）に 1 本の legacy スレッド**へ移す。`(created_at,id)` 順に seq を採番し、role user→user、node→assistant、text/run_id/task_id/metadata を保存する。`legacy_message_id` の UNIQUE で再実行を冪等にする。古い task_id は metadata 内の出典に保持し、run を起こさない。
- 旧 CoS session は複数スレッドへ共有せず retire する。初回再開で当該 thread の履歴を読む。旧 messages は削除せず、移行の watermark と legacy_message_id により Console で二重表示しない。非 CoS ノードの会話は旧 messages のまま。
- 既存 DB の migration 中は daemon の通常の排他・schema version 手順を使う。稼働中の旧 CoS run は切替前に drain/stop して最終返事を取り込み、その後に migration/backfill。二つの書込み口を同時に正本として運用しない（D6）。

## D2 CoS run と API 契約

### 継続・キュー・停止

1 thread に現役 session は 1 本、同時実行 run は 1 本。複数 thread は並列可だが ADR-0089 の全体 CoS 上限を守る。人のメッセージは受信 transaction で即保存し、seq 順 FIFO で一つずつ run に渡す。稼働中の worker に勝手に新発言を混ぜない。`interrupt` は停止を要求し、新発言を次の入力にする明示操作（未着手メッセージより前に渡し、追い越したものはキューに残す）。

`stop` は指定 run の cancellation を伝え、process group の停止・lease 解放を確認してから stopped にする。途中本文/tool 結果は interrupted として残す。既に済んだ API 操作を取り消したとは表示しない。停止では `queue_paused=true` にして、次の発言を自動起動しない。`resume-queue` または新しい人の送信で明示再開（送信には `resume_queue=true` が必要）。割り込みは旧 run 終了後にその割り込み発言だけを優先し、残る queue は元の pause 状態に従う。古い run_id に対する遅延 stop は後続 run を止めない。

再起動時は DB の queue と lease を回収する。実行中だった run は既存の孤児検出が確定するまで再起動しない。消失確認後 interrupted とし、受信済み入力・適用済み operation を渡して新 run で継続する。外部への副作用の成否が不明なら自動再実行せず D3 の人待ちにする。重複送信は client_message_id で同じ応答に戻す（本文・添付・mode が違えば 409）。

session の照合 key は `(thread_id,harness,provider,llm_source,account_id,cwd,model)`。一致すれば resume、変更・cache 消失・resume 拒否・context 上限なら retire し summary+未要約履歴で fresh。resume 拒否後の fresh 再試行は 1 回だけで、二重に入力や操作を適用しない。Claude の UUID、同一 account/cwd 条件、account cache をコピーしない条件は ADR-0054/0140 を継承する。rollover の判定は既存 `[sessions] rollover_tokens` とハーネスの context 枯渇イベントによる決定的なもの。

要約は CoS worker が更新する（人の指示・決定、未完了作業、operation/添付の id を含み、`summary_through_seq` までを確定）。通常の run 終了時に次回用を保存する。要約が無い/古い場合は DB の範囲付き履歴と未解決事項を初期入力にし、worker が履歴取得 API をページ送りして補完する。古い内容を無言で切り捨てず「要約未作成の範囲」を明示する。dispatcher が LLM 要約を作ることはない。再開中は未配送 seq 以後の差分と現在の規則だけを渡す。summary_through_seq は配送 cursor と別であり、summary の更新だけで入力を処理済みにしない。配送は input_message_id と入力 message.state によって記録する。

### config と harness の写像

以下は新規 `[cos]` 契約。`harness` はこの節では実行ハーネス名（adapter 選択）であり、仕事の分野を表す既存 `[[harnesses]]` の id と混同しない。CoS 専用 role を内部で組み立て、非 CoS の conversation 設定は変えない。

```toml
[cos]
enabled = true
harness = "claude-code"  # claude-code / codex / opencode（adapter=acp）
llm_source = "claude_oauth"  # 省略時は適合 provider の source
# provider = "claude-pool"  # 指定すればその provider に固定
# account_id = "claude_max_lab"  # 指定すればその account に固定
# model = "..."  # 指定時は tier のモデル解決より優先
tier = "frontier"
max_turns = 70
max_wall_secs = 900

[cos.triage]
policy_skill = "cos-inbox-triage"
policy_version = "1"
min_confidence = 0.85
human_required = ["fundamental_change", "external_publish", "destructive", "resource_overrun", "security", "explicit_human"]
unavailable_after_secs = 120
```

`llm_source` は ADR-0132 の `LlmSourceRef` と同じ文字列（claude_oauth/codex_oauth/celeris/openai_compatible:<id>）。none/unknown は CoS では不可。明示の組合せが provider/adapter/model/account と矛盾するなら config reload を拒否し、以前の設定を保つ。初回起動で不正なら起動失敗。省略時は claude-code に適合する有効 provider を既存規則で解決する。候補が無い場合は理由付き unavailable とし、勝手に別 harness/source に変更しない。実効値を chat_runs に保存し、変更は次 run から適用する。

account 未固定時は session の sticky を優先し、その後 ADR-0089 の least-loaded 選択を行う。固定 account が枯渇したら待ち、他 account に逃がさない。全候補 quota 切れ・cooldown は待ちの理由を UI に出す。quota は無制限にせず、token・費用・node-hours の既存制約、ログイン、provider の能力を尊重する。通常タスク用の枠が埋まっても CoS は走れる。`max_cos_runs`（既定 2、0 は通常枠へ戻す）の意味、pool concurrency 例外・account 上限 +1・非 pool の concurrency を維持する。受信箱の CoS run もこの枠に含める。`cos.enabled=false` と `max_cos_runs=0` を混同しない。内部の検証済み CoS 起動経路だけがこの例外を使え、任意の task が cos を名乗って迂回できない。

| harness | 継続と tool 実行の契約 | 正規化 |
|---|---|---|
| claude-code | 初回 UUID/session-id、以後同じ session の resume。従来の read tool 6 件制限を撤去し、D3 の範囲で通常のファイル・shell・MCP/API 道具を提供 | text→text_delta、tool_use/tool_result→tool、thinking は公開要約だけ |
| codex | 既存 adapter の exec resume と thread.started の id 確定を利用。CoS 用 read-only 固定を撤去。使用 adapter が resume を保証できなければ明示 fresh+DB 履歴 | message/command/MCP の event を text_delta/tool に変換。識別不能なものは status |
| opencode | adapter=acp、provider の command/args は既存設定で指定。session/new→load（能力なし・拒否なら明示 fresh）。CoS 対話だけの permission 一律拒否を撤去し D3 で許可する | session/update の agent_message_chunk/tool_call/tool_call_update を写像。terminal/fs/MCP の道具を同じ作業 dir と権限で渡す |

これは adapter 境界で実装・偽プロトコル試験を要する契約であり、現状の全 CLI が同一能力を持つという主張ではない。未対応能力は config 検証/実行時の reason に出し、道具が使えた・画像を見たと偽らない。ADR-0140 の通常 WU continuation は claude の対象のままで、本 ADR の CoS 会話の 3 harness 対応とは別。

### REST: 後段の固定契約

以下のパスは全て `/api/v1` から。既存 bearer/admin と web gateway の認証を共用し、書込みは管理権限必須。run credential はその管理操作能力を持つが actor と human_required の制約は D3 に従う。chat の閲覧にも既存の認証を要求する。時刻は RFC3339、id は文字列（SSE cursor も文字列）。`T/M/R/A/O/I` は次表の JSON オブジェクト名で、配列中もその形。未知フィールドは書込みで 400、型/内容不正は 422、未認証 401、権限不足 403、対象なし 404、状態/冪等 key 不一致 409、容量超過 413、queue 上限 429、CoS 無効時の新規送信は 503。エラーは既存 ApiProblem の `application/problem+json` に従う。

| 型 | JSON 形（null を含めて返す。例の id/本文は仮値） |
|---|---|
| T: Thread | `{"id":"t","kind":"human","title":"画面の修正","project_id":null,"status":"open","queue_paused":false,"active_run_id":null,"queued_count":0,"revision":1,"created_at":"2026-10-05T00:00:00Z","updated_at":"2026-10-05T00:00:00Z"}` |
| M: Message | `{"id":"m","thread_id":"t","seq":1,"role":"user","text":"直して","state":"queued","client_message_id":"c","reply_to_id":null,"run_id":null,"attachment_ids":["a"],"cards":[],"created_at":"2026-10-05T00:00:00Z","updated_at":"2026-10-05T00:00:00Z"}` |
| R: Run | `{"id":"r","thread_id":"t","input_message_id":"m","output_message_id":"m2","state":"running","reason":null,"harness":"claude-code","llm_source":"claude_oauth","provider":"claude-pool","account_id":"account","model":"resolved-model","tier":"frontier","session_mode":"resumed","started_at":"2026-10-05T00:00:00Z","finished_at":null}` |
| A: Attachment | `{"id":"a","thread_id":"t","name":"screen.png","media_type":"image/png","size_bytes":123,"sha256":"64桁hex","state":"ready","preview_url":"/api/v1/chat/attachments/a/preview","download_url":"/api/v1/chat/attachments/a/content","expires_at":null}` |
| I: Triage item | `{"id":"i","source_kind":"decision","source_key":"d","source_revision":"1","thread_id":"t","message_id":"m","state":"escalated","reason":"外部公開","operation_id":null,"policy_version":"1"}` |
| O: Operation | `{"id":"o","thread_id":"t","run_id":"r","item_id":"i","target_kind":"decision","target_id":"d","action":"answer","state":"applied","actor":"cos","reason":"承認済み仕様内の選択","policy_version":"1","event_id":"123","supersedes_id":null,"result":{}}` |
| Card（M.cards の要素） | `{"kind":"task","id":"task-id","title":"修正","state":"running","href":"/tasks/task-id","actor":"cos","reason":null,"operation_id":null}`。kind は task/decision/question/approval/plan_gate/notice/operation。actor は human/cos/system |

| メソッド・endpoint | 入力 JSON / query | 成功応答 JSON・status |
|---|---|---|
| GET `/chat/threads` | `q=&status=open&before=<cursor>&limit=50`（最大 100） | 200 `{"items":[T],"next_cursor":null}`。q は題名/本文検索 |
| POST `/chat/threads` | `{"title":"新しい会話","project_id":null,"client_thread_id":"client-key"}` | 201 `{"thread":T}`。client_thread_id は必須の冪等 key（再送は 200）。kind は人から指定不可 |
| GET `/chat/threads/{t}` | なし | 200 `{"thread":T,"active_run":null,"last_event_id":"0"}`（active_run は R または null） |
| PATCH `/chat/threads/{t}` | `{"title":"新題","status":"archived","expected_revision":1}`（title/status の少なくとも一方） | 200 `{"thread":T}`。run/queue が残る archive は 409。inbox は archive 不可 |
| GET `/chat/threads/{t}/messages` | `before_seq=100&limit=50`（最大 200、省略時最新。`after_seq` は差分用、before と排他） | 200 `{"items":[M],"next_before_seq":null,"next_after_seq":null,"snapshot_event_id":"123"}`。items は seq 昇順 |
| POST `/chat/threads/{t}/messages` | `{"client_message_id":"c","text":"直して","attachment_ids":["a"],"reply_to_id":null,"mode":"queue","resume_queue":false}` | 202 `{"message":M,"run_id":null,"queue_position":1}`。mode=queue/interrupt。未起動時 run_id は null |
| DELETE `/chat/threads/{t}/messages/{m}` | なし | 200 `{"message":M}`（state=cancelled）。queued の user 入力だけ取消可、開始済みは 409 |
| POST `/chat/threads/{t}/stop` | `{"run_id":"r"}` | 202 `{"run":R,"queue_paused":true}`。終端への再送は 200、現在 run が異なれば 409 |
| POST `/chat/threads/{t}/resume-queue` | `{"expected_revision":2}` | 200 `{"thread":T}` |
| GET `/chat/threads/{t}/runs/{r}` | なし | 200 `{"run":R}` |
| GET `/chat/threads/{t}/runs/{r}/events` | `after=<cursor>&limit=100`（最大 500） | 200 `{"items":[E],"next_cursor":null}`（E は SSE と同じ envelope） |
| GET `/chat/threads/{t}/stream` | `after=<event-id>`、または `Last-Event-ID` | 200 `text/event-stream`、下表の E。両方指定して不一致なら 400 |
| POST `/chat/threads/{t}/attachments` | multipart/form-data、`file` 1 個、`client_upload_id` 1 個（必須の冪等 key） | 201 `{"attachment":A}`。同じ key/bytes/name は 200、違えば 409 |
| GET `/chat/attachments/{a}` | なし | 200 `{"attachment":A}` |
| GET `/chat/attachments/{a}/content` | なし | 200 バイト列、Content-Disposition: attachment、nosniff |
| GET `/chat/attachments/{a}/preview` | なし | 200 安全に再エンコードした raster。非対応は 404、A.preview_url=null |
| DELETE `/chat/attachments/{a}` | なし | 204。未参照の upload のみ、参照ありは 409、既削除への再送は 204 |
| POST `/chat/attachments/{a}/references` | `{"owner_kind":"task","owner_id":"task-id","idempotency_key":"copy-1"}` | 200 `{"attachment_id":"a","owner_kind":"task","owner_id":"task-id"}`。task/knowledge_inbox のみ。D3 の監査対象 |
| GET `/cos/inbox` | `state=pending&after=<cursor>&limit=50`（最大 200） | 200 `{"items":[I],"next_cursor":null}`。未指定は全状態 |
| POST `/cos/threads/{t}/checkpoint` | `{"run_id":"r","summary":"決定と残作業","through_seq":12,"expected_summary_through_seq":8}` | 200 `{"thread_id":"t","summary_through_seq":12}`。当該 run credential のみ。through_seq は既に配送済みの範囲、summary は UTF-8 32 KiB 以下、旧値の不一致は 409 |
| POST `/cos/inbox/{i}/resolve` | `{"idempotency_key":"k","expected_revision":"1","outcome":"answer","answer":{"option":"continue","note":"既定手順内","payload":null},"reason":"承認済み計画と一致","confidence":0.97,"policy_version":"1","escalation":null}` | 200 `{"item":I,"operation":O}`。outcome=answer/observe/escalate、answer 以外では answer=null。observe では operation=null 可。escalate は下記 packet 必須 |
| POST `/cos/operations` | `{"idempotency_key":"create-1","expected_revision":null,"reason":"人が修正を依頼","policy_version":"1","request":{"method":"POST","path":"/api/v1/tasks","body":{"title":"画面修正","objective":"依頼の全文"}}}` | 200 `{"operation":O}`。task/KB 等の通常制御操作。request.body は指定先の既存 JSON schema で検証。id は run credential から確定 |
| GET `/cos/operations/{o}` | なし | 200 `{"operation":O}`（結果・理由・監査参照） |
| POST `/cos/operations/{o}/override` | `{"idempotency_key":"undo-1","expected_event_id":"123","mode":"return","reason":"仕様を再検討する"}` | 200 `{"operation":O,"item":I,"remediation_task_id":null}`。mode=revoke/return、人の認証だけが使える |

R の未起動の実効設定・started_at、M の未確定 run_id は null、O.actor は cos または human（override）、O.result は未確定なら null。全 enum は D1/D2 の状態を使う。checkpoint は内部の要約更新で、人の入力/決定を上書きしない。resolve は当該 triage run credential のみが呼べる。人は既存 inbox/領域 API で直接回答できる。

thread/upload の冪等 key は `chat_client_requests(kind,scope_id,key,request_hash,result_id,created_at)`（PK kind/scope_id/key）の追加表で保持する。thread の scope_id は認証済みの利用者/インスタンス、upload は thread_id。API は id の存在だけで他の scope の添付を使用させない。現行 single-user の管理者範囲を継承し、将来の複数利用者分離を暗黙に保証しない。

本文は UTF-8 64 KiB、空白のみは添付ありの場合だけ可、1 メッセージ 10 添付、thread あたり待機 100 件。送信応答前に保存し、ネットワーク再送で新 run を増やさない。assistant 本文は progress を DB に積んで随時反映し、完了時の full text を最終値にする。履歴 snapshot と snapshot_event_id は同じ read transaction の値なので、そこから SSE を継げば gap が無い。

### SSE: event と再接続

SSE の `event:` は次表の type、`id:` は chat_events.id の十進文字列。`data:` は共通 envelope **E**: `{"id":"124","type":"text_delta","thread_id":"t","run_id":"r","message_id":"m2","at":"2026-10-05T00:00:00Z","data":{...}}`。run_id/message_id は対象なしなら null。id は thread 内でも単調増加（連番とは限らない）。永続化してから送信し、再送は同じ id。UI は適用済み id 以下を捨てる。

| type | data の JSON 形 | 表示/適用 |
|---|---|---|
| `message` | `{"message":M}` | id ごとに全体を置換（queued、確定返事、system カード） |
| `text_delta` | `{"offset":0,"text":"確認します"}` | UTF-8 byte offset が現在本文長と一致するときだけ追記。不一致は履歴を再取得 |
| `status` | `{"phase":"thinking","summary":"対象を確認中"}` | phase=queued/starting/thinking/working/waiting。公開要約のみ、非公開推論本文は保存しない |
| `tool` | `{"call_id":"call-1","name":"Bash","state":"running","summary":"ファイルを確認","detail":null,"error":false,"truncated":false}` | state=running/completed/failed。call_id ごとに更新。detail は redact 後最大 4 KiB |
| `run` | `{"run":R}` | starting 以降・停止・終端。終端 R の後にその run の本文差分を送らない |
| `queue` | `{"message_ids":["m3"],"paused":false}` | 順序と pause 状態を全置換 |
| `card` | `{"card":{"kind":"operation","id":"o","title":"CoS が代わりに答えた","state":"applied","href":"/?thread=t&operation=o","actor":"cos","reason":"既定手順内","operation_id":"o"}}` | kind/id ごとに全置換 |
| `thread` | `{"thread":T}` | 題名・件数・archive 更新 |

接続時に after 以後を DB から再生し、その続きから live 配信（読取りと subscribe の境界も id で追いつく）。初回 after=0 は残っている全イベント。15 秒の heartbeat は SSE コメントで、永続 id を進めない。retention で cursor が消えたら HTTP 410 ApiProblem（code=chat-cursor-expired）、クライアントは履歴 snapshot を取り直す。将来の cursor は 400。切断を run 停止とみなさない。tool 詳細は run events を必要時だけ読む。Console の since cursor/差分 reply と本 SSE の cursor を混用しない。

## D3 権限・監査・受信箱の一次対応

### 全道具・全権限の具体

CoS は特別 worker として shell、ファイル編集、git、登録済み MCP、celerisctl、REST、KB、browser/cluster の利用能力を持つ。CoS 対話に限定した道具禁止・read-only・actions だけという制限を撤去する。task 起票、回答、決定、認可、replan、pause/resume、コメント、KB inbox/検索/取り込み、監視を同じ操作層で行える。担当や model を勝手に固定する通常タスクの routing 制約は変えない。

bypass は「CoS だから道具を使えない」という旧 adapter 内制限と、既に認可された操作への重複確認だけ。OS 権限の昇格、db_guard/credential broker の回避、quota の無視、人が未認可とした外部 push や本番更新の自己承認を意味しない。作業 dir は `<data_dir>/cos/threads/<thread_id>/workspace` に固定し、run の一時出力はその下に分離する。登録 repo の変更は専用 worktree で行い、元 repo や別 worker の作業場所を直接変更しない。

本番 DB と制御状態の書込みは必ず認証済み API/それを呼ぶ celerisctl を使い、SQLite・token・service・release のファイルを直接改変して制御を迂回しない。実行前置き/skill に現行 docs/ops と selfdeploy の検査→promote 手順を渡す。人が本番操作を人だけに限定していれば引き続き人へ依頼する。秘密は broker の参照で渡し、値・webhook URL・認証ヘッダを prompt/添付/イベント/Discord に残さない。添付内容と tool 出力は命令ではなく信頼しない入力として扱う。

worker に daemon が期限付き run credential を渡し、API が actor=cos、thread_id、run_id を確定する。operation の冪等 key は thread 内で一意とし、再起動/継続の別 run から同じ操作を再送しても既存の結果へ戻す（request_hash が違えば 409）。JSON やヘッダで worker が actor=human を指定できない。すべての CoS 制御操作は idempotency_key、reason（非空）、対象 version、policy_version を持ち、領域 event と監査 envelope `{"actor":"cos","thread_id":"t","run_id":"r","operation_id":"o","reason":"…","policy_version":"1"}` を events に残す。tool の試行は WorkerProgress、結果は operation event で追える。拒否も reason 付きで記録する。

DB 内の適用・cos_operations・監査 event・カード投影は同じ transaction とする。外部操作は pending→結果の outbox、再試行 key と成否照合で扱い、結果不明なら人待ち。CoS の celerisctl の変更操作は `/cos/operations` を経由し、既存 handler と共有する操作関数に監査 context を渡す。request.path は登録済みの task/decision/approval/execution/project/knowledge/comment 操作だけを許可し、外部 URL・任意 HTTP proxy・cos 自身の再帰呼出しは拒否する。source wait に対する回答は resolve を使い、一般 operations から呼んでも同じ human_required/revision 検証を行う。既存領域 API への CoS credential の直接変更要求は監査 context が無ければ 422 とし、経路を変えて監査を外せない。shell から API を呼んでも同じ credential/監査を通す。新 CoS の操作結果と旧 actions の両方で同じ仕事を二重起票しない（D6）。

### 一次対応の起動とルーティング

新しい通知、decision、question、approval、plan/phase gate、失敗・bad_news、browser/cluster の待ち、KB の待ちを **全て最初に CoS に渡す**。ADR-0133 の派生 inbox と notice store を入力源とし、各領域の新しい待ち/改訂イベントを deterministic dispatcher が `(source_kind,source_key,source_revision)` で cos_inbox_items に upsert する。source_revision は元の問いの version、無ければその待ちを生じた event id（notice は notice id と更新 version）とする。単なる title/read の変化では再質問にしない。DB の source event cursor は投入 transaction と同時に進める。イベントが無い旧由来の待ちは起動時・reconcile 時の差分検査で補完し、events 全履歴の毎 tick 全走査はしない。

行に対応する system message を **専用の「受信箱」CoS スレッド**へ保存し、その継続 session の run を起こす。元 task に origin thread があれば、結果カードをそちらにも参照として表示する（一次対応の二重実行はしない）。CoS が走っていれば永続 queue に積み、1 run 最大 20 項目でまとめる。まとめる場合は入力 system message に item_ids を列挙し、chat_runs.input_message_id はその message を指す。個々の item の完了は resolve で別々に記録する。応答の無い run、故障、quota を D6 で検出する。CoS 自身の回答・通知・送信結果を再び一次対応の新着として取り込む循環は source operation_id により除外する。

dispatcher は「人に任せるか」を推論しない。CoS worker が source の最新状態・人の規則・policy skill を読んで answer/observe/escalate を選ぶ。API は構造・権限・現在の revision・明示的 human_required を決定的に検証する。失効した問いには回答せず 409 を返し、新しい revision で判定し直す。人が先に回答した場合も同じで、CoS が上書きしない。明示 human_required の検査は resolve にだけ置かず、CoS credential で既存の領域 API を直接呼んだ場合にも共通操作層で適用する。observe は判断の必要な未解決待ちには使えず、422 を返す。

### 人に回す基準

| 条件 | 既定の扱い |
|---|---|
| 仕様・設計の根本変更、目的/受け入れ条件の変更 | escalate（人の決定を task に結びつける） |
| 外部公開、外部への push、外部への送信・契約の新規認可 | escalate。既に対象/範囲が認可された操作はその evidence を付ける。CoS 自身の Discord 判断依頼は本決定で認可済み |
| 不可逆・破壊的な操作 | escalate。確認を「CoS 全権限」で代用しない |
| node-hours・費用・quota 等の上限超過/引上げ | escalate。待つ/規定範囲内の代替案は CoS が選べる |
| 秘密、権限、セキュリティ方針の変更、認証/TOTP | escalate。既存 standing authorization の範囲での利用は再認可不要 |
| 人が「人の判断」と指定、human check、human_required の gate | 必ず escalate。config/skill で無効にできない |
| CoS の confidence が min_confidence 未満、情報不足・矛盾、外部副作用の結果不明 | escalate。confidence 欠落も低確信と扱う |
| 上記以外で、人の既存指示・仕様・予算・認可内の定型選択/再試行 | reason と根拠付きで answer。plan gate も既承認計画の具体化だけなら回答可 |
| 判断不要の通知・復旧済みの失敗 | observe しチャットに要点を残す。人への割込み通知は出さない |

既定 skill はこの表を含む。人は config の human_required、min_confidence と policy_skill で厳しさや業務規則を調整できる。ただし対象に人が設定した human_required と API の既存認可は skill で解除できない。分類理由/参照した認可/実効 policy_version を保存し、低確信を数値だけで強制的に高くする運用をしない。CoS が新たな standing permission を自分で作ることは security として人に回す。

### 代答・取り消し・差し戻し

`resolve.outcome=answer` は既存 InboxAnswerBody（option/note/payload）を共通操作層へ渡す。questions/decisions/approvals/plan gate の元の検証を迂回しない。events の actor=cos と理由を、受信箱/元チャット両方の「**CoS が代わりに答えた**」カードに出す。原質問・選んだ答え・根拠・時刻・関連 task・人の操作リンクを表示する。observe も判断不要とした理由を残す。

人は override で **revoke（取消）/return（差し戻し）** を選べる。未消費の回答/認可なら失効させ、待ちを新 revision で開き直す。既に後続 run が消費した場合は対象 task と当該判断に依存する子/WU をまず pause（実行中は停止を要求）し、回答を superseded にして新しい人待ちを作る。戻せない外部副作用は消さず needs_remediation とし、補償/修正の task を起票してその id を返す。いずれも人の理由と元 operation_id を events に残し、監査履歴を削除しない。旧 epoch の回答で run を再開できないことを検証する。

### Discord 通知

人に必要なものだけ `outcome=escalate` とし、`escalation` は `{"summary":"公開前の確認","options":[{"key":"publish","label":"公開する"},{"key":"hold","label":"保留"}],"recommended":"hold","recommendation_reason":"公開先が未確認","web_path":"/tasks/task-id"}`。options は現在の問いの選択肢、recommended はその key（推奨なしは null と理由）。自由記述の問いには option key=reply を使い「web で回答」を示す。observe/answer の escalation は null。

worker は webhook の秘密を受け取らず、resolve API が永続 outbox を作り、既存 notifier が送信する。文面は「CoS から判断のお願い」、要点、選択肢、推奨と理由、止まっている範囲、**該当 web 画面への絶対リンク**を必須にする。web_path はサーバが対象から導く同一アプリ内 path と一致確認し、自由な外部 URL を送らない。`notify.gui_base_url` を web の URL として使う（互換名を維持）。未設定は送信設定エラーとして可視化し、リンク無しで成功扱いにしない。

**Discord の返信・リアクションでは回答しない**。今回 webhook の送信だけを使い、web の認証済み画面で回答する。文面にも「回答はリンク先で」と書く。理由は webhook だけでは回答者と対象 revision を確認できないため。bot/interaction の認証付き受付は別の設計とする。

本文は最大 1,900 文字に収める。選択肢が多い場合は概要に絞り、完全な選択肢は web へ誘導する。`allowed_mentions={"parse":[]}` で意図しない mention を無効化し、秘密や添付の本文は送らない。送信は 1 tick 最大 1 通、429 の Retry-After と再送規則は既存を継承。内部 dedup key は source の revision に結びつける。HTTP 成功後の crash には webhook の exactly-once 保証が無いので、まれな重複は同じ参照 id で判別できるようにし、「必ず一度」とは約束しない。

## D4 添付

種類は拡張子による allowlist を設けず任意ファイルを受け付ける（PDF、画像、ソース、archive、未知 binary も保存可）。既定の上限は **1 ファイル 25 MiB、1 メッセージ合計 100 MiB/10 件、インスタンスの保存量 10 GiB**。`[cos.attachments] max_file_bytes / max_message_bytes / max_files_per_message / max_storage_bytes` で調整する。streaming upload 中にもバイト数を数えて止める。Content-Length だけを信じない。web gateway/daemon の body limit も同じ上限に合わせ、全ファイルをメモリに載せない。storage 上限は同時 upload の予約分を含めて transaction で管理し、完了/abort/crash 回収で解放する。

保存先は `<data_dir>/chat/attachments/<attachment_id>/blob`、一時物も同じ管理下の staging に置く。DB にはその相対 path、原名、検出 MIME（不明は application/octet-stream）、byte 長、SHA-256 を持つ。任意の絶対 path を API から受け取らない。原名は表示情報だけで path に使わず、symlink/.. を拒否し、directory 0700・file 0600、排他的生成と atomic rename で保存する。hash は daemon が計算して worker 受渡し時にも照合する。hash による cross-thread の id 共用や利用者への存在漏洩をしない。

任意 file の保存と inline 実行を区別する。HTML/SVG/PDF/未知 binary はダウンロード専用、preview は JPEG/PNG/WebP/GIF の安全な decoder で再エンコードした raster のみ（40 megapixel 上限、GIF は先頭フレーム）。失敗は preview=null、元ファイルは残る。ブラウザで添付を実行せず、tool での archive 展開も自動で行わない。

run へは添付 manifest `{"id":"a","name":"screen.png","media_type":"image/png","size_bytes":123,"sha256":"…","path":"<workspace>/attachments/a/screen.png","delivery":"image"}` を渡し、read-only に stage する。画像は harness の native image content/画像読取 tool に接続する（claude の image block、codex の画像入力、ACP の image/resource capability）。native 非対応なら path と画像読取 tool を使い、どちらも無ければ unsupported を返して画像を読めたと答えない。PDF・その他は path+manifest を渡し、必要な道具を CoS が選ぶ。原文を無条件に prompt に丸ごと埋めない。

CoS が screenshot を task に渡すときは attachment references API で task へ pin し、task の入力 manifest に加える。PDF の KB 取り込みはまず KB inbox candidate を作って pin し、その candidate が原ファイルの provenance（hash、chat/message、依頼本文）を保持する。候補の承認は既存 KB 操作を使う。owner の作成成功と pin の完了を確認してから引渡し済みと表示し、worker には参照から stage して渡す。元チャットの run path を後続 worker に渡すだけで済ませない。

未送信 upload は 24 時間後に GC。メッセージ/task/KB candidate の参照がある blob は自動削除しない。最後の参照が外れたら 30 日後に削除する（未完了の upload/送信中/実行中 run からの参照は先に解放しない）。archive は削除ではない。chat の本文・要約・監査は保持し、chat_events の text/tool 詳細は既定 30 日、run 終端後に限り GC する。retention の値は `[cos.attachments] orphan_ttl_hours=24, unreferenced_retention_days=30`、`[cos] stream_retention_days=30` で指定する。人が原文削除を求める運用では参照先も確認し、hash/操作監査だけを残す。

## D5 UX

- web `/` は CoS チャット。初期表示は会話本文と composer だけを主役にし、案件統計・実行状況・受信箱の詳細は既定で隠す。会話一覧は desktop の開閉 sidebar、狭い幅は drawer。新規、題名変更、検索、過去会話の再開、archive を同じ導線にする。選択 thread は `/?thread=<id>` で共有/再読込できる。「新しい会話」は新 thread で、既存の会話を捨てない。
- assistant の Markdown は sanitize し、コード/リンク/表を表示。tool call は名前・短い要約・成功/失敗の行に折り畳む。thinking は「考え中」または公開要約。streaming 本文を最終 message と二重表示しない。
- 下端を見ているときだけ自動追従。上へスクロールして履歴を読む間は位置を保ち、「最新へ」と未読件数を出す。再接続は保存済み event cursor から再開、cursor 失効は履歴再取得。送信再試行でも吹き出しを増やさない。
- composer は Enter 送信、Shift+Enter 改行、IME composition 中の Enter は送信しない。タッチ用送信ボタンを併設。添付ボタン、drag & drop、clipboard の画像/ファイル貼付けを同じ upload queue に接続し、preview/原名/容量/進捗/取消/失敗を出す。upload 完了前は送信せず、画像だけの送信も可能。
- run 中も入力可。既定はキューに追加し、待機中の内容と順番・取消を表示する。「停止」と「割り込んで送信」を別操作として出し、停止後の未送信キューと再開を見せる。quota 待ち、停止要求中、切断、failed は区別する。
- 作成した task、質問/決定/認可、plan gate、添付引渡し、CoS 代答はカードで文脈に載せる。カードから元画面へ行け、認可/決定は現行 API の選択肢を使う。CoS 代答カードには取消・差し戻しを置く。自動一次対応の中身は「受信箱」会話で追え、人待ちはその件数だけ控えめに表示する。ブラウザの直接 push 通知を別系統にしない。
- 既存の下部タブバー（チャット/受信箱等の主要画面）と共存させ、composer はタブバーと safe-area の上。virtual keyboard が出ても入力/送信/停止を隠さない。320 px 幅から横溢れを防ぎ、44 px 以上の操作領域、キーボード移動、drawer の focus 復元、live region はまとめて読み上げる。全 token を読み上げない。

## D6 移行

### 置き換えの境界

| 旧決定/経路 | 本 ADR 適用後 |
|---|---|
| ADR-0033 D4 の CoS 道具禁止・返答専用 | D2/D3 の特別 worker。非 CoS ノードの conversation は今回変えない |
| ADR-0048 D1/D2 の全体 Console、D3/§2 の actions だけ、D4 のホーム | 全体 Console は `/console` の監視用に存続。ホームは D5、CoS の入力は chat API、書込みは D3 の道具/API。progress の正規化は共用 |
| ADR-0054 D1 の全体 1 session、D2/§2 の read-only/actions で終了、D3 の「新会話=捨てる」 | thread ごとに resume と summary rollover、全道具、queue/stop、独立した新 thread。actions の有無を run の終了条件にしない |
| ADR-0089 の CoS 判定 | 内部 CoS chat/triage run を対象へ追加。上限・account/quota 会計は維持 |
| ADR-0037/0050 の直接通知と ADR-0133 D6 の inbox_new/reminder/digest | **通常の人への通知は CoS の escalation だけ**。既存の直接送信候補生成・直接 reminder/digest を停止。下記 unavailable fallback だけ例外 |
| ADR-0050 の依頼の保持 | 維持。task 起票時の人の依頼と当時の添付を snapshot し、後続発言を過去の依頼へ混入させない。調査だけへ縮小しない |

`/console`・`/console/stream` は新しい CoS messages も legacy_message_id/run_id で重複排除して読み取る。旧 `POST /console/instruct`、`POST /org/cos/messages`、MCP の CoS 入力は当面互換 facade とし、scope ごとに選ぶ既定 legacy thread へ投入する。旧応答の message_id/task_id/node_id は維持（受付時に裏方 task id を確保する）。新 UI は task_id を会話 id に使わない。`/console/new-conversation` は互換 scope の既定 thread を新規作成して 204 を返す。新 UI の別 thread を retire しない。非 CoS の `@node`/org 会話は従来経路へ委ねる。

新 CoS run では result.actions を実行しない。旧 role の実行中 run を drain した後、互換 actions は非移行の旧ノードに限って残し、旧 console_action_runs の冪等記録は消さない。actions による起票を使う旧 prompt/skill は CoS から外して celerisctl/API に更新する。新 CoS が誤って actions を出したら明示エラーのカードにし、黙って無視/重複実行しない。

`gui/` は新チャットへの全面改修の対象外。旧 Console を互換 API で維持し、新 web チャットへの導線と CoS 代答表示に必要な最小対応だけを後段で行う。共通 API schema が変わる場合は gui と web の生成型を両方更新する。web の既存受信箱/通知画面は派生データの閲覧と人の手動回答のために残す（CoS だけに情報を隠さない）。

### 通知の一本化と退避

既存の `celeris::notify::schedule_routes`（outbound_inbox/outbound_digest）、旧 scan/schedule、ブラウザ notify_now、タスク完了・返事・bad_news・cluster login・decision・plan/phase gate の直接送信を棚卸しする。全候補は D3 の一次対応に入力し、notifier の送信入口は **cos escalation / cos unavailable fallback / 管理者の明示 test** に限定する。明示 test は障害確認用の管理操作であり、自動の人向け通知ではない。通知フィードの永続化/既読と受信箱の元状態、reports、送信履歴は残す。notice を作っただけで Discord が鳴る接続は撤去する。

切替 transaction でルート版 `cos_v1` と source cursor を保存し、旧未送信 outbox はその source を CoS item へ結び、旧送信を superseded にする。既に sent の履歴は消さない。導入時は現在の未解決待ちを 1 回だけ取り込み、過去の解決済み/終端通知を一斉送信しない。以後は daemon 再起動をまたいで source revision と outbox key を保持する。

**CoS が動けない場合だけ決定的な直接通知へ退避する**。条件は CoS run の失敗/crash、全候補の quota 切れ/ログイン不可、`cos.enabled=false`、または容量待ち/停止を含め未応答が `unavailable_after_secs`（既定 120 秒）を超えた場合。run が成功終了しても item が未処理なら未応答として検出する。worker 内に LLM を再度呼ばせて退避を判断する必要はない。CoS の run wall-clock 上限とは別に待ちの期限を監視する。CoS daemon 自体が停止している間に送信できるとは約束せず、再起動時に未解決行を回収する。

fallback は「**CoS 不在のため直接通知**」、不在理由、元の待ちの要点・既存選択肢（自由文は web 回答）・推奨（無ければ「CoS の推奨なし」）、該当 web link を定型で送る。回答は人に残し、自動で approve しない。CoS が回復しても、同じ revision の fallback を再度 escalation で送らず、既に人へ委ねた項目として session に渡す。人の回答を優先し、CoS が勝手に引き取って代答しない。送信前にも元待ちの解決を再確認し、解決済みなら outbox を取り下げる。

通常/退避共通の outbox claim を `(source_kind,source_key,source_revision)` で一意にし、run 失敗と escalation の競合でも二系統の pending を作らない。未設定 webhook/送信失敗は消さず retry/failed を保持し、web に「外部通知未達」を常時表示する。3 回失敗後も項目は残り、人の明示 retry と設定復旧で再送可。**送れない待ちを既読/解決済みにはしない**。この検査は LLM 無しで行い、退避失敗が新しい CoS 待ちを無限生成しないようにする。

### 導入順と受け入れ条件

1. 本 ADR と旧 ADR 付記を design 段で確認する（Fable）。store-api が 0050・REST/SSE/schema・添付を実装、cos-run が特別 worker・triage・actor・notifier 移行を実装する。追加要望は任意の後日作業にせずこの段に含める。
2. main-sync が並行 web task の統合を確認し、web-chat が D5 を実装する。ops-docs は config、backup/restore、切替・rollback、未達時の確認、SPEC/索引を実装に合わせる。旧 daemon への rollback は新 schema を直接開かず、停止時 backup から戻す手順として人に示す。本番の service/release/DB はこの WorkUnit で変更しない。
3. 偽 harness（外部 LLM 無し）＋一時 SQLite＋偽 webhook で下表を実行する。時刻/queue/run 終了/HTTP 応答を test clock・barrier・イベント待ちで制御し、sleep の長さや確率に依存させない。

| 必須検証 | 入力・確認する結果 | 担当 |
|---|---|---|
| 一次対応 A: 人不要 | 承認済み範囲の decision/question/approval/plan gate を各 1 件。fake が answer、元待ち解消、events actor=cos/理由、カード表示、webhook **0 通**、同じ source 再配送で再回答なし | cos-run |
| 一次対応 B: 人必要 | 外部 push/設計変更/explicit_human/低確信を各 1 件。fake が escalate、偽 webhook に要点・選択肢・推奨・理由・web link、元待ちは残る。人回答後の通知は止まる。Discord reply/reaction で状態は変わらない | cos-run |
| 一次対応 C: CoS 不在 | fake run 失敗、quota 切れ、cos 無効、期限超過を独立 fixture。LLM 無しの fallback が各 revision 1 outbox、理由と link、再起動/CoS 復帰で再通知・代答なし | cos-run |
| 通知一本化 | ADR-0037/0050/0133 の全通知 kind を入力。通常時は直接 webhook/ブラウザ通知 **0**、observe も **0**、escalate だけ送信。既存 pending 移行、reminder/digest 停止、失敗と escalation の競合、429/未設定/送信失敗を検証 | cos-run / close-out |
| 代答の修正 | 未消費は revoke で待ち復元、消費済みは return で pause+新 revision、不可逆は remediation task。人と CoS の競合は 409、監査履歴不変、古い認可で再開不可 | cos-run / web-chat |
| 保存・API | 旧 messages の移行/再実行/非 CoS 保全、FTS、client id 再送、同時 claim、snapshot→SSE、再接続重複排除、cursor GC の 410、stop の run id 競合、添付上限/.. /MIME/GC/pin | store-api |
| 継続と道具 | 3 harness の初回→resume、restart、rollover summary、account/source/model/cwd 変更、resume 拒否の 1 回 fallback、task 起票/API/KB 操作、quota/ADR-0089 会計と全枠埋まり時の起動、DB 直書き禁止・actor 偽装拒否 | cos-run |
| 添付の引渡し | screenshot→CoS の画像入力→task manifest、PDF→CoS の path→KB inbox pin。chat run の一時 file を消しても後続が読める。未対応画像能力は明示失敗 | store-api / cos-run |
| UI | Playwright: 会話一覧/検索/再開、Markdown/tool/streaming、停止/queue、ボタン/D&D/paste、IME、最新へ、320 px と desktop、下部タブバー・keyboard、CoS 代答/人待ち。画面 screenshot を証拠にする | web-chat |
| 実機 1 回 | 試験用 DB の daemon で既定 claude-code と同じ会話を複数往復、resume と添付/API 操作を確認。実効設定、run id、redact 済み event/画面を記録する。本番 webhook は使わない | live-check |

close-out は `bash scripts/dev/test-parallel.sh`、`cargo clippy --workspace -- -D warnings`、web の typecheck/test/e2e と文書検査を実行し、各 WorkUnit の進捗に結果を残す。実機未実行・不合格を fake 成功で置き換えず、完了状態にしない。通知の通常経路が CoS のみで、例外が本節の不在退避と明示 test だけであることを送信入口の call-site 調査と上表の試験の両方で確認する。

## 付記: 実装との突き合わせ（cos-run、2026-10-06）

D2/D3 の小節ごとに、実装の場所・試験名・ADR との差分（未実装・縮退・食い違い）を書く。検査は統合後 HEAD `5434785b`（文書だけを足した `c9d3919d` でも再実行）で `bash scripts/dev/test-parallel.sh` exit 0（4230 passed）と `cargo clippy --workspace -- -D warnings` exit 0。試験はすべて偽 harness・一時 SQLite・偽 webhook を使う決定的なもの。略号: tc=`crates/task-core`、td=`crates/task-dispatch`、tw=`crates/task-worker`、ta=`crates/task-api`、cc=`crates/celeris`、ctl=`crates/celerisctl`。D6 の受け入れ行と試験名の対応は `agent-docs/progress/2026-10-06-cos-run.md` にある。

### D2 継続・キュー・停止

- 実装: 起動と枠は td `dispatcher/cos_chat/launch.rs`、stop・interrupt・queue_paused・再起動後の回収は `control.rs`、resume 拒否後の fresh 1 回と rollover は `rollover.rs`、session 照合 key と retire 理由は td `sessions/cos_chat/`、session 行は tc `chat/session.rs`、run・queue の保存は tc `chat/run_store.rs`・`chat/store.rs`、要約 checkpoint は tc `chat/credential.rs` と ta `cos/mod.rs`（`POST /cos/threads/{t}/checkpoint`）。
- 試験: `cos_chat_run_control_stop_kills_group_pauses_queue_and_late_stop_isolated`・`_interrupt_precedes_queue_without_changing_pause`・`_live_interrupt_waits_for_old_worker_exit`・`_restart_waits_for_orphan_then_continues_input`・`_pending_side_effect_waits_for_human`（td `dispatcher/tests/cos_chat_launch.rs`）。`cos_chat_run_session_first_run_is_new_and_match_resumes`・`_any_key_change_retires`・`_cache_missing_or_bad_id_retires`・`_resume_refusal_retires_as_fresh_after_refusal`・`_context_limit_retires`（td `sessions/cos_chat/tests.rs`）。`cos_chat_run_rollover_resume_refusal_retries_fresh_exactly_once`・`_retry_does_not_reapply_input_or_operation`・`_token_limit_makes_next_run_fresh`・`_context_exhaustion_makes_next_run_fresh`（td `dispatcher/tests/cos_chat_rollover.rs`）。`cos_chat_ops_checkpoint_enforces_cursor_and_size`・`_requires_current_run`・`_rejects_queued_gap_before_interrupt_input`（tc）。`cos_chat_run_proto_unsummarized_range_is_explicit`・`_prompt_puts_interrupt_first_and_forbids_actions`（tw `cos_chat/tests.rs`）。client_message_id の冪等と queue 上限は store-api の `chat_api_post_replay_limits_and_cancel`・`chat_api_queue_limit_and_run_stop_resume`（ta `tests/chat_api.rs`）。
- 差分: 無し。ただし dispatcher と API router を同じ一時 DB で並べた 1 本通しの試験は無い（progress の未解決に記録）。

### D2 config と harness の写像

- 実装: `[cos]`・`[cos.triage]` の読み込み・検証・reload は cc `config/cos.rs`。reload の拒否と旧設定の維持は cc `daemon/admin.rs`。CoS 枠（ADR-0089 の例外・`max_cos_runs`・固定 account の +1・sticky）は td `dispatcher/cos_chat/launch.rs`。harness の写像は tw `claude_code/`（全道具・UUID の resume）、tw `codex/`（exec resume・thread.started）、tw `acp/`（session/new→load・permission の許可）。能力の表は tw `cos_chat.rs`。
- 試験: `cos_chat_config_defaults_and_unavailable_reason`・`_rejects_unknown_and_invalid_values`・`_explicit_provider_conflicts_and_resolves`・`_reload_rejects_change_and_preserves_old_values`（cc `config/cos_tests.rs`）。`cos_chat_config_reload_keeps_old_config_on_invalid_mapping`（cc `daemon/admin.rs`）。`cos_chat_run_launch_two_threads_fifo_and_capacity`・`_non_pool_provider_keeps_its_limit`・`_max_cos_zero_disables_global_exception`・`_no_candidate_records_unavailable`・`_fixed_account_gets_one_extra_slot_then_waits`・`_sticky_account_falls_back_when_logged_out`（td）。`cos_chat_harness_claude_*`（7 件）・`cos_chat_harness_codex_*`（9 件）・`cos_chat_harness_acp_*`（6 件）・`cos_chat_harness_caps_*`（4 件）・`cos_chat_harness_config_*`（5 件）。`cos_chat_harness_e2e_{claude,codex,acp}_*`（td `dispatcher/cos_chat/harness_tests.rs`）。
- 差分（縮退）: opencode（acp）は、session/load・画像の能力が未確認の場合に config の warning と実行時の reason を出し、明示 fresh で動く（`cos_chat_harness_config_opencode_lists_unconfirmed_capabilities`・`cos_chat_harness_acp_missing_load_starts_fresh_once`）。これは ADR の「能力なし・拒否なら明示 fresh」どおり。「一般 pool が満杯でも CoS が別枠で起動する」を 1 本で通す fixture は無い（枠 0 の例外と CoS 枠の容量は上の試験で確かめた）。

### D2 REST の固定契約

- 実装: `/chat/*` は ta `chat/mod.rs`・`chat/attachments.rs`（store-api 担当）。`/cos/inbox`・resolve は ta `cos/inbox.rs`、`/cos/operations` は ta `cos/operations.rs`、override は ta `cos/override_op.rs`、run credential は ta `cos/mod.rs` と tc `chat/credential.rs`。
- 試験: `chat_api_*`（15 件。ta `tests/chat_api.rs`、store-api 担当）。`cos_chat_ops_api_*`（6 件）・`cos_chat_ops_auth_*`（5 件）・`cos_chat_ops_checkpoint_api_*`（2 件）・`cos_chat_triage_resolve_*`（3 件）・`cos_chat_triage_override_*`（8 件）（いずれも ta `tests/`）。
- 差分: 無し。

### D2 SSE の event と再接続

- 実装: ta `chat/stream.rs`（cursor・410・heartbeat、store-api 担当）。worker の進行を chat_events に写すのは td `dispatcher/cos_chat/sink.rs`（Text/ToolUse/ToolResult/Thinking/Status → text_delta/tool/status）。
- 試験: `chat_stream_snapshot_then_live_has_no_gap`・`chat_stream_last_event_id_reconnect_has_no_duplicate`・`chat_stream_heartbeat_does_not_advance_the_id`・`chat_stream_expired_cursor_is_*`（ta `tests/chat_stream.rs`）。`cos_chat_run_sink_maps_text_tool_thinking_status_in_order`・`_pairs_tool_calls_and_fails_open_calls_at_end`・`_redacts_tool_detail_and_caps_it`・`_sends_nothing_after_terminal`・`_terminal_mapping`（td `sink_tests.rs`）。
- 差分（食い違い）: 受信箱の一次対応で `cos_triage_claim` が起こす chat_run は output message と message event を作らない。このため SSE は次の run event まで更新されない（`agent-docs/progress/2026-10-06-cos-run/triage.md` の未解決）。

### D3 全道具・全権限の具体

- 実装: 全道具は D2 の写像の行のとおり（read-only・道具禁止の撤去。非 CoS の secretary は read-only のまま）。監査付きの操作層は ta `cos/operations.rs`（allowlist `ALLOWED`）と tc `chat/operations.rs`（同じ transaction で領域 write・`cos_operations`・監査 envelope・カード）。直接の領域 API に CoS credential を使うと 422（ta `cos/mod.rs`）。celerisctl の変更操作は ctl `commands/cos_ops.rs` で `/cos/operations` に包む。前置き skill は `config/skills/cos-operator/SKILL.md`。
- 試験: `cos_chat_ops_store_*`（4 件、tc）。`cos_chat_ops_api_rejects_paths_outside_the_allowlist_with_reasoned_events`・`_idempotency_same_hash_same_operation_different_hash_409`・`_body_shape_and_identity_claims_are_rejected`（ta）。`cos_chat_ops_domain_*`（7 件、ta `tests/cos_operations_domains.rs`）。`cos_chat_ops_auth_get_allowed_and_direct_mutation_is_422_with_audit`・`_identity_claims_are_ignored_or_422`（ta）。`cos_chat_ops_ctl_*`（9 件、ctl）。`cos_chat_run_proto_credential_value_stays_out_of_request_json_and_prompt`（tw）。`cos_chat_harness_codex_non_cos_secretary_stays_read_only`（tw）。
- 差分（食い違い・未実装）: D3 は「replan・pause/resume を同じ操作層で行える」とするが、`ALLOWED` に登録されているのは task.create・comment.create・decision.answer・approval.decide・execution.phase_gate・execution.plan_gate・question.answer・project.update・knowledge.reject だけである。`PUT /tasks/{id}/execution-plan`、`POST /tasks/{id}/pause|resume`、`POST /projects/{id}/pause` は未登録。一方、`config/skills/cos-operator/SKILL.md` の操作表は、replan・pause/resume（ほかに `/knowledge/inbox/{id}/accept` と `/standing-rules`）を `/cos/operations` 経由で行うと書いている。そのため CoS が skill どおりに実行すると、allowlist 外として拒否される（`cos_chat_ops_api_rejects_paths_outside_the_allowlist_with_reasoned_events` が確かめている経路）。D3 が求める KB の取り込み（accept）も同じく未登録。**人の判断事項**: (a) `ALLOWED` に execution-plan PUT・pause/resume（task・project）・knowledge accept を足し、共有の操作関数に監査 context を通す（D3 に合わせる）、(b) skill の該当行を「allowlist 未登録のため人に依頼する」に直し、D3 の「全権限」の範囲を縮める。推奨は (a)。standing-rules は D3 の人に回す基準（新しい standing permission は security）により、どちらの場合も人に回す。

### D3 一次対応の起動とルーティング

- 実装: `(source_kind,source_key,source_revision)` の upsert と cursor は tc `chat/triage.rs`、ingest と受信箱スレッドへの投入（1 run 最大 20 項目・cos operation 由来を除く・origin thread への参照カード）は td `dispatcher/cos_chat/triage.rs`、resolve と revision の検証は ta `cos/inbox.rs`。
- 試験: `cos_chat_triage_store_*`（4 件、tc）。`cos_chat_triage_ingest_*`（7 件、td `dispatcher/tests/cos_chat_triage.rs`）。`cos_chat_triage_resolve_conflicts_on_stale_revision`・`cos_chat_triage_observe_needs_no_judgment`（ta）。`cos_chat_triage_a_*`（ta・td）。
- 差分: 上の SSE の差分（triage run の output message 無し）だけ。

### D3 人に回す基準

- 実装: 既定 policy は `config/skills/cos-inbox-triage/`。`human_required`・`min_confidence` は cc `config/cos.rs`。API 側の決定的な検査（明示の human_required を resolve と operations の両方で拒否・confidence 欠落の扱い）は ta `cos/inbox.rs`・`cos/operations.rs`。
- 試験: `cos_chat_triage_human_required_refused_on_resolve_and_operations`・`cos_chat_triage_b_escalates_human_matters_and_keeps_the_wait`（ta）、`cos_chat_triage_b_*`（td・cc）。
- 差分: 表の各分類（根本変更・外部公開など）を選ぶのは CoS worker（LLM）の判断であり、決定的に試せるのは明示の human_required と構造・revision の検査だけ。これは ADR の設計どおりで、分類の正しさは実機確認（live-check 担当、未実施）で見る。

### D3 代答・取り消し・差し戻し

- 実装: ta `cos/override_op.rs`・tc `chat/override_op.rs`（revoke/return・消費済みの pause・superseded・needs_remediation と補償 task）。代答カードは tc `chat/operations.rs`。
- 試験: `cos_chat_triage_override_revoke_reopens_unconsumed_decision_and_preserves_audit`・`_return_pauses_consumed_task_and_blocks_old_epoch`・`_irreversible_op_creates_remediation_task`・`_rejects_cos_credential_and_missing_reason`・`_human_revision_wins_race_with_cos_answer`・`_reblocks_released_work_unit_until_human_answer`（ta）、`cos_chat_triage_override_new_revision_reopens_triage_and_old_is_closed`（td）、`cos_chat_triage_override_revokes_a_resolved_answer_end_to_end`（ta scenarios）。
- 差分（縮退）: plan gate の回答の withdraw は中止の連鎖を伴うため 422 で拒否する（triage.md の未解決）。

### D3 Discord 通知

- 実装: escalation packet の検証・web_path の照合・outbox は ta `cos/inbox.rs` と tc `chat/triage.rs`。送信（文面・1,900 文字・`allowed_mentions`・Retry-After・送信前の撤回）は cc の notifier。CoS 不在の退避は td `dispatcher/cos_chat/fallback.rs`。
- 試験: `cos_chat_triage_escalation_packet_is_validated`・`cos_chat_triage_web_path_must_match_the_target`・`cos_chat_triage_escalate_claims_one_outbox_row`（ta）。cc `tests/cos_chat_triage_notify.rs` の 8 件（`_escalation_packet_has_bounded_text_and_absolute_link`・`_long_packet_keeps_link_within_1900_characters`・`_webhook_disables_mentions_and_obeys_retry_after` など）。`cos_chat_triage_unified_only_cos_escalation_reaches_the_webhook`（cc scenarios）。`cos_chat_triage_fallback_*`（11 件、td）。
- 差分: Discord の返信・リアクションを無視することは、受信経路を実装していないことの帰結であり、それを積極的に確かめる試験は無い。`notify.gui_base_url` が未設定の場合の表示を docs に反映するのは ops-docs 担当。
