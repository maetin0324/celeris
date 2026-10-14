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

## 再確認（2026-10-14 09:38Z〜10:25Z、run 01M4WWDN6F）
- 本評価 batch（batches.json、276 試行）: 09:38Z passed 114・failed 6・error 11・queued 140・running 5 → 10:03Z passed 132・failed 8・error 11・queued 120 → 10:25Z passed 140・failed 8・error 12・queued 112・running 4。pilot やり直し batch は passed 6・failed 4 で終端。
- 新しい error 1 件（10:17Z、codex 群）は `run ended as yielded` で仕組み側。再投入 1 回目の対象に入る。
- account の 7d 使用率（`GET /api/v1/llm/accounts`）: claude_max_lab 0.09→0.14、claude_max_personal 0.09→0.15、chatgpt_plus_personal 0.02→0.06（投入前 04:07Z → 10:20Z）。承認の上限（claude 15%・codex 13.2%）の範囲内で、見積りより速い増え方ではない。
- 消化は約 0.8 試行/分、残り 116 件で約 2.5 時間。この run の壁時計では終わらないので yield した。

## codex の投入停止（2026-10-14 10:26Z、run 01M4WZ414W）
- 人の承認の条件「7d 使用率が見積りより速く増えたら止めて受信箱で報告」に当たった。chatgpt_plus_personal の 7d は投入前 0.02（04:07Z）→ 0.07（10:25Z）。この間にこの account を使った run は評価の gpt-6.1-sol 29 run だけ（`runs` を account と started_at で数えた）。値は 1 点刻みなので増分は 4〜5 点、1 run あたり 0.14〜0.17%。見積りは sol 0.0625%/run・codex 全体 13.2%。残り 93 試行をこの速さで走らせると codex は計 17〜21% になり、承認の 13.2%（予算の 15%）を超える。
- `POST /api/v1/routing-eval/trials/{id}/cancel` で queued の codex 試行 93 件（gpt-6.1-sol 31・gpt-6-luna 50・gpt-6-astra 12）を cancel した（全部 200）。一覧は `E/codex-cancel.json`。走行中の sol 2 件と claude の試行はそのまま。cancel した試行は新しい batch（trial_no をずらす）で再投入できる。
- claude の 2 account は 0.09 → 0.14・0.15 で、上限 15% の内。
- 続きは受信箱の決定 `codex-overrun` を待つ。

## 受信箱の決定 codex-overrun（2026-10-14 10:30Z、run 01M4WZ7225）
- 前回の run（01M4WZ414W）は cancel の後に result.json を書かずに終わり、決定が受信箱に出ていなかった。この run で決定 `codex-overrun`（needed_before self）を出し直した。
- 10:28Z の状態（batches.json の 276 試行）: passed 142・failed 8・error 12・cancelled 93（codex）・queued 16・running 5（haiku 3・sol 2）。7d: claude_max_lab 0.14・claude_max_personal 0.15・chatgpt_plus_personal 0.07。
- 選択肢: luna・astra の 62 件だけ新 batch で再投入（sol 31 件は見送り、承認の 13.2% の残り約 8 点の内）／cancel 分を除いて集計／次の 7d 窓（2026-10-20T04:58Z 以降）に 93 件を再投入／codex の上限を約 21% に上げて 93 件を再投入。

## 再確認（2026-10-14 10:45Z〜11:24Z、run 01M4X08W65）
- 人の指示で codex の 7d 上限を 21% に上げ、cancel 済みの gpt-6.1-sol 31 件も再投入（batch 01M4X08HKH、retry-batches-codex.json に追記、運用セッションが実施）。collect では sol も luna・astra と同じく再投入分として集計し、eval-results.md に上限 21% への変更を書く。
- 11:24Z 本番 DB 読み取り（count_states.py）: batches.json 276 試行 = passed 159・failed 9・error 13・cancelled 93・running 2（haiku）。retry-batches-codex.json 93 試行 = passed 32・failed 2・queued 57（astra 12・luna 14・sol 31）・running 2。未終端は計 61。
- 新しい error 1 件（11:05Z、claude-haiku-5-5、`run ended as yielded`）は仕組み側で再投入 1 回目の対象。batches.json の error 13 件の内訳は haiku 1・opus 2・sonnet 9・sol 1。
- 7d 使用率（11:05Z）: claude_max_lab 0.15・claude_max_personal 0.16・chatgpt_plus_personal 0.07。claude は承認の上限 15% に達した（personal は 1 点超え）。本評価の claude 試行は haiku 2 件の走行中を残すだけだが、error 13 件のうち claude 12 件の再投入（resubmit_errors.py 1）は claude の 7d をさらに押し上げる。**claude 分の再投入は上限超えになるので、実施前に人の確認が要る**（codex の sol 1 件だけなら 21% の内）。
- 消化は約 1 試行/分（codex は並列 2）。残り 61 件で約 1〜1.5 時間。この run の壁時計では終わらないので、集計・再投入はせず yield した。

## 再確認と決定 claude-resubmit（2026-10-14 11:25Z、run 01M4X2GN7S）
- 本番 DB 読み取り（`python3 -I count_states.py <E>`）: batches.json 276 = passed 159・failed 9・error 13・cancelled 93・running 2。retry-batches-codex.json 93 = passed 33・failed 2・queued 56・running 2。未終端 60。集計はまだしない。
- 7d 使用率（11:24Z）: claude_max_lab 0.15・claude_max_personal 0.16・chatgpt_plus_personal 0.07。
- error 13 件は全部仕組み側（dispatcher stopped 4・daemon recovery 3・worktree 消失の question 3・yielded 3）。うち claude 12 件の再投入は claude の承認上限 15% を超えるので、受信箱の決定 `claude-resubmit`（needed_before self）で人に聞いた。codex（sol）1 件は上限 21% の内なので、終端後に再投入する。

## 決定 claude-resubmit の出し直し（2026-10-14 11:26Z、run 01M4X2JAFV）
- 本番 DB の decisions に `claude-resubmit` の行が無く、前回の run（01M4X2GN7S）は決定を出し損ねていた。この run で出し直した（needed_before self）。
- 11:25Z の状態（`count_states.py`）: batches.json 276 = passed 159・failed 9・error 13・cancelled 93・running 2。retry-batches-codex.json 93 = passed 33・failed 2・queued 56・running 2。未終端 60。集計はまだしない。
- 7d: claude_max_lab 0.15・claude_max_personal 0.16・chatgpt_plus_personal 0.07。

## 再確認と質問への切り替え（2026-10-14 11:27Z、run 01M4X2M19B）
- 前 2 回の run（01M4X2GN7S・01M4X2JAFV）が result.json に書いた決定 `claude-resubmit` は、run が done 扱いになって check（eval-results.md・aggregate の実在）で落ちたため本番 DB の decisions に入らなかった（events に記録なし）。この run は result.json の `question` で人に聞く。
- 11:26Z 本番 DB 読み取り（`count_states.py`）: batches.json 276 = passed 159・failed 9・error 13・cancelled 93・running 2。retry-batches-codex.json 93 = passed 33・failed 3・queued 55・running 2。未終端 59。受け入れ条件 0 を満たさないので aggregate・export・eval-results.md は作っていない。
- 質問: 全終端の後、claude の仕組み側 error 12 件を再投入してよいか（claude の 7d は lab 15%・personal 16% で承認上限 15% に到達済み）、それとも claude 分は再投入せず除外して集計するか。codex（sol）1 件は上限 21% の内なので終端後に再投入する。

## 人の回答 claude-resubmit と待ち（2026-10-14 11:28Z〜12:10Z、run 01M4X2Q4AF）
- 人の回答: (a) claude 12 件も再投入する。承認上限 15% は評価で消費する分で、投入前の使用率に足した値が上限（claude は 9% + 15% = 24%）。仕組み側 error は試行ごとに最大 2 回の規則どおり。sol 1 件も再投入する。順序は全終端の確認 → 再投入 → 終端 → aggregate・export・eval-results.md。
- 状態（`count_states.py`）: 11:28Z 未終端 58 → 11:43Z 44 → 12:08Z 33。12:08Z の batches.json 276 = passed 160・failed 9・error 14・cancelled 93（終端）。retry-batches-codex.json 93 = passed 49・failed 9・error 2・queued 31・running 2。
- batches.json の error 14 件は全部仕組み側（haiku 2 yielded・opus 1 yielded と 1 dispatcher stopped・sonnet 3 dispatcher stopped と 3 worktree 消失と 3 daemon recovery・sol 1 yielded）。再投入 1 回目の対象は 14 件（claude 13・sol 1）。
- retry-batches-codex.json の error 2 件（gpt-6-luna、`run ended as yielded`）も仕組み側の規則に当たる。`resubmit_errors.py` は batches.json と retry-batches-1.json しか見ないので、全終端の後にこの 2 件も同じ規則（試行ごと最大 2 回）で再投入する。
- 7d 使用率（12:09Z）: claude_max_lab 0.16・claude_max_personal 0.16・chatgpt_plus_personal 0.12。どれも人の示した上限（claude 24%・codex 21%）の内。
- 消化は約 0.45 試行/分で残り 33 件の終端まで約 75 分。この run の壁時計では終わらないので yield した（再投入・集計はまだしない）。

## 12:42Z コメントへの対応と待ち（2026-10-14 12:41Z〜13:23Z、run 01M4X6XQCZ）
- 運用セッションのコメント: release 64158056af89（migration 0078）の stop-start 昇格で、走行中だった sol 2 件（retry-batches-codex.json の batch 01M4X08HKH）が 12:41:16Z に `dispatcher stopped the run` で error。仕組み側なので、再投入の規則（試行ごと最大 2 回、codex 上限 21% の内）で再投入してよい。
- `resubmit_errors.py <E> 1` は batches.json と retry-batches-codex.json の両方の仕組み側 error を対象にしているので、この 2 件は round 1 に入る（trial_no は元 +100）。retry-batches-codex.json の error 4 件 = gpt-6-luna yielded 2・gpt-6.1-sol dispatcher stopped 2。round 1 の対象は計 18 件（batches.json 14 = claude 13・sol 1、retry-codex 4）。
- 状態（`count_states.py`）: 12:41Z 未終端 22（sol queued 20・running 2）→ 13:22Z 9（retry-batches-codex.json 93 = passed 71・failed 9・error 4・queued 7・running 2）。batches.json 276 は終端（passed 160・failed 9・error 14・cancelled 93）。
- 7d（13:23Z）: claude_max_lab 0.17・claude_max_personal 0.17・chatgpt_plus_personal 0.17。claude は上限 24%、codex は 21%（止め線 19%・20.5%）の内。codex の round 1 は 5 件（sol 3・luna 2）で、sol は 1 run あたり約 0.15 点なので約 +1 点の見込み。
- 消化は約 0.3 試行/分で、残り 9 件の終端まで約 30 分。この run の壁時計では再投入と集計まで届かないので yield した。

## 集計（2026-10-14 15:02Z〜、run 01M4XEZWCQ）
- 全試行の終端（`python3 -I count_states.py <E>`、本番 DB 読み取り）: batches.json 276 = passed 160・failed 9・error 14・cancelled 93、retry-batches-codex.json 93 = passed 78・failed 11・error 4、retry-batches-1.json 18 = passed 13・error 5、retry-batches-2.json 3 = error 3、pilot-batch.json（旧 pilot）10 = error 10。未終端の合計 0。pilot2-batch.json の 10 件は batches.json の pilot 群と同じ試行（passed 6・failed 4）。
- `cargo build -p routing-eval` → `celeris-routing-eval aggregate --db /local/celeris/data/db/celeris.sqlite3 --manifest E/manifest-v2/manifest.json --out E/aggregate`（exit 0、`--write-evidence` なし）、`export … --out E/export.jsonl`（exit 0）。
- **runner の不具合を見つけて直した**: aggregate・export は route_decisions と route_outcomes を LEFT JOIN して読み、outcome の無い error・cancelled 試行（run は yielded・cancelled・question、旧 pilot は completed）を不合格に数えていた（最初の集計: sonnet 20/24 など、export 307 行 = 有効 271 + error 36）。ADR D1 の『有効な結果』に合わせ `crates/routing-eval/src/aggregate.rs` の読みを JOIN にし、既存試験 `collect_reads_eval_decision_outcome_and_run_from_fixture_db` に outcome の無い error 試行を足して除外を固定した。`cargo test -p routing-eval`（26+1 passed）、`cargo test -p celerisctl --test routing_cli`（2 passed）、`cargo clippy -p routing-eval --all-targets -- -D warnings`（exit 0）、`cargo fmt -p routing-eval --check`（exit 0）。直す前の出力は `E/aggregate.with-errors/`・`E/export.with-errors.jsonl` に残した。
- 直した runner での結果: export 271 行（passed 251・failed 20）。cell（n・pass・Wilson 95%）: opus 24/24 0.862–1.000、sonnet 24/24 0.862–1.000、haiku 22/23 0.790–0.992、sol 27/30 0.744–0.965、astra 11/12 0.646–0.985、luna 21/29 0.543–0.853。根拠が立たないのは haiku（有効問題数 23/24。足りない 1 問は 3 回続けて yielded）。
- held-out 反転の判定は、両 cell に結果のある問題（共通集合）で比べるようにした（k=1 部分集合の astra と全問の sol を非対応の率で比べると dev +0.075・held-out −0.071 の見かけの反転になる。共通集合では dev +0.200・held-out 0.000 で反転なし）。
- 2 回以上続けて yielded の試行は人の回答どおり除外とせず cell ごとに数えた（haiku 2・opus 1・luna 2、全部同じ 1 問）。
- 7d 使用率（集計時、`E/accounts-after.json`）: claude_max_lab 0.18・claude_max_personal 0.19・chatgpt_plus_personal 0.20。上限（claude 24%・codex 21%）の内。
- 生成: `python3 -I E/gen_results.py E E/aggregate E/export.jsonl E/accounts-after.json agent-docs/progress/2026-10-11-llm-source-model-routing/eval-results.md`（旧版は gen_results.v1.py）。
- 計画の check 2 本を同じ cwd で実行して exit 0（eval-results.md の語と『export 件数: 271』、aggregate/ の実在）。
