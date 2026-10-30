# ADR 2026-10-30: クラスタ job の待ち上限の認可に、案件 CoS が定型の代答で「延長」と答える

---
tasks: [01M655XC0MZBH3CB9TDWHW1Y20]
---

- 状態: 採用（実装済み）
- 関係: [2026-10-28-cos-routine-delegated-answers](2026-10-28-cos-routine-delegated-answers.md)（定型の代答。本 ADR が 7 種類目を足す。付記 A4）、
  [ADR-0090](0090-durable-wait-for-cluster-jobs.md)（クラスタ job の durable wait と上限での人への質問）、
  [2026-10-29-cos-review-rework-reopen](2026-10-29-cos-review-rework-reopen.md)（5 種類目。policy_version 7）、
  [2026-10-30-cos-plan-conforms-approve](2026-10-30-cos-plan-conforms-approve.md)（6 種類目 `plan_conforms`。policy_version 8）

## 背景

クラスタ job の durable wait（ADR-0090）が上限（例 28800 秒）に達すると、dispatcher（`time_out_cluster_job_wait`）は task を
`blocked` にしたまま「延長する（job を待ち直す）／job を取り消して続ける／取り下げる」を問う認可（authorization）を出す。
定型の代答のどれにも当たらないので、案件 CoS は人に回していた。例: BenchFS task 01M5XDTAQQ19D22E1N1TXP5QAD の Sirius job 48196 は
10-29 15:12Z から人待ちになり、案件が 6 時間以上止まった。

待ちの間 job は node を使わず（node-h を消費しない）、celeris は job を qdel しない。人は「job 待ちに関しては基本認可するしかないので、
そのようなケースも案件 CoS で自動対応するようにしてください」と指示した（統括 CoS thread 01M4B11RX2K4CMYDHGVEX0Q11D seq 4121、2026-10-30）。

## 決定

### D1. 定型の代答の 7 種類目 `job_wait_extend`

案件 CoS は、クラスタ job の待ち上限の認可に「延長」とだけ答えてよい。daemon が決定的に延長する案は採らない。
認可の本文と task の状況（同じ案件の別の判断・人の指示）を CoS が読み、記録・取り消しは既存の代答と同じ仕組み
（`cos_operations` の行と `CosOperation` event、reason「定型の代答（job_wait_extend）: …」、payload の `basis`、
チャットのカードからの `override`）に乗せるため。

### D2. API が決定的に検査する条件

`basis.category = "job_wait_extend"` の代答（`POST /cos/inbox/{i}/resolve` の outcome=answer、`/cos/operations` の
`approval.decide`・`inbox.answer`）は、次を全て満たすときだけ通る。外れれば 422 で監査に rejected として残り、CoS は escalate する。

| 条件 | 決定的な検査 | 外れたとき |
|---|---|---|
| (1) 待ち上限の到達による認可である | 対象は kind=authorization の待ち。その approval が未決で、task が一致する。task の events で、approval の question と同じ本文の最後の `QuestionRaised` の直前のクラスタ wait の event が `ClusterJobWaitFinished{state: timed_out}` で、その後に wait の event が無い | 422 `cos_basis_invalid` |
| (2) job がまだ待ち（Q）である | その wait の申告した job（`ClusterJobWaitStarted` の `jobs`）が全て、最後の poll（`ClusterJobWaitFinished.jobs`）で `queued`。`held`・`running`・`exiting`・`finished`・`gone`・poll に出ない（`unknown`）が 1 つでもあれば外れる | 422 `cos_basis_job_not_queued` |
| (3) 答えは延長だけ | `decision`（受信箱の `option`）が `once`、note（`approval.decide` では `answer`）が前後の空白を除いて「延長」と一致する。`standing`（恒久の認可の新設）・`denied`（取り下げ）・「取り消して続ける」・台数や資源の変更・別の確保は note が「延長」にならないので外れる | 422 `cos_basis_invalid` |
| (4) 対象に human_required が無い | 既存の `human_required_reason`（task の `human_required` label 等）と人の override 待ちの検査が先に効く | 403 `cos_human_required` |
| (5) 延長の回数の上限 | 同じ task・同じクラスタで、今の wait と job を 1 つ以上共有する wait の `timed_out` の数（今回を含む）が `JOB_WAIT_EXTEND_MAX` = 3 以下 | 422 `cos_basis_extend_limit` |
| 根拠の出典 | `refs` に `{"kind":"approval","id":"<答える approval の id>"}` がある | 422 `cos_basis_invalid` |

- 延長は台数・資源・walltime・node-h の枠を変えない。答えは次の run に渡り、run は同じ job を待ち直す（ADR-0090 の質問文のとおり）。
  延長の答えは新しい job の投入や確保を含まないので、`within_approved_frame` の枠の算術は要らない。
- 回数の上限 3 は、既定の上限 28800 秒なら同じ job を約 32 時間（4 回分）待った後に人へ回る量である。人と CoS のどちらが答えた延長も数える
  （人が延長した後の上限到達も 1 回と数える）。累計の待ち時間ではなく回数にしたのは、events から決定的に数えられ、上限の秒数を
  クラスタごとに変えても意味が変わらないため。
- (1) は質問の文言ではなく events の並び（timeout の `ClusterJobWaitFinished` の直後に出る `QuestionRaised`）で判定する。文言を変えても検査は変わらない。
- 他の category（例 `within_approved_frame`）で同じ認可に答える経路は変えない（その category の条件が別に効く）。

### D3. 人に回すもの

job が実行中・保留・終了・状態不明、同じ job の上限到達が 4 回目以降、job の取り消し・取り下げ・台数や資源の変更・別の確保への
切り替え、恒久の認可、`human_required` の付いた task。cos-inbox-triage skill の §2 の表と §3 (g) に書く。

### D4. policy_version 9

`cos-inbox-triage` skill を version 9 にし、`[cos.triage] policy_version` の既定を "9" にする。評価契約
（`docs/ops/mlsys-main-4-ops-r1.md` §8）に "8" → "9" を追記する。plan gate の自動承認（task 01M655SMDYDX81M6QF2N4YP5C1、
`plan_conforms`）が先に main に入って版 "8" と (f) を使ったので、本 ADR は後から入る変更として版 "9" と (g) を使う。

## 本番への移行

- 本番 config に `[cos.triage] policy_version` を書いている場合は "9" に書き換える（書いていなければ既定の "9" になる）。
- KB の skill 正本を差し替えている場合は `config/skills/cos-inbox-triage/SKILL.md` の版 9 に差し替える。
- 版が変わるので CoS の session は fresh で始まる（policy_changed）。

## 検証

- `crates/task-api/tests/cos_job_wait_extend.rs`（fake harness の CoS run credential、LLM なし）: Q の job の延長が通り approval が
  `once`・answer「延長」で決まり task が `ready` に戻る。`denied`・`standing`・取り消し・台数変更・approval の ref 欠け・他 category は 422、
  job が R・H は 422 `cos_basis_job_not_queued`、上限到達でない認可は 422、同じ job の 4 回目は 422 `cos_basis_extend_limit`、
  `human_required` label は 403。`/cos/operations` の `approval.decide` も同じ検査。
- `crates/task-api/src/cos/basis/job_wait_extend.rs` の単体試験（events からの判定と回数）。
