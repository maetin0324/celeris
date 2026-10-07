# CoS チャットの設定と運用

---
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV, 01M4APB5FP8T3TAAE51Z20E3M1]
---

[CoS チャット ADR](../../agent-docs/adr/2026-10-05-cos-chat-home.md) D1〜D6 の運用手順。本番 config・service・release・DB を変える操作は、以下を読んだ人が本番 host で実行する。導入時は [selfdeploy](selfdeploy.md) の release → verify → promote と、旧 CoS run の drain・停止、DB と添付の一体バックアップを先に計画する。

## 設定

本番の `~/.config/celeris/config.toml` に置く例。既存の節があれば重複させず統合する。`provider`・`account_id`・`model` は登録済みの実値へ置き換える。固定しなければ適合 provider を解決し、継続 session の account を優先する。`model` は `tier` より優先する。

```toml
[cos]
enabled = true
harness = "claude-code" # 既定。ほかに codex / opencode（ACP）
llm_source = "claude_oauth" # 登録済み source。省略可
# provider = "claude-pool"
# account_id = "claude_max_lab"
# model = "<登録済みモデル名>"
tier = "frontier"
max_turns = 70
max_wall_secs = 900
stream_retention_days = 30

[cos.triage]
policy_skill = "cos-inbox-triage"
policy_version = "1"
min_confidence = 0.85
human_required = ["fundamental_change", "external_publish", "destructive", "resource_overrun", "security", "explicit_human"]
unavailable_after_secs = 120

[cos.attachments]
max_file_bytes = 26214400
max_message_bytes = 104857600
max_files_per_message = 10
max_storage_bytes = 10737418240
orphan_ttl_hours = 24
unreferenced_retention_days = 30
```

`llm_source` は `claude_oauth` / `codex_oauth` / `celeris` / `openai_compatible:<id>` などの登録済み source を指定する。harness・source・provider・account・model の不整合は config 検証で拒否される。`[execution] max_cos_runs` の既定は 2。0 は専用並列枠を使わない設定であり、CoS の停止は `cos.enabled = false` で行う。変更は次の run から有効になり、session の照合条件が変われば旧 session を retire して DB の要約・履歴から再開する。quota 切れの固定 account を他 account に自動変更しない。

## 権限と受信箱

CoS は特別 worker として shell・ファイル・git・登録済み MCP・celerisctl・REST・KB・browser/cluster の道具を使う。作業場所は `<data_dir>/cos/threads/<thread_id>/workspace`。登録 repo の編集は専用 worktree を使う。本番 DB、token、service、release をファイル操作で直接変更せず、制御操作は認証済み API/その API を呼ぶ celerisctl に通す。CoS の操作には actor=cos、対象 revision、理由、policy version と operation id が events に残る。秘密は broker 経由で参照し、値や webhook URL をチャット・添付・ログ・Discord に出さない。

新しい通知・質問・decision・approval・plan/phase gate・失敗は CoS の受信箱スレッドへ届く。CoS は判断不要なら理由を残して観測し、既存の指示・認可内なら代答する。設計の根本変更、外部公開・push、破壊的操作、資源上限超過、秘密・権限・セキュリティ、人が指定した人の判断、低確信は人へ回す。対象に付いた `human_required` は config/skill で解除できない。代答はチャットに「CoS が代わりに答えた」と表示され、人は取消・差し戻しを行える。

人への Discord 通知は通常 CoS の escalation だけ。要点・選択肢・推奨と理由・web の該当画面リンクを送る。Discord の返信・リアクションは回答にならないので、リンク先の認証済み web 画面で答える。CoS run の失敗、quota 切れ、無効化、期限超過時は、未解決の待ちを決定的な直接通知へ退避する。Webhook が未設定・送信失敗でも待ちは解決しない。旧 inbox reminder/digest を通常通知として運用しない。通知設定と到達確認は [受信箱と通知](inbox-notifications.md) を参照する。

## CoS skill の配置（導入時に必須）

CoS の前置きは KB の skill を読む。`[knowledge] root` の `skills/` に、repo の `config/skills/cos-operator` と `config/skills/cos-inbox-triage` をディレクトリごと置く。無いと CoS run は `CoS unavailable: CoS skills unavailable` で起動せず、チャットは失敗する。本番の KB への配置は人が行う。

```sh
# <kb_root> は config の [knowledge] root
cp -r config/skills/cos-operator config/skills/cos-inbox-triage <kb_root>/skills/
```

確認: `test -s <kb_root>/skills/cos-operator/SKILL.md && test -s <kb_root>/skills/cos-inbox-triage/SKILL.md`。そのうえで web の新規スレッドで一往復し、返事が返れば配置できている（`CoS skills unavailable` が出たら path と `[knowledge] root` を見直す）。

## CELERIS_API_URL（API の向き先）

CoS run の `celerisctl` は、run の環境に入る `CELERIS_API_URL`（起動した daemon 自身の `[api] listen` から作る `http://127.0.0.1:<port>/api/v1`）を最優先で使う。優先順は `--api-url` > `CELERIS_API_URL` > `CELERIS_CONFIG` の `[api] listen`。試験用 DB の staging daemon から起動した CoS run も、本番ではなく自分の daemon へ送る。手で `celerisctl` を使うときも staging を向けたいなら `CELERIS_API_URL` を明示する。値は `/api/v1` で終わる必要がある。

## 添付の引き継ぎ（attach-handoff）

- task へ: CoS は起票の request（`POST /api/v1/tasks`・operation `task.create`）の `attachment_ids` で、task の作成と添付の pin を同じ transaction で行う（[ADR 2026-10-07-cos-live-fixes](../../agent-docs/adr/2026-10-07-cos-live-fixes.md) D1。「起票してから pin」はしない）。pin された添付は、その task の作業 run の開始時に hash・size を照合して作業ツリー外へ read-only で stage し、前置きの入力 manifest に載る（screenshot を見て画面修正する依頼など）。照合失敗と ssh remote の task は `delivery=unavailable` になる。
- 知識ベースへ: PDF などは `POST /api/v1/knowledge/inbox`（CoS operation `knowledge.record`、scope は `project:<slug>`）の `attachment_ids` で、候補の作成と同時に provenance 付きで pin される（同 ADR D2）。candidate は人が確認してから正本に入る。
- 確認は task 詳細の入力添付と KB の candidate で行う。stage 先の完了後削除は未実装。

## 実機確認の再実行（cos-chat-live.sh）

CoS chat の (a)〜(e)（往復・session の継続・画像入力・起票・screenshot の task への引き継ぎ・PDF の KB 取り込み）は
`scripts/dev/cos-chat-live.sh` で再実行する（同 ADR D5）。試験用 data dir に専用の config・DB・KB を作り、`CELERIS_CONFIG` ほか
`CELERIS_*` をその下だけに向けて一時 daemon を起こす。本番の設定・DB・KB・port（既定 17932、`COS_CHAT_LIVE_PORT` で変える）は
読まない・書かない。外部への通信は `full` の LLM 呼び出し（claude_oauth の既存ログイン）だけ。試験・CI からは呼ばない。

```sh
cargo build -p celeris -p celerisctl
bash scripts/dev/cos-chat-live.sh "$PWD" "$CARGO_TARGET_DIR/debug" <data dir> dry   # LLM を呼ばない疎通確認
bash scripts/dev/cos-chat-live.sh "$PWD" "$CARGO_TARGET_DIR/debug" <data dir> full  # (a)〜(e) を流す
```

- 引数が 4 つでない、または 4 つ目が `dry|full` 以外なら usage を出して exit 2。`<data dir>` は毎回新しい空の場所にする。
- 試験用 provider は `concurrency = 4`（1 では CoS の turn が task worker と受信箱の triage の後ろで止まる）。
- 各 turn の `run_id` は `POST …/messages` の応答（queued では `null`）でなく、`GET /api/v1/chat/threads/{t}/messages` を
  `client_message_id` で引いて待つ。
- 終わりに一時 daemon を process group ごと止め、`pgrep -f <data dir>` で残りが無いことを `steps.log` に書く。

見る証跡（`<data dir>/evidence/`）:

- `verdict.txt` — (d)・(e) の PASS/FAIL。(d) は operation `task.create` が `applied` で `result` に screenshot の添付 id があり、
  `chat_attachment_refs`（`owner_kind=task`）があり、その task の最初の run の `prompt.txt` の「## 入力の添付」に載ること。
  (e) は operation `knowledge.record` が `applied` で、`GET /api/v1/knowledge/inbox/{id}` の `provenance` に PDF の添付 id が出ること。
- `steps.log` — 各 turn の `run_id`・終端状態・`session_mode`、daemon の停止と残り process。
- `db.txt` — `chat_runs`・`node_sessions`・`chat_messages`・`cos_operations`（`result_json` つき）・`cos_operation` の events・
  `chat_attachment_refs`・`tasks`。
- `kb-inbox.json`・`kb-inbox-<id>.json` — KB 候補の一覧と詳細（provenance）。
- `input-manifest.txt`・`prompt-files.txt`・`staged.txt` — 起票された task の入力 manifest、`prompt.txt` の一覧、stage された添付。
- `run-<turn>.json`・`post-<turn>.json`・`msg-<turn>.json`・`daemon.log` — 各 turn の run と daemon のログ。

## 添付と保持

画像・PDF を含む任意ファイルを送れる。既定は 1 ファイル 25 MiB、1 メッセージ 100 MiB・10 件、インスタンス全体 10 GiB。元 blob は DB の親ディレクトリを `data_dir` とした `<data_dir>/chat/attachments/<attachment_id>/blob`、upload 中は `<data_dir>/chat/staging/` に置く。DB に原名・サイズ・MIME・SHA-256・参照を保存する。CoS へは検証した添付を read-only で stage し、画像は harness の画像入力または読取 tool、その他は path と manifest で渡す。task/KB へ引き継ぐ際は参照を pin する。

未送信 upload は 24 時間後、最後の参照が外れた blob は 30 日後に GC される。参照中の message・task・KB candidate と実行中 run の blob は自動削除しない。本文・要約・監査は保持し、終端 run の text/tool stream 詳細だけ既定 30 日で GC する。daemon の周期 GC は約 1 時間ごと。容量不足時は参照と保持期限を web/API で確認し、管理外の `rm` で blob だけを消さない。backup/restore は SQLite と `chat/attachments` を同じ整合点で扱い、復元後に hash と参照の整合を確かめる。

## 本番への反映と昇格

1. 人が現行 config と DB・添付のバックアップ方法を確認し、上記設定を本番 config に反映する。旧 CoS run を drain/stop し、移行中に旧 Console と新チャットの両方を正本にしない。
2. 人が [selfdeploy](selfdeploy.md) に従い `scripts/selfdeploy/release.sh <ref>` → `scripts/selfdeploy/verify.sh <sha12>` を実行する。`scripts/selfdeploy/status.sh` で `verify.json.ok` と `live_ok`、`celerisctl release preview <sha12>` で対象・schema・差分を確認する。
3. 人が上記の検査結果と [selfdeploy の昇格条件](selfdeploy.md#4-昇格するpromotesh人だけ) を確認してから `scripts/selfdeploy/promote.sh <sha12>` を実行する。CoS に将来 standing authorization を与える場合も、対象・範囲・安全確認を policy skill に明記し、その範囲外や結果不明は人へ回す。現行 selfdeploy runbook の「人だけ」の指定を CoS が自己承認で越えない。
4. 人が `scripts/selfdeploy/status.sh`、`GET /api/v1/health`、web の新規スレッドでの一往復、`GET /api/v1/cos/inbox` と Discord test 送信で起動・経路を確認する。本番の変更操作や test 送信は、この手順を実施する人が行う。

旧版へ戻す場合は、まず [rollback 手順](selfdeploy.md#5-戻すrollbacksh人だけ) と schema version を確認する。旧 binary が新 schema を開けない場合は停止時に取得した DB と添付の対を人が復元する。DB だけ、または blob だけを戻さない。復元後に生じたチャットや操作は失われるため、作業前に現在の状態を保全する。

## 停止・session rollover・障害確認

チャットの「停止」は当該 run を止め、未着手 queue を pause する。再開は「キューを再開」または明示的な送信操作で行う。`cos.enabled=false` は新しい CoS 入力を止め、待ちの直接通知への退避条件になる。daemon 全体を止めた間は Discord 送信もできず、再起動時に未解決分を回収する。

rollover は context 上限、`[sessions] rollover_tokens`、または harness/session 設定の変更で起きる。`node_sessions` の thread ごとの現役 session と `chat_runs` の resolved config、スレッドの summary/`summary_through_seq`、次 run の履歴を照合する。session cache 消失や resume 拒否時は DB 要約・履歴から fresh session を作る。CoS が同じ入力・操作を二重実行していないか、operation id と events の actor/reason を確認する。

返答が止まったときは web で run の `queued/running/stopping/failed`、queue pause、quota/account 理由と最後の tool event を見る。`GET /api/v1/chat/threads/{thread_id}`、`GET /api/v1/chat/threads/{thread_id}/stream` または `GET /api/v1/chat/threads/{thread_id}/runs/{run_id}/events`、`GET /api/v1/cos/inbox` で保存状態を確認し、通知は `GET /api/v1/notify` と未達表示・daemon ログを見る。SSE cursor の期限切れは履歴 snapshot を取り直す。外部副作用の成否が不明なら再実行せず人に判断を上げる。CoS 不在の待ちは fallback 通知の理由・web link・未解決状態を確認する。

## 受信箱一次対応の運用確認

導入後と通知設定を変えた後に、人が web と API で次の 4 点を確かめる。本番での test 送信と override は人が行う。

1. **代答**: `GET /api/v1/cos/inbox?state=answered` で item の `reason`・`policy_version`・`operation_id` を見る。`GET /api/v1/cos/operations/{operation_id}` と元 task の events で actor=cos と理由を確認し、「受信箱」スレッドに「CoS が代わりに答えた」カードがあることを見る。
2. **Discord escalation**: `state=escalated` の item に対応する通知が `GET /api/v1/notify` に 1 件だけあり、本文に要点・選択肢・推奨と理由・web link（`[notify] gui_base_url` + 該当画面）が入っていることを見る。`gui_base_url` が未設定・不正なら送信せず、`notify.gui_base_url is missing or invalid` として未達のまま残る。webhook の secret（`[notify] discord_webhook_secret`）が無ければ未設定扱いで未達になる。どちらでも元の待ちは解決しない。Discord の返信・リアクションでは状態が変わらないことも確かめる。
3. **CoS 不在の退避**: `cos.enabled=false`・CoS run の失敗・quota 切れ・`unavailable_after_secs` 超過のどれかで、item が `fallback` になり、「CoS 不在のため直接通知」の見出しと不在理由が付いた通知が revision ごとに 1 件だけ出ることを見る。CoS が回復しても同じ revision を再通知・代答しない。
4. **取消・差し戻し**: 代答カードの取消/差し戻し、または `POST /api/v1/cos/operations/{o}/override`（`mode=revoke|return`、人の認証のみ、理由必須）を使う。未消費の回答は待ちが新 revision で開き直り、消費済みは対象 task が pause されて新しい人待ちになり、不可逆な副作用は補償 task の id が返る。いずれも元の events は消えない。CoS の認証で override すると拒否される。

通知の通常経路が CoS の escalation だけであることは、旧 inbox reminder/digest・完了通知などが Discord に出ないこと（`GET /api/v1/notify` に直接送信の新規行が増えないこと）で確かめる。例外は上の退避と管理者の明示 test だけである。
