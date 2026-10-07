---
title: Pi + Hashline の導入手順書と config 例
tasks: [01M49X6CW5KE5HY0EZRQR0S045]
status: done
updated: 2026-10-07
---
# Pi + Hashline の導入手順書と config 例（WorkUnit ops-docs）

## 実装
- `docs/ops/coding-harness-pi-hashline.md`: Pi 導入（openclaw 同梱 / npm prefix）、Hashline 配置、config 追記、戻し方・明示指定、(model, harness) の見方、確認の流れ。
- `config/celeris.pi-hashline.example.toml`: claude-code 行・pi 行・`adapter_policy = "model_family"` の harness。
- `scripts/dev/docs-layout.tsv` に手順書を human で追記。crates/ は不変。

## 証拠
- `sh scripts/dev/check-doc-links.sh` → ok、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok。

## 未解決事項
- Hashline の配布元・tool 名は未確認。例の `hashline_read`/`hashline_edit` は仮の名前。
- `adapter_policy` と `adapter_choice` は dispatch 葉の統合後に実在する。統合後に config 例を読む既存試験で確認する。
