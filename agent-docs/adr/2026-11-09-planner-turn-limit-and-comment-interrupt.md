---
status: accepted
tasks: [01M6ZE0AJQJVE240W7DEWSE561]
---
# planner run の turn の上限: 残った計画 file は検証にかけ、comment の割り込みは試行に数えない

- 関係: ADR-0074 D5.3（planner の上限）、ADR-0072 D14（不正な計画の再試行）、ADR-0079 D9（/3 の `plan_invalid`）、
  ADR-0140 付記（comment-resume の割り込みの判定）、ADR 2026-10-27-review-resync-halt-visible-and-reviewer-reuse

## 背景（本番の事実。operator が本番 DB を読み取りで集計した）

1. 2026-11-06 以降、planner run 13 件が `error_max_turns`（`BudgetExhausted{Turns}`）で終わった。すべて健全性 v2 の task で、
   費用の合計は約 23 USD（1 件 1.2〜2.4 USD・97〜538 秒）。task は 01M6VTFG9K、01M6VTFGWF、01M6TYVJSTYG、01M6XHG4PV、
   01M6TYVJSWEY、01M6Y8EG6Q（4 回）、01M6YJF7HJ（2 回）、01M6ZDF99M、01M6ZDF9Q4。本番 config に `[execution.planner]` の
   上書きは無く、既定の 24 turn / 900 秒で走った。
2. 失敗した run の tool 呼び出しは 25〜28 回で、多くは ADR・進捗・コードの読み取りだった。計画の下書きや自前の検証の
   script に入ったところで上限に達した。壁時計（900 秒）に当たった run は無い。
3. 01M6YJF7HJ の 2 回目の run は `artifacts/execution-plan.json` を書き終え、自前の検証も通していた。しかし run が
   `error_max_turns` で終わったので、daemon は計画を検証せず `execution-plan.rejected.<run>.attempt-2.json` に移した
   （ADR-0072 D14「計画の検証は run が `Terminal::Done` で終わったときだけ」）。人がその file を土台にするよう指示した
   3 回目は 105 秒・0.99 USD で終わり、設計 review を経て実行された。
4. 01M6ZDF9Q4 では、planner run が人（operator）の comment の割り込み（outcome `interrupted: comment`）で止まり、
   計画の試行 1 回に数えられた。続く 1 回が上限で終わった時点で「2 回とも検証に通らなかった」として `plan_invalid` の
   決定が出た（ADR-0079 D9）。割り込みは計画の失敗ではない。
5. G2（01M6TYVJSTYG、2026-11-08 に本番へ）で planner の prompt に規約・design_brief・health の文脈が入り、planner が
   読む量と書く量が増えた。24 turn は G2 より前（ADR-0074 D5.3、2026-09）に決めた値である。

## 原因

- (a) 上限が今の planner の仕事量に足りない（事実 1・2・5）。24 turn は「planner はコードを調べない」前提で 40 から
  下げた値で、G2 で増えた読む物・書く物（規約・design_brief・health の文脈）を見込んでいない。
- (b) 上限で終わった run の成果を捨てている（事実 3）。計画の採否は決定的な検証と設計 review で決まるのに、run の
  終わり方（`Done` か否か）が検証の手前で門になっている。
- (c) 試行の数え方が割り込みを含む（事実 4）。`planner_attempts_in_window` は窓の中の
  `WorkerStarted{role: Planner}` をすべて数え、人の comment で止めた run も「不正な試行」の 1 回にしていた。

## 決定

### D1. turn の上限で終わった planner run の計画 file は、通常の検証と設計 review にかける

- planner run が `BudgetExhausted{Turns}` で終わったときも、`Terminal::Done` と同じく `artifacts/execution-plan.json` を
  読み、同じ手順にかける: 形式の parse・harness の検査・`validate_with`（計画の上限、/3 の最後の試行での上限の緩和を含む）・
  退役した key・daemon が足した key の検査・design brief の検査（D3.3）・設計 review（D3.5・D3.6）・root の計画の承認
  （ADR-0079 D8）。どれかに落ちれば、今と同じく不正な試行として `execution-plan.rejected.<run>.attempt-<n>.json` に
  移し、再試行・`plan_invalid`（/3）・atomic（/1・/2 の新規）・blocked（/1・/2 の replan）に進む。
- file が無い・JSON として読めないときも不正な試行である。理由の文には run の終わり方を添え
  （`artifacts/execution-plan.json が見つからない（budget_exhausted(Turns): …）`）、次の planner に「上限で終わった」ことを渡す。
- 対象は turn の上限だけにする。claude-code の `--max-turns` は turn を終えてから止まるので、file の書き込みは途中で
  切れない。壁時計（`WallClock`）は SIGTERM で途中を止めるので、書きかけの file を拾いうる。`Tokens` は木の token 予算の
  停止（ADR 2026-11-06-budget-usd-caps の止める判断）、`Context` は planner では起きていない。これらと `Yielded`・
  `Question`・`Error` は今どおり不正な試行である。
- run の記録は変えない。`WorkerFinished.end` は `BudgetExhausted{Turns}` のまま残り、採用された場合だけ outcome が
  `done: <n> work unit(s) planned` になる。operator は「上限で終わった件数」と「そのうち採用された件数」を別に数えられる。
- 判定は `design_gate::plan_file_checked`（`run_end_of` と同じ所。dispatcher の module の大きさの制約による）に置き、
  `on_planner_finished` はそれを呼ぶ。

### D2. 人の comment の割り込みで止まった planner run は試行に数えない

- `planner_attempts_in_window`（ADR-0072 D14 の「窓」の中の planner 試行の数。`MAX_PLANNER_ATTEMPTS` = 2 と比べる）は、
  人の comment の割り込みで止まった run の `WorkerStarted{role: Planner}` を数えない。判定は ADR-0140 付記の
  `interrupted_by_comment`（`WorkerFinished.outcome` が `interrupted: comment`、または直前の遷移が comment の割り込みで
  `interrupted: …` で閉じた run）をそのまま使い、二重に実装しない。
- この数は 3 か所で使われる: 再試行か諦めるかの判断、/3 の最後の試行での上限の緩和（ADR-0079 D3）、拒否した計画の
  file 名の試行番号。3 か所とも割り込みを除いた数になる。
- 他の止め方（cancel・pause・daemon の停止・孤児の回収）はこの ADR の範囲に入れない（本番の事実が無い）。

### D3. `[execution.planner]` の既定を 40 turn / 1,200 秒に戻す

- `max_turns` 24 → **40**、`max_wall_secs` 900 → **1,200**（ADR-0074 D5.3 より前の値）。事実 2 の run は 24 turn で
  97〜538 秒（1 turn 最大約 22 秒）だったので、40 turn に上げると 900 秒の壁時計が次の上限になる。そのため壁時計も戻す。
- ADR-0074 D5.3 のほかの決定（planner の lane は `standard`、「計画 run はコードの調査をしない」の prompt、計画の
  サイズの上限）は変えない。
- 既定の正本は `task_core::PlannerConfig::default` の 1 か所にする。`crates/celeris/src/config/execution.rs` の
  `default_planner_max_turns`・`default_planner_max_wall_secs` は、これまで同じ値を別に持っていた。今後はそこから導く。

### D4. planner の prompt に turn の予算と D1 の規則を書く

- planner の prompt（`build_execution_plan_prompt`）に「## 予算 (budget)」の節を足す: この run の上限（turn・秒）、
  上限で終わっても完全な `execution-plan.json` は検証と設計 review にかかること、file が無ければ不正な試行に数えること、
  上限の約半分までに計画を一度書き切り、読み直しと自前の検証はその後に行って同じ file を上書きすること。
- execute の run の予算の節（ADR-0072 D10 の `yield` と checkpoint）は planner には出さない（planner の run は続きを
  起こさない）。

## 採らなかった案

- **task の大きさや深さで turn を決める**: 13 件は root と子 task、深さ 0〜2 にまたがり、大きさとの関係を示す
  データが無い。式を決める根拠が無いまま規則と分岐を足すことになる。D5.3 の「1 つの設定値」を保つ。
- **上限で終わった run の session を続ける（resume）**: planner の run は続きを起こさない設計である
  （`run_end_of` は planner の `Yielded` を不正な試行とする）。resume には planner の試行ごとの session の状態が要り、
  長い context をもう一度送る費用もかかる。file を書き終えた run は D1 で救え、書く前に尽きた run は D3 で減らせる。
  前の試行の拒否の理由を次の planner に渡す仕組み（F5-fix3）は既にある。
- **prompt だけで早く書かせる（D4 だけ）**: model は自分の turn の数を見られない。事実 2 では turn の多くが
  読み取りで、prompt だけで上限内に収まる根拠が無い。D4 は D1・D3 の補いとして採る。
- **既定を上げるだけ（D3 だけ）**: 01M6YJF7HJ のように書き終えた計画を捨てる経路が残り、上限に当たった run の費用が
  無駄になる。
- **`MAX_PLANNER_ATTEMPTS` を 3 にして割り込みを吸収する**: 割り込みが無い task でも不正な計画の試行が 3 回に増え、
  ADR-0079 D9 の「2 回」の意味が変わる。割り込みは計画の失敗ではないので、数えないのが正しい。

## 既存の決定との関係

- **ADR-0074 D5.3**: 既定の値（24 / 900）を D3 で改める。lane・prompt・計画のサイズの上限は変えない。D5.3 に付記を置く。
- **ADR-0072 D14**: 「計画の検証は run が `Terminal::Done` で終わったときだけ」を、D1 で「`Done` か turn の上限」に
  広げる。不正なら 1 回だけ再試行する規則は変えない。
- **ADR-0079 D9**: 「/3 の計画が 2 回不正なら `plan_invalid`」は変えない。D2 は「試行」が何かを明確にする
  （人の comment で止めた run は試行ではない）。D1 で、turn の上限で終わっても妥当な計画を残した run は「不正」ではなくなる。
- **ADR 2026-10-27-review-resync-halt-visible-and-reviewer-reuse**: 合否は run の終わり方でなく、決定的な検査で決める
  という同じ考え方に立つ。あちらは reviewer の合格を snapshot に束縛して再利用するが、D1 は前の run の検証を再利用しない
  （残った file を毎回はじめから検証する）。final review と配送の経路は変えない。
- **CLAUDE.md「ワーカーの自己申告だけで完了にしない」**: 計画の採用は今どおり daemon の決定的な検証と設計 review で
  決まる。run の終わり方は採否に使わない。

## 本番への移行

- 本番 config に `[execution.planner]` の上書きは無いので、promote すると D3 の既定（40 / 1,200）が効く。config の
  書き換えは要らない。上書きを置いていれば、その値が優先する。
- promote の後に operator が見る値: (1) planner run のうち `error_max_turns`（`WorkerFinished.end` が
  `BudgetExhausted{Turns}`）で終わった件数（2026-11-06〜08 の 13 件と比べる）、(2) そのうち outcome が
  `done: … planned`（D1 で採用）の件数、(3) `plan_invalid` の決定の件数。
