---
tasks: [01M62TJ5V2B65Y0C804HHQ4E3Q, 01M669QB069GND2ZKA1T97QPY9]
---
# ops-r1・ops-r2 B の CoS 受付の記録（celeris-campaign ops-r1-intake）

B の依頼は task API へ直接入れず、測定案件の案件 CoS thread へ、認可済みの operator（または MCP）principal の relay として入れる。
この道具はその message を trial と人の指示の出典に結び、記録 JSON を書く（ADR `agent-docs/adr/2026-10-29-ops-r1-cos-intake-record.md`、
`agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md` D5）。人の token は使わない。測定 daemon の config に人の token を足さない
（token は `ops-r1-measure-config.sh` が作る `operator:ops-r1` だけ）。新しい principal・権限・恒久承認は作らない。

## 出典の file（`--source`）

人が承認票で承認した固定依頼文を relay するとき（`approved_request`）:

```json
{"kind": "approved_request", "daemon": "production", "decision_id": "<承認の id>",
 "said_at": "<人が承認した時刻、RFC 3339 UTC の …Z>", "request_sha256": "<承認票に書かれた依頼文の sha256>",
 "decided_by": "human:<name>", "approved_by": "human:<name>"}
```

人の発言の原文を relay するとき（`chat_message`）は `decision_id` の代わりに `thread_id`・`message_id`（本番 daemon の人の message）を書く。
`said_at` はその message の時刻。知らない欄は拒否する。

## 使い方

```sh
celeris-campaign ops-r1-intake --api-url http://127.0.0.1:<測定 daemon の port> --token-env CELERIS_MEASURE_TOKEN \
  --thread <案件 CoS thread の id> --trial-id <trial_id> --condition-id <condition_id> \
  --request-file <固定依頼文> --source <source.json> --operator-session <実験専用 session の id> \
  --relay --out <raw>/intake.json
```

- `--relay` は operator principal の token（`<state>/token`）で `POST /api/v1/chat/threads/<t>/relays` に
  `{client_message_id, text, said_at, task_ids: [], expires_at: null}` を送る。`client_message_id` は trial から決めるので、再実行しても 1 件。
- 既に relay した message を結ぶときは `--relay` の代わりに `--bind-message <message_id>`。
- `--operator-session` は human-ops の `actor` に使う（省くと relay の author）。
- `--api-url` は測定 daemon の origin（path なし）。本番 daemon には向けない。
- token の値は環境変数で渡し、argv・出力には出ない。
- 本文は `--request-file` の bytes と完全一致が要る（末尾の改行も含む）。

## 出力（`ops-r1-intake/2`）

`/1` の欄（`trial_id`・`condition_id`・`thread_id`・`project_id`・`message_id`・`message_seq`・`accepted_at`（daemon の `created_at`）・
`request_sha256`・`request_bytes`・`intake_mode`（`bind`/`relay`）・`client_message_id`・`actor`（`kind`・`id`））に加えて:

| 欄 | 値 |
|---|---|
| `source` | `kind`・`daemon`・`thread_id`・`message_id`・`decision_id`・`said_at`・`request_sha256`（`--source` の値） |
| `decided_by` | 出典の主体（`{kind: human, id: human:<name>}`） |
| `executed_by` | relay の `author`（daemon が認証した `operator:<name>` か `mcp:<id>`）。`actor` と同じ |
| `approved_by` | 有料試行を承認した人。承認の出典は `source.decision_id` |
| `relayed_by` | relay の `relay.relayed_by` |
| `human_op` | human-ops /2 の relay の op 行に渡す値（`human-op record` の `--actor-kind`・`--actor`・`--decided-by`・`--executed-by`・`--approved-by`・`--credential`・`--request-ref`・`--request-revision`・`--policy-ref`・`--celeris-event-ref` と同じ書式） |

relay の op 行は `actor` が人でないので、人の能動時間に数えない。人の能動時間は人が記録した区間だけで測る
（`docs/ops/eval-raw-record-definitions.md` §3.3）。`ops-r1-assemble` は `/2` と K4 の `/1` の両方を読む。

## 拒否する場合（記録を書かない、exit 1）

- `--source` が無い・知らない欄がある・`daemon` が `production` でない・`approved_request` に `decision_id` が無い・`chat_message` に
  `thread_id`/`message_id` が無い・`said_at` が RFC 3339 UTC でない・`decided_by`/`approved_by` が `human:<name>` でない・
  `request_sha256` が `--request-file` の sha256 と違う。`--relay` ではこの検査を送る前に行う（何も送らない）。
- thread が案件 CoS thread でない、または open でない。
- message が見つからない、`role` が user でない、取り消し済み、本文の sha256 が依頼文と違う。
- `author` が `operator:`・`mcp:` でない。`author` が `human:` の message（人の token で送った）は受けない。
- message に `relay` が無い（出典の無い operator の message）、`relay.relayed_by` が `author` と違う、`relay.said_at` が出典の `said_at` と違う。
- 人の token で `--relay` を呼んだ（daemon が 403 `chat_relay_forbidden` を返す）。
- `--out` に別の記録が既にある。
