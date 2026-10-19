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
