# ADR 2026-10-27: 外部 operator・MCP が取り次いだ人の指示を CoS が根拠として引ける記録にする

---
tasks: [01M5XXF7QGZ69112THK8RE3TEB]
---

- 状態: 採用（実装済み）
- 2026-10-28 付記: 取り次ぎの無い代答の範囲と根拠の形は [2026-10-28-cos-routine-delegated-answers](2026-10-28-cos-routine-delegated-answers.md)。`instructed_by` の付いた操作は人の判断なので `basis` を要らない。skill の policy_version は 6。
- 関係: [2026-10-27-api-principals](2026-10-27-api-principals.md)（A1 の principal）、
  [2026-10-27-external-operator](2026-10-27-external-operator.md) D4（B3 の取り次ぎ経路）、
  [2026-10-27-human-ops-actor-roles-request-ref](2026-10-27-human-ops-actor-roles-request-ref.md) D2・D4（主体の語彙、判断者・実行者）、
  [2026-10-07-cos-inbox-thread-conversation](2026-10-07-cos-inbox-thread-conversation.md) D3（`instructed_by`）、
  [2026-10-22-cos-fixed-threads](2026-10-22-cos-fixed-threads.md)（統括・案件の固定スレッド）

## 背景

利用者が外部 operator（Claude Code 等）や MCP client（ChatGPT 等）に伝えた指示・承認は、その会話の中にしか無い。
CoS は「人の既存の指示を指せない」として人へ回し、その「人」の役を operator が担うので、記録上は人の回答になる
（10-27 01M5XDTAQQ の問い: CoS が 02:13 に人へ回し、利用者が operator に「何台落としてもいいので続けて」と伝え、
operator が 04:01 に回答。10-25「sonnetで」、10-26 試行承認枠の record「operator session での発言」）。

既存の 3 つの仕組みを確かめた。

- CoS thread への人の入力（`POST /chat/threads/{t}/messages`）: 原文と時刻は残るが、誰が投稿したか（A1 の principal）も、
  それが人自身の発言か取り次ぎかも、適用範囲も残らない。
- standing rule: 認可の規則（node と action）であって、人の発言の原文・時刻・範囲を持つ記録ではない。
- `instructed_by`（`/cos/operations`）: 「この run の入力になった人の発言」だけを根拠にできる。一次対応の run（system 入力）は
  前の発言を指せないので、人の既存の指示があっても人へ回すしかない。

最小なのは 1 つ目で、CoS thread の人の発言に「取り次ぎ」の記録を足し、`instructed_by` がそれを根拠として引けるようにする。

## 決定

### D1. 取り次ぎの記録は CoS thread の `role=user` の発言

取り次がれた指示は、案件の固定 CoS スレッド（案件が無ければ統括）の `role=user` の発言として保存する。
本文は**利用者の発言の原文**。metadata に次を持ち、API の `ChatMessage` に `author`・`relay` として出す。

| 欄 | 意味 |
|---|---|
| `author` | 投稿した A1 の principal（`operator:<name>` / `mcp:<client_id>` / `human:<name>`）。認証だけが決める（本文からは読まない） |
| `relay.relayed_by` | 取り次いだ主体（`author` と同じ値。取り次ぎでない発言には `relay` が無い） |
| `relay.said_at` | 利用者が発言した時刻（RFC 3339。投稿時刻より後は 422） |
| `relay.scope.project_id` | 適用する案件（スレッドの案件。統括スレッドなら無し） |
| `relay.scope.task_ids` | 適用する task（空なら案件全体）。存在しない task・別案件の task は 422、最大 20 |
| `relay.scope.expires_at` | 期限（RFC 3339。無ければ期限なし。`said_at` より前は 422） |

通常の `POST /chat/threads/{t}/messages` も `author` を記録する（ADR api-principals D2 の「各 handler は主体を伝える」）。

### D2. 経路は operator の専用 route と MCP の `console_instruct`

- `POST /chat/threads/{t}/relays`（**管理系**）: body `{client_message_id, text, said_at, task_ids?, expires_at?}`、
  202 `ChatPostMessageResponse`。**`operator:<name>` の principal だけ**が呼べる。人の token（`human:<name>`）は
  403 `chat_relay_forbidden`（人は自分の発言を `…/messages` に書く。取り次ぎは他人の発言を運ぶ記録）。
  CoS run・MCP の内部 token・未認証は従来どおり管理系として拒否される。
- MCP: `console_instruct { text, project_id?, thread_id?, relay?: {said_at, task_ids?, expires_at?} }`。
  `relay` を付けた入力は `relayed_by = mcp:<client_id>` の取り次ぎになる。
- 取り次ぎの発言は通常の人の発言と同じく queue され、CoS run の入力になる（`mode=queue` 固定）。
- 旧い `chat_mcp_instruct` の `json_set` による `author` の後付けは、投稿時の metadata に統合して消す。

### D3. `instructed_by` は取り次ぎを根拠にできる

`/cos/operations` の `instructed_by` は次のどちらかの発言を受け付ける（他は従来どおり 422 `cos_instruction_invalid`）。

1. この run の入力になった人の発言（ADR 2026-10-07 D3 のまま）。
2. 同じスレッドの、取り消されていない取り次ぎの発言（`relay` を持つ `role=user`）。**どの run からでも**（一次対応の system 入力の run を含む）。

取り次ぎの発言はさらに、operation の対象が案件の範囲内であること（案件を特定できない対象も拒否）、operation の時点で `expires_at` を過ぎていないこと、`scope.task_ids` が空でなければ
operation の対象（task、または受信箱の件の task）がその中にあることを要求する。外れれば 422 `cos_instruction_invalid`
（監査に rejected で残る）。

payload の `instructed_by` は `{message_id, seq, relay?, decided_by, executed_by}`。

- `decided_by` = `{"kind":"human","id":null}`。取り次ぎなら `relayed_by: {kind, id}` を足す
  （kind は human-ops D2 の語彙: `operator:` → `external_operator`、`mcp:` → `mcp_client`）。
- `executed_by` = `{"kind":"cos_project"|"cos_general","id":"<thread_id>"}`（スレッドの種類から決める）。
- 監査の reason は「人の指示（seq n、operator:<name> の取り次ぎ）: …」。

human_required の件でも、取り次がれた指示は人が決めたものとして通る（領域 API の検証は同じ）。

### D4. CoS の prompt に有効な取り次ぎを渡す

`cos_chat.relayed_instructions`（worker protocol）に、スレッドの未取消・期限内の取り次ぎ（新しい順、最大 20）を
message id・seq・`said_at`・`relayed_by`・範囲・原文つきで渡す。prompt の「取り次がれた人の指示」節と
受信箱の relay 規則・`cos-inbox-triage` skill の文を D3 に合わせる（skill の policy_version は 4）。

### D5. operator の手順書は B3 をこの経路にする

[外部 operator 手順書](../../docs/ops/external-operator.md) §4-3 と §7 の依存表の B3 をこの route に更新する。
operator は利用者の発言を要約せず原文で、発言時刻と範囲（task・期限）を付けて取り次ぐ。
自分の判断は取り次ぎにせず、A1 の principal で通常の API を呼ぶ。

## 検証

- operator principal が案件スレッドに取り次ぎを投稿し、その run の CoS が `instructed_by` で受信箱の件（human_required）に答え、
  operation の payload に `relay`・`decided_by`（human、取り次ぎ operator）・`executed_by`（cos_project）が残る。
- 一次対応の run（system 入力）が同じスレッドの取り次ぎを根拠に answer できる。期限切れ・範囲外の task は 422 で rejected に残る。
- 人の token は relays route で 403。`said_at` の未来・`expires_at` の逆転・存在しない task は 422。
- MCP の `console_instruct` に `relay` を付けると `relayed_by = mcp:<client_id>` で同じ記録になる。
- prompt の「取り次がれた人の指示」節に期限内の取り次ぎだけが載る。
- `bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`。

## 付記（2026-10-28、[2026-10-28-single-human-window-general-cos](2026-10-28-single-human-window-general-cos.md)）

- 取り次ぎの主体に統括 CoS を足す。CoS operation `chat.relay_forward`（`POST /chat/threads/{t}/forwards`、CoS operation 専用）が、
  統括スレッドの人の発言（`author` が `human:<name>`）の原文と `created_at`（`said_at`）を案件スレッドへ写す。
  `relay.relayed_by` は `cos:<統括スレッド id>`、`relay.source` は元の `{thread_id, message_id, author}`。範囲は task 1〜20 件と期限が必須。
- D3 の payload: 統括 CoS の取り次ぎを引いた operation は `decided_by = {kind: human, id: <元の author の name>,
  relayed_by: {kind: cos_general, id: <統括スレッド id>}}`、`executed_by = {kind: cos_project, …}`。
- D3 に排他を足す: 統括スレッドの run が案件の対象に `instructed_by` 付きで operation を実行するのは 422
  `cos_instruction_invalid`（統括 CoS は案件の件を取り次ぐだけで、答えるのは案件 CoS）。
- `POST /chat/threads/{t}/relays` は operator 専用のまま（CoS からは除外のまま）。

## 付記 A2（2026-10-28、取り次ぎ後の再判断。task 01M60JC1QB4K9MD2Z73CYMQB9Y）

背景: 2026-10-28T05:20Z、BenchFS の一次対応 run が走っている間に operator が人の指示 3 件を取り次いだ（seq 95〜97）。
run は prompt を作った後に届いた取り次ぎを見ずに件を escalate し、取り次ぎは次の run の入力として待つだけだった。
escalated の件を取り次ぎで再判断する入口が無い。また 06:15Z、CoS が `authorization-…` に `instructed_by` 付きで答えたとき、
件は 06:14Z に operator が答えて受信箱から消えていた。D3 の範囲検査は件 → task を生きた受信箱だけから引くので、
対象を「task 無し」と見て「applies to project …, not this target」で拒否した（本当の理由は「件はもう待っていない」）。

- **R1（再判断の入力）**: dispatcher は tick ごとに、CoS スレッドの `escalated` の件（CoS の一次対応 run が判断したもの。
  直送の件は除く）と、同じスレッドの未取消・期限内の取り次ぎのうち、**件を判断した一次対応の入力 message より後の seq** の
  ものを組にする。件の task（受信箱の件の `task`・`blocking` の task）が取り次ぎの `task_ids` に入るとき（`task_ids` が空なら
  スレッドの案件全体）、その件を再判断の対象にする。対象は一次対応と同じ claim（system message 1 件 + chat run 1 件、
  件は `running`）で CoS に渡す。入力 message は 1 行目が従来どおり `CoS 受信箱の一次対応: item_ids=[…]`、2 行目が
  `取り次ぎによる再判断: [{"item_id","source_key","relay_message_ids":[…]}]` で、CoS は取り次ぎを根拠に `instructed_by`
  付きで答えるか、範囲外と判断すれば理由を付けて再び escalate する。受信箱の件が消えていれば（人が答えた等）対象にしない。
- **R2（二重回答を防ぐ）**: 同じスレッドに `running`/`stopping` の chat run があれば再判断の claim をしない（claim の transaction で
  確かめる）。走っている一次対応 run が終わってから次の tick で渡す。再判断の claim で件の入力 message が取り次ぎより後の seq に
  なるので、同じ取り次ぎで同じ件を 2 度渡さない（状態は SQLite の `cos_inbox_items.message_id` と `chat_messages.seq` だけで決まる）。
  取り次ぎ自身の run が先に答えた件は受信箱から消えるので R1 の対象にならない。消えた件への 2 度目の回答は領域 API が拒否する。
- **R3（範囲検査の写像）**: D3 の範囲検査で受信箱の件が生きた受信箱に無いとき、件 id から task を引く
  （`authorization-`・`acceptance_check-` は approvals の task、`decision-` は decisions の task、`question-`・`failed-` 等の
  `<kind>-<task id>…` はその task）。範囲に入れば領域 API が「件はもう開いていない」（409 `cos_revision_conflict`）と答える。
- migration は無い（既存の列だけで決まる）。`cos-inbox-triage` skill に再判断の入力の読み方を足す（判断の基準は変えないので policy_version は 6 のまま）。
