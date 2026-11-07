---
title: 健全性v2 G2 guideline/generic — 汎用規約 generic.md
tasks: [01M6VCR06J339SH4JTK3Q8XHEP]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G2 guideline/generic — 汎用規約 generic.md

完了日: 2026-11-07

WorkUnit `generic`（G2 の unit guideline の工程「汎用規約本文と profile の [review] 節」）。基準は `$CELERIS_WU_BASE`。根拠は [ADR 2026-11-07 健全性を設計と review に組み込む](../../adr/2026-11-07-health-review-integration.md) の (1)・D5.1 の表・不変条件 I1/I2。

## 変更
- `crates/task-worker/src/change_rules/generic.md` を新規に書いた。1 行目 `# 変更の規約（汎用）`、`## 規約` に番号付き 10 項目、`## 実装前 brief` に番号付き 7 項目、末尾に「repo の必須規則と健全性の guide に具体があれば、それに従う。」の 1 文。
- 規約 (1)〜(6) と (10) に D5.1 の kind を括弧で併記した: (1) `symptom_fix_outside_owner`、(2) `duplicate_implementation`、(3) `local_hook_in_shared_interface`、(4) `dual_source_of_truth`、(5) `unapproved_fallback_or_switch`、(6) `removal_missing`、(10) `test_change_unjustified`。順は D5.1 の表と同じ。
- `change_rules.rs`・`prompt.rs` の配線は U1 の範囲なので作っていない（この file はまだどこからも読まれない）。

## 証拠（提出時の HEAD で実行）
- 項目数: `awk` で `## 規約` と `## 実装前 brief` の下の `^[0-9]+\. ` 行を数えた → 10 と 7
- 固有名: `grep -ciE 'celeris|TaskStore|Dispatcher|sqlite|events|CLAUDE|crate' generic.md` → 0 件
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → 3 本とも exit 0
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → 上の 2 file だけ
- `bash scripts/dev/test-affected.sh --list` → 7 crate（celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）。足した .md はどの source からも読まれず build に入らないため、nextest と clippy は走らせていない（配線する U1 で走る）
- `cargo fmt --all -- --check` → exit 0

## 未解決事項
- 無し。配線（U1）と profile の `[review]` 節（WU `profiles`）は別の unit。

## 提案
- 無し。
