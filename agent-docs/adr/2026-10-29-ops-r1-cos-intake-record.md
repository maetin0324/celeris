---
tasks: [01M62TJ5V2B65Y0C804HHQ4E3Q]
---
# ADR 2026-10-29: ops-r1 B の受付は案件 CoS thread の人の message として記録する

- 状態: 採用（実装済み）
- 関係: [2026-10-27-relayed-instructions](2026-10-27-relayed-instructions.md) D1（message の `author` は認証した主体）、
  ops-r1 棚卸しの g-b-cos-intake

## 1. 文脈
ops-r1 の B は「人が案件 CoS に頼む」通常運用を測る。既存の B executor（`celeris_exec.rs`）は `POST /api/v1/tasks` で
直接投入するので使えない。測定 daemon は本番昇格済み release 94f86928dc13 の binary のまま使い、daemon のコードは変えない。

## 2. 決定
- D1: `celeris-campaign ops-r1-intake` を足す。`--bind-message <id>`（人が web・celerisctl で送った message を結ぶ）か
  `--send`（人の token で `--request-file` の固定依頼文を `POST /chat/threads/{t}/messages` で送る）のどちらか 1 つ。
  呼ぶ API は chat thread の GET 2 種と POST messages だけで、`POST /api/v1/tasks` は呼ばない。
- D2: thread は open の `kind=project` に限る。message は `role=user`・取り消されていない・本文の sha256 が依頼文と一致、
  `author` が `human:` のときだけ記録する。operator の relay・MCP・author 無しは記録しない（actor を誤って人に数えない）。
- D3: 受付時刻は daemon が記録した message の `created_at`。記録は API の値だけから作り（道具の時計を入れない）、
  `--out` が既にあれば同じ bytes のときだけ受ける。`--send` の `client_message_id` は `ops-r1-intake:<trial_id>` で、
  再実行しても daemon が同じ message を返す。
- D4: actor は human-ops の語彙（`human`・`external_operator`・`mcp_client`・`unknown`）に写し、`id` は `author` のまま残す。

## 3. 影響
daemon・task-*・web は変えない。trial への消費帰属（g-b-trial-attribution）と trace（g-b-trace-export）は別の葉。
