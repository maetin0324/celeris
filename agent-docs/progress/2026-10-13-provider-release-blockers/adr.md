---
title: provider 廃止リリース不具合 5 件の ADR 付記
tasks: [01M4T98YHKGT945HH1588SC2VR]
status: complete
completed: 2026-10-13
---

# provider 廃止リリース不具合 5 件の ADR 付記

対象 ADR `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の末尾に「2026-10-13 付記（本番投入の不具合）」を追加した。旧 routing event の read-only 互換、Pi 宣言、proxy wire model、OpenCode namespace、execution class 別 route の5件について、決定・根拠・変更箇所・実装 leaf と回帰試験名を記録した。付記は runbook の本番移行内容も実装 leaf に委ねる形で記し、既存節は変更していない。

## 完了日

2026-10-13

## 証拠コマンドと結果

- `grep -q '2026-10-13 付記（本番投入の不具合）' agent-docs/adr/2026-10-11-llm-source-harness-selection.md && test -s agent-docs/progress/2026-10-13-provider-release-blockers/adr.md` — exit 0。
- 前回の範囲 check は exit 1。原因は作業ツリー直下の未追跡 `adr-appendix.txt` で、付記草稿が成果物の許可 path 外として検出されていた。草稿を削除し、ADR と本記録だけを残した。
- 指定の範囲 check（`CELERIS_WU_SCOPE_PATHS` の snapshot を使い、許可 path 以外を拒否）— exit 0。変更 path は ADR と本記録のみ。
- `git diff --quiet "${CELERIS_WU_BASE:-HEAD}" -- crates/` — exit 0（crates 差分なし）。
- `git diff --check "$CELERIS_WU_BASE"` — exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` — ADR と本記録の2 path のみ。

## 未解決事項

Claude Code / Codex harness を proxy 経由にする機能は設計対象外。endpoint/auth の注入、daemon 起動経路、proxy bearer secret の受け渡しを別 task で設計するまで Codex OAuth proxy fallback を設定しない。実装 leaf は本番 config/runbook の値を config backup と照合して反映する。

## 提案

実装後、担当 leaf で `docs/ops/provider-llm-source-migration.md` を決定内容に合わせて更新し、指定した回帰試験を実行する。段の統合では leaf checks に加えて daemon workspace `test-parallel` を実行する。
