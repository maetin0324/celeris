# ADR: root 同士の depends_on は、依存先が既定 branch に取り込まれるまで待つ

- Date: 2026-10-17
- Status: 実装済み
- 決定者: 人の依頼（2026-10-17 Pairs 案件の障害）を受けた設計。細部はこの ADR が決める
- 関連: [task の待ちの理由](2026-10-11-task-wait-reasons.md)（M 系統の追加）、ADR-0121（delivery_skipped）、
  ADR 2026-10-11-stranded-done-branches（取り込み状態の判定）、[破壊的変更を既定とする](2026-10-11-breaking-changes-by-default.md)

## Context

2026-10-17 12:52:33、Pairs 案件（auto_integrate on）で PRA-03 が review_pass で `done` になった同じ秒に、
`depends_on` する PRA-05 が dispatch された。PRA-03 の成果はまだ main に無く（delivery は `department_unresolved` で skip）、
PRA-05 の worker は「依存の成果が main に無い」と質問して止まった。

`depends_on` の検査（`wait::db_waits` D5）は依存先の `status == done` しか見ない。木の内側（子が親 branch に入る）では
`done` が「親 branch に入った」を意味するが、別々の root 同士は既定 branch を介して成果を渡すので、`done` は成果の到着を意味しない。

## Decision

- **D1 対象**: 依存元・依存先がどちらも木の子でない task（root 同士）で、依存先が `done` のとき、依存先の branch
  `celeris/<task_id>` が repo の既定 branch の祖先になるまで依存元を `ready` のまま待たせる。木の内側の依存（どちらかが木の子）、
  対話、support task の依存先は対象外（従来どおり `done` で足りる）。
- **D2 判定**: git の祖先判定（`git merge-base --is-ancestor`）を dispatcher の tick で行う（DB だけでは決まらない事実。
  LLM 呼び出しは入れない）。repo は依存先の `repos`、無ければ案件の登録済み Local repo から branch の存在する repo を探す。
  branch がどの repo にも無い（repo を変えなかった）・Remote repo は取り込む物が無いので待たない。既定 branch を解決できない
  repo は「待つ」（理由に原因を出す）。祖先と分かった依存先は dispatcher がメモリに覚え、以後 git を読まない
  （既定 branch は前にしか進まない。daemon の再起動で忘れても結果は同じ）。
- **D3 待ち理由**: `TaskWait::DependsOnIntegration { task_id, cause, detail }`（M10。dispatcher のメモリの理由で、
  snapshot の `task_waits` と task 一覧の `waits` に載る）。`cause` は
  `unmerged`（取り込み待ち。delivery が進行中を含む）／`delivery_skipped`（delivery が取り込みを見送った。`detail` に
  `DeliverySkipReason` の code と説明）／`held`／`discarded`（人が取り込まないと決めた。人が `depends_on` を外すまで進まない）。
  依存先が複数あるときは task_id 順の最初の 1 件を出す。
- **D4 人への通知**: 依存先が `delivery_skipped` なら、その取り込み見送りの受信箱項目（ADR-0121 D3 の `DeliverySkipped`）の
  `summary` に、待っている依存元の task id を足す。新しい受信箱の種類は作らない（人が見る項目は 1 つのまま）。
- **D5 取り込み後**: sweep や delivery が既定 branch に merge すると次の tick で git が祖先と答え、依存元が dispatch される。
  待ちを解く別の状態遷移は無い（`ready` のまま）。
- **旧い挙動**: root 間でも `done` だけで dispatch する経路は残さない。切替 flag は置かない。

## 試験

temp DB と一時 git repo だけ（外部ネットワークなし）。未取り込みの依存先では dispatch されず待ち理由が載る → main へ merge すると
dispatch される。delivery_skipped では理由に skip の code が載り、受信箱項目に依存元が出る。
