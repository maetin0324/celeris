# ADR 2026-10-05: CoS チャットをホームにし、スレッド継続・全道具・受信箱の一次対応を担う特別ワーカーにする

---
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV]
---

- 日付: 2026-10-05（2026-10-06 付の人の追加要望を含む）
- 状態: **実装済み（2026-10-07 close-out 完了。同日の差し戻し修正を反映済み。ただし実機確認の (d) screenshot の task 入力引き継ぎと (e) PDF の KB 候補は未達で、下の「付記: 差し戻し修正と実機確認（close-out 2、2026-10-07）」の D1〜D4 が未解決）**。人が既に決めた方針を以下の契約に具体化し、design 段の確認（Fable）を経て store-api・cos-run・web-chat・live-check・ops-docs 各段で実装済み。統合後 HEAD `21f38050` で `bash scripts/dev/test-parallel.sh`（4355 passed）・`cargo clippy --workspace -- -D warnings`・web の typecheck/lint/test/build・e2e（functional 278 passed）・文書検査 4 本が exit 0（`agent-docs/progress/2026-10-05-cos-chat-home.md`）。**2026-10-06 付記（cos-run 完了）: cos-run 担当の実装（`[cos]` config・CoS 特別 worker としての chat run 起動/継続/停止/queue/SSE・3 harness（claude-code/codex/opencode）の継続と全道具・画像入力の写像・受信箱一次対応と Discord escalation・CoS 不在退避・通知一本化・代答の取消/差し戻し・旧 Console/MCP 入力の legacy facade・`/cos/operations` 監査付き操作層・run credential）は実装済みで、統合後 HEAD `5434785b` で `bash scripts/dev/test-parallel.sh`（4230 passed）・`cargo clippy --workspace -- -D warnings`・文書検査 4 本が exit 0 を確認済み（`agent-docs/progress/2026-10-06-cos-run.md`）。D6 表の cos-run 担当行の `cos_chat_` 試験対応表（130 件）と、実機 1 回（live-check）・replan/pause/resume の allowlist 登録・添付→task manifest と KB provenance の連結を未解決として記録する。→ 添付の連結は 2026-10-07 の attach-handoff 付記で解決済み。**
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

`/console`・`/console/stream` は新しい CoS messages も legacy_message_id/run_id で重複排除して読み取る。旧 `POST /console/instruct`、`POST /org/cos/messages` は当面互換 facade とし、scope ごとに選ぶ既定 legacy thread へ投入する。旧応答の message_id/task_id/node_id は維持（受付時に裏方 task id を確保する）。新 UI は task_id を会話 id に使わない。`/console/new-conversation` は互換 scope の既定 thread を新規作成して 204 を返す。新 UI の別 thread を retire しない。非 CoS の `@node`/org 会話は従来経路へ委ねる。

### 互換 facade の改訂（人の決定、2026-10-08）

CoS チャットで「chatgpt-rdc 経由の依頼は新しい CoS スレッドを作成して受け入れる」と決定されたため、
MCP の `console_instruct` を互換 facade から外す。全 MCP client で、thread_id を省略した呼び出しは
`kind=human` の新 thread を作る。題名は client_id と本文先頭行（最大60文字）、project_id は thread に保持し、
user message の metadata.author に `mcp:<client_id>` を保存する。通常の queue と CoS chat run が動く。
thread_id 指定は同じ thread に続ける。不存在/archived/legacy と project 不一致は invalid_params。
作成と投入・出どころは一つの transaction。message_id/task_id に thread_id を追加して返す。
task_id は入力メッセージ ID と同じ互換の受付 ID で、task 行や legacy message は作らない。
console_reply は task_id または thread_id のどちらか一つで取得する。受付 ID はその入力、thread_id は
最新の user input の run/output を読む。completed→done、failed→failed、stopped/interrupted/cancelled→cancelled、
その他→pending（wait_secs は上限60秒）。actions[] は常に空。
REST の POST /console/instruct・POST /org/cos/messages・/console/new-conversation と旧データは維持する。
実装は task-core chat store の chat_mcp_instruct/chat_mcp_reply と celeris-mcp tools/console.rs。
試験は mcp_integration の cos_chat_mcp_*、task-core の cos_chat_mcp_metadata_and_atomic_validation、
task-api の cos_chat_legacy_console_instruct_queues_and_keeps_ids。

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
- 差分（設計判断は解消、2026-10-09）: [全 API 変更操作の CoS 監査経路 ADR](2026-10-09-cos-operations-all-mutations.md) D1〜D6 により、人が API でできる変更操作の全てを `/cos/operations` に登録する方針を確定した。原因は harness の bypassPermissions ではなく daemon の ALLOWED が一部に限られることである。execution-plan PUT・task/project pause/resume・knowledge accept を含め、142 method/path の領域割当と理由付き除外を新 ADR の表に確定した。人は promote・standing-rules 作成/削除・browser control・DELETE を許可し、秘密/attestation 系と console/instruct は除外と決めた。standing-rules を常に人へ回す旧一般基準はこの明示決定で更新し、既存 human_required の検査は維持する。旧 (a)/(b) の選択は (a) に決定済みで、人の判断待ちは無い。**実装の差分は後続の registry・領域・ctl・skill・verify WorkUnit で解消する**（この追記だけで ALLOWED の登録済みとはしない）。
  2026-10-09 実装付記（task 01M4F5KS8E1MXZDNJTRAVFJESW）: decision revise/withdraw・knowledge accept・task edit/reopen/retry/pause/resume・execution-plan PUT（replan）・LLM 割り当て PUT/DELETE を `ALLOWED` に登録し、共有の操作関数で監査と同じ transaction に入れた（詳細は新 ADR の「付記: 実装（2026-10-09）」）。
  2026-10-09 close-out 付記: 残りの route も全て登録し、変更 route 142 行は ALLOWED 106・EXCLUDED 36 のどちらか一方だけに入る（`PENDING` は撤去）。celerisctl の変更系 subcommand は CoS credential で `/cos/operations` に包み、skill の操作表と除外表は registry と一致する。**この差分節の実装差分は解消した**（新 ADR の「付記: close-out」）。

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

## 付記: 実装との突き合わせ（web-chat、2026-10-07）

D5 の各項目と D6 の置き換えの境界・UI 行ごとに、実装の module・試験名・ADR との差分を書く。検査は統合後 HEAD `ed9ae61b`（本節を足す前の web-chat 最終 HEAD）で `bash scripts/dev/test-parallel.sh`（4355 passed）・`cargo clippy --workspace -- -D warnings`・web の typecheck/lint/test（vitest 486 + server 57）/build・e2e 全体（functional 277 passed）と文書検査 3 本が exit 0（`agent-docs/progress/2026-10-05-cos-chat-home/web-chat.md`）。web の試験は `web/features/chat/` の vitest（fake timer・試験側 stream）と `web/e2e/chat/` の Playwright（fake-daemon の制御 endpoint と `expect.poll` の出来事待ち。sleep 不使用）。

### D5 ホームと会話一覧

- 実装: `web/routes/index.tsx`（`/` → `ChatHome`、`?thread=<id>` を URL 正本に）と `web/features/chat/home/chat-home.tsx`（URL を選択の正本とし、未指定なら直近の人の会話を自動選択・無ければ新規作成。下部タブ・safe area・keyboard を避けた高さ計算）。一覧は `web/features/chat/threads/{threads,threads.model,threads.test}` で新規・題名変更（revision）・検索（FTS）・archive・受信箱の固定表示。狭い幅は drawer（focus 復元・Escape）。
- 試験: vitest `chat_threads_*`（create が client_thread_id で冪等・search が FTS を使う・rename/archive の revision と 409・drawer の focus 復元）。e2e `web/e2e/chat/threads.spec.ts`（「新しい会話を作る、題名を変える、検索で見つけ、保管で一覧から消す」「共有された ?thread= URL で開き、再読込しても同じ会話と本文・composer が残る」）、`web/e2e/chat/mobile.spec.ts` の drawer 2 件。
- 差分: 受信箱の「人待ち件数」は thread API に無いので、`chat-home.tsx` が `inboxItemsQuery` の `counts.total` を `inboxWaitingCount` として渡す（D5 の「件数だけ控えめに表示」）。100 件より古い受信箱は最初の頁に入らない限り数に入らない（`kind=inbox` フィルタが無い）。

### D5 assistant の Markdown・tool・streaming・追従

- 実装: `web/features/chat/messages/{message-list,message-item,parts,logic}.tsx`。Markdown は `web/lib/markdown`（sanitize・code/リンク/表）。tool は `parts.tsx` の `ToolCall`（名前・要約・成功/失敗の折りたたみ）。streaming は `web/features/chat/data/stream.ts`（reducer）が draft を確定 message に置き換えて二重表示にしない。自動追従は `logic.ts`（下端 48 px 以内だけ追従、prepend で位置保持、未読と「最新へ」ボタン `parts.tsx` の `JumpToLatest`）。
- 試験: vitest `chat_messages_*`（sanitize・code の copy・未閉 fence を code とする・表・tool 折りたたみ/展開/streaming draft→確定が 1 回だけ・追従の閾値と未読）、`chat_data_*`（text_delta の offset 整合・cursor 410 で resync・二重表示しない）。e2e `web/e2e/chat/content.spec.ts`（Markdown・code・表・tool の表示、「streaming は text_delta を流し、確定 message に置き換えて二重表示にしない」）、`web/e2e/chat/resync.spec.ts`（cursor からの再開・410 で snapshot 再取得・「上へ scroll 中は位置を保ち、未読と『最新へ』が出て、押すと下端へ戻る」「古い頁を足しても見ている位置を保つ」）。
- 差分: 共有 Markdown（`web/components/content/markdown.tsx`）の `textBlocks` が code fence 直後の表を解析しない（fence 除去で段落先頭に `\n` が残るため）。e2e は表を fence より前に置いて回避（`web/components/` は web-chat の範囲外。未修正を web-chat.md の未解決に記録）。

### D5 composer（Enter/IME/添付/停止/割り込み/キュー）

- 実装: `web/features/chat/composer/{chat-composer,upload-queue}`。Enter 送信・Shift+Enter 改行・IME composition 中の Enter は送信しない。添付ボタン・drag&drop・clipboard 貼付けを同じ upload queue（`upload-queue.ts`）に接続し、preview/原名/容量/進捗/取消/失敗を出し、upload 完了前は送信しない。停止・割り込み（`mode=interrupt`）・キューの順と取消を別操作として出す。
- 試験: vitest `chat_composer_*`（enter/shift/ime、drop と paste が file 抽出を共有、upload の進捗/失敗/再試行/取消、queue・stop・interrupt・resume・status が別制御、quota/切断/failed の状態）。e2e `web/e2e/chat/queue.spec.ts`（Enter/Shift/IME、「キューに追加された順に表示し、取消で消す」「停止でキューが停止し、停止中の送信は resume_queue で再開」「割り込み送信は mode=interrupt で active run を stopping にする」）、`web/e2e/chat/attachments.spec.ts`（「ボタン・drop・画像 paste が同じキューに入り、upload 中の取消と進捗が表示される」「失敗は表示され送信を止め、同じ upload id の再試行成功後は画像だけ送れる」）。
- 差分: 画像 preview は `URL.createObjectURL` のため、document CSP の `img-src` に `blob:` を許可した（`web/server/app.js`・回帰試験 `app.test.mjs`。人の replan v7 で範囲に追加許可済み）。

### D5 チャット内カード（task/質問/決定/認可/plan gate/知らせ/CoS 代答）

- 実装: `web/features/chat/cards/{chat-card,card-model,card-actions,override-dialog,pending-badge}`。7 種（task・decision・question・approval・plan_gate・notice・operation）を表示し、decision/question/approval/plan_gate はその場で選択肢から回答（受信箱 API）。operation（CoS 代答）は取消（revoke）・差し戻し（return）を dialog で出し、`POST /cos/operations/{o}/override` に送る。`pendingHumanCount` が人待ちを badge に出す。
- 試験: vitest `chat_cards_*`（各 kind の表示・その場回答・operation の revoke/return・409 は競合・remediation の link・pending badge）。e2e `web/e2e/chat/cards.spec.ts`（「7 種のカードが表示され、その場で回答できるカードは選択肢で答えられる」「CoS 代答は取消・差し戻しでき、理由が必須、409 は競合として表示される」「受信箱 thread の badge は受信箱の人待ち項目数を示し、回答で減る」「詳細 link は web 内の実在画面へ SPA で行き、戻るで元会話がそのまま残る」）。
- 差分: (a) 詳細 link は `card.href` をそのまま使うが、実 daemon が代答カードに置く `/cos/operations/{o}` 等の href に web の route が無く 404 になる（fixture は実在 route で検証）。route 追加は API/schema に触れるため別 task。(b) triage が作るカードの href は `/?thread=<inbox>` で、元の待ち画面（`/tasks/{id}` 等）へ行けない。元待ち href を Card に足す案は API schema 変更のため別 task。

### D5 狭い幅・下部タブバー・keyboard・a11y

- 実装: `chat-home.tsx`（タブバー/keyboard の退避量を 1 回だけ引く高さ計算）と `chat-composer.tsx`（shrink-0 でホーム枠の末尾に置く）。`web/e2e/chat/mobile.spec.ts` が 320/390 px の横溢れ・44 px 以上の操作領域・drawer・タブバーと composer の共存・`visualViewport` 縮小時の入力/送信/停止の視界・hit target を決定的に検査。
- 試験: e2e `web/e2e/chat/mobile.spec.ts` の 8 件（320/390 の横溢れ 0 と 44 px 以上、drawer の focus 復元、タブバー共存、keyboard、320 の添付/カード、desktop のカード操作領域）。`pnpm mobile-audit`（32 paths × 4 widths）で 44×44 未満のタップ領域が無いことを確認。
- 差分: keyboard は `visualViewport` の fake と resize イベントによる決定的検査で、実端末 OS の keyboard/IME の実機確認は含まない。

### D6 置き換えの境界（web 側）

- 実装: ホーム `/` は `ChatHome` に置き換え、旧 Console は `web/routes/console.tsx`（`ConsoleView`・`ConsoleRegion`・`HomeEntries`）へ移設して監視用に存続。`/console/stream` は CoS messages を legacy_message_id/run_id で重複排除して読み取る（`web/features/console/`）。CoS の入力は chat API、書込みは D3 の道具/API。
- 試験: e2e `web/e2e/parity/console.spec.ts`・`gateway.spec.ts`（旧 Console の parity）、`mobile-audit` が `/` と `/console` の両方を検査。
- 差分: 互換 facade（`POST /console/instruct`・`POST /org/cos/messages`・`/console/new-conversation`）の daemon 側実装は cos-run/store-api 担当で、web の UI からは使わない（web は chat API へ）。gui/ は新チャットへ触れず、生成型（`types.ts`）のみ共通 schema に従う（本 WU で不変）。

### D6 UI 行（Playwright 必須検証）

- 実装: 上の D5 各項目と同一。e2e は `web/e2e/chat/` の 7 spec・31 試験（threads・content・queue・resync・cards・attachments・mobile）。
- 試験: 会話一覧/検索/再開（threads）、**長い題名の旧会話でも badge は一行に収まり、題名を省略する**（threads。fix-visual で追加）、Markdown/tool/streaming（content）、**streaming は text_delta を流し、確定 message に置き換えて二重表示にしない**（content。生成途中の caret・「停止」ボタンを撮る前に検証）、停止/queue（queue）、ボタン/D&D/paste（attachments）、IME・割り込み（queue）、最新へ・cursor（resync）、320 px と desktop・下部タブバー・keyboard（mobile）、**320/390 px: 狭い幅では会話一覧は drawer になり、開閉と Escape の focus 復元が成り立つ**（mobile）、**320 px で添付・カードは横溢れを出さず、カードの操作領域は 44 px 以上**（mobile。送信前後の preview `naturalWidth=96` を poll）、CoS 代答/人待ち（cards）。
- 差分: 画面 screenshot を `agent-docs/progress/2026-10-05-cos-chat-home/web-chat/screenshots/` の **11 枚**に記録（chat-threads-1440・chat-resume-1440・chat-content-1440・chat-queue-paused-1440・chat-resync-1440・chat-jump-latest-1440・chat-mobile-320・chat-cards-1440 に加え、**chat-streaming-1440**（生成途中＋停止）・**chat-drawer-320**（drawer 開いた 320）・**chat-attachment-sent-320**（送信後の会話内画像 preview）を reclose 前の polish 段で追加）。reclose（統合後 HEAD `b1e61693`）で 11 枚を再生成し、final review の 4 指摘（(1) streaming 中・(2) badge 縦積み・(3) tool 状態 contrast・(4) 狭い幅の drawer と送信後添付）をそれぞれの上の試験と screenshot で満たすことを再確認（`agent-docs/progress/2026-10-05-cos-chat-home/web-chat.md`）。

## 付記: 実装との突き合わせ（close-out、2026-10-07）

全体検査と 3 経路・通知一本化の決定的な試験の結果は `agent-docs/progress/2026-10-05-cos-chat-home.md` に受け入れ条件ごとに記録する。統合後 HEAD `21f38050` で `bash scripts/dev/test-parallel.sh`（nextest 141 + doc 10 binaries、4355 passed / 0 failed / 14 ignored）・`cargo clippy --workspace -- -D warnings`・web の typecheck/lint/test（vitest 490 + server 57）/build・e2e 全体（functional 278 passed / 8 skipped）・e2e チャット（31 passed）・boundaries/parity/secrets/mobile-audit・文書検査 4 本（ADR 採番 149 files・docs layout・docs links・progress index）が exit 0。

- 3 経路（一次対応 A: 人不要・B: 人必要・C: CoS 不在）と通知一本化の試験は「導入順と受け入れ条件」の表の cos-run 担当行に通り、`cos_chat_triage_{a,b,c,unified,fallback}_*` の 24 件が全て本 HEAD で通過（`agent-docs/progress/2026-10-06-cos-run/triage.md` の対応表）。通知は **CoS の escalation と不在退避・管理者の明示 test だけ**が送信入口であり（`notify::spawn_send` の call-site 2 か所）、ADR-0037/0050/0133 の直接通知は通常時に送信 0 を `cos_chat_triage_unified_only_cos_escalation_reaches_the_webhook`・`unified_no_direct_outbox_for_any_notice_kind` で検証済み。
- 実装と食い違う点・縮退・未実装は、上の cos-run 付記（D2/D3 各小節）と web-chat 付記（D5/D6 各小節）の「差分」に集約される。D3「replan・pause/resume・KB accept」の allowlist 未登録については、2026-10-09 に [全 API 変更操作の CoS 監査経路 ADR](2026-10-09-cos-operations-all-mutations.md) で追加方針と例外の人の判断を確定した（cos-run 付記 D3 参照）。設計判断は解消済み、実装と検証は後続 WorkUnit。
- 本付記は実装を修正しない（close-out の範囲は検査と文書）。

## 付記: 実装との突き合わせ（attach-handoff、2026-10-07）

cos-run 付記の未解決だった「添付→task 入力 manifest と KB provenance の連結」は**解けた**（下の D4 各文）。統合後 HEAD `756f3371` で `bash scripts/dev/test-parallel.sh`（4597 passed / 0 failed / 14 ignored）・`cargo clippy --workspace -- -D warnings` が exit 0。証拠は `agent-docs/progress/2026-10-05-cos-chat-home/attach-handoff.md`。

### D4「task へ pin → task の入力 manifest」「read-only に stage、hash 照合」「元チャットの run path を使わない」

- 実装: `crates/task-dispatch/src/dispatcher/input_attachments.rs` の `stage_task_input_attachments`（呼び出しは `worker_task.rs` の run 組み立て、`Dispatcher::input_attachment_source` が CoS chat と同じ data dir・DB を指す）。`chat_attachment_refs` の owner_kind=`task`・owner_id=その task を `task_core::chat::attachments` の `list_for_owner`（pin 順）で読み、`read_verified` が `<data_dir>/chat/attachments/<id>/blob` を DB の sha256・size と照合してから `<task_dir>/attachments/<id>/<name>`（WU の run は `<task_dir>/wu/<key>/attachments/…`、作業ツリーの外）へ dir 0500・file 0400 で stage する。名前は `safe_filename`、symlink は `reject_symlinks` で拒否（CoS stage と共通）。manifest は `task_worker::protocol::InputAttachment`（id・name・media_type・size_bytes・sha256・path・delivery、`docs/protocol/worker-protocol.schema.json`）として `RunContext.input_attachments` に載り、`task-worker/src/preamble.rs` が前置きの入力 manifest に出す。元チャットの run path・thread workspace は参照しない（原本は data dir の blob）ので、chat run の一時 file を消しても後続 run が読める。
- 試験: `cos_chat_attach_handoff_task_run_stages_pinned_attachment`（stage・0400・manifest・チャット workspace を消しても読める）、`cos_chat_attach_handoff_task_run_hash_mismatch_not_delivered`（blob 改ざんは渡さず `unavailable`）、`cos_chat_attach_handoff_task_run_without_pins_is_unchanged`、`cos_chat_attach_handoff_remote_run_marks_unavailable`（以上 `task-dispatch/src/dispatcher/tests/cos_chat_attach_handoff.rs`）、`cos_chat_attach_handoff_preamble_lists_input_manifest`（`task-worker/src/preamble/tests.rs`）、`cos_chat_attach_handoff_list_for_owner_returns_ready_pins_in_pin_order`（task-core）。
- 差分（縮退・未実装）: (a) **子孫 task には継がない**。pin はその task の入力で、子には CoS が子 task へ pin する。(b) **planner と reviewer の run には stage しない**（planner は `spawn_worker` が除外、reviewer は別経路 `review.rs`）。作業する run（atomic task の run と、その task の WU の run）だけ。(c) **ssh remote 実行の run には stage できず**、manifest に `delivery=unavailable` と理由で載せる。(d) 照合失敗・欠落も同様に `unavailable`（読めたと装わない）。(e) 画像の native image content への接続は chat run 側（cos-run 付記）のままで、task run では path と manifest を渡すだけ。(f) stage 先の掃除（task 完了後の削除）は未実装で、task_dir の寿命に従う。

### D4「PDF は KB inbox candidate を作って pin し、候補が provenance を保持」「候補の承認は既存 KB 操作」

- 実装: `crates/task-api/src/knowledge.rs` の `KnowledgeCandidateProvenance` と `candidate_provenance`（`task_core::chat::attachments::provenance_for_owner(conn, "knowledge_inbox", id)`）。`GET /knowledge/inbox`（一覧）と `GET /knowledge/inbox/{id}`（詳細）の候補に `provenance[]`（attachment_id・name・media_type・size_bytes・sha256・thread_id・message_id・request_text・pinned_at）が付く。候補は DB 行でなく KB の `_inbox/<id>.md` ファイルなので、**migration なしで** `chat_attachments`・`chat_attachment_refs`・`chat_messages` を join して読む（候補ファイルは書き換えない）。pin の owner 検査は `task-api/src/chat/attachments.rs`: owner の task／候補が無ければ REST・CoS 経由とも 404。承認は既存の `accept` 等のまま（変更なし）。スキーマと生成型（`docs/api/v1/api-v1.schema.json`・`web/api/generated/`・`gui/app/celeris/types.ts`）を再生成済み。
- 試験: `cos_chat_attach_handoff_kb_candidate_keeps_provenance`（一覧・詳細に sha256・thread_id・request_text が出る。chat の file を消しても残る）、`cos_chat_attach_handoff_pin_missing_owner_404`（以上 `task-api/tests/cos_chat_attach_handoff_kb.rs`）。
- 差分: (a) provenance は API の読み取りで合成する表示情報で、承認後に KB 正本ページへ書き写さない（accept 後の出典欄は未実装）。(b) 添付がメッセージに載っていない pin は `message_id`・`request_text` が無い。(c) web/gui の候補画面に provenance を出す UI は未実装（API と生成型まで）。

### D4「owner の作成成功と pin の完了を確認してから引渡し済みと表示」

- 実装: CoS 側の経路は新 API でなく既存の references API（`POST /api/v1/chat/attachments/{id}/references`）を監査付き操作 `attachment.reference`（`task-api/src/cos/operations.rs` の `ALLOWED`、`chat/attachments.rs`）で包む。手順（owner 作成→応答確認→pin→応答の `state: applied` と `attachment_id`/`owner_kind`/`owner_id` を確かめてから「引き渡し済み」と言う）は `config/skills/cos-operator/SKILL.md` §3a と `task-worker/src/cos_chat.rs` の前置きに書いた。人向けの「引渡し済み」表示は CoS が応答を確認して文で言う規則であり、dispatcher や UI が自動表示する機構ではない。
- 試験: `cos_chat_attach_handoff_cos_pins_attachment_to_created_task_and_kb_candidate`（`task-api/tests/cos_chat_attach_pin.rs`。直接 route は 422 `cos_audit_context_required`、owner 不在は 404、同じ operation の再送は行を増やさない）、`cos_chat_attach_handoff_cos_preamble_explains_pin`（`task-worker/src/cos_chat/tests.rs`）。
- 差分: (a) 前置き・skill の規則は LLM が守る前提で、決定的に強制するのは API 側の 404/409/422 だけ。(b) KB 取り込みの accept の allowlist 未登録は、[全 API 変更操作の CoS 監査経路 ADR](2026-10-09-cos-operations-all-mutations.md) で追加方針を決定済み（実装は後続 WorkUnit）。

## 付記: 差し戻し修正と実機確認（close-out 2、2026-10-07）

final review の差し戻し（不具合 1: cos_chat run が result.json 不在で failed、不具合 2: CoS run の celerisctl が本番の設定へ向かう、添付の引き継ぎ未実装）を 3 つの葉で直し、patch なしの tree で実機確認をやり直した。統合後 HEAD `21763506` で `bash scripts/dev/test-parallel.sh`（4607 passed / 0 failed / 14 ignored）・`cargo clippy --workspace -- -D warnings`・文書検査 4 本が exit 0。証拠は `agent-docs/progress/2026-10-05-cos-chat-home.md` と `…/2026-10-05-cos-chat-home/{result-fix,api-env,attach-handoff,live-check,ops-docs2}.md`。

### D2「CoS chat run は result.json を必須にしない」（commit `f184f88b`、統合 `5664ef5f`）

- 実装: CoS chat の前置き（`task_worker::cos_chat::build_prompt`）は result.json を求めず返事を本文で流す。adapter も `context.cos_chat` の run では result.json を必須にしない。claude-code（`claude_code.rs` の `terminal_from_result`）は success で result.json が無ければ `result` の本文（空なら流した assistant text）で Done、失敗は failed のまま、result.json があればそれが勝つ。acp（`agent_message_chunk` の連結）・pi（最後の assistant message）も同様。codex は既に最後の `agent_message` で Done になるので変更せず試験で固定。cos_chat でない run は従来どおり result.json 必須。
- 試験: `cos_chat_harness_claude_done_without_result_json`・`…_claude_error_without_result_json_stays_failed`・`…_claude_non_chat_run_still_requires_result_json`・`…_codex_done_without_result_json`・`…_codex_failed_turn_without_result_json_stays_failed`・`…_acp_done_without_result_json`・`…_pi_done_without_result_json`（task-worker）、`cos_chat_harness_e2e_claude_without_result_json_completes`（task-dispatch。偽 claude は result.json を書かず、run は completed・reason なし）。
- 実機: patch なしの tree `5664ef5fb20e` で claude-code の 5 往復がすべて completed（new→resumed、同じ node_sessions row）。

### D3「CoS run の API 先は daemon 自身」（commit `3dd632af`、統合 `57814d95`）

- 実装: `dispatcher/cos_chat/launch.rs` の `cos_run_env` が run の env に `CELERIS_API_URL`（`[api] listen` から作る `http://127.0.0.1:<port>/api/v1`）と daemon 自身の dir を先頭にした `PATH` を入れる。`celerisctl` の優先順は `--api-url` > `CELERIS_API_URL` > `CELERIS_CONFIG`（`commands/cos_ops.rs` の `api_config`）。
- 試験: `cos_chat_run_launch_sets_api_url_env_and_path`、`cos_chat_ops_ctl_prefers_api_url_env_over_config`。
- 実機: `--api-url` なしの `celerisctl add` が試験用 daemon に届き、events に `cos_operation` actor=cos（thread_id・run_id・reason つき）が残った。本番 API には届いていない。
- ops: KB の `[knowledge] root` の `skills/` に `cos-operator`・`cos-inbox-triage` が無いと CoS は unavailable になる件は挙動を変えず、`docs/ops/cos-chat.md` に配置手順を書いた（ops-docs2、統合 `21763506`）。

### D4 添付の引き継ぎ（attach-handoff 付記のとおり、統合 `d5b98376`）と実機の結果

- 試験は attach-handoff 付記のとおり通るが、**実機では (d)(e) が未達**:
  - (d) screenshot を pin した task: pin は成立したが、起票した task はすぐ ready になり、作業 run が pin の約 4 秒前に始まったため入力 manifest に載らなかった（**D1**。skill §3a の「起票してから pin」が dispatcher と競合）。
  - (e) PDF の KB inbox candidate: CoS に監査つきで KB 候補を作る経路が無く（`celerisctl knowledge record` は CoS credential で拒否、`POST /knowledge/inbox` は `cos_operation_not_allowed`）候補は作られなかった（**D2**。scope の書式も skill と CLI で不一致）。
- 他の実機の指摘: **D3** `cos-inbox-triage` skill が求める confidence を `ResolveBody` が受けない（reason に書いて回避）。**D4** triage thread で終わった run が `orphan takeover` で interrupted になり続きの run が重複して起きた（`cos_chat/control.rs`）。
- 未解決: 不具合 4（短い往復で `summary_through_seq` が 0 のまま）と 5（provider に model を書かないと `chat_runs` の実効 model が null）。いずれも実装は変えていない。
- 本付記は実装を修正しない。D1〜D4 の修正方針は進捗の提案節に記す。

## 付記: D1〜D4 の修正と実機再確認（close-out 3、2026-10-07）

close-out 2 付記で実機 FAIL だった D1〜D4（(d)(e) 未達の原因）は、ADR `agent-docs/adr/2026-10-07-cos-live-fixes.md` の決定どおり直った。live-fixes の tip `767a15760901` で、運用セッション（Claude Opus 5.5）が試験用 daemon（本番でない shell・claude_oauth・data dir は `/var/tmp`）に `scripts/dev/cos-chat-live.sh full` を流し、**(a)〜(e) がすべて PASS**した（証跡は `agent-docs/progress/2026-10-05-cos-chat-home/live-check.md`「実機確認（D1〜D4 修正後）」）。

- **D1（作成時 attachment_ids）**: `POST /tasks`（と CoS operation `task.create`）が `attachment_ids` を受け、task の作成と pin を 1 transaction で行う。実機 (d): 最初の作業 run の `prompt.txt` に「## 入力の添付」と読み取り専用の screen.png が載った。
- **D2（knowledge.record operation）**: `POST /knowledge/inbox` を `/cos/operations` の `knowledge.record` として許可し、scope は `project:<slug>`。実機 (e): 候補が `_inbox/` に作られ、PDF の provenance・pin・events の `cos_operation`（actor=cos）がそろった。
- **D3（confidence）**: `ResolveBody.confidence`（0..=1）を足した。実機: `cos_operations` の result に `"confidence":0.97`。
- **D4（takeover）**: 原因は `ChatRunSink::write` が Conflict を終端と誤読したことで、終端 run を takeover しない。実機: triage thread の `chat_runs` は 1 件、takeover/orphan は 0 件。
- 統合後 HEAD で `bash scripts/dev/test-parallel.sh`（4660 passed / 0 failed / 14 ignored）・`cargo clippy --workspace -- -D warnings`・文書検査が exit 0。gui-api の節番号は 3.127〜3.131 で重複が解消済み。
- 未解決（実装は変えていない。進捗の提案節）: 不具合 4（`summary_through_seq` が 0 のまま）、5（provider に model が無いと実効 model が null）、台本の残り（knowledge init・project fixture・LEFTOVER の誤報・FAIL でも exit 0）、`attachment_pin_rules` の前置きが旧手順のまま。

## 付記: rollover の token 会計を context 占有へ是正（cos-chat-prompt-cache T4、2026-10-08）

仮説 H4（`agent-docs/adr/2026-10-08-cos-chat-prompt-cache.md`）: 従来の `usage_tokens` は `input+output` を `node_sessions.approx_tokens` に**累積**して `[sessions] rollover_tokens` と比べていた。この値は (a) cache_read/cache_creation を含まず、(b) 1 run 内の全 API 呼び出しの合算なので、その時点の context 長とも課金相当の入力とも一致しない（cache 中心の run で大きく過小評価する）。

### D1 占有が取れるか（harness 別）

- **claude-code: 取れる。** `result.usage` は run 内の全 API 呼び出しの合算（占有にならない）。一方 stream-json の各 `assistant` 行の `message.usage`（`input_tokens`・`cache_read_input_tokens`・`cache_creation_input_tokens`）はその 1 呼び出しの値なので、**最後の main thread の assistant 行**の 3 値の和が、その run の終わりの context 占有。sub-agent の行（`parent_tool_use_id` が非 null）は別 context なので数えない。`ExplorationTracker::observe_assistant_usage` が観測し、`annotate` が `result.usage.context_tokens` に載せる。
- **pi: 取れる（cache は input の外数で確定）。** `message_end` の assistant message の `usage.input + cacheRead + cacheWrite` の最後の値（`PiStream`）が占有。出典: pi-ai 0.84.2（`~/.local/celeris/npm/pi-0.84.2/lib/node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-ai/dist/`）の `api/anthropic-messages.js:398-401`（input=input_tokens、cacheRead=cache_read_input_tokens、cacheWrite=cache_creation_input_tokens）、`api/openai-completions.js:1106-1125`・`api/openai-responses-shared.js:441-447`（input=prompt から cached・cache_write を差し引き）、`utils/estimate.js:3-4`（calculateContextTokens = totalTokens || input+output+cacheRead+cacheWrite）。
- **codex: 取れない（None）。** `turn.completed.usage` は turn 合算で呼び出し単位の値が無い（`cached_input_tokens` は input の内数）。**acp / aider も None。** 実機で確認していないので、これらが占有を持つかは「不明」。
- 取れない場合は D3 の fallback。

### D2 保存と判定

- `Usage.context_tokens: Option<u64>`（追加のみ。省略可）。API schema・gui/web 生成型に反映。
- migration `0062_cos_chat_context_tokens`（schema 62）で `node_sessions` に `last_context_tokens INTEGER NULL`（最後に観測した占有。**置き換え**で保存、累積しない。占有を報告しない run は直前の値を残す）と `billed_input_tokens INTEGER NOT NULL DEFAULT 0`（run ごとの harness 別の課金相当入力の累積（定義は D4）。**情報用で判定に使わない**）を足す。`approx_tokens` は従来の `input+output` 累積のまま残す。
- CoS chat の rollover 判定は `rollover_measure(session) = last_context_tokens ?? approx_tokens` が `rollover_tokens` 以上なら次 run を fresh（`token_rollover`）。cache token を累積して context 長とみなさない。
- **config 名 `[sessions] rollover_tokens` は保つ**（既定 400,000）。意味は「CoS chat では context 占有の閾値」（`crates/celeris/src/config/dispatch.rs` の注釈に明記）。WU・review の継続セッション（`sessions.rs`）は従来どおり `approx_tokens` の累計で、挙動を変えない。

### D3 互換と fallback

- 既存行（0062 未適用）は `last_context_tokens = NULL` なので `approx_tokens` で判定し、従来と同じ結果になる。旧い run の usage（`context_tokens` 欠落）も同じ。
- 占有を報告しない harness（codex・acp・aider）は、`last_context_tokens` が NULL のまま `approx_tokens`（`input+output` の累積）へ fallback する。この場合は従来どおり cache を含まない過小評価が残る（cache 込みの値が取れないので、是正できない）。

### D4 課金相当入力の harness 別定義（billed-input-fix）

worker 一般の `Usage` と既存パーサは変えず、CoS の `session_usage` に実行した `WorkerAdapter::id()` を渡す。`billed_input_tokens` は金額ではなく入力 token の累積で、rollover 判定に使わない。

- **claude-code（外数）**: `claude_code.rs` の result パーサは Anthropic の `input_tokens`（cache を含まない）、`cache_read_input_tokens`、`cache_creation_input_tokens` を個別に保持する。`billed_input = input + cache_read + cache_creation`。
- **codex（内数）**: `codex.rs` の `turn.completed` パーサは `input_tokens` をそのまま、内数の `cached_input_tokens` を `cache_read_tokens` にも保持する。`billed_input = input` のみ（cache を足さない）。既存 fixture の input=10 / cached=4 / output=20 は billed_input=10、fallback 加算=30。
- **acp / opencode（不明）**: `acp.rs` の `PromptResponse` 処理は安定版に token usage フィールドが無く、常に `usage: None`。内数・外数は判定不能。現在は入力計上なし。将来 usage が来ても、定義を確認するまで保守的に input のみとする。
- **pi（外数・確定）**: `pi.rs::PiStream::event` は assistant の `message_end.usage.{input,output,cacheRead,cacheWrite}` を個別に合算する。pi-ai は provider 差を正規化し `usage.input` は cache を含まない外数なので、`billed_input = input+cache_read+cache_creation`（claude-code と同じ）。占有（最後の呼出しの input+cacheRead+cacheWrite）と累積入力の両方が確定。試験 `cos_chat_pi_cache_is_external_occupancy_and_billed_input`（input=100,cacheRead=40,cacheWrite=5 を 2 呼出し: 占有 145、累積入力 290）。出典: pi-ai 0.84.2（`~/.local/celeris/npm/pi-0.84.2/lib/node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-ai/dist/`）の `api/anthropic-messages.js:398-401`（input=input_tokens、cacheRead=cache_read_input_tokens、cacheWrite=cache_creation_input_tokens）、`api/openai-completions.js:1106-1125`・`api/openai-responses-shared.js:441-447`（input=prompt から cached・cache_write を差し引き）、`utils/estimate.js:3-4`（calculateContextTokens = totalTokens || input+output+cacheRead+cacheWrite）。
- **その他の定義不明の harness**: input のみ。欠落値は 0、加算と i64 変換の飽和処理は維持する。

### 試験

`cos_chat_billed_input_codex_cached_input_is_not_added_twice`・`cos_chat_billed_input_claude_adds_external_cache_tokens`・`cos_chat_billed_input_unknown_cache_definition_uses_input_only`（dispatcher）で会計と context/fallback の分離を確認する。

`cos_chat_run_rollover_measure_prefers_context_occupancy`・`…_falls_back_for_legacy_rows`（`sessions/cos_chat/tests.rs`）、`cos_chat_run_session_touch_separates_occupancy_from_cumulative`（task-core）、`cos_chat_run_rollover_cache_heavy_run_is_not_underestimated`・`…_occupancy_is_replaced_not_accumulated`・`…_unknown_occupancy_falls_back_to_cumulative`（dispatcher 結合）、`context_tokens_is_last_main_thread_call_not_result_sum`・`…_unknown_without_assistant_usage`（task-worker）。実機（LLM 呼び出し）での占有の実測は本葉では行っていない（「不明」）。

## 付記: resume 時の差分配送（cos-chat-prompt-cache T6、2026-10-08）

D2「再開中は未配送 seq 以後の差分と現在の規則だけを渡す」の実装。従来の `launch.rs` は session_mode に関係なく `history_since_summary` を呼び、resume でも summary と未要約履歴を毎回渡していた（prompt-cache ADR の H3）。

### D1 配送 cursor

- migration `0063_cos_chat_delivered_through`（schema 63）で `node_sessions.delivered_through_seq INTEGER NOT NULL DEFAULT 0` を足す。「この session に配送済みの thread seq の水位」。0 は不明（既存行・まだ completed の run が無い）。
- 上げるのは `run_claimed` の終端の後だけ（`launch::advance_delivery_cursor`）。DB に記録された run の状態が `completed` のときに、run の session 行（`chat_runs.session_row_id`。fresh 再試行なら新しい行）を `MAX(cursor, 入力の seq)` に上げる。入力と同じ run の返事（claim 時に挿入）の間に user 以外の発言が無ければ返事の seq まで上げる（間の user 発言は待ち行列の入力で、cursor に関係なく入力として配送される）。間に system などがあれば入力の seq で止め、次の resume で再配送する。`store.chat_session_set_delivered_through` は下げない・retire 済みの行は動かさない。
- 試験 `cos_chat_resume_delta_cursor_covers_the_reply_only_over_queued_user_messages`・`…_cursor_stops_at_the_input_before_an_unseen_message`（launch/tests.rs）、`…_cursor_rises_only_on_live_row_and_is_independent_of_summary`（task-core）。

### D2 resume で差分、それ以外は全文

- `rollover::choose_session` が `SessionChoice.delta_from` を決める: mode が resumed、cursor > 0、かつ前回の run（この session の最新の終端 run）が再起動後の回収（`control::ORPHAN_TAKEOVER_REASON`）で終わっていないときだけ `Some(cursor)`。
- `rollover::history_for_run`: `Some(cursor)` なら summary を省き、cursor より後で入力より前の発言だけを `unsummarized` に入れる（summary の水位は checkpoint の期待値に要るので thread の値をそのまま渡す）。`None` なら従来の summary＋未要約履歴。
- worker protocol `CosChatContext.delivered_through_seq: Option<i64>`（追加のみ・省略可。`docs/protocol/worker-protocol.schema.json` を再生成）。worker の prompt は、ある時「## これまでの要約」を「要約は session 内。水位 seq N。」の 1 行にし、「## 前回の配送以後の発言 (seq a..=b)」を出す。入力・規則（返事と操作・checkpoint・履歴 API）は従来どおり毎回渡す。
- 全文の場合: new、fresh（key 変更・cache 消失・context 枯渇・token rollover。新しい行の cursor は 0）、resume 拒否後の fresh 再試行（`prepare_fresh_retry` が `delivered_through_seq = None` にして全文を組み直す）、再起動後の回収、cursor 0。
- 試験: `cos_chat_resume_delta_resumed_run_gets_only_messages_after_the_cursor`・`…_fresh_reasons_deliver_the_full_text`（4 理由）・`…_refused_resume_retries_fresh_with_the_full_text`・`…_restart_recovery_delivers_the_full_text`（harness_tests.rs）、`cos_chat_resume_delta_prompt_omits_summary_and_names_the_cursor`（task-worker）。

### D3 入力の必達と失敗時

- 入力（割り込み・待ち行列・回収の継続）は `inputs` で cursor に関係なく渡す。配送記録は従来どおり `input_message_id`・`message.state`、checkpoint の水位 `summary_through_seq` は別管理。cursor より前の seq の待ち入力（割り込みが追い越した発言）も入力として届く（`…_interrupt_and_queued_inputs_are_always_delivered`）。
- completed にならなかった run（failed・interrupted・stopped・終端前の process 消失）は cursor を動かさない。次の resume は古い cursor から再配送する（再配送は許し、欠落は許さない。`…_failed_run_keeps_the_cursor_and_redelivers`）。checkpoint の水位と cursor は互いに動かさない（`…_checkpoint_watermark_and_cursor_are_independent`）。

### 計測（隔離 live、claude-code・claude_oauth、同一 thread 10 turn × before/after 各 2 回）

**旧計測（account 不明。今回の固定計測とは別の参考値）**: `scripts/dev/cos-chat-bench.sh … same-thread`（S2 だけを流す mode を追加）。t2〜t10 合計の 2 回平均: 非 cache input（`Usage.input_tokens`）50 → 50（±0%、turn ごとに同じ 4〜6）、cache write 51,647 → 44,630（−13.6%）、cache read −5.6%、prompt bytes 55,064 → 43,457（−21.1%。after は turn によらず約 4.8 KB で一定）。claude-code では prompt は cache write に入り、非 cache input は構造上ほぼ一定なので、この指標では改善が出ない。計測時は CoS が checkpoint を書かず summary が空だった（既知の不具合 4）ので、before の再送は未要約履歴だけだった。summary がある thread の差はこれより大きい見込み（未計測）。codex・acp・pi は未計測（不明）。

### 判定指標（人の決定、2026-10-08）

統合の判定は 2026-10-08 の人の決定（(a) の方向性）で「非 cache input・cache write・prompt bytes の before/after が記録され、cache write と prompt bytes のどちらかが改善している」に変更された。旧計測は cache write −13.6%、prompt bytes −21.1% だった。今回の明示account固定再計測でも改善を確認した（下段）。非 cache input は判定指標に採用しない（claude-code では prompt が cache write に入り非 cache input は turn ごとの 4〜6 で構造上一定のため、改善が出ない）。


### subscription account 固定再計測（2026-10-08、lab-bench）

- 人の回答「claude_max_labは再ログインしましたが、CoSはlabアカウント固定である必要はないです。personalアカウントのbudgetも使用して構いません」に従い、account 限定を変更した。更新済み lab は2ターン成功後、週間利用率98%（選択上限97%）で選択できなくなったため、本計測4セットは `claude_max_personal` に明示固定した。lab の名前への付け替えはしていない。
- 計測 account の受け入れ条件は、人の上記回答を出典とする許可集合 `{claude_max_lab, claude_max_personal}` から1 accountを明示固定し、各 set 内で一致すること。全 run が `llm_source=claude_oauth`・`state=completed` で、各 `summary.meta.account_id` が set の実 account と一致することを確認する。今回の4 set 40 run はすべて `claude_max_personal` でこの条件を満たす。既存の実測を再検証して使用し、account_id の書き換え・再計測は行わない。
- `COS_CHAT_BENCH_LAB_DIR` は従来どおり basename `claude_max_lab` 以外を拒否する。追加した `COS_CHAT_BENCH_ACCOUNT_DIR` は明示した subscription account の実 basename を保持する。両指定の同時使用・無効な account id を拒否。`.credentials.json` だけを隔離 data dir の `accounts/<account_id>/` に mode 600 で複製し、provider の `account_pool = true`、CoS の `account_id`、`accounts.claude_dir` を設定する。session cache も `CLAUDE_CONFIG_DIR=<data dir>/claude-config` に隔離する。
- before は開始時の main `10566e5bc7666a18c0850fabb1c177aef4472483` を `$TMPDIR` に `git archive` して debug build。after は branch `5e65a547521aff0ce8b389e73a8b27df0983d29f` の debug build（製品コードは merge-main `fe5d609d` と同じ）。渡された `CARGO_TARGET_DIR` と compiler 設定を使用。台本と bench script は両方とも `5e65a547521aff0ce8b389e73a8b27df0983d29f`。port 17957、一時DB、隔離 data dir で計測した。
- main は計測中に `fb300299` へ進んだ。before の呼び手が記録した `summary.meta.commit` はその可変 ref だが、実バイナリは開始時の archive build のまま。実出所はバイナリ SHA-256 で固定し、`summary.meta.binary_source_sha` と `binary-provenance.json` に記録。元の `meta.commit` は監査のため残した。
- before → before-2 → after → after-2 と、成功した直前の隔離認証コピーを引き継いだ。**40 runすべて completed・account_id=claude_max_personal・llm_source=claude_oauth**。各 `summary.meta.account_id` も `["claude_max_personal"]`。account_id は実際のAPI run行から取得し、設定値で補完していない。全4セットでt1はnew、t2〜t10はresumed。beforeのDBには配送cursor列がなく、afterには存在することを確認。
- t2〜t10 **合計の2回平均**: 非 cache input 51.0 → 54.0（+5.88%）、cache write 40,377.0 → 36,487.5（-9.63%）、prompt bytes 75,254.5 → 64,258.5（-14.61%）。判定: **改善あり**（cache write か prompt bytes の改善という人の指定条件を満たす）。統合は delivery の後続段に委ね、本 run では main への統合・本番操作を行っていない。
- 最初のpersonal試行は host の session cache が読取専用で全fresh_after_refusalとなり、afterの依存ビルドもmainのschema62を再利用していたため無効。session cache隔離と変更対象crateのclean build後に4セットを再実行した。same-threadはfailed turnに加え、t2以降がresumedでない場合も停止する。無効試行は `invalid-fresh-*/`、lab停止は `lab-quota-before-attempt3/`、前attemptの認証切れは `failed-before-attempt2/` に分け、本平均に含めない。
- host lab/personalの認証のSHA-256・mtime・sizeは前後不変。hostの認証・本番config/DB/release/systemdを変更していない。隔離認証コピーは計測後に削除。summaryありthread・他harnessの効果は不明。
- 証跡: task `01M4DE3G78D16NJ80SEMWVAVAK` の `wu/lab-bench/artifacts/resume-delta-bench-lab/` に `{before,before-2,after,after-2}/runs.json`、各 `summary.json`、`compare.md`（全runのaccount_idとcommit出所）、`validation.json`、`binary-provenance.json`、`host-integrity.json`。
- 検証: CoS chat 152件、resume差分配送12件、workspace clippy、shell構文、引数拒否4ケース、driverのcompleted/failed/fresh停止3ケース、文書リンク・progress-indexはすべて成功。
