# agent-docs/ — agent 向け文書（読む順）

ADR-0128 により、設計判断の経緯・進捗・作業記録は `docs/` ではなくここに置く。人が現行の仕様・使い方を読みたいときは
[`docs/README.md`](../docs/README.md) を見る。

読む順:

1. [`../docs/SPEC.md`](../docs/SPEC.md) — 現行の仕様（正本）
2. [`../docs/architecture-map.md`](../docs/architecture-map.md) — subsystem → crate/module → entry point → ADR の索引
3. 関係する ADR — `adr/`（番号付き、0128 まで）と `adr/YYYY-MM-DD-<slug>.md`（0128 以降の新しい ADR）。
   `gui/adr/`・`web/adr/` はそれぞれ gui・web/ SPA 固有の ADR。
4. `progress/` の索引 — `sh scripts/dev/progress-index.sh` で現在地（running・blocked を先に）を生成する。
   旧 `PROGRESS.md`（凍結、以後の追記は無い）はこの索引ができる前の記録。

その他:

- `reports/` — 日付付きの調査・実装報告
- `notes/` — 作業メモ
- `ops/` — 一度きりの作業手順・agent 向け運用規則
- `guides/` — agent 向けの設計・検証・作業規則（例: `guides/testing.md`）
- `gui/`, `web/` — gui・web/ SPA の設計記録・ADR・進捗
- `GOAL_TEMPLATE.md` — 新規 goal のテンプレート

ADR の採番規則（番号は 0128 で止まり、以後は日付+slug）は `adr/0128-docs-layout.md` D5 を見る。
進捗ファイルの書き方（task ごと・並列 WorkUnit ごと）は同 ADR D3 を見る。
