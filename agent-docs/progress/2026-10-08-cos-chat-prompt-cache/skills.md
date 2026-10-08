---
title: CoS chat 最適化 6 — cos-operator / cos-inbox-triage の再構成（T5）
tasks: [01M4DE37B9EE6308ZDAVXXBRTX]
status: done
updated: 2026-10-08
---

# CoS chat 最適化 6: skill 再構成（T5）

設計は [ADR 2026-10-08-cos-chat-prompt-cache](../../adr/2026-10-08-cos-chat-prompt-cache.md) の付記 D7。branch は main `8ce73e81`（Core 分離込み）を fast-forward で取り込んでから作業した。

## 実装

- `config/skills/cos-operator/`: SKILL.md 20,158 B → 4,650 B の入口（version 3。場面ごとに読む file の表、description から「常に読む」を除去）。詳細は `operations.md`（登録表・本文の形）・`attachments.md`・`production.md`・`explaining.md` に分けた。
- Core（`crates/task-worker/src/cos_chat.rs`）: idempotency・409・登録済み path・回避禁止・直接触らない物・秘密・信頼しない入力を追加し、添付の手順を「作成の body に `attachment_ids`」の 1 回の operation に直した。6,911 B（上限 7,000 B）。
- `operations.md` の登録表は `ALLOWED` の 12 行と一致。未登録の PUT execution-plan・pause/resume・standing-rules は「登録されていない操作」に移した。
- `task-dispatch` `cos_chat_skills`: `cos-inbox-triage` は受信箱 thread の run か inbox_items がある run だけ mount。
- `cos-inbox-triage/SKILL.md` は §5/§10 の参照先と description だけ直した（policy version 2 のまま）。
- SOURCE.md 2 件、`docs/ops/cos-chat.md`（`celerisctl skills import` での配置と mount の条件）、`docs/architecture-map.md`、`scripts/dev/cos-chat-live.sh` の §3a 参照を更新。

## 試験（新規・変更）

- task-api: `cos_operator_skill_table_matches_allowed`、`cos_chat_ops_api_rejects_unregistered_operations_regardless_of_skill`
- task-dispatch: `cos_chat_skills_mount_triage_only_in_the_inbox_thread_or_with_inbox_items`、`cos_chat_skill_ordinary_thread_run_does_not_mount_inbox_triage`、`cos_chat_skill_inbox_thread_runs_mount_inbox_triage`、`cos_chat_run_launch_fake_progress_credential_and_session`（期待を 1 skill に変更）
- task-worker: `cos_chat_core_carries_the_minimal_cos_operator_rules`、Core 上限 7,000 B

認可・監査が skill 読込に依存しない根拠: 上の 2 件と既存の `cos_chat_ops_api_rejects_paths_outside_the_allowlist_with_reasoned_events`・`cos_chat_ops_api_idempotency_same_hash_same_operation_different_hash_409`・`cos_chat_ops_auth_get_allowed_and_direct_mutation_is_422_with_audit`・`cos_chat_ops_auth_expired_revoked_and_unknown_are_401`・`cos_chat_ops_checkpoint_api_403_for_human_and_other_run`（API に skill の概念は無い）。

## 計測（before = T2 baseline、after = 本 branch。隔離 daemon・claude_oauth・同じ台本の full 26 run）

| 指標 | before | after |
|---|---|---|
| S1 新規 thread の skill 読込 | 10/10 run が 1 回 | 0/10 |
| S2 の skill 読込 | turn 1 だけ | 起票 turn だけ |
| S1 非 cache input / cache write（中央値） | 8 / 21,864 | 12 / 17,566 |
| S2 非 cache input / cache write | 6 / 6,617 | 6 / 4,602 |
| S1 / S2 総 latency ms | 15,802 / 19,672 | 18,895 / 13,782 |
| 管理操作（起票 1・コメント 4） | 5/5 completed | 5/5、`cos_operations` 全て applied |

S1 の latency・cache read は増えた（Core 分離と本変更が両方入った差で、寄与は分けていない。before の tool 回数は残っておらず不明）。skill 読込と cache write は減り、管理操作の成功率は落ちていないので統合する。生データ: task artifacts の `bench-after/`、`prompt-bench-after.json`。

## 証拠

- `test "$(wc -c < config/skills/cos-operator/SKILL.md)" -le 7000` → exit 0（4,650 B）
- `cargo test -p task-dispatch -p task-ops -- cos_chat skill` → exit 0（task-dispatch lib 105 passed ほか）。受け入れ条件の `cargo test … cos_chat skill`（`--` 無し）は cargo 1.98.1 が 2 つ目の TESTNAME を `unexpected argument` で拒むため、変更に関係なく exit 1
- `cargo test -p task-api -- cos_operator_skill_table cos_chat_ops_api_rejects` → 1 + 2 passed
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 100（passed 4,779 / failed 64。失敗は全て browser・launcher・credentiald・CDP・scratch の sandbox 既知失敗。cos_chat・skill は 0 件）
- `bash scripts/dev/cos-chat-bench.sh … full claude-code` → exit 0、26 run completed、pgrep 残存なし
- `celerisctl skills import config/skills --name cos-operator --name cos-inbox-triage --root <一時 KB>` → exit 0（cos-operator は 5 付属 file）

## 未解決事項

- 本番 KB への取り込み（`celerisctl skills import`）は人が行う。本番 config/DB/KB/release/systemd には触れていない。
- S1 の latency 増の原因（Core 分離か skill 再構成か）は不明。T8 で切り分ける。
- 受け入れ条件 1 のコマンド形（`--` が要る）。

## 提案

- 受け入れ条件 1 を `cargo test -p task-dispatch -p task-ops -- cos_chat skill …` に直す。
- T8 で S1 の tool 回数を before と同じ方法で残す（bench の runs.json に tool event 数を足す）。
