---
task: 01M59KY5FNM0K4XGPPS61D0CA2
wu: accept
status: done
completed: 2026-10-19
---
# DAG・外部待機・障害復旧の比較ドライバ: 通し受入

WU ごとの記録は `2026-10-19-dag-fault-comparison/<wu>.md`。ここは accept WU の統合記録。

## 区分表

| 区分 | 内容 |
|---|---|
| 実装済み | 固定 scenario（k2-mini/k2-wide・7 固定点）、ext-sim と独立 ledger、非同期 DAG baseline、Celeris 通常経路 driver（`celeris_side`）、event 到達トリガの障害 runner（`fault`）、生ログからの指標再生成（`metrics`）、`dagbench run/metrics` CLI（baseline arm） |
| 無 LLM 検証済み | `dagbench_accept`（正常 + 3 障害 × 両方式、生ログから再生成）、`dagbench_fault`/`_metrics`/`_baseline`/`_celeris`/`_ext`。有料 LLM・job 0 |
| 実 pilot 未確認 | 実 LLM・実クラスタ job を使った機構の確認（本 driver は stub のみ）。本番 daemon での障害注入も未実施 |
| 任意・延期 | 固定点掃引の実行（84 項）、center（複数 campaign）での障害、`--arm celeris` の CLI 化、制御 process 単体の CPU 標本（現状は in-process の上限値） |
| 承認待ち | 実 LLM を使う検証・予算を使う再測定（この task では承認も実施もしない） |

## 証拠コマンド
- `cargo test -p celeris-campaign --test dagbench_accept` → 6 passed（4 回連続）。
- `cargo clippy -p celeris-campaign --lib --bins --test dagbench_accept --test dagbench_fault --test dagbench_metrics -- -D warnings` → 0。
- `cargo fmt --all -- --check` → 0。
- workspace 全体の `bash scripts/dev/test-parallel.sh` と `cargo clippy --workspace -- -D warnings` は統合段（integrate-verify）の結果を参照。

## 効果・有意性について
未測定の効果・有意性は主張しない。この試験は断言（二重計上 0、実起動数 = 外部記録、再開）が成り立つことを 1 seed・1 点で示すだけで、
baseline と Celeris の makespan・overhead の優劣、反復間の分散、有意性は固定点掃引を人が実行するまで未測定。

## 未解決事項
- faults と metrics の結線: baseline の障害 run は進捗 log（`set_progress_log`、`worker.jsonl` へ node_started/finished のみ）で書くため、
  `post_wait_started`/`consumed` を使う metrics の一部（ready 待ち・後処理のやり直し）は baseline 障害 run で通常 run より粗い。
  通し受入は完了量・二重計上・起動数・再開を断言し、粗い欄は断言していない。`set_worker_log` との統一は後続で判断。
- `control.jsonl` は試験側が観測 ledger から組み立てる（runner は書かない）。論理秒は trigger 時点の値で代用。
- 障害の点は k2-mini × camp-1（baseline が複数 campaign 非対応のため）。

## 提案
- progress log と worker_log を統一し、障害 run の指標を正常 run と同じ精度にする。
- runner が `control.jsonl` を直接書く。
- 掃引の人手順を `celerisctl`/script 化する場合は別 task で（主比較をブロックしない）。
