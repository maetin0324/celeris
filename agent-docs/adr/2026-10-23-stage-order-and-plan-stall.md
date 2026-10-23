# ADR 2026-10-23: 段の順位は未完の行で決める・行の無い依存を拒む・計画の詰まりを waits と受信箱に出す

## 背景
本番 2026-10-22T12:10Z〜10-23T04:30Z、moshi-brewed の子 task 01M5HDSN3AWKK4ZJ0T1K3QTC8X（/3、計画 01M5HHK8M0GFFYXRM67SS4XZ6P）が
16 時間 run 0 本で止まった。段の順は gap-a, gap-b, gap-c, gateway, screens, chat, close。integrate-gap-b が done になった直後、
gap-c の chat-port・diff-viewer・integrate-gap-c は pending のまま、close の parity-final だけが `dependency_ready` で ready に
なった。API の waits は []、受信箱にも warn log にも出なかった。人の replan（04:48Z、版 01M5KHDY52WA105SNTGAG2HW8D）でも同じ形が
再生産された。

### 原因
- 工程の障壁（ADR-0074 D1.3、`task_core::wait::earliest_unfinished_phase`）は段の順位を `phase_ranks`（段の**有効な行**の
  `seq` の最小値）で引いていた。replan は done の行の `seq` を書き換えない（R5b-fix1。done の行は前の版の並びのまま）。前の版で
  done になった close・chat などの段の行（parity-close・chat-view ほか）は、前の版の小さい `seq` を持ち続ける。新しい版で
  未完の gap-c の行は新しい版の並び（大きい `seq`）を持つ。結果、close の順位が gap-c より前になった。
- そのため parity-final は「前の段（gap-a, gap-b）は済んだ」と判定されて ready になり、chat-port・diff-viewer は「前の段 close に
  done でない行（parity-final）がある」と判定されて pending のまま。scheduler の現在の段（非終端の行の最小 `seq`）は gap-c で、
  gap-c には ready が無い。close の parity-final は現在の段でないので起こされない。互いに待つ deadlock。
- replan（`task_ops::execution::plan_replan`）も、採用時の ready の決め直し（`newly_ready(projected)`）を新しい版の並びに
  `seq` を書き換える**前**に行い、done の古い `seq` も同じ規則で数えたので、採用の時点で同じ形を作った（04:48Z の再生産）。
- 待ちの理由（`plan_waits` D8/D9）は「どこかに ready の unit がある」と動いているとみなして出さず、木の生存確認（ADR-0079 D10）も
  ready の unit を「走れる」と分類した。dispatcher は `settle_phase` の `Advance` → `runnable_in_phase` 空で Skip を返し続け、
  debug log しか残らなかった。

## 決定
- D1（段の順位）: `phase_ranks` は、段に done でない有効な行があればその行の `seq` の最小値、すべて done の段だけ従来どおり
  有効な行の最小値を順位にする。done の行の `seq` は前の版の並びのまま残る（R5b-fix1・replay は変えない）ので、
  順位の比較に使うのは「今の版の並びを持つ行」だけにする。工程の障壁・`unit_waits`・GUI の段の帯は同じ関数を使う。
  - 人の指示（2026-10-23T05:05Z「段の順は今の版の spec.stages の宣言順で決める。前の版の行は rank に入れない」）との関係:
    今の版の未完の行の `seq` は `materialized_order`（spec.stages の宣言順に段ごとに並べ、段の末尾に統合 WU を置く）の index
    （D2 で replan の採用時に書き直す）なので、未完の行がある段の順位は spec.stages の宣言順と一致する。前の版の行（done の
    繰り越し行）は、その段に未完の行がある限り順位に入らない。すべて done の段（今の版に行が無い gateway・screens・chat を含む）は
    前の版の `seq` で並ぶが、工程の障壁（`earliest_unfinished_phase`）は未完の行のある段しか見ないので待ちの判定には効かない。
    spec を引数に取らずに済むので、`newly_ready`・`unit_waits`・`runnable_work_units` の呼び出し側（dispatcher・replay・
    decision・tree_adopt）は変えない。
  - daemon が足した phase 付きの行（統合の repair WU）は replan で `seq` を書き換えない。段を並べ替える replan でだけ古い
    `seq` が段の順位を下げうる。現在の段に属すので、並べ替えが無い限り害は無い（既知の残り）。
- D2（replan の ready の決め直し）: `plan_replan` は、持ち越し・新規の未完の行の `seq` を新しい版の並び（`materialized_order`）に
  書き換えてから `newly_ready` で ready を決める。
- D3（行の無い依存）: 計画の unit の `depends_on` は、検証（`UnknownDependency`）で spec の中の key に限られる。そのうえで、
  採用（新規・replan）の時点で、未完の行の依存はすべて**有効な行**（superseded・cancelled 以外。版を問わない）に解決できなければ
  ならない。前の版で done の行（繰り越し unit。この版の `plan_id` を持たない行）は**満たされた依存**として扱う（行は前の版の
  `plan_id` のまま残り、`newly_ready` は版を問わず done の key を数える）。有効な行に解決できない依存（例: 行が superseded の
  key、daemon が足したが行の無い key）を持つ計画は `PlanValidationError::DependencyWithoutRow` で拒む（422）。拒むか満たすかの
  2 通りに統一し、「依存先の行が無いまま pending で待ち続ける」形は作らない。`child:<key>` の依存は子 Task で解くので対象外。
- D4（詰まりの待ちの理由）: `task_core::plan_stall(units, external_done, open_decision_keys)` を足す。走っている unit が無く、
  scheduler が起こせる unit（`runnable_work_units`）も上げられる unit（`newly_ready_with`）も無く、現在の段の統合も走れず、
  failed・blocked の unit・子待ち・未回答の決定も無いのに、非終端の unit が残っているとき `Some`。`plan_waits` はこのとき
  `TaskWait::PlanStalled{stage, unit_keys, detail}` を出す（`stage` は現在の段、`unit_keys` は現在の段の外で ready の unit と
  現在の段の pending の unit）。D8/D9 の `WorkUnits`（unit ごとの待ち）が出せる形（ready の unit が無い）なら続けて出す。本番の形は ready の unit があるので D8/D9 は出ず、`PlanStalled` だけが出る。
- D5（受信箱）: 木の生存確認（ADR-0079 D10）の節点の事実に `plan_stalled` を足し、`Some` なら他の unit の分類より先に
  `Unexplained{reason: "plan_stalled"}` にする。`liveness_timeout_secs`（既定 600 秒）続けば従来どおり `StallDetected` と
  障害通知を 1 回出す。受信箱の attention には `AttentionItem::Stalled` を足す: dispatcher が「最後の event が `StallDetected`
  の ready の task」を snapshot の `stalled_tasks` に載せ、`build_attention` がそこから項目を作る（task ごとに events を読まない）。
  何か起きて最後の event が変われば snapshot から消える。
  - 実装: `NodeLivenessFacts.plan_stalled`（`task_ops::tree::node_liveness_facts` が `plan_stall` の `detail` を入れる。
    `child:` の依存は unit の事実で名指しの待ちになるので空の集合で渡す）、`Dispatcher.stalled_tasks` と
    `DaemonSnapshot.stalled_tasks[]`（`SnapshotStalledTask{task_id, reason, detail, since, at}`。生存確認ごとに作り直し、
    最後の event が `StallDetected` の Unexplained の task だけを残す）、`AttentionItem::Stalled`、受信箱の kind `stalled`
    （`InboxKind::Stalled`。選択肢は replan・cancel で人の手を示すだけ、native なし。答えると 409 `native_action_required`。人の手は task 詳細の replan か取り消し）。
    `plan_stalled` 以外の理由（`nothing_runnable`・`replans_exhausted` など）の `StallDetected` も同じく受信箱に出る。
- 設定の追加・変更は無い（`[execution.tree].liveness_timeout_secs` の既定 600 秒をそのまま使う）。

## 結果
- 本番と同じ形（前の版で done の後段の行が小さい `seq` を持つ）の計画で、前の段 gap-c の unit が ready になり、close の
  parity-final は gap-c の統合まで pending のまま。replan の採用でも同じ。
- 計画の都合で dispatch されない task は waits に `plan_stalled` が出て、生存確認の時間を過ぎれば受信箱に出る。
