# ADR: campaign の継続監視/再開 driver と cluster job の終了確認

- Date: 2026-10-16
- Status: Accepted
- 決定者: 監査修正 4 watch-driver（上位依頼 01M530W6D1C60YJZ107ESWA58M）
- 関連: [campaign-budget-reservation](2026-10-16-campaign-budget-reservation.md)、[campaign-comm-failure-tracking](2026-10-16-campaign-comm-failure-tracking.md)、`crates/campaign/src/watch.rs`

## 背景

`celeris-campaign run` は 1 回走査で戻る。非同期 executor は Wait を返すので、本測定では人が繰り返し呼ぶ必要があった。
また `CelerisExecutor` は `stopped()` を持たず（既定 true）、task が cancelled になっただけで cluster job が live でも
予約を解除し得た。

## 決定

- D1: `celeris-campaign watch`（`run` と同じ引数 + `--interval-secs`（既定 60）`--max-iterations`（既定 1440））。`runner::run` を
  間隔を置いて繰り返し、台帳に未完了行も実行待ちも無くなれば `settled`、上限回数に達すれば `max_iterations` で戻る（どちらも exit 0。
  出力 JSON の `watch.end`）。scheduler は作らない。状態の正本は台帳で、再起動は同じ引数で呼び直すだけ。
  投入の可否は `run` の予約 gate（ADR campaign-budget-reservation）がそのまま決めるので、承認済み manifest 以上は投入しない。
- D2: 時計は `watch::Clock` を注入する（試験は sleep を記録するだけの時計）。同じ out に driver を同時に 2 つ走らせない。
- D3: `CelerisExecutor::stopped` は task 木の全 task が終端、かつ全 cluster job が終了（PBS `F`・Slurm 終端・scheduler が gone と明示）
  のときだけ true。state が unknown/欠測の job は live とみなす。`poll` も同様で、timeout で cancel した後は木を読み直し、
  停止を確認できるまで `Wait`（Done にも予約解除にもしない）。task cancelled と job 停止は別の事実。
- D4: cluster job には試行の walltime 上限を渡す: task objective に `cluster-job-walltime-secs: N`（N = min(trial timeout, 残り wall)）、
  cancel 手順の台帳 `cluster-cancel.jsonl` にも `walltime_secs`。job の自動 qdel/scancel はしない（既存方針: 人手手順を記録）ので、
  job の終了確認は scheduler 側 walltime か人の実行後の `finished/gone` を待つ。
- D5: cancel の所有判定。台帳の task が `campaign-<hex>` の冪等 label を持たない（本 campaign の投入物でない）なら cancel も
  job 手順の記録もせず `Refused` を返す（行は cancel_pending のまま残り、comm_failures に理由が出る）。
  cancel 対象は台帳の root から辿れる子孫と、その木の job だけ。

## 想定最大超過

D1 の driver は poll 間隔 = `--interval-secs`。tokens の想定最大超過は budget-reservation の式
`max_parallel × (interval + 終端確認) × 消費速度`。job が人手 cancel 待ちの間は予約を維持するので、その間も新規投入の枠は戻らない。
