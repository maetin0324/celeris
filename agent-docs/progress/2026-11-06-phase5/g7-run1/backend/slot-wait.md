---
title: CoS の枠のみの待ちを Waiting で保持し期限で閉じる（WU slot-wait）
tasks: [01M6SQJF9YD3WPTD8NSB191PZW]
status: done
completed: 2026-11-07
---
# CoS の枠のみの待ち（WU slot-wait）

完了日: 2026-11-07。正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md) D2。新しい設計判断はなく、ADR は足していない。
`launch-and-wait.md` は reason unit の記録で、この WU は別ファイルに書く（並列 WU の記録は WU key ごと、ADR-0128 D3）。

## 変更（commit は件名で列挙）
- `phase 5: CoS の枠のみの待ちを Waiting で保持し期限で閉じる`

- `task-dispatch/dispatcher/cos_chat/slot_wait.rs`（新規）: `hold_slot_wait`（待ちに入れる。chat_run_status Waiting「CoS の枠の空き待ち」を 1 回だけ書く）、`retry_slot_waits`（tick ごとに run を読み直し、停止・期限・枠を処理）。待ちの正本は DB の run 行（Running・`launched_at` なし）で、メモリの表（`slot_waits`、run id → thread・route 起点）は走査対象を覚えるだけ。
- `cos_chat/routes.rs`（`launch_routes`）: `select_cos_route_from` が失敗し `previous` が無いとき `slot_only_wait(&failure, &trace)`（`RouteTrace` の構造化理由。文字列は見ない）が真なら失敗にせず待ちに入れる。偽なら従来どおり `route_notice` + `fail_routes`（route_unavailable）。
- `cos_chat/launch.rs`: `slot_waits` 欄、tick で `retry_routes` の後・新しい claim の前に `retry_slot_waits`、待ち中の thread の `run_configs` を保持。
- `cos_chat/control.rs`: 待ち中の run を orphan 回収の対象から外す（この daemon が所有者）。
- 待ちの動作: 次 tick で同じ run・同じ入力・同じ `first_route`（route 順の起点）で `launch_routes` を呼び直す。枠が空けば起動し、Working「CoS の枠が空いたので起動しました」を残す。枠以外の理由に変わったら従来どおり閉じる。待ち中は worker も route 予約も持たないので `cos_in_flight`・`implicit_account_in_use` に数えない。`max_cos_runs` に達している間、停止中・ディスク不足・設定無効の間は選び直さず待つ。
- 期限: 閉じる条件は `started_at` からの経過 ≥ run 開始時の `max_wall_secs`（`run_configs` の写し。後から設定を延ばしても延びない）。到達で `chat_run_finish_prelaunch(Failed, SlotWaitExpired)`。driver の共有期限（D3）は driver が `[cos] max_wall_secs` を残り以下にして守る側で、この WU は触らず、daemon の期限はそれ以上に延びない。
- 人の stop・interrupt: 待ち中の Stopping は `StoppedBeforeLaunch` で閉じる（stop は Stopped、interrupt は Interrupted）。
- 旧い仕組み: 枠待ちの旧経路は無く、消すものは無し。`routes.rs` の `supply_failure`（文字列）は起動後の失敗分類で、枠待ちの判定には使っていない。
- in_use_cos: `GET /providers` の `in_use_cos` 欄は provider 廃止で消えており、現行の同じ観測は `cos_in_flight()` と `implicit_account_in_use()`。どちらも worker か route 予約を持つ起動済みの run だけを数える。試験はこの 2 つで固定した。

## 修正前の回帰
試験だけを先に適用（`cos_slot_wait.rs` 4 本。実装は未着手）。
- コマンド: `cargo test -p task-dispatch --lib cos_slot_wait`
- exit 101、4 件中 3 件失敗（cooldown の 1 件は元から合格）:
  - `slot_only_wait_keeps_the_run_waiting_then_launches_the_same_run`: `claim stays` left Failed right Running（枠だけで失敗してしまう）
  - `slot_wait_expires_at_the_run_deadline_and_is_not_extended`: `29s < 30s` left Failed right Running
  - `stop_closes_a_waiting_run_before_launch`: left Failed right Stopped

## 修正後
- 同じコマンド: 4 passed, 0 failed。
- 受け入れ 0: 葉の worker が `p1` の唯一の枠を握る間、B の CoS run は Running・`launched_at` なし・Waiting「CoS の枠の空き待ち」が 1 件のまま 3 tick（新 run・失敗 run なし）。葉が終わると同じ run が起動して Working 記録→完了。
- 受け入れ 1: source の cooldown では待たず Failed、Waiting の記録なし、次 tick でも run は 1 本。
- 受け入れ 2: `max_wall_secs` を 10000 に reload しても 29 秒は Running、30 秒で Failed・`prelaunch_reason = slot_wait_expired`・`launched_at` なし。枠が空いても起動せず run は 1 本のまま。
- 受け入れ 3: 待機中 `cos_in_flight() == 0`、`implicit_account_in_use("p1") == 1`（葉のみ）。起動後 1 と 1。
- stop: 待ち中に stop すると Stopped・`stopped_before_launch`。
- 周辺: `cargo test -p task-dispatch --lib cos_chat` 210 passed。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh`: exit 0。3468 tests run, 3468 passed, 10 skipped（nextest）、`progress_index_commit_hashes` ok。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）の変更 path は task-dispatch の `cos_chat/`・`tests/` と、この進捗ファイルだけ。

## 未解決
- 既存の制約: `tick_cos_chat_launch` は `cos_chat_launch` を `take()` して走るため、その間 `route_state` は CoS chat 同士の route 予約を見ない。CoS 同士が同じ暗黙 account の枠を取り合う場合、枠が効かない（葉の worker・reviewer との重なりは効く）。この WU の受け入れ（worker・reviewer と CoS の重なり）の範囲外で、試験の握り手は葉にした。直すには `dispatcher.rs` か `route_select.rs` に予約を渡す変更が要り、この WU の範囲外。
- `CosReservation`（Claude の 5 時間枠予約）と `CosRunLimit` は route_select がまだ出さない（reason unit からの引き継ぎ）。`max_cos_runs` は `has_capacity` で、待ち中の run にも tick ごとに適用している。
- `launched_at` の書き込みと `prelaunch_reason` の他の値（route_unavailable 等）、起動時回収は launch-wire unit。この WU の試験は `launched_at` が空であることを確認するが、launch-wire の統合前は元々空なので、起動の印そのものは横断試験（cross）で確かめる。
- 再起動後の枠待ち run の回収（`daemon_restart_before_launch`）は launch-wire の起動時回収に任せ、横断試験で確かめる。

## 提案
- 統合時に、`routes.rs` の `launch_routes` の枠待ち分岐（`slot_only_wait` の直後）を launch-wire 側の `fail_routes` 変更と並べて確認する。
- CoS 同士の枠の取り合いを効かせるなら、launch tick の間も予約が見えるよう、`select_cos_route_from` に `route_reservations` を渡す変更を別 task にする。
