# move-records: docs/progress の新設 6 file を agent-docs/progress へ移し、PROGRESS.md を main 版へ戻す

WorkUnit `move-records`（親: records-fix）。コードは変えていない。

## 変更

- `git mv` で `docs/progress/{merge-train-evaluation,phase-direct-route,phase-writeset,phase-writeset-actual-record,phase-writeset-core-model,review-sync-main}.md` を `agent-docs/progress/2026-10-03-<元の名前>.md` へ移した。
- front matter を progress-index の必須欄（title・tasks・status・updated）に揃えた。`merge-train-evaluation` と `phase-direct-route` は 1 行目が front matter でなかったため先頭に置き、本文の `tasks:` 行は front matter へ移した。
- `docs/progress/review-sync-main.md` を移動先への相対 symlink（`../../agent-docs/progress/2026-10-03-review-sync-main.md`）にした。移動先の front matter 直後に、子 task の受け入れ検査がこの path を読むための 1 行を書いた。
- `agent-docs/PROGRESS.md` を `git checkout $(git merge-base HEAD main) -- agent-docs/PROGRESS.md` で main 版へ戻した。
- このブランチが足した追記（+ 行 120 本、front matter 3 行を除く 117 行）を `2026-10-03-review-sync-main.md` の末尾「## agent-docs/PROGRESS.md から移した節」へ逐語で写した。リンクは旧 PROGRESS.md 基準のため、リンク検査の対象外にするため code block に入れた。
- 旧 path への参照を新 path へ直した: `merge-train-evaluation.md`（表の `phase-direct-route` / `phase-writeset` 2 件、evaluate 節の参照）、`review-sync-main.md`（merge-train の参照）、`phase-effect-ab/ab-record.md`（merge-train の参照）。`review-sync-main` の参照は symlink が残るため変えていない。
- `tasks` の id は、PROGRESS.md の追記 front matter にあった `01M3XZ5PYSTTC6GXAH8TZVRHSA` を `review-sync-main` 側へ加えた。

## 検査

- `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0。
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`、exit 0。
- `git diff --quiet main -- agent-docs/PROGRESS.md` → 差分なし（main と同一）。
- `docs/progress/` の通常ファイルは `README.md`（main 既存）のみ。`review-sync-main.md` は symlink。
- 追記の逐語確認: 移動元の + 行 117 本を `agent-docs/progress/` の全 .md と照合 → 欠落 0。
- 旧 path 参照: `git grep -n -E 'docs/progress/(merge-train|phase-direct|phase-writeset)'` → 0 件。

## 未解決・提案

- `merge-train-evaluation.md` は eval-fix WorkUnit も編集する。統合時に同じ行が衝突しうる（表名・migration 番号の箇所）。git の rename 追跡で解けるはずだが、衝突したら eval-fix 側の内容を採る。
- `review-sync-main.md` の symlink は、子 task の受け入れ検査が読む path を保つための暫定。検査の path を移動後の path に直せた時点で消してよい。
