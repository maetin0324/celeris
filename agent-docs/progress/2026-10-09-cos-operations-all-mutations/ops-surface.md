---
tasks: [01M4F5KS8E1MXZDNJTRAVFJESW]
wu: ops-surface
status: done
completed: 2026-10-09
---
# ops-surface: surface の PENDING 21 行を監査付きで登録

`crates/task-api/src/cos/ops/surface.rs` の `PENDING` は空（21 行を ALLOWED へ。既存の `attachment.reference` と
合わせて ALLOWED 22 行）。`/console/instruct` と browser の credential/attestation 系は EXCLUDED のまま。
これで全領域の `PENDING` が空になった。commit 30100aa6・d361eeed と、この文書の commit。

## 登録した操作

- A（domain 書き込みと監査を 1 transaction）: `chat.thread_create`・`chat.thread_update`・`chat.message_post`・
  `chat.message_cancel`・`chat.run_stop`・`chat.queue_resume`・`console.new_conversation`。
  task-core に `chat_thread_create_tx`・`chat_thread_patch_tx`・`chat_thread_resume_queue_tx`・
  `chat_message_cancel_tx`・`chat_run_stop_tx`・`chat_legacy_new_conversation_tx` を足し、既存の store 関数
  （route が使う）も同じ `_tx` を呼ぶ。chat のエラーは route と同じ status・code（`chat_problem`）で rejected 行になる。
- CoS の chat 書き込みは連鎖しない: `chat_message_post_cos_tx` は本文を **完了済みの `assistant` 発言**
  （`metadata.posted_by=cos`・`operation_id`）として入れる。CoS run を起こす待ち行列は `role='user' AND state='queued'`
  だけなので、CoS の書き込みは他の thread でも自分の thread でも run を起こさない。待ち行列に作用する
  `mode=interrupt`・`resume_queue=true` は 422 `validation`。CoS ノード自身への `POST /org/cos/messages` は
  旧 Console の待ち行列に入って CoS run を起こすので 422 `cos_self_chain`（記録あり）。
- C（2 段監査。pending → applied、再送は再実行しない）:
  - file: `chat.attachment_upload`（multipart の代わりに `{"client_upload_id","name","content_base64"}`。中身は
    記録に残さず byte 数だけ）、`chat.attachment_delete`。attachment store の拒否（上限・衝突・不在・不正 path）は
    rejected、I/O・DB の失敗は結果不明として pending。
  - run の起動: `org.message`（対話用 task を作る。route と `conversation::start_conversation` を共有）。
  - docs git: `artifact.promote`（`docs::promote_op` を route と共有し `docs_effect` を通す。`page_exists`・
    `artifact_not_found`・`docs_unavailable` を変更前の拒否に足した）。
  - browser: `browser.site_policy_put/delete`・`browser.task_policy_put`・`browser.request_open`・`browser.control`・
    `browser.control_disconnect`・`browser.agent_begin/end`・`browser.auth_section`・`browser.live_event`。
    broker・worker・Live View が読む状態なので objective どおり C にした。いずれも SQLite の 1 transaction の
    write で、失敗は何も変えないので `cos::operations::external_store` で rejected に settle する。
- browser の検査は route と同じ: control は owner session の署名付き assertion（task・run・session・期限）を
  `browser_live::verify` で確かめ、owner でなければ 403 `not_owner_session`、lease の holder は assertion の
  owner session（CoS は holder にならない）、resume は assertion の `origin_ok` も要る（無ければ 422
  `resume_not_verified`）、他の session の holder の renew は 403 `not_lease_holder`。`agent/*`・`auth-section`・
  live event は daemon 認証のある構成だけ（route と同じ）。記録の payload では `assertion` を
  `{"redacted":true}` に伏せる（request hash は元の本文）。
- 除外の追加（人の決定 secrets=exclude の適用）: `browser.request_open` で `credential`・`credential_policy_id`・
  `trusted_login` を含む待ちは credential 系なので 422 `browser_credential_attestation`。click/download などの
  operation 承認の待ちは登録済み。
- handler の共有化: `browser::open_wait/parse_task_policy/set_task_policy`、`browser_site_policies::validated_policy/
  upsert_policy/delete_policy`、`browser_control::control_request` と `*_in`、`browser_live::target_in/event_checked/
  append_in`、`console::new_conversation_project`、`conversation::check_node/start_conversation`、
  `chat::attachments::delete_effect`。route の挙動（status・code・順序）は変えていない。
- celerisctl: surface の route を書く専用 subcommand は無い。`celerisctl api-request` は登録済みの任意の変更を
  `/cos/operations` で送るので、そのまま使える。`cos_mapped` の追加は無し。
- 文書: skill `operations.md` の surface 表（lib 試験 `cos_operator_skill_table_matches_allowed` で ALLOWED と一致）、
  登録待ちの節を削除、`SKILL.md` の「登録待ち」の案内を除外だけに直した。`docs/api/v1/gui-api.md` §3.129 の表に
  surface 21 行と「PENDING は全領域で空」を追記し、`scripts/sync-gui-docs.sh` で gui 写しを同期。
- 依存: `base64` を task-api の dev-dependencies から dependencies へ移した（lock は既存の 0.22）。

## 試験

`crates/task-api/tests/cos_ops_ops_surface.rs`（一時 DB・local git・fake の署名鍵。userns・外部 network なし）:

- `..._chat_writes_are_audited_in_one_transaction`: chat A 6 route と console を `run_domain`（直接 422 + rejected
  監査、経由で applied + 監査 event + chat card）。古い revision は 409 `chat_conflict` で rejected、改名されない。
- `..._cos_message_does_not_chain_a_cos_run`: 人の thread と CoS 自身の thread への書き込みが assistant/completed で、
  queued の user 入力 0、`chat_run_claim_next` が None（自分の run を終えた後も）。interrupt は 422、
  `POST /org/cos/messages` は 422 `cos_self_chain` で task が増えない。
- `..._files_runs_and_docs_are_external`: upload/delete・`org.message`・`artifact.promote` を `run_external`
  （直接 422、pending → applied の 2 event、同じ key の再送は同じ operation）。upload の中身が記録に無いこと、
  再送で対話 task が 1 件のままであること、promote が main に commit されたことを確かめる。
- `..._browser_policies_and_requests_are_external`: site policy put/delete、task policy、operation 承認の待ち。
  credential を含む待ちは 422 `browser_credential_attestation`。
- `..._browser_control_keeps_owner_origin_and_lease_checks`: agent begin/end、pause・takeover（holder=owner）、
  非 owner 403・他 session の assertion 403・他 owner の renew 403 `not_lease_holder`・origin 不一致の resume 422、
  disconnect・auth-section・live event。記録に `signature` が残らないこと。

## 証拠

- `cargo nextest run -p task-api --test cos_ops_ops_surface` → 5 passed。
- `cargo nextest run -p task-api --lib skill_table` → 1 passed（skill 表 = ALLOWED）。
- `TMPDIR=/tmp cargo nextest run -p task-api -p task-core` → 1536 passed, 0 failed。
- `TMPDIR=/tmp cargo nextest run -p task-api -E 'binary(/^cos/) or test(/cos/)'` → 107 passed。
- `cargo clippy -p task-core -p task-api --all-targets -- -D warnings` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0、`passed 4946, failed 0, ignored 14`、nextest 176.8 s。
  run の既定 TMPDIR（長い path）では browser の Unix socket 試験が SUN_LEN で落ちる既知の事情のため `/tmp` で流した。

## 未解決

- C にした browser の DB write（site policy・task policy・待ち・control）は tx1 と tx2 の間に domain write が入る
  （2 段の契約どおり）。A の 1 transaction にするには browser store の各関数に `_tx` 版が要る。
- `chat.run_stop` の stop 理由は route と同じ「stopped by human」の文言（`chat_run_stop_tx` を共有するため）。
  CoS が止めたことは監査行・chat card に残る。
- ops-closeout: Registry の `pending` 欄と各領域の空の `PENDING` 定数の撤去、registry 試験の「ALLOWED か EXCLUDED の
  一方だけ」への変更、ADR 3 本の状態・付記の更新、cos-inbox-triage など他 skill の食い違い確認は closeout の担当。

## 提案

- browser store（`browser_site_policy_upsert/delete`・`browser_task_policy_set`・`browser_wait_open`・
  `browser_control_mutate`）に呼び出し側 transaction を受ける版を作れば、browser の設定系は A にできる。
