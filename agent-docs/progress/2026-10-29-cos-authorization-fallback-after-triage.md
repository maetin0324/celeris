---
title: 認可の件の Discord 通知を CoS の一次対応の後だけ送る
tasks: [01M62FCC7WC7EFEZRN56GFG5NA]
status: done
updated: 2026-10-29
---
# 認可の件の Discord 通知を CoS の一次対応の後だけ送る

完了日: 2026-10-29

ADR: [2026-10-29-cos-authorization-fallback-after-triage](../adr/2026-10-29-cos-authorization-fallback-after-triage.md)

## 変更
- `phase 1: 認可の件の Discord 退避を CoS の一次対応の後だけにする` — `fallback.rs` の `unavailable_reason` が
  `FallbackDeadlines` を受け、認可の件は run が queued・running・stopping の間は 120 秒で退避しない。安全網
  `[cos.triage] authorization_unavailable_after_secs`（未設定なら `max_wall_secs + 300` = 1200 秒）を、run が無いときは
  到着から、run があるときは run の開始から数える。config・example.toml・skill・ADR（cos-chat-home D6 付記、
  cos-inbox-triage-fallback 決定 2 付記）・docs/ops/cos-chat.md を合わせた。
- `phase 1: 進捗を書く` — 本ファイル。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo test -p task-dispatch cos_chat && cargo test -p celeris notify` → exit 0（task-dispatch 134 passed、celeris notify 12 passed ほか）
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 2803 passed、9 skipped。source-size gate 0 failure、文書検査 ok）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 停止・中断した run の件は ADR 2026-10-07-cos-inbox-thread-conversation D1 のとおり CoS にもう一度渡る（従来どおり）。
  人が run を止め続けると、run が動くたびに安全網の起点が run の開始に戻る。実害は小さいと判断し上限を足していない。
- `approval` は `cos_inbox_items.source_kind` に現れない（カードの種類だけ）。対象は `authorization` だけにした。

## 本番への移行
- config の書き換えは要らない（未設定で 1200 秒）。

## 提案
- 認可以外（question・decision 等）も同じ扱いにするかは人が決める。今回は依頼の範囲どおり変えていない。
