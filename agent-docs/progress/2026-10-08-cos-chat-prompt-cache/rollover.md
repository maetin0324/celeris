---
title: CoS chat 最適化 4 — rollover の token 会計を context 占有へ是正
tasks: [01M4D3XKE07NNNF0Z0F3SVZ9P1]
status: done
updated: 2026-10-08
completed: 2026-10-08
---
# rollover の token 会計（H4）

設計・決定は [ADR 2026-10-05-cos-chat-home 付記](../../adr/2026-10-05-cos-chat-home.md)「rollover の token 会計を context 占有へ是正」。

## 完了したこと
- claude-code: 最後の main thread assistant 行の usage から `Usage.context_tokens` を取る（result.usage は run 合算で使えない）。pi も取れる。codex/acp/aider は None（fallback）。
- migration 0061（schema 61）: `node_sessions.last_context_tokens`（置換）・`billed_input_tokens`（累積・判定に不使用）。`approx_tokens` は従来の fallback。
- 判定は `rollover_measure = last_context_tokens ?? approx_tokens`。`[sessions] rollover_tokens` の名前は保ち、意味を config 注釈と ADR に明記。
- API schema と gui/web 生成型を再生成。

## 証拠
- `cargo test -p task-dispatch -p task-core cos_chat`: exit 0（新規試験を含む）。
- `cargo test -p task-worker context_tokens`: 2 passed。
- `cargo clippy --workspace -- -D warnings`・`bash scripts/dev/test-parallel.sh`: 下の「最終検査」に結果を記す。

## 未解決・提案
- codex/acp/aider の context 占有は未確認（「不明」）。実機計測は未実施。
- WU・review の継続セッション（`sessions.rs`）は従来の `approx_tokens` 累計のまま。同じ誤りがあり得るので別 task で扱うのがよい。

## 最終検査
- 受け入れ 0（cos_chat 試験）exit 0、受け入れ 1（clippy -D warnings）exit 0。
- `bash scripts/dev/test-parallel.sh`: passed 4744 前後、failed は browser・launcher・credentiald・CDP・scratch reflink 試験のみ（worker sandbox の既知の環境依存。base でも落ちる）。この変更で落ちた schema 版数・migration・schema drift 試験（task-core 4・task-worker protocol 1）は直して通した。
- 副作用: `Usage` が 16 byte 増え clippy `large_enum_variant` が acp の `RawOutcome::TimedOut` に出たため `Box<Terminal>` にした。
