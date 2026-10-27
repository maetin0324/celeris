---
title: 外部 operator の手順と CoS との二重対応防止
updated: 2026-10-27
status: done
tasks: [01M5XXF7QQ6KB8MVV0RE96ZKJ8]
---
# 外部 operator の手順と CoS との二重対応防止

完了日: 2026-10-27

## 変更

- `phase 1: 外部 operator と CoS の分担を定め二重対応を防ぐ` — ADR、手順書、selfdeploy の CoS promote 説明と push_error 復旧手順、文書入口。
- 文書のみ。新 principal・取り次ぎ・通知連携の実装、本番変更は含めない。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- 条件 0: ADR・手順書に責務/起動/権限/model と方針版/分担/二重対応防止/依存表を記載。既存 CoS production と操作 registry、外部効果 ADR、delivery の push 実装に照合。
- `bash scripts/dev/test-affected.sh` → exit 0。文書のみで Rust/web の試験対象なし。health 検査合格（Python 38+1 件、進捗 hash 検査 5 件、リンク self-test）。source-size は failure 0、既存 debt warning 3、reasoned exception 1。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 条件 1: `sh scripts/dev/progress-index.sh --check` → exit 0、`progress-index --check: ok`。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0。
- `git diff --check` → exit 0。全体試験/release gate・本番操作は実施せず。

## 未解決

A1/B3/C3 の新経路と D1/D2 の詳細契約は本 checkout で確認できず、依存表で未実装扱いと明記。
本番 push_error の残存・remote 包含は本 run では確認/修復していない。

## 本番への移行

人が外部 session の旧巡回を停止し、起動設定から除去する。台本は追跡ファイルに存在せず本 run では変更しない。
新経路は対応 ADR・配備版・認証と監査の検証後に依存表を更新して利用する。

## 提案

なし。依存 task の実装状況は手順書の表で引き継ぐ。
