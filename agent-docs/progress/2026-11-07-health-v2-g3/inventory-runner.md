---
title: 健全性v2 G3 inventory-runner — 全体棚卸しの runner（適用判定・差分検査・未変更 repo の省略・Busy の印）
tasks: [01M6ZDF99MKHFN57PN0H7G6Z9Z]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 inventory-runner — 全体棚卸しの runner（適用判定・差分検査・未変更 repo の省略・Busy の印）

完了日: 2026-11-09

WorkUnit `runner`（task 01M6ZDF99MKHFN57PN0H7G6Z9Z、unit inventory の子）。[G3 の ADR](../../adr/2026-11-08-health-enforcement-loop.md) の (6) D6.1・D6.3〜D6.5 と (8) に従う。基準は `$CELERIS_WU_BASE`。

## 実装前 brief
1. 根本原因: cron の種類 `health_inventory`（unit kind）は sink の method `post_health_inventory` を呼ぶが、daemon の sink（`CosReviewSink`）は既定の Err のままで、棚卸しを走らせる runner が無い。検査器の実行の口 `gate::run_health_check` は証跡しか返さず、起票の文面に要る blocking の説明を同じ実行から取れない。
2. 責務 owner: 検査器の実行と report の解釈は `health_review::gate`、適用判定は `task_core::required_checks::health`、表は `task_core::health_inventory`、cron の sink は `dispatcher::cos_chat::review`。runner の本体は `health_review/inventory.rs`。
3. 既存の再利用先: `repo_facts`・`applicability`（delivery の `check_required_evidence` と同じ呼び方）、`gate::run_health_check` の評価の本体、`policy::git_stdout`、`task_ops::changes::resolve_default_branch`（既定の branch の唯一の解決経路）、`SqliteStore::health_inventory_append`・`health_inventory_latest_evaluated`・`health_inventory_fire_counts`、`gate_tests` の stub の検査器。
4. 選択肢: (a) runner が report を自分で読み直して説明を作る（解析が 2 つになる。不採用）、(b) gate に説明も返す口を足し、`run_health_check` をその wrapper にする（採用）。Busy の印は (a) store の行で判定（実行中を表せない。不採用）、(b) Dispatcher の欄に JoinHandle を持つ（採用。`in_flight` に数えられる）。
5. 新しい状態・分岐・fallback・port: Dispatcher の欄 `health_inventory: InventoryRuns`（プロセス内メモリの印だけ。正本は表の行）。fallback・flag は無し。
6. 撤去対象: `run_health_check` の評価の本体（`run_health_check_with_details` に移し、`run_health_check` は説明を捨てる wrapper）。`cos_chat/review.rs` の `rfc3339` の写し（`dispatcher.rs` の同じ関数を使う）。
7. 不変条件: 下の試験。

## 変更
- `crates/task-dispatch/src/health_review/gate.rs`: `run_health_check_with_details`（証跡と `HealthBlockingDetail` の列）を足した。`judge` が読んだ同じ report の finding から、`blocking` の fingerprint ごとに rule_id・evidence の path（重複を除く）・rationale を写す。`run_health_check` はこれを呼んで説明を捨てる。report の finding に `rationale` の欄を読む。
- `crates/task-dispatch/src/health_review/inventory.rs`（新規）: `InventoryRuns`（job id → `JoinHandle`）、sink の口 `post`（`CosReviewSink` から 1 行で委譲）と `start_inventory`、`InventoryFire::{run, inventory_repo, record}`。案件の `RepoKind::Git` の repo ごとに、既定の branch の HEAD を `git rev-parse` で読むだけで ref・作業ツリーは変えない。検査器は `repo_facts` → `applicability` だけで選ぶ。`NotApplicable` は `not_applicable` の行、`Unavailable` と HEAD を読めない repo は verdict Error の `evaluated` の行（次の base に使われない）。base は最後に評価できた行の head_sha、初回は base = head。`(head_tree, checker.sha256)` が最後に評価できた行と同じなら `unchanged` の行。実行は `spawn_blocking` で、行は repo ごとに 1 件。起票の欄は 0（起票は unit e2e が繋ぐ）。LLM・run・chat は起こさない。
- `crates/task-dispatch/src/dispatcher/cron_tick.rs`: `#[path = "../health_review/inventory.rs"] mod inventory` と、sink に `inventory: &mut self.health_inventory` を渡す 1 行。
- `crates/task-dispatch/src/dispatcher/cos_chat/review.rs`: sink の欄 `inventory` と `post_health_inventory` の 1 行の委譲、型の別名 `Post`。
- `crates/task-dispatch/src/dispatcher.rs`: 欄 `health_inventory`・初期化・`in_flight` の 1 項。
- `crates/task-dispatch/src/health_review.rs`: 説明の 2 行だけ（mod 宣言は cron_tick にある）。
- 試験: `health_review/inventory_tests.rs`（新規、6 件）、`dispatcher/tests/health_inventory.rs`（新規、1 件）、`dispatcher/tests/mod.rs` に mod 1 行。`gate_tests.rs` は stub に起動の数を数える `count` を足し、使い回す fixture を `pub(crate)` にした（判定の試験は変えていない）。

## size と依存の扱い
- `dispatcher` module の本体の行は閾値を超えた既存の単位なので、配線で足した行を同じ file の整理で相殺した（`size_growth._rust` の数え方で dispatcher.rs 1643 → 1641、cron_tick.rs 45 → 47、cos_chat/review.rs 251 → 247、合計 −4）。
  - `in_flight` の CoS の項を既存の `cos_in_flight()` で数える（`running` の CoS でない run + `cos_in_flight()` は `running.len()` + CoS chat の run と同じ値）。
  - `cos_chat/review.rs` の `rfc3339` を `dispatcher.rs` の同じ関数（`unwrap_or_default`）に替えた。
  - `post_project_reconcile` の委譲を `use` で短くした（呼ぶ関数と引数は同じ）。
- runner の file は `health_review/` に置くが、module は `dispatcher::cron_tick::inventory`（`design_gate.rs`・`policy_review.rs` と同じ `#[path]` の形）。`dispatcher` から `crate::health_review::…` を参照すると、`health_review` → `dispatcher` の既存の辺と合わせて deps.cycle が 3 単位から 4 単位に悪化したため（一度目の check-health で確認）。

## 試験
守る不変条件と、既存の試験で検出できない理由（既存の試験は cron の種類・表・起票を別々に見るだけで、runner の base の選び方・省略・検査器の選択・説明の出どころ・Busy の印を通さない）:
- `health_inventory_first_run_records_without_filing`: 評価できた行が無い初回は base = head で評価し、行を 1 件足すだけで task を作らない。
- `health_inventory_skips_unchanged_repo`: tree が同じ別の commit では検査器を走らせず `unchanged` の行だけを足す。tree が変われば評価し、base は最後に評価できた行の head。
- `health_inventory_evaluates_previous_head_as_base`: verdict Error の行を飛ばし、その前の評価できた行の head_sha を base にする。
- `health_inventory_busy_while_running`: 同じ job が走っている間は `Busy`、`in_flight` は 1。終われば次の発火が始まり行が残る。store が無ければ Err。
- `health_inventory_details_come_from_same_run`: 検査器は 1 回だけ走り、説明の fingerprint が `blocking` と同じ順で一致し、path・rationale が report の finding から来る。
- `health_inventory_selects_checker_by_applicability`: base に検査器があれば同梱より base、無ければ同梱、どちらも無ければ Error（`checker_missing`）、remote workspace は検査器を走らせず `not_applicable`。
- `health_inventory_running_mark_counts_in_flight_and_skips_overlap`（dispatcher）: 印は Dispatcher の欄で `in_flight` に数え、tick の発火は `skipped_overlap`。終わった後の発火は `created`（detail は repo の数）。

## ADR との違い（ADR 本文は G3 closeout が直す）
- D6.4 の省略の鍵は `(head_tree, policy_sha256, checker.sha256)` だが、`(head_tree, checker.sha256)` にした。`policy_sha256` は検査器の report からしか求まらず、鍵に入れると省略の判定のために検査器を走らせることになる。base は前回の head なので、tree が同じなら base と head の tree は同じで、policy は同じ tree から読まれ、new・worsened の finding は出ない。
- D6.5 の `CronJobRun.detail` には始めたこと（案件と repo の数）だけを書く。評価は tick の外で後から終わるので、評価・省略・対象外・起票の数の正本は表の行（`health_inventory_fire_counts`）にした。
- D6.3 の「案件の Git の repo」は `RepoKind::Git` の repo とした（`dir` の repo は行を作らない）。HEAD を読めない repo と `Unavailable` は verdict Error の `evaluated` の行にした（ADR (1) D1.2 の「`Unavailable` は error」と同じ。次の base には使われない）。`NotApplicable` の理由は表に列が無いので log にだけ出す。
- runner の module の位置は上の「size と依存の扱い」のとおり（ADR (8) の `health_review/` の path に収まる）。
- 行の `created_at` は発火の時刻（注入した時計）。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/health_inventory_|health_evidence_gate|cron/)'` → exit 0、37 passed（`health_inventory_` 7 件、gate の 9 件、cron の既存の試験）。
- `bash scripts/dev/test-affected.sh` → exit 0（654 秒）。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。冒頭の check-health は verdict pass・exit 0・fail_grade 0・worsened 0（new 1 件はこの unit で変えていない path の finding）、`repo_health_gate: all checks passed`。cargo nextest 3614 passed・11 skipped、doc test 合格、script bundle の SKIP 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check`・`sh scripts/dev/check-doc-links.sh`・`git diff --check` → exit 0。
- 範囲（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変更は `crates/task-dispatch/src/{dispatcher.rs,dispatcher/cron_tick.rs,dispatcher/cos_chat/review.rs,dispatcher/tests/{health_inventory,mod}.rs,health_review.rs,health_review/{gate,gate_tests,inventory,inventory_tests}.rs}` とこの file だけ。

## 未解決事項
- 起票（`task_ops::health_debt::file_health_debts`）は繋いでいない（unit e2e）。`RepoInventory.details` と `record` の間に入れる。
- 前回の head の commit が repo から消えた（force push・gc）場合、評価は毎回 Error になり、base は進まない。人が job を作り直すまで続く。
- Busy の印はプロセス内メモリだけ。daemon の再起動の後は同じ job を走っていないものとして扱う（再起動の前の発火の行は残り、次の発火で未変更の repo は省略される）。

## 提案
- G3 closeout が ADR (6) D6.4・D6.5 に上の違いを付記する。

## 本番への移行
- この unit では無し（config・表・API の変更は無い）。job は既定で無いので本番の挙動は変わらない。有効化の手順は unit record が書く。
