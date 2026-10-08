# cos-operator: 添付の引き渡し（pin）

チャットの添付（screenshot・PDF など）を後続の task や KB 候補に渡すときに読む（ADR cos-chat-home D4、
ADR 2026-10-07 cos-live-fixes D1・D2）。要点は Core の「添付」節にもある。

- pin された添付だけが後続の run の入力・KB 候補の原ファイルとして残る。
- **元チャットの run path を後続に渡さない**。前置きの添付の path は chat run の一時の場所で、run が終われば消える。
  起票本文・objective・KB 本文・コメントに path を書かない。
- 渡し先: **screenshot など画像は task へ**（`owner_kind: "task"`）、**PDF など資料は KB 候補へ**（`owner_kind: "knowledge_inbox"`）。
- 添付 id は前置きの添付一覧の id（このチャット thread の添付だけ。他の thread の添付は 422）。

## task へ渡す（画像）: 起票と pin を 1 回で

起票の body に `attachment_ids` を入れる。task の作成と pin が同じ transaction で入るので、最初の run の入力に
添付が必ず載る。「起票してから pin」はしない（起票で task が ready になり、pin が届く前に worker run が始まる）。

```json
{"idempotency_key":"create-screen-fix-1","expected_revision":null,
 "reason":"人がチャットで screenshot つきで画面修正を依頼した（seq 12）","policy_version":"3",
 "request":{"method":"POST","path":"/api/v1/tasks","body":{
   "title":"画面修正","objective":"依頼の全文（添付の screenshot の画面）",
   "acceptance":[{"type":"reviewer","text":"添付の screenshot の崩れが直っている"}],
   "attachment_ids":["<添付 id>"]}}}
```

応答の operation が `state: "applied"` で、`result` に `task_id` と送った `attachment_ids` が全部返っていることを確かめてから
「引き渡し済み」と言う。422 `invalid_attachment`（存在しない・削除済み・期限切れ・重複・他の thread の添付）なら
**task も作られていない**。理由つきで人に書き、添付を確かめて同じ本文（別の idempotency_key）で送り直すか人に回す。

## KB へ渡す（PDF など資料）: 候補の作成と pin を 1 回で

`POST /api/v1/knowledge/inbox`（action `knowledge.record`）の body に `attachment_ids` を入れる。候補ファイルと
pin（provenance）が一緒に入り、どちらかが失敗すれば候補も残らない。本文の形は [operations.md](operations.md) の「KB 候補」。

`celerisctl knowledge record --title … --scope project:<slug> --source message:<id> --attachment-id <添付 id> < body.md`
も同じ operation を送る（CoS credential のときは API 経由。`--reason` で理由を渡す）。

応答が `applied` で `result` に候補 `id` と送った `attachment_ids` が全部返っていれば引き渡し済み。
`GET /api/v1/knowledge/inbox/<id>` の `provenance` にも添付が出る。422 `invalid_attachment` なら候補も作られていない。

## 既にある task・候補へ後から足す

このときだけ `POST /api/v1/chat/attachments/{id}/references` を `/cos/operations` に包んで送る
（直接叩くと 422 `cos_audit_context_required`）:

```json
{"idempotency_key":"pin-<添付 id>-<owner id>","expected_revision":null,
 "reason":"…","policy_version":"3",
 "request":{"method":"POST","path":"/api/v1/chat/attachments/<添付 id>/references",
            "body":{"owner_kind":"task","owner_id":"<owner id>","idempotency_key":"pin-<添付 id>-<owner id>"}}}
```

再試行は同じ idempotency_key で（同じ key・違う owner は 409）。応答が `applied` で `result` に同じ
`attachment_id`・`owner_kind`・`owner_id` が返っていれば引き渡し済み。404・409・422 なら引き渡していない。

最後に、要約 checkpoint に owner id と pin した添付 id を残す。
