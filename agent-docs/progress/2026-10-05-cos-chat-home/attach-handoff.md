---
title: 添付の引き継ぎ（task run 入力・KB candidate provenance・CoS の pin 経路）
tasks: [01M4AD3K4WCAJR4WE2FEB6QKPV]
status: done
updated: 2026-10-07
---

# attach-handoff — ADR cos-chat-home D4 の未実装部分

完了日: 2026-10-07。cos-run 付記の未解決「添付→task 入力 manifest と KB provenance の連結」を実装し、`agent-docs/adr/2026-10-05-cos-chat-home.md` に「付記: 実装との突き合わせ（attach-handoff、2026-10-07）」を足した（各 D4 文の実装箇所・試験・差分はそちら）。

## 完了内容（WorkUnit ごと）

- task-input: `task-dispatch/src/dispatcher/input_attachments.rs`。task に pin された添付を、作業する run（atomic task と WU）の開始時に hash・size を照合して作業ツリー外へ 0400 で stage し、`RunContext.input_attachments`→前置きの入力 manifest に載せる。照合失敗・ssh remote は `delivery=unavailable`。試験 `cos_chat_attach_handoff_task_run_*` 4 本ほか。
- cos-pin: 既存の references API を監査付き操作 `attachment.reference` として `/cos/operations` に許可し、前置きと `cos-operator` skill に「owner 作成→pin→応答確認→引渡し済み」の順を書いた。
- kb-provenance: `GET /knowledge/inbox`・`/knowledge/inbox/{id}` の候補に `provenance[]`（sha256・thread_id・message_id・request_text ほか）。migration なし。pin の owner 不在は 404。
- close（本 WU）: ADR 付記と本ファイル、全体検査。コードは変えていない。

## 証拠

| コマンド | 結果 |
|---|---|
| `bash scripts/dev/test-parallel.sh`（HEAD `756f3371`） | exit 0。nextest 147 binaries + doc 10、4597 passed / 0 failed / 14 ignored。userns 由来の失敗なし |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `check-adr-numbers.sh` / `check-doc-links.sh` / `progress-index.sh --check` / `check-doc-layout.sh scripts/dev/docs-layout.tsv` / `check-architecture-map.py` | 全て exit 0 |

## 未解決事項

- 子孫 task には pin を継がない（子へは CoS が個別に pin）。planner・reviewer の run には stage しない。
- ssh remote 実行の run には stage できず `unavailable`。
- stage 先（task_dir の attachments）の完了後削除は未実装。
- accept 後の KB 正本ページへの出典の書き写し、web/gui の候補画面での provenance 表示は未実装（API・生成型まで）。
- 「引渡し済み」の表示は CoS が pin 応答を確かめて言う規則で、機構での強制は API の 404/409/422 のみ。

## 提案

- 子 task へ pin を継ぐ要否を人が決める（継ぐなら objective に書かれた範囲だけ）。
- KB accept 時に provenance を正本ページの front matter へ写す葉。web の候補詳細に provenance 欄を出す葉。
