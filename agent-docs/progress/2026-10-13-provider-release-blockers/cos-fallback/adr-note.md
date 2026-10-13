---
tasks: [01M4TVHNHGGP1Y7CW1C7WJBXFX]
completed: 2026-10-13
---

# CoS の Codex fallback 撤去 — ADR 付記（決定 cos-codex）

## 決定

人の決定 `cos-codex`（2026-10-13）: (a) CoS は Claude のみ。CoS の候補から Codex OAuth（`codex_oauth` / `gpt-6-astra`）を外す。

## 変更内容

- `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の末尾に `### 2026-10-13 付記（CoS の Codex fallback 撤去 — 決定 cos-codex）` を追加した。既存の文は書き換えていない。
- 付記は次を置き換えると宣言する: 本文「## CoS の経路」の Codex fallback 例、実装突き合わせ付記（2026-10-12）が指す runbook の CoS 行と `[cos]`・`[[llm_routes]]` の Codex 宣言、選択結果の表の CoS 行と確認手順。
- 戻す条件を書いた: `claude_code` harness を llm-proxy 経由にする接続（endpoint・auth 注入、daemon 起動経路、proxy bearer secret の引き渡し）が要る。別 task とする。

## 同じ工程の他の leaf（この葉では触らない）

- runbook `docs/ops/provider-llm-source-migration.md`、進捗 TOML `agent-docs/progress/2026-10-11-provider-layer-redesign.md`、試験 `celeris::config::tests::migration_runbook_toml_loads` は `align-cos` の leaf が揃える。
- 進捗ファイル `agent-docs/progress/2026-10-13-provider-release-blockers.md` は closeout の leaf が書き直す。

## 証拠

- 付記の見出し: `grep -n '^### 2026-10-13 付記（CoS' agent-docs/adr/2026-10-11-llm-source-harness-selection.md` → 448 行目に 1 件。
- 既存文の変更なし: `git diff` は ADR の 16 行追加のみ（削除 0 行）。
- 前回の check 不合格（`missing: 2026-10-12 付記`）の原因: 付記の置き換え対象の列挙が「実装突き合わせ付記（2026-10-12、…）」の書き方で、見出しの語句 `2026-10-12 付記` を含んでいなかった。対象の列挙に「2026-10-12 付記（planner・reviewer の選択）」を明記して直した（付記の内容は変えていない）。
- 付記の check（再実行、`sh -c` と bash の両方）: 6 語すべて含む → exit 0。
