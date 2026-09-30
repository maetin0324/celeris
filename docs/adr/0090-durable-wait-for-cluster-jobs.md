# ADR-0090: クラスタ job（PBS / Slurm）の durable wait

- 日付: 2026-09-30
- 状態: **Accepted（Phase R7-1 で実装）**
- 関連: [ADR-0072](0072-task-execution-decomposition.md) D7〜D11・D18（continuation / checkpoint / yield）、
  [ADR-0080](0080-browser-phase2-policy-broker-approval.md) D4（browser の durable wait。`blocked` で待ち、専用の経路だけが再開する形を借りる）、
  [ADR-0079](0079-recursive-task-decomposition.md) D10（生存確認）・付記 R6-1 D4（runs 索引の照合）、
  [ADR-0018](0018-remote-clusters-over-ssh.md) / [ADR-0019](0019-worktree-sync-for-large-repositories.md) / [ADR-0059](0059-command-only-remote-workspace.md)
  （クラスタ・ssh master・`.celeris/remote-exec`）
- 番号: 0082〜0089 は他のブランチ（browser Phase 3/4、dispatcher の分割、CoS の枠）が使っているので 0090。

## 文脈

本番 2026-09-30 05:16Z / 05:38Z（docs/progress/phase-R.md「R6」）: BenchFS の実験の子 task 01M3R8BWFYT81RKEWZCEW5S3HK が sirius で
PBS の job（E1 v2 42634〜42636 が Q、A0 v2 が R）を投げたまま run を終え、reviewer が「job がまだ終わっていないので完了を確認できない」で
2 回落とし、task failed → 根の replan → 子を作り直して再投入、の churn になった。1 run（≤ 1800 s）→ review の cadence では、数時間かかる
クラスタ job を扱えない。continuation で qstat を poll し続けるのは run と quota の浪費で、`max_continuations_per_work_unit` / 進捗なしの
上限にも当たる。

## 決定

### D1. worker protocol: `result.json` の `wait`

run は `artifacts/result.json` に次の形を書いて終われる（`WorkerMessage::Wait` と同じ形。入れ子の `{"summary": .., "wait": {..}}` も受ける）:

```json
{"type": "wait", "kind": "cluster_job", "cluster": "sirius", "jobs": ["42634", "42635"],
 "scheduler": "pbs", "poll_secs": 300, "timeout_secs": 43200,
 "checkpoint": {"completed": ["..."], "remaining": ["..."], "next_action": "..."},
 "summary": "..."}
```

- `kind` は `cluster_job` だけ（省略時も同じ）。`scheduler` は `pbs`（既定）| `slurm`。`jobs` は 1〜64 件、`[A-Za-z0-9._-\[\]]`
  （先頭は英数字、64 文字以内。シェルに渡すので厳しく）、重複は落とす。`cluster` は省略時 task のクラスタ。
- 優先順位は `question` > `wait` > `summary` > `yield`。不正な wait は `error(retryable)`（`invalid cluster job wait: …`）で、待たない。
- `checkpoint` は `yield` と同じ扱い（`merge_checkpoint` で合成し `CheckpointSaved{end: waiting}` に残す）。
- **continuation の回数（`max_continuations_per_work_unit`）・進捗なしの窓（`no_progress_limit`）・attempts に数えない**:
  `RunEnd::Waiting` は `is_continuable()` が偽、`consecutive_continuations` は `waiting_for_cluster_jobs` / `cluster_job_resume` を
  `dispatch` と同じく読み飛ばし（数えず窓も切らない）、`no_progress_streak` と進捗の基準（`latest_progress_checkpoint`）は
  `end = waiting` の checkpoint を除く。
- adapter（claude-code / codex / acp / aider / subprocess の直接プロトコル）は `Terminal::Waiting` を返す。reviewer・planner・疎通確認の
  run が wait を書いても待たない（reviewer はやり直し、planner は不正な試行）。

### D2. daemon: `cluster_job_waits` と poll

- migration 0034（**schema 34**）: `cluster_job_waits(wait_id, task_id, work_unit_id?, run_id, cluster, scheduler, jobs_json, poll_secs,
  timeout_secs, summary, checkpoint_json, created_at, deadline, last_polled_at, finished_at, state, last_status_json)`。
  `state` は `waiting | satisfied | timed_out | cancelled`。**events が正本**で、表は event と同じトランザクションで書く派生の索引
  （`cluster_job::apply_event_tx` を `append_event_tx` と `apply_transition_tx` の extra events から呼ぶ）。例外は `last_polled_at` と
  状態の変わらない poll の `last_status`（event を出さない）。
- **止め方**（run の後段 `finish_worker_result`、同じトランザクションで `ClusterJobWaitStarted`）:
  - atomic の run: task を `Trigger::ClusterJobWait`（`running → blocked`、reason `waiting_for_cluster_jobs`、attempts 不変、lease 解放）。
  - v2 / v3 の WorkUnit の run: unit を `blocked(cluster_jobs)`（`WorkUnitBlockedReason::ClusterJobs`）、task は `advance`。同じ段階の兄弟は
    止めない（`runnable_work_units` と `settle_phase` は `decision` / `infra` と同じ扱い）。段階は完了しない。
  - v1（段階の無い /1）の WorkUnit の run: unit を `blocked(cluster_jobs)`、task を `ClusterJobWait`。
  - 検証: クラスタは `[[clusters]]` にあり、remote の task なら自分のクラスタと同じ（local の task は名前を明示）。`poll_secs` は
    クラスタの `job_wait.poll_secs` 以上、`timeout_secs` は `job_wait.max_wait_secs` 以下（省略時はそれ）に丸める。
- **poll**（tick の `poll_cluster_job_waits`、`refresh_cluster_liveness` の後）: `waiting` の wait ごとに、`last_polled_at + poll_secs` を過ぎて
  いて poll が走っていなければ、クラスタの `env` / `setup` の後に `qstat -xf <ids>`（PBS）/ `sacct -n -P -X -o JobID,State,ExitCode -j <ids>`
  （Slurm）を `ssh -o BatchMode=yes <host> -- <script>`（人が張った ControlMaster を借りる。60 秒で打ち切り）で OS スレッドに流し、
  結果は次の tick 以降に拾う（tick を塞がない）。明示的に切れている（`cluster_connected = false`）クラスタには流さない。
  poll を起こした時点で `last_polled_at` を書くので、再起動の直後にも `poll_secs` に高々 1 回。
- **読み方**: PBS は `Job Id:` の段落の `job_state` と `Exit_status`（`Q/W/T/M` → queued、`H/S/U` → held、`R/B` → running、`E` → exiting、
  `F/X/C` → finished）。stderr の `Unknown Job Id <id>` は `gone`（履歴が消えた。終わったものとして扱う）。出力に現れない job は `unknown`
  （poll の失敗かもしれないので終わったとは扱わない）。exit ≠ 0 でどの job の状態も分からなければ poll の失敗（次の `poll_secs` で再試行）。
  Slurm は `COMPLETED/FAILED/CANCELLED/TIMEOUT/OUT_OF_MEMORY/NODE_FAIL/…` を finished、`ExitCode` の前半を終了コードにする。
- **すべて finished / gone**: `ClusterJobWaitFinished{satisfied, jobs}`。atomic は同じトランザクションで `Trigger::ClusterJobResume`
  （`blocked → ready`、reason `cluster_job_resume`）、unit は `needs_continuation`（`cluster_jobs_finished`）。次の run は continuation で、
  前置きの「クラスタ job の結果」節に job ごとの最終状態と `Exit_status`、待つ前の要約、回収の指示が入る（`ContinuationContext.cluster_jobs`）。
- **上限（`deadline`）を過ぎた**: `ClusterJobWaitFinished{timed_out}`。atomic（と v1）の task は `blocked` のまま人への質問
  （`QuestionRaised`「…の待ちが上限（N 秒）に達しました: 42634 (R) …。延長する／job を取り消して続ける／取り下げる…」と承認の行）。回答で
  `ready` に戻り、次の run は答えと job の状態（`timed_out`）を受け取る。**v2 / v3 の unit** は unit を `needs_continuation` に戻し、続きの run の
  前置きが「待ちの上限に達した。人に聞く」を指示する（task を `blocked` にすると兄弟の run が止まるため。質問はその run の `question` が出す）。
  木の決定（`decision`）にはしない（木でない /2 にも同じ規則で効かせるため）。
- `waiting` の間は一般の回答・途中確認の再開で `ready` に戻さない（`apply_transition_tx` が `cluster_job_wait_pending` で拒む）。
  受信箱の質問にも出さない（上限を過ぎた後の質問だけが出る）。

### D3. events

`ClusterJobWaitStarted{wait}`、`ClusterJobWaitPolled{wait_id, jobs}`（**状態が変わった poll だけ**）、`ClusterJobWaitFinished{wait_id, state, jobs,
detail}`。`EVENT_TYPES` は 48 語。replay は task の状態（新しい reason は attempts を使わない）・runs（`waiting`）・work_units
（`cluster_jobs` → `blocked(cluster_jobs)`、`cluster_jobs_timed_out` → `blocked(question)`）を events だけから作り直せる。

### D4. 生存確認・枠・runs 索引・reviewer

- wait の間は run が無い: provider の枠・account・クラスタの並列度を使わず、lease も持たない。
- 生存確認（ADR-0079 D10）: `blocked(waiting_for_cluster_jobs)` の節点と `blocked(cluster_jobs)` の unit は名指しの待ち（`cluster_jobs`）で、
  `StallDetected` にしない。
- run は `WorkerFinished{end: waiting}` で閉じ、`runs.status = waiting`（`running` ではないので R6-1 の照合は閉じ直さない）。
- reviewer は起こさない（task は `reviewing` にならない）。最終レビューは続きの run が完了を申告した後。

### D5. プロンプトと API / GUI

- remote-exec の worker の前置き（`ssh::remote_exec_instructions`）と /3 planner の leaf の基準（`CLUSTER_JOB_PLANNER_GUIDANCE`）に 1 段落:
  長い job は投げたら `wait` で終える、Q / R の間に完了を申告しない、回収は続きの run、受け入れ条件は「すべての PBS job が F で
  Exit_status 0」を要求してよい。「投入」と「回収」を別の unit に分けない。
- `GET /tasks/{id}` の `cluster_job_wait`（`waiting` の wait: job ごとの状態、`status_line`、`next_poll_at`、`deadline`）。GUI の task の
  ページは 1 行「クラスタ job を待っています: 42634 (R) 42635 (Q) …」。

### D6. 中止と job

task が終端（中止・連鎖の中止・失敗）になったら、同じトランザクションで `waiting` の wait を `cancelled` に閉じ `ClusterJobWaitFinished{cancelled}`
を残す。**celeris は `qdel` / `scancel` しない**: job はクラスタの資源の予約で、人が結果を拾う・投げ直す判断の余地を残す（誤った自動の取り消しは
数時間の計算を捨てる）。取り消したい人は `.celeris/remote-exec qdel <id>` か クラスタで直接行う。

### D7. 設定

`[[clusters]] job_wait = { poll_secs = 300, max_wait_secs = 86400 }`（既定）。`poll_secs >= 30`、`poll_secs <= max_wait_secs <= 14 日`
（`Config::validate`）。**本番の設定に足すのは、この欄を知る release の昇格の後**（旧い版は未知のキーで起動に失敗する。P-G1-1）。

## 結果

- 数時間の job は 1 つの unit / task のまま「投入 run → daemon の poll → 回収 run」で扱え、reviewer の不合格 → replan の churn が止まる。
- poll は `poll_secs` に高々 1 回の `qstat`（ssh master 越しの 1 コマンド）で、LLM も worker の枠も使わない。
- schema 34（migration 0034）。旧いバイナリはこの DB を開けない（`SchemaTooNew`）ので、昇格は stop → start（N-1 の検査は `live_ok = false`）。

## 残したもの

- v1 の unit の上限切れの回答の後の run は continuation にならない（`resume_after_answer` が `ready` に戻す。前置きに job の状態は出ない）。
- `timed_out` の後の「延長」は、続きの run が再び `wait` を書くことで行う（daemon 側の延長操作は無い）。
- PBS の job history（`qstat -x`）が無効なクラスタでは、終わった job は `Unknown Job Id` → `gone`（終了コード不明）になる。
- Slurm は `sacct`（accounting）が前提。実機（Slurm のクラスタ）では未確認。
