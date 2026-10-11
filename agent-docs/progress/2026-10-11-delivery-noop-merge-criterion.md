---
title: "commit の無い root で取り込み判定を出さず、審査範囲を base..head と明示する"
tasks: [01M4P84BYNVR2FT0Y7VTN8P8PF]
status: running
updated: 2026-10-11
---

# 取り込む commit が無い root の delivery 見送り（2026-10-11）

決定: [ADR-0121 付記: 取り込む commit が無い root（2026-10-11）](../adr/0121-root-delivery-without-assignee.md)。

## 起きたこと

日次整理 task `01M4P733B9588PC6QZ9C2VH0NT` は commit 0 件（head = base = `ca7025be`）なのに【部署内のマージ可否判定】criterion が足され、
reviewer が「指定SHAの実際の差分」を `4d97d291..ca7025be`（別 task の成果）と読んで 2 回不合格にした。

## 変更（WU noop-delivery）

- `task-core`: `DeliverySkipReason::NoCommits`（code `no_commits`、label「取り込む commit がありません」）。
- `task-ops/src/delivery.rs` `prepare`: base・head 解決後に `git merge-base --is-ancestor <head> <base>` が真なら `no_commits` で見送り（criterion を足さない、`DeliverySkipped` 1 件）。
- 同 criterion 文: 審査対象を `git log {base}..{head}` と `git diff {base}...{head}` に限定し、head 単体の親との差分・base 側の commit は対象外と明記。旧語「指定SHAの実際の差分」は削除。
- `task-ops/src/inbox.rs` `build_attention`: `no_commits` は注意事項にしない。
- schema 再生成: `UPDATE_SCHEMA=1 cargo test -p task-core --lib schema` / `-p task-api --lib schema`、`corepack pnpm@12.6.0 -C web gen:types`。

## 証拠

- `cargo test -p task-ops --lib -- no_commits merge_criterion delivery_skip existing_reviewer` → 13 passed, 0 failed。
- `cargo test -p task-ops --lib -- delivery inbox` → 75 passed, 0 failed。
- `grep -c no_commits docs/api/v1/api-v1.schema.json docs/api/v1/event.schema.json web/api/generated/types.ts web/api/generated/schema.json` → 各 1。
- `bash scripts/dev/test-parallel.sh` → 5212 run: 5102 passed, 110 failed, 13 skipped。失敗は全て browser/launcher/credentiald（celeris-credentiald・task-worker・task-api・celerisctl）で、sandbox の長い `$TMPDIR`（93 文字）で Unix socket path が `SUN_LEN` を超える（`path must be shorter than SUN_LEN`、fixture readiness timeout）。delivery・inbox の試験に失敗なし。
- 同 4 package を短い TMPDIR で取り直し: `TMPDIR=$(mktemp -d /tmp/np.XXXX) cargo nextest run -p celeris-credentiald -p task-worker -p task-api -p celerisctl --no-fail-fast` → exit 0、2077 passed, 11 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-doc-links.sh`・`check-adr-numbers.sh` → 全て exit 0。

## 本番への移行

不要。DB の変更なし（events の reason 値が 1 つ増えるだけで migration なし）。config の変更なし。

## 未解決事項

- 全体試験は sandbox の TMPDIR 長で browser 系が落ちる（本変更と無関係）。verify WU では短い TMPDIR で取るか同じ切り分けを記す。

## 提案

- なし。
