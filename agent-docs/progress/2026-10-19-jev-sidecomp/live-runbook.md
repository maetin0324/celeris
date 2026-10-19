---
title: "C1 実 daemon 検査の人向け手順書と区分の記録"
tasks: [01M59KY5FPEWVNW0DT07HZRA3Q]
unit: live-runbook
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# live-runbook

`docs/ops/jev-c1-live-check.md` を追加。Jev enabled・認証・モデル版の確認（管理 API/CLI のみ）、jev-stages の取り方、残予算内のみ実行・副実験から縮小して通知待ち、キー値を出さない、主比較の非依存、区分表（実装済み/無 LLM 検証済み/実 pilot 未確認/任意・延期/承認待ち）を記載。本番 host の操作・実 LLM 実行はしていない。未測定の効果・有意性は書いていない。

## 証拠
- `sh scripts/dev/check-doc-links.sh docs/ops/jev-c1-live-check.md` → ok（exit 0）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok

## 未解決・提案
- 記録から jev-stages の入力を自動で組む道具が無い（手作業）。Jev の版が記録に出ない（inventory 不足 1）。
