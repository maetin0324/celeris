---
title: CoS の自由質問への本文回答
updated: 2026-10-27
status: done
tasks: [01M5XXF7QECJ78NWVAHDH7A2BF]
---
# CoS の自由質問への本文回答

完了日: 2026-10-27

## 変更
- commit 件名: `phase 1: CoS の自由質問を本文で回答できるようにする`
- worker の自由質問で option 省略と note 本文を受け付ける。resolve・relay・人の受信箱で検証を共用。
- 選択式の問いは提示 key の検証を維持。空本文・未知 key を拒否。
- ADR、API schema・web 生成型、CoS 配布 skill を更新。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo test -p task-api --test cos_triage` → exit 0、15 passed。追加試験は resolve と relay で本文回答が applied、task が Ready、再送で Answered が 1 件のままであることを確認。空本文・未知 key、および選択式 decision の option 省略も拒否。
- `UPDATE_SCHEMA=1 cargo test -p task-api schema --lib` → exit 0、3 passed。`node web/scripts/gen-types.mjs` で生成型更新。
- `bash scripts/dev/test-affected.sh` → exit 0、Rust 4640 passed / 20 skipped、web 型検査成功・Vitest 682 passed・Node 98 passed、対象 doc test 成功（1 ignored）。既存 campaign 試験の未使用 import 警告あり。
- `cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check` → exit 0。fmt 差分なし。
- `git diff --check` → exit 0。
- 全体試験・web e2e・本物 browser/userns の試験は release gate のみの規則に従い実行していない。

## main への rebase（2026-10-27、operator の再開コメントに対応）
- main へ rebase。衝突は `config/skills/cos-inbox-triage/SKILL.md` の 1 か所だけ。main 側の `policy_version` "3" を採り、自由質問の回答規則の行を残した。
- rebase 後の HEAD で上の検査をすべて再実行した。`cos_triage` は 15 passed、test-affected は exit 0、clippy と fmt も exit 0。

## 未解決
なし。

## 提案
なし。

## 本番への移行
DB migration・設定変更なし。通常の release で API と配布 skill を更新する。
