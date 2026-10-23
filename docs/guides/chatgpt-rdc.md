---
tasks: [01M5MWG7FB4V940XSF0C4F93CM]
---
# ChatGPT（Remote Desktop Commander）から Celeris を読む・使う

この文書は ChatGPT が RDC の端末（専用ユーザー `chatgpt-rdc`）で読む使い方です。`scripts/rdc/setup-chatgpt-rdc.sh`
が専用ユーザーの `~/CELERIS-CHAT.md` に写します。仕組みと権限は `docs/guides/mcp.md` §8、決定は
`agent-docs/adr/2026-10-23-mcp-read-only-db-and-api.md` にあります。

## 1. 入口は `celeris-chat` だけ

```
celeris-chat --list                       # 使える道具（client の scope で決まる）
celeris-chat <道具> '<JSON の引数>'
```

- 出力は JSON です。エラーは stderr に出て exit code 1（引数の誤りは 2）。
- DB ファイル・API の token・curl はこのユーザーには渡していません。DB と API は次の 2 つの道具で読みます。
- 書き込み（コメント・回答・CoS への依頼）は `task_comment` / `task_answer` / `console_instruct` など、scope で
  許された道具だけです。`db_query` と `api_get` は読むだけです。

## 2. DB を読む: `db_schema` と `db_query`（scope `db:read`）

```
celeris-chat db_schema
celeris-chat db_query '{"sql":"SELECT id, status, title FROM tasks ORDER BY updated_at DESC","limit":20}'
celeris-chat db_query '{"sql":"SELECT seq, ts, json FROM events WHERE task_id = ?1 ORDER BY seq","params":["01M5…"]}'
```

- SQL は SELECT（WITH・WITH RECURSIVE を含む）1 文だけ。`?1` などに `params` を順に結びます。
  `PRAGMA table_info(表)` などの読み取りの PRAGMA は使えます。書き込み・ATTACH・他の PRAGMA は拒否されます。
- 返り値は `{columns, rows, row_count, truncated, truncated_reason?, masked?, elapsed_ms}`。`rows` は配列の配列です。
- 上限: 行数は既定 500・最大 5000（`limit`）、結果 1 MiB、実行 10 秒、同時 2 本。`truncated: true` なら
  WHERE・LIMIT・集計で絞ってください。大きな表（`events` など）は `task_id` や `seq` で絞ると速く済みます。
- 秘密の列（token の hash、session の鍵、browser の login 状態など）は常に NULL で読まれます。どの列かは
  `db_schema` の `masked` に出ます。結果に `masked` があれば、その列が NULL に置き換わっています。
- `events.json` は 1 行 1 出来事の JSON です。`json_extract(json, '$.type')` で種類を見ます。

### よく使う問い合わせ

lane の決定（`routing_decided`）の lane と Jev の提案の集計:

```sql
SELECT json_extract(json, '$.record.features.decision.lane') AS lane,
       json_extract(json, '$.record.features.decision.lane_assist.jev_lane') AS jev_lane,
       json_extract(json, '$.record.features.decision.lane_assist.classifier_status.status') AS classifier,
       count(*) AS n
FROM events
WHERE json_extract(json, '$.type') = 'routing_decided'
GROUP BY 1, 2, 3
ORDER BY n DESC
```

Jev の入出力（直近 20 件の lane_assist 全体）:

```sql
SELECT task_id, ts, json_extract(json, '$.run_id') AS run_id,
       json_extract(json, '$.record.features.decision.lane_assist') AS lane_assist
FROM events
WHERE json_extract(json, '$.type') = 'routing_decided'
ORDER BY ts DESC
LIMIT 20
```

1 つの task の出来事の流れ:

```sql
SELECT seq, ts, json_extract(json, '$.type') AS type, json
FROM events WHERE task_id = ?1 ORDER BY seq
```

決定ログ（run ごとの提案 lane・決定 lane・実行した model・結果）:

```sql
SELECT run_id, task_id, proposed_lane, decided_lane, fixed_lane, jev_lane, lane_basis,
       exec_harness, exec_model, run_end, wu_verdict, wall_ms
FROM decision_log_runs ORDER BY updated_at DESC LIMIT 50
```

evidence 表（genre × harness × lane × model の合格率と費用）:

```sql
SELECT genre, harness_id, lane, model_id, n, pass, pass_lower, wall_p90_s, usd_per_pass, low_confidence
FROM routing_evidence ORDER BY genre, harness_id, lane
```

完了単位の結果（lane・Jev・token・時間）:

```sql
SELECT unit_kind, unit_id, genre, lane, jev_lane, jev_specificity, passed, model_id, tokens_in, tokens_out,
       wall_s, completed_at
FROM completion_outcomes ORDER BY completed_at DESC LIMIT 50
```

## 3. API を読む: `api_get`（scope `api:read`）

```
celeris-chat api_get '{"path":"/api/v1/tasks","query":{"status":"running"}}'
celeris-chat api_get '{"path":"/api/v1/tasks/01M5…/events"}'
celeris-chat api_get '{"path":"/api/v1/tasks/01M5…/routing"}'
celeris-chat api_get '{"path":"/api/v1/routing/evidence"}'
celeris-chat api_get '{"path":"/api/v1/schema"}'      # endpoint と型の一覧
```

- Celeris の daemon が `/api/v1` に GET を代わりに送ります。GET 以外はできません。
- `path` は `/api/v1/` で始め、query は `query` に object で渡します（path に `?` は書かない）。
- 返り値は `{status, content_type, body}`。4xx/5xx もそのまま返ります。応答は 1 MiB・30 秒まで。
- secret・保存 credential・browser の identity と Live View・TOTP の接続・login・添付の生データ・SSE（stream）の
  path は拒否されます。

## 4. その他の道具

```
celeris-chat tasks_list '{"status":"blocked","limit":5}'
celeris-chat knowledge_search '{"query":"pegasus"}'
celeris-chat console_instruct '{"text":"調査結果をタスクにして"}'
celeris-chat console_reply '{"task_id":"<上の task_id>","wait_secs":30}'
celeris-chat task_comment '{"id":"<task ulid>","text":"見ました。続けてください"}'
```

呼び出しは Celeris の `mcp_calls` に記録されます（`db_query` の SQL と `api_get` の path は残り、結果は残りません）。
