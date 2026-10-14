---
task: 01M4W3PTCHJD4XQPZGT81WBTRM
wu: aggregate
status: done
completed: 2026-10-14
---
# aggregate: error・cancelled の試行を合否から除き、除外数を根拠表に出す

## 変更
- `crates/routing-eval/src/aggregate.rs`
  - `collect` が `eval_trials` を `decision_id` で LEFT JOIN し、`Trial.eval_status` に `eval_trials.status` を入れる。古い error 行・`route_decisions` は消さず、そのまま読む。
  - `build` は `eval_status` が `error`・`cancelled` の試行を `n`・`pass`・Wilson / bootstrap 区間・paired 比較から外し、cell ごとに `ExcludedCount { error, cancelled }` を数える。
  - 出力: `evidence.json` に `excluded`（cell ごと）、`trials.json` の各 cell に `excluded`、`evidence.md` の表に `Excluded (error/cancelled)` 列、除外の合計行、除外のあった cell の表（全試行が除外された cell も載る）。stdout は `trials= excluded= cells=`。
  - DB に書く `EvidenceRow`（task-core）の形は変えない。除外数は根拠表の側にだけ出す。
- `crates/routing-eval/src/export.rs`: 除外した試行は cell を持たないので JSONL に出さず、`excluded_trials=` を出す。
- 試験 `aggregate::tests::aggregate_excludes_error_trials`: eval_trials を持つ fixture DB に done 2 件（passed/failed）+ error 2 件 + cancelled 1 件を入れ、`collect` で 5 件読めること（行は消さない）、`n=2, pass=1`、区間が `wilson_95(1,2)` と一致し `wilson_95(1,4)` と違うこと、`evidence.md`・`evidence.json`・`trials.json` に除外数 2/1 が出ることを確かめる。

## 証拠
- `cargo test -p routing-eval --lib -- --exact aggregate::tests::aggregate_excludes_error_trials` → `test result: ok. 1 passed`
- `cargo test -p routing-eval` → lib 25 passed、pipeline_stub 1 passed
- `cargo clippy --workspace -- -D warnings` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、nextest 5081 run / 5081 passed（summary: passed 5084, failed 0）

## 未解決事項
- queued・running・scoring の試行（`eval_trials.status` が終わっていない）は従来どおり数えている（`route_outcomes` が無ければ不合格扱い）。集計は全試行の終了後に回す前提。
- 設計判断の ADR 付記は fix-run / closeout 側の ADR 付記に「aggregate は error・cancelled を合否から除外し除外数を出す」を 1 行入れてほしい（並行 WU との同一ファイル衝突を避けてこの WU では触らない）。

## 提案
- 未終了の試行を数えないよう、aggregate が `queued`・`running`・`scoring` を見つけたら警告するか拒否する。
