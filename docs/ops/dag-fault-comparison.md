# DAG・外部待機・障害復旧の比較ドライバ（dagbench）

---
tasks: [01M59KY5FNM0K4XGPPS61D0CA2, 01M5BT8CRNPJX1THX36XF4C7PZ, 01M5KVHDZ6PGW45VNA5E6MGRPC, 01M5KVHHCMVFY0ZDPKYVS09GXC]
---

無 LLM で Celeris の通常経路と合理的な非同期 DAG baseline を比べる driver の公開仕様と、通常の実行・再生成手順。
固定 scenario は `crates/campaign/fixtures/dagbench/scenarios.json`（schema_version 1。DAG・seed・資源枠・点は版に依らず固定）。
記録の dataset は `control_measured`（実 LLM キャンペーンの記録とは別）。有料 LLM・実クラスタ job は 0。

## baseline 仕様

実装は `crates/campaign/src/dagbench/baseline.rs`（module doc comment と同内容）。
Celeris の通常経路と比べるための「合理的な非同期 DAG 実行器」で、わざと弱くしていない。

- **依存解決**: 全依存が `done` の node を `ready` にし、scenario の node 定義順（FIFO）で worker 枠へ割り当てる。
- **資源枠**: `worker_slots`（外部 node の投入前 `pre`・完了後 `post` の作業も 1 枠を使う）と
  `external_job_slots`（外部 job stub 側が強制。空かなければ `submit_pending` で次の poll まで待つ）。
- **非同期待機**: 外部 job を投入したら worker 枠を即返し、`poll_secs`（既定 30。Celeris の `MIN_POLL_SECS` と同じ）ごとに
  論理 ID で照会する。待機中も他の ready node を進める。完了通知が先に届けばそれも使う。
- **永続 state**: 遷移ごとに `state.json` を write-temp → fsync → rename → fsync(dir) で原子的に書く。
  node の状態は `pending / ready / running / submitted / submit_pending / waiting / done`。
  外部投入は「投入する」意図（`submitted`）を先に書き、投入後に結果（`waiting`）を書く。
- **冪等キーによる再照合**: 外部 job は論理 job ID を冪等キーとして投入する。投入の応答が失われたとき（`submitted` のまま）は、
  論理 ID で照会する。見つかれば追跡を続け、見つからなければ同じ論理 ID で再投入する（外部側は既存の物理起動に寄せる）。
- **重複通知**: 待機中でない node への完了通知は捨てる（`duplicate_notices_ignored`）。結果の消費は論理 job ごとに 1 件。
- **持たないもの**: 複数 process の lease、計画・レビューの再作成、人の受信箱（機構比較の対象外。両系とも台本で同じにする）。

### 手動復旧手順

制御 process が止まった（kill・クラッシュ・再起動）ときは、**同じ `state.json` と同じ外部 ledger で再起動するだけ**でよい。

1. `Baseline::open(scenario, cfg, state.json)` が state を読む。`running` だった作業は止まった時点で失われたとみなし、
   最初から再実行する（`reruns` が増える）。
2. `Baseline::run` の冒頭で、`submitted` / `waiting` の外部 job を論理 ID で照会する。完了済みなら結果を消費して `post` 作業へ進む。
3. 外部 job は再投入しても二重起動しない（論理 ID で寄せる）。外部 ledger の `physical_launch` 数が論理 job 数と一致することで確かめる。

state file が壊れて読めないときは読み込みエラーで止まり、勝手に作り直さない。state を捨てて最初からやり直す場合は、
人が `state.json` を退避してから再起動する（外部 ledger は追記専用なので消さない）。この場合、外部 job は論理 ID が
ledger に残っているため再起動されない。

### 試験

`cargo test -p celeris-campaign --test dagbench_baseline`: `k2-mini` / `k2-wide` を完走して全 job が外部 ledger で
1 回ずつ起動・消費されること、保存点ごとに中断→再開で完走すること（応答喪失の注入あり）、通知重複で二重消費しないことを確かめる。
有料 LLM・実 job は 0。

## データ版（raw / 指標 / overhead）

| 種別 | schema | 作り手・読み手 |
|---|---|---|
| in-process raw | `celeris.dagbench-raw/1` | `dagbench run` と `metrics::compute`（/2 は拒否） |
| 別 process raw（外部時計・通知） | `celeris.dagbench-raw/2` | `process::compute_process`（/1 は拒否） |
| A2 保存後停止 raw | `celeris.dagbench-durable-raw/1` | 正式 harness と `metrics::compute`（生存・保存証拠を必須検証） |
| 指標表 | `celeris.dagbench-metrics/1` | `control_measured_metrics.json` / `.tsv` |
| 制御 overhead | `celeris.dagbench-overhead/1` | `overhead::regenerate`（`control_isolated_overhead.json` / `.tsv`） |

版が違う raw は混ぜず Err で拒否する。旧 in-process の `driver_process_inclusive` / `samples.jsonl` は overhead として読まない。

## 外部時計と完了通知

外部 job の stub（ExtSim と ledger）は制御 process とは別の独立 process が唯一所有する（`dagbench::process`、IPC は `dagbench::ipc`）。
制御 child を kill しても外部側は生き続ける。論理時計は独立の observer が `observe` op（通知を消費せず ack もしない）で進める。
完了通知は seq 付き・ack 付き・再送の at-least-once で、重複は seq と論理 job ID で冪等に捨てる。通知を失っても poll の `lookup` で拾える。
通知は「永続適用後に ack」する。両方式とも同じ台本・同じ `poll_secs`（30）で外部 job を照会する。

## 実行手順（通し受入・指標再生成・固定点掃引）

有料 LLM・実クラスタ job は 0。外部 job は ext-sim の stub、worker は台本。本番 daemon・本番 DB・`~/.config/celeris`・
`~/.local/celeris` には触れない（試験は一時 dir の SQLite と、測定側が起こした子 process だけを使う）。

### 試験

```
cargo test -p celeris-campaign --test dagbench_accept --test dagbench_metrics
cargo test -p celeris-campaign --test dagbench_process --test dagbench_overhead
```

- `dagbench_accept`: `k2-mini`（campaign 1 本）を baseline と Celeris 通常経路（task-api の実 router への POST）の両方で、正常と 3 障害
  （a1/a0 制御 process の SIGKILL・再起動、b 投入応答の喪失、c 完了通知の重複）で流す。各 run の生ログ（`ext.jsonl`・`worker.jsonl`・
  観測 ledger `fault.jsonl`）から `control.jsonl` と `run.json` を整え、`dagbench metrics` と同じ `compute` で指標を再生成して断言する:
  同一 DAG/資源の完了量、重複通知で二重計上なし、応答喪失で実起動数 = 外部記録の物理起動数、停止後に永続 state から再開。
  停止した制御 process への SIGKILL は runner が握る子 `Child` にだけ送る。
- `dagbench_metrics`: 同じ raw 形式から両方式の指標が出ること、同じ raw から byte 単位で同じ表になること。
- `dagbench_process`: 外部 process が制御 child の終了後も生き続けること、通知の継続・応答喪失からの回復（`recovery_notice_continuation` /
  `recovery_lost_response`）、raw 版の取り違えの拒否。
- `dagbench_overhead`: 方式ごとに隔離した制御 child の CPU/メモリ標本（下記）。

### 1 点を流して指標を出す

baseline（CLI）:

```
cargo run -p celeris-campaign -- dagbench run --scenario k2-mini --point center --arm baseline --out <raw dir>
cargo run -p celeris-campaign -- dagbench metrics --raw <raw dir> --out <表の出力 dir>
```

`--seed` を省くと scenarios.json の `seed`。別 process raw（/2）の要約は `dagbench process-metrics --raw <dir> --out <dir>`。

Celeris（試験 harness）: `--arm celeris` は CLI 未対応（task-api の router が dev-dependency のため）。
正式な反復入口は下の「反復実行入口」。
`RunHeader::from_spec` + `CelerisSide::drive` を呼ぶ試験で流す（例は `tests/dagbench_metrics.rs` の
`celeris_metrics_come_from_the_same_raw_format`、別 process 版は `tests/dagbench_process.rs` と `tests/dagbench_overhead.rs`）。
どちらの方式も、同じ scenario・点・seed・資源枠・`poll_secs` を使う。

### 出力の見方

`control_measured_metrics.json`（と 1 行の `.tsv`）。`dataset` は常に `control_measured`（実 LLM キャンペーンの
`llm_campaign` とは別 file・別 schema。成功率の欄は無い）。`condition` は `normal` または `fault:<id>`。主な欄:

- `makespan`: campaign ごとの投入受理→最後の node 完了（論理秒）。`completed`: 完了 node/campaign と仕事秒・外部 job 秒。
- `ready_wait`: ready から開始までの待ち。枠待ち（`slot_wait_*`）と制御遅れ（`control_delay_*`）に分ける。
- `duplicates`: `extra_physical_launches`（論理 job あたりの物理起動 − 1 の和）と `double_counted`。どちらも 0 が期待値。
- `reconciliation` / `recovery` / `lost_progress`: 障害後の論理 ID 照会、再確定までの論理秒・実時間、やり直した作業。
- `interventions`: 人の介入の件数（`control.jsonl` の `intervention`）。`raw_sha256`: どの生ログから作った表か。

同じ raw からは byte 単位で同じ表になる。表の値を直接編集せず、常に raw から再生成する。

### 制御 overhead（隔離した制御 child）

CPU とメモリは別 process 版（`control_isolated_overhead.*`、scope `control_child_isolated`）で出す。
対象は runner が方式ごと・再起動ごとに新規に起動した制御 child だけで、External・Observer・試験 harness・他方式の child は含めない。
CPU は epoch（制御 child 1 起動分）内の差分を合算し、メモリは `VmHWM`（start barrier で `/proc/self/clear_refs` により現在値へ戻す。
戻せない場合は生涯 HWM と明記し、開始前の値を `pre_start_hwm_kb` に残す）。標本欠損・ack 欠落・starttime 不一致は 0 にせず `NA`。
従来の `control_overhead`（in-process の driver 全体の上限値）は、単体の制御 overhead として読まない。再生成は `overhead::regenerate(raw)`。

### 固定点掃引（人が実行する）

掃引は本 driver の試験では実行しない。一覧は `metrics::sweep_items(file, seeds)`（scenario × 7 固定点 × seed × 系、one-factor-at-a-time、
総当たりでない。84 項）。各項を人が実行する:

1. 一覧から `(scenario, point, seed, arm)` を取り、項ごとに空の raw dir を決める（既存 dir は使い回さない）。
2. baseline は上の `dagbench run … --arm baseline`。Celeris は上記の試験 harness で流す。
3. `dagbench metrics --raw … --out …`（別 process raw は `process-metrics`・`overhead::regenerate`）で表を作り、
   `measurement_id` と `dataset: control_measured` を確かめてから集める。
4. 障害の点は scenarios.json の `faults.apply_at`（`k2-mini`/`center`）だが、baseline は 1 DAG instance の実行器なので、通し受入は両方式とも
   campaign 1 本（`camp-1`）で同じ条件を与えている。center での障害掃引は別途 baseline の複数 campaign 対応が要る（未実施）。

CPU を焼く負荷は掛けない。時間は論理時計と出来事待ちで進む。

### 主比較 A0/B0 との関係

A0（直接 Claude Code+Sonnet）と B0（Celeris+Sonnet）の実 LLM 主比較の実行・pilot・最終報告は、この driver に**依存しない**。
この driver の測定は別 dataset `control_measured` で、主比較 A0/B0 とは別、B1/C1 とも別である。機構（DAG・外部待機・障害復旧）の
無 LLM 比較であり、結果を主比較の成功率・品質の主張に混ぜない。この driver の完了・未完了は、既存の主比較 task をブロックしない。

## 反復実行入口（baseline と Celeris を同じ手順で）

`crates/campaign/tests/dagbench_harness.rs`（正式な test-harness 入口。`--arm celeris` が CLI で使えない代わり）。
case（scenario）・point・seed・故障条件・出力先を環境変数で与え、同じ手順で両方式を流して raw から指標を再生成する。

```
DAGBENCH_ARM=baseline|celeris DAGBENCH_SCENARIO=k2-mini DAGBENCH_POINT=camp-1 [DAGBENCH_SEED=<n>] \
[DAGBENCH_FAULT=a1-restart-after-accept|a2-restart-after-wait|b-submit-response-lost|c-completion-duplicate] \
DAGBENCH_OUT=<dir> \
  cargo test -p celeris-campaign --test dagbench_harness -- --exact run_from_env --ignored --nocapture
```

出力: `<out>` の raw 一式（`run.json`・`ext.jsonl`・`worker.jsonl`・故障時は `fault.jsonl`・`control.jsonl`）と
`<out>/metrics/control_measured_metrics.{json,tsv}`。資源枠は scenarios.json の値（両方式で同じ）。
故障条件は camp-1 の点だけ（並行 campaign を評価対象にする場合は、fault runner の複数 campaign 化が別途要る。今回は拡張していない）。
通常 run は camp-4 など複数 campaign の点でも流せる。試験 `both_arms_run_through_the_same_entry_and_regenerate` /
`fault_condition_is_a_parameter_of_the_same_entry`（a1・a2 を両方式で）が入口を固定している。

## stage 変換は依存を強めない

Celeris 側の計画は位相レベルごとに 1 stage（`RunSpec::plan_json`）。stage の順序は「前の stage の node 全部」を待つので、
DAG の祖先でない node を待たせると scenario に無い依存を足す。`RunSpec::stage_extra_precedence` はその対（a, b）を返し、
`stage_conversion_does_not_add_dependencies` が固定 scenario × 全 point で空であること、レベルが揃わない DAG では
検出器が空でないことを確かめる。

## 独立 grader 付きの仕事（検証失敗 → 既存 repair → 完遂）

`crates/campaign/src/dagbench/graded.rs` と `tests/dagbench_graded.rs`。scenario の 1 node（`analyze`）を、固定入力
（生データ・評価側が取得した scheduler 記録・評価テスト）から作った実成果物の仕事にし、評価側 grader
（`celeris-campaign grade`: 評価テストの実行・scheduler 記録との突き合わせ・raw からの数値再計算）が判定する。

- 最初の試行は speedup を捏造した成果物。grader の `recompute` が不合格にする（負の対照を試験で固定）。
- Celeris: WorkUnit の `checks` が grader。不合格 → 既存の check 不合格の経路（`previous_check_failures` を次の run へ）→
  2 回目の run が正しい成果物 → 合格 → task Done。baseline: 同じ成果物の列を同じ grader で検査し、不合格なら同じ node を
  最初から再実行（`Baseline::set_verifier`）。試行上限も同じ（`Grading::max_attempts`）。上限で合格しなければ完遂にしない。
- 既存の実 case: `real_k3_case_is_graded_independently_on_both_arms` が K3（`k3-locusta-ab-analysis`）の固定 grade-case
  （`fixtures/grade-cases/dev/k3-…json`、45 本の raw を sha256 固定）と、実 Sirius の IOR 結果 tgz（`main-43223.tgz`、hash 固定。
  `CAMPAIGN_K3_TGZ` で差し替え可。無い環境は実行不能として飛ばす）から成果物を作り、最初の試行（報告の数値を 6% ずらす）を
  実 grader が不合格 → repair/再実行 → 合格を両系で通す。玩具 fixture（`fabricated_first_attempt_…`）は scheduler 観測
  （`--observed`）と実装検査の経路用に残す。K1/K2 は実装検査に実 build（BenchFS submodule・UCX）が要るので対象外。
- 成功宣言だけの成果物は同じ grader が不合格にする。常に true の check は科学的成功の根拠にしない（判定付きでない node の
  `true` は台本の node で、成功率の欄に入れない）。
- 正式入口 `tests/dagbench_harness.rs` の `Params.grading` / `DAGBENCH_GRADING=<Grading JSON>` が両 arm に同じ設定を渡す。
  対応範囲は固定 scenario の camp-1、外部待機でない1 node。`max_attempts > 0` を必須とする。
  `run.json.grading` に case hash、grader 実行ファイル hash/版、入力ファイル hash、改変列、node、attempt 上限を固定する。
  出力先は新規ディレクトリ必須。子 controller は同じ設定と永続 state で再開する。
- `artifacts/c0/<node>.attempts/<0始まり番号>/artifact/` と隣の `grade.json` に各試行の成果物・独立採点 stdout/stderr/exit を保存する。
  `<node>` は最新成果物へのリンク。過去の attempt は成功時にも上書きしない。
- `execution_finished` は実作業の終了、`validated_completed` は検証済み完了。
  Celeris は独立 grader の結果を保存し、さらに WU check が合格した後に完了を記録する。
  baseline は verifier 合格後に記録する。graded 集計は `celeris.dagbench-metrics/2` とし、完遂・makespan・依存 ready は
  `validated_completed` だけを使う。旧 raw の `node_finished` による集計は旧定義として残す。
- `execution_busy_secs` は各実行区間の和（失敗も含む）。`lost_progress.rerun_node_starts` / `rerun_work_secs` は再実行から計算する。
  障害時は採点失敗以外の再作業も加わるため、再作業回数を必ず1とはしない。
- attempt は開始前に消費し、Celeris は worker ledger、baseline は state.json から再開時に復元する。
  上限まで失敗した場合は未完遂として停止し、再開しても予算を補充しない。baseline の最終失敗も保存する。
  Celeris の replan/no-progress 枠には指定 attempt 分の repair 機会を確保するが、実作業の上限は独立した attempt counter が守る。
  共通の測定予算は `max_logical_secs`。予算停止も永続化する。`max_wall_secs` は各 controller の安全用 watchdog であり測定予算ではない。

実 K3 の原票を使う保存付き反復（seed 17/23 × nofault/A1/B/C × 両 arm）は次で実行する。
`CAMPAIGN_K3_TGZ` が未取得なら明示的に失敗し、fixture 合格で代用しない。

```sh
DAGBENCH_SUITE_OUT=/absolute/fresh/output \
  cargo test -p celeris-campaign --test dagbench_harness \
  real_k3_repetitions_from_formal_entry -- --exact --ignored
```

各 run の `metrics/` は raw だけから `celeris-campaign dagbench metrics --raw <run> --out <new-dir>` で再集計する。
これは実 LLM の能力評価や実 HPC 障害測定ではなく、固定原票を使った control-plane の接続検証である。

## 測定専用 controller の停止（既知 job ID・wait 永続化後）

`a2-restart-after-wait` は正式 harness から Supervisor / IpcExt に接続する。
外部 job は controller と別 process にあり、停止中に observer が論理時計を進める。
`durable_wait_saved` は seed で選んだ job の Waiting と job ID を読み直した境界。
Celeris は TaskStore の cluster_job_waits_waiting、baseline は sync/rename 後の state.json を使う。
親が再確認してから所有 controller だけを SIGKILL・回収し、外部完了の sync ack 後に再開する。
保存前の旧 external_wait_started 停止は `a0-restart-before-wait-save` として分離する。
旧 A2 raw は保存後停止の証拠として使用できない。

A2 raw は celeris.dagbench-durable-raw/1。process-events.jsonl に job/wait ID、保存確認、終了、外部完了、再開の順序を保存し、
durable-wait.json に親の読み取り結果を保存する。共通 metrics はこの証跡と同一物理 job の一度だけの起動・完了・回収を検証する。
A2のCPU/RSSは停止前と再開直後の疎な標本であり、全runのCPU消費比較には使わない（別のisolated overhead手順を使う）。
worker/grade の定義・attempt 上限は上記タスク4と共通。外部完了までの停止時間は論理時間として計上し、実 HPC 所要時間とは解釈しない。

```sh
DAGBENCH_A2_OUT=/absolute/fresh/output \
  cargo test -p celeris-campaign --test dagbench_harness \
  durable_checks::real_k3_a2_repetitions -- --exact --ignored
```

seed 17/24（2番目/1番目のwait）× attempt上限1/2 × 両armで、失敗維持と復旧後の独立採点を確認する。
固定K3入力を使用し、graderを出力先へpinする。summary.jsonと各runのmetricsを保存する。
これは実HPC復旧の証明ではない。既存PBS/Slurm pollerとSSH adapterは利用可能だが、
正式harnessのExtBackendはまだIpcExt/ExtSimであり、実scheduler投入・成果物回収への接続は未実装。
実HPCを主張するには、測定専用controller、投入台帳/job ID、停止中のqstat/sacct原票、同一job成果物とhash、
独立grader用のobserved入力を結び、両armで等しい資源・予算・障害境界を固定する必要がある。
タスク3のcollector schemaはこの無LLM検証では使用しない。追加有料測定・本番障害注入は実施しない。

## 指標の定義と、何が実測で何が未実施か

| 項目 | 出どころ |
|---|---|
| 完遂/正しさ | `completed`（node 数・仕事秒）＋ 判定付き node は独立 grader の合否 |
| 回復時間 | `recovery`（障害後の再確定までの論理秒・実時間） |
| 再実行/重複 | `lost_progress`・`duplicates`（`extra_physical_launches`・`double_counted`） |
| 追加消費 | `lost_progress.total_secs`（やり直した作業秒）。LLM 消費は 0 |
| 人介入 | `interventions` |

この driver の結果は全て `control_measured`（論理時計・台本 worker・ext-sim・grader は実走）で、実 LLM / 実 HPC
campaign の完遂率とは別の dataset（`llm_campaign`）。**実 LLM・実 HPC での比較は未実施**。必要な最小枠は、
独立 grader の case 1 件 × 2 方式 × 故障あり/なし（a2）で、各 1〜2 回（seed 固定）の実 HPC job（短時間・1〜2 node）と、
LLM 比較をする場合の同一 model/harness・同一予算上限。既存承認の残枠で足りるかは人が確認する
（新規枠・予算増額はこの task の範囲外）。
