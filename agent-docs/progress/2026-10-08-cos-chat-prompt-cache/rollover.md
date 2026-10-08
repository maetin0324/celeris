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
- migration `0062_cos_chat_context_tokens`（schema 62）: `node_sessions.last_context_tokens`（置換）・`billed_input_tokens`（累積・判定に不使用）。`approx_tokens` は従来の fallback。
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

## billed-input-fix（2026-10-08）

- `session_usage` に実行した adapter ID を渡し、claude-code は `input+cache_read+cache_creation`（外数）、codex は `input`（cached は内数）を累積する。worker 一般の `Usage` と既存パーサ結果は維持した。
- acp/opencode は安定版 `PromptResponse` で usage を報告せず、内数・外数は不明。pi は `message_end.usage.{input,output,cacheRead,cacheWrite}` を個別に合算するが包含関係は不明。両者とその他の定義不明 harness は input のみとする（現行 ACP は usage=None のため加算 0）。ADR 付記 D4 に根拠を記した。
- `context_tokens`（最後の観測の置換）と `approx_tokens` fallback（input+output の累積）の挙動は維持した。fake harness の結合試験は占有 600 に対して billed_input=10、2 run の占有 300 に対して累積入力 20 を確認する。
- rollover migration は `0062_cos_chat_context_tokens.sql` / schema 62。ADR と本進捗の旧記述を同期した。
- `cargo test -p task-dispatch -p task-core cos_chat`: exit 0、131 passed（task-core 27、task-dispatch lib 93、integration 11）。新規 `cos_chat_billed_input_*` 3 件も通過。codex は 1 run の保存値 10 / 2 run の累積 20、claude 形は 16 / 32 を確認した。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `bash scripts/dev/test-parallel.sh`: exit 100、4764 passed / 64 failed / 14 ignored（doctest exit 0）。失敗は未変更の browser・launcher・credentiald・CLI socket・CDP・scratch 系。ログに Unix socket の `path must be shorter than SUN_LEN` と fixture readiness timeout があり、worker sandbox / 長い TMPDIR の既知の環境制約に該当する。今回の CoS 会計試験は通過した。
- `cargo fmt --all -- --check`・`git diff --check`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`: exit 0。
- `sh scripts/dev/check-doc-links.sh`: exit 1。未変更の `crates/task-dispatch/src/dispatcher/cos_chat/sink_tests.rs:544` のテスト入力文字列 `docs/SKILL.md` を壊れた参照として拾う（修正前 HEAD にも同じ文字列あり）。今回編集した ADR / 進捗への指摘はない。
- Python で ADR と本進捗の両方に `0062_cos_chat_context_tokens` / `schema 62` があり、旧 `0061` / `schema 61` がないことを検査: exit 0。
- run の検査ログ: 成果物ディレクトリの `billed-input-cos-chat-test.log`・`billed-input-clippy.log`・`billed-input-test-parallel.log`・`billed-input-doc-links.log`。

## pi-accounting-fix（2026-10-08）

- 一次情報: pi-ai 0.84.2 は provider 差を正規化し usage.input は cache を含まない外数（anthropic-messages.js:398-401、openai-completions.js:1106-1125、openai-responses-shared.js:441-447、utils/estimate.js:3-4）。占有 input+cacheRead+cacheWrite（pi.rs:534-539）は正しく、誤りは累積入力側だった。
- 修正: `session_usage` で pi も claude-code と同じく `billed_input = input+cache_read+cache_creation`。codex は input のみ、acp は usage なしのまま。worker 一般の Usage は不変。
- 試験: `cos_chat_pi_cache_is_external_occupancy_and_billed_input`（2 呼出し input=100,cacheRead=40,cacheWrite=5: 占有 145、累積入力 290）。`cos_chat_billed_input_unknown_cache_definition_uses_input_only` から pi を外した。
- ADR 2026-10-05-cos-chat-home の付記 D1・D4 を「pi は外数・確定」で一致させ pi-ai の path:行を出典に記載。ADR 2026-10-08-cos-chat-prompt-cache に pi の包含関係の記述は無く変更不要。

## 再試行（check-doc-links の是正）
- 前回 check 不合格の原因: `sink_tests.rs:544` の試験入力 `docs/SKILL.md` が check-doc-links の「生きた参照」として壊れた docs パス扱いになった。入力を `notes/SKILL.md` に変更（試験の意味＝.claude/skills 外の SKILL.md は対象外、は不変）。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → 全て ok（exit 0）。
- `cargo test -p task-dispatch cos_chat` → 94 passed / 0 failed（cos_chat_pi_ 含む）。
