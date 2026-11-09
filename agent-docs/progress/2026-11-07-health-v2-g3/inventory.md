---
title: 健全性v2 G3 inventory — 既存 cron の任意・低頻度の全体棚卸し（差分検査・未変更 repo の省略・重複起票の防止）
tasks: [01M6ZDF99MKHFN57PN0H7G6Z9Z]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 inventory — 既存 cron の任意・低頻度の全体棚卸し（差分検査・未変更 repo の省略・重複起票の防止）

完了日: 2026-11-09

task 01M6ZDF99MKHFN57PN0H7G6Z9Z（G3 の unit inventory）の記録。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の (6) を実装した。既存の cron（ADR-0131）に種類 `health_inventory` を足し、既定では job を作らない。基準は `git merge-base HEAD main`（各 WorkUnit は `$CELERIS_WU_BASE`）。ADR の本文と状態（`状態: 提案`）は変えていない。下の「ADR との違い」の付記は G3 の closeout が書く。

人の手順（有効化・無効化・確かめ方）は [docs/ops/cron-jobs.md](../../../docs/ops/cron-jobs.md) の §9 にある。

## 各 unit の記録
| unit | 内容 | 記録 |
|---|---|---|
| kind | cron の種類 `health_inventory`、task を作らない種類の共通の検証 `validate_input_kind`、sink の method `post_health_inventory`（既定 Err）、手動実行の拒否 | [inventory-kind.md](inventory-kind.md) |
| alias | kind が残した `reconcile_project_id` の wrapper を消し、呼び出し元を `template_project_id` に替えた | [inventory-alias.md](inventory-alias.md) |
| store | migration 0121（表 `health_inventory_runs`、追記だけ）と store の method を `crates/task-core/src/health_inventory` に、migration の登録を表 1 つにして `crates/task-core/src/migration_registry.rs` に置いた | [inventory-store.md](inventory-store.md) |
| filing | debt の draft の起票 `task_ops::health_debt::file_health_debts`（検査器の `blocking` だけ・冪等キー・重複の防止・1 回 3 件まで） | [inventory-filing.md](inventory-filing.md) |
| runner | 棚卸しの runner `health_review/inventory.rs`（適用判定で検査器を選ぶ差分検査・未変更 repo の省略・表への記録・Busy の印） | [inventory-runner.md](inventory-runner.md) |
| e2e | runner と起票の接続、発火をまたぐ重複起票なし・書き換えなし・LLM 不起動の通しの試験 | [inventory-e2e.md](inventory-e2e.md) |
| record | `docs/ops/cron-jobs.md` §9（有効化・無効化・確かめ方）とこの記録。コードは変えていない | この file |

## commit（件名）
- 「inventory kind: cron の種類 health_inventory と task を作らない種類の共通の検証」
- 「inventory kind: 進捗に test-affected の結果を書く」
- 「phase 1: add append-only health inventory store」
- 「phase 1: record plan issue for inventory store scope and health checks」
- 「phase 1: move inventory store and migration registry out of store module」
- 「phase 1: record inventory store checks」
- 「inventory alias: reconcile_project_id の別名を消し呼び出し元を template_project_id に替える」
- 「phase 1: file health debt drafts from inventory blocking」
- 「phase 1: record inventory filing checks」
- 「phase 1: health inventory runner with applicability-selected diff checks」
- 「phase 1: record inventory runner checks」
- 「phase 1: wire health inventory runner to debt filing with e2e tests」
- 「phase 1: record inventory e2e checks」
- 「phase 1: rename inventory filing tests to fit the unit scope」
- 「phase 1: record inventory e2e checks after rename」
- 段の統合の merge commit「integrate wu/<key> (phase core)」・「integrate wu/<key> (phase wire)」
- 「phase 1: record health inventory enablement procedure and inventory record」（この記録と docs/ops/cron-jobs.md §9）

## health_inventory_ の試験（29 件）
task の受け入れ条件 2（5 件以上）に対し 29 件（task-core 5・task-ops 12・task-dispatch 12）。各試験が守る不変条件と、既存の試験で検出できない理由は各 unit の記録にある。

### 既定で無効
- `health_inventory_disabled_by_default`（`crates/task-ops/src/cron_jobs/health_inventory_tests.rs`）: job の無い store は発火しない。`enabled` を省いた seed と同じ一時停止の job も発火しない。`config/*.toml` の例のどれにも `health_inventory` が無い。
- `health_inventory_without_sink_fails_fire`: daemon の tick 以外（`NoCronInputSink`）の発火は `error` を履歴に残し、task を作らない。手動実行は拒否する。

### 未変更 repo を走らせない・差分検査
- `health_inventory_skips_unchanged_repo`（`crates/task-dispatch/src/health_review/inventory_tests.rs`）: tree が同じ別の commit では検査器を走らせず `unchanged` の行だけを足す。tree が変われば評価し、base は最後に評価できた行の head。
- `health_inventory_first_run_records_without_filing`: 初回は base = head で評価し、行を 1 件足すだけで task を作らない。
- `health_inventory_evaluates_previous_head_as_base`: verdict Error の行を飛ばし、その前の評価できた行の head を base にする。
- `health_inventory_selects_checker_by_applicability`: base の検査器 → 同梱の検査器 → Error（`checker_missing`）の順、remote workspace は `not_applicable`。
- `health_inventory_details_come_from_same_run`: 検査器は 1 repo につき 1 回で、起票の説明は同じ実行の report から来る。
- `health_inventory_records_counts`（`crates/task-dispatch/src/health_review/inventory_filing_tests.rs`）: 3 回目（tree が同じ）の発火は 2 repo とも `unchanged` で起票 0。

### 同じ fingerprint の負債を重複して起票しない
- `health_inventory_does_not_file_same_debt_twice`（`crates/task-ops/src/health_debt/tests.rs`）: 同じ repo と fingerprint の終端でない debt があれば、同じ head でも別の head でも 2 件目を作らない。
- `health_inventory_does_not_file_same_debt_twice_across_fires`（`inventory_filing_tests.rs`）: head を 2 回進めた発火でも debt の task と冪等キーは 1 件のまま。
- `health_inventory_refiles_after_debt_task_closed`: debt が終端になった後は別の head でだけ起票し直す。接頭辞の同じ別の fingerprint は塞がない。
- `health_inventory_files_only_checker_blocking`・`health_inventory_exception_finding_not_filed`: 起票の対象は検査器の `blocking` だけ。例外の付いた finding は起票しない。
- `health_inventory_files_nothing_unless_fail`: verdict が Fail でなければ task も冪等キーも作らない。
- `health_inventory_caps_filing_at_three`: 1 回の発火の起票は 3 件まで。`health_inventory_records_counts` は repo をまたぐ枠（3 件と残り 1）を確かめる。

### 書き換えを自動で始めない・LLM を呼ばない
- `health_inventory_files_drafts_only`（`health_debt/tests.rs`）: 起票は draft の 1 件で、親・子・依存・段の名指しを持たず、objective は自分の finding の path だけ。
- `health_inventory_files_drafts_only_without_rewrite`（`inventory_filing_tests.rs`）: `execution_plans`・`work_units`・`runs`・`chat_runs`・`chat_messages` の行は 0。repo の ref・HEAD・作業ツリー・worktree・stash は発火の前後で同じ。
- `health_inventory_does_not_call_llm`（`crates/task-dispatch/src/dispatcher/tests/health_inventory.rs`）: tick の発火から起票まで通して `runs`・`chat_runs`・`chat_messages` の行の数が変わらない。debt は dispatch されない。
- `health_inventory_fires_through_the_sink_once_per_time_and_never_creates_a_task`（`health_inventory_tests.rs`）: sink は予定時刻ごとに 1 回、task は作らない。

### 種類の検証・重ね掛け・記録の表
- `health_inventory_requires_project_id`・`health_inventory_rejects_task_fields_and_overlap`・`health_inventory_shares_input_kind_validation`（`health_inventory_tests.rs`）: `extra.project_id` 必須、task の欄 9 つと `overlap = queue` は作れない、3 種類の検証の文が同じ。
- `health_inventory_busy_while_running`・`health_inventory_running_mark_counts_in_flight_and_skips_overlap`: 走っている間の発火は `skipped_overlap`、印は `in_flight` に数える。
- `health_inventory_store_appends_only`・`health_inventory_store_roundtrips_evidence`・`health_inventory_store_latest_evaluated_skips_error_and_unchanged`・`health_inventory_store_derives_fire_counts`・`health_inventory_store_rejects_inconsistent_evidence`（`crates/task-core/src/health_inventory/tests.rs`）: 追記だけ、証跡の往復、Error と省略の行を次の base に使わない、発火ごとの集計、`evaluated` と証跡の有無の一致。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-core -p task-ops -p task-dispatch --no-tests=fail -E 'test(/::health_inventory_[a-z0-9_]+$/)'` → exit 0、29 passed。
- `sh scripts/dev/check-doc-links.sh` → exit 0。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `bash scripts/dev/test-affected.sh --list` → 対象の crate 無し（変更は文書だけ）。
- `cargo fmt --all -- --check` → exit 0。
- `git diff --check` → exit 0。範囲（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変更は `docs/ops/cron-jobs.md` とこの file だけ。
- コードを変えていないこと: `git diff --stat "$CELERIS_WU_BASE" -- crates web scripts config` → 差分 0。

### 検査の結果
- `bash scripts/dev/test-affected.sh` → exit 0（97 秒）。「試験対象の変更なし（docs のみ等）」で crate の試験は走らない。冒頭の check-health は verdict pass・exit 0・fail_grade 0・new 1・worsened 0・existing 717、`repo_health_gate: all checks passed`。new の 1 件は fail 級でない（fail_grade 0）。この unit はコードを変えていない。`reviewable=false` は `$CELERIS_WU_BASE` が HEAD の祖先でないため（前の unit の記録と同じ）。
- `cargo clippy --workspace -- -D warnings` → exit 0。

## ADR との違い（ADR 本文は G3 の closeout が付記する）
- 省略の鍵（D6.4）: ADR は `(head_tree, policy_sha256, checker.sha256)` と書くが、実装は `(head_tree, checker.sha256)`。`policy_sha256` は検査器の report からしか求まらず、鍵に入れると省略の判定のために検査器を走らせることになる。policy は base の tree から読まれ、base は前回の head なので、tree が同じなら policy も同じになる（unit runner）。
- base の選び方（D6.3）: base は「前回の棚卸しの HEAD」でなく「同じ job で最後に評価できた行（`evaluated` かつ verdict が Error でない）の head」。Error（HEAD を読めない・検査器が無い・`Unavailable`）と `unchanged`・`not_applicable` の行は次の base に使わない（unit store・runner）。
- 記録（D6.5）: 発火ごとの数（repo・評価・省略・対象外・起票・既存で省いた数・枠で残した数）の正本は表 `health_inventory_runs` の行の集計（`SqliteStore::health_inventory_fire_counts`）。`CronJobRun.detail` には始めたこと（案件と Git の repo の数）だけを書く。評価は tick の外で後から終わり、履歴の 1 行は発火の時点で書かれるため（unit runner・e2e）。D6.6 の「残りは数を detail に書く」も同じで、枠で残した数は行の `limit_remaining` にある。起票の結果の `missing_detail` は表に列が無く、log の warn にだけ出す。起票に失敗した repo は行を書かない（書くと base が進み、その finding が起票されなくなる）。
- 起票の対象（D6.6・D6.7）: Rust は fail 級の判定（severity・delta・候補）をし直さず、検査器の判定の結果である `HealthEvidence.blocking` だけを起票する（判定の正本を 2 つにしない。結果は同じ集合）。同じ head のキーは debt の終端の後も再利用しない（unit filing）。
- 配置（ADR (8) の write_set との違い）: ADR (8) は棚卸しの表の読み書きを `^crates/task-core/src/store/[a-z_]+\.rs$` に置くとしたが、棚卸しの store は `crates/task-core/src/health_inventory`（`health_inventory.rs`・`health_inventory/store.rs`・`health_inventory/tests.rs`）に、migration の登録（版数と SQL の組の表 `MIGRATIONS`・`RESERVED_VERSIONS`・`migration_sql`）は `crates/task-core/src/migration_registry.rs` に置いた。理由: store/ に 1 行足すだけで repo health の `size.responsibility-growth` が worsened になる（`crates/task-core/src/store` の本体行 20025・`store::SqliteStore` の method 731 が既存の超過）。この配置で本体行は 19736、method は 730 に減った（unit store）。runner の本体は `health_review/inventory.rs` で、module は `dispatcher::cron_tick::inventory`（`#[path]`。deps.cycle を増やさないため。unit runner）。
- 有効化の手順（ADR「本番への移行」）: ADR は「Web の cron の画面で job を追加し、種類・案件・周期・重ね掛けを選んで有効にする」と書くが、今の Web の cron 画面（`web/features/cron/cron-screen.tsx`）には job を作る操作が無い（一覧・詳細・「一時停止」・「再開」・「手動実行」だけ）。そこで手順は、`celerisctl cron create`（`POST /api/v1/cron-jobs`）で停止中の job を作り、Web で雛形を確かめて「再開」を押す形にした。`[[cron.seed]]` は `cron_jobs` 表が空のときだけ読まれ、job のある本番では使えないので手順に入れていない。止めるのは ADR のとおり Web の「一時停止」。
- 種類の分岐の置き場所（D6.1）: 本体は `cron_jobs/health_inventory.rs`、共通の検証は `cron_jobs/input_kind.rs` に置き、`cron_jobs.rs` には mod 宣言・分岐・委譲だけを足した（unit kind）。

## 撤去したもの
- `crates/task-ops/src/cron_jobs.rs` の `pub fn reconcile_project_id`（`template_project_id` を呼ぶだけの wrapper）。unit kind が残し、unit alias が消して呼び出し元（`decide_post`・試験）を `template_project_id` に替えた。`git grep reconcile_project_id -- crates/` は一致無し。
- `cos_review`・`project_reconcile` の validator の中の task の欄の一覧と overlap の検査の写し（共通の `validate_input_kind` に寄せた。unit kind）。
- `store/migrations.rs` の `MIGRATION_*` の const 115 個と `migration_sql` の match、`store/mod.rs` の試験用 `use migrations::{MIGRATION_0001..0011}`（`migration_registry.rs` の表 1 つに移した。名前の再 export・別名は置いていない。unit store）。`pub const SCHEMA_VERSION`（121）は `store/migrations.rs` に残した。selfdeploy の `sd_schema_version_of_tree` がこの file から版数を読むため。
- `run_health_check` の評価の本体（`run_health_check_with_details` に移し、`run_health_check` は説明を捨てる wrapper にした）と `cos_chat/review.rs` の `rfc3339` の写し（unit runner）。

## 本番への移行
- release: 表 `health_inventory_runs` を足す migration 0121（`SCHEMA_VERSION = 121`）は、この変更を含む最初の release に入る。昇格は [docs/ops/selfdeploy.md](../../../docs/ops/selfdeploy.md) の通常の手順で、config の変更は要らない。表を足すだけで、既存の表・欄は変えない。
- 既定では job が無いので、昇格しても本番の挙動は変わらない。
- 有効化（人）: [docs/ops/cron-jobs.md](../../../docs/ops/cron-jobs.md) §9.3 の手順。`celerisctl cron create` で `--enabled false`・`--overlap skip`・週 1 回（例 `0 5 * * 1`、`Asia/Tokyo`）・雛形 `{"title":"全体棚卸し","kind":"health_inventory","project_id":"<案件の ULID>"}` の job を作り、Web の `/cron/<name>` で雛形を確かめて「再開」を押す。1 つの案件に job は 1 件だけにする。
- 無効化（人）: Web の `/cron/<name>` の「一時停止」（§9.4）。削除は §7 の `DELETE`。
- 確かめ方: §9.5（履歴の detail は始めたことだけ、数は表 `health_inventory_runs` を読み取り専用で集計、起票は受信箱の「下書きの受け入れ」）。

## 未解決
- 枠（1 回 3 件）で残した finding は、次の発火で base が進むので `existing` になり、後から自動では起票されない（ADR D6.6 の設計のまま。数は行の `limit_remaining` に残る）。後から拾うには人が行の数を見て起票するか、枠の扱いを ADR で決め直す（unit e2e）。
- 前回の head の commit が repo から消えた（force push・gc）場合、評価は毎回 Error になり、base は進まない。人が job を作り直すまで続く（unit runner）。
- Busy の印はプロセス内メモリだけ。daemon の再起動の後は、再起動の前に走っていた棚卸しを走っていないものとして扱う（unit runner）。
- Web の cron 画面は、`health_inventory` の発火の outcome `created` を「タスクを作成」と表示する（`web/features/cron/cron-screen.tsx` の outcome の文言は種類で分けていない）。task の link は出ないが、文言は実際と合わない。§9.5 に注記した。`cos_review`・`project_reconcile` も同じかは未確認。
- 表 `health_inventory_runs` を読む API と画面は無い。確かめ方は DB の読み取り専用の SQL（§9.5）。

## 提案
- review 方針の高 risk の path class `store` は `^crates/task-core/src/store/` と `^crates/task-core/migrations/` だけを指し、この unit が置いた `crates/task-core/src/health_inventory`（表の読み書き）と `crates/task-core/src/migration_registry.rs`（migration の登録）はこの class から外れる。2 つの path（`^crates/task-core/src/health_inventory`・`^crates/task-core/src/migration_registry\.rs$`）を `store` の class に足す policy だけの branch を提案する。branch は ADR やコードと混ぜず（混ぜると check-health が policy の変更と他の変更の混在で exit 1 になる）、人の承認の後に operator が main に merge する。
- G3 の closeout が ADR (6) D6.3〜D6.6・(8)・「本番への移行」に、上の「ADR との違い」を付記する。
- Web の cron 画面に、task を作らない種類の outcome を「入力を積んだ」などの別の文言で出す変更を検討する（今回の範囲外。コードは変えていない）。
