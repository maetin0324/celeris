---
status: accepted
tasks: [01M6VQH1GY87Q79J406DFNQP6G]
---
# root_done_tick: delivery の待ちの間は事実を作らず、進捗ファイルは git grep で探す

## 背景
本番（release 66d9c1dae567）の slow tick 234 件のうち 154 件で、cos.root_done_tick が 1 tick 平均 4.7 秒・最大 27.5 秒かかった。WU root-done-probe（ADR `2026-11-07-root-done-tick-phase-timing`）は本番 DB の写しで同じ遅さを再現し、時間の 99.6〜99.9% が `progress_markdown` だと測った（`agent-docs/progress/2026-11-07-root-done-tick-slow/root-done-probe.md`）。

原因は 2 つの掛け算である。
1. `root_done_advance` は new の行ごとに毎 tick `collect_facts` を呼び、その後で delivery が待ちの状態（Reviewing・MergeQueued・Merging・Preparing）なら何もせずに戻る。待ちは最大 `DELIVERY_WAIT_SECS`（3600 秒）続くので、作った事実は 1 時間のあいだ毎 tick 捨てられる。待ちの判定に要るのは delivery の state だけである。
2. `progress_markdown` は delivery の head の tree で `agent-docs/progress` の `.md` を名前順に 1 file ずつ `git show` する。写しの root では 1 行・1 tick あたり git の process を 1,549 回起こし、git 1 回 3.6〜11.4 ms で 1 行 5.5〜18 秒になった。費用は進捗ファイルの数に比例し、file は日々増える。

store の待ち（lock）は写しでは 1 tick 0〜3.5 ms で、原因ではなかった。

## 決定
- D1: new の行は、先に `delivery_get` だけを読む。delivery が待ちの状態で、行の年齢が `DELIVERY_WAIT_SECS` 未満なら、事実を作らずに戻る。待ちの tick の費用は行ごとに store の読み取り 1 回になり、git は起こさない。待ちが終わった tick（または上限を過ぎた tick）に、読んだ delivery をそのまま使って事実を 1 回作る（`delivery_get` を 2 回呼ばない）。待ちの分岐の条件（state の 4 つと上限）は変えない。
- D2: `progress_markdown` は、`git grep -l -z -E --no-color -e '^[[:space:]]*tasks:.*<task id>' <head> -- agent-docs/progress` で、`tasks:` の行に task id を含む file だけを候補にする。候補を grep の出力順（tree の名前順。`ls-tree` と同じ順）に `git show` し、`progress_mentions_task`（front matter の `tasks:` の判定）を満たす最初の file を返す。返す file は今までと同じ（`tasks:` にその id を持つ名前順で最初の file）で、git の起動は「grep 1 回 + 候補の数」になる。候補は front matter または本文の `tasks:` 行にその id を書いた file だけなので、並列 WU の進捗（同じ id を持つ数件）を除けば 1〜数件である。grep が一致なし（exit 1）・失敗のときは `None`（事実では「不明」）で、今までと同じ。
- D3: `-z` で path を NUL 区切りにするので、`core.quotePath` が path を引用符で囲むことは無い。`-E` と `--no-color` は、利用者の `grep.patternType`・`color.grep` の設定で出力や正規表現の解釈が変わらないように付ける。
- D4: 計測（ADR `2026-11-07-root-done-tick-phase-timing`）に `cos.root_done.delivery_wait_rows`（待ちで事実を作らずに戻った行の数）を足す。D1 の `delivery_get` は `cos.root_done.facts.delivery_get.*` に数える。`facts_calls`（事実づくりの回数）は待ちの行では増えない。
- D5: 結果を process の中に覚える案（(task, head) ごとの cache）は採らない。D1 と D2 で、1 行あたりの git の起動は待ちの後の 1 回の事実づくりで「1 + 候補の数」になり、cache が要る大きさではない。daemon の状態を増やさない。

## 試験（時計・sleep・CPU の負荷を使わない）
- dispatcher の試験: 進捗ファイルを N 件（3 件と 60 件）持つ git repo と、Reviewing の delivery を持つ root の new 行を作り、`tick_cos_chat_launch` を 3 回まわす。3 回とも `facts_calls` 0・`delivery_wait_rows` 1・`facts.delivery_get` の呼び出し 1・`progress_git_calls` 0 で、行は new のまま。delivery を Merged にした次の pass で `facts_calls` 1・`progress_git_calls` 2（grep 1 + show 1）になり、行は fallback に進む。N が 3 でも 60 でも同じ回数になることで、進捗ファイルの数と待ちの tick 数に比例しないことを固定する。時間は試験の時計（`advance`）だけを進める。
- `progress_markdown` の単体試験: 本文に id を書いただけの file・`tasks:` に別の id を持つ file・`tasks:` にその id を持つ file 2 件を置き、名前順で最初の該当 file を返し、git の起動が「1 + 候補の数」であること、該当が無ければ `None` で git 1 回であることを確かめる。

## 本番への移行
config・migration の変更は無い。promote の後に operator が journal の "slow tick phases" で cos.root_done_tick の中央値と最大を見る（手順は `agent-docs/progress/2026-11-07-root-done-tick-slow/root-done-fix.md`）。
