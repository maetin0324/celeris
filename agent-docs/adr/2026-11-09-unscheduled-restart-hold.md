---
status: accepted
tasks: [01M6ZS10R7YCVNNE2A9EHJ68N1]
---
# 起動直後の停滞の判定は、dispatch の走査が 1 回終わるまで保留する（task_unscheduled の誤検知）

- 関係: ADR 2026-10-27-project-stagnation-inbox（D1 の `task_unscheduled`・D3 の判定の間隔）、
  ADR 2026-10-29-stagnation-triage-notify-wiring（D1・D2 の「判定が分からない間は保留」）、
  ADR 2026-10-11-task-wait-reasons（D4 のメモリの理由 M1〜M10）、ADR 2026-10-18-parent-waits-for-children、
  ADR 2026-10-17-root-dependency-waits-for-integration

## 背景（本番の事実。operator が本番 DB・log を読み取りで確認した）

1. 2026-11-09 09:05:40Z の promote（live handoff）の直後、健全性 v2 の親 task（子 9 件が未完。通常は
   `waits = [delegated_children]`）が `task_unscheduled` として受信箱に出た。約 1 分後の `GET /tasks/{id}` は
   `waits=[delegated_children]` を返し、件は消えた。
2. 同じ形が過去の promote の直後にもあった（`cos_inbox_items` の `source_key` が `task_unscheduled-*`）:
   11-05 22:44:09 → 22:44:52、11-06 09:34:09 → 09:35:00、11-07 18:23:59 → 18:24:21、11-09 09:05:40 → 直後。
   どれも子または依存を待つ親で、CoS の一次対応が「計画どおり待っている」として片付けた（毎回 CoS の run を使った）。

## 原因（コードで確かめた）

- `Dispatcher::tick`（`crates/task-dispatch/src/dispatcher.rs`）は `check_liveness_and_stagnation` を
  `dispatch_ready` より前に呼ぶ。
- `check_stagnation`（`dispatcher/tree_units.rs`）は `snapshot_task_waits()` のメモリの待ちを
  `MemoryWaits::from_waits` で `task_ops::stagnation::gather` に渡す。`gather` は `task_ops::wait::task_waits`
  （DB の待ち D1〜D13 + メモリの待ち）が空の ready の task を `waiting: false` にする。
- ready の task に付くメモリの待ちは、どれも `dispatch_ready` の中の `note_dispatch_wait` だけが書く
  （`dispatch_waits`。`dispatch_ready` の先頭で clear）:
  M1/M9 `Capacity`、M2 `Unroutable`、M3 `Cluster`、M4 `WriteSet`、M5 `Backoff`、M6 `DiskCritical`、
  M7 `DelegatedChildren`（`child_tasks.rs` の `parent_waits_for_children`）、M10 `DependsOnIntegration`
  （`task_ops::dep_integration::integration_wait`）。
- 起動直後の process では `dispatch_waits` が空で、`StagnationWatch.checked_at` が `None` なので、最初の tick の
  `check_stagnation` は必ず走る。この走査は `dispatch_ready` より前なので、メモリの待ちを持つ ready の task は
  「待ちの理由なし」と見え、最後の出来事から `task_unscheduled_secs`（既定 1800 秒）を過ぎていれば検知される。
  次の判定（`scan_interval_secs` = 60 秒後）ではメモリの待ちが揃うので消える。本番の「約 1 分で消えた」と合う。
- live handoff の新しい daemon は standby の間 `tick` を呼ばず（ADR-0040 D4）、active になった最初の tick で
  この順序に当たる。

## 案

- (a) 子の待ちを DB の事実から `task_ops::wait` で作る（M7 を DB の待ちに移す）。web の待ち表示も再起動に強くなる。
  - 子の待ちの判定 `task_ops::delegate::child_wait::awaited_children` は、失敗した子の扱いに CoS chat の store の
    `cos_triage_failure_escalated` を使う。`task_ops::wait::task_waits` は `TaskStore` しか受けないので、一覧・詳細・
    木の API と停滞の判定の全呼び出しに chat の store を渡す変更が要る。
  - 本番の件には「依存を待つ親」もある。M10 `DependsOnIntegration` は git の祖先の判定（`integration_wait`）で、
    API や停滞の判定から毎回呼ぶ重さではない。M1〜M6 は容量・経路・cluster・write-set・backoff・disk の
    その時の状態で、DB の事実から作れない。(a) だけでは M7 以外の誤検知が残る。
  - dispatcher の dispatch の門（`parent_waits_for_children`）と DB の待ちが同じ判定を 2 か所で持つことになる。
    正本を 1 つにするには dispatch の門も DB の待ちから引くよう作り直す必要があり、この task の範囲を超える。
- (b) 起動後に `dispatch_ready` が 1 回終わる（メモリの待ちが揃う）まで、停滞の判定を保留する。
  - M1〜M10 のどれにも効く。判定の関数（`gather`・`detect`）は変えない。
  - 保留の間、`StagnationWatch.available` は `false` のまま（`stagnation_snapshot()` は `None`）。ADR
    2026-10-29-stagnation-triage-notify-wiring D2 により、CoS の取り込み・退避・送信直前の照合は停滞 4 種を
    「分からない」として保留し、`resolved` にしない。再起動の前から開いていた正当な `task_unscheduled` の行も
    保留されるだけで、閉じて作り直されることはない。
- (b') `task_unscheduled` の事実（ready の task）だけを保留し、ほかの 3 種は最初の tick から判定する。
  判定を「成功」（`available = true`）にすると D3 の照合が 4 種すべてを照合するので、再起動の前から開いていた
  正当な `task_unscheduled` の行が一度 `resolved` になり、次の判定で同じ key の行が作り直される（CoS の run が増える）。
  これを避けるには種類ごとの「分からない」を snapshot と取り込みに足す必要があり、変更が広がる。
- (c) tick の中で `check_stagnation` を `dispatch_ready` の後へ動かす。順序に依る暗黙の前提になり、`dispatch_ready`
  が走らない tick（disk critical・draining）の最初の判定では同じ誤検知が残る。

## 決定

- D1（採用: b）: `StagnationWatch` に `waits_known`（この process で `dispatch_ready` が 1 回でも最後まで走ったか）を持つ。
  `tick` は `dispatch_ready` が `Ok` で返ったときに `true` にする（容量が無くて照合しなかった枝も、窓の分の
  `Capacity` を書いて返るので揃ったとみなす）。`false` に戻すことはない（process の寿命の間 1 回だけの保留）。
- D2（判定の時刻）: 停滞の判定を走らせるかは `task_ops::stagnation::scan_due(checked_at, scan_interval_secs,
  waits_known, now)` の純関数 1 か所で決める。`waits_known` が `false` なら走らせず、`checked_at` も進めない。
  `check_stagnation` の中にあった間隔の判定はこの関数に置き換えて消す。起動後の最初の判定は 2 回目の tick
  （`tick_ms` 後）になる。`project_stalled`・`release_unpromoted` の検知もその分だけ遅れるが、閾値は時間単位なので
  影響は無い。
- D3（メモリにしか無いほかの待ち）:
  - `dispatch_waits` の M1〜M7・M9・M10 はすべて D1 の保留で揃ってから判定に使われる。
  - `awaiting_children`（ADR-0016 D2 / M5 の review 後の子待ち）と `awaiting_human`（M8 `ReviewAwaitingHuman`）は
    `reviewing` の task にしか付かない。`task_unscheduled` は `ready` の task だけを見るので、この 2 つは誤検知を
    起こさない（`awaiting_children` は再起動で空になるが、それは review の再開側の問題でこの ADR の範囲外）。
  - `dispatch_ready` が走らない tick（`accepting_new_work = false`・disk critical）では、メモリの待ちは最後に
    走った `dispatch_ready` の値のまま（この ADR の前と同じ）。起動直後から disk critical が続くと、停滞の判定は
    disk が戻って最初の `dispatch_ready` が終わるまで保留になる。その間 snapshot は「分からない」で、行は保留される。
- D4（採らない: a・b'・c）: 理由は「案」に書いた。web の待ち表示（`MemoryWaits::from_snapshot`）は tick の後に
  公開される snapshot を読むので、起動直後の数秒以外は今どおりメモリの待ちを含む。この ADR では変えない。

## 結果

- 旧い経路: `check_stagnation` の中の間隔の判定（`checked_at` と `scan_interval_secs` の比較）を消し、
  `scan_due` に一本化した。config・API の欄は変えない。
- 本番 config の変更は要らない（`[maintenance.stagnation]` の欄は変えない）。
- 試験: `crates/task-dispatch/src/dispatcher/tests/unscheduled_restart.rs` の `unscheduled_restart_*`
  （注入した時計 `test_now`。再起動直後に子・依存の統合を待つ親が出ないこと、待ちの理由が無い ready の task は
  出ること）と、`task_ops::stagnation` の `scan_due` の単体試験。
- promote の後に operator が見る値: promote の直後（最初の 1〜2 分）に `cos_inbox_items` の `source_kind =
  'task_unscheduled'` の行が新しく出ないこと。出たら `GET /tasks/{id}` の `waits` が空かを見る（空なら正しい検知）。
