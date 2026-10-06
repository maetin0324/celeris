# ADR 2026-10-06: CoS chat run を worker protocol に載せる境界（一時 Task・credential の env 渡し）

---
tasks: [01M47J2YNMZ14NQ4AXTNA355AA]
---

- 日付: 2026-10-06
- 状態: 採用（proto WorkUnit で protocol・前置き・試験を実装済み。dispatcher の起動は後続の launch WU）
- 関連: [ADR 2026-10-05 cos-chat-home](2026-10-05-cos-chat-home.md) D2/D3/D4、[ADR-0003](0003-worker-protocol.md)（protocol は追加だけ）、[ADR-0054](0054-stateful-sessions-and-streaming-chat.md)、[ADR-0013](0013-taskd-api-and-gui-foundations.md) D11（秘密は request に載せない）

## 背景

cos-chat-home D2 は CoS の会話を `chat_threads`/`chat_messages`/`chat_runs` に持ち、run を worker として起動する。既存の worker protocol は `RunRequest.task: Task` を必須とし、全アダプタ（claude-code / codex / acp / aider / fake）が `task` と `context` から前置きを組む。CoS chat run は task ではないので、protocol に乗せる境界を決める必要がある。

## 決定

### D1 一時 Task と判定
- dispatcher は CoS chat run ごとに **保存しない一時の `Task`** を組み、`RunRequest.task` に入れる（`CosChatContext::transient_task`。id は毎回新しい ULID、kind=Execute、workspace=`<data_dir>/cos/threads/<thread_id>/workspace`、budget は `[cos]` の max_turns/max_wall_secs）。この値は `tasks` 表にも events にも書かない。worker から見て task の状態遷移は無い。
- CoS chat run かどうかは `context.cos_chat`（`Option<CosChatContext>`）の**有無だけ**で決める（`task_worker::protocol::is_cos_chat_run`）。題名・objective・label は判定に使わない。`context.cos_chat` を埋めるのは dispatcher の検証済み起動経路だけなので、任意の task が CoS を名乗っても CoS chat run にはならない（ADR-0089 の例外を迂回させない）。
- `context.cos_chat` が `None` の run の `request.json` とプロンプトは 1 バイトも変わらない（serde の skip）。protocol 版数は据え置く（追加だけ）。

### D2 CosChatContext の中身
`thread_id`、chat の `run_id`、配送する入力 `inputs[]`（id/seq/text/interrupt/attachment_ids。割り込みが先頭）、`summary` と `summary_through_seq`、未要約履歴 `unsummarized{from_seq,through_seq,messages}`、添付 manifest `attachments[]`（id/name/media_type/size_bytes/sha256/path/delivery）、mount する skill 名 `skills[]`、`credential_env`、`api_base_url`。
- 未要約履歴は範囲を明示する。`messages` が範囲の一部しか持たない場合、前置きは欠けている seq の範囲を名指しし、履歴 API（`GET /chat/threads/{t}/messages`）で読めと書く（無言で切り捨てない。D2「要約未作成の範囲」）。
- `delivery` は `image`（JPEG/PNG/WebP/GIF。D4 の安全な decoder がある raster）か `file`（それ以外。SVG・PDF を含む）。`CosChatDelivery::for_media_type` で決める。前置きは image を画像として読ませ、読めなければ読めたと答えさせない。
- `summary_through_seq` は配送 cursor とは別（D2）。配送の記録は `chat_runs.input_message_id` と message.state が持ち、この context は入力の写しにすぎない。

### D3 credential は env で渡す
- run credential（ops の `celeris-cos-run.` bearer）の**値**は `CosChatContext` に入れない。値は dispatcher がアダプタの子 process の環境変数 `CELERIS_COS_RUN_CREDENTIAL`（`COS_RUN_CREDENTIAL_ENV`。`celerisctl` と同じ名前）にだけ置き、context には変数名だけを載せる。よって `request.json`・`prompt.txt`・run log に値が出ない。
- 前置きは `$CELERIS_COS_RUN_CREDENTIAL` を参照する curl / celerisctl の書き方だけを示し、値を表示・記録しないよう指示する。

### D4 前置き
- `claude_code::build_prompt`（全アダプタ共通）は `context.cos_chat` があれば `task.kind` に依らず `cos_chat::build_prompt` に回す。共有の前置き（profile・知識・記憶など `preamble::render`）は残すが、旧 CoS 対話の `actions` 指示（`conversation_addressee`）と途中目標レビューの指示は外す。
- 前置きは、返事は本文として書く（そのまま chat に流れる）、旧 `result.actions` を使わない（出すとエラーのカード）、checkpoint API（`POST /cos/threads/{t}/checkpoint`。through_seq は配送済みまで、32 KiB、409 は読み直し）、履歴 API のページ送りを書く。要約は worker が作り、dispatcher は LLM を呼ばない。

## 影響
- 後続 WU: launch は一時 Task と `CosChatContext` を組んで env に credential を置く。attach は stage した manifest をこの型に写す。session は `context.session` を thread 単位で埋める。sink は progress を chat_events に写す。
- `docs/protocol/worker-protocol.schema.json` を再生成し、`worker-protocol.md` の context 表に 1 行足した。
