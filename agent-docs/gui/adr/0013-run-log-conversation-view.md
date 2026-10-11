---
tasks: [01M3MCA20JSAKGENH571NZES1F]
---
# ADR-GUI-0013: run ログを会話形式で表示する（harness ごとの変換層 + 共通の表示部品）

- 日付: 2026-09-28
- 状態: **Accepted**（人の依頼「run のログが生の JSON を貼っただけで読みにくい。Claude Code / Codex の remote-control のブラウザ版のようにしてほしい」）
- 関連: ADR-GUI-0006 D2 / D4（`stdout.jsonl` の分類と生ログ画面）、DESIGN §4.3（`?offset=` の追尾）、§8.3（LLM の出力は信用しない）

## 1. 文脈

`/tasks/:id/runs/:runId` は `stdout.jsonl` を 1 行 1 枠で並べ、`lib/stream-json.ts` が assistant の最初の本文・最初の
tool_use・result だけを拾い、残りは生の JSON 行のまま出していた。本番の run（2026-09-28 時点の 73 run）を数えると、
行の大半は codex の `item.*`（command_execution / file_change）、claude-code の `system:thinking_tokens`・
`tool_result`・`thinking` で、ほぼ全部が生の JSON として見えていた。WorkUnit の run も同じ画面（`RunSummary.work_unit`）。

本番の harness と形式: claude-code（`--output-format stream-json`）、codex（`exec --json`）、opencode（ACP の JSON-RPC）。

## 2. 決定

### D1. 変換層 `app/lib/run-log.ts` は harness ごとの adapter に閉じ込め、共通の `RunLogEvent` を返す

- 1 行ずつ形式を判定する（`jsonrpc` → ACP、`item.*` / `turn.*` / `thread.*` → codex、`assistant` / `user` / `system` / `result` 等 → claude-code）。
- 種類: `message`（assistant の本文。Markdown）/ `thinking` / `tool`（名前・主な引数の要約・入力・結果・差分）/ `command`（コマンド・終了コード・出力）/
  `file_change`（パスと差分）/ `error` / `usage`（トークン・費用・所要時間・turn 数）/ `system`（init・hook・rate limit 等の短い 1 行）/ `unknown`。
- 状態を持つ結合: claude-code の `tool_use` と後続の `tool_result`（`tool_use_id`）、codex の `item.started` と `item.completed`（`item.id`）、
  ACP の `tool_call` と `tool_call_update`（`toolCallId`）、ACP の連続する `agent_message_chunk` / `agent_thought_chunk` を 1 つのイベントにする。
  同じ見出しの `system` が続いたら（`thinking_tokens` の連打、heartbeat 等）1 行にまとめて件数を出す。
- **どのイベントも元の行（`raw: string[]`）を全部持つ**。JSON でない行・知らない `type` は `unknown` として捨てずに残す。
- 分類は表示のためだけ。成否・リトライ可否などの celeris の判断は再実装しない（ADR-GUI-0006 D2 と同じ）。

### D2. 表示部品 `components/RunLog.tsx` は harness を知らない

- 種類ごとにアイコン・色・見出しを変え、時系列に並べる。長い本文（ツール結果・コマンド出力・差分・思考・長い入力）は既定で畳み、
  開いたときだけ中身を描く（長い run でも DOM を膨らませない）。
- 各イベントに「JSON」（元の行の表示）と「コピー」を付ける。run 全体は従来の「生テキスト」切り替え（CodeViewer）に「全体をコピー」を足す。
- スマホ（360〜412 px）で横にはみ出さない（長い語は折り返し、コード・差分は枠の中だけ横スクロール）。

### D3. 追記は既存の 1 秒ごとの `?offset=` 追尾のまま。新しい配信経路を作らない

行の配列が増えるたびに全体を変換し直す（純粋関数。結合が行をまたぐので差分変換より単純で正しい）。

### D4. 秘密値

表示するのは従来どおり `stdout.jsonl` の中身だけ（新しいファイル・API を読まない）。生テキスト表示で既に見えていたもの以上は出さない。
コピーはユーザが押したときだけ、表示中の行と同じ文字列をクリップボードに入れる。

## 3. 採らない

- SSE / WebSocket での新しい配信（D3）。
- celeris 側で正規化したイベントを返す API（GUI の表示の都合なので GUI 側の変換で足りる。harness が増えたら adapter を足す）。
- 差分の構文強調ライブラリの追加（依存を増やさない。`+`/`-` の行の色分けだけ）。
