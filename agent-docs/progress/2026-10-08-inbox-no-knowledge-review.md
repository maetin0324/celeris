---
title: 受信箱に KB の取り込み待ち（knowledge_review）を出さない
tasks: [01M4CRRQRMXE9F7478FRCVTTDD]
status: done
updated: 2026-10-08
completed: 2026-10-08
---
# 受信箱に KB の取り込み待ちを出さない（人の決定 2026-10-08）

ADR-0133 末尾の付記「KB の取り込み待ちを受信箱に出さない」で D1.2 の表・D2 の種類・未解決事項を改訂。

## やったこと

- task-ops `human_inbox`: `InboxKind::KnowledgeReview`・`KnowledgePending`・`human_inbox(.., knowledge)` 引数・
  `knowledge_review` の組み立てを削除（種類ごと無くした）。呼び出し元（celeris notify / triage、dispatcher の
  CoS triage、各試験）の末尾の `None` を外した。
- task-api `human_feed`: KB の `_inbox/` を数えない。answer の `KnowledgeReview` 分岐を削除。
  `GET /inbox/items/knowledge_review`・answer は 404 `inbox-item-gone`、`?kind=knowledge_review` は 400。
- 通知・CoS 受信箱に代わりの項目は作っていない（CoS 側は元から `None` を渡していた）。KB の候補の仕組みは不変。
- 生成物: `docs/api/v1/api-v1.schema.json`、`gui/app/celeris/types.ts`（base で古かった browser の欄も追従）、
  `web/api/generated/`。
- web: `INBOX_KINDS`・`KIND_LABELS` から削除、`inbox-model.test.ts` に `isInboxKind("knowledge_review") === false`、
  fake daemon の 5 件目を `cluster_login-pegasus` に置換（件数 5 を前提とする e2e を保つ）、`fake-daemon.test.ts` の期待を更新。
- 文書: ADR-0133（D1.2 行・D2 の kind 欄・並び・未解決を閉じる・付記）、ADR 2026-10-04-web-inbox-notifications-screens D6、
  `docs/api/v1/inbox-notifications.md`、`docs/api/v1/gui-api.md` §5.1。`docs/ops/inbox-notifications.md` には該当記述なし。

## 証拠

- `bash scripts/dev/test-parallel.sh` → exit 0、passed 4743 / failed 0 / ignored 14。
- `cargo clippy --workspace -- -D warnings` → exit 0（`--all-targets` も警告なし）。`cargo fmt --all -- --check` → exit 0。
- 新試験: `task_ops::human_inbox::tests::human_inbox_never_has_knowledge_review`、
  `task-api tests/knowledge.rs::candidates_never_appear_in_the_human_inbox`（候補 2 件で受信箱 0・404 ×2・通知に無し・accept 200）。
- `pnpm -C web lint`（exit 0）・`typecheck`（exit 0）・`test`（vitest 608 passed、node --test 77 passed）。
- `pnpm -C web e2e:all` で受信箱関係 9 spec（parity/inbox・work/inbox-notifications・shell/inbox-badge・shell/tabbar・
  chat/cards・parity/knowledge・parity/notifications・states・realtime/refetch-scope）→ 97 passed / 3 skipped。
- release.sh / verify.sh: 報告本文と下の節を参照。

## 未解決事項

- `python3 scripts/dev/check-architecture-map.py` は base から NG（存在しないパス 3 件、本変更と無関係）。

## 提案

- なし。
