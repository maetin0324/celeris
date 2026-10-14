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

## 再確認（2026-10-14 06:37Z、attempt run 01M4WJ2B5Y）
本番 DB 読み取り: passed 52・failed 4・error 13・queued 217（変化なし）。本番 config は reserved_runs = 0 のまま。人の回答（修正 task 01M4WHYHVZ の本番反映 → reserved_runs = 2 で再開 → queued 全終端、error 13 は新 batch で 1 回だけ再投入可）に従い、何もせず待つ。

## 再確認（2026-10-14 08:18Z〜08:53Z、run 01M4WQTFCV、投入再開後）
- 08:18Z: passed 60・failed 4・error 15・queued 204・running 3。08:28Z: passed 71・queued 194。08:53Z: passed 83・failed 4・error 18・queued 178・running 3。
- 消化は約 1 試行/分。残り約 180 試行で終端まで約 3 時間。この run の壁時計では終わらないので yield した（集計はまだしない）。
- 08:52Z ごろ、走行中の claude-sonnet-5-5 の 3 試行（`01M4W9H8WQ1H…`・`01M4W9H8WQ80…`・`01M4W9H8WQ6P…`）が、run の途中で作業 worktree と CARGO_TARGET_DIR を外から消され、question で終わって error になった。3 件が同時刻に消えたので、仕組み側の掃除が走行中の試行の作業場所を消したと見ている（運用セッションに確認を依頼）。これらは人の回答どおり 1 回の再投入の対象（`resubmit_errors.py`）。

## 再投入の方針の変更（2026-10-14 08:57Z コメントへの対応、run 01M4WSY725）
- 運用セッションの指示: 再投入は仕組み側の error（worktree 消失・dispatcher stopped・daemon recovery・yielded）に限り、試行ごとに最大 2 回。failed とそれ以外の error は再投入しない。
- `artifacts/eval/resubmit_errors.py` をこの規則に書き直した（旧版は `resubmit_errors.v1.py`）: `python3 -I resubmit_errors.py <E> 1` は batches.json の仕組み側 error を `retry-batches-1.json` へ（trial_no +100）、`… 2` は retry-batches-1 の仕組み側 error を `retry-batches-2.json` へ（trial_no +200）。どちらも対象群の全試行が終端でなければ投入しない。`--dry` で数だけ出す。
- 08:55Z 時点の本評価 batch の error 11 件の内訳: dispatcher stopped 4・daemon recovery 3・worktree 消失の question 3・yielded 1。11 件とも仕組み側で、再投入の対象。旧 pilot の error 10 件（run ended as error）は集計除外のまま対象外。
- 状態の推移（本番 DB 読み取り）: 09:05Z passed 96・failed 4・error 21・queued 160・running 5 → 09:21Z passed 102・queued 155 → 09:37Z passed 114・failed 6・error 21・queued 141・running 4。新しい error は無い。消化は 0.6〜1.2 試行/分で、残り 141 件の終端まで 2〜4 時間。この run の壁時計では終わらないので yield した（集計・再投入はまだしない）。
