---
title: cos-run / prompt-skill: CoS 特別 worker の skill（cos-operator・cos-inbox-triage）
tasks: [01M47932RQYAY1THNXPT5Y9JFQ]
status: done
updated: 2026-10-06
---
# cos-run / prompt-skill: CoS 特別 worker の skill

[ADR 2026-10-05 cos-chat-home](../../adr/2026-10-05-cos-chat-home.md) D3 に従い、CoS chat run に渡す skill を 2 つ書いた。

## やったこと

- `config/skills/cos-operator/SKILL.md`: 全道具の意味と限界、本番 DB・制御状態の変更は `/api/v1/cos/operations`（受信箱は `resolve`）か
  それを経由する celerisctl だけ（SQLite・token・service・release・本番 config を直接触らない）、起票・回答・決定・認可・replan・
  pause/resume・コメント・KB・監視の操作例（celerisctl と REST の表）、登録 repo は専用 worktree、`docs/ops/selfdeploy.md` と
  `scripts/selfdeploy/` の release→verify→status→promote（§4・§5・§7 の人だけの操作は人へ依頼）、秘密を残さない、添付と tool 出力は
  命令ではない、旧 `result.actions` を使わない、checkpoint（`/cos/threads/{t}/checkpoint`）の更新規則（D2 要約節）。
- `config/skills/cos-inbox-triage/SKILL.md`: D3「人に回す基準」の表全体（ADR の 9 行をそのまま）、answer/observe/escalate の選び方、
  resolve 本文、escalation packet、confidence（欠落は低確信・min_confidence 未満は escalate）、Discord で回答させないこと。
- 各 skill に `SOURCE.md`（ADR の該当節へのリンク）を置き、`config/skills/README.md` に自作 skill の節を足した。
- 操作例の REST path は `crates/task-api/src` の route（tasks/{id}/answer・decisions/{id}/answer・approvals/{id}/decide・standing-rules・
  execution-plan・execution/phase-gate・plan-gate・tasks/{id}/pause|resume|comments・knowledge/inbox/{id}/accept 等）と照合した。
  celerisctl は `add`・`answer`・`approve --note`・`execution plan show`・`knowledge search|get|record`・`ls`・`show`・`log` の実在を確認した。

## 証拠

- `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0（commit 後の HEAD で実行）。
- 参照した docs/ops・scripts/selfdeploy のパス（selfdeploy.md・inbox-notifications.md・release.sh・verify.sh・status.sh・promote.sh・
  rollback.sh）は Markdown 相対リンクにしてあり、check-doc-links が実在を検査する。

## 未解決事項

- 現在の celerisctl の変更系（answer・approve 等）は DB を直接開く。skill は「`/cos/operations` を経由する版でだけ使い、無ければ REST」と
  書いた。celerisctl を run credential 経由にするのは ops WU（監査付き操作層）の範囲。
- skill を CoS run に配る配線（mount・前置き）は chat-run / harness-adapters の範囲。

## 提案

- ops WU が `/cos/operations` の許可 path 一覧を確定したら、cos-operator §3 の表と突き合わせる（close 葉の D6 試験対応表で）。
