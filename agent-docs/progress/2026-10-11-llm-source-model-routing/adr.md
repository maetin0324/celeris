---
task: 01M4M8D4AQHBC2KG3H1BPJ7Z0J
unit: adr
status: done
completed: 2026-10-11
---
# adr: source・model 選択の評価方法・選択方式・段階・legacy・実装の分け方

## 成果
- ADR `agent-docs/adr/2026-10-11-llm-source-model-routing-evaluation.md`（D1 評価方法 / D2 選択方式 / D3 段階と止める条件 / D4 legacy の扱い / D5 実装の分け方 / D6 genre ごとの適用）
- task の進捗 `agent-docs/progress/2026-10-11-llm-source-model-routing.md`（現在地・設計の要約・未解決・提案・本番への移行）

## 証拠コマンド
- `sh scripts/dev/progress-index.sh --check`: exit 0
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/ gui/`: exit 0（コードは変えていない）
