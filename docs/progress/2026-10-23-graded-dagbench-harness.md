# graded DAG harness 再監査4：実装・検証記録

---
tasks: [01M5KVHDZ6PGW45VNA5E6MGRPC]
---

## 反映状態

監査対象・作業開始時の main はともに `13d64acd17806d73a1797bffbf831efc9cb5ab6a`。
指摘された grading 未接続、検証前の完了計上、attempt の非対称・再開時消失は残っていた。
最新 main で解消済みとして除外した項目はない。
実装 commit は `44412170`（タスクブランチ）。main 反映 commit は未確定で、Celeris の配送・統合待ち。
本番 daemon・DB・設定の変更や有料評価は行っていない。

## 実装した契約

正式な `tests/dagbench_harness.rs` の共通 `Params.grading` / `DAGBENCH_GRADING` から両 arm に
case、入力、grader、node、max_attempts を渡す。run.json は hash と `validated-attempts/1` を固定する。
graded 出力先の再使用・pin 変更は拒否する。成果物と採点は attempt ごとに保存し、失敗を成功で上書きしない。

実行終了は `execution_finished`、検証済み完了は `validated_completed`。
Celeris の後者は通常の WU check 合格後、baseline は独立 verifier 合格後に記録する。
新しい graded 集計 `celeris.dagbench-metrics/2` の完遂・makespan・依存 ready は後者だけを使い、
実行区間の busy time と再作業は失敗を含む全 attempt から算出する。

attempt は実行開始前に消費する。Celeris は worker ledger、baseline は state.json から復元する。
baseline の最終失敗も保存する。両 arm とも上限に達すると新しい実作業を始めず、再開でも補充しない。
共通の論理時間予算 `max_logical_secs` の停止も永続化する。
wall watchdog は controller ごとの安全上限であり、論理時間の測定予算とは別。

## 検証

- 初回の `cargo test -p celeris-campaign --test dagbench_graded --test dagbench_harness` は入口の負例1件で exit 101。
  既存 graded 4件は合格。
  K3 実原票の試験も実行した。入口の負例は、途中で見つけたエラー原因の隠蔽を修正し全件検査で再検証した。
- 正式入口の回帰試験：初回fail→2回目pass、max_attempts=1、全3回fail、失敗後の別process再開、論理予算停止後の再開。
  各 grade の保持、試行数、未検証 node の完了除外、失敗分 busy time を検査する。
- `bash scripts/dev/test-affected.sh`：commit後の最終実行 exit 0、2,588件合格・5件skip、doc tests合格。
  ログ：`test-affected-verified.log`。
- `cargo clippy --workspace -- -D warnings`：commit後も exit 0。
- `cargo fmt --all -- --check`：exit 0。

検査ログは `/local/celeris/data/workspaces/01M5KVHDZ6PGW45VNA5E6MGRPC/artifacts/` に保存。
途中の全件検査は 2,588件合格。その後の検査で変更範囲外の `root_done_summary::restart_does_not_duplicate`
と `launcher::a_version_mismatch_and_missing_inputs_are_rejected_with_zero_execution` が一時失敗した。該当2件の nextest 単独再実行は2件合格。K3反復終了後に全件検査を再確認した。
失敗ログも削除していない。

## 固定原票と保存先

K3 原票は既存の Sirius `main-43223.tgz`、SHA256
`bdb155d4c1690ae5743621d129c270fcc53c919751d62c56d0c4395b74d2e90f`。
取得元は `/local/celeris/data/workspaces/01M45ACZQWTW9BP8WVY8KAW9TC/artifacts/locusta-latency/raw/main-43223.tgz`。
既存 K3 grader と固定45本の raw hash を使う。原票不足の blocker はない。

保存付き反復は次のコマンドで実行する（成果物先は新規ディレクトリ必須）。

```sh
DAGBENCH_SUITE_OUT=/local/celeris/data/workspaces/01M5KVHDZ6PGW45VNA5E6MGRPC/artifacts/k3-repetitions-v3 \
  cargo test -p celeris-campaign --test dagbench_harness \
  real_k3_repetitions_from_formal_entry -- --exact --ignored
```

seed 17/23 × nofault/A1/B/C × baseline/Celeris の16条件が完走（438.32秒、exit 0）。
全条件で初回 fail・2回目 pass、計32件の grade を保存。16条件すべての grading pin が同一で、
保存済み raw から frozen grader binary の CLI で再集計した JSON/TSV は元の表と byte 単位で一致した。
機械向け集計は `k3-repetitions-v3/repetition-summary.json`、再集計記録は `reaggregation.log`。
各 `<seed>/<arm>-camp-1-<condition>/` に run.json、worker/ext/fault ledger、controller log、DB/state、
`artifacts/c0/analyze.attempts/<n>/artifact/`、隣の grade.json、`metrics/` の JSON/TSV を保存する。
`grader` は反復専用の固定実行ファイル、`grading.json` は共通設定、`inputs/good` は固定入力から作った成果物。
同一 raw からの再集計一致を各条件で検査する。

v1 は A1 の追加再作業を「必ず1回」とした試験側の誤った期待で停止。
v2 は並行した再ビルドによる grader hash 変更を pin 検査が検出して停止。
両ディレクトリ・ログは履歴として保存し、v3 では grader を固定コピーした。
これら途中の記録を完走した測定として混ぜない。

## 次の測定の開始可否

対象は固定 scenario/camp-1 の1つのローカル graded node。任意 DAG、多 campaign fault、
汎用 exactly-once、semantic artifact invalidation、新しい durable wait 基盤は実装していない。
実行は既存の正式 test harness から通常 API/router・dispatcher・WU check を通す無 LLM 検証。
実 LLM campaign launcher での有料 pilot や実 HPC 障害測定の完了を意味しない。

A2 の DB durable wait 後停止と、controller から独立した外部 process が継続する最終接続試験はタスク5に残る。
従って障害復旧比較の本測定開始はタスク5と合わせて判断する。本タスクだけを根拠に開始可とはしない。

## 再試行2：fault baseline の busy/redo 修正

前回レビューで基準2を差し戻し。実装 `cbbaccdb` で修正した（main 反映は配送待ち）。
前回 v3 は validated completion・grade 保持の証拠として残すが、fault baseline の
busy/redo 比較には使用しない。特に seed17/B は旧 busy=3180秒、redo=3回であり、
外部待機と初回 post 作業を余分に計上していた。

原因は fault driver が使う `set_progress_log` と通常 driver の `set_worker_log` のイベント差。
progress 側も post を `post_wait_started` として記録し、submit・応答喪失の reconcile・
再開時の未確定投入の照合から、両 logger に `external_wait_started` を記録する。
待機開始は ext の停止 hook より先に記録する。既存 metrics の区間定義・grading pin の意味は変えない。
無故障/B/C の両 arm は busy=1200秒、redo=1回/120秒（analyze の失敗分）で一致する。
これは worker の論理実行時間であり、grader の壁時計時間ではない。

追加した通常回帰試験は以下。

- `cargo test -p celeris-campaign --test dagbench_baseline worker_and_progress_logs_agree_on_external_execution_boundaries -- --exact`：exit 0。
  両 logger、通常投入/応答喪失を検査し、外部 node の開始570・待機630・post960・終了990秒を固定。
- `cargo test -p celeris-campaign --test dagbench_harness graded_faults_exclude_external_wait_and_post_from_redo -- --exact`：exit 0。
  正式入口 nofault/B/C × 両 arm の6条件で費用とイベント件数を検査。

修正後の固定K3反復は新しい絶対パス
`/local/celeris/data/workspaces/01M5KVHDZ6PGW45VNA5E6MGRPC/artifacts/k3-repetitions-v4` に保存する。
v1〜v3 の原票・grade・集計は変更しない。A1 の実際の再作業は arm ごとに異なり得るため、
B/C と同じ redo 回数を強制しない。A2 の最終停止境界・独立外部 process は引き続きタスク5の範囲。
本測定の開始可否は上記「次の測定の開始可否」から変更しない。

再試行2の必須検査はすべて exit 0。
`bash scripts/dev/test-affected.sh` は2,590件合格・5件skip、doc tests合格（199.858秒）。
`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check` も合格。
ログは成果物ディレクトリの `retry-test-affected.log`、`retry-clippy.log`、`retry-fmt.log`。
追加試験のログは `retry-boundaries.log`、`retry-fault-cost.log`。

K3 v4 は `DAGBENCH_SUITE_OUT` を上記 v4 パスに設定し、前掲の
`real_k3_repetitions_from_formal_entry -- --exact --ignored` で16条件完走（663.34秒、exit 0）。
全条件で初回fail→2回目pass、計32 grade を保存した。nofault/B/C は両 seed・両 arm で
busy=1200秒、redo=1回/120秒。A1 は baseline=1200秒/1回、Celeris=1320秒/3回で、
後者の外部 node pre 作業の再実行を保持している。

`python3 /local/celeris/data/workspaces/01M5KVHDZ6PGW45VNA5E6MGRPC/artifacts/summarize-repetitions-v4.py`
も exit 0。16条件の同一 grading pin、32件の grade、固定 grader による raw 再集計と
元の JSON/TSV の byte 一致を検証した。
機械向け集計は `k3-repetitions-v4/repetition-summary.json`、各 run の再集計は `reaggregated/`、
実行ログは `k3-repetitions-v4.log` と `reaggregation-v4.log`。
再採点は新規 v4 の各 attempt で実施し、v3 の grade は上書きしていない。
実資産の欠落 blocker はなく、この再試行の差し戻し事項は解消した。
実LLM/HPC障害比較の開始判断は、引き続きタスク5の完了と合わせて行う。
