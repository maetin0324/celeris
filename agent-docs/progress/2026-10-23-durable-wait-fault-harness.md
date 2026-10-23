---
title: 再監査5 durable wait後停止と独立外部process
tasks: [01M5KVHHCMVFY0ZDPKYVS09GXC]
status: done
updated: 2026-10-23
completed: 2026-10-23
---
# 再監査5 durable wait後停止と独立外部process

## 既存成果と今回の境界

基点22711d25にはタスク4のd4f9195e（graded/attempt）と067e94d2（baseline費用境界）が含まれる。
調査時main d7cb0fd4との差分はdagbench実装・正式harnessでゼロ。A2の保存前停止とcontroller内ExtSimは未解消だった。
今回の変更はA2をSupervisor/IpcExtへ接続し、通常TaskStoreまたはbaseline保存済みstateからjob ID付きWaitingを読み直す。
親の再確認→所有controllerのSIGKILL・終了回収→独立Externalの完了sync ack→同じstateで再開、をrawに残す。
保存前停止はA0へ分離し、過去rawは上書きしない。既存durable wait基盤には変更しない。
タスク3の新collector schemaは使用しないため、この無LLM検証の受入依存には含めない。

## 検証

- 基準0–3: `cargo test -p celeris-campaign --test dagbench_harness fault_condition_is_a_parameter_of_the_same_entry -- --exact --nocapture` はexit 0（1 test、70.71秒）。両armのA2で保存確認・独立process継続・同一job回収を検証。
- 基準4: `CAMPAIGN_K3_TGZ=/local/celeris/data/scratch/mlsys-pilot-2/inputs/main-43223.tgz DAGBENCH_A2_OUT=/local/celeris/data/workspaces/01M5KVHHCMVFY0ZDPKYVS09GXC/artifacts/k3-a2-v1 cargo test -p celeris-campaign --test dagbench_harness durable_checks::real_k3_a2_repetitions -- --exact --ignored --nocapture` はexit 0（363.17秒）。seed17/24 × attempt上限1/2 × 両armの8条件、12 grade（初回fail8・再試行pass4）。上限1は未完遂4、上限2は独立検証済み完遂4。全条件で重複物理投入0。
- 生ログからの再生成: `python3 /local/celeris/data/workspaces/01M5KVHHCMVFY0ZDPKYVS09GXC/artifacts/reaggregate-a2.py` はexit 0。8条件すべてで保存JSON/TSVと再生成物がbyte一致、grade本体の合否も確認。
- 成功例の論理作業時間は両seed・両armで1200秒、やり直しは解析120秒。不合格維持例は870秒。LLM呼出し0、追加実HPC job 0。
- 初回fixture検査ではseed24 baselineの外部待機990秒がbusyへ誤加算された。pre作業終了を投入前に明記する修正後、K3で解消。通常fixture回帰も必須検査に含む（停止証跡欠落・job ID改変は再集計拒否）。
- 基準6: `bash scripts/dev/test-affected.sh` はexit 0（2593 passed、6 skipped、124.245秒、doc tests合格）。ログは成果物ディレクトリのtest-affected.log。
- 基準7: `cargo clippy --workspace -- -D warnings` はexit 0（68秒）。ログはclippy.log。
- 基準8: `cargo fmt --all -- --check` はexit 0。ログはfmt-check.log。

## 保存場所・版

raw/grade/集計は絶対パス /local/celeris/data/workspaces/01M5KVHHCMVFY0ZDPKYVS09GXC/artifacts/k3-a2-v1。
各seed/attempt/arm配下にrun.json、ext/worker/control.jsonl、process-events.jsonl、durable-wait.json、controller-outcome.json、
artifacts/c0/analyze.attempts/{0,1}/grade.json、metrics、reaggregated-v2を保存。
全体の機械集計はsummary.jsonとreaggregation-summary.json、入力・grader hashはprovenance.json。
元K3 tgzのSHA256はbdb155d4c1690ae5743621d129c270fcc53c919751d62c56d0c4395b74d2e90f。
再集計初版はgrade wrapperのpassedではなくfinal_passを読んで失敗したため、新規reaggregated-v2で検査し直した。既存raw/gradeは変更していない。
タスク4のk3-repetitions-v4および過去測定履歴は変更していない。

## コミットと追加検査

実装commitは38a755a5（報告c69d4a2d。旧runの4f13a062と同内容をこのbranchへ載せたもの）。38a755a5・c69d4a2dはmainに反映済み（`git merge-base --is-ancestor 38a755a5 main`・`c69d4a2d main`ともにexit 0、確認時main 81fd646a）。
任意のprogress-index全件検査は既存8文書のfrontmatter不備で非0。本タスクの新規文書は該当せず、必須health gateの対象外。

## attempt 2（必須検査の不合格への対応）

前回はtest-affectedが`credential_wait_notify`の偽webhookで落ちた（HTTP要求前のEOFを空本文として数え、left 3 / right 1）。
本タスクの差分ではなく、修正165d76e1は既にmainにある。`git merge --no-ff main`（f5a08c93、衝突なし）で取り込んだ。A2の実装は変えていない。

- `bash scripts/dev/test-affected.sh`はexit 0（2600 tests run: 2600 passed、6 skipped、176秒、doc tests合格）。
- `cargo clippy --workspace -- -D warnings`はexit 0。
- `cargo fmt --all -- --check`はexit 0。

## delivery-repair（release gate の cargo-test 不合格）

release 準備（a3933dc2）の cargo-test で task-dispatch `dispatcher::tests::work_units::parallel_units_survive_dispatcher_restart` が `max_active` 1≠2 で落ちた（本 task の差分外、負荷で 10ms の WU run が重ならない時間依存）。
試験用 `ParallelWuAdapter` に `meeting(n)`（同時に走る run が n に届くまで終わらない）を足し、再起動後の adapter に `meeting(2)` を付けて並列の観測を負荷に依らず決めた。
検査: `cargo nextest run -p task-dispatch work_units::` 55 passed、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0、`bash scripts/dev/test-affected.sh` exit 0（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch、2600 passed、6 skipped）。修正 commit は「phase 6: 再起動後の並列 WU 試験を同時実行の待ち合わせで決定化する」（05eec4c5、このbranchの祖先。mainへは配送待ち）。旧記載の337f3c64はrebase前のhashで誤り。

## 残る実HPC接続と測定開始可否

無LLM固定入力の復旧比較はタスク4と今回のA2を合わせて開始可能（必須3検査合格済み）。共通graded/attempt契約で検証済み。
実HPC復旧比較は開始不可。既存PBS/Slurm poller/SSH adapterはあるが、この正式harnessのExtBackendを
実schedulerの投入・成果物回収へ結ぶ接続は無い。測定専用controller、両armの投入台帳・job ID、
停止中qstat/sacct原票、再開後の同一job成果物hashと独立grader入力を揃える必要がある。
K3既存原票の再採点は新たなHPC復旧測定ではない。新規有料評価・実クラスタ投入・本番障害注入は行っていない。

## attempt 2 再提出（commit 参照の訂正）

review で commit 参照の誤り（337f3c64 は提出履歴に無い、main 反映済みを配送待ちと記載）を指摘された。上の 2 箇所を訂正した。
main 81fd646a（browser runner request の修正 c051132c を含む）を `git merge --no-ff main` で取り込んだ（衝突なし）。A2 実装は不変。
検査（再提出時）: `bash scripts/dev/test-affected.sh` exit 0（2603 passed、6 skipped）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
