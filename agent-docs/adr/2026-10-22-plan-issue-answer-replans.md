# ADR 2026-10-22: plan_issue への人の回答で計画を直す（「回答して再開」と「計画を直す」）

## 背景
本番 2026-10-22、moshi-brewed の WU transfer-ui（task 01M5HJ4XEG）が「範囲 check の許可 path が Objective と矛盾」と
`plan_issue` を申告した。`max_replans` を使い切っていたので dispatcher は replan せず、task は `blocked`（質問）になった。
operator は回答で範囲の拡張を許したが、回答（`Trigger::Answer`）は `wu_dispatch_gate` の `resume_after_answer` で
WU を `blocked(plan_issue)` → `ready` に戻すだけで、計画に固定された check は変わらない。worker は 2 分後に同じ申告を
繰り返した。同日の WU sidebar（01M5HDSN3A）も同じ形。上限を使い切った後、人が check を直す手段が無かった。

## 決定
- D1: 人の回答に意図（`AnswerIntent`）を持たせる。`resume`（既定。今までの回答。WU を `ready` に戻し、回答は次の run の
  `context.answers` に載る）と `replan`（計画を直す）。`POST /tasks/{id}/answer` の本文に `intent` を足す
  （省略は `resume`）。CoS の代理回答（`answer_op`）も同じ検証・同じ events を書く。
- D2: `replan` は、task の active な計画に `blocked(plan_issue)` の WU があるときだけ受ける（無ければ 400 の検証
  エラー。計画を直す対象が無い）。回答と同じ transaction で `Answered` と
  `ExecutionHintSet{replan: true, note: "plan_issue への人の回答で計画を直す（WorkUnit <key>: <申告>）: <回答>", source: "<by> (plan_issue)"}`
  を書く。次の dispatch は既存の人の replan の経路（`pending_replan_request` → `RunPlanner{replan: true}`）で planner を
  起こし、planner はこの note を「人の指示」として受ける（`replan_reason`）。done の WU は今までどおり持ち越す。
- D3: 上限の扱い: 人の回答起点の replan は `max_replans` の**外**（`counted_replans` は `ExecutionHintSet{replan: true}`
  の次の版を数えない。decompose・計画の承認の replan と同じ）。別の上限は設けない: 人が 1 回答えるごとに 1 回なので
  回数は人が決める。木の節点の `max_tree_replans` は今どおり `tree_run_limit_hold` が見る（木の上限は木全体の資源の
  上限で、別の決定の要求になる）。
- D4: planner への入力は replan の通常の context（今の版・WU ごとの状態・持ち越す done の key）と D2 の note。
  「該当 unit の check・範囲だけを直す」は note の文で指示する（計画の形は planner が書き、採用の検証は今どおり）。
  WU の check を直接差し替える API は作らない: 人の回答は自然文（「範囲を広げてよい」）で、check の command や
  許可 path を人に書かせるより planner が回答から書く方が人の手間が小さく、計画の検証（`adopt_plan`）も通る。
- D5: 受信箱の質問（`InboxKind::Question`）は、task に `blocked(plan_issue)` の WU があれば選択肢を 2 つ出す:
  `answer`「回答して再開（計画はそのまま）」と `replan`「計画を直す（回答を planner への指示にして check・範囲を直す）」。
  推奨は `replan`（同じ check での再開は同じ申告を繰り返し得る）。`POST /inbox/{id}/answer` は `option` を `intent` に写して `POST /tasks/{id}/answer` へ渡す。`QuestionItem` に
  `plan_issue_units`（blocked(plan_issue) の WU の key）を足し、受信箱はそれで選択肢を決める。WU を読むのは events に `WorkUnitTransitioned{to: blocked, reason: "plan_issue"}` がある task だけ（受信箱の読み取りを task 数に比例させない。`request_lock_counts` の試験）。
- D6: 上限を使い切った後の `plan_issue` の質問文の末尾に「計画を直すなら受信箱で『計画を直す』を選ぶ」と添える
  （`resume` を選ぶと同じ申告が繰り返され得ることを人が判断できるように）。

## 影響
- 旧挙動（回答はいつも WU を `ready` に戻すだけ）は `resume` として残る（意図の既定値。互換分岐ではない）。
- `AnswerBody`・`QuestionItem` の schema が変わる（`UPDATE_SCHEMA=1` で再生成、web の型も再生成）。
