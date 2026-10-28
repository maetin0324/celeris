---
title: 受信箱の件を切らずにチャットへ出し、CoS 不在の件は後から CoS が要約する
tasks: [01M61HWT7Q8CPF0WTYX5R7NM71]
status: done
updated: 2026-10-28
---
# 受信箱の件を切らずにチャットへ出し、CoS 不在の件は後から CoS が要約する

完了日: 2026-10-28

設計は [ADR 2026-10-28-cos-inbox-chat-summary](../adr/2026-10-28-cos-inbox-chat-summary.md)（状態: 実装済み）。

## 変更（commit の件名）

- `cos-inbox-chat-summary: 受信箱の件を切らずにチャットへ出し、CoS 不在の件は後から要約させる`
- `cos-inbox-chat-summary: schema 版数の試験と進捗を合わせる`

## 何をしたか

- チャットに件が出る 4 経路（退避の行・一次対応の digest・統括スレッドの札・CoS の返事）を調べ、切れていた所を表にした（ADR §1）。
- 受信箱の `detail` は 600 字で切るが、切ったら `links` に「全文」（`/api/v1/tasks/<id>`）を足し、切る前の本文を `detail_full` に残す。
  退避の行・outbox・直送は全文を使う（チャットは 4,000 字を超えたら web へのリンク付きで切る）。
- migration 0101 `cos_inbox_items.detail`: 取り込んだ revision の本文を題名と一緒に保存する。知らせの束が育っても、
  退避の本文は同じ revision のもの（題と本文の食い違いの修正）。
- escalation packet の選択肢に任意の `detail` を足し、digest・札・prompt で「label: detail」を 1 行ずつ出す。
- CoS 不在で人へ回した件は、daemon が統括スレッドに queued の system message（`CoS 不在で人へ直接回した件の要約: items=[…]`）を積み、
  CoS が戻ると通常の queue claim で run になり、最終文が要約としてチャットに残る。`instructed_by` には使えない（role=system）。
- skill `cos-inbox-triage`（§6 packet の書き方・§11 新設）・`cos-operator`（説明の行）・prompt の受信箱節を新しい動きに合わせた。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）

- `bash scripts/dev/test-affected.sh` → 報告本文を参照。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決事項

- 要約の依頼の run が失敗した場合（起動後の quota 切れなど）、依頼は failed のまま残り、自動の再試行は無い（人が web で再送する）。
- 実際の CoS（LLM）が依頼から書く要約の質は実機で未確認。試験は依頼が run になるところまで（fake adapter）。
- Discord の通知本文は従来どおり 1,900 字で切れ、web へのリンクが付く。

## 本番への移行

- migration 0101 は起動時に自動で当たる（列の追加だけ。既存行は空文字で、退避は 0101 前の行を従来に近い読み方で扱う）。config の変更は無い。

## 提案

- 依頼の run が失敗したときの再試行（回数の上限付き）を、要約の依頼の行の状態から決定的に行う。
