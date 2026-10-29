# ADR 2026-10-29: 停滞の項目を CoS 一次対応と送信直前の照合に接続する

## 背景
ADR 2026-10-27-project-stagnation-inbox は停滞 4 種（`project_stalled`・`human_wait_aging`・`release_unpromoted`・
`task_unscheduled`）を daemon の snapshot（`DaemonSnapshot.stagnation`・`stagnation_policy`）から受信箱に出す。
一方、CoS 一次対応の取り込み（`task-dispatch` の `cos_chat/triage.rs` の `human_inbox`）と、通知の送信直前の照合
（`celeris::notify::triage` の `source_unresolved`・`withdraw_if_resolved`）は `human_inbox(store, None, …)` と
snapshot なしで組んでいた。そのため次の 3 つが起きていた。

- 停滞 4 種は CoS の取り込みに入らない（`GET /inbox/items` には出るのに CoS は知らない）。
- 停滞は event に結び付かない（「何も起きない」が停滞）ので、event 起点の取り込み（`triage_events`）では拾えない。
- 仮に行があっても、送信直前の照合と CoS 不在の退避（`fallback.rs`）は snapshot が無いので「元の待ちが閉じた」と
  判定し、通知を撤回して item を `resolved` にする。

定期の `cos_review`（ADR 2026-10-28 review 裏 thread）は別の経路で停滞を受け取っていて、この ADR では変えない。

## 決定
- D1（停滞だけの snapshot）: dispatcher は停滞の判定の結果から、停滞の欄だけを埋めた `DaemonSnapshot`
  （`DaemonSnapshot::stagnation_only`）を返す `Dispatcher::stagnation_snapshot()` を持つ。判定が一度も成功していない・
  最後の判定が失敗した・verify mode のときは `None`（「分からない」）。CoS の取り込み・CoS 不在の退避・relay の再判定・
  送信直前の照合はこれを `human_inbox` に渡す。他の snapshot 由来の種類（`unroutable`・`stalled`・
  `review_resync_halted`）はこの ADR の範囲外で、今までどおり CoS 経路には入れない。
- D2（snapshot が要る種類）: `task_ops::human_inbox::STAGNATION_KINDS`（停滞 4 種）は snapshot が無いと判定できない。
  snapshot が `None` のとき、この 4 種は「解消」と扱わない。
  - CoS の取り込み（起動時の照合・event 起点の stale の掃除）は 4 種の行を `resolved` にしない（保留）。
  - CoS 不在の退避は 4 種の元が一覧に無くても `resolved` にせず、次の pass に回す。
  - 送信直前の照合は `SourceCheck::Unknown` を返し、`withdraw_if_resolved` は通知を撤回も送信もせず
    `Sendable::Held` を返す（行は pending のまま次の判定で照合し直す）。tick は warn の log を残す。
- D3（停滞の取り込み）: CoS の取り込みは、停滞の判定が新しくなるたび（`StagnationWatch.generation` が進むたび。
  間隔は `[maintenance.stagnation] scan_interval_secs`）に 4 種それぞれを `cos_triage_reconcile` で照合する。
  挿入は `(source_kind, source_key, source_revision)` の一意制約で冪等、現在の一覧に無い行（解消・改訂で
  revision が変わった旧行）は `resolved` になる。同じ generation では照合し直さない。
- D4（変えないもの）: CoS の権限・自動回答・監査・idempotency・通知の制限（1 tick 1 通・間隔・`cos_triage_outbox_sendable`
  の撤回規則）は変えない。停滞の項目は受信箱では答えられない（409 `native_action_required`）ままで、CoS は
  observe か escalate しかできない。

## 結果
- 停滞 4 種は通常の CoS 一次対応で pending になり、escalate すれば送信直前の照合で生きていると判定されて送られる。
  解消・改訂すれば旧行は `resolved` になり、旧い通知は送信直前に撤回される。
- snapshot が無い間、停滞の通知は送られず撤回もされない。次に判定が成功した tick で照合し直す。
