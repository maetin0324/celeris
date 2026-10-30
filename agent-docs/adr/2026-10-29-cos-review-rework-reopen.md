# ADR 2026-10-29: reviewer の技術的な差し戻しで failed になった task を CoS が定型の代答で開き直す

---
tasks: [01M63S650J8GS7GMJ587X3RXFS]
---

- 状態: 採用（実装済み）
- 関係: [2026-10-28-cos-routine-delegated-answers](2026-10-28-cos-routine-delegated-answers.md)（定型の代答 4 種類。本 ADR が 5 種類目を足す）、
  ADR-0044 D2（`POST /tasks/{id}/reopen`）、ADR-0006 D2（`context.prior_review`）、
  [2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md) D3（代答・取り消し・差し戻し）

## 背景

修正 task 01M6320TM5XCF2QA8XXTT57BRE は attempt 3 で実装と試験が通ったが、reviewer が「依頼範囲外の生成物 index.json の混入」で
差し戻し、task は failed になった。受信箱の失敗の件（選択肢 retry / reopen / cancel、推奨なし）は定型の代答 4 種類のどれにも当たらず、
CoS は人に回した。人は「開き直してください、こういうのも自動で対応できると思うのでそうしてください」と指示した（2026-10-29、chat seq 1616）。

## 決定

### D1. daemon の決定的な処理ではなく、CoS の定型の代答の 5 種類目 `review_rework` で開き直す

「差し戻しが技術的な不備で、直す内容が書いてあり、目的・受け入れ条件・仕様を変えない」かどうかは、reviewer の判定文
（`ReviewVerdict.reason`、自由文）を読まないと決まらない。reviewer の判定には差し戻しの種類を表す構造化した欄が無いので、
daemon は決定的に判定できない（daemon・store に LLM 呼び出しは入れない）。そこで:

- 判定（技術的な不備か）は CoS が行う。cos-inbox-triage skill（policy_version 7）に (e) の条件と、(e) にしない条件を書く。
- 判定以外の条件（下の D2）は API が決定的に検査し、外れれば 422 / 403 で拒む。CoS は拒まれたら escalate する。
- 記録と取り消しは既存の代答と同じ仕組みに乗る。通った開き直しは `cos_operations` の行と task の `CosOperation` event
  （reason「定型の代答（review_rework）: …」、payload に `basis`）に残り、受信箱の件は answered になり、CoS のチャットに
  代答のカードが出る。人はカードから `POST /cos/operations/{o}/override`（revoke / return）で取り消せる。

daemon が決定的に reopen する案は採らない。上の理由で「技術的な不備か」を判定できず、全ての reviewer 差し戻しを機械的に
開き直すと、目的の変更や範囲外の作業を求める差し戻しまで人を通さずに繰り返すため。

### D2. API が検査する条件

`basis.category = "review_rework"` の代答（`POST /cos/inbox/{i}/resolve` の outcome=answer、`/cos/operations` の `inbox.answer`・`task.reopen`）は次を全て満たすときだけ通る。

| 条件 | 外れたとき |
|---|---|
| 対象は失敗（kind=failed）の受信箱の件で、答えは `reopen` | 422 `cos_basis_invalid` |
| task の最後の worker run に、reviewer の判定（`reviewer(<reviewer run id>): …` の形の `ReviewVerdict{pass:false}`）があり、その後に reopen が無い。command check・`reviewer infra failure`・予算切れ等で落ちた task は当たらない | 422 `cos_basis_invalid` |
| `refs` に `{"kind":"review","id":"<reviewer run id>"}` が 1 件以上あり、どれも上の差し戻した reviewer run を指す | 422 `cos_basis_invalid` |
| task の受け入れ条件に human check（`Check::Human`）が無い | 403 `cos_human_required` |
| task に `human_required` の label が無い（既存の検査） | 403 `cos_human_required` |
| 同じ task で CoS が `review_rework` で開き直した回数（task の events の `CosOperation{state:"applied", target_kind:"task"}` で reason が「定型の代答（review_rework）」で始まるもの）が 2 回未満 | 422 `cos_basis_rework_limit` |
| 今回の差し戻し文（reviewer の prefix を除き空白を詰めた、落ちた条件の文の集合）が、前に CoS が開き直したときの差し戻し文と同じでない | 422 `cos_basis_repeated_failure` |

- 上限は 2 回（`REVIEW_REWORK_MAX`）。人が自分で開き直した回は数えない。人が代答を取り消しても回数は戻らない（保守側）。
- 失敗の件への `reopen` を他の category で答えると 422 `cos_basis_invalid`。
- 既存の拒否（human override 待ち、revision、confidence、policy_version）はそのまま先に効く。

### D3. `/cos/operations` の `task.reopen` も待ちへの回答として扱う

失敗の件の native の route は `retry` なので、`POST /api/v1/tasks/{id}/reopen` を直接呼ぶと従来は `basis` の検査を通らなかった。
受信箱に開いている失敗の件がある task への `task.reopen` は、その件への `reopen` の回答として扱い、`instructed_by` が無ければ
`basis` 必須（D2 の検査）とする。受信箱に件が無い task（done の task、24 時間より前の失敗）は従来どおり domain の検査だけ。
人の指示を取り次ぐ（`instructed_by`）開き直しは人の判断なので `basis` は要らない。

### D4. 差し戻し文は次の attempt に渡る

reopen は task を ready に戻し attempts を 0 にする（ADR-0044 D2）。次の worker run の `context.prior_review` は、events の
最後の `ReviewVerdict` の run（＝差し戻された run）の判定から作る（`task_ops::derive::prior_review_from_events`）ので、差し戻し文は
新しい仕組みを足さずに「前回の判定」として prompt に載る。CoS の note を別に写さない。

### D5. policy_version 7

`cos-inbox-triage` の `metadata.version` と `[cos.triage] policy_version` の既定を "7" にし、評価契約
（`docs/ops/mlsys-main-4-ops-r1.md` §8）に "6" → "7" を追記する。`[cos.triage] human_required` の分類は変えない
（(e) にしない条件は既存の `objective_change`・`secret_or_security`・`external_publish`・`destructive`・`beyond_frame`・`explicit_human` に当たる）。

## 本番への移行

- 本番 config に `[cos.triage] policy_version = "6"` を明示していれば "7" に書き換える（未設定なら既定の "7"）。
- 本番の KB に `cos-inbox-triage` skill の写しを置いている場合は、この版の `config/skills/cos-inbox-triage/` に差し替える。

## 検証

- `crates/task-api/tests/cos_review_rework.rs`（fake harness の CoS run credential で API を呼ぶ）:
  開き直しが通り監査と prior_review に差し戻し文が残る・人が取り消せる、同じ差し戻し文で 422・2 回の後の 3 回目で 422 と escalate、
  human check・`human_required` label で 403、command check・infra の失敗で 422、`/cos/operations` の直接の `task.reopen` が `basis` を要る。
- `crates/task-api/src/cos/basis/review_rework.rs` の単体試験（差し戻しの読み取り）。
