---
tasks: [01M59KY5FNM0K4XGPPS61D0CA2]
---
# DAG・外部待機・障害復旧の比較ドライバ（dagbench）

設計は `agent-docs/adr/2026-10-19-dag-fault-comparison-driver.md`、固定 scenario は
`crates/campaign/fixtures/dagbench/scenarios.json`。記録の dataset は `control_measured`（実 LLM キャンペーンの記録とは別）。
この文書は他の節（faults / metrics / 手順）を後続の WorkUnit が追記する。

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

## 実行手順（通し受入・指標再生成・固定点掃引）

有料 LLM・実クラスタ job は 0。外部 job は ext-sim の stub、worker は台本。本番 daemon・本番 DB・`~/.config/celeris`・
`~/.local/celeris` には触れない（試験は一時 dir の SQLite と、測定側が起こした子 process だけを使う）。

### 通し受入試験

```
cargo test -p celeris-campaign --test dagbench_accept
```

`k2-mini`（campaign 1 本）を baseline と Celeris 通常経路（task-api の実 router への POST）の両方で、正常と 3 障害
（a1/a2 制御 process の SIGKILL・再起動、b 投入応答の喪失、c 完了通知の重複）で流す。各 run の生ログ（`ext.jsonl`・`worker.jsonl`・
観測 ledger `fault.jsonl`）から `control.jsonl` と `run.json` を整え、`dagbench metrics` と同じ `compute` で指標を再生成して断言する:
同一 DAG/資源の完了量、重複通知で二重計上なし、応答喪失で実起動数 = 外部記録の物理起動数、停止後に永続 state から再開。
停止した制御 process への SIGKILL は runner が握る子 `Child` にだけ送る。

### 1 点を流して指標を出す（baseline）

```
cargo run -p celeris-campaign -- dagbench run --scenario k2-mini --point center --arm baseline --out <raw dir>
cargo run -p celeris-campaign -- dagbench metrics --raw <raw dir> --out <表の出力 dir>
```

`--seed` を省くと scenarios.json の `seed`。`--arm celeris` は CLI 未対応（task-api の router が dev-dependency のため）。
Celeris 側は `tests/dagbench_metrics.rs` の `celeris_metrics_come_from_the_same_raw_format` と同じく試験内で `CelerisSide::drive` を使う。

### 出力の見方

`control_measured_metrics.json`（と 1 行の `.tsv`）。`dataset` は常に `control_measured`（実 LLM キャンペーンの
`llm_campaign` とは別 file・別 schema。成功率の欄は無い）。`condition` は `normal` または `fault:<id>`。主な欄:

- `makespan`: campaign ごとの投入受理→最後の node 完了（論理秒）。`completed`: 完了 node/campaign と仕事秒・外部 job 秒。
- `ready_wait`: ready から開始までの待ち。枠待ち（`slot_wait_*`）と制御遅れ（`control_delay_*`）に分ける。
- `control_overhead`: `/proc` 標本の CPU・ピーク RSS（in-process のため driver 全体の上限値。`scope` に明記）。
- `duplicates`: `extra_physical_launches`（論理 job あたりの物理起動 − 1 の和）と `double_counted`。どちらも 0 が期待値。
- `reconciliation` / `recovery` / `lost_progress`: 障害後の論理 ID 照会、再確定までの論理秒・実時間、やり直した作業。
- `interventions`: 人の介入の件数（`control.jsonl` の `intervention`）。`raw_sha256`: どの生ログから作った表か。

同じ raw からは byte 単位で同じ表になる。表の値を直接編集せず、常に raw から再生成する。

### 固定点掃引（人が実行する）

掃引は本 driver の試験では実行しない。一覧は `metrics::sweep_items(file, seeds)`（scenario × 7 固定点 × seed × 系、one-factor-at-a-time、
総当たりでない。84 項）。各項を人が実行する:

1. 一覧から `(scenario, point, seed, arm)` を取り、項ごとに空の raw dir を決める（既存 dir は使い回さない）。
2. baseline は上の `dagbench run … --arm baseline`。Celeris は `RunHeader::from_spec` + `CelerisSide::drive` を呼ぶ試験 harness（上記参照）で流す。
3. `dagbench metrics --raw … --out …` で表を作り、`measurement_id` と `dataset: control_measured` を確かめてから集める。
4. 障害の点は scenarios.json の `faults.apply_at`（`k2-mini`/`center`）だが、baseline は 1 DAG instance の実行器なので、通し受入は両方式とも
   campaign 1 本（`camp-1`）で同じ条件を与えている。center での障害掃引は別途 baseline の複数 campaign 対応が要る（未実施）。

CPU を焼く負荷は掛けない。時間は論理時計と出来事待ちで進む。

### 主比較 A0/B0 との関係

A0（直接 Claude Code+Sonnet）と B0（Celeris+Sonnet）の実 LLM 主比較の実行・pilot・最終報告は、この driver に**依存しない**
（別 task 01M58VGDNQ8GWGJPB8SJ73XSPF）。この driver は機構（DAG・外部待機・障害復旧）の無 LLM 比較で、結果を主比較の
成功率・品質の主張に混ぜない（dataset が別）。この driver の完了・未完了は A0/B0 の着手条件ではない。
