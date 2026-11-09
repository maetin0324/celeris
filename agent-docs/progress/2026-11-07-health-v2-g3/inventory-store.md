---
title: 全体棚卸しの追記専用 store
tasks: [01M6ZDF99MKHFN57PN0H7G6Z9Z]
status: done
updated: 2026-11-09
---
# 全体棚卸しの追記専用 store

## 変更

- `phase 1: add append-only health inventory store` — migration 0121（表 `health_inventory_runs`）、SqliteStore の固有 method、HealthBlockingDetail、専用試験を追加した。
- `phase 1: move inventory store and migration registry out of store module` — 棚卸しの型・method・試験を store module の外へ移し、migration の登録を表 1 つにして `crates/task-core/src/migration_registry.rs` へ移した。
- 棚卸し: `crates/task-core/src/health_inventory.rs`（型 `HealthInventoryRun`・`HealthInventoryOutcome`・`HealthInventoryFireCounts`）、`health_inventory/store.rs`（`impl SqliteStore` の `health_inventory_append`・`health_inventory_latest_evaluated`・`health_inventory_fire_counts` と読み出しの補助 1 つ）、`health_inventory/tests.rs`（試験 `health_inventory_store_*` 5 件）。更新・削除の method は無い。TaskStore と supertrait は変えていない（`store/task_store.rs` の差分は 0）。
- migration の登録: `migration_registry::MIGRATIONS`（版数と SQL 本体の組の表。115 行、版数の昇順）、`RESERVED_VERSIONS`、`migration_sql`（表を引くだけで、無い版数は `None`）。`store/migrations.rs` には `pub const SCHEMA_VERSION: u32 = 121;` と適用の処理（`apply_migration_version`・backfill）だけが残り、`include_str!` と `MIGRATION_*` の const は無い。移した名前の再 export・別名は置いていない。試験は `migration_sql(N).unwrap()` と `crate::migration_registry::RESERVED_VERSIONS` を直接使う。
- 0018 の注釈（`knowledge_runs`）は元の file で 0020 の const に付いていたので、表では 0018 の行に付け直した。
- HealthBlockingDetail は `required_checks/health.rs` にあり、欄は fingerprint・rule_id・paths・message だけ（判定の欄は無い。判定の正本は `HealthEvidence.blocking`）。
- owner は地図の「SQLite 永続化（facade + 領域別 impl）」（task-core::store）。設計の根拠は ADR 2026-11-08-health-enforcement-loop (6) D6.1〜D6.7。ADR と policy は変えていない。

## 実装前 brief

1. 根本原因: repo ごとの棚卸しの履歴の表と読み出し口が無い。前回の配置（`store/health_inventory.rs`）は repo health の size.responsibility-growth で worsened になった。単位は `<pkg>/src/<src 直下の第 1 要素>` で、base で `crates/task-core/src/store` は本体行 20025、`store::SqliteStore` は method 731 の既存超過のため、store/ に 1 行足すだけで worsened になる。
2. 責務 owner: 表の読み書きは task-core の領域 module（chat・browser_wait と同じ形）、migration の登録は task-core の migration の表。適用の処理は store。
3. 既存の再利用先: `SqliteStore::lock`・`with_read_conn`・`format_rfc3339`・`parse_rfc3339`、HealthEvidence の serde、`chat/store.rs`・`browser_wait/sql.rs` の配置。
4. 選択肢: (a) store/ の下の dir に分ける（単位が同じで減らない。不採用）、(b) policy の例外（policy だけの branch と人の承認が要る。不採用）、(c) 領域 module に出し、migration の登録も store の外の表へ移す（採用。store の本体行と method 数が減る）。
5. 新しい状態・分岐・fallback・port: 表 `health_inventory_runs` だけ。fallback・切替は無し。
6. 撤去対象: `store/migrations.rs` の `MIGRATION_*` の const 115 個と `migration_sql` の match、`store/mod.rs` の試験用 `use migrations::{MIGRATION_0001..0011}`、`store/health_inventory.rs`・`store/health_inventory_tests.rs`。
7. 不変条件: 追記だけ、証跡の全欄の往復、Error と省略の行を前回の評価に使わない、時刻は値で比べる（試験 `health_inventory_store_*`）。migration は全版が同じ順で当たる（既存の store・cron・cluster_job・delivery・roadmap・routing_log・script_domains・client_open の試験）。既存試験にはこの表の契約が無いので専用試験で固定した。

## 配置の理由と ADR (8) との違い

ADR (8) は inventory の write_set を `^crates/task-core/src/store/[a-z_]+\.rs$`（`store/health_inventory.rs`）と書いている。これは G1 の検査器（size.responsibility-growth）が入る前の配置の前提で、その配置では store module の本体行と SqliteStore の method 数が増えて check-health が exit 1 になる。そこで棚卸しを `crates/task-core/src/health_inventory`（単位が別）に、migration の登録を `crates/task-core/src/migration_registry.rs`（単位が別）に置いた。結果として store module の本体行は 20025 → 19736、`store::SqliteStore` の method は 731 → 730 に減った（`migration_sql` を表の関数にしたため）。ADR 本文への付記は G3 closeout の担当とする。

## review 方針の path class から外れること

review 方針の高 risk の path class `store` は `^crates/task-core/src/store/` と `^crates/task-core/migrations/` だけを指す。新しい 2 つの module（`crates/task-core/src/health_inventory.rs` と `health_inventory/`、`crates/task-core/src/migration_registry.rs`）はこの path class から外れ、変えても設計 brief と設計 review の対象にならない。migration の SQL 本体（`crates/task-core/migrations/`）は引き続き class に入る。

## 既存試験の更新

migration 0121 に伴い、版数を固定した試験（store/tests.rs・route_records.rs・routing_evidence.rs・credential_wait_tests.rs・routing_log_tests.rs・cron/store_tests.rs・cluster_job/tests.rs・delivery.rs）を 120 から 121 に、未来版の拒否試験を 122 に直した。旧版 DB を作る roadmap・script_domains・cron・cluster_job の fixture は新しい表を消してから移行履歴を戻す。`crates/campaign/src/ops_r1_trace.rs` の `SUPPORTED_SCHEMA_VERSIONS` に 121 を足し、doc に 121 の 1 文を足した（表を足しただけで export が読む表・欄は同じ）。登録の移動に伴い、試験の `MIGRATION_*` と `migrations::RESERVED_VERSIONS` の参照を `migration_registry` に替えた（store/tests.rs・script_domains_tests.rs・client_open_tests.rs・routing_log_tests.rs・feed/tests.rs）。検査する契約は変えていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0。verdict pass、new 0、worsened 0、existing 718。store module の本体行 19736、`store::SqliteStore` の method 730。検査器の unit 試験 73 件と gate の試験 7 件も合格。
- `cargo nextest run -p task-core health_inventory_` → exit 0、5 passed。`cargo nextest list` の件数 5（4 以上）。
- `cargo nextest run -p task-core` → exit 0、1338 passed。
- plan の file 検査（migration 1 つ・表・HealthBlockingDetail・2 module・`include_str!` 無し・`SCHEMA_VERSION` が store/migrations.rs・`sd_schema_version_of_tree .`）→ exit 0、`schema=121 max=121`。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/task-core/src/store/task_store.rs scripts/selfdeploy` → exit 0。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` と plan の正規表現）→ 範囲外の path 無し。
- `bash scripts/dev/test-affected.sh` → exit 0（15 分 35 秒）。冒頭の check-health 合格、cargo nextest 8042 passed・23 skipped、doc test 合格、web の offline install・`tsc -b`・unit 試験 104 file 711 passed、test-ts-verify 20 passed、script bundle の SKIP 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/progress-index.sh --check`・`sh scripts/dev/check-adr-numbers.sh`・`git diff --check` → exit 0。

## 本番への移行

この unit は schema の追加だけで、cron を有効化しない。本番の操作はしない。有効化の人の手順は inventory の record unit が記録する。

## 経緯

- 一度目の run は `store/health_inventory.rs` に置き、check-health の worsened 2 件（SqliteStore の method 735、store module の本体行 20176）と範囲 check の外の試験の更新で止まった。plan_issue で申告し、親の replan でこの配置になった。

## 未解決

無し。前回の run で落ちた web の offline install は、今回の test-affected では成功した。

## 提案

- policy だけの branch で、review 方針の `store` の path class に `^crates/task-core/src/health_inventory` と `^crates/task-core/src/migration_registry\.rs$` を足す。どちらも SQLite の表と migration の登録を持ち、store と同じ高 risk の変更になる。branch は ADR やコードと混ぜず、人の承認の後に operator が main に merge する。
- ADR 2026-11-08-health-enforcement-loop (8) の inventory の行（`^crates/task-core/src/store/[a-z_]+\.rs$`）を、G3 closeout がこの配置（`health_inventory`・`migration_registry.rs`）に合わせて付記する。
