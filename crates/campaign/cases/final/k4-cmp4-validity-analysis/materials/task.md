# K4: 4 FS 比較の有効性分析

入力は cmp4 v3 の N=2 原票 tgz（job 43706、公開入力 v2）。`inputs/main-v3-n2-43706-public2.tgz` を `inputs/` 直下に展開する
（`cd inputs && tar -xzf main-v3-n2-43706-public2.tgz` → `inputs/runs/<run>/…`、`inputs/review28/…`）。
- `runs/<run>/`: `planned-cells.json`、`cells/<fs>/rep<n>/<workload>/`（`cell.json`・`exit.json`・`ior.json`・stdout/stderr・`mdtest-ranks.csv` 等）、
  `fs/<fs>/rep<n>/<workload>/`（FS 側の log）、`runner/`（投入・実行 script）、job・host・topology の記録。
- `review28/`: scheduler の原票（`qstat-<job>.txt`、`v3-submissions.json`）。
FS（BenchFS/CHFS/GekkoFS/UnifyFS）×workload ごとの帯域・IOPS と、セルが有効か無効か（理由つき）を原票から決め、
有効セル数／全セル数の分母つきの表と報告を作る。原票から自分で計算すること。

## 判定規則（採点器も同じ規則で原票から再計算する）
- 期待セル集合は `runs/<run>/planned-cells.json`（fs・rep・workload）。全セル数はその大きさ。欠測（dir や `cell.json`・`exit.json` が無い）も表の行に残し、分母に残す。
- セルは次がすべて成り立つときだけ有効:
  1. `cell.json` と `exit.json` が在り、どちらも `exit` が 0。
  2. IOR（w1a/w1b/w2）: `ior.json` が parse でき `Finished` を持ち、`summary` の write/read の `bwMeanMIB`・`OPsMean` が正、
     `tests[0].Parameters` の `fsync` が 1、`checkRead` が 1、`api` が FS の期待値（BenchFS=BENCHFS、CHFS=CHFS、GekkoFS/UnifyFS=POSIX）、
     かつ stdout/stderr に `fsync() not supported` や `WARNING: fsync(...) failed` が無い（実効 fsync）。
  3. mdtest（w3）: `mdtest-ranks.csv` の `all` 行の `rate-File creation` / `rate-File stat` / `rate-File removal` が正で、stdout に `-- finished at` がある。
  4. job の終了状態: セルの job（`cell.json` の `job`）の `qstat -xf` 記録（`review28/qstat-<job>.txt`）が `job_state = F`。
     記録が無い・`Unknown Job Id`・F 以外（R/Q/E 等）のセルは有効にできない。`F` かつ `Exit_status = 0` は正常終了。
     `F` かつ `Exit_status ≠ 0` は job が異常終了したことを示し、自身の完了記録（1〜3）を持たないセルの理由になる（完了記録を持つセルは有効のまま）。
     `job_state = F` だけを失敗と読まないこと。

## 出力（採点器が読む形。これ以外の自由記述は採点に使われない）
- `results/cells.csv`: 1 行 = 期待セル集合の 1 セル（欠測も行にする）。列（順不同、余分な列は可）:
  `fs,rep,workload,valid,reasons,write_MiB_s,read_MiB_s,write_ops,read_ops,md_create,md_stat,md_remove`。
  `valid` は `valid` か `invalid`。無効セルの `reasons` には主理由の語を含める
  （欠測 `missing`、終了 code `exit`、job 状態 `job`、`fsync`、`checkRead`、`api`、IOR 出力不完全 `ior`、mdtest 不完全 `mdtest`）。
  有効セルの数値は IOR `summary` の `bwMeanMIB`（write/read）・`OPsMean`（write/read）、mdtest `all` 行の rate（creation/stat/removal）。
  該当しない列は空。
- `report.md`: 表の説明に加えて、次の `metric <name> = <value>` 行を書く（表と一致させる）:
  `cells_total`、`cells_valid`、各 FS の `cells_total_<fs>`、`cells_valid_<fs>`、
  および `cell.json` を持つセルの件数 `cells_with_cell_json`、`cells_benchfs`、`cells_chfs`、`cells_gekkofs`、`cells_unifyfs`。
