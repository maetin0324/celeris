---
tasks: [01M4VAAKFJHRSVKZ3996NFKSKS]
title: eval-run collect — 集計（全試行の終端待ちで停止）
status: blocked
updated: 2026-10-14
---
# collect: 集計

## 状態: 停止（全試行が終端でない）

受け入れ条件 0「全試行が終端」を満たさないため、aggregate・export・eval-results.md には進まなかった。
運用セッションのコメント（2026-10-14 06:35Z）どおり、本番は reserved_runs = 0 で投入停止中（修正 task 01M4WHYHVZTYNBRGM5C3PWHMZE の本番反映待ち）。queued の試行は消さずに待つ。

## 確かめた command と結果（2026-10-14 06:35Z、本番 DB 読み取りのみ）

`sqlite3 "file:/local/celeris/data/db/celeris.sqlite3?mode=ro"` で batches.json・pilot-batch.json・pilot2-batch.json の trial_id ごとに eval_trials.status を数えた。

| batch | 群 | 状態 |
|---|---|---|
| 01M4W9H8WDQG… | claude | passed 46・error 2 |
| 01M4W9H8WNXE… | claude | error 1・queued 47 |
| 01M4W9H8WW9W… | claude | queued 48 |
| 01M4W9H8Z511… | codex | queued 60 |
| 01M4W9H8ZDWY… | codex | queued 50 |
| 01M4W9H8ZKD0… | codex | queued 12 |
| 01M4W7JDSJ…（pilot やり直し） | codex | passed 6・failed 4 |
| 01M4W35QEM…（旧 pilot、集計除外） | codex | error 10 |

eval_trials 全体: passed 52・failed 4・error 13・queued 217。running・scoring は 0。

## 未解決事項
- queued 217 試行が終端になるまで collect は再実行できない。投入再開（reserved_runs を 2 に戻す）後、全試行の終端を待ってからこの unit をやり直す。
