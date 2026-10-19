# dagbench の Celeris 側 driver（celeris-drv）: in-process の実 Dispatcher・位相レベル stage・論理時計

- 日付: 2026-10-19
- 状態: 採用（実装済み: `crates/campaign/src/dagbench/celeris_side.rs`、試験 `crates/campaign/tests/dagbench_celeris.rs`）
- tasks: [01M59KY5FNM0K4XGPPS61D0CA2]
- 親: `agent-docs/adr/2026-10-19-dag-fault-comparison-driver.md`（D1 の `celeris-drv`）

## 決定

### C1. 制御は driver の process 内の実 `Dispatcher`（daemon process は起こさない）
- 一時 SQLite（task-api の試験部品 `TestEnv` と同じ file）を開き、`Dispatcher::new` と既存の公開口
  （`set_llm_route_registry`・`set_harness_adapters`・`set_cluster_job_poller`・`set_cluster_liveness_probe`）だけで組む。
  Celeris 本体（task-core / task-dispatch / task-api）には手を入れない。
- campaign の root task は `celeris_exec::Transport` 越しの `POST /api/v1/tasks`（実 router、bearer 付き）で作る。
  label は `celeris_exec::idempotency_label(<測定 ID>-<campaign>)`。
- 理由: 本番 daemon の poller は ssh 経由（`clusters.rs`）で ext-sim を差せず、daemon の起動には測定 config 一式が要る。
  親 ADR D1 の「制御は子 process で起こす」は、障害 (a) の SIGKILL を担う faults WU が必要に応じて行う
  （同じ SQLite を開き直して `CelerisSide` を作り直せば、R4 の orphan 回収の経路を通る）。

### C2. DAG → v3 計画: node = leaf WorkUnit、位相レベルごとに 1 stage
- 既存の計画検証は「1 stage の unit は 6 以下」「stage 内の依存は 1 本まで」（`plan_invalid` で人の決定に落ちる）。
  そこで node の位相レベル（依存の最長経路）ごとに stage を作り、依存辺は stage の順序で表す（stage 内 `depends_on` なし）。
- k2-mini / k2-wide は各レベルの node が前レベルの node 全部に依存するので、DAG の制約と同じになる（余分な待ちを作らない）。
  将来この性質を持たない scenario を足すときは、stage の障壁で生じる待ちを ready 待ち（制御遅れ）として数える。
- 7 段は既定の `[tree] max_stages = 5` を超え、上限近くの承認（`PlanApprovalRequested`）に掛かるため、
  測定 config として `max_stages = 16` を与える（人の承認を台本で挟まない）。

### C3. worker 枠の写し方と git の作業場所
- `worker_slots` → `[execution] max_parallel_work_units` と dispatcher 全体の同時 run 上限（`config_max_concurrency`）の両方。
  planner・統合などの制御 run も同じ上限を使う（Celeris の実際の挙動。論理時間は消費しない）。
- Celeris は git の作業場所でだけ WorkUnit を並列に走らせる（ADR-0074 D1.2、非 git は並列 1）。campaign ごとに
  一時 dir に git リポジトリを作る（`GIT_CONFIG_GLOBAL=/dev/null`・`GIT_CONFIG_NOSYSTEM=1`、利用者の設定を読まない）。

### C4. 論理時計（離散事象）と poll
- 台本 worker（`WorkerAdapter`）は論理時計で `work_secs` 待つ。全ての in-flight の run と poll が論理時計待ちで、
  tick を 3 回回しても何も起きない（generation が変わらない）ときだけ、最も早い期限（または次の campaign の到着）へ進める。
  sleep の長さで時間を作らない（ADR-0125）。実時間の tick 間隔（10ms）は速さだけを決める。
- poller は wait 開始から job の期限（`job_secs` は `poll_secs` の倍数なので poll の刻みに一致）まで論理時計で待ち、
  ext-sim に `lookup` してから `qstat -xf` 形の `F` を返す。途中の `R` の poll は返さない（待ち時間は同じ、
  ext-sim の `lookup` は wait ごとに 1 回）。ext-sim の完了通知（notice）は Celeris 側では使わない（poll で知る）。
- 投入の `NoSlot`（外部 job 枠 2 が埋まっている）は台本 worker が `poll_secs` ごとに投入し直す（何も起こしていない拒否だけ）。

## 結果
- `cargo test -p celeris-campaign --test dagbench_celeris`: k2-mini camp-1 は makespan 1980（臨界経路の理論値）、
  center（2 campaign、到着間隔 600）も完走。worker ledger・ext-sim ledger・SQLite（root done・WorkUnit done・
  cluster_job_waits satisfied）が整合、物理起動 = 論理 job = 消費、worker 同時数 ≤ worker_slots。

## 却下した案
- 実 `celeris` daemon を子 process で起こす: poller が ssh 固定で ext-sim を差せない。口を開けるには task-dispatch / celeris の
  変更が要り、この WU の範囲（crates/campaign）を越える。
- dispatcher の注入時計（`test_now`）を公開する: `cfg(test)` 専用で、公開には本体の変更が要る。論理時計を driver 側に持てば不要。
