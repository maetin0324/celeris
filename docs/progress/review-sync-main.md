---
tasks: [01M408E4BX8A3FNSBJTZ0CC67A, 01M40FWV6Z6N4P5ESHZMK0KH2B]
---
# review sync: main 取り込み

merged-main: c448d9c77d18eb54396907835efb91c5d0a985e8

task branch（`e269a8c4`）に `git merge --no-ff main` で上の main を取り込んだ（rebase なし）。衝突した 16 ファイルは両側の変更を残して解いた。

| ファイル | 解き方 |
|---|---|
| `crates/celeris/tests/instance_handoff.rs` | branch 側の `STATE_WAIT` 定数で待つ形を残し、値を main 側の長い保険 120 秒に揃えた。gate 待ちの台本と説明の comment は main 側（`timeout 300` の出来事待ち）を採った |
| `crates/task-api/src/query.rs` | `EVENT_TYPES` の要素は自動 merge で両側（review target / integration repair 5 件と `delivery_skipped`）が入ったので、配列長を 54 と 50 から 55 にした |
| `crates/task-api/src/query/tests.rs` | branch 側の review target・integration repair の試験 2 件を残し、件数の検査を 55 にした |
| `crates/task-core/src/cluster_job/tests.rs` | schema を戻す SQL に両側の DROP（deliveries・node_sessions の列と `idx_events_delivery_skipped`）を並べた。`SCHEMA_VERSION` は 41 |
| `crates/task-core/src/store/migrations.rs` | 両側の登録を残した。main の `0037_events_delivery_skipped_index`（`MIGRATION_0037`）と `0041_feed_notices`、branch の 0038〜0040 を登録した。branch の `0037_review_target_sync` は `MIGRATION_0037_REVIEW_TARGET_SYNC` として、重複した 37 の arm（`#[allow(unreachable_patterns)]`）に残した。0038〜0040 は本物が入ったので `RESERVED_VERSIONS` を空にした。`SCHEMA_VERSION` は 41。番号の振り直しは次段の migrations 葉で行う |
| `crates/task-core/src/store/tests.rs` | 8 箇所の `SCHEMA_VERSION` の検査をすべて 41 にした |
| `crates/task-ops/src/delivery.rs` | `use task_core::{…}` を両側の和集合（`RepoId` と main の `DeliverySkipReason`・`OrgNode`・`PlanOrigin`・`RunIndexRole`・`TaskId`）にし、branch の `MAX_TARGET_RESYNCS`・`MAX_INTEGRATION_REPAIRS`・`target_restale_count` を残した |
| `crates/task-ops/src/delivery/tests.rs` | branch の `target_advanced_count_resets_only_on_explicit_restart` と main の部署の fallback 試験・delivery skip 試験を両方残した |
| `crates/task-worker/tests/browser_injection_wire.rs` | 同じ試験の 2 版のうち、改良側の main 版（CDP 応答待ち 60 秒）を採った |
| `crates/task-worker/tests/browser_shared_cdp.rs` | 両側とも probe の再試行版。改良側の main 版（期限 90 秒、`ValueError` も再試行、上限の検査あり、host の待ちは 120 秒）を採った |
| `docs/PROGRESS.md` | 3 箇所とも両側の節を全部残した。目次の Browser Phase 1 の行は main 側の追記版を採った |
| `docs/architecture-map.md` | branch の write-set・behind・直行経路の行と、main の ADR-0134 リンクの両方を残した |
| `docs/progress/time-dependent-tests-injection.md` | add/add。main 側の内容は branch 側に追記を足したものだったので、main 側を採った |
| `gui/app/routes/inbox.tsx` | branch の `IntegrationRepairPanel` と main の `delivery_skipped` 詳細ブロックを両方残した |
| `web/api/realtime/event-kinds.ts` | branch の review target・integration repair の 6 種と main の `delivery_skipped` を両方残した |
| `web/api/realtime/invalidation-map.ts` | 上の 7 種の invalidation の行を両方残した |

衝突のない file のうち、main の新しい試験の struct 初期化子に branch の欄を足した（`crates/task-ops/src/human_inbox/tests.rs` の `AttentionItem::Failed` に `integration_repair: None`、`crates/task-ops/src/notify_feed/tests.rs` の `Delivery` に `target_sha`・`reviewed_sha`・`merge_candidate_sha`）。`cargo check --workspace --all-targets` は exit 0 だった。

## migration の振り直し

- 理由: main の `0037_events_delivery_skipped_index`（本番 DB に適用済み）と branch の `0037_review_target_sync` が版数 37 で重複し、branch 側の arm が unreachable で deliveries の列が当たらず task-core の試験 4 件が落ちた。main 側の 0037 は変えず branch 側だけを振り直す。
- `crates/task-core/migrations/0037_review_target_sync.sql` → `0042_review_target_sync.sql`（git mv。main は 0041 まで、全 celeris/* で 0042 以上は未使用）。
- `MIGRATION_0037_REVIEW_TARGET_SYNC` → `MIGRATION_0042`、`migration_sql` の 42 の arm へ移し `#[allow(unreachable_patterns)]` を外した。`SCHEMA_VERSION` 41 → 42。
- 0038〜0040（work_unit_sessions・write_sets・behind_targets）は node_sessions の列と新しい表だけで、0042 の deliveries の列（target_sha 等）に依存しないので番号はそのまま。`RESERVED_VERSIONS` は空のまま。
- `store/tests.rs`・`cluster_job/tests.rs` の `SCHEMA_VERSION` の期待値を 42 にした（cluster_job の schema 33 へ戻す SQL は deliveries の 3 列も落としており変更不要）。
- `feed/tests.rs` の飛び埋め試験は「41 の記録だけ消して feed の表を落とす」形にした（37 より上を全部消すと 0038 の ALTER が再適用で重複列になる）。
- `delivery.rs` の試験名を `migration_0042_*` に改め、main の schema 41 の DB（1〜37 と 41 のみ）を開くと 38〜40 と 42 が当たり deliveries に target_sha・reviewed_sha・merge_candidate_sha ができる試験 `migration_0042_fills_gaps_in_main_schema_41_database` を足した。

## migration 振り直し表

ブランチ由来 migration 4 本を main の 0042 以降の空き番号へ振り直した（全 celeris/*・celeris-wu/* ブランチの `crates/task-core/migrations` を `git for-each-ref refs/heads refs/remotes` で走査し、0043〜0045 が未使用であることを確認済み。main は 0041 まで、0042 は別 branch が使用中、定期実行 task の 0039_cron_jobs も別途振り直しが必要なため 0046 以降は避けた）。

| 旧番号・ファイル名 | 新番号・ファイル名 |
|---|---|
| `0037_review_target_sync.sql` | `0042_review_target_sync.sql`（既に振り直し済み） |
| `0038_work_unit_sessions.sql` | `0043_work_unit_sessions.sql` |
| `0039_write_sets.sql` | `0044_write_sets.sql` |
| `0040_behind_targets.sql` | `0045_behind_targets.sql` |

- `crates/task-core/src/store/migrations.rs`: `MIGRATION_0038`〜`MIGRATION_0040` を `MIGRATION_0043`〜`MIGRATION_0045` に改名し `include_str!` のパスを新ファイル名へ直した。`migration_sql` の match を 38/39/40 から 43/44/45 へ移した。`SCHEMA_VERSION` を 42 → 45 にした。
- 0038〜0040 は番号として空いたが、他の celeris/* ブランチが別内容でまだ使用中のため `RESERVED_VERSIONS` に `[38, 39, 40]` を入れ直した（`migrate()` がこれらを恒久的に飛ばす）。
- `crates/task-core/src/delivery.rs` の `migration_0042_fills_gaps_in_main_schema_41_database` は、38〜40 が予約で飛んだまま 41〜45 が当たる形に更新した（記録される版数は `1..=37` に `[41,42,43,44,45]` を足した列）。
- `crates/task-core/src/cluster_job/tests.rs`・`crates/task-core/src/store/tests.rs` の `SCHEMA_VERSION` 直書きの検査（`assert_eq!(SCHEMA_VERSION, 42)` 等）をすべて 45 に直した。
- `crates/task-core/src/feed/tests.rs` の飛び埋め試験のコメントを更新した（挙動は `RESERVED_VERSIONS` を動的に参照しているため変更不要）。
- `UPDATE_SCHEMA=1 cargo test -p task-core` と `-p task-api` を実行したが、スキーマ生成物に差分は無かった（`git status --porcelain` が migration の rename と上記 5 ファイルの変更のみ）。

## ADR 振り直し

- `docs/adr/0124-claude-session-resume.md` を `docs/adr/0140-claude-session-resume.md` へ移した。`0124` は atomic direct route に残す。空き番号は `git for-each-ref refs/heads refs/remotes` で全ブランチを走査して docs/adr の番号を確認し、0139 が使用済み（ADR-0139）だったため 0140 を選んだ。
- Claude Code の session resume・continuation・checkpoint に属する参照を内容で判断して修正した。`git show e9cfcb69 66ce1652` の旧表記を含む削除行を数えた結果、crates 45 件、docs 18 件、gui 4 件、web 4 件を振り直した。gui の 4 件は schema 生成コメントを手動更新（pnpm 11.27.0 の store DB が開けず生成不能）。web の schema/type は pnpm 12.6.0 で再生成した。
- 残した ADR-0124 参照は direct route の説明である。`rg -o 'ADR-0124|0124-claude-session-resume'` の確認では crates 37 件、docs 21 件、gui 3 件、web 4 件が残る。これらの ADR-0124 は atomic direct route を指し、session resume 用の `0124-claude-session-resume` ファイル名参照は `docs/progress` と `docs/PROGRESS.md` の履歴記録に限る。
- `web/api/generated/schema.json` は `docs/api/v1/api-v1.schema.json` からの再生成で一致させた。

## 検証結果

完了日: 2026-10-03。検証した HEAD は `f7b262c521fa6fc9a1ab2c9ee8bb3de867324f04`。上記の `merged-main` は、この HEAD に取り込み済みの main `a1a3f60f03a3bc2872400e7ff27e8ec09b0387d8` を示す。

`git merge-tree --write-tree HEAD main` は exit 1。main `c448d9c77d18eb54396907835efb91c5d0a985e8` との衝突は `crates/task-worker/src/claude_code/prompt.rs`、`docs/api/v1/api-v1.schema.json`、`docs/architecture-map.md`、`gui/app/celeris/types.ts`、`web/api/generated/schema.json` の 5 ファイル。指示に従い main は取り込まず、`merged-main` 行は変更していない。

| コマンド | 結果 | exit |
|---|---:|---:|
| `cargo clippy --workspace -- -D warnings` | 警告なし | 0 |
| `cargo test -p task-core` | 688 passed | 0 |
| `cargo test -p task-ops` | 451 passed、1 ignored（手動計測） | 0 |
| `cargo test -p task-api`（run sandbox） | 123 passed、1 failed、2 ignored。browser H3 試験で userns の `unshare: Operation not permitted` | 101 |
| `cargo test -p task-api`（通常権限で再実行） | 439 passed、2 ignored | 0 |
| `cargo test -p task-dispatch` | 578 passed | 0 |

未解決: 新しい main との 5 ファイルの衝突はこの WorkUnit では解いていない。run sandbox の userns 制約は task-api の通常権限での再実行では発生しなかった。ログは run の成果物ディレクトリに各コマンド別に保存した。

## 最新 main の再取り込み（2026-10-03）

`c448d9c77d18eb54396907835efb91c5d0a985e8` を `git merge --no-ff main` で取り込んだ。`agent-docs/PROGRESS.md`、`crates/task-worker/src/claude_code/prompt.rs`、`docs/architecture-map.md` の内容衝突は両側の追記を保持して解いた。schema の 3 生成物は `UPDATE_SCHEMA=1` の task-core・task-api 試験、GUI の `pnpm gen:types`、web の `gen-types.mjs` で作り直した。ブランチ由来の ADR は `agent-docs/adr/` に移し、migration 0042〜0045 と ADR 0140 の番号を保持した。

`docs/progress/review-sync-main.md` は親 task の受け入れ条件が読むパスに残した。GUI の型検査では生成型の追加項目に合わせ、通知ラベルと fixture を更新した。`scripts/dev/check-architecture-map.py`、`scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`、`scripts/dev/check-doc-links.sh`、`cargo fmt --all -- --check`、`cargo clippy --workspace -- -D warnings`、GUI の `pnpm typecheck` は成功した。

web は再生成型に合わせて `execution_routed` の購読と `NewTaskBody` の参照を補い、`pnpm typecheck` と realtime 関連テスト 47 件が成功した。GUI の対象テストは 58 件成功した。

## 再検証（final review 差し戻し後）

final review の差し戻し（criterion 7/9）を受けて、取り込んだ main と 4 crate の試験を記録し直した。転記元は兄弟 unit の記録（`agent-docs/progress/2026-10-03-review-sync-main/*.md`）。コードは変えていない。

- 取り込んだ main: `merged-main` 行の `c448d9c77d18eb54396907835efb91c5d0a985e8`（HEAD の祖先。`git merge --no-ff`）。この sha は本節の作成時点の main（`f8a89553`）より前で、main の新しい分は取り込んでいない。
- userns 不可の sandbox での skip（`api-userns-skip`）: `crates/task-api/tests/common/mod.rs` に `userns_available()` を足した（`unshare -Ur true` の probe。`CELERIS_USERNS_TESTS=require` のときだけ偽を assert 失敗にする）。唯一 `unshare` を直接使う 3 試験（`browser_h3_injection.rs`、`browser_restore_deliver.rs`、`browser_restore_live_session.rs`）は probe が偽のとき早期 return する。この sandbox では probe が真で skip は発動しなかった。
- 同じ sandbox で見えた範囲外の失敗（`credentiald-sandbox`）: `celeris-credentiald` の `broker.rs` の子 process（`serve`・python resolve・`bridge`）に `env_clear()` を入れ、一時の `HOME`・`XDG_RUNTIME_DIR` だけを渡した。`cargo test -p celeris-credentiald --test broker daemon_rejects_worker_secret_retrieval_even_with_valid_lease -- --exact` は exit 0（1 passed）。

### 4 crate の試験結果

| crate | コマンド | exit | passed / failed / ignored | real |
|---|---|---:|---|---:|
| task-core | `cargo test -p task-core` | 0 | 688 / 0 / 0 | 16.76 s |
| task-ops | `cargo test -p task-ops` | 0 | 451 / 0 / 1（手動計測用） | 16.38 s |
| task-api | `cargo test -p task-api` | 0 | 439 / 0 / 2 | 41.1 s（unit 記録の時点。本節の作成時の再実行は 2 分 37 秒） |
| task-dispatch | `cargo test -p task-dispatch` | 0 | 578 / 0 / 0（lib 574 + unified_kill 4） | 42.92 s |

- 単体で 60 秒を超えた試験は無し（4 crate とも libtest の警告なし）。
- 任意の参照: 統合後の `cargo test --workspace` は exit 0（126 試験バイナリすべて ok、5 分 35 秒）、`cargo clippy --workspace -- -D warnings` は exit 0。詳細は `agent-docs/progress/2026-10-03-review-sync-main/core-ops-dispatch-tests/ws-green.md`。
- 本節の task-api 再実行のログは run の成果物 `task-api-recheck.log` にある。
