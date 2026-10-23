# 進捗ファイルの書き方とテンプレート

置き場所・front matter・持ち主は [ADR-0128 D3](../adr/0128-docs-layout.md)。commit の参照方法は
[ADR 2026-10-23-commit-refs-by-subject](../adr/2026-10-23-commit-refs-by-subject.md)。

## 規則
- task ごとに `agent-docs/progress/YYYY-MM-DD-<slug>.md`、並列 WorkUnit は `agent-docs/progress/YYYY-MM-DD-<slug>/<wu-key>.md`。
- commit は **件名で列挙し、hash を書かない**。celeris は review 前の main 同期で task branch を rebase するので、
  task branch の commit の hash は提出後に必ず変わる。
- 基準は「`git merge-base HEAD main`」（WorkUnit は `$CELERIS_WU_BASE`）と式で書く。固定の hash を書かない。
- 検査結果は「提出時の HEAD で実行」と書き、最後の同期（main の取り込み・rebase・merge）の後に再実行した結果だけを載せる。
  「検証した SHA」「tested_sha」の欄は置かない（検査した SHA は celeris が記録する）。
- hash を書いてよいのは main に入った commit（他 task の修正など。rebase で変わらない）だけ。
  `sh scripts/dev/progress-index.sh --check` は、この branch が変えた進捗ファイルの 7〜40 桁の hex が main の祖先の commit で
  なければ失敗する。digest を書くなら全桁で書く。

## テンプレート

```markdown
---
title: <題>
tasks: [<task id>]
status: running
updated: YYYY-MM-DD
---
# <題>

完了日: YYYY-MM-DD

## 変更
- `phase 1: <要約>` — <何をしたか>
- `phase 1: <要約>` — <何をしたか>

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0（N passed）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決

## 提案
```
