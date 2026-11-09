---
title: 健全性v2 G3 inventory-e2e — 全体棚卸しの runner と debt の起票の接続・通しの試験
tasks: [01M6ZDF99MKHFN57PN0H7G6Z9Z]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 inventory-e2e — 全体棚卸しの runner と debt の起票の接続・通しの試験

完了日: 2026-11-09

WorkUnit `e2e`（task 01M6ZDF99MKHFN57PN0H7G6Z9Z、unit inventory の子）。[G3 の ADR](../../adr/2026-11-08-health-enforcement-loop.md) の (6) D6.1〜D6.7 と (8) に従う。基準は `$CELERIS_WU_BASE`。ADR 本文は変えていない（G3 closeout が直す）。

## 実装前 brief
1. 根本原因: runner（unit runner）は repo ごとの評価の行を起票の欄 0 で追記し、起票の関数 `task_ops::health_debt::file_health_debts`（unit filing）を呼ぶ口が無い。daemon の sink は起票の組み立てに要る `[[roles]]`・`[[genres]]` を runner に渡していない。
2. 責務 owner: runner の本体 `crates/task-dispatch/src/health_review/inventory.rs`（`InventoryFire`）。起票の判定と作成は `task_ops::health_debt`、表は `task_core::health_inventory`、tick の sink は `dispatcher::cos_chat::review::CosReviewSink`。
3. 既存の再利用先: `file_health_debts`・`HealthDebtInput`・`FiledHealthDebts`・`MAX_DEBTS_PER_FIRE`、`RepoInventory.details`（`gate::run_health_check_with_details` の同じ実行の説明）、`SqliteStore::health_inventory_append`・`health_inventory_fire_counts`、tick の `CronFireContext`（cron の task の起票と同じ roles・genres）、`gate_tests` の stub の検査器と fixture。
4. 選択肢: 起票の結果を (a) 行を追記した後に更新する（表は追記だけで更新の method が無い。不採用）、(b) 起票してから 1 回だけ追記する（採用）。roles・genres の渡し方は (a) `CosReviewSink` に欄を足す（dispatcher module のコード行が増える。不採用）、(b) sink の欄 `inventory` の型を印と tick の文脈を持つ `inventory::Port` にする（dispatcher module の行の増減 0。採用）。
5. 新しい状態・分岐・fallback・port: `inventory::Port`（`InventoryRuns` の可変参照と `CronFireContext` の参照。tick の間だけの借用で状態は持たない）。`InventoryFire` の欄 `project_id`・`roles`・`genres`（tick の外へ持ち出す写し）。fallback・flag は無し。
6. 撤去対象: runner の「起票の欄は 0」の記録（`record` は起票の結果を受け取る形に替えた）と、その旨の doc comment。
7. 不変条件: 下の試験。

## 変更
- `crates/task-dispatch/src/health_review/inventory.rs`:
  - `InventoryFire::run` は repo の順に `inventory_repo` → `file` → `record` を行い、起票の枠 `MAX_DEBTS_PER_FIRE`（3）を repo をまたいで減らす。
  - `InventoryFire::file`（新規）: `inventory_repo` の同じ検査器の実行の `HealthEvidence` と `HealthBlockingDetail` をそのまま `file_health_debts` に渡す。検査器を走らせ直さず、report を読み直さない。評価しなかった repo（`unchanged`・`not_applicable`）は何もしない。`missing_detail` が 1 以上なら log に warn を出す。
  - `InventoryFire::record` は `&FiledHealthDebts` を受け取り、`task_ids`・`existing_skipped`・`limit_remaining` を同じ行に入れて 1 回だけ追記する（行は更新しない）。
  - `Port`・`InventoryRuns::port` を足し、`start_inventory` は `CronFireContext` を受け取って roles・genres を写す。
- `crates/task-dispatch/src/dispatcher/cos_chat/review.rs`: 欄 `inventory` の型を `inventory::Port<'a>` に替えた（1 行の置き換え）。
- `crates/task-dispatch/src/dispatcher/cron_tick.rs`: sink の初期化を `self.health_inventory.port(&ctx)` に替えた（1 行の置き換え）。
- 試験: `health_review/inventory_e2e_tests.rs`（新規、4 件）、`dispatcher/tests/health_inventory.rs`（1 件を足し、Dispatcher の組み立てを `setup` に出した）、`health_review/inventory_tests.rs`（`InventoryFire` の欄と `start_inventory`・`record` の引数の追従だけ。判定は変えていない）。

## size の扱い
- `dispatcher` module（`crates/task-dispatch/src/dispatcher`）の本体のコード行の増減は 0（`review.rs` と `cron_tick.rs` は 1 行ずつの置き換え）。`dispatcher.rs` は変えていない。
- 本体は `health_review/inventory.rs`（module `crates/task-dispatch/src/health_review`）に置いた。試験の file（`tests/`・`*_tests.rs`）は size の検査の対象外（`scripts/dev/repo-health/health/adapters.py` の分類）。

## 試験
守る不変条件と、既存の試験で検出できない理由: runner の試験（`inventory_tests`）は起票の欄が 0 の行だけを見て、起票の試験（`task_ops::health_debt::tests`）は手で組んだ `HealthEvidence` を渡すだけなので、同じ検査器の実行の証跡と説明が起票に渡ること・行に起票の結果が 1 回で入ること・発火をまたぐ重複の防止・repo をまたぐ枠・tick を通した LLM の不起動を検出できない。検査器は `gate_tests` と同じ stub（一時 git repo の base に commit した `repo_health.py`）で、外部ネットワークに出ず LLM を呼ばない。
- `health_inventory_does_not_file_same_debt_twice_across_fires`（runner を直接）: 1 回目の発火で 1 件起票し、head を 2 回進めた発火では評価済みの行（head は新しい commit、blocking は同じ fingerprint）が起票 0・既存で省いた 1 を持つ。debt の task は 1 件、冪等キーも 1 件のまま。
- `health_inventory_files_drafts_only_without_rewrite`（runner を直接）: 起票した task は全部 draft で、親・依存・子 task を持たない。objective は finding の path を挙げる。表 `execution_plans`・`work_units`・`runs`・`chat_runs`・`chat_messages` の行は 0。repo の ref・HEAD・symbolic-ref・作業ツリー（ignored を含む）・worktree・stash は発火の前後で同じ。
- `health_inventory_records_counts`（runner を直接）: 2 repo × blocking 2 件で、`health_inventory_fire_counts` が repos 2・evaluated 2・起票 3・枠で残した 1 を返し、行の task id の集合が store の debt の task と一致する（repo ごとに 2 件・1 件、枠で残した 1 は 2 つ目の repo）。head を進めた 2 回目は起票 1（前回の枠の残り）・既存で省いた 3。3 回目（tree が同じ）は 2 repo とも `unchanged` で起票 0。
- `health_inventory_exception_finding_not_filed`（runner を直接）: report の finding 2 件はどちらも severity error・delta new で、1 件は検査器の例外（`exception`）付き。行の blocking は例外でない 1 件だけで、起票はその 1 件だけ。例外の fingerprint の task は無い。
- `health_inventory_does_not_call_llm`（dispatcher の tick）: tick の発火から起票まで通し、次の tick の後も `runs`・`chat_runs`・`chat_messages` の行の数は予定時刻の前の tick の後と同じ。debt は draft の 1 件で dispatch されず、発火の集計は repos 1・evaluated 1・起票 1。
  - `chat_threads` は数えない。draft の debt は受信箱の項目になり、CoS triage の取り込み（`task_core::chat::triage` の `item_thread`）が案件の CoS thread を 1 件用意するため（message・run は作らない。試験の土台は `max_cos_runs = 0`）。

`health_inventory_` で始まる task-dispatch の試験は runner の 7 件と合わせて 12 件。

## ADR との違い（ADR 本文は G3 closeout が直す）
- D6.5 は `CronJobRun.detail` に repo の数・評価・省略・起票の数を書くとするが、detail は runner が書く「始めたこと」（案件と git の repo の数）のままにした。評価と起票は tick の外で後から終わるので、発火ごとの数の正本は表 `health_inventory_runs` の行の集計（`SqliteStore::health_inventory_fire_counts`）だけにした。D6.6 の「残りは数を detail に書く」も同じで、枠で残した数は行の `limit_remaining` にある。
- 起票の結果の `missing_detail`（blocking にあるのに説明か path が無い数。unit filing が足した欄）は表に列が無く、detail にも書かないので、log の warn にだけ出す。
- 起票に失敗した repo（store のエラーなど）は行を書かない。次の発火が同じ base から評価し直し、失敗の前に作れた debt は冪等キーと終端でない task で `existing_skipped` になる。行を書くと base が進み、その blocking の finding は次から `existing` になって起票されなくなるため。
- 発火をまたぐ枠の数え方は runner の `run` が持つ（repo の順に残りの枠を渡す）。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/health_inventory_|health_evidence_gate|cron/)'` → exit 0、42 passed（`health_inventory_` 12 件）。
- `bash scripts/dev/test-affected.sh` → 下の「test-affected」に記す。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変更は `crates/task-dispatch/src/{dispatcher/cron_tick.rs,dispatcher/cos_chat/review.rs,dispatcher/tests/health_inventory.rs,health_review/{inventory,inventory_tests,inventory_e2e_tests}.rs}` とこの file だけ。

## 未解決事項
- 枠（3 件）で残した finding は、次の発火で base が進むので `existing` になり、後から起票されない（ADR D6.6 の設計のまま。数は行の `limit_remaining` に残る）。後から拾うには人が行の数を見て起票するか、枠の扱いを ADR で決め直す。
- 別々の job が同じ案件を同時に棚卸しすると、`has_open_debt` の確認と作成の間に別の head で同じ fingerprint を起票しうる（同じ head は冪等キーで防ぐ）。job は案件に 1 件の運用を本番への移行の手順（unit record）に書く。

## 提案
- G3 closeout が ADR (6) D6.5・D6.6 に「発火の数の正本は表の行の集計、detail は始めたこと」「`missing_detail` は log だけ」「起票に失敗した repo は行を書かない」を付記する。

## 本番への移行
- この unit では無し（config・表・API の変更は無い）。job は既定で無いので本番の挙動は変わらない。有効化の手順は unit record が書く。
