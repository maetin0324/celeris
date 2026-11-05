# ADR 2026-11-05: 第 4 フェーズ P4-C — CoS 一次対応の受信箱を「触れた task の分だけ」作る

- 状態: 採用。
- 根拠: `agent-docs/progress/2026-11-05-phase4/p4c.md`、raw は `agent-docs/progress/2026-11-05-phase4/p4c/`。前提は [P4-A](2026-11-05-phase4-p4a-baseline-contract.md) の D1 G4・D2・D3。

## 文脈
P4-A の計測で、blocked の task の数に比例する dispatch の遅延の大半は `tick_cos_chat_launch` の中の
`triage_events` と `triage_fallback` が、`task_ops::human_inbox::human_inbox`（全件の受信箱）を 1 tick に最大 2 回作り直すことだった。
全件の組み立ては blocked の task ごとに event を 4 回（質問・途中確認・browser の前提・計画の承認）読み、`store.list` を 2 回呼ぶ。

## 決定
- D1 `task_ops::human_inbox::human_inbox_touching(store, snapshot, ctx, now, evidence, touching)` を足す。返す項目は
  全件の受信箱から「`InboxItem::task_ids()`（`task`・`blocking.root`・`blocking.tasks`）が `touching` と交わる項目」を同じ並びで
  抜いたものと同じ。task ごとの走査（blocked・failed）は「自身か root（`tree::root_id_of`）が `touching` にある task」だけを読み、
  受信箱に出ない `requeue_limit_near` と task に触れないクラスタの項目の走査は飛ばす。同値は試験
  `human_inbox_touching_equals_full_inbox_filtered_by_touched_tasks` が固定する。
- D2 `triage_events` は page ごとにその page で触れた task の `human_inbox_touching` を使う（以前も触れた task の項目しか使っていない:
  直送・`cos_triage_resolve_stale_touched`・`sources_for_touched`）。`resolve_stale_touched` は key に触れた task id を含む行だけを見るので、
  その行の項目は触れた task に触れ、抜いた受信箱に必ず入る。
- D3 `triage_fallback` は退避の候補の key（`item_task_id`）が指す task の `human_inbox_touching` で項目を引き、無いときだけ全件を作って
  引き直す（項目が閉じたかどうかの判定は全件で行う）。
- D4 全件の組み立て（`inbox::inbox`）も、blocked の task の 3 種の attention を 1 回の event の読みで判定する（並びは以前と同じ: 種類ごとの列を順に足す）。
  `human_inbox` は `inbox_parts` が返す task の一覧を使い、`store.list` を読み直さない。
- D5 起動の照合（`triage_reconcile`）・停滞の照合（`triage_stagnation`）・relay の再判定・API の `GET /inbox/items`・通知は全件のまま。
  切替の flag は置かない（旧い経路を残さない）。

## 守ること
受信箱の項目の内容・並び・CoS の一次対応の行（挿入・resolved）・退避の本文は変えない。human gate・wait・depends_on・lease・
scheduler には触れない。migration は使わない（予約 0116 は未使用）。

## 残り
- `store.list(None)` は 1 回の組み立てに 1 回残り、件数に比例する（600 件 37 ms、2,000 件 約 120〜200 ms、10,000 件 約 1.1 s、debug build）。
  10,000 件で dispatch の遅延 p50 は 3.3〜4.0 秒。1 tick で 1 回の task の一覧を共有するか、範囲つきの一覧の query に寄せるのが次の手（提案）。
