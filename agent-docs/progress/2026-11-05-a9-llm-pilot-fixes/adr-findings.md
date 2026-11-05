---
title: A9 pilot の不具合の ADR 付記（adr-findings）
tasks: [01M6MH3ZCVYGBFX6ZF8TTE17NJ]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# A9 pilot の不具合の ADR 付記

完了日: 2026-11-05

## 結論
`agent-docs/adr/2026-11-05-a9-llm-pilot-findings.md` を書いた。修正方針は 2（D1 到達に数えない）・1（D2 課題と hidden）・3（D3 effect stub）の順、続けて D4（reviewer の確認）。各決定に理由・採らなかった案・旧い仕組みの扱い・実装 task への引き渡しがある。コードは変えていない。

## commit（件名）
- adr-findings: A9 実 LLM pilot の修正方針を ADR 付記に書く

基準は `$CELERIS_WU_BASE`。

## 検査（提出時の HEAD で実行）
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh`
- `git diff --quiet $CELERIS_WU_BASE -- crates/ web/ scripts/`

## 未解決・提案
- 前の ADR（long-horizon-bench の D5・付記 A8 の 4、project-controller の付記 A9）の旧い文面への「改訂」注記は、実装 task が実装と同じ変更で入れる。
- 未確認の点は ADR の「未確認」節にまとめた。
