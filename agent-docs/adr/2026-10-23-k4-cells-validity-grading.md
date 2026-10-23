# ADR: K4 の採点はセル有効性を固定 raw から決定的に再計算して表と報告に照合する / 固定入力 tgz を generator → preflight → grader まで接続する

- 日付: 2026-10-23
- 状態: 実装済み（agent-docs/progress/2026-10-23-k4-validity-grading.md 参照）
- 関連: ADR 2026-10-21-grader-own-input-extraction-and-max-turns（D1 の `input_archive` を実資産 case に適用する）、ADR 2026-10-16-mlsys-independent-grader

## 背景
2026-10-23 の main（13d64acd）再監査で、K4（`k4-cmp4-validity-analysis`）の grade-case が「cell.json の件数 5 個」の再計算しか持たず、
課題（FS×workload の帯域・IOPS、有効/無効と理由、有効数／全数の分母、failed job・fsync 不一致・欠測の扱い）を何も判定していないことが分かった。
mlsys-main-3 の実 pilot では A が 30/32、B が 28/32（UnifyFS W3 を無効）と別の答えを出したが、旧 grader はどちらも区別できない。
また `RecomputeSpec.input_archive` は実装済みだが K3/K4 の grade-case には無く、generator も生成せず、preflight は archive を事前確認しない。

## 決定
### D1 セル有効性の再計算（`cells_validity`）
`RecomputeSpec` に `cells`（`CellsSpec`）を足す。grader は固定 raw（`input_archive` の展開先）から次を決定的に計算する。
- 期待セル集合: `<run>/planned-cells.json`（fs・rep・workload）。分母はこの集合の大きさ。欠測は分母に残る。
- セルの完了: `cell.json` と `exit.json` が在り、両方 `exit == 0`。
- 実効設定: IOR（w1a/w1b/w2）は `ior.json` が parse でき `Finished` を持ち、`summary` の write/read の `bwMeanMIB`・`OPsMean` が正、
  `tests[0].Parameters` の `fsync == 1`・`checkRead == 1`・`api` が FS の期待値、かつ stdout/stderr に `fsync() not supported` /
  `fsync(...) failed` が無い（実効 fsync）。mdtest（w3）は `mdtest-ranks.csv` の `all` 行の File creation/stat/removal が正で stdout に `-- finished at`。
- job 終了状態: セルの job（`cell.json.job`。欠測セルは run の唯一の job）の `qstat -xf` 記録（`jobs` map。raw 内の path）を読む。
  記録が無い／`Unknown Job Id` → `job_unknown`、`job_state != F` → `job_not_finished`（どちらも無効）。`F` かつ `Exit_status == 0` → 正常。
  `F` かつ `Exit_status != 0` → `job_failed`: 自身の完了記録を持たないセル（欠測・exit≠0・出力不完全）の理由になる。完了記録を持つセルは有効のまま
  （job 43706 は F/-20 で終わったが、人が承認した final-v3 の判定どおり完了済み 30 セルは有効、BenchFS W2 の 2 セルは job 失敗による欠測）。
  「state F だけで失敗」とはしない。
- 値: `write_MiB_s` `read_MiB_s` `write_ops` `read_ops`（IOR summary）、`md_create` `md_stat` `md_remove`（mdtest all 行）。

### D2 照合する成果物
- `results/cells.csv`（列 `fs,rep,workload,valid,reasons,write_MiB_s,read_MiB_s,write_ops,read_ops,md_create,md_stat,md_remove`。余分な列は可）。
  行集合は期待セル集合と一致（欠測も行にする）。`valid` は `valid`/`invalid`。無効セルの `reasons` は期待する主理由の語
  （missing / exit / job / fsync / checkRead / api / ior / mdtest）を含む。有効セルの数値は許容差 `tolerance_rel` 以内。
- `report.md` の `metric` 行: `cells_total` `cells_valid` `cells_total_<fs>` `cells_valid_<fs>`。表から数えた件数と報告の件数が違えば「表と報告の不一致」。
- 旧い 5 個の件数 metric も残す（固定 raw からの件数で、害はない）。自由記述は読まない。LLM に採点させない。

### D3 固定入力 tgz の接続
- `fixtures/grade-cases/tools/gen_recompute.py` は K3/K4 の grade-case に `input_archive`（case dir 基準の `<name>.tgz`、sha256、`extract_to=inputs`）を書き、K4 には `cells` を書く。
- tgz は repo に置かない（`.gitignore`）。評価側の写しは `<GRADE_CASES_ROOT>/<split>/<id>/<name>.tgz`。`celeris-campaign stage-grader-inputs
  --grade-case-root <dir> --inputs-src <dir>` が `--inputs-src/<name>` から sha256 を照合して写す（launcher の `prepare` が呼ぶ）。
- preflight の `graders` 検査は archive の実在・sha256・公開 `case.inputs[].sha256` との一致・展開・`pinned_raw`・`cells` の参照 file を確かめる。
  欠落・不一致・展開失敗は投入前に拒否する。grader は従来どおり欠落・不一致を `env_failure` にする。
- `check-main-pins.py` は `--pins <json>`（または `<root>/pinned/main/pins.json`）で測定ごとの固定値を受け、既定は mlsys-main-1 の値のまま
  （旧 bundle の検査を壊さない）。grade-case の `input_archive` の実在・sha256 を検査し、公開入力と同一 bytes の archive は leak と数えない。

### D4 やらないこと
- short-ml の `input_archive` 化（次の測定で有効にしない）。既存の測定履歴・raw・grade の書き換え。再採点は新しい grader 版として別 file に保存する。
