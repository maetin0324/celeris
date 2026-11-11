---
title: 健全性v2 R5 dedupe-celeris — celeris の重複 setup と旧実装の固定の整理
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R5 dedupe-celeris — celeris の重複 setup と旧実装の固定の整理

完了日: 2026-11-11

WorkUnit `dedupe-celeris`（段 dedupe）。`contracts.json` の celeris の候補のうち R5 の write set に入るものを片付けた。基準は `$CELERIS_WU_BASE`。

## 結論
- `duplicate_setup` 7 件（D06・D08・D29・D34・D39・D42・D49）の写しを `crates/celeris/tests/support/` の helper に寄せた。試験関数の数は変わらない。
- `old_impl` の `old-incremental-backup-measurement` の試験 1 本を消した（人の決定 `test-removal` は a）。mutation は KILLED で、記録は [`removals/celeris.json`](removals/celeris.json)。`cases.py` は exit 0（試験関数 806 → 805）。
- celeris の `equivalent_scenario` の候補は `contracts.json` に 0 件。objective が例に挙げた `script_compare_*` と `project_controller_e2e`・`project_controller_replay` は候補に無いので、表にまとめていない（ADR D5-1: 候補の無い試験は消さない）。
- `legacy_event_startup`（replay の契約）は触っていない。
- 残す試験の検出力は 9 つの mutation で確かめた。KILLED 8 件、SURVIVED 1 件（D08 の 1 つ目の mutant。残す試験が通らない分岐で、試験は消していない）。

## 実装前 brief
1. 根本原因: 同じ helper（試験の repo で git を走らせる・状態の変化を待つ・偽の Discord webhook・`/proc/self` の読み取り・daemon を 3 tick 回す）を試験 file ごとに写していた。症状は、本体・`RunOptions`・`Hook` の形が変わると複数か所を直す必要があること。旧方式の backup 計測試験は、置き換え済みの rusqlite の online backup の性質を測るだけで、celeris の関数を呼ばない。
2. 責務 owner: `crates/celeris/tests/support/`（celeris の結合試験の共通 helper）。src の unit 試験（`src/knowledge_curation/tests.rs`）も `#[path]` で同じ file を読む。
3. 既存の再利用先: `support/mod.rs`・`support/script_compare.rs`（`mod support;` で読む）、repo 直下の `tests/support/http_async.rs`（`fake_http`）、`cases.py`・`mutate.sh`。
4. 選択肢: (a) 全部を `support/mod.rs` に足して `mod support;` で読む案は、`script_compare.rs`（769 行）を使わない target にもコンパイルさせる。(b) 小さな helper は `support/` の別 file にして `#[path]` で読み、既に `mod support;` を読む組（`proc_stats`・`run_daemon`）だけ `support/` の module に足す案。R5 の目的（build の負荷を減らす）に合う (b) を採った。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: helper の写し 16 関数と `Hook` の struct 2 つ、旧方式の計測試験 1 本。
7. 不変条件: 消した 1 本を除き、試験関数の名前と数は変わらない（`cases.py`）。不変条件の試験は消していない。各組の残す試験が `mutation_target` の mutant を落とす（`mutate.sh`）。

## 変更
| 候補 | 写し | 寄せた先 | 読み方 |
|---|---|---|---|
| D06・D34 | `git`（`cos_delivery_suite` の `stranded_default_branch`・`stranded_integration`、`daily_curation.rs`、`src/knowledge_curation/tests.rs`） | `support/git.rs` の `git` | `#[path]` |
| D08 | `wait_until`（`instance_handoff.rs`・`unpromoted_release.rs`） | `support/wait.rs` | `#[path]` |
| D29 | `run_daemon`（`script_compare_cos_digest.rs`・`script_compare_lane_rules.rs`） | `support/script_compare.rs` | `mod support;` |
| D39・D49 | `status_kb`・`pct`（と同じ組の `threads`。`long_run_soak.rs`・`project_controller_replay.rs`） | `support/proc_stats.rs` | `mod support;` |
| D42 | `Hook`・`hook`（`cos_delivery_suite` の `cos_chat_triage_scenarios`、`root_done_summary.rs`） | `support/webhook.rs` | `#[path]`（読む crate の根に `fake_http` が要る） |

- D06 と D34 の `git` は本体が少し違った（D34 だけが `-c user.name/user.email` と `GIT_TERMINAL_PROMPT=0` を付ける）。どちらも「試験の一時 repo で git を走らせ、成功を assert し、stdout を返す」同じ意味なので、D34 の形の 1 つにした。stranded の 2 試験は commit の author を見ない。
- 削除: `src/db_maintenance/tests.rs` の `incremental_backup_progress_restarts_after_other_connection_writes`。根拠の確認: 件名「定期DBバックアップの再開始ループと中断遅延を解消」の commit が定期 backup を `VACUUM INTO` に替え、ADR 2026-10-10-local-disk-growth-paths の現状の節が `VACUUM INTO` の方式を書いている。ADR-0064 D3 の「rusqlite の backup API」は置き換え前の記述。試験は DB に残る行や event を読まないので replay の契約ではない。`invariant=false` で、capability-map の evidence にも無い。rusqlite の `backup` feature は task-core が使うので、celeris の `Cargo.toml` から消すものは無い。

## mutation の結果（`sh agent-docs/progress/2026-11-10-health-v2-r5/mutate.sh <patch> celeris <filter>`）
patch は [`mutations/celeris/`](mutations/celeris/)。src は毎回 mutate.sh が戻し、戻した後の内容が当てる前と同じことを mutate.sh が確かめた（どの回も exit 3 は出ていない）。D29・D39・D49 の残す試験は script の bundle が無いと SKIP で抜けるので、`$TMPDIR` に control-ts の写し（`docs/api/script-host` を隣に置き、git init と `pnpm install --offline`）を作り、`CELERIS_SCRIPT_CONTROL_TS_DIR` と `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` を付けて走らせた。

| 候補 | patch | mutant | 結果 | 落ちた試験 |
|---|---|---|---|---|
| old_impl | `db-maintenance-keep-partial.patch` | `remove_backup_files` が partial の本体を消さない | KILLED | `incomplete_backups_are_cleaned_and_cancel_does_not_publish`（14 件中 13 件が通り、消した試験も通った） |
| old_impl | `db-maintenance-no-publish.patch` | `backup_once` が rename で公開しない | KILLED | `backup_once_writes_a_restorable_copy_and_keeps_recent_generations`・`spawned_tasks_run_on_their_interval_and_stop_cleanly`（消した試験は PASS） |
| D42 | `notify-empty-webhook-content.patch` | `post_webhook` の content を空にする | KILLED | `cos_chat_triage_scenarios` の 2 本、`root_done_summary` の 2 本 |
| D06 | `stranded-default-branch-unresolved.patch` | `default_branch_for` が常に解決できない | KILLED | `stranded_default_branch_null_with_main_is_integrated` |
| D34 | `knowledge-curation-no-push.patch` | 整理の apply が push しない | KILLED | 残す 2 本（unit 試験と `daily_curation`） |
| D08 | `instance-live-active-ignores-pid.patch` | `is_live_active` が pid を見ない | SURVIVED | 無し（残す 2 本はこの分岐の差を見ない） |
| D08 | `instance-standby-never-promotes.patch` | `decide_tick` の standby が昇格しない | KILLED | 残す 2 本（`wait_until` が 120 秒で false を返し assert が落ちた） |
| D29 | `script-layer-no-startup-rollback.patch` | 起動時に mode を rust へ戻さない | KILLED | 残す 2 本 |
| D39・D49 | `project-decider-ts-leaks-a-thread.patch` | `decide` のたびに眠る thread を 1 本残す（故障注入） | KILLED | `project_controller_replay_resource_v8_100_projects`（threads grew: idle 11, after 311） |

- filter: old_impl は `test(/^db_maintenance::tests::/)`、ほかは各候補の `keep_tests`（D42 は `root_done_summary` の binary 全体）。
- D08 の SURVIVED は helper を寄せた変更の結果ではなく、残す 2 試験が `is_live_active` の pid の判定を通らないことを示す。この組では試験を消していないので、削除の規則（SURVIVED なら消さない）には当たらない。

## 扱わなかった候補
- write set の外（数字を含む file 名、または試験でない src の file）: D04・D21・D26（`campaign_*_e2e.rs`）、D09（`src/daemon/classify_tick/e2e.rs`）、D25（`src/config/{jev,routing_mode,script}.rs` の試験 module）、D28（`ops_r1_measure.rs`）、D43（`src/daemon/adapters.rs`）。ADR D5-6 のとおり R6 か別の task に回す。
- D37（`is_timeout`）: `script_generation_e2e.rs` は write set の外。`script_compare_cos_digest.rs` の写しは `CosDigestCompareRecord` を受け、support の `is_timeout` は `ScriptCompareRecord` を受ける。型が違うので同じ関数ではなく、寄せていない。
- daemon を起こす他の写し（`script_runtime.rs` の `run_daemon` は観測値も返す、`script_generation_daemon.rs`・`routing_eval_run.rs`・`instance_handoff.rs` の `RunOptions` は値が違う）は候補に無いので触っていない。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0。対象は celeris・celerisctl。1006 tests run: 1006 passed（3 slow）、2 skipped。script bundle は台本が source から作り、SKIP 0 件。doc test は全 crate で失敗 0。repo-health は verdict=pass（base は `git merge-base HEAD main`、new=3・worsened=0。new の 3 件は R5 の先行 unit の `cases.py`・`classify.py`・`contracts.py` の `dup.contract` の warning で、この unit の差分ではない）。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（verdict=pass、new=0・worsened=0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate celeris --base "$CELERIS_WU_BASE"` → exit 0（試験関数 806 → 805、表の行 561 → 561、removals 1 件）。`--base "$(git merge-base HEAD main)"` も exit 0（不変条件の試験 321 件・35 file を照合）。
- 変えた binary の試験: `cargo nextest run -p celeris -E 'binary(cos_delivery_suite) | binary(daily_curation) | binary(instance_handoff) | binary(unpromoted_release) | binary(root_done_summary) | binary(long_run_soak) | binary(project_controller_replay) | binary(script_compare_cos_digest) | binary(script_compare_lane_rules) | test(/^knowledge_curation::tests::/)'` → 99 passed。bundle を付けた 4 binary（`script_compare_cos_digest`・`script_compare_lane_rules`・`long_run_soak`・`project_controller_replay`）→ 19 passed、1 skipped（`#[ignore]` の子 process 用）。
- `{ git diff --name-only "$CELERIS_WU_BASE"; git ls-files --others --exclude-standard; } | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`。`git merge-base HEAD main` 基準も ok。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → `crates/celeris/` の 20 file と `agent-docs/progress/2026-11-10-health-v2-r5/` の下（この file・`removals/celeris.json`・`mutations/celeris/` の 9 patch）だけ。
- `python3 scripts/dev/test-target-inventory.py --json` → test_targets 291。`support/` の 4 file は target ではない（`tests_dir_support_files` に数えられる）。
- 文書の検査（`check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check`・`check-human-text.py`）は報告本文に書く。

## commit
- `phase 1: celeris の試験の helper の写しを tests/support に寄せ、旧い backup 方式の計測試験を消す`
- `phase 1: dedupe-celeris の mutation の patch を足す`
- `phase 1: dedupe-celeris の進捗を書く`

## 未解決事項
- D08 の `is_live_active` の pid の判定（生きている active か）を落とす試験は、残す 2 本には無い。`instance.rs` の unit 試験が持つかは未確認。
- write set の外の候補 8 件（D04・D09・D21・D25・D26・D28・D37・D43）は残る。

## 提案
- R6（または write set を広げた task）で、write set の外の 8 件を同じ規則で寄せる。D37 は記録の型が 2 つあるので、`ScriptCompareRecord` と `CosDigestCompareRecord` の両方を受ける形にするかを先に決める。
- `instance-live-active-ignores-pid.patch` が SURVIVED だったので、`is_live_active` の pid の判定を固定する試験が `instance.rs` の unit 試験に有るかを確かめ、無ければ足す。
