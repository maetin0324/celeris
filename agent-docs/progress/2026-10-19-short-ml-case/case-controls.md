---
task: 01M59KY5FTRJTJ80PACNW3K533
wu: case-controls
status: done
completed: 2026-10-19
---
# case-controls: 短い ML case は延期（deferred）— 記録は case 探索外

## 結果

- 延期。状態は `crates/campaign/optional/short-ml-campaign/status.json` にある（`cases/` の外）。
- `cases/dev/short-ml-campaign/` は作らない。
- 採用・無 LLM 正負例の成立・実 pilot は主張しない（status.json の `claims` は全て false）。
- loader（`crates/campaign/src/cases.rs`）・grader・既存 case・manifest は base c9654bff から不変。

## 証拠コマンドと結果

- `python3 crates/campaign/scripts/verify-short-ml-case.py --check` → exit 0。延期記録の整合と、`cases/dev`・`cases/final` に延期専用 dir が無いことの確認であり、case の成立や採用を意味しない。
- `git diff c9654bff -- crates/campaign/src crates/campaign/cases crates/campaign/fixtures` → 差分ゼロ。

## 主実験への影響

- なし。K2/K4 は置き換えない。
