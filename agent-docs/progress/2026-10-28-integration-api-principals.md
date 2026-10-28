---
title: 統合 API の認証主体を状態・event・介入集計へ記録する
tasks: [01M60JC1P2CYMFPZR9KY8E6494]
status: done
updated: 2026-10-28
---
# 統合 API の認証主体を記録する

完了日: 2026-10-28

## 変更

- `phase 1: 統合 API を共通認証に通し操作主体を記録する` — 統合 router が guard の外にあった原因を除去。
  受信箱 answer の内部転送も共通 router に統一。Caller の extension 欠落を拒否する。
- merge / hold / discard / reopen の状態・event と、統合依頼の回答は token principal を保持する。
  CoS の integrate-task は operations の thread principal を保持する。MCP の変更・CoS の直接変更は従来どおり拒否する。
- 統合処分 4 種と IntegrateTaskRequested を decision log の recovery に投影。
  human-ops は actor_kind の既存写像を使い、operator の統合介入が人の件数に入らない回帰試験を追加。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- 条件0: `cargo test -p task-api --test integration_principals --test cos_integrate_task` → exit 0（当時6件成功）。
  最終の test-affected では追加した direct integrate-task の web/operator 試験も成功。
  merge / hold / discard / reopen を各 token と受信箱経由で実行し、応答・DB・event の主体を検査。
  統合依頼回答の web/operator と、既存の CoS integrate-task の thread 記録も確認。
  MCP mutation は403、CoS直接 mutation は422、未知 token は401、Origin付き変更は403で状態不変。
- 条件0・1: `bash scripts/dev/test-affected.sh` → exit 0。Rust 6654 passed、21 skipped、対象 doc test 3 passed・1 ignored。
  web `tsc -b` 成功、unit 685 passed、gateway 105 passed。decision log の5種の統合操作と主体分類、
  human-ops の operator 介入を人の件数に入れない写像も成功。
- 条件2: `cargo clippy --workspace -- -D warnings` → exit 0。
- 条件2: `cargo fmt --all -- --check` → exit 0（差分ゼロ）。
- `git diff --check` → exit 0。test-affected 内の文書・進捗・ソース健全性検査も成功。

web 依存の offline install はキャッシュ不足で失敗したため、`pnpm install --frozen-lockfile` で補完（exit 0）。
Rust の試験用ビルドでは既存の `dagbench_pbs_e2e.rs` に未使用 import 警告が1件出たが、指定検査はすべて成功。

## 未解決

なし。

## 提案

なし。

## 本番への移行

config・DB migration は不要。過去の誤った actor を含む events は追記専用のため書き換えない。
