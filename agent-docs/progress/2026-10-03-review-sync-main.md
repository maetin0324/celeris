---
title: review sync — main 取り込み
tasks: [01M408E4BX8A3FNSBJTZ0CC67A, 01M40FWV6Z6N4P5ESHZMK0KH2B, 01M3XZ5PYSTTC6GXAH8TZVRHSA]
status: done
updated: 2026-10-03
---
（この文書は agent-docs/progress/ が正本。）
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

親 task の受け入れ条件が参照する文書はこの正本へ移した。GUI の型検査では生成型の追加項目に合わせ、通知ラベルと fixture を更新した。`scripts/dev/check-architecture-map.py`、`scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`、`scripts/dev/check-doc-links.sh`、`cargo fmt --all -- --check`、`cargo clippy --workspace -- -D warnings`、GUI の `pnpm typecheck` は成功した。

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
merged-main: 0a941d7d227138067f97515acdff2c6fd84677d7

## main 再取り込み（merge-main）

`git merge --no-ff main`（main `0a941d7d`、rebase なし）。衝突 5 件を両側保持で解いた。migration・ADR 番号は main と重ならなかったので振り直しはしていない（main の 0046_cron_jobs、ADR 0131-cron-jobs。branch の 0042〜0045、ADR 0140）。

| ファイル | 解き方 |
|---|---|
| `crates/task-api/tests/browser_h3_injection.rs` | main 側の `userns_gate::skip_unless_userns_tests()`（ADR-0126 の opt-in gate）を採った。branch 側の `userns_available()` は捨てた |
| `crates/task-core/src/cluster_job/tests.rs` | 試験本体は自動 merge で両側の DROP（deliveries・node_sessions の列、cron 表）が残った。`SCHEMA_VERSION` の期待値は main 側の 46 |
| `crates/task-core/src/store/migrations.rs` | `MIGRATION_0042`〜`0045`（branch）と `MIGRATION_0046`（main）を番号順に登録。`migration_sql` の arm も 42〜46 の順。`RESERVED_VERSIONS` は `[38, 39, 40]`（0042〜0045 は実在するので外した）。`SCHEMA_VERSION` は 46 |
| `crates/task-core/src/store/tests.rs` | 8 箇所の `SCHEMA_VERSION` 検査はすべて 46（main 側） |
| `scripts/dev/check-adr-numbers.sh` | `ALLOWED_OVER_LAST` は両側の和（`0140-claude-session-resume.md` と `0131-cron-jobs.md`） |

衝突以外で直したもの:

- `crates/task-core/src/delivery.rs` の migration 0042 試験: 版数の検査を 45 → 46、記録される版数の列に 46 を足した。
- `crates/task-core/src/cron/store_tests.rs` の「版数 37 の DB に戻す」試験: 巻き戻しが cron 表と feed 表だけだったため、0042〜0045 が足した列・表が残って `duplicate column name: target_sha` で落ちた。巻き戻しに 0042〜0045 の逆操作（deliveries の 3 列、node_sessions の 4 列と index、write_sets・behind・hints の表）を足した。

### 検証

| コマンド | 結果 | exit |
|---|---|---:|
| `git grep -n -E '^(<<<<<<<\|>>>>>>>) ' -- crates scripts docs gui web` | 一致なし | 1（マーカー無し） |
| `ls crates/task-core/migrations \| cut -c1-4 \| sort \| uniq -d` | 出力なし（重複無し） | 0 |
| `sh scripts/dev/check-adr-numbers.sh` | `check-adr-numbers: ok (131 files)` | 0 |
| `cargo test -p task-core` | 706 passed / 0 failed | 0 |
| `cargo test -p task-api` | 全 crate の unit・integration が ok。browser_h3 の userns 試験は gate で skip | 0 |
| `cargo clippy --workspace -- -D warnings` | 警告なし | 0 |
| `node web/scripts/gen-types.mjs --check` | 生成物の差分なし（web の `types.ts` と `schema.json` は merge 後の内容が古かったので再生成した） | 0 |
| `node gui/scripts/gen-types.mjs --check` | 差分なし | 0 |

`docs/api/v1/api-v1.schema.json` は `cargo test -p task-core` の schema 一致検査と `-p task-api` の検査が通ったので再生成していない。

## PROGRESS v2（replan v2 の受け入れ）

unit `gui-web-record`（WU `progress-v2-record`）の記録。この節の作成時点の HEAD は `11c5c006`。

### (1) main 祖先

```sh
git merge-base --is-ancestor 0a941d7d227138067f97515acdff2c6fd84677d7 HEAD
```

exit 0。上の `merged-main: 0a941d7d227138067f97515acdff2c6fd84677d7`（本節より前の、最後の `merged-main:` 行）は HEAD の祖先。

### (2) 番号振り直し

```sh
ls crates/task-core/migrations | cut -c1-4 | sort | uniq -d
sh scripts/dev/check-adr-numbers.sh
```

`uniq -d` は出力なし（exit 0、先頭 4 桁の重複なし）。`check-adr-numbers.sh` は `check-adr-numbers: ok (131 files)`（exit 0）。

### (3) remote/restart 試験

```sh
cargo test -p task-dispatch session_resume
```

exit 0、`test result: ok. 23 passed; 0 failed`（`unified_kill` の 0 件を除く）。remote 3 件
（`session_resume_remote_same_account_and_local_cwd_resumes`・`session_resume_remote_other_account_falls_back_to_checkpoint`・
`session_resume_remote_rejected_resume_falls_back_to_checkpoint`）と restart 3 件
（`session_resume_after_restart_kept_session_file_resumes_same_session`・
`session_resume_after_restart_missing_session_file_falls_back_to_checkpoint`・
`session_resume_after_restart_corrupt_session_file_falls_back_to_checkpoint`）を含む。詳細は
`agent-docs/progress/2026-10-03-session-resume-tests/remote-resume.md`・`restart-resume.md`
（ADR-0140 D3: remote worktree と daemon 再起動後の continuation、いずれも done）。

### (4) 前後比較

`agent-docs/progress/2026-10-03-merge-train-evaluation.md`（evaluate 葉）の結論: merge train / project-level
integration branch は **現時点では不要**。本番の delivery 56 件のうち 23% が `merge_base` repair
を要し 11% が repair 上限で人待ちになっているが、その窓を狙った Phase 1/2/5（ADR-0118・ADR-0120・
ADR-0130）は本節作成時点でまだ本番 release に 1 件も入っていない（導入後の実測値はすべて 0 件）。
段の中の統合（`integrate-<stage>`）は衝突 4.3%・検査不合格 22% のどちらも自動修復で done。再評価
の閾値（`[needs-human]` 10% 超など 4 条件）は本番 release 反映後、delivery 30 件到達時点で見る。

`agent-docs/progress/2026-10-03-phase-effect-ab.md`（ab-verify 葉）の決定的 A/B 試験
（`cargo test -p task-dispatch --lib phase_effect_ab -- --nocapture`、exit 0、4 passed）:

| scenario | run 数 | wall time（秒） | 入力 token | 新 session 数 |
|---|---|---|---|---|
| continuation（Phase 3 session resume） | 3 → 3 | 360 → 280 | 120000 → 48000 | 3 → 1 |
| atomic_route（Phase 4 直行経路） | 3 → 2 | 360 → 240 | 120000 → 80000 | 3 → 2 |
| review_sync（Phase 1/2 review 前同期） | 4 → 3 | 420 → 330 | 160000 → 120000 | 4 → 3 |

いずれも導入後（on）で run 数・wall time・入力 token・新 session 数が同じか減った。値は偽アダプタ
による決定的な勘定（新 session 40000 token・resume 4000 token）で、実 LLM の値ではない。model の
A/B はこの模擬の範囲で、本番の統合失敗率（merge-train-evaluation.md の 23%・11%）は模擬していない
ため、merge train の要否判断には実測側（'## 結論' の閾値）を使う。

### (5) gui/web/clippy

前の WU `gui-web-verify`（`agent-docs/progress/2026-10-03-review-sync-record/gui-web-verify.md`、
HEAD `7b33c4185ed3` 時点、done）の記録を引用する。本 WU では gui/・web/・crates/ を変更していない。

| コマンド | exit | 要点 |
|---|---:|---|
| `corepack pnpm@11.27.0 -C gui install --frozen-lockfile` | 0 | 340 packages |
| `corepack pnpm@11.27.0 -C gui typecheck` | 0 | `react-router typegen && tsc -b`、エラーなし |
| `corepack pnpm@11.27.0 -C gui test` | 0 | vitest 90 test files / 1287 tests すべて passed |
| `corepack pnpm@12.6.0 -C web install --frozen-lockfile` | 0 | 200 packages |
| `corepack pnpm@12.6.0 -C web typecheck` | 0 | `tsc -b`、エラーなし |
| `corepack pnpm@12.6.0 -C web test` | 0 | vitest 26 files / 188 tests + node:test 42 tests、すべて passed |
| `cargo clippy --workspace -- -D warnings` | 0 | warning 0 件 |

`git diff --stat crates/` は空、`git status --short` も空（gui/・web/・docs/ のいずれにも
main 取り込み由来の修正は不要だった）。


## agent-docs/PROGRESS.md から移した節

旧 `agent-docs/PROGRESS.md`（追記は終了、ADR-0128）に、このブランチが追記した節を逐語で写す。`[..](progress/…)` などの相対リンクは旧 PROGRESS.md 基準のまま残すため、下の code block に入れてリンク検査の対象外にする。front matter の 3 行は移動先の front matter（`tasks:`）に合わせて本文には写していない。

```text

## Phase 1 — review 前の target 同期と merge candidate 固定（ADR-0118、2026-10-02）

reviewer と deterministic checks の前に task worktree を target へ rebase し、reviewed SHA と merge candidate SHA を一致させて記録する。merge/delivery 時に target が進んでいれば再同期・再検査・再 review へ戻す。tree child は親ブランチへの再帰統合を保ち、candidate SHA を照合する。

- 証拠: `cargo fmt --all -- --check` exit 0。`cargo clippy --workspace -- -D warnings` exit 0。
- `cargo test --workspace` は 2 回実行。いずれも task-crate tests を含む前半は成功したが、`celeris --test instance_handoff` が 5/8 で失敗し全体 exit 101。3 件はこの環境で user namespace 作成が `Operation not permitted`（ADR-0095 guard）、2 件は daemon dispatch/standby 待ちが成立しなかった。再実行でも再現したため、Phase 1 差分外の環境制約として変更せず。
- 着手時の target: `git log -1 main` = `5f14fe7480f1512a0a62c35cf41c1fedb11a6944 integrate wu/merge-main (phase land)`。`git merge-tree --write-tree --name-only HEAD main` は tree `70360b86cb8d6add2d8493c7c8c6f15ad38aa2ab` を返し、`crates/task-dispatch/src/dispatcher/review_spawn.rs` の content conflict を 1 件検出。これは並行 review-decisions と同じ箇所で、衝突の自動解決は Phase 2。
- 未解決: 衝突の自動解決は Phase 2。Phase 1 は衝突を検出して安全に止め、成果を保持する。

### fix-sync-stop: review 前同期の停止が attempts を消費していた不具合の修正 — 2026-10-02（work unit `fix-sync-stop`）

final review 差し戻し（criterion 4 fail）で指摘された不整合を修正した。`stop_review_for_target_sync`（`crates/task-dispatch/src/dispatcher/review_spawn.rs`）が同期の停止全般に `Trigger::ReviewFail` を使っており、`retry_or_fail` を通って attempts を 1 消費し Ready/Failed に落ちていた。これは ADR-0118 D4（stale はコードの不合格ではなく review の試行回数に数えない）に反し、root delivery の `[merge-base]` 局所修復経路と tree child の段階統合衝突経路を壊していた。

- 衝突・dirty worktree・ref 不読は同期を省略し、成果を保った未同期 HEAD のまま従来どおり review に進む（attempts は変化しない）。
- stale（同期中の target 再進行・delivery の base/head 不一致・検査後の reviewed snapshot 変化）は `ReviewTargetAdvanced` を event に残し、状態遷移を起こさず再 sync → 再 check →再 review のループに入る。上限超過時は Reviewing のまま止める（attempts を消費しない）。
- `crates/celeris/src/delivery.rs`: 候補 SHA が NULL かつ main と分岐している行は rereview へ戻さず `[merge-base]`（局所修復）へ渡す。
- 追加試験: `crates/celeris/src/delivery/tests.rs`、`crates/task-dispatch/src/dispatcher/review_spawn.rs` / `review_verdict.rs` に pre_review_sync 系の試験を追加。ADR-0118 に D2/D4/D6 の付記。

### reland-main: 最新 main の取り込みと全検査の再実行 — 2026-10-02（work unit `reland-main`）

fix-sync-stop の後、最新 main（`29e2d76875e0cb3ab21ea606b0014aab62413cd8`、confirm-release 統合・BenchFS 方向転換の記録を含む）をこの task ブランチへ merge した。`git merge-tree --write-tree --name-only HEAD main`（事前確認）は衝突ファイル名を 1 件も返さず（tree `abb8120a171f9edb22e63ff2c9fc6ed998c00bb1`）、実際の merge も衝突なし（`docs/progress/phase-R.md` に 1 行追加されるのみの自動 merge）。`git merge-base --is-ancestor <main-sha> HEAD` → exit 0。衝突マーカーは `git grep -n '^<<<<<<<\|^=======$\|^>>>>>>>' .` → 該当なし。

- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo test --workspace` → exit 0（全 118 テストバイナリで `test result: ok`、`0 failed`）。
- `cargo test --workspace pre_review_sync` → 1 passed（`delivery::tests::pre_review_sync_conflict_unsynced_candidate_goes_to_merge_base_repair`）、`cargo test --workspace reviewed_sha` → 3 passed（`delivery::tests::target_advanced_*`）。`cargo test -p task-dispatch` の `dispatcher::tests::target_sync::*` 8 件（pre_review_sync_dirty_keeps_attempts・pre_review_sync_conflict_keeps_attempts_and_branch・pre_review_sync_target_advanced_keeps_attempts・pre_review_sync_target_advanced_limit_halts_without_attempts 等）も exit 0。既知の flaky `task-worker` の `local_deep_research::tests::missing_celeris_result_line_is_retryable_error` を単独で再実行し 1 passed（workspace 全体実行でも今回は失敗なし）。
- sandbox 内で userns 必須試験（`instance_handoff` 等）が完走できない場合があることは把握済みだが、今回の `cargo test --workspace` では該当失敗なし（daemon はこの制約の外で checks を実行するため、sandbox 制約を plan_issue の理由にはしない）。

### land-main-2: 再進行した main（web GUI 統合）の取り込みと全検査の再実行 — 2026-10-02（work unit `land-main-2`）

reland-main 後に main が web GUI（task 01M3QE4D330YESFT6FY8G50R12、ADR-0081 TanStack SPA + gateway、Phase 1〜6）の統合で `e730f0569db12f8dc1d46fde1932636f40db48a9` まで進み、integrate-verify の check `git merge-base --is-ancestor main HEAD` が落ちていた。着手時にコードの修正（c5e9498a・fix-sync-stop）は済んでおり `review_spawn.rs` に `Trigger::ReviewFail` は無いことを確認済み。`git merge-tree --write-tree --name-only HEAD main` の事前見積もりは `crates/task-worker/tests/browser_shared_cdp.rs` 1 件だけを衝突として返した（`docs/PROGRESS.md` は自動 merge）。

- 実際の merge（commit `31dead31c5fb3f56e37d0644b1bd9339ef93657a`）も見積もりどおり衝突は `browser_shared_cdp.rs` のみ。両側とも R7-12 系の CDP probe EOF 無限待ち修正だったため、main 側（`ISOLATION`/`PREFLIGHT_TIMEOUT` 定数・`probe.err` への例外書き出し・`recv_exact` によるフレーム読み切り・`preflight()`）を基本に採用し、task 側にあって main に無かった「接続後の handshake / CDP 応答が一度失敗しても deadline まで接続からやり直す」期限付き再試行を `except OSError` の外側ループに戻す形で足した。Rust 側の外側タイムアウトは task 側の 75 秒（main は 60 秒）をそのまま残した。`docs/PROGRESS.md` は衝突なしで両節とも残っている。
- `git grep -n '^<<<<<<<\|^=======$\|^>>>>>>>' .` → 該当なし。`git merge-base --is-ancestor main HEAD` → exit 0。
- `cargo build -p task-worker --bins` → exit 0（browser_shared_cdp.rs のテストが前提とする `celeris-browser-sandboxd`/`celeris-browser-egress` bin を先に生成）。
- `cargo test -p task-worker --test browser_shared_cdp --no-run` → exit 0（コンパイル確認）。
- `cargo test -p task-worker --test browser_shared_cdp -- --nocapture` → exit 0（2 passed。sandbox では preflight が失敗して `SKIPPED (environment unavailable, not passed)` を出すだけで、panic はしない）。
- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo test --workspace` → exit 0（118 テストバイナリすべて `test result: ok`、FAILED・`error[` なし）。
- `cargo test --workspace pre_review_sync` → 9 passed、0 failed（`delivery::tests::pre_review_sync_conflict_unsynced_candidate_goes_to_merge_base_repair` 1 件 + `dispatcher::tests::target_sync::pre_review_sync_*` / `target_sync_pre_review_sync_*` 8 件）。
- `cargo test --workspace reviewed_sha` → 3 passed、0 failed（`delivery::tests::target_advanced_*_reviewed_sha`）。
- Phase 1 のコード（review 前同期・stale 検出・同期停止が attempts を消費しない経路）は変更していない。既知の flaky（`local_deep_research` の Spawn NotFound、`browser_shared_cdp` の sandbox TCP probe）は今回の `cargo test --workspace` では再現しなかった。

## Phase 2 — review 前同期の IntegrationRepair（ADR-0120、2026-10-02）

review 前の target への rebase が衝突した場合、元の成果を保持する `integration-repair-<n>` WU を同じ task に追加する。WU が完了すると最新 target へ再同期し、checks と reviewer を新しい SHA でやり直す。2 回の上限、修復不能時の安全な rollback と未同期 HEAD での従来経路への復帰を実装した。`IntegrationRepairScheduled` / `Resolved` / `Exhausted` を task event に残し、`TaskDetail.integration_repair` と受信箱で通常の実装失敗と区別する。Phase 2 の衝突解消経路の実装完了日は 2026-10-02。

- 最新 main の確認: `7f3482a3` と、その後に進んだ `39e22633` は `git merge-tree --write-tree --name-only HEAD main` で衝突なしを確認して取り込んだ。さらに進んだ `95595105` への見積もりでは `docs/PROGRESS.md` の content conflict だけを検出したため、Phase 1/2 の節と main の Browser capability Phase 1 追記を両方残して解消した。ADR-0120 の番号は main の `docs/adr/` に無く一意である。
- 契約照合: ADR-0120 D5 の event 名・payload、API の `TaskDetail.integration_repair` 欄、`MAX_INTEGRATION_REPAIRS = 2`、rollback の clean/reflog/branch 条件を `task-core`・`task-ops`・`task-dispatch` と照合した。GUI の案内を `FailureBanner` より上に配置し、ADR の付記に実装済み範囲と残件を記した。
- 検査: `cargo build -p task-worker --bins` → exit 0。`cargo fmt --all -- --check` は既存の `crates/task-api/tests/integration_repair.rs` の整形差分で初回 exit 1、`cargo fmt --all` 後の再検査は exit 0。`cargo clippy --workspace -- -D warnings` → exit 0（main 再取り込み後にも exit 0）。
- `cargo test --workspace integration_repair` → exit 0、19 passed / 0 failed。dispatcher の起票・最新 target への再同期・2 回上限・rollback、event/API の射影を含む。`git diff --check` と ADR 番号・architecture-map・PROGRESS の受け入れ check → exit 0。
- `cargo test --workspace` → exit 101。`celeris --test instance_handoff` の 8 件中 5 件が失敗した。3 件は worker db guard が user namespace を作れず `Operation not permitted`、2 件はその結果として dispatch/standby の待ち条件が成立しなかった。`unshare -U -r true` も `uid_map: Operation not permitted` で exit 1。この sandbox の制約として記録し、これを `plan_issue` の理由にしない。daemon の決定的 check は sandbox 外で実行される。
- `cargo test --workspace --exclude celeris` → exit 101。`e2e --test account_pool_scenarios` の 3 件も daemon 起動時に同じ worker db guard の user namespace probe で停止した。これは上記の制約が `celeris` crate 固有ではないことを示す。
- `cargo test --workspace --exclude celeris --exclude e2e` → exit 101。`task-api --test browser_h3_injection` の `production_h3_injects_once_without_exposure` が `unshare: Operation not permitted` で失敗した。いずれも IntegrationRepair の失敗ではなく、この sandbox の user namespace 制約による。
- `cargo test --workspace --lib` → exit 101。`task-worker --lib` は 657 passed / 12 failed / 4 ignored。失敗は browser 実 runtime と `db_guard` の namespace を要する試験で、`Operation not permitted` を含む。IntegrationRepair に関係する `task-core`・`task-ops`・`task-dispatch`・`task-api` の lib 試験は別コマンドでも切り分ける。
- `cargo test -p task-core -p task-ops -p task-dispatch -p task-api --lib` → exit 0、計 1,611 passed / 0 failed / 2 ignored（74 + 630 + 518 + 389）。
- `95595105` 取り込み後に `cargo build -p task-worker --bins && cargo fmt --all -- --check && cargo clippy --workspace -- -D warnings` → exit 0。`cargo test --workspace integration_repair` → exit 0、19 passed / 0 failed。`git merge-base --is-ancestor main HEAD` → exit 0。
- 未解決: ADR-0120 D5 の `WorkUnitView.integration_repair` / `ExecutionWorkUnitView.integration_repair` と GUI の WU 行の専用 badge は未実装。`task_core::integration_repair_snapshots` の event 投影までは実装済み。sandbox 外での `cargo test --workspace` exit 0 の確認も残る。本番への昇格はこの worktree の範囲外。

## Phase 3 — Claude Code continuation metrics（ADR-0140、2026-10-02）

`ExecutionMetrics` と `GET /metrics/execution` に worker run の fresh / resumed / unknown 別の run 数・総 wall time・入力 token・再探索重複、および WU 別値と fresh fallback 理由別件数を追加した。定義は [continuation-metrics.md](api/v1/continuation-metrics.md)。API schema は `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema::tests::committed_schema_matches_generated` で再生成する。GUI / web の `gen:types` はこの WorkUnit の対象外で未実施。検証: `cargo test -p task-core --lib` 638 passed、`cargo test -p task-api --lib` 75 passed / 2 ignored、schema 一致、`cargo clippy --workspace -- -D warnings` と `cargo fmt --all -- --check` exit 0。着手時の main `95ac1644` との merge-tree は `task-api/query.rs` など 8 ファイルで衝突を検出したが、この WorkUnit の変更ファイルとは重ならない。

### Phase 3 session resume — workspace verification（2026-10-02、run `01M3Y90WZRZTGW8D3QZ1K3M0EE`、ADR `claude-session-resume`）

実装参照を確認: [architecture-map](architecture-map.md) の継続 session / execute continuation / Claude Code adapter の行は `node_session.rs`、`sessions.rs` と `dispatcher/continuation_session.rs`、`claude_code.rs` と ADR-0140 を指す。`ExecutionMetrics` 実体は `crates/task-core/src/execution_metrics.rs`。

- 衝突見積もり: `git merge-tree --write-tree --name-only HEAD main` → exit 1。worktree の `main` は `0d438ec19d9a474c5b82507cefd0d9e63846d0d6`、HEAD は `f4fd17d03444fb1822cfbdaa8140384ed416f885`（main は HEAD の祖先でない）。競合は `crates/task-api/src/query.rs`, `crates/task-api/src/query/tests.rs`, `crates/task-core/src/cluster_job/tests.rs`, `crates/task-core/src/store/migrations.rs`, `crates/task-core/src/store/tests.rs`, `crates/task-ops/src/delivery.rs`, `crates/task-ops/src/delivery/tests.rs`, `docs/PROGRESS.md`, `gui/app/routes/inbox.tsx`。最新 main の確認に使える remote/fetch はこの worktree に無いため、登録された `main` ref を対象にした。
- `unshare -U -r true` → exit 1（`/proc/self/uid_map: Operation not permitted`）。
- `cargo test --workspace` → exit 101。`instance_handoff` は 8 件中 3 passed / 5 failed（他のテスト binary はこの失敗までに完了）。失敗名: `starting_the_same_release_twice_exits_three`, `normal_mode_does_not_inject_the_smoke_builtins`, `verify_mode_never_dispatches_and_never_touches_daemon_instances`, `a_newer_release_takes_over_while_the_old_one_finishes_its_run`, `a_stale_heartbeat_promotes_the_standby`。前3件のうち namespace 拒否がログで明示された3件は user namespace 制限、後2件は handoff/standby 待ち失敗。
- `cargo test -p celeris --test instance_handoff` 単独再実行 → exit 101、同じ5件が再現（namespace 拒否3件、handoff/standby 待ち2件）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- 前回 run の acceptance check `grep -q 'claude-session-resume' docs/PROGRESS.md` は、この節に当該識別子が無かったため exit 1 だった。今回の見出しに ADR slug を明記し、同じ check を再実行して通過を確認した。
- コード変更なし。workspace test は環境制限により受け入れ条件未達として報告する。

### Phase 3 session resume — instance handoff 待ち時間の安定化（2026-10-02、run `01M3YAZJHT6ERY2RNBFV3HGW5E`）

前回の final review で sandbox 外の `cargo test --workspace` が高負荷により `instance_handoff` の10秒状態待ちを越えて失敗したため、3つの状態待ちを共通の60秒上限に延長した。条件成立時には直ちに抜ける。fake adapter のゲートも最大60秒（1200 × 0.05秒）にし、状態観測後に解放する。drain / idle の終了待ちも60秒に揃えた。理由は試験コードの定数コメントに記した。CPU を焼く負荷再現は行っていない。

- この run の `cargo test -p celeris --test instance_handoff` → exit 101（3 passed / 5 failed、60.20秒）。3件は ADR-0095 worker db guard の user namespace `Operation not permitted`、残り2件は sandbox 上で handoff dispatch / standby 状態待ちが成立しなかった。sandbox は user namespace を拒否するため、試験は daemon を起動できず、待ち時間の大小にかかわらず実行確認には使えない。
- instance_handoff を含む実行確認は sandbox 外で daemon が走る stage 統合の workspace check と final review の `cargo test --workspace` に委ねる。この sandbox 内の試験失敗は plan_issue としない。
- `cargo fmt --all -- --check` と `cargo clippy --workspace -- -D warnings` はこの run で確認する。変更範囲は `crates/celeris/tests/instance_handoff.rs` と本節のみ。

## Atomic coding task の planner なし直行経路 — Phase 4 統合検証（2026-10-02）

統合後 HEAD `dd4b5a6131b4` の `cargo build --workspace --bins` と `cargo clippy --workspace -- -D warnings` は exit 0。`cargo test --workspace` は exit 101（instance_handoff 8 件中 3 passed / 5 failed）で、`cargo test -p celeris --test instance_handoff` の単独再実行でも同じ5件が失敗した。3件は ADR-0095 worker db guard の user namespace probe が `Operation not permitted`、2件は handoff dispatch/standby 待機 assertion 失敗。結果、未解決事項、再確認提案は [Phase 4 検証記録](progress/phase-direct-route.md) を参照。実装箇所の案内として architecture map の direct route 行を実ファイル・関数名に更新した。

## expected/actual write-set による並列制御と behind 指標（完了 2026-10-02、ADR-0130）

Phase 5 の実装は完了。expected path hint の正規化、Git 差分からの actual write-set 記録、強く重なる同一 repo の run 待機、target からの behind commits/age の観測、長期 stale task の review 前 sync 優先、API/GUI 表示を接続した。実装箇所は [architecture map](architecture-map.md)、仕様は [ADR-0130](adr/0130-write-set-parallelism-and-behind.md)、検証の証拠・衝突見積もり・未解決事項は [Phase 5 検証記録](progress/phase-writeset.md) を参照。

- 証拠: `cargo fmt --all -- --check` と `cargo clippy --workspace -- -D warnings` は exit 0。
- `cargo test --workspace` と全 binary を続行する `cargo test --workspace --no-fail-fast` はともに exit 101。no-fail-fast は24 targets の失敗を検出し、instance handoff / e2e の worker DB guard user namespace 拒否と browser runtime の `unshare: Operation not permitted` / `NoChildPid` を確認。task-core 657、task-dispatch 557、task-ops 398、task-api lib 76 の各試験は通過。失敗 target の全一覧と test ごとの結果は Phase 5 検証記録に記載。
- 未解決事項: workspace test 全件の合格は sandbox 制約により未確認。最新 main との merge-tree は10ファイルの衝突を予測。
- 提案: 統合時に衝突を解消し、通常権限の stage 統合または final review で workspace suite を再実行する。

## review-sync main 取り込みの検証（2026-10-03）

完了日: 2026-10-03。検証 HEAD は `f7b262c521fa6fc9a1ab2c9ee8bb3de867324f04`。`git merge-tree --write-tree HEAD main` は exit 1 で、main `c448d9c77d18eb54396907835efb91c5d0a985e8` との衝突 5 ファイルを検出した。指示に従い main は取り込まず、この文書の `merged-main` は既に取り込み済みの `a1a3f60f03a3bc2872400e7ff27e8ec09b0387d8` のままにした。

| コマンド | テスト数・結果 | exit |
|---|---:|---:|
| `cargo clippy --workspace -- -D warnings` | 警告なし | 0 |
| `cargo test -p task-core` | 688 passed | 0 |
| `cargo test -p task-ops` | 451 passed、1 ignored（手動計測） | 0 |
| `cargo test -p task-api`（run sandbox） | 123 passed、1 failed、2 ignored | 101 |
| `cargo test -p task-api`（通常権限で再実行） | 439 passed、2 ignored | 0 |
| `cargo test -p task-dispatch` | 578 passed | 0 |

未解決事項: 新しい main との衝突は `crates/task-worker/src/claude_code/prompt.rs`、`docs/api/v1/api-v1.schema.json`、`docs/architecture-map.md`、`gui/app/celeris/types.ts`、`web/api/generated/schema.json`。run sandbox での task-api の失敗は browser H3 試験の `unshare: Operation not permitted` によるもので、同じコマンドは通常権限では exit 0 だった。検査ログは run の成果物ディレクトリに保存した。


## review-sync の main 再取り込み（2026-10-03）

`c448d9c7` を no-ff merge し、上記の未解決だった 5 ファイルと文書移設による `PROGRESS.md` の衝突を解消した。生成 schema と GUI/web の型は再生成済み。経緯と検証結果は `agent-docs/progress/2026-10-03-review-sync-main.md` を参照する。

## 最終 main 取り込み（2026-10-03）

検証結果は [merge-main-2](2026-10-03-review-sync-fix/merge-main-2.md) を参照。

merged-main: 90e3ca26ce1412e0cc2d4fcdc4613717df12973a

## drop-symlink 修復（2026-10-03）

`main` の `90e3ca26ce1412e0cc2d4fcdc4613717df12973a` を no-ff merge し、`docs/architecture-map.md` の両側の記述を保持した。旧 progress symlink を削除し、この記録内の参照を正本の場所へ直した。

merged-main: 90e3ca26ce1412e0cc2d4fcdc4613717df12973a
