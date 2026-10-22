# ADR 2026-10-22: 進展の無い自動 replan を止める（同じ WU・同じ失敗の繰り返し）

## 背景
本番 2026-10-22、moshi-brewed の task 01M5HV2YTM で、段の統合 WU integrate-kcore の検査
（`./gradlew --offline :core:test`）が毎回同じ compile error（`RealSshdSessionTest.kt:185 Unresolved reference 'destroy'`。
原因は task root の古い uniffi binding で、計画では直らない環境の問題）で落ちた。13:24〜13:30Z の約 6.5 分で自動 replan が
19 回（v3〜v21、1 周約 20 秒、worker の作業なし）続き、`max_replans`（20）を使い切った。planner は毎回 done の unit kt-core の
check の書き方（`bash -c` の包み・`set -e`・`JAVA_HOME` の export）を変えるだけだった。後の正当な replan（最終レビューの
不合格）の分が残らず、人の決定に回った。

経路: `classify_review_failure` が compile error を `Substantive` にし、repair WU を作らず `integration_gives_up` へ。そこは
`counted_replans < effective_max_replans` だけを見て `Continue{Replan}` を返す。planner の replan は done の unit の
`checks` だけを変えられ（R7-3 D1）、採用で統合 WU は `pending` に戻ってすぐ同じ統合が走る。前の版との進展を比べる仕組みが
どこにも無かった。

## 決定
- D1（記録）: daemon が自動 replan（`Continue{Replan}`）を起こすたびに `Event::AutoReplanRequested{key, failure, source}` を
  同じ transaction で積む。`key` は失敗した WU（統合 WU を含む）の key、`failure` は D2 の失敗の識別子、`source` は
  `integration`（段の統合の検査の失敗）か `work_unit`（WU の run の失敗・no progress・plan_issue、兄弟待ちの後の判定）。
  状態は変えない（`replay` は無視する）。使う所は `integration_gives_up`・`finish_worker_result` の WU の replan・
  `deferred_work_unit_trigger` の 3 か所。
- D2（識別子）: `task_ops::plan_gate::failure_signature(cmds, output)`。
  - cmd の正規化（`normalize_check_cmd`）: `bash|sh [-l|-e|-c の組] -c '<inner>'` の包みを外し（入れ子も）、`&&`/`;` で区切った
    前置きのうち `set -…`・`export VAR=…`・`VAR=val`（代入だけの段）を捨て、`env VAR=val … cmd` と `VAR=val cmd` の代入の
    前置きを外し、空白を 1 つに詰める。`cd` は意味を変えるので残す。
  - output からは、行頭（空白を除く）が `e: ` か `error:` の行と、`error[` を含む行だけを取る（Kotlin の `e: file://…`、rustc の
    `error[E…]` / `error: could not compile`）。時刻・所要時間の行は入らない。
  - 正規化した cmd（昇順・重複除去）と取り出した行（出現順）を FNV-1a 64 で hash し、16 桁の hex にする。外部の crate は使わない。
  - 取り出せる行が無いときは cmd だけの hash になる（間に commit が無ければ、同じ check の不合格はコードが同じなので同じ失敗とみる）。
- D3（進展）: 2 つの自動 replan の間の「葉の進展」は `WorkUnitCommitted` のうち `commit` が `base` と違う（変更があった）もの。
  WU の run・repair WU の run のどちらでもよい。人の replan の依頼（`ExecutionHintSet{replan: true}`、遷移 `plan_replan` /
  `phase_replan` / `answer`）は連なりを切る（人が手を入れた後は数え直す）。
- D4（回数）: `plan_gate::stalled_replans(events, key, failure)` は、末尾から見て同じ `(key, failure)` の自動 replan が進展なしで
  何回続いたかを返す。`STALLED_REPLAN_LIMIT = 2` に達したら（1 回目の失敗で replan、同じ失敗の 2 回目でもう 1 回 replan、
  3 回目で止める。`counted_replans` は 2 以下）自動 replan をやめる。`max_replans` に余りがあっても止める。
- D5（行き先）: 止めたときは人への質問（`WorkerQuestion`。木の節点でも決定ではなく質問。止めた理由は上限ではなく
  「同じ失敗が繰り返した」なので `limit:max_replans` の決定の選択肢〈上限を上げる〉は合わない）。文の頭は
  `REPLAN_STALLED_QUESTION_PREFIX`（「同じ失敗で進展の無い replan が続いたので自動 replan を止めました」）。回答は
  上限を使い切った後の質問と同じく人の replan の依頼として扱い、上限に数えない（`answered_replan_exhausted` が両方の頭を
  認める）。統合の失敗では統合 WU を `blocked(question)` にし、`QuestionRaised` に本文を残す（上限切れと同じ形）。
- D6（compile error の分類）: `classify_command` は merge-base・fmt・lint の cmd の判定の後、test の cmd の判定（失敗数）より先に reason（stdout/stderr の tail）を見て、compile error
  （行頭の `e: file://`、`Compilation error`、`error[E`、`error: could not compile`）なら新しい `RepairClass::Build`
  （bucket `build`、予算 30 turn / 1800 秒）にする。統合の検査の失敗は repair WU（Task の worktree）になり、`max_repairs`・
  同じ class の上限は今どおり。fmt・lint の cmd（`cargo clippy` の失敗の出力にも `error: could not compile` が出る）は今どおり Format・Lint。最終レビューの決定的 check の compile error も同じ分類で repair になる。repair が直せない
  （環境の問題）なら repair WU は `plan_issue` / 失敗で終わる。repair WU の key は回ごとに違う（`repair-<phase>-<n>`）ので D4 の連なりにはならないが、回数は `max_repairs`・同じ class の上限（既定 2）で止まり、その後の統合の失敗は D4 / D7 で止まる（自動 replan は高々 repair の数 + 1）。
- D7（done の check だけの計画）: 直前の自動 replan が `source: integration` で、その後に人の依頼が無いとき、planner の
  replan（`origin: planner`）が done の unit の `checks` しか変えない（done の unit の checks を前の版に戻すと、unit・段階が
  前の版と同じ）計画は `plan_replan` が検証エラーで拒む（「進展なし」）。拒んだ計画は既存の planner の不正な計画と同じ扱い
  （1 回だけ再試行し、2 回目は質問・木の節点は `plan_invalid` の決定）。人の replan（`ExecutionHintSet{replan: true}` の後、
  origin human）は今どおり done の check を直せる。planner の prompt（replan 節）に「統合の失敗の replan で done の check
  だけを変える計画は拒まれる。新しい葉か repair を足す」と書く。
- D8（表示）: 木の上限の決定文（`limit_decision`）は `count == max` のとき「使い切りました（{count}/{max}）」と書く
  （以前は「{count} > 上限 {max}」で、使い切りを超過と読めた）。`count > max` は今どおり。
- 人の `intent=replan`（ADR 2026-10-22-plan-issue-answer-replans）の replan を上限に数えない挙動は変えない。

## 影響
- `Event` に `AutoReplanRequested` が増える（events の種類名は `auto_replan_requested`。API の schema（docs/api/v1・web/api/generated）に出るので、web の realtime の種類一覧 `web/api/realtime/event-kinds.ts` と無効化表 `invalidation-map.ts`（timeline と実行の欄 `T`・`E`）にも足す）。
- `RepairClass` に `Build` が増える（`RepairScheduled.class` に `build` が出得る）。
- 旧い挙動（同じ失敗でも `max_replans` まで回し続ける）は残さない。
