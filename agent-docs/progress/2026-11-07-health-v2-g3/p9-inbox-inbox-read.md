---
title: 健全性v2 G3 P-9 inbox-read — 各 task の最新の DependentsReopened を events の全走査なしで読む
tasks: [01M71K2HHSJ20836QAMXJ0YMEW]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 G3 P-9 inbox-read — 各 task の最新の DependentsReopened を events の全走査なしで読む

WorkUnit `inbox-read`（P-9 (c) の前段）。ADR [2026-11-08-health-enforcement-loop](../../adr/2026-11-08-health-enforcement-loop.md) の D7.3 の範囲。受信箱 `dependents_reopened` の正本は各 A の最新の `Event::DependentsReopened` で、受信箱の組み立てはこれを 1 回の問い合わせで読む。基準は `$CELERIS_WU_BASE`。受信箱の項目そのものは後続の unit `inbox` が持つ。

## 変更
件名「P-9 inbox-read: 最新行の読み出しを種類で一般化し、dependents_reopened の部分 index を足す」の commit。
- `crates/task-core/src/store/events.rs`:
  - 種類の enum `LatestEventKind { DeliverySkipped, DependentsReopened }`。`index_predicate` が migration の部分 index と同じ WHERE 式を返す。`store` の `pub use` で `task_core::store::LatestEventKind` として出す。
  - `latest_event_rows_sql(kind)` と module の関数 `latest_event_rows(conn, kind)`。`SqliteStore` の method にしない。
  - 旧い `DELIVERY_SKIPPED_PREDICATE`・`latest_delivery_skipped_sql`・`SqliteStore::latest_delivery_skipped_rows_impl` を消した。
  - 行を `EventRow` にする処理を `read_event_rows` の 1 か所に畳んだ。使う所は `latest_event_rows`・`integration_request_rows_tx`・`events_since_impl`・`event_rows_for_impl` の 4 か所。読めない event の誤りは、どこでも `events_since_impl` と同じ文（event の id と task を含む `StoreError::Invalid`）にした。
- `crates/task-core/src/store/task_store.rs`・`task_store_impl.rs`: trait method `latest_delivery_skipped_rows()` を `latest_event_rows(kind)` に置き換えた。method の数は変わらない。
- `crates/task-ops/src/inbox.rs`: `build_attention` の呼び出しを `latest_event_rows(LatestEventKind::DeliverySkipped)` にした（関数の行は 311 のまま）。
- `crates/task-core/migrations/0122_events_dependents_reopened_index.sql`: `CREATE INDEX IF NOT EXISTS idx_events_dependents_reopened ON events(task_id, seq) WHERE json_extract(json,'$.type')='dependents_reopened'`。
  - `IF NOT EXISTS` は 0119 と同じ。版を巻き戻して当て直す試験の土台（`cluster_job/tests.rs` の `version >= 34`、`celerisctl/tests/no_migrate.rs` など）は、index を消さずに 0122 を当て直す。
  - 既存の migration は変えていない。
- `migration_registry.rs` の `MIGRATIONS` に 122 を足し、`store/migrations.rs` の `SCHEMA_VERSION` を 122 にした。
- 版の固定を 122 に直した: `delivery.rs`（`assert_eq!` と適用済みの版の一覧）、`cron/store_tests.rs`、`cluster_job/tests.rs`、`store/` の `tests.rs`（8 か所）・`routing_log_tests.rs`（2 か所）・`roadmap_tests.rs`・`credential_wait_tests.rs`・`route_records.rs`・`routing_evidence.rs`。
  - `roadmap_tests.rs` の 0112 の土台は、0122 の index を消し、版 122 の記録も消す（`MAX(version) = 112` の前提を保つ）。
  - `store/tests.rs` の `open_rejects_db_with_schema_version_newer_than_supported` は、「この binary より新しい版」に固定値 122 を使っていた。これを `SCHEMA_VERSION + 1` から作るようにした（試験の意味は「1 つ先の版を拒む」のまま）。
- `crates/campaign/src/ops_r1_trace.rs`: `SUPPORTED_SCHEMA_VERSIONS` に 122 を足した。版の注記に「122（migration 0122）は events に dependents_reopened の部分 index を足しただけで同じ（ADR 2026-11-08-health-enforcement-loop）」を足した。

## 実装前 brief
1. 根本原因: 受信箱 `dependents_reopened` の正本（各 task の最新の `DependentsReopened`）を、events を全件走査せずに読む口が無い。`latest_delivery_skipped_rows` は同じ意味（種類ごとの task 単位の最新 1 件）を `delivery_skipped` だけに固定していた。
2. 責務 owner: 読み出しは `store/events.rs`（events の問い合わせ）、口は `TaskStore`。部分 index は migration。
3. 既存の再利用先: `latest_delivery_skipped_sql` の問い合わせの形（副問い合わせの `MAX(seq)` と `(task_id, seq)` の結合）、migration 0037 の部分 index の形、試験 `latest_delivery_skipped_rows_uses_partial_index`。
4. 選択肢: (a) `latest_dependents_reopened_rows` を別に足す。同じ仕様を二重に実装し、method も増えるので採らない。(b) 種類を引数に取る 1 つの読み出しに置き換え、種類を enum にする。(b) を採った。enum の置き場は `crate::model` も考えた。しかし `model/events.rs` は高 risk の class `events` で、範囲の外でもある。そこで enum は読み出しと同じ `store/events.rs` に置いた。`crate::transition` に置く純粋な判定は、この unit には無い。
5. 新しい状態・分岐・fallback・port: enum `LatestEventKind`（2 種）と部分 index 1 本。fallback・切替 flag は無し。
6. 撤去対象: `TaskStore::latest_delivery_skipped_rows` と impl、`SqliteStore::latest_delivery_skipped_rows_impl`、`DELIVERY_SKIPPED_PREDICATE`、`latest_delivery_skipped_sql`。旧い名前の試験 2 件は、新しい読み出しの試験に置き換えた（下）。
7. 不変条件: 種類ごとに部分 index を使い、events を全件 SCAN しない。task ごとに最新の 1 件だけを id の昇順で返し、ほかの種類を混ぜない（下の試験）。

## 試験（守る不変条件と、既存の試験で検出できない理由）
- 共通の確かめ `assert_latest_rows_use_partial_index(kind, index, make)`（`store/tests.rs`）。守る不変条件は 2 つ。
  - (1) EXPLAIN QUERY PLAN が種類の部分 index を使い、events を読む `SCAN` はすべてその index を通る。
  - (2) task ごとに最新の 1 件だけを id の昇順で返し、同じ task に書いた別の種類の event を混ぜない。
  - 行を比べるだけの試験では (1) の退行を検出できない。index が無いと plan は `SCAN events USING INDEX sqlite_autoindex_events_1`（UNIQUE(task_id, seq) を通る全件の読み出し）に落ちるが、返る行は同じだからである。
  - 問い合わせと試験は同じ文字列 `latest_event_rows_sql(kind)` を使う。
- `dependents_reopened_latest_rows_use_partial_index`（新規）: `idx_events_dependents_reopened`。
  - migration の WHERE 式の値を変える変異（`'dependents_reopened '`）で落ちることを確かめ、元に戻した。plan は上の全件の読み出しになった。
  - 空白だけの違い（`json_extract(json, '$.type')`）では落ちなかった。SQLite は WHERE 式を空白ではなく式の木で照合するので、index は使われ続ける。
  - 「events を SCAN しない」の確かめは最初、`USING` の無い `SCAN` だけを見ていた。これでは上の全件の読み出しを見逃すので、「events の `SCAN` はすべて部分 index を通る」に直した。
- `delivery_skipped_latest_rows_use_partial_index`（旧 `latest_delivery_skipped_rows_uses_partial_index` の置き換え）: `idx_events_delivery_skipped` で、上と同じ確かめ。
  - 旧 `latest_delivery_skipped_rows_returns_latest_per_task_only` は消した。その土台（a に 2 件・b に 1 件と別の event）と確かめ（task ごとの最新）は、この試験の (2) がそのまま持つ。(2) は、行の中身の比較・件数・別の種類を混ぜないことまで確かめる。
  - 旧い名前の試験を残すと、範囲の検査 `! grep -rnE 'fn latest_delivery_skipped_rows' crates/` にも当たる。
- `human_inbox_delivery_skipped_assign_or_skip`（task-ops、既存）: 受信箱の `delivery_skipped` が新しい読み出しで合格する。

## 証拠（提出時の HEAD で実行）
- 範囲の検査（計画の check。`sh "$CELERIS_WU_SCOPE_PATHS"` の path を許可の正規表現で絞る）: 範囲外の path は 0 件。
- `cargo nextest run -p task-core -p task-ops --no-tests=fail -E 'test(/dependents_reopened_latest_rows|delivery_skipped/)'`: exit 0、3 passed。上の新しい試験 2 件と `human_inbox_delivery_skipped_assign_or_skip`。
- 版の check（`SCHEMA_VERSION` は 122、0122 の SQL に `dependents_reopened`、registry に `0122_`、`ops_r1_trace.rs` に 122）と `cargo nextest run -p celeris --no-tests=fail -E 'test(/ops_r1_trace/)'`: exit 0、5 passed。
- `bash scripts/dev/test-affected.sh`: exit 0（約 8 分半）。
  - nextest は 8173 passed・23 skipped。web は tsc -b 合格、vitest は 104 file・711 件合格。test-ts-verify は 20 passed。
  - 1 回目は `open_rejects_db_with_schema_version_newer_than_supported` が 1 件落ちた（固定値 122）。上の修正の後の 2 回目が合格した。
  - 先頭の check-health は verdict=pass、fail 級 0、worsened 0。new の 2 件（`dup.contract` の `UnavailableReason`、`dup.public-surface` の `judge`）は warning の候補で、この unit が変えていない file にある。
- `cargo clippy --workspace -- -D warnings`: exit 0（外部 crate `deno_core` の warning 1 件だけ）。`cargo fmt --all -- --check`: exit 0。
- 健全性の値（`size.responsibility-growth` と同じ数え方。check-health の base は `git merge-base HEAD main`）:
  - store の module の行: base 20026、unit event の後 19727、この unit の後 19676（unit event の後から −51）。
  - `SqliteStore` の method: base 732、unit event の後 730、この unit の後 729。
  - `build_attention` は 311 のまま。

## 未解決
- 範囲の外にあって、この unit では直せない旧い名前が 2 か所に残る。
  - migration 0037 の注釈が `latest_delivery_skipped_rows_impl` を指す。既存の migration は変えない決まりなので、そのまま残す。新しい読み出しの所在は `LatestEventKind::index_predicate` の doc comment と 0122 の注釈に書いた。
  - ADR `agent-docs/adr/0121-root-delivery-without-assignee.md` の付記が `latest_delivery_skipped_rows` を指す。

## 提案
- unit `record` が、ADR-0121 の付記に「読み出しは `TaskStore::latest_event_rows(LatestEventKind::DeliverySkipped)` に一般化した（ADR 2026-11-08-health-enforcement-loop D7.3、migration 0122）」の 1 行を足す。
- 部分 index の式の一致について、0037 と ADR-0121 の注釈は「字句まで同じ」と書いている。実際の SQLite は式の木で照合し、空白の違いは許す。規則としては厳しい側なので、そのままでよい。
