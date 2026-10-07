---
title: 葉 live-script — live 台本を scripts/dev/cos-chat-live.sh として入れ docs/ops/cos-chat.md を直す
tasks: [01M4APB5FP8T3TAAE51Z20E3M1]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 live-script: CoS chat の live 台本（ADR 2026-10-07-cos-live-fixes D5）

## やったこと

- `scripts/dev/cos-chat-live.sh`（新規、bash）: live-check2 の WU 成果物 `live2-ops.sh` を元にした。
  - 呼び方 `bash scripts/dev/cos-chat-live.sh <repo> <bin dir> <data dir> dry|full`。引数が 4 つでない・4 つ目が `dry|full`
    以外は usage を出して exit 2。bin dir に `celeris`・`celerisctl` が無い、data dir が本番の置き場（`~/.config/celeris`・
    `~/.local/celeris`・`/local/celeris/state`）、port が使用中のときも exit 2。
  - config・DB・KB・state・log は data dir の下だけ。`CELERIS_*` は呼び手の env を消してから data dir に向ける。
    port は既定 17932（`COS_CHAT_LIVE_PORT`）。
  - 試験用 provider は `concurrency = 4`。
  - 各 turn の `run_id` は POST の応答に頼らず `GET /api/v1/chat/threads/{t}/messages` を `client_message_id` で引いて待つ
    （上限 150 回 × 2 秒の保険）。
  - (d) は `POST /api/v1/tasks` の `attachment_ids`（D1）で起票と pin を 1 operation にするよう依頼し、`cos_operations`
    `task.create` の `applied`・`result_json` の添付 id、`chat_attachment_refs`（owner_kind task）、その task の最初の
    `prompt.txt` の「## 入力の添付」で判定する。別の `attachment.reference` operation の数も記録する。
  - (e) は `POST /api/v1/knowledge/inbox`（D2、operation `knowledge.record`、scope `project:agent-platform`）で候補作成と pin を
    1 operation にするよう依頼し、`knowledge.record` の `applied` と `GET /knowledge/inbox/{id}` の `provenance` で判定する。
  - 判定は `evidence/verdict.txt` に PASS/FAIL で残す。
- `docs/ops/cos-chat.md`: 「実機確認の再実行（cos-chat-live.sh）」節を追加（`cargo build -p celeris -p celerisctl` →
  `bash scripts/dev/cos-chat-live.sh "$PWD" "$CARGO_TARGET_DIR/debug" <data dir> dry|full`、見る証跡）。「添付の引き継ぎ」の
  task・KB の手順を D1・D2 の形（作成時の `attachment_ids`）に直した。front matter の tasks にこの task を足した。

## 証拠

- `bash -n scripts/dev/cos-chat-live.sh` → exit 0
- `bash scripts/dev/cos-chat-live.sh` → usage・exit 2、`… a b c bogus` → usage・exit 2
- `grep -c` で messages 待ち・`concurrency = 4`・`attachment_ids`・`knowledge/inbox` を確認（下の「受け入れ条件」）
- この run では dry も full も実行していない（daemon・LLM を起こさない指示）

## 未解決事項

- 実機の dry / full は未実行。close 以降で、認証の使える host で人か運用セッションが流し、`verdict.txt` を転記する。
- (d) の `prompt.txt` の場所は task id を含む path か本文の task id で探す。daemon の workspace 配置が変わると
  `input-manifest.txt` が空になりうる（`prompt-files.txt` で確かめる）。

## 提案

- なし
