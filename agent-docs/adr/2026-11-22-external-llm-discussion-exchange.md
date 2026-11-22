---
tasks: [01M7XQ9QK1KC34Q2A6PJ960TJB]
---
# ADR 2026-11-22: 外部 LLM（ChatGPT・Muse など）との議論の交換を、既存の MCP の scope と chat の REST に置く

- 日付: 2026-11-22
- 状態: Accepted
- 関係: 戦略層 v1 子 4（task 01M7XQ9QK1KC34Q2A6PJ960TJB）。子 0 の設計 brief（`agent-docs/progress/2026-11-21-strategy-v1-0/design-brief.md`）の §5
  （共有フォーマット・規則 R1〜R13）。子 2 の [ADR 2026-11-21-strategy-evidence-contract](2026-11-21-strategy-evidence-contract.md)（D2.2 の承認の導出・
  `task_core::strategy::envelope`）。子 3 の [ADR 2026-11-21-chat-discussion-mode](2026-11-21-chat-discussion-mode.md)（D1 の mode・D2 の拒否・
  D5 の編集と撤回・D6 の `ChatCitation`・D7 の actor）。[ADR-0056](0056-mcp-server.md)（MCP の client・scope・監査・流量制限）、
  [ADR 2026-10-23-mcp-read-only-db-and-api](2026-10-23-mcp-read-only-db-and-api.md)（`db:read`・`api:read`・RDC の経路）、
  [ADR 2026-10-27-api-principals](2026-10-27-api-principals.md)（`Principal`）、[ADR 2026-10-27-relayed-instructions](2026-10-27-relayed-instructions.md)、
  [ADR 2026-10-09-cos-operations-all-mutations](2026-10-09-cos-operations-all-mutations.md)（書き込みの route の登録）。
- 人の決定: 計画 v1 の承認と、決定 `ext-client-scope` の答え **b**（承認系の 2 道具 `decision_answer`・`task_plan_gate` を `tasks:decide` に移し、
  `db:read`・`api:read` は運用の分析用に残す。機微な結果が ChatGPT に出うることは人が受け入れた）。答えた人は rmaeda（operator が AskUserQuestion で聞いた）。

## 背景

- 人は ChatGPT Chat を主な議論の入口にし、Celeris を正本にしたい。Muse など他の外部 LLM も同じ契約で使いたい。
- 今の外部の入口は MCP の道具だけで、議論に使えるのは `console_instruct`・`console_reply`（CoS の run を起こし、返事の全文を返す）と、
  `db_query`・`api_get`（DB と API の全読み）である。人が許可した範囲・出典・`content_hash`・consent・idempotency を持つ snapshot と取り込みの契約が無い。
- 本番の MCP client（2026-11-22 に run の読み取り専用 DB に `celerisctl mcp client ls` を当てて確かめた。token は出ない）:
  - `chatgpt-rdc-2`（有効）: `knowledge:read,knowledge:propose,tasks:read,tasks:interact,console:instruct,db:read,api:read`
  - `claude-code`（有効）: `knowledge:read,knowledge:propose,tasks:read,console:instruct,org:read,skills:read,skills:write`
  - `chatgpt`・`chatgpt-rdc`: 失効済み
- `chatgpt-rdc-2` は `tasks:interact` を持つので、今は ChatGPT から `decision_answer`・`task_plan_gate` で人の決定に答えられる（記録は `by=mcp:chatgpt-rdc-2`）。
- `task_core::strategy::envelope` の R10・R13 は、envelope の中の自己申告の `approval.by` が `human:` なら承認とみなす（子 2 の最終 review の advisory 1）。
  外部から届く文書が自分で「人が承認した」と書けば R10 を通る。
- knowledge のページには機密の分類（classification）の欄が無い。機密の分類を持つのは戦略の文書（`celeris.strategy/1` の `acl.classification`）だけである。

## 決定

### D1: MCP の 3 道具と scope

#### D1.1 名前と scope

| 道具 | scope | 何をするか |
|---|---|---|
| `discussion_snapshot` | `discussion:read`（新設） | 案件 thread のうち、人が共有した範囲と、呼んだ client 自身の propose だけを返す |
| `discussion_evidence_search` | `discussion:read`（新設） | snapshot と同じ範囲の中だけを語で探し、出典・confidence・decision_state 付きで返す |
| `discussion_propose` | `discussion:propose`（新設） | 未承認の案（draft）を案件 thread に 1 件記録する |

- `McpScope` に `DiscussionRead`（`discussion:read`）と `DiscussionPropose`（`discussion:propose`）を足す（`crates/task-core/src/mcp.rs`）。
  `mcp_clients.scopes` は文字列の列なので migration は要らない。
- 新設の理由: 既存の scope では「人が共有した議論を読む・案を記録するが、CoS に指示しない」を表せない。
  - `console:instruct` は `console_instruct` で execute の CoS run を起こせ、`console_reply` は CoS の返事の全文を返す。Muse などに渡すと最小権限にならない。
  - `knowledge:read` は knowledge の全ページ、`tasks:read` は task の全件を返す。議論の snapshot の範囲（人の共有）と意味が違う（変更の規約 8）。
  - `knowledge:propose` の受信箱は日次の整理を通ると正本に適用されるので、未承認の案の置き場所にできない（子 3 D3 と同じ理由）。
- 3 道具は既存の `ToolDef` の registry（`crates/celeris-mcp/src/tools/mod.rs` の `all()`）に並べる。認証（`auth.rs`）・client ごとの scope・
  `mcp_calls` の監査・流量制限（`rate_limit_per_min`）はそのまま使う。HTTP・`celerisctl mcp stdio`・`celerisctl mcp call` のどれからも同じ契約で呼べる。

#### D1.2 対象の thread

- 引数は `project_id`・`thread_id` のどちらか、または両方。`thread_id` だけなら、その thread が `kind='project'` であること。`project_id` だけなら、
  その案件の固定の thread（`idx_chat_threads_project`）を使う。両方あれば thread の案件と一致しなければ拒否（`-32602`、`thread_project_mismatch`）。
- 統括（`general`）・`review`・`legacy` の thread は 3 道具の対象にしない（`-32602`、`thread_kind_not_project`）。

#### D1.3 共通の形と入力長の上限

すべての項目（snapshot の item・検索の hit・propose の記録）は、子 0 brief §5 の envelope の欄を持つ:
`project_id`・`thread_id`・`source {kind, label, thread_id}`・`actor {kind, id}`・`source_revision {request_sha256, roadmap_version}`・`timestamp`・
`content_hash`（`sha256:<hex>`、送る本文の SHA-256）・`consent {granted_by, scope, granted_at}`・`references[]`・`confidence`・`decision_state`。

| 入力 | 上限 |
|---|---|
| `body`（propose・取り込み） | 16,384 byte（`envelope::BODY_MAX_BYTES`） |
| `references` | 32 件。各 `url` は 2,048 byte、`note` は 500 文字 |
| `idempotency_key` | 1〜128 文字、`[A-Za-z0-9._:-]` |
| `source_label` | 1〜64 文字、`[a-z0-9._-]`（例 `chatgpt`・`muse`） |
| 検索の `query` | 1〜512 文字 |
| `limit` | 既存の `clamp_limit`（既定 20、上限 100） |
| 取り込みの Markdown・JSON の全体 | 32,768 byte |
| 共有の `message_ids` | 1〜200 件 |

上限を超えた入力は `-32602`（MCP）・422（REST）で拒否し、切り詰めて受けない。未知の欄は `deny_unknown_fields` で拒否する（`approval`・`consent`・
`actor` を client が書くことも含む。これらは Celeris が埋める）。

#### D1.4 `discussion_snapshot`

引数:

```json
{"project_id": "01M2WTS3DKNZBSZ2JMVB4CZMBW", "thread_id": null, "after_seq": 0, "limit": 50}
```

返り値（`schema` は `celeris.discussion-snapshot/1`）:

```json
{
  "schema": "celeris.discussion-snapshot/1",
  "project_id": "01M2WTS3DKNZBSZ2JMVB4CZMBW",
  "thread_id": "01M5JJTVPW15CKFRCA0FAYCHK4",
  "source_revision": {"request_sha256": "d7b584ad…", "roadmap_version": 3},
  "generated_at": "2026-11-22T03:00:00Z",
  "items": [
    {
      "kind": "message",
      "message_id": "01M7XPWTXR1A0CNWYN7D1HBZWB",
      "seq": 4201,
      "revision": 1,
      "citation": {"thread_id": "01M5JJTVPW15CKFRCA0FAYCHK4", "message_id": "01M7XPWTXR1A0CNWYN7D1HBZWB", "revision": 1, "sha256": "…64 桁…"},
      "source": {"kind": "celeris_cos_thread", "label": "celeris", "thread_id": "01M5JJTVPW15CKFRCA0FAYCHK4"},
      "actor": {"kind": "human", "id": "human:rmaeda"},
      "timestamp": "2026-11-21T05:00:00Z",
      "content_hash": "sha256:…",
      "consent": {"granted_by": "human:rmaeda", "scope": "selected_excerpt", "granted_at": "2026-11-22T02:55:00Z"},
      "share_id": 98123,
      "references": [],
      "confidence": "estimated",
      "decision_state": "discussion",
      "body": "…",
      "masked": 0
    }
  ],
  "omitted": [{"message_id": "01M7…", "reason": "edited_since_share"}],
  "next_after_seq": 4201
}
```

- `kind` は `message`（chat の message）・`summary`（子 3 D6 の `discussion/summary/*.json` の添付）・`strategy`（子 3 D3 の `strategy/*/v*.json` の添付）・
  `proposal`（この client の `discussion_propose` の記録）。
- `decision_state` は Celeris が導く。chat の message と要約は `discussion`、案は記録の値（`discussion` か `proposed`）。`approved` は、子 2 D2.2 の決定
  （`strategy-approve-v<n>` に `human:` の主体が `approve` で答え、問いの sha256 が一致する）があるときだけ付く。文書や client が書いた値は使わない。
- `confidence` は message と要約では `estimated` に固定する（人や CoS の発言は検証済みの主張ではない）。戦略の文書・案は記録の値を返す。
- 1 回の上限は 200 item。続きは `next_after_seq` を `after_seq` に渡す。

#### D1.5 `discussion_evidence_search`

引数: `{"project_id": "01M2…", "query": "SCC 17", "limit": 20}`。

返り値（`celeris.discussion-evidence/1`）:

```json
{
  "schema": "celeris.discussion-evidence/1",
  "project_id": "01M2WTS3DKNZBSZ2JMVB4CZMBW",
  "thread_id": "01M5JJTVPW15CKFRCA0FAYCHK4",
  "hits": [
    {
      "kind": "strategy",
      "citation": {"thread_id": "01M5…", "message_id": "01M7…", "revision": 1, "sha256": "…"},
      "path": "hypotheses[1]",
      "snippet": "…400 文字まで（マスク済み）…",
      "references": [{"kind": "commit", "repo": "agent-platform", "sha": "…"}],
      "confidence": "contradicted",
      "decision_state": "proposed",
      "actor": {"kind": "cos", "id": "cos:01M5JJTVPW15CKFRCA0FAYCHK4"},
      "source": {"kind": "celeris_cos_thread", "label": "celeris", "thread_id": "01M5…"},
      "content_hash": "sha256:…"
    }
  ]
}
```

- 探す範囲は D1.4 の snapshot に出る item だけ（共有の無い message、失効した message、knowledge は探さない）。chat の本文は既存の FTS5 の検索を
  使い、結果を共有の範囲で絞る。戦略の文書は `hypotheses[]`・`evidence[]`・`references[]` を語で照合する。
- knowledge は返さない。knowledge のページには機密の分類が無いので、人の共有の単位を作らずに外へ出すと「機微な Knowledge は既定で外に出ない」を
  保てないからである。外部 client に knowledge を渡すかは、既存の `knowledge:read` を人が client に与えるかで決まる（今の `chatgpt-rdc-2` は持つ）。

#### D1.6 `discussion_propose`

引数:

```json
{
  "project_id": "01M2WTS3DKNZBSZ2JMVB4CZMBW",
  "thread_id": "01M5JJTVPW15CKFRCA0FAYCHK4",
  "idempotency_key": "chatgpt-2026-11-22-001",
  "source_label": "chatgpt",
  "body": "仮説 H2 は SCC の数で反証できる。最小の識別実験は …",
  "references": [
    {"kind": "chat", "thread_id": "01M5…", "message_id": "01M7…", "revision": 1, "sha256": "…"},
    {"kind": "url", "url": "https://example.org/paper", "note": "外部の資料。未検証"}
  ],
  "confidence": "estimated",
  "decision_state": "proposed"
}
```

返り値: `{"message_id": "01M8…", "thread_id": "01M5…", "project_id": "01M2…", "seq": 4302, "content_hash": "sha256:…", "decision_state": "proposed", "duplicate": false}`。

- Celeris が埋める欄: `source = {kind: "mcp_client", label: <source_label>, thread_id}`、`actor = {kind: "mcp", id: "mcp:<client_id>"}`、`timestamp`、
  `content_hash`（本文から計算）、`source_revision`（今の `Project.request` の sha256 と承認済み Roadmap の版）、
  `consent = {granted_by: "mcp:<client_id>", scope: "mcp_scope:discussion:propose", granted_at: <now>}`。
- `decision_state` は `discussion` か `proposed` だけ。`approved`・`rejected`・`withdrawn` は拒否（R10・R11）。
- `chat` の参照は、呼んだ client の snapshot に今出る item を指し、`revision`・`sha256` がその時の message と一致すること（子 3 D6 の
  `ChatCitation` の検査を使う）。一致しなければ `-32002`（`citation_invalid`）。`url` の参照は文字列として保存するだけで、Celeris は取りに行かない。

#### D1.7 付記: mcp-tools の実装（2026-11-22）

- 3 道具は `crates/celeris-mcp/src/tools/discussion.rs` に置き、`tools::all()` に並べた。引数と error の写しだけを持ち、範囲と規則は
  `SqliteStore::chat_exchange_snapshot`・`chat_exchange_propose`（`task_core::discussion_exchange`）と envelope の純関数が決める。
  呼んだ主体は認証した client から `mcp:<client_id>` として作り、引数からは取らない。scope は `client.scopes` の `as_str()` を R8 に渡す。
- `McpScope::DiscussionPropose.as_str()` は `envelope::MCP_PROPOSE_SCOPE` を返す（綴りは 1 か所）。`McpScope` の JSON Schema の enum にも
  2 つを足したので、`ApiV1Schema`（`GET /mcp/clients` の `scopes`）の生成物 `docs/api/v1/api-v1.schema.json`・`web/api/generated/` が変わる。
- D1.5 の検索は、FTS5 ではなく、呼んだ client の snapshot の item（マスク後の本文）を語で照合する。理由: (1) 範囲の実装を snapshot の 1 つに
  する（規約 2。FTS5 の結果を共有の範囲で絞る処理を別に書かない）。(2) FTS5 はマスク前の今の本文を引くので、秘密の前置き（`ghp_` など）で
  探すと、出力に秘密が出なくても「当たった」ことで秘密の有無が分かる。snapshot の本文はマスク後なので当たらない。照合は空白で区切った語が
  すべて含まれるもの（大文字小文字を区別しない）。snapshot の頁（200 件）を最大 50 頁まで読む。
- D1.6 の `chat` の参照の検査（`citation_invalid`）は MCP の道具が行う。呼んだ client の snapshot の全頁の item の `citation` と
  `thread_id`・`message_id`・`revision`・`sha256` が一致しなければ `-32002`。`check_propose`（task-core）は参照の形と長さだけを見るので、
  REST の取り込みが同じ検査を要るなら、検査を task-core の兄弟 module に移して両方から呼ぶ（mcp-tools の書き込み範囲は task-core を
  含まないため、ここでは MCP 側に置いた）。
- 要約・戦略の file（`ExchangeDocument`）は MCP からは渡さない（`documents = []`）。`McpState` は CoS の workspace の場所を持たず、file を
  読む処理を MCP に足すと CoS の workspace の読み方が 2 つになる。したがって今の MCP の snapshot・検索の item は `message` と `proposal`
  だけで、confidential の文書は MCP から出ない。要約・戦略の file を外部 LLM に渡すかは未解決（mcp-tools の進捗）。

### D2: snapshot が返す範囲（人の共有）、失効、秘密のマスク

#### D2.1 既定は出さない。人の共有だけを出す

- 人の共有の記録は、thread の `chat_events` に追記する 2 つの type で持つ（新しい表・列・migration は足さない）:
  - `exchange_shared`: data `{by, message_ids, revisions: [{message_id, revision, sha256}], scope: "selected_excerpt"|"summary", include_confidential, expires_at}`。
  - `exchange_share_revoked`: data `{by, share_event_id}`。
- 書けるのは `Principal::Human` だけ（D4 の REST）。`by` は呼んだ人の `caller.actor()`（`human:<name>`）。operator・`mcp:`・`cos:`・認証の無い要求は 403。
- `expires_at` は必須で、共有した時刻から 30 日以内。
- 有効な共有は保存しない。読むたびに次から導く: `exchange_shared` があり、同じ id の `exchange_share_revoked` が無く、`expires_at` が過ぎていない。
  状態の正本は追記の event の列だけで、状態の遷移の表は持たない（規約 4。状態機械を足さない）。
- 共有が 1 件も無い thread の snapshot は `items: []` を返す。

#### D2.2 撤回・編集で失効した message

- 共有は message の revision と本文の sha256 に固定する。snapshot を組むとき、今の message の revision・sha256・`retracted`（子 3 D5.2）を照合する。
  - 撤回された message: 出さない。`omitted` に `{message_id, reason: "retracted"}` だけを出す（本文・撤回の理由は出さない）。
  - 共有の後に編集された message: 出さない（人が共有したのは前の版）。`omitted` に `reason: "edited_since_share"`。人がもう一度共有すれば今の版が出る。
  - 要約・案の添付で、引用（子 3 D6 の `ChatCitationView`）が 1 つでも `current` でないもの: 出さない（`reason: "citation_stale"`）。
- `exchange_shared` は追記なので、失効の後も「いつ誰が何を外に出せるようにしたか」は監査に残る。

#### D2.3 機密

- chat の message と要約には機密の分類が無い。人の共有がそのまま外に出す許可である。
- 戦略の文書（`strategy`）は、`acl.classification = "confidential"` か `acl.outbound = false` なら、共有に `include_confidential: true` が無い限り出さない
  （`reason: "confidential"`）。これが R13 の「人の承認」の記録になる（D5.2）。
- knowledge は 3 道具のどれも返さない（D1.5）。

#### D2.4 秘密

- 共有の時、対象の message の本文が秘密の形（R9。`task_core::strategy::contains_secret` と同じ正規表現）を含めば 422 `exchange_secret` で拒否する。
  人は message を編集してから共有し直す。
- snapshot と検索と書き出しは、出す直前に本文・snippet の秘密の形を `[masked]` に置き換え、置き換えた数を `masked` に出す（共有の後に正規表現が
  広がったときの二重の守り）。`content_hash` はマスク後の送る本文の hash、`citation.sha256` は元の版の sha256（子 3 D6 の引用と同じ値）。
- マスクの関数は envelope-fix が `task_core::strategy::envelope` に 1 つだけ公開し（`super` の正規表現を使う。`strategy/mod.rs` は変えない）、MCP・REST・
  書き出しが同じ関数を使う（規約 2）。
- token・cookie・password・webhook URL を返り値・監査（`mcp_calls.request`）・log・試験の出力に書かない。3 道具は `audit_request` の対象に入れない
  （本文を監査に写さない）。

### D3: propose は未承認の draft の記録だけ

- 置き場所: 案件 thread の `chat_messages` に `role='user'`・`author='mcp:<client_id>'` の message を 1 件作る。本文は `body`、
  `metadata_json` に `discussion_mode: "discuss"` と `exchange`（D1.6 の envelope から `body` を除いた欄）を書く。新しい表・列・migration は足さない。
- 添付・knowledge の受信箱は使わない。添付は CoS の workspace の file を前提にしており（子 3 D3）、受信箱は日次の整理で正本に適用されるからである。
- mode は client が選べない。常に `discuss` で、CoS の run は既存の仕組みで起き（新しい分岐を dispatcher に足さない）、子 3 D2 によりすべての
  CoS operation が API で拒否される。したがって propose から task の起票・Roadmap の変更・決定への回答・publish に進む経路は無い。
  費用は CoS の run 1 回分で、MCP の流量制限と idempotency（D4.3）が数を抑える。
- 案を承認に進めるのは人だけで、既存の経路（人が execute で依頼し、task の run が成果物を作り、決定 `strategy-approve-v<n>` に人が答える。子 2 D2.2）を通る。
- idempotency: `client_message_id = "mcp:<client_id>:<idempotency_key>"`（既存の索引 `idx_chat_messages_client`）。client の id で名前空間を分けるので、
  別の client の記録を上書きも確認もできない。同じ key で同じ `content_hash` なら既存の message を返す（`duplicate: true`、新しい run は起きない）。
  同じ key で違う `content_hash` は `-32002`（`R7:idempotency_conflict`）。

### D4: MCP 非対応の経路（人が選んだ要約・引用の Markdown/JSON の交換）

#### D4.1 REST

| route | 主体 | 何をするか |
|---|---|---|
| `POST /api/v1/chat/threads/{t}/exchange/shares` | `Principal::Human` | D2.1 の共有を記録する。本文 `{message_ids, scope, include_confidential, expires_at}` |
| `POST /api/v1/chat/threads/{t}/exchange/shares/{share_id}/revoke` | `Principal::Human` | 共有を取り消す |
| `GET /api/v1/chat/threads/{t}/exchange/export?format=json\|markdown&share_id=…` | `Principal::Human` | 共有の範囲を D1.4 と同じ item で書き出す |
| `POST /api/v1/chat/threads/{t}/exchange/imports` | `Principal::Human` | 人が ChatGPT などから選んだ要約・引用を 1 件取り込む |

- 4 つとも operator・`mcp:`・`cos:`・認証の無い要求は 403 `exchange_forbidden`。書き込みの 3 route は `crates/task-api/src/cos/ops/surface.rs` の `EXCLUDED` と
  [ADR 2026-10-09-cos-operations-all-mutations](2026-10-09-cos-operations-all-mutations.md) の表に登録する（CoS の run には渡さない）。wire の型は
  `crates/task-api/src/schema.rs` の `ApiV1Schema` に登録し、`docs/api/v1` と `web/api/generated` を再生成する。
- 書き出しは読むだけで、記録は作らない（何を外に出せるようにしたかは共有の event が記録する）。書き出しの envelope は `direction: "outbound"`、
  `consent.granted_by` は共有した人。
- `celerisctl` に同じ 4 つを呼ぶ subcommand を足す（既存の人の API token で REST を呼ぶ。MCP の token は使わない）。

#### D4.2 取り込みの形

JSON（1 件の envelope）:

```json
{
  "format": "json",
  "client_message_id": "import-2026-11-22-001",
  "envelope": {
    "project_id": "01M2WTS3DKNZBSZ2JMVB4CZMBW",
    "source": {"kind": "human_paste", "label": "chatgpt"},
    "source_revision": {"request_sha256": "d7b584ad…", "roadmap_version": 3},
    "timestamp": "2026-11-22T03:10:00Z",
    "content_hash": "sha256:…",
    "references": [{"kind": "chat", "thread_id": "01M5…", "message_id": "01M7…", "revision": 1, "sha256": "…"}],
    "confidence": "estimated",
    "decision_state": "discussion",
    "body": "ChatGPT との議論で人が選んだ要約 …"
  }
}
```

Markdown（`format: "markdown"`、`content` に全文）:

```markdown
---
schema: celeris.discussion-envelope/1
project_id: 01M2WTS3DKNZBSZ2JMVB4CZMBW
source: {kind: markdown_import, label: chatgpt}
source_revision: {request_sha256: d7b584ad…, roadmap_version: 3}
timestamp: 2026-11-22T03:10:00Z
content_hash: sha256:…
confidence: estimated
decision_state: discussion
references:
  - {kind: chat, thread_id: 01M5…, message_id: 01M7…, revision: 1, sha256: …}
---
ChatGPT との議論で人が選んだ要約 …
```

- `content_hash` は front matter の後の本文（末尾の改行 1 つを除く）の SHA-256。Markdown と JSON の往復で同じ値になる（試験 `external_exchange_markdown_json_roundtrip_hash`）。
- Celeris が埋める欄: `actor = {kind: "human", id: <caller>}`、`consent = {granted_by: <caller>, scope: "selected_excerpt", granted_at: <now>}`。
  文書が `actor`・`consent`・`approval` を書いていれば、呼んだ人と一致しなくても一致しても拒否する（422。自己申告を受けない）。
- `source.kind` は `human_paste` か `markdown_import`（R3 で actor は human）。`decision_state` は `discussion` か `proposed`（R11）。
- 取り込んだ message は `role='user'`・`author=<caller>`・`metadata_json.discussion_mode = "discuss"`・`metadata_json.exchange`（envelope から `body` を除いた欄）。
  mode を `discuss` に固定するので、この message を入力にした CoS の run はすべての operation を拒否される。子 3 D1.2 の導出では、この message は
  後の execute の run の入力の集合に入らない（失敗して持ち越されたときも mode は厳しい側に倒れる）。したがって取り込んだ文を「人の発言の引用」として
  execute の operation の根拠にする経路は無い。人が実行したければ、自分の言葉で execute の message を送る。
- ChatGPT の会話全文・Memory を Celeris から読みに行く経路は作らない。取り込みは人が選んで貼った 1 件だけで、browser の login・Cookie も使わない。

#### D4.3 idempotency（R7）

- key は既存の `client_message_id`（`idx_chat_messages_client`、thread ごとに一意）。照合は `content_hash`。
- 同じ `client_message_id` で同じ `content_hash`: 200 で既存の message を返す（新しい message・run は作らない）。
- 同じ `client_message_id` で違う `content_hash`: 409 `R7:idempotency_conflict`。
- `EnvelopeContext.idempotency`（key → content_hash）は、呼び出し側が thread の既存の message の `client_message_id` と `metadata_json.exchange.content_hash`
  から作る。

#### D4.4 ChatGPT custom remote MCP は optional

- 最低限の経路は D4.1 の書き出しと取り込みで、どのプラン・platform の ChatGPT でも人の貼り付けで使える。RDC の経路（D6）はその次、custom remote MCP
  （[docs/guides/mcp.md](../../docs/guides/mcp.md) §7.1 の Secure MCP tunnel）は使える環境だけの optional である。
- 外部に公開する MCP の本番配置（tunnel の設定・公開の listener・本番の client の発行）は、この子では行わない。人が承認した後に人が行う。

### D5: envelope の R10・R13 を決定の記録から導く（envelope-fix）

#### D5.1 直す前

`validate_envelope` は envelope の中の `approval {by, via}` を読み、`by` が `human:` で `via` が `web`・`api_human_token` なら R10 を満たし、
`by` が `human:` なら R13 を満たす。どちらも文書の自己申告で、外から届く文書が自分で書ける。

#### D5.2 直した後

- envelope の `approval` の欄を形から外す。`approval` の key を持つ envelope は、`decision_state` によらず `R10:approval_needs_human` で拒否する
  （自己申告の承認を受けたことを、受理でなく拒否で示す）。旧い読み方（`Approval` の型と `human_approval` の closure）は同じ変更で消す。
- `EnvelopeContext` に、呼び出し側が store から集めた記録を渡す欄を足す:
  - `approval: Option<ApprovalRecord>`（子 2 の `task_core::strategy::ApprovalRecord` をそのまま使う。同じ形を 2 つ作らない）。
  - `outbound_share: Option<OutboundShare>`（D2.1 の共有の event から導いた `{by, sha256s, include_confidential}`）。
  - `caller: String`（認証した主体。`human:<name>` か `mcp:<client_id>`）と `caller_scopes`（MCP のとき）。
- R10: `decision_state = approved` は、`approval.last_answer = (approve, by)` で `is_human_principal(by)`、かつ `approval_subject(approval.question)` の
  sha256 が envelope の `content_hash` と一致するときだけ通る。`by` が `mcp:`・`cos:`・`operator:` の答えは承認にならない。DecisionAnswered の `by` は
  API の principal から決まり（[ADR 2026-10-27-api-principals](2026-10-27-api-principals.md)）、MCP の答えは `mcp:<client_id>` になるので、外部の client は
  `human:` の答えを作れない。
- R13: `direction = outbound` で confidential の envelope は、`outbound_share` が `human:` の主体で、`include_confidential = true` で、`sha256s` に
  `content_hash` を含むときだけ通る（または R10 と同じ承認の決定）。
- R3 に主体の一致を足す: `actor.id` が `caller` と違えば `R3:actor_mismatch`（MCP の client が `human:rmaeda` や別の client を名乗れない）。
- R8 を呼んだ主体から導く: `human_paste`・`markdown_import` は `consent_granted(consent)` かつ `consent.granted_by == caller`。`mcp_client` は
  `caller` が `mcp:` で `caller_scopes` に `discussion:propose` があるとき。client が書いた consent は使わない（Celeris が D1.6・D4.2 で埋める）。
- R11（外部の source と注入文は `discussion`・`proposed` から進めない）と R9（秘密）と D2.4 のマスクを、取り込み・書き出し・MCP が共有する
  純関数として `envelope` から公開する。同じ規則を `discussion_exchange`・`celeris-mcp`・`task-api` に書き直さない（規約 2）。
- envelope の試験の case は今、子 0 の進捗の fixture（`agent-docs/progress/2026-11-21-strategy-v1-0/fixtures/` の `baseline/`・`negative/`・`context.json`）を
  `crates/task-core/src/strategy/tests.rs` が `include_str!` で読む。そのうち `baseline/human-approved-in-web.json` と `negative/llm-approval-ref.json` は
  自己申告の `approval` を使う。envelope-fix は、試験が読む envelope の case を `crates/task-core/src/strategy/fixtures/envelope/` に移し、
  自己申告の `approval` の case を「`approval` の欄は無く、`EnvelopeContext.approval` に決定の記録を渡す」形に直す（承認の正の case は `human:` の
  決定、負の case は `mcp:`・`cos:` の決定と自己申告の欄）。子 0 の進捗の fixture と参照の検査器 `validate.py` は、子 0 の時点の記録として変えない
  （試験からは読まなくなる。他の task の進捗を書き換えない）。

#### D5.3 付記: envelope-fix の実装（2026-11-22）

- 公開した純関数（`task_core::strategy::envelope`）: `human_approved`（R10）・`outbound_allowed`（R13）・`consent_ok`（R8）・`state_allowed` と
  `contains_injection`（R11）・`contains_secret`（R9。`super` の関数の再公開）・`mask_secrets`（D2.4）。`validate_envelope` もこれらを呼ぶ。
  型は `EnvelopeContext { project_id, idempotency, caller, caller_scopes, approval, outbound_share }`・`OutboundShare`・`Masked`。
  `EnvelopeContext` は `Deserialize` を外した（呼び出し側が store と認証から組む。文書から読まない）。
- R10 の決定は D2.2 の形に限る: `key == approval_decision_key(問いの版)`、`status = answered`、最後の回答が `(approve, human:<名前>)`、
  問いの sha256 が `content_hash` と一致。
- R3 の主体の一致と R8 の呼んだ主体との照合は、受け取る外部の文（`direction` が `outbound` でない、`human_paste`・`mcp_client`・`markdown_import`）
  だけに掛ける。外へ出す item の actor・consent は元の記録の主体で、読む client と違ってよい。
- R8 は D5.2 より 1 点厳しくした: MCP の client の propose も `consent.granted_by == caller` と `consent.scope == "mcp_scope:discussion:propose"`
  （D1.6 で Celeris が埋める形）を求める。client が人の consent を書いても通らない。外へ出す item では記録の形（人の抜粋・要約の許可か、
  `mcp:` の主体と上の scope）だけを見る。
- scope の名前 `discussion:propose` は `envelope::MCP_PROPOSE_SCOPE` に 1 回だけ書いた。mcp-tools が `McpScope::DiscussionPropose` を足すときは、
  `as_str()` をこの定数にする（同じ文字列を 2 か所に書かない）。
- マスクは秘密の形が見つかった所から行末までを `[masked]` に置き換える。`password:`・`cookie:` は正規表現が前置きだけに当たるので、値ごと消すため。
- 文書の検査（`strategy/mod.rs` の `Input::human_approval`）にも「最後の回答が `approve` で `human:` の主体」の照合がある。子 5 と並行するため
  `mod.rs` は変えず、envelope 側に `human_approved` を置いた。子 5 の統合の後に、文書の検査が `human_approved` を呼ぶ形にそろえる（envelope-fix の進捗の提案）。

### D6: RDC home-dev → celeris-chat → MCP で thread_id・project_id・出典で続ける

既存の経路は ChatGPT → RDC の device agent（専用ユーザー `chatgpt-rdc`）→ `scripts/rdc/celeris-chat` → `celerisctl mcp call` →
`127.0.0.1:18200/mcp`（token）→ Celeris MCP。script も `celerisctl mcp call` も変えない（道具の名前と JSON を渡すだけ）。

1. 人が Web か `celerisctl`（人の API token）で、議論に使う message を共有する（D4.1 の `shares`）。秘密を含む message は 422 で止まる。
2. ChatGPT が `celeris-chat discussion_snapshot '{"project_id":"01M2WTS3DKNZBSZ2JMVB4CZMBW"}'` を呼ぶ。返り値の `thread_id`・`project_id`・各 item の
   `citation`・`content_hash` が以後の会話の鍵になる。
3. ChatGPT は必要なら `celeris-chat discussion_evidence_search '{"project_id":"…","query":"…"}'` で出典を探す。
4. 人が ChatGPT の中で採用したい案を選んだら、ChatGPT が
   `celeris-chat discussion_propose '{"thread_id":"…","idempotency_key":"…","source_label":"chatgpt","body":"…","references":[{"kind":"chat",…}],"confidence":"estimated","decision_state":"proposed"}'`
   を呼ぶ。同じ呼び出しを繰り返しても 1 件になる。
5. 次の `discussion_snapshot` に、その案が `kind: "proposal"`・`actor.id: "mcp:<client_id>"`・出典付きで出る。CoS の discuss の返事は、人が共有した
   ときだけ出る。
6. 続きの会話は同じ `thread_id` を渡す。ChatGPT の会話を新しく始めても、`project_id` だけで同じ固定の thread に戻る。

- Muse など MCP 対応の他の client は、同じ 3 道具を HTTP（token）か `celerisctl mcp stdio` で呼ぶ。推奨 scope は `discussion:read,discussion:propose`
  だけ（`console:instruct`・`db:read`・`api:read`・`knowledge:read` を渡さない）。
- 決定 `ext-client-scope` の答え b により、`chatgpt-rdc-2` の推奨 scope は `knowledge:read,knowledge:propose,tasks:read,tasks:interact,console:instruct,db:read,api:read,discussion:read,discussion:propose`
  になる。`decision_answer`・`task_plan_gate` は `tasks:decide` に移るので、`tasks:decide` を持たない `chatgpt-rdc-2` は scope を変えなくても人の決定に
  答えられなくなる。`db:read`・`api:read` は残るので、`chatgpt-rdc-2` では D2 の範囲の制限は DB と API の全読みに対して効かない。これは人が受け入れた
  運用で、D2 の制限は `db:read`・`api:read` を持たない client（Muse などの推奨 scope）を守る。
- 本番の scope の置き換え（`celerisctl mcp client scopes chatgpt-rdc-2 --scope …`）と、本物の RDC home-dev・ChatGPT での確認は、昇格の後に人が行う。
  手順は scope-guard と rdc-path の進捗の「本番への移行」に書く。

### D7: 試験と WorkUnit の書き込み範囲

#### D7.1 試験の名前（接頭辞）と守る不変条件

| unit | 接頭辞 | crate |
|---|---|---|
| envelope-fix | `external_exchange_envelope_` | task-core |
| exchange-core | `external_exchange_`（`external_exchange_envelope_` 以外） | task-core |
| mcp-tools | `mcp_discussion_` | celeris-mcp |
| rest-import | `chat_exchange_` | task-api |
| scope-guard | `mcp_external_scope_` | celeris-mcp |
| rdc-path | `rdc_celeris_chat_` | celerisctl・celeris-mcp |

#### D7.2 負の試験の一覧

どれも base では、道具・route・関数が無いか、R10 が自己申告の承認を受理するので落ちる（各 unit の進捗に base の出力の要点を書く）。

| 種類 | 試験 | 守ること |
|---|---|---|
| 外部の発言を承認に数えない | `external_exchange_envelope_self_reported_approval_rejected` | envelope の `approval` の欄は R10 で拒否 |
| 同上 | `external_exchange_envelope_mcp_answer_not_approval`・`external_exchange_envelope_cos_answer_not_approval` | `by=mcp:`・`cos:` の答えは R10 を満たさない |
| 同上 | `external_exchange_envelope_approval_sha_mismatch` | 別の sha256 への人の承認は使えない |
| 同上 | `mcp_discussion_propose_author_is_mcp_not_human` | propose の message の author は `mcp:<client_id>`、`actor.kind = mcp` |
| 同上 | `mcp_external_scope_mcp_answer_not_strategy_approval` | MCP の答え（`by=mcp:`）は子 2 D2.2 の承認に数えない |
| 同上 | `chat_exchange_import_injection_not_approval` | 取り込んだ文の message は discuss で、それを引いた CoS operation は拒否 |
| prompt injection | `external_exchange_envelope_injected_text_cannot_approve` | 注入文（「以前の指示を無視」「approve the plan」）を含む文は R11 で `discussion`・`proposed` から進めない |
| 同上 | `external_exchange_import_injection_stays_discussion` | 取り込みの `decision_state` が進まない |
| 同上 | `mcp_discussion_injection_quote_no_escalation` | 注入文を含む引用の propose でも mode は discuss、決定・task・Roadmap は変わらない |
| identity と scope | `external_exchange_envelope_actor_identity`・`external_exchange_envelope_consent_other_human` | `actor.id`・`consent.granted_by` が呼んだ主体と違えば R3・R8 |
| 同上 | `mcp_discussion_scope_missing_rejected`・`mcp_discussion_tools_listed_by_scope` | scope の無い client は 3 道具を一覧でも呼び出しでも使えない |
| 同上 | `mcp_discussion_other_project_thread_rejected`・`external_exchange_snapshot_other_project_rejected`・`chat_exchange_import_other_project` | 別案件の thread・別案件の envelope は拒否（R6） |
| 同上 | `chat_exchange_import_non_human_forbidden`・`chat_exchange_share_non_human_forbidden` | operator・mcp・cos・認証無しは 403 |
| 同上 | `chat_exchange_consent_granted_by_is_caller` | 書き出し・取り込みの consent は呼んだ人 |
| 権限昇格 | `mcp_external_scope_decision_answer_needs_tasks_decide`・`mcp_external_scope_plan_gate_needs_tasks_decide`・`mcp_external_scope_task_approve_needs_tasks_decide` | `tasks:interact` だけの client は人の決定・plan・task の承認に答えられない |
| 同上 | `mcp_external_scope_recommended_has_no_decide` | 文書と `setup-chatgpt-rdc.sh` の推奨 scope に `tasks:decide` が無い |
| 二重取り込み | `external_exchange_import_idempotent_same_hash`・`external_exchange_import_conflict_different_hash` | 同じ key と本文は 1 件、違う本文は R7 |
| 同上 | `mcp_discussion_propose_duplicate_single`・`chat_exchange_import_duplicate`・`chat_exchange_import_conflict` | MCP・REST の両方で同じ |
| 秘密 | `external_exchange_snapshot_masks_secret`・`mcp_discussion_output_has_no_secret` | 返り値に token・cookie・webhook の形が出ない |
| 同上 | `chat_exchange_import_secret_rejected`・`chat_exchange_share_secret_rejected` | 秘密を含む取り込み・共有は 422 |
| 同上 | `rdc_celeris_chat_token_not_in_output` | celeris-chat の標準出力・標準エラーに token の値が出ない |
| 機微な情報の既定の非送出 | `external_exchange_snapshot_default_empty` | 共有が無ければ snapshot は空 |
| 同上 | `external_exchange_snapshot_omits_confidential`・`chat_exchange_export_confidential_default_excluded` | confidential・`outbound=false` の文書は `include_confidential` の共有が無ければ出ない |
| 同上 | `mcp_discussion_evidence_search_excludes_unshared_and_confidential` | 検索は共有の範囲の外と knowledge を返さない |
| 同上 | `external_exchange_snapshot_omits_retracted`・`external_exchange_snapshot_omits_edited_since_share` | 撤回・共有の後の編集は出さない |
| 入力長 | `external_exchange_import_too_long` | 上限を超えた入力は切り詰めずに拒否 |
| 往復 | `external_exchange_markdown_json_roundtrip_hash` | Markdown と JSON の往復で `content_hash` が保たれる |
| RDC の経路 | `rdc_celeris_chat_snapshot_propose_continue` | celeris-chat → `celerisctl mcp call` → 試験の MCP server で snapshot → propose → 次の snapshot に出典付きで出る |

既存の試験がこれらを検出できない理由: 3 道具・4 route・`discussion:*` の scope・共有の event は base に無く、R10 の既存の fixture は自己申告の
`approval` を「承認あり」の正の case として使っている（子 2 の試験は自己申告の承認を受理することを期待している）。

#### D7.3 WorkUnit の書き込み範囲（子 5 と重ねない）

子 5 が書く `crates/task-core/src/strategy/mod.rs`・`crates/task-api/src/cos/strategy_gate*`・`control-ts/` は、どの unit も変えない。
`ChatPostMessageRequest`・`ChatRelayRequest` に欄を足さない。

| unit | 書いてよい path |
|---|---|
| exchange-design | `agent-docs/adr/`・`agent-docs/progress/` |
| envelope-fix | `crates/task-core/src/strategy/envelope.rs`（と `envelope/`）・`crates/task-core/src/strategy/fixtures/`・`crates/task-core/src/strategy/tests.rs`・`agent-docs/progress/`・`agent-docs/adr/` |
| exchange-core | `crates/task-core/src/discussion_exchange.rs`（と `discussion_exchange/`・`discussion_exchange_tests.rs`）・`crates/task-core/src/lib.rs`（`pub mod` の 1 行）・`crates/task-core/src/chat/`（既存の関数の公開範囲の変更と、`chat_events` の 2 つの type の追加だけ。本体に関数を足さない）・`crates/task-core/Cargo.toml`・`Cargo.lock`・`agent-docs/progress/`・`agent-docs/adr/` |
| mcp-tools | `crates/celeris-mcp/`・`crates/task-core/src/mcp.rs`（scope の 2 つ）・`docs/guides/mcp.md`・`Cargo.lock`・`agent-docs/progress/`・`agent-docs/adr/` |
| rest-import | `crates/task-api/`（`cos/strategy_gate*` を除く）・`crates/celerisctl/`・`docs/api/v1/`・`web/api/generated/`・`Cargo.lock`・`agent-docs/progress/`・`agent-docs/adr/`（2026-10-09 の表を含む） |
| scope-guard | `crates/celeris-mcp/`・`crates/task-core/src/mcp.rs`・`crates/celerisctl/src/commands/mcp.rs`・`docs/guides/mcp.md`・`docs/guides/chatgpt-rdc.md`・`scripts/rdc/`・`agent-docs/progress/`・`agent-docs/adr/` |
| rdc-path | `crates/celeris-mcp/tests/`・`crates/celeris-mcp/Cargo.toml`・`crates/celerisctl/tests/`・`crates/celerisctl/Cargo.toml`・`Cargo.lock`・`scripts/rdc/`・`docs/guides/mcp.md`・`docs/guides/chatgpt-rdc.md`・`agent-docs/progress/`・`agent-docs/adr/` |
| close | `agent-docs/progress/` |

- 子 5 と重なりうる file は `crates/task-core/src/lib.rs`（module の宣言の行）・`Cargo.lock`・ADR 2026-10-09 の表・`crates/task-api/src/schema.rs`。
  行が違えば統合の機械的な merge で解ける。同じ行の衝突なら、その unit は進捗に書いて止まり、子 5 の統合を待つ。
- 同じ段の mcp-tools と scope-guard は `crates/celeris-mcp/`・`crates/task-core/src/mcp.rs` を共有するので、scope-guard は mcp-tools の後に走る（計画の依存のとおり）。

## 足すもの・足さないもの

- 足す port と理由:
  - MCP の道具 3 つ（D1）: 外部 LLM に、人の共有の範囲・出典・hash・idempotency を持つ vendor-neutral な契約を渡すため。既存の `console_*`・`db_query`・
    `api_get` はこの範囲を表せない。
  - `McpScope` の 2 つ（`discussion:read`・`discussion:propose`）: D1.1 のとおり、CoS への指示と全読みを渡さずに議論だけを渡すため。
  - REST の 4 route（D4.1）と `celerisctl` の subcommand: MCP の無い client（ChatGPT のどのプランでも）と、人の共有・取り込みの主体を `Principal::Human` に
    限るため。MCP の道具からは共有も取り込みもできない。
- 足す記録: `chat_events` の type 2 つ（`exchange_shared`・`exchange_share_revoked`）と、`chat_messages.metadata_json` の key `exchange`。既存の列に入れる。
- 足す分岐: 取り込み・共有・書き出しで `Principal::Human` 以外を 403 にする分岐、snapshot・検索で共有の外・失効・confidential を出さない分岐、
  envelope の R3・R8・R10・R13 を記録から導く分岐。
- 足さないもの: 新しい DB 表・列・migration、Manager・trait・状態機械、Agent サービス・gateway、ChatGPT の会話全文・Memory の取得・同期、browser の
  login・Cookie、外部の URL を取りに行く処理、`ChatPostMessageRequest`・`ChatRelayRequest` の欄、承認・publish の経路、外部公開の MCP の配置。
- 消すもの: envelope の自己申告の `approval` を読む経路（D5）、`tasks:interact` で `decision_answer`・`task_plan_gate` を呼べる経路と、`docs/guides/mcp.md` §8.2・
  `docs/guides/chatgpt-rdc.md`・`scripts/rdc/setup-chatgpt-rdc.sh` の旧い推奨 scope の記述（scope-guard、決定 b）。

## 実装前 brief

1. 根本原因: 外部 LLM の入口が全権に近い道具（`db_query`・`api_get`・`decision_answer`）と `console_*` しか無く、人の共有の範囲・出典・consent・idempotency を
   持つ契約が無い。envelope の R10・R13 は文書の自己申告の承認を受理する。
2. 責務 owner: 道具と scope は `crates/celeris-mcp` と `crates/task-core/src/mcp.rs`、REST は `crates/task-api` の chat、交換の規則は `task_core::strategy::envelope`
   （検査）と兄弟 module `task_core::discussion_exchange`（snapshot・書き出し・読み込み）。
3. 既存の再利用先: MCP の認証・scope・監査・流量制限、`chat_mcp_instruct` の author `mcp:<client_id>`、`client_message_id` の一意索引、子 3 の
   `discussion_mode`・`ChatCitation`・`message_revised`、子 2 の `ApprovalRecord`・`approval_subject`・`contains_secret`、`chat_events` の追記、
   `scripts/rdc/celeris-chat`・`celerisctl mcp call`。
4. 選択肢: (a) 既存の MCP の registry と chat の REST に足し、規則は task-core の純関数に置く（採用。認証・権限の正本が 1 つ）。(b) 外部 LLM 用の gateway と
   会話 DB（不採用。新しいサービスと表の禁止に反する）。(c) ChatGPT の会話を browser で同期する（不採用。Memory・全文の取得、login・Cookie の禁止に反する）。
   共有の記録の置き場所は、(i) `chat_events` の追記（採用。追記だけで監査に残り、列を足さない）、(ii) message の `metadata_json` の書き換え（不採用。
   取り消しで記録が上書きされる）、(iii) 新しい表（不採用）を比べた。
5. 新しい状態・分岐・fallback・port: 上の「足すもの」のとおり。fallback・新旧切替の flag は無い。
6. 撤去対象: 上の「消すもの」のとおり。
7. 不変条件: 外部 LLM の発言・MCP の答えは human actor にも承認にもならない。人が共有していない範囲・confidential・knowledge は既定で外に出ない。
   同じ key と本文は 1 件、違う本文は拒否。注入文は decision_state も権限も変えない。新しい表・migration が無い（close の `git diff` の check）。
   Project.request・承認済み Roadmap を書かない。本番昇格・publish・外部公開 MCP の配置をしない。試験は D7.2。

## 未確認

- 本物の RDC home-dev・ChatGPT（RDC と custom remote MCP）・Muse での確認は未確認。昇格の後に人が行う。
- `task_answer`・`task_decompose`・`task_comment` は決定 b の範囲（承認系の 2 道具）の外なので `tasks:interact` に残る。これらを外部 client に渡すかは
  この ADR では決めていない。
- 子 3 の web の chat 画面に、取り込んだ message（`metadata_json.exchange`）を外部の文として示す表示は子 7 が決める。この子では wire の `ChatMessage` を変えない。
