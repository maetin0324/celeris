---
title: cluster job の wait を NQSV（Pegasus）でも job の終了で解く
tasks: [01M86R8E4JRK12W0PKPQR7FAWZ]
status: running
updated: 2026-11-24
---
# cluster job の wait を NQSV（Pegasus）でも job の終了で解く

状態: running（実装・試験・失敗注入は済んだ。最後の修正の後の HEAD での `test-affected.sh` の全体の再実行が残っている）。
基準は `git merge-base HEAD main`。

## commit（件名）

- adr: cluster job の wait を NQSV（Pegasus）でも job の終了で解く
- cluster job wait: NQSV の poll をログインシェルの qstat -f で流し、does not exist を終わりとして読む
- cluster job wait: schema 127 を ops-r1 の対応版数・migration の版数の試験に足し、schema を再生成する

## 実機の確認（読み取りだけ）

`ssh -o BatchMode=yes pegasus` で 2026-11-24T12:27Z〜12:28Z に実行した（job の投入・取消はしていない）。

- `bash -lc "command -v qstat"` → `/opt/nec/nqsv/bin/qstat`。非ログインの `qstat -f 1` → `bash: 行 1: qstat: コマンドが見つかりません`、exit 127。非ログインの PATH は `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games:/snap/bin`。
- `bash -lc "qstat -f 315477.nqsv"` → stdout に `Batch Request: 315477.nqsv does not exist on nqsv.`、stderr 空、exit 0。
- `bash -lc "qstat -f 315477 315478.nqsv"` → 2 行（`315477.nqsv`・`315478.nqsv`）、exit 0。server 名の無い id も `.nqsv` の形で答える。
- `bash -lc "qstat -xf 315477.nqsv"` → `qstat: invalid option -- 'x'` と usage。
- `qstat -Q` の状態の列: `ARR WAI QUE PRR RUN POR EXT HLD HOL SUS MIG STG`（gpu キューに 137 件の request があった）。
- `qstat -a`・`qstat -u all` は何も出さない（一般の利用者には自分の request しか見えない）。自分の request が無かったので、**Q・R の request の `qstat -f` の書式は未取得**。

## 受け入れ条件ごとの証拠

0. ADR: `agent-docs/adr/2026-11-24-cluster-job-wait-nqsv.md` を実装の前の commit（件名「adr: cluster job の wait を NQSV（Pegasus）でも job の終了で解く」）で足した。ADR-0090 に付記した。
1. NQSV の poll と読み方:
   - poll の script は `bash -lc '<env> && <setup> && qstat -f <ids>'`（`cluster_job::poll_script`、`ClusterRuntime::job_poll_request`）。
   - `Batch Request: <id> does not exist on <server>.`（stdout・stderr）は `gone` で、wait は `satisfied`（`nqsv_request_that_does_not_exist_is_gone_and_satisfies_the_wait`、dispatcher の `an_nqsv_wait_polls_in_a_login_shell_and_resumes_when_the_request_no_longer_exists`）。
   - Q・R（合成の段落）、状態の読めない段落、出力に無い request、`qstat -xf` の usage では解けない（`nqsv_queued_running_and_unread_requests_do_not_satisfy_the_wait`、dispatcher の試験の 1・2 回目の poll）。
   - PBS・Slurm の既存の試験の期待値は変えていない（`poll_commands_only_carry_valid_ids`・dispatcher の `{ module load openpbs; } && qstat -xf 42634 42635` はそのまま）。
2. 失敗注入（`artifacts/failure-injection.log`、注入の後に `git checkout -- crates/task-core/src/cluster_job.rs` で戻した）:
   - A（NQSV の poll を `bash -lc` で包まない）: `nqsv_poll_script_runs_qstat_f_inside_a_login_shell` と dispatcher の NQSV の試験が落ちた。
   - B（`does not exist` を `gone` にしない）: task-core の NQSV の試験 2 件と dispatcher の NQSV の試験が落ちた。
   - C（qstat に出ている request を終わりとして読む）: `nqsv_queued_running_and_unread_requests_do_not_satisfy_the_wait` と dispatcher の NQSV の試験が落ちた。
   - 試験は sleep に頼らない（試験用の時計を `advance` で進め、偽の poller が出力を返す）。
   - 案内: `CLUSTER_JOB_RECOVERY_GUIDANCE`・`cluster_job_wait_instructions`・`CLUSTER_JOB_PLANNER_GUIDANCE`・`docs/protocol/worker-protocol.md` を NQSV に合わせた。`qstat -x` は PBS Pro の道具としてだけ書いている。
3. 検査:
   - `cargo nextest run -p task-core -E 'test(/cluster_job|migration_0127/)'` → 18 passed。
   - `cargo nextest run -p task-dispatch -E 'test(/cluster_job|cluster_wait|job_poll/)'` → 23 passed。
   - 最初の `bash scripts/dev/test-affected.sh`（2 つ目の commit の時点）→ exit 100。8907 件のうち 15 件が落ちた（schema 127 を知らない `ops_r1_*` の対応版数、`migration_0042_fills_gaps_in_main_schema_41_database` の版数の列、schema の生成物 3 件）。web の typecheck・unit は 108 files・773 tests passed。
   - 修正（3 つ目の commit）の後、落ちた 15 件と cluster job の試験を含む filter（`committed_schema_matches_generated|event_row_schema_matches_committed|ops_r1_trace|ops_r1_attr|ops_r1_pipeline|migration_0042_fills_gaps|nqsv|cluster_job|cluster_wait|migration_0127|schema_version`、6 crate）→ 95 passed。
   - 3 つ目の commit の後: `cargo fmt --all -- --check` exit 0、`cargo clippy --workspace -- -D warnings` exit 0。
   - **未実行**: 3 つ目の commit の後の `bash scripts/dev/test-affected.sh` の全体の再実行（この run の壁時計が足りなかった）。

## 本番への移行

- config の書き換えは要らない（クラスタの設定は変えない）。
- schema 127（migration 0127）を含むので、昇格は stop → start。旧いバイナリはこの DB を開けない。
- 昇格の後、worker は Pegasus の job を `"scheduler": "nqsv"` で待つ（案内に書いた）。昇格の前からある Pegasus の `waiting` の wait（`scheduler = pbs`）は `qstat -xf` を流し続け、期限切れで解ける（自動では書き換えない）。

## 未解決事項

- Q・R の request の `qstat -f` の書式は未取得。読めなければ `unknown`（解けない側）になる。その場合、Q/H の連続時間（`queued_wake_secs`）は進まず、`queue_stalled` の起床は起きない。Pegasus で自分の request があるときに `bash -lc "qstat -f <id>"` を取り、fixture を実機の出力に替える。
- NQSV では終了コードが分からないので、早期起床の `job_failed` は起きない（全 request が消えた時点で `satisfied`）。
- 続きの run の前置きの `gone` の行は「no longer known to the scheduler (history expired?; exit status unknown)」のまま（PBS の期待値を変えないため）。NQSV の説明は案内の段落に書いた。

## 提案

- scheduler をクラスタの設定（`[[clusters]] scheduler`）に移し、wait の申告から `scheduler` を消す（worker が誤った scheduler を書く余地を無くす）。protocol・DB・campaign の wait の組み立てを一度に変えるので、別の task にする。
- 終了コードが要るなら、`racctreq` の出力の書式と反映の遅れを実機で確かめてから、poll の後段で読む案を ADR で決める。
