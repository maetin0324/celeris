---
title: 全体棚卸しの追記専用 store
tasks: [01M6ZDF99MKHFN57PN0H7G6Z9Z]
status: blocked
updated: 2026-11-09
---
# 全体棚卸しの追記専用 store

## 変更

- `phase 1: add append-only health inventory store` — migration 0121、SqliteStore の固有 method、HealthBlockingDetail、専用試験を追加。
- owner は地図の「SQLite 永続化（facade + 領域別 impl）」（task-core::store）。設計の根拠は ADR 2026-11-08-health-enforcement-loop (6) D6.1〜D6.7 と (8)。ADR は変更しない。
- 履歴は `store::health_inventory::HealthInventoryRun`。`health_inventory_append`、`health_inventory_latest_evaluated`、`health_inventory_fire_counts` を公開。TaskStore と supertrait は変更しない。
- outcome は evaluated・unchanged・not_applicable。evaluated のみ HealthEvidence を serde で保存し、repo id の不一致は拒否する。verdict の列、別の証跡形式、集計の保存先は足さない。
- 最新評価は予定時刻、作成時刻、追記順で選び、Error と省略行を除く。時刻は読み出して OffsetDateTime で比較する。
- HealthBlockingDetail は fingerprint・rule_id・paths・message のみ。判定は HealthEvidence.blocking のまま。

## 実装前 brief

1. 原因: repo ごとの棚卸し履歴の表と読み出し口が無い。
2. owner: task-core の store・migration・共通 health 型。
3. 再利用: SqliteStore 固有 method、時刻変換、HealthEvidence の serde。
4. 選択: 別の集計表を置かず履歴行から集計する。
5. 新状態: 指定された追記専用表のみ。fallback は無し。
6. 撤去対象: 無し（既存機構の置換ではない）。
7. 不変条件: 追記だけ、証跡の全欄の往復、失敗を前回評価に使わない、時刻の意味で比較する。既存試験にはこの表の契約が無いので専用試験で固定する。

## 既存試験の更新

migration に伴い、store・cron・cluster_job・delivery の試験の schema 期待値を 120 から 121 に更新した。未来版の拒否試験は 122 に更新した。旧版 DB を作る roadmap・script_domains・cron・cluster_job の fixture は、新表を消してから移行履歴を戻す。検査する契約と保持行の検証は変更していない。これらは新 migration で失敗した既存試験の修正であり、cron の種類や他 unit の本体は変更していない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-core -E 'test(health_inventory_)'` → exit 0、5 passed。
- `cargo nextest run -p task-core` → exit 0、1338 passed（専用試験5件を含む）。
- `cargo test -p task-core --doc` → exit 0、3 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0（vendor deno_core の既存 deprecated warning は出る）。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-affected.sh --list` → exit 0。task-core と逆依存、web 型検査・unit、test-ts-verify が対象。
- `bash scripts/dev/test-affected.sh` → exit 1。冒頭の check-health で停止し、逆依存の試験には進んでいない。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 1。size.responsibility-growth の worsened 2 件。SqliteStore の method 数 735（閾値 60）、store module の本体行 20176（閾値 8000）。file の分割では合算値は減らない。
- `pnpm install --offline --frozen-lockfile`（web）→ exit 1。collapsible の package が offline store に無い。型検査・unit は未実行。ネットワークは使っていない。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → exit 0。
- `git diff --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0。指定成果と、新 migration に伴う既存試験の修正だけ。

## ADR との差分

D6.5 の列を objective の指定どおり outcome・既存で省いた数・上限で残した数・作成時刻で具体化した。省略 repo も証跡無しの行に残す。発火の件数は本表から導出する。ADR 本文への反映は G3 closeout の担当。

## 本番への移行

本 unit は schema の追加だけで、cron を有効化しない。本番の操作はしない。有効化の人の手順は inventory-record unit が記録する。

## 未解決

完了は宣言しない。WU 基準でも、必須の SqliteStore 固有 method と store module 本体の増加が size.responsibility-growth に抵触する。既存の大きな store を縮小するには、この unit の範囲外の owner の変更が要る。policy を変える場合も、別の policy 専用 branch と人の承認が必要になる。check を迂回せず、result.json の yield.plan_issue で親へ再計画を申告する。

web の依存不足も未解決。必要な package を用意した環境で、逆依存と web を含む test-affected を再実行する必要がある。

## 二度目の run（attempt 1、2026-11-09）の確認

前回の run の後の check 2 件の不合格を確かめた。どちらも成果の直し方では通らない計画の問題で、yield.plan_issue で申告した。

- 範囲 check: migration を 1 つ足すと SCHEMA_VERSION が 121 になり、`crates/task-core/src/cluster_job/tests.rs`・`crates/task-core/src/cron/store_tests.rs`・`crates/task-core/src/delivery.rs`（`#[cfg(test)]` の試験）の `assert_eq!(SCHEMA_VERSION, 120)` が落ちる。直さないと test-affected が落ち、直すと範囲 check が落ちる。
- check-health: size.responsibility-growth の単位は `<pkg>/src/<src 直下の第 1 要素>` で、`store/` の下の dir に分けても単位は `crates/task-core/src/store` のまま。base で module の本体行 20025（閾値 8000）、`store::SqliteStore` の method 731（閾値 60）。`store/` に 1 行でも足すと worsened になる。新しい migration に必ず要る `store/migrations.rs` の `121 =>` の 3 行だけでも worsened になる。ADR (8) の write_set（`^crates/task-core/src/store/[a-z_]+\.rs$`）は G1 の検査器が入る前の配置の前提で書かれている。

## 提案

親の計画に store の責務縮小を担当する unit を置き、method 数と module 本体行の既存超過を減らしてから本 unit を再検査する。別の方針を採るなら、指定する store API と健全性 policy の整合を設計で決める。migration に伴う cluster_job/tests.rs・cron/store_tests.rs・delivery.rs の試験更新も範囲 check に含める。

具体案（plan_issue と同じ）:

1. 範囲 check に `crates/task-core/src/(cluster_job/tests|cron/store_tests|delivery)\.rs` を足す。
2. 棚卸しの SqliteStore の impl を `store/` の外の領域 module に置く（既存の例: `chat/store.rs`・`browser_wait/sql.rs`）。単位が別になり、method 3 件と本体行は閾値の下に収まる。
3. migration の登録の 3 行は 2 の後も store の worsened になる。人の決定で、(a) policy だけの branch で例外を置くか、(b) migration の登録（`store/migrations.rs`）を `store/` の外の module に移す unit を先に置くかを選ぶ。どちらも ADR (8) との違いを G3 closeout が付記する。
