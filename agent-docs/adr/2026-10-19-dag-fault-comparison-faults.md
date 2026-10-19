# dagbench の障害注入 runner（faults）: event 到達の停止 hook・所有 Child への SIGKILL・観測 ledger

- 日付: 2026-10-19
- 状態: 採用（実装済み: `crates/campaign/src/dagbench/fault.rs`、試験 `crates/campaign/tests/dagbench_fault.rs`）
- tasks: [01M59KY5FNM0K4XGPPS61D0CA2]
- 親: `agent-docs/adr/2026-10-19-dag-fault-comparison-driver.md`（D4）、`2026-10-19-dag-fault-comparison-celeris-driver.md`（C1）

## 決定

### F1. 停止点は ext-sim の event 到達で決め、制御 process 側の hook で止める
- ext-sim に `FaultKind::StopControl` を足す。trigger event の nth 件目（`nth = 1 + seed mod occurrences`）で
  `fault_fired` を ledger に fsync してから、制御 process が差した停止 hook（`ExtSim::set_stop_hook`）を呼ぶ。
  hook（`fault::block_until_killed`）は stdin を読んで止まるだけで、runner の SIGKILL を待つ。
  runner が先に居なくなれば EOF で exit 86（続きを勝手に進めない）。
- 理由: ledger の行を見てから kill するだけでは、kill までに制御側が先へ進み停止位置が揺らぐ。hook で止めれば
  停止位置は event 到達そのものになり、sleep・時刻依存の待ちが要らない（ADR-0125）。
- a2 の trigger `external_wait_started` は ext-sim の event にした（`ExtSim::wait_started`）。両方式が待機に入るときに
  呼び、論理 job ごとに 1 回だけ記録・計数する（再開後の再追跡は数えない）。これで 3 種の trigger がすべて
  独立の外部 ledger 上に乗る。

### F2. SIGKILL は runner が握る `Child` にだけ送る
- 制御 process は `FaultRunner` が `std::process::Command` で起こし、`Child::kill`（unix で SIGKILL）だけを使う。
  pid 番号で process を探して signal を送る経路は無い。本番 daemon・無関係 process には届かない。
- 試験では制御 process = 試験 binary 自身を `--exact child_control` で起こし直したもの（task-api の試験部品を使う
  Celeris 側の router が dev-dependency にしか無いため）。runner の module は command を注入で受け取る。

### F3. 観測 ledger（`fault.jsonl`）は runner だけが書き、制御停止と runner 停止を別 event 種別にする
- `runner_started` / `control_started`（pid・generation・restart・`ext_seq_at_start`）/ `trigger_observed`（fault_id・
  seed・nth・event・event_seq・ext_seq・logical_secs・logical_job_id）/ `control_stopped`（SIGKILL・cause）/
  `control_exited`（code・signal・killed_by_runner）/ `control_completed` / `runner_finished`。
- 評価 runner の停止は、次の runner が開いたときに「`runner_started` に対応する `runner_finished` が無い」ことで
  検出し、`runner_stopped`（`detected_at_resume: true`）として残す。制御停止は `control_stopped`。
- 介入数は `intervention` event の件数（今回の 3 種は自動で解けるので 0。手動操作を足すときはここに書く）。

### F4. 再開の仕方（両方式とも既存の機構を使う）
- baseline: state file を読み、`submitted`/`waiting` を論理 ID の照会で解く（baseline 仕様のまま）。
  進捗 log（`worker.jsonl`、celeris 側と同じ形）を足し、失われた進捗を ledger から数えられるようにした。
- Celeris: 同じ SQLite と ledger で `CelerisSide::with_faults` を作り直す。driver の状態（論理時計・worker ledger の
  seq・投入済み campaign・待機中 job の poller 表）は ledger から復元し、DB 側の `running` の run は既存の孤児回収
  （R4、`Dispatcher::set_orphan_takeover`。制御 process ごとに `daemon_instances` に active 行を登録）が拾う。
  再実行された WorkUnit の再投入は ext-sim が同じ論理 ID の既存物理起動に寄せる（`submit_deduped`）。
- 応答喪失（b）は Celeris 側の台本 worker も論理 ID の照会で解く（baseline と同じ再照合）。
- SIGKILL で書きかけになった ledger の末尾行は、開き直すときに捨てる（`ext::drop_torn_tail`）。

### F5. 障害の点は k2-mini × camp-1（campaign 1 本）
- baseline は 1 DAG instance の実行器なので、両方式に同じ条件を与えるため campaign 1 本の点で流す
  （scenarios.json の `faults.apply_at` は center。複数 campaign の baseline は metrics WU の掃引の範囲）。
- 同時に ready になる外部 node の投入順は方式で違うため、nth が当たる論理 job は条件ではなく結果として記録する。

## 結果
- `cargo test -p celeris-campaign --test dagbench_fault`: a1 / a2 / b / c × baseline / celeris と runner 停止の区別、6 passed。
  物理起動 = 論理 job = 消費、二重計上 0、各 node の完了はちょうど 1 回、完走、復旧時間・失われた進捗・介入数を ledger から算出。

## 却下した案
- ledger の行を見てから kill するだけ（hook なし）: 停止位置が実時間の競合で揺れる。
- pid を記録して外から `kill -9 <pid>`: pid 再利用で無関係 process に当たりうる。
