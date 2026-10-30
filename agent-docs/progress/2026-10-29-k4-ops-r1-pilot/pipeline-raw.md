---
title: ops-r1 A raw と attribution の model・harness（pipe-raw）
tasks: [01M655T6CGPRQ2HSQPYQFXYGZX]
status: done
updated: 2026-10-30
---
# ops-r1 A raw と attribution の model・harness（pipe-raw）

完了日: 2026-10-30

## 変更
- commit 件名: `phase 1: add ops-r1 A raw and attribution v2`
- `ops-r1-a-raw --transcript <JSONL> --metadata <JSON> --out <JSON>` と公開の型・build を追加。
- operator_usage の session/message ID による重複除去を再利用。指定 session の usage 4 欄を合計し、欠測は null、models はソートした集合。
- stop は exited・cutoff・token_cap・operator_stop。測定者の記録を受け取り、transcript から推測しない。
- attribution を ops-r1-attribution/2 に置換。runs.model・runs.adapter を model・harness に写し、欠測と chat_runs は null。
- manifest・human-op・daemon は変更していない。試験は stub transcript と一時 DB の写しだけを使い、LLM・本番 DB は使わない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 条件0: `cargo test -p celeris-campaign --test ops_r1_pipeline_raw` → exit 0、3 passed。重複 usage・4種類の stop・欠測・入力拒否・byte 一致を確認。
- 条件1: `bash scripts/dev/test-affected.sh` → exit 0、2,835 passed、9 skipped。対象5 crate（celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）の nextest と doc test が成功。attribution/2 の model・harness、欠測、DB 不変を含む。
- `cargo clippy --workspace -- -D warnings` → exit 0、警告なし。
- `cargo fmt --all -- --check` と `git diff --check` → exit 0、差分なし。
- `bash scripts/dev/check-health.sh` → exit 0。構造・文書・進捗の検査が成功。
- 条件2: stub transcript と試験 fixture の一時 DB のみ。DB の読み取りは既存の open_copy（READ_ONLY・immutable）を維持し、LLM・本番 DB にはアクセスしていない。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、変更は crates/campaign/・crates/celeris/tests/・docs/ops/・agent-docs/ のみ。

## 未解決事項
- 全段の組み立ては pipe-assemble、全段の結合試験は pipe-e2e の担当。

## 提案
- 無し。
