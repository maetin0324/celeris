---
title: pilot が CoS の終了と usage 永続化を待ち、期限超過を unknown にする（pilot-drain）
tasks: [01M6TER3DG6Z9EXM9PN8X9GRJD]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# pilot-drain

完了日: 2026-11-07。正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md)（D3・D5-3・D5-4）、
付記 [2026-11-07-pilot-drain-shared-deadline](../../../adr/2026-11-07-pilot-drain-shared-deadline.md)（承認の wall は総量）。
実 LLM・外部ネットワーク・本番操作・全 suite・web e2e は実行していない。基準は `git merge-base HEAD main`。「修正前」は task の base の版（`05522ee4`）の driver・judge・stub。

## 変更
- `driver.py`: 共有の期限（`--approval-started-at`・`--approval-wall-secs`・`--approval-expires-at`。`wait_end`・drain・`[cos] max_wall_secs` が使う）、
  最後の操作の後・SIGTERM の前の drain、facts の `drain`、CoS invocation の `launch`・`prelaunch_reason`・`slot_wait_secs`・`drain_timeout`、
  `usage.missing` の `cos_drain_timeout`。起動前に終わった CoS run は `cos_usage_null` に数えない。
- `pilot_drain.py`（新規）: 注入できる時計の drain、起動状態の判定（backend の `launch_tracked`・`launched_at`・`prelaunch_reason` を DB で読む。列の無い旧 DB は unrecorded）、期限の計算。
- `pilot_gate.py`: `cos_runs` は起動した CoS run だけ、`cos_not_launched`・`cos_drain_timeout`、`drain` 検査（期限超過・wall 上限到達で unknown）。g7 pass は cos_usage_null・cos_drain_timeout・cos_not_launched がすべて 0 のときだけ。欄の無い旧 raw は unrecorded（起動済み）として読む。
- `long-horizon-pilot-llm.sh`: 承認の開始時刻と総量を全試行の driver に渡す（`--total-wall-secs`）。config-stub の試験用の絞り（`LONG_HORIZON_PILOT_STUB_COS_DELAY`・`_WORKER_DELAY`・`_MAX_RUNS`）。`stub_harness.py`: `--cos-delay-secs`・`--worker-delay-secs`。
- 試験: `test_pilot_drain.py`（新規、注入時計）、`test_driver_usage.py`・`test_pilot_gate.py` に追加、`long-horizon-pilot/test-pilot-drain.sh`（新規、実 daemon の config-stub）。`test-long-horizon-pilot.sh` の末尾から呼ぶ。

## 受け入れ条件ごとの証拠
- 条件 0: `python3 -m unittest discover -s scripts/dev/long-horizon-pilot -p "test_*.py"` は 91 件 OK（下の最終検査の節を参照）。
  新規は drain の 14 件（遅い CoS が期限直前に終わる・usage 永続化の遅れ・期限の境界・待機中に生まれる run・期限超過後の run・複数試行の残り時間）と、
  usage 集計 5 件（起動前 run は欠測でない・起動後 null は欠測・旧行と列挙外 reason は欠測・drain timeout・列の無い旧 DB）、judge 7 件（旧 raw の判定が変わらない・config-stub は pending）。
- 条件 1: `bash scripts/dev/long-horizon-pilot/test-pilot-drain.sh --out <dir> --before-rev 05522ee4`（exit 0。LLM 0、実 daemon・一時 DB、約 8 分）。出力:
  - (1) 遅い CoS（返事 20 秒）: `drain waited 86.2 s; every CoS run has usage; g7 pending`。daemon を止める前に待ち、`usage.missing` は null。
  - (2) 供給元の `max_runs = 1`・worker を 5 秒遅らせる: `roles {'worker': 4, 'reviewer': 4, 'cos': 6} cos slot waits [0, 4, 5, 4, 1, 0]`。CoS は枠の解放を待ってから起動（全 run が `launch=launched`・usage あり）。root ごとに worker と reviewer が 1 本ずつで重複しない（root 4 件に worker 4・reviewer 4）。
  - (3) 期限超過（CoS 90 秒、試行の上限 75 秒）: `timed_out 2 missing cos_drain_timeout:2 verdict unknown`。facts の `drain.timed_out_runs` に run_id・state・reason。usd・tokens は null。judge の `drain` 検査は unknown、g7 は pending、`cos_drain_timeout` は null。
  - (4) 2 試行が承認の総量 100 秒を分け合う: `shared deadline 2026-11-07T08:16:14Z walls [73, 31] wall_limit_hit [False, True]`。2 本目の期限は 1 本目の開始から数え、wall の和は 104 秒（試行ごとにリセットすると約 150 秒）。2 本目の `drain` 検査は unknown。
  - 修正前（`05522ee4` の driver・judge）で (1)(3) と同じ条件: `before before-slow-cos missing cos_usage_null:1 cos 1 cos_usage_null 1`、`before before-deadline missing cos_usage_null:1`。
    CoS run が daemon の SIGTERM で殺され usage が null になった（facts に `drain` 欄が無い。期限・理由は残らない）。修正後の同条件 (1) は usage が全部ある。
  - config-stub は全場面で G7 pending、`cos_usage_null=null`。実 LLM の合格は主張しない。
- 条件 2: 下の最終検査の節。

## 未実施・残り
- 実 LLM の G7 再実行は人の承認後（この task では行っていない）。本番 config・host への変更は無い（人が実行する手順は無し）。
- `launched_at`・`prelaunch_reason` が API で欄ごと省略される点に合わせ、driver は DB の列を直接読む（API は使わない）。
- `slot_wait_secs` は `launched_at - started_at`（秒の差）で、枠待ち以外の起動準備の時間も含む上限値。
- 修正後の (2) では worker の遅延で CoS が枠待ちになった（待ち 4〜5 秒）が、`not_launched` になる場面（枠待ちの期限切れ）は backend の試験が持つ。この task の e2e では起こしていない。

## 提案
- ADR 2026-11-06-cos-run-launch-accounting-and-drain D3-1 の式を付記 2026-11-07-pilot-drain-shared-deadline で改めた（承認の wall は総量）。本体の文面の更新は人の判断に任せる。
