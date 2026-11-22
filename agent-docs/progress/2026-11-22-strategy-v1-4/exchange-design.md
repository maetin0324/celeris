---
title: 戦略層 v1 子 4 exchange-design — 外部 LLM との議論の交換の ADR
tasks: [01M7XQ9QK1KC34Q2A6PJ960TJB]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# exchange-design（WorkUnit。子 4 の段 design）

完了日: 2026-11-22。設計の記録は [ADR 2026-11-22-external-llm-discussion-exchange](../../adr/2026-11-22-external-llm-discussion-exchange.md)。
source（`crates/`・`web/`・`scripts/`・`docs/`）は変えていない。

## 結論

- MCP の道具は `discussion_snapshot`・`discussion_evidence_search`（scope `discussion:read`）と `discussion_propose`（scope `discussion:propose`）。
  scope を 2 つ新設する理由は、既存の scope では「人が共有した議論を読み、案を記録するが、CoS に指示しない・全読みしない」を表せないこと（ADR D1.1）。
- snapshot と検索が返すのは、人（`Principal::Human`）が共有した message（revision と sha256 に固定）と、呼んだ client 自身の propose だけ。既定は空。
  撤回・共有の後の編集・引用の失効・confidential の文書は出さない。knowledge は返さない。秘密は共有の時に 422、出す時にマスク（ADR D2）。
- 共有の記録は `chat_events` の追記（`exchange_shared`・`exchange_share_revoked`）。有効かどうかは読むたびに導き、保存しない。
- propose は案件 thread の `role='user'`・`author='mcp:<client_id>'`・`discussion_mode: discuss` 固定の message。CoS の run は既存の仕組みで起き、
  子 3 D2 ですべての operation が拒否される。idempotency は `client_message_id = mcp:<client_id>:<key>` と `content_hash`（ADR D3）。
- MCP 非対応の経路は REST の 4 route（共有・取り消し・書き出し・取り込み、すべて `Principal::Human` だけ）と `celerisctl` の subcommand。
  取り込んだ message も discuss 固定で、execute の operation の根拠にならない（ADR D4）。
- envelope の R10・R13 は、自己申告の `approval` の欄を拒否し、呼び出し側が渡す決定の記録（`ApprovalRecord`、`human:` の答え）と共有の記録から導く。
  R3 に主体の一致、R8 に呼んだ主体からの consent を足す（ADR D5）。
- 決定 `ext-client-scope` の答え b を反映: `decision_answer`・`task_plan_gate` は `tasks:decide` へ、`db:read`・`api:read` は `chatgpt-rdc-2` に残る（ADR D6）。
- 新しい DB 表・列・migration・Manager・trait・状態機械・Agent サービスは足さない。

## 本番の MCP client の scope（読み取りで確かめた）

2026-11-22 に、run の読み取り専用 DB（`CELERIS_RUN_DB`）に `celerisctl mcp client ls` を当てた（token は出ない。書き込みはしていない）:

- `chatgpt-rdc-2`（有効）: `knowledge:read,knowledge:propose,tasks:read,tasks:interact,console:instruct,db:read,api:read`
- `claude-code`（有効）: `knowledge:read,knowledge:propose,tasks:read,console:instruct,org:read,skills:read,skills:write`
- `chatgpt`・`chatgpt-rdc`: 失効済み

## 証拠（提出時の HEAD で実行）

- `test -f <ADR> && grep -q 'discussion_snapshot\|snapshot' … && grep -q 'R10' … && grep -q 'celeris-chat' … && grep -q 'client_message_id' … && grep -q 'mcp_discussion_' …` → exit 0
- `sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/check-doc-links.sh` → `check-adr-numbers: ok (478 files)`・`check-doc-links: ok`、exit 0
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path から `agent-docs/adr/`・`agent-docs/progress/` を除く）→ 範囲外 0 件

## 未解決事項

- 本物の RDC home-dev・ChatGPT（RDC と custom remote MCP）・Muse での確認は未確認（昇格の後に人が行う）。
- `task_answer`・`task_decompose`・`task_comment` は決定 b の範囲の外で `tasks:interact` に残る。
- 子 2 の advisory の 2（`StrategyRef` と `ArtifactPointer` の重複）・3（D5.3 の fail-closed）はこの子の範囲の外。close が提案に書く。

## 提案

- 取り込んだ message（`metadata_json.exchange`）を Web の chat で外部の文として示す表示は、子 7 で決める。
