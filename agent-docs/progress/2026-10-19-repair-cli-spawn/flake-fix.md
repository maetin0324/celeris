---
task: 01M5AKP5N96D4K8R8DK4BW2NQT
wu: flake-fix
status: done
completed: 2026-10-19
---
# flake-fix: ipc_external_survives_control_exit の再起動 control の panic

## 原因（panic 箇所と競合）
- 再現: `cargo test -p celeris-campaign --test dagbench_process` と同じ libtest 一括実行（5 試験が同じ process で並走）を `DAGBENCH_PROCESS_RAW_DIR` 付きで 3 回流し、1 回失敗（75.16s）。control-2.stderr:
  `panicked at crates/campaign/tests/dagbench_process.rs:437:52: drive failed: stuck: nothing sleeping and campaigns not terminal at logical 1710 ([Ready])`
  つまり celeris arm の `control_role` の `report.unwrap_or_else(|e| panic!("drive failed: {e}"))`。CelerisSide::drive が静止判定で「stuck」を返した。exit 101 はこの panic。
  失敗までの 73s の大半は、control 失敗後に External を drop した stop の猶予（`dropped_unstopped`、elapsed 60005ms）。control 2 自体は再開から約 1.3s で落ちていた（stderr の mtime 19:31:22.197）。
- 競合: control 1 は nth=2 件目の `wait_started` の ack で止まる。その時点で 1 件目の WU（exp-uring）の `Waiting` を dispatcher が記録して、すぐ poll を起こすかどうかで分岐する。
  - 通るとき（raw1・raw3）: SIGKILL の時点では 2 WU とも `running`。control 2 の孤児回収で 2 件とも初めから走り直す。
  - 落ちるとき（full2）: control 1 が exp-uring の `cluster_job_wait_started` を記録し、poll を起こしていた。DB の `cluster_job_waits` は `waiting`、`last_polled_at=19:31:19.754`。その poll の thread は論理時計待ちのまま SIGKILL され、結果を返さない。
    再起動した dispatcher は `last_polled_at` から壁時計で `poll_secs`（30s）経つまで同じ wait を poll し直さない（`crates/task-dispatch/src/dispatcher/cluster_job_wait.rs` `poll_cluster_job_waits`）。
    exp-posix が終わると、in-flight も sleeper も 0 になる。driver はこれを静止とみなし、次の期限が無いので「stuck」で Err を返していた。
  - 負荷で「1 件目の wait の記録と poll」が SIGKILL より先に来やすくなると落ちる。sleep・timeout の長さの問題ではなく、driver の静止判定の欠けだった。
  - SIGSTOP stutter で control を遅らせても再現しなかった（論理時計なので遅さそのものは効かない）。効くのは順序だけ。

## 修正（crates/campaign のみ）
- `src/dagbench/celeris_side.rs` `drive`: 静止していて次の期限（sleeper・到着）が無いとき、`waiting` の cluster job wait が残っていれば stuck にしない。論理時計は進めず tick を続け、dispatcher の次の poll（poller の呼び出し）を待つ（上限は既存の壁時計の観測上限 `cap_wall`）。sleep の延長・retry・assert の緩和はしていない。
- `tests/dagbench_process.rs`: 落ちた順序を決定的に作る `ipc_external_survives_control_exit_mid_poll` を追加。control 1 を、最初の poll の `lookup` で止める（`Park::Poll`。この時点で dispatcher は wait の `last_polled_at` を書き、poll は走っている最中）。止める場所は出来事（stdout の行）で決める。既存の `ipc_external_survives_control_exit` は `Park::WaitAck` のまま。

## 証拠
- 修正前の driver（`git show HEAD:…/celeris_side.rs` に戻す）で新しい試験を実行: FAILED（63.70s）。control-2.stderr は `drive failed: stuck: nothing sleeping and campaigns not terminal at logical 1620 ([Ready])` で、元の失敗と同じ。
- 修正後 `cargo test -p celeris-campaign --test dagbench_process`: 6 passed、exit 0（36.91s）。同じ binary の一括実行をさらに 4 回: 4 回とも 6 passed（35〜52s）。mid_poll は dispatcher の壁時計の再 poll（最大 30s）を待つ分だけ長い。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。`bash scripts/dev/test-parallel.sh`: exit 0（passed 5868・failed 0・ignored 15、nextest 205.6s）。

## 未解決事項・提案
- `cargo clippy -p celeris-campaign --all-targets` は既存の `tests/env_lock.rs:529`（`from_mode(0)`）で落ちる。task-dispatch の lib test にも既存の clippy エラーがある。どちらも範囲外で、触っていない。
- 提案: dispatcher の cluster job poll の周期は壁時計なので、dagbench の論理時計とずれる。再起動直後の wait では、最大 `poll_secs` の壁時計待ちが測定の壁時間に乗る（論理時間の測定値は変わらない）。壁時間も測るなら、dispatcher の時計を注入できる公開口が要る（本体側の変更なので別 task）。
