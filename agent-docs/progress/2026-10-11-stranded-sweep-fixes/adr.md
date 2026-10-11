---
title: adr — 取り残し検出の不備の ADR 付記
updated: 2026-10-11
status: done
tasks: [01M4NM5720BYCYHXT02297GFKK]
---

# adr

完了日: 2026-10-11

- `agent-docs/adr/2026-10-11-stranded-done-branches.md` の末尾に「付記: 取り残し検出の不備（2026-10-11 本番 deploy）」を追加した。F1 `resolve_default_branch`（解決順・消すもの・呼び出し元ごとの失敗時の振る舞い）、F2 `not_applicable`（migration・受信箱に出さない・既存行の移行）、F3 状態が同じなら行を書かない（受信箱 item の `at`・id の同一性）。
- 進捗ファイル `agent-docs/progress/2026-10-11-stranded-sweep-fixes.md` を作成した。
- コードは変えていない（`git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/` は exit 0）。
