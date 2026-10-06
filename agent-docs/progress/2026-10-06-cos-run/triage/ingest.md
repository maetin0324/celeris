# CoS triage ingest（dispatcher の一次対応取り込み）

---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
status: done
completed: 2026-10-06
---

## 完了

- [ADR 2026-10-06 cos-inbox-triage-ingest](../../../adr/2026-10-06-cos-inbox-triage-ingest.md) に source の同一性・cursor・除外・起動の決定を書いた。
- `crates/task-dispatch/src/dispatcher/cos_chat/triage.rs` を追加した。派生 inbox（events cursor 以後の非進捗 event が触れた task の項目だけ）と notice（`(last_at,id)` cursor 以後）を `cos_triage_ingest_batch` で投入する。起動時と reconcile 時は `cos_triage_reconcile` で差分を照合し、cursor が無いときの導入は 1 回だけ。CoS operation の transaction で書かれた待ちと `secretary_reply` は取り込まない。元 thread に参照カードを置く。
- `launch.rs` の `start_thread` を `ClaimSource::{Queue,Triage}` で共用にし、受信箱 thread の run を既存の chat-run 経路（account・容量・credential・session）で起こす。tick（`tick_cos_chat_launch`）に取り込みと起動を登録した。
- `[cos.triage]` を `CosChatLaunchConfig.triage`（`CosTriageSettings`）として daemon（`crates/celeris/src/daemon/bootstrap.rs`）から丸ごと渡す。

## 証拠

- `cargo test -p task-dispatch --lib cos_chat_triage_ingest` → 7 passed（1 待ち → 1 行・1 system message・1 run／再配送・題名・既読で増えない／21 件 → 20+1／CoS operation 由来と secretary_reply を除外／導入時 1 回・無効時は run なし／帰属規則の単体／元 thread の参照カード）。
- `bash scripts/dev/test-parallel.sh` → exit 0、nextest 4145 passed・0 failed・13 ignored。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy --workspace --all-targets -- -D warnings` → exit 0。

## 未解決

- store の `cos_triage_claim` が作る chat_run は `resolved_config_json` と output message を持たず、system message の `message` chat_event も出さない（SSE は次の run event まで更新されない）。store 側で直すのが自然（提案）。
- 起動に失敗した run（unavailable）の item は `running` のまま残る。不在退避（fallback 葉）が拾う前提。
- `knowledge_review`（KB の取り込み待ち）は dispatcher が KB の件数を持たないので取り込まない（`KnowledgePending=None`）。
- CoS operation 帰属は batch の境界で transaction が分かれると外れうる（その項目は取り込まれる。循環しても item は同じ triple なので 1 回だけ）。

## 提案

- store の `cos_triage_claim` に `resolved_config` を渡し、output message と `message` event を同じ transaction で作る。
