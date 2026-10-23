# agent-docs/ — agent 向け文書（読む順）

ADR-0128 により、設計判断の経緯・進捗・作業記録は `docs/` ではなくここに置く。人が現行の仕様・使い方を読みたいときは
[`docs/README.md`](../docs/README.md) を見る。

読む順:

1. [`../docs/SPEC.md`](../docs/SPEC.md) — 現行の仕様（正本）
2. [`../docs/architecture-map.md`](../docs/architecture-map.md) — subsystem → crate/module → entry point → ADR の索引
3. 関係する ADR — `adr/`（番号付き、0128 まで）と `adr/YYYY-MM-DD-<slug>.md`（0128 以降の新しい ADR）。
   `web/adr/` は SPA 固有の ADR。廃止された画面の ADR も歴史的判断として保持する。
4. `progress/` の索引 — `sh scripts/dev/progress-index.sh` で現在地（running・blocked を先に）を生成する。
   旧 `PROGRESS.md`（凍結、以後の追記は無い）はこの索引ができる前の記録。

必須規則の正本は repo 直下の `CLAUDE.md` だけ。Claude Code は直接読み、Codex・Pi・ACP は worker が prompt の先頭へ写す（配布経路は地図の「共通 agent 指示と配布」）。

変更の方針: 破壊的変更を是とし、置き換えた旧い仕組みは同じ変更で消す。救出は git と release rollback で行う
（[`adr/2026-10-11-breaking-changes-by-default.md`](adr/2026-10-11-breaking-changes-by-default.md)）。

その他:

- `reports/` — 日付付きの調査・実装報告
- `notes/` — 作業メモ
- `ops/` — 一度きりの作業手順・agent 向け運用規則
- `guides/` — agent 向けの設計・検証・作業規則（例: `guides/testing.md`、WU の `checks` と範囲 check の書き方 `guides/work-unit-checks.md`）
- `web/` — SPA の設計記録・ADR・進捗
- `GOAL_TEMPLATE.md` — 新規 goal のテンプレート
- `PROGRESS.md` — 旧 `docs/PROGRESS.md`（凍結。追記しない）

検査（land 系の check に必ず入れる。ADR-0128 D7）: `sh scripts/dev/check-doc-links.sh`（参照の実在）、
`sh scripts/dev/check-adr-numbers.sh`（ADR の名前と番号）、`sh scripts/dev/progress-index.sh --check`（進捗ファイルの front matter）。

ADR の採番規則（番号は 0128 で止まり、以後は日付+slug）は `adr/0128-docs-layout.md` D5 を見る。
進捗ファイルの書き方（task ごと・並列 WorkUnit ごと）は同 ADR D3 を見る。
