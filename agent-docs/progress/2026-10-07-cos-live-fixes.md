---
title: CoS 実機 live2 の不具合 D1〜D4・live 台本・gui-api 節番号
tasks: [01M4APB5FP8T3TAAE51Z20E3M1]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# CoS 実機 live2 の不具合 D1〜D4（ADR 2026-10-07-cos-live-fixes）

## 完了したこと

- D1 作成時の `attachment_ids` と同一 transaction の pin（`POST /tasks`・CoS `task.create`）→ [d1-create-pin](2026-10-07-cos-live-fixes/d1-create-pin.md)
- D2 `POST /knowledge/inbox`・`ALLOWED` の `knowledge.record`・CLI は CoS credential で API → [d2-kb-inbox](2026-10-07-cos-live-fixes/d2-kb-inbox.md)
- D3 `ResolveBody.confidence` → [d3-confidence](2026-10-07-cos-live-fixes/d3-confidence.md)
- D4 終端済み run を takeover から外す（原因は triage run の本文 Conflict を終端と誤読した sink）→ [d4-takeover](2026-10-07-cos-live-fixes/d4-takeover.md)
- 台本 `scripts/dev/cos-chat-live.sh` と `docs/ops/cos-chat.md` → [live-script](2026-10-07-cos-live-fixes/live-script.md)
- `docs/api/v1/gui-api.md` の節番号の重複（3.127〜3.128）を振り直し → [adr](2026-10-07-cos-live-fixes/adr.md)
- ADR を「実装済み」にし、実装との突き合わせ付記を書いた。`docs/architecture-map.md` に route・台本の行を足した。
  親の `2026-10-05-cos-chat-home/live-check.md` の D1〜D4 に「直した」を追記した。

## 証拠（統合後の HEAD 39c95b40）

| コマンド | 結果 |
|---|---|
| `bash scripts/dev/test-parallel.sh` | exit 0。nextest 4660 passed・0 failed・14 ignored（binaries 160）、doctest exit 0。userns 由来の失敗なし |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo test -p task-api --test cos_live_fix_d1` | 3 passed |
| `cargo test -p task-api --test cos_live_fix_d2` | 3 passed |
| `cargo test -p task-api --test cos_triage`（`cos_live_fix_d3_*`） | 11 passed（d3 は 3 本） |
| `cargo test -p task-dispatch --lib cos_live_fix_d4` | 5 passed |
| `cargo test -p celerisctl cos_live_fix`（d2 の CLI） | 3 passed |
| `sh scripts/dev/check-adr-numbers.sh` / `check-doc-links.sh` / `python3 scripts/dev/check-architecture-map.py` | いずれも exit 0 |
| `cargo fmt --check` | **exit 1**（下の未解決事項 1） |

## 未解決事項

1. `cargo fmt --check` が `crates/task-api/tests/cos_triage.rs:773` の整形差（d3 の試験）で落ちる。この task（close）はコードを直さない指示なので
   直していない。`cargo fmt -p task-api` の 1 回で直る。
2. 実機の `scripts/dev/cos-chat-live.sh` の dry / full は未実行（daemon・LLM を起こさない run）。認証の使える host で
   人か運用セッションが流し、`evidence/verdict.txt` を live-check.md に転記する。
3. `crates/task-worker/src/cos_chat.rs` の `attachment_pin_rules`（CoS run の前置き）が旧手順（owner を先に作って references で pin）のまま。
   skill §3a と食い違い、live の (d) が再び「起票してから pin」で走りうる。試験 `cos_chat_attach_handoff_cos_preamble_explains_pin` の文言も合わせる要。
4. 人の API 経路の pin に専用の event が無い（`chat_attachment_refs.created_at` が記録）。
5. `run_claimed` の早期 return（DB 読み取りエラー）は終端を書かず、次の tick で takeover に回る。
6. 親 live-check の不具合 4（`summary_through_seq` が 0）・5（`model: null`）は未解決のまま。

## 提案

- 上の 1 と 3 を小さな follow-up task にする（fmt は 1 コマンド、3 は前置きと試験の文言）。
- triage run にも出力 message を持たせるか、sink が `output_message_id` を先に読んで本文を送らないようにして、Conflict の往復をなくす。
- pin の event が要るなら `Event::AttachmentPinned` を足す別 task（schema・web event-kinds 込み）。
