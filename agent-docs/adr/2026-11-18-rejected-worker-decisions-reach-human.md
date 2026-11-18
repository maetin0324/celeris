---
tasks: [01M7RFVV60RQTDKYEWAEH4YYGT]
---
# ADR 2026-11-18: 受け付けられない worker の決定の要求は 1 件の決定にまとめて人に出し、その unit を止める

- 日付: 2026-11-18
- 状態: Accepted
- 関係: [ADR-0079](0079-recursive-task-decomposition.md) D7・§7 R3a 11.（worker の決定）。この ADR は R3a 11. の
  「捨てた要素は進行の 1 行に理由を残し、run は失敗させない」を置き換える。束ね（上限を超えた分の `bundle`）と、正しく受け付けた
  要求の扱いは変えない。
- task: 01M7RFVV60RQTDKYEWAEH4YYGT

## 事象

task 01M7Q5ZABN9NFQCH6QBTTP3EZ8（アーキ v4 E の子）の WorkUnit `target-leaf` の run が `result.json` の `decisions` で人の確認
（key `work-units-size-trim`）を求め、`needed_before` に `integrate-impl`（段階 impl の integrate の unit）を書いた。daemon は
「needed_before names an unknown unit "integrate-impl"」で要求を捨て、events に `worker_progress` の 1 行（seq 436、
2026-11-18T14:46:28Z）を残しただけだった。decisions の行も受信箱の項目も作られず、run は成功のまま進み、task は done・本番まで
進んだ。worker の報告は「人の確認を求めた」と書いたままで、人は事後に知った。

## 原因

`Dispatcher::worker_decisions`（`crates/task-dispatch/src/dispatcher/tree_units.rs`）は
`task_core::decision::prepare_worker_decisions` の `rejected`（理由の文字列）を `worker_progress` にするだけで、run の結果にも
決定にも反映しなかった（ADR-0079 R3a 11. の規則どおり）。`worker_progress` は受信箱にも通知にも出ず、worker の次の run にも届かない。
`needed_before` に書ける unit から Integrate の unit を除いているのは正しい（integrate は daemon が作る unit で、決定で止める対象に
しない。R3a の他の unit を止める経路も Integrate を飛ばす）。誤りは「拒否した要求を黙って捨てる」ことにある。

## 決定

- D1（(a) を選ぶ）: 受け付けられなかった要求は、1 run につき 1 件の `choice` の決定にまとめて記録する（`DecisionRequested`。
  派生の表 `decisions` に行ができ、受信箱の `decisions` 節と通知に出る）。
  - key: `rejected`（この節点で使われていれば `rejected-2`, `rejected-3`, …。束ねの `bundle` と同じ付け方）。
  - 問い: 受け付けなかった件数と、各要求の 1 行（元の key と問い・選択肢・推奨。読めなかった要素は位置 `#n`）と拒否の理由。
    元の要求の 1 行の書式は束ねと同じ関数で作る。
  - 選択肢: 束ねと同じ key（`all-recommended`〈推奨〉/ `see-note`）。新しい kind・新しい効き目は足さない（答えの効き目は choice の
    `resume`）。
  - `needed_before`: `self`（leaf の run ならその leaf の key に書き換える。atomic の run は `self` のまま節点の止め）。
    worker が何の前に答えを要るとしたかを daemon は確定できないので、要求を出した仕事そのものを止める。leaf は done にならず
    `blocked(decision)` になり、その leaf を待つ unit（同じ段階の integrate を含む）も進まない。答えると leaf がもう一度走り、
    前置きの「人の決定」節に答えの 1 行（問いに元の問いと理由が入る）が載る。worker はここで要求の誤りと人の答えを知る。
  - 後戻りの大きさ: 読めた要求の最大。読めなかった要求が 1 件でもあれば `high`（大きさが分からないので大きい方に倒す）。
  - 未回答の上限（`max_open_decisions`）は数えない。束ねと同じく 1 件は必ず出す。1 run で増えるのは束ねの 1 件とこの 1 件まで。
  - 進行の 1 行（`worker の決定の要求を記録できませんでした（ADR-0079 D7）: <理由>`）は今までどおり残す。
- D2（(b) を選ばない理由）: run の完了を拒んで worker に書き直させる方式は、run の決着（`run_settlement`・`worker_finish.rs`。
  高 risk の path class）に新しい失敗の種類を足す。試行の上限を使い切ると unit が失敗して人に届くが、それまで人は何も知らず、
  worker が同じ誤りを繰り返せば試行の分だけ予算を使う。(a) は既存の決定の経路（記録・受信箱・通知・答えの流れ・`self` の止め）
  だけで人に届き、答えの後の run で worker にも届くので、(c) を別に作らない。
- D3（重複の扱い）: key の重複で拒否した要求は、同じ key で記録済み（またはこの run の中で先に受け付けた）要求と**問いが同じなら**
  人に出さない（同じ要求の読み直し。R3a 11. の「同じ `result.json` を読み直しても二重に出ない」を保つ）。問いが違えば別の要求なので
  D1 の決定に入れる。D1 の決定そのものも、同じ問いの決定がこの節点に既にあれば出し直さない（古い `result.json` を読み直したとき）。
  このため `prepare_worker_decisions` の `taken` は key の集合から「key → 記録済みの問い」の表に変える。
- D4（`needed_before` を読み替えない）: integrate の unit 名を `stage:<key>` や `self` に読み替えることは検討したが、しない。
  `stage:<key>` は段階の pending / ready の unit だけを止め、Integrate も要求を出した leaf 自身も止めないので、事象の
  `integrate-impl` → `stage:impl` は意図（integrate の前に答える）を満たさない。`self` への読み替えは leaf と integrate の
  依存関係によって意味が変わり、一通りに決まらない。D1 の `self` の止めが常に意図より早く止まる側に倒れるので、読み替えの規則は
  置かない。拒否の理由は今の文言（`use self, a unit key of the plan or stage:<key>`）のまま D1 の問いに入る。
- D5（範囲）: `followups.json` の形の誤り（2026-11-16、01M7H1RNG5）は直さない。

## 試験

- `task_core::decision` の単体試験 `rejected_worker_decisions_become_one_self_decision`: 本番と同じ入力（`needed_before` に
  integrate の unit 名）と、形の誤り・読めない要素・問いの違う key の重複が 1 件の `rejected` にまとまり、`needed_before` が
  `self`（leaf の key）になること。問いが同じ key の重複は入らないこと。同じ問いの決定が記録済みなら出さないこと。
- dispatcher の試験 `rejected_worker_decision_reaches_the_inbox_and_holds_the_leaf`（`dispatcher/tests/tree_decisions.rs`）:
  leaf が `needed_before: ["integrate-s1"]` の要求を出すと、decisions に `rejected` の行（open・origin worker・
  `needed_before = [leaf]`、問いに元の問いと理由）ができ、受信箱の未回答の決定（`task_ops::decision::open_rows`）に出て、
  leaf は `blocked(decision)`・integrate は done にならない。答えると leaf が走り直し、前置きに答えが入り、木が終わる。
- 既存の `worker_decisions_are_validated_and_self_is_resolved`・`worker_decisions_beyond_the_cap_are_bundled` の期待値は
  変えない（`accepted`・`rejected`・`bundled` は今までどおり）。`worker_declared_decisions` は形の誤りの要素
  （`{"key":"broken"}`）を「捨てられる」と確かめていた。この契約は D1 で廃止したので、その要素と確かめを外し、
  新しい期待値は上の dispatcher の試験で確かめる。
