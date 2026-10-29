---
tasks: [01M62TJ5V2B65Y0C804HHQ4E3Q]
---
# ops-r1 B の CoS 受付の記録（celeris-campaign ops-r1-intake）

B の依頼は task API へ直接入れず、測定案件の案件 CoS thread へ人の message として入れる。
この道具はその message を trial に結び、記録 JSON を書く（ADR `agent-docs/adr/2026-10-29-ops-r1-cos-intake-record.md`）。

## 使い方
人が画面（web の案件 CoS thread）か celerisctl で固定依頼文を送った場合:

```sh
celeris-campaign ops-r1-intake --api-url http://127.0.0.1:<測定 daemon の port> --token-env CELERIS_HUMAN_TOKEN \
  --thread <案件 CoS thread の id> --trial-id <trial_id> --condition-id <condition_id> \
  --request-file <固定依頼文> --bind-message <message_id> --out <raw>/intake.json
```

道具に送らせる場合は `--bind-message <id>` の代わりに `--send` を付ける（人の token の環境変数名を `--token-env` に渡す）。

- `--api-url` は測定 daemon の origin（path なし）。本番 daemon には向けない。
- token の値は環境変数で渡し、argv・出力には出ない。
- 本文は `--request-file` の bytes と完全一致が要る（末尾の改行も含む）。

## 出力（`ops-r1-intake/1`）
`trial_id`・`condition_id`・`thread_id`・`project_id`・`message_id`・`message_seq`・`accepted_at`（daemon の `created_at`）・
`request_sha256`・`request_bytes`・`intake_mode`（`bind`/`send`）・`client_message_id`・`actor`（`kind`・`id`）。

## 拒否する場合（記録を書かない、exit 1）
- thread が案件 CoS thread でない、または open でない。
- message が見つからない、`role` が user でない、取り消し済み、本文の sha256 が依頼文と違う。
- `author` が `human:` でない（operator の relay・MCP・author 無し）。`--send` でこれが起きたら message は送られているので、
  返った message_id を人が確かめる。
- `--out` に別の記録が既にある。
