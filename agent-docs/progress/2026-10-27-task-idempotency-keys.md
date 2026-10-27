---
title: 通常の task 作成と retry の冪等キー
tasks: [01M5XXF7QRGY65F3WTKSKVAZVK]
status: done
updated: 2026-10-27
---
# 通常の task 作成と retry の冪等キー

完了日: 2026-10-27

## 変更

- `phase 1: task 作成と retry の再送を共通の冪等キーでまとめる` — SQLite に要求 hash と task ID を保存。
  同じ要求は既存結果、異なる要求は 409。campaign 専用欄・label 検索を削除し、client と生成型を更新。
- 同時要求、失敗時 rollback、campaign 再送、DB 再オープンを回帰試験に追加。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- 条件0: `cargo test -p task-api --test task_idempotency --test campaign_ident --test campaign_variant_atomic`
  → exit 0、10 passed。再送・同時要求・本文変更409・旧欄拒否・失敗時rollback・campaign再送を確認。
- `UPDATE_SCHEMA=1 cargo test -p task-api schema --lib` → exit 0、3 passed。
  `node web/scripts/gen-types.mjs` → exit 0、生成型を更新。
- 条件1: `bash scripts/dev/test-affected.sh` → exit 0、Rust 6525 passed / 21 skipped、
  対象doc test 3 passed / 1 ignored、Web型検査合格、Vitest 682 passed、Node 98 passed。
  store のキー照合・retry・DB再オープン試験もこの検査に含む。
- 条件2: `cargo clippy --workspace -- -D warnings` → exit 0。
  `cargo fmt --all -- --check` → exit 0（差分ゼロ）。
- `git diff --check` → exit 0。
- 全体試験・Web e2e・実ブラウザ試験・release操作は実行していない。


## 本番への移行

通常の release 手順で migration を適用する。外部 client は campaign 専用欄を `idempotency_key` に変更する。
過去の task・識別・labels・events は保持する。旧要求の本文 hash は保存されていないため復元しない。
同じ campaign 試行の再送は既存識別で解決し、その要求から共通キーを保存する。
本番 host・DB の直接操作は行わない。

## 未解決

なし。

## 提案

なし。
