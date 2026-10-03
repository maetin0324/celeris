---
title: docs/ を人向け（docs/）と agent 向け（agent-docs/）に分け、並列 task でも衝突しない記録の形にする
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
status: done
updated: 2026-10-02
---
# docs/ を人向け（docs/）と agent 向け（agent-docs/）に分け、並列 task でも衝突しない記録の形にする

完了日: 2026-10-02。方式は [ADR-0128](../adr/0128-docs-layout.md)。分類と移動先の一覧は `scripts/dev/docs-layout.tsv`
（human 28・agent 164・delete 14 行。移動 176 件）。段ごとの WorkUnit の記録は [2026-10-02-docs-layout/](2026-10-02-docs-layout/move-docs.md) の下にある。

## 結果

- `docs/`（人向け）: 31 ファイル。SPEC.md・architecture-map.md・api/・protocol/・guides/（8 本）・ops/（5 本）と、移行期間の案内 `docs/adr/README.md`・`docs/progress/README.md`。
- `agent-docs/`（agent 向け）: 184 ファイル。ADR・進捗・報告・作業規則。入口は [agent-docs/README.md](../README.md)。
- 進捗は task ごとのファイル（D3）、索引は `sh scripts/dev/progress-index.sh` で生成（D4）、ADR は番号でなく日付+slug（D5）。
- 検査台本: `check-doc-links.sh`・`check-doc-layout.sh`・`check-adr-numbers.sh`・`progress-index.sh --check`（自己試験は `scripts/dev/testdata/`）。
- `docs/DESIGN.md` は人の決定（design-md）どおり削除し、CLAUDE.md の参照は SPEC.md へ変えた。

## land-verify: main の取り込み（ADR-0128 D6）

main `f1904ecd` を merge した（merge commit `48cd7afa`、親 `675a9c96` と `f1904ecd`）。衝突は `docs/architecture-map.md` の 1 件で、
新パス側の表を採り、main が足した launcher の行（`browser_launcher/mod.rs`・ADR-0115/0116）を新パスで足した。

task の base `0d438ec1` 以降に main が旧配置へ足したものを、次のように移した（tsv の末尾 9 行にも記録）:

| 旧（main が足した場所） | 新 |
|---|---|
| `docs/adr/0116-browser-launcher-implementation.md`・`0125-deterministic-time-tests.md`・`0127-skills-native-delivery.md` | `agent-docs/adr/` の同名 |
| `docs/progress/time-dependent-tests.md` | `agent-docs/progress/2026-10-02-time-dependent-tests.md` |
| `docs/progress/time-dependent-tests-{dispatch,injection,kill}.md` | `agent-docs/progress/2026-10-02-time-dependent-tests-fix/{dispatch,injection,kill}.md` |
| `docs/progress/phase-skills-progressive.md` | `agent-docs/progress/2026-10-02-skills-native-delivery.md` |
| `agent-docs/PROGRESS.md` に入った節「browser: ADR-0115 権限分離 launcher」「browser: ptrace 境界分離 launcher 実装・実 process 実証完了」（land-main・land-main2・land-main3・pick-chrome を含む） | `agent-docs/progress/2026-10-02-browser-launcher.md` |
| 同「codex・opencode への skill の付属ファイルと段階的な読み込み」 | `agent-docs/progress/2026-10-02-skills-native-delivery.md` の末尾 |
| 同「release 準備失敗の切り分け（0d438ec1）」 | `agent-docs/progress/2026-10-02-release-prepare-failure.md` |
| 同「時間依存試験の決定化」（(6)・人が実行する手順・実環境での確認を含む） | `agent-docs/progress/2026-10-02-time-dependent-tests-fix.md` |
| `docs/ops/browser-launcher-host-setup.md` | 移さない（人が行う host 準備手順なので docs/ops/ に残し、docs/README.md の一覧に足した） |

移した節の本文は変えず、相対リンクだけ新配置へ直した。各ファイルに D3 の front matter を付けた。
main が `agent-docs/PROGRESS.md` の既存行（prompt-rule 節の引用）を書き換えた 1 行は main の版のまま残した。

## 削除したファイル（D9）

内容は `git show <最後の commit>:<旧パス>` で取り出せる。

| 旧パス | 段（WU） | 理由 | 最後の commit |
|---|---|---|---|
| `docs/DESIGN.md` | move-docs | 人の決定（design-md）で削除。仕様の入口は docs/SPEC.md、CLAUDE.md の参照もそちらへ | `ea69c29a` |
| `docs/gui/bootstrap/CLAUDE.md` | move-docs | 初期配布用の作業規則。現行は gui/CLAUDE.md にあり重複。人の確認で削除 | `926e19c0` |
| `docs/gui/bootstrap/GOAL_TEMPLATE.md` | move-docs | 初期配布用テンプレート。現行は gui/docs/GOAL_TEMPLATE.md にあり重複。人の確認で削除 | `926e19c0` |
| `docs/gui/bootstrap/PROGRESS.md` | move-docs | G0〜G5 を未着手とする初期状態。現行の進捗は gui/docs/PROGRESS.md。人の確認で削除 | `926e19c0` |
| `docs/gui/bootstrap/README.md` | move-docs | run-gphases.sh による gui/ 作成を指示するが gui/ は実装済み・台本は無い。人の確認で削除 | `926e19c0` |
| `docs/gui/bootstrap/agents/auditor.md` | move-docs | 初期コピー元。現行は gui/.claude/agents/auditor.md。人の確認で削除 | `926e19c0` |
| `docs/gui/bootstrap/agents/implementer.md` | move-docs | 初期コピー元。現行は gui/.claude/agents/implementer.md。人の確認で削除 | `926e19c0` |
| `docs/ops/adr-0079-r5b-runbook.md` | move-docs | ADR-0079 R5b の一度きりの作業手順。人の確認で削除（JSON フィクスチャは crates/task-api/tests/fixtures/ へ切り出し） | `243009c3` |
| `docs/ops/home-nfs-migration-2026-09-25.md` | move-docs | 2026-09-25 の一度きりの移行手順。人の確認で削除 | `d0e78511` |
| `docs/web/dogfood.md` | move-docs | 一度きりの作業文書（手順と未決定の記録が混在）。現行の運用は docs/ops/web-parallel-operation.md。人の確認で削除 | `c63d53c2` |
| `docs/execution-architecture-2026-09-24.md`（移動後 `agent-docs/reports/execution-architecture-2026-09-24.md`） | cleanup-reports | ADR-0072 前の現状調査。Task:run 1:1・dispatcher.rs の行番号が今の実装と食い違う | `a42f9a54` |
| `docs/execution-decomposition-report-2026-09-25.md`（移動後 `agent-docs/reports/…`） | cleanup-reports | E6 時点の分析。挙げた問題は Phase F で実装済み、gate 提案は ADR-0079 で置き換わった | `a42f9a54` |
| `docs/execution-parallel-report-2026-09-28.md`（移動後 `agent-docs/reports/…`） | cleanup-reports | 案件計画（ADR-0074 D3）は ADR-0079 で廃止。決定は ADR-0074 本文にある | `a42f9a54` |
| `docs/notes/build-cache-tiering-input-2026-09-28.md`（移動後 `agent-docs/notes/…`） | cleanup-reports | ADR-0075 の入力メモ。方針は ADR-0075 と実装に入り、現状の記述（sccache 未導入など）は食い違う | `a42f9a54` |

統合（ファイルは残る）: `docs/celeris-api-v1.md` → `docs/api/v1/overview.md` の中身は cleanup-api で `docs/api/v1/gui-api.md` §3.125 へ統合した。
`overview.md` は gui/docs/celeris-api-v1.md（変更禁止）の参照を保つ転送ページとして残る（統合前の最後の commit `a42f9a54`）。
guides・ops・api・protocol の各文書は節の削除・修正だけで、ファイルの削除は無い（cleanup・cleanup-ops・cleanup-api の記録）。

## 検証（land-verify、HEAD = merge 後）

| コマンド | exit | 要点 |
|---|---|---|
| `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok`（全体） |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 | `check-doc-layout: ok`（merge 前は cleanup-reports で消した 4 本の行が agent のままで 4 件違反。delete に直した） |
| `sh scripts/dev/check-adr-numbers.sh` | 0 | `ok (116 files)` |
| `sh scripts/dev/progress-index.sh --check` | 0 | `ok` |
| `python3 scripts/dev/check-architecture-map.py` | 0 | 191 件のパスを確認 |
| `cargo fmt --all -- --check` | 0 | 差分なし |
| `cargo clippy --workspace -- -D warnings` | 0 | 警告なし |
| `cargo test -p task-ops --lib` | 0 | 400 passed |
| `cargo test -p task-api --lib` | 0 | 72 passed、2 ignored |
| `cargo test -p task-worker --lib` | 101 | 703 passed、7 failed、4 ignored。失敗は `browser::tests` の 7 件だけで、全て `isolated_runtime_unavailable`（run の sandbox では隔離 browser runtime を起こせない環境要因。この task の crates の差分は docs パスの文字列だけ） |

workspace 全体の `cargo test --workspace` は daemon の workspace check に任せた（Objective どおり）。

## 未解決

- `cargo test -p task-worker --lib` の `browser::tests` 7 件は、隔離 runtime が使える環境（daemon の workspace check）で通ることを確かめる必要がある。
- `deploy/systemd/celeris-web@.service` のコメントが旧パス `docs/web/parallel-operation.md` を指したまま（cleanup-ops の提案。deploy/ は昇格で sha12 確認になるため未変更）。
- `gui/docs/celeris-api-v1.md`（gui/ 側の写し）は正本とずれている（refs-repo の記録）。gui/ はこの task では変えない。
- `docs/ops/selfdeploy.md` §4e（一時回避の撤去手順）は、本番の drop-in が残っているので残した。人が撤去したら消してよい。
- `gui/app/` のコメントは gui/docs/ と旧 `docs/DESIGN.md` を指す（gui/ の範囲外）。

## 提案

- 移行期間の終わり（merge-base が move-docs の merge より前の `celeris/*` task branch が無くなったとき）に、次を後続 task で行う:
  - 旧ディレクトリの案内 `docs/adr/README.md`・`docs/progress/README.md` を削除する。
  - `agent-docs/PROGRESS.md` と `agent-docs/progress/phase-F.md` の壊れたリンクを直し、`check-doc-links.sh` の対象外（`MIGRATION_EXCLUDE`）から外す。
    同時に `scripts/dev/check-adr-numbers.sh` も対象外から外し、旧ディレクトリの列挙を消す（refs-repo の提案）。
  - それまでに main に入った旧配置の ADR・進捗・PROGRESS.md 末尾の節は、land する task が本ファイルの「land-verify」節と同じ規則で移す。
- `deploy/systemd/celeris-web@.service` のコメントを `docs/ops/web-parallel-operation.md` へ直す（昇格の sha12 確認が要る）。
- `crates/task-worker/src/scratch/tests.rs:835` の ETXTBSY は `crate::test_support::write_executable` で書くよう直す（cleanup-ops の提案）。

## sync-main-2: main の取り込み（5d6df9f3）

main `5d6df9f3` を取り込んだ。衝突は `agent-docs/PROGRESS.md` の時間依存試験節のみ。main が旧 `docs/PROGRESS.md` に追加した原因・方式の 6 行を、ADR-0128 D6 に従い `agent-docs/progress/2026-10-02-time-dependent-tests-fix.md` の該当節へ移した。凍結した `agent-docs/PROGRESS.md` には追記せず、`docs/PROGRESS.md`・`docs/DESIGN.md` は復活させていない。

| 文書検査 | exit |
|---|---:|
| `sh scripts/dev/check-doc-links.sh` | 0 |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 |
| `sh scripts/dev/check-adr-numbers.sh` | 0 |
| `sh scripts/dev/progress-index.sh --check` | 0 |

build cache 判定: **残っている**。`crates/task-dispatch/src/dispatcher/tests/mod.rs:3026` の sccache 系 helper は `run_until_idle(&mut d, 60).await`、同 `:3027` は直後に `Status::Done` を assert する。`crates/task-dispatch/src/dispatcher/tests/build_cache.rs:396-401` の `every_cargo_path_uses_the_scratch_target_dir` は `run_until_state` で `Done` を待つ形に直っている（同 `:489-494` の task 単位経路も同様）。

## cdp-sink-wait（main ba8302c0 同期）

`crates/task-worker/tests/browser_cdp_sink.rs` を `ba8302c0` の版に揃えた（60 秒応答 timeout、Browser.getVersion の期限付き再試行、fixture 起動待ち 60 秒）。試験は 2 件中 `inner_cdp_sink` が成功し、`real_browser_injection_receipt_and_origin_guards` は `unshare: unshare failed: Operation not permitted` で失敗（exit 101）。namespace 作成前の環境制約で止まるため、この run では受け入れ条件を満たさず plan_issue。

| コマンド | exit | 要点 |
|---|---:|---|
| `timeout 1800s cargo test -p task-worker --test browser_cdp_sink` | 101 | 1 passed、1 failed。実 browser 試験で unshare が Operation not permitted |
| `cargo fmt --all -- --check` | 0 | 差分なし |
| `cargo clippy -p task-worker --all-targets -- -D warnings` | 0 | 警告なし |

## sync-main-2 merge-latest

2026-10-03、main `40189604`（`163f41c0` を含む）を merge commit `0231b07e` で取り込んだ。13 件の衝突は次のように解消した。

- `crates/task-dispatch/src/dispatcher/worker_task.rs`、`crates/task-worker/src/{adapter.rs,probe.rs,langmem_run.py}`、`scripts/selfdeploy/install-units.sh`: main の実装を基にし、移動後に必要な文書パスだけを保持した。`browser_launcher_ptrace.rs` と `browser_launcher/` は main と一致する。
- main で撤去した `crates/scratch-cache/tests/sccache_webdav_e2e.rs`、`crates/task-worker/tests/scratch_sccache_e2e.rs` は削除した。
- `docs/web/dogfood.md` は削除のまま。main の変更は `docs/web/parallel-operation.md` → `docs/ops/web-parallel-operation.md` と `docs/workspace.md` → `docs/guides/workspace.md` の移動先に反映し、旧パスは削除した。
- `docs/architecture-map.md` は新パスの参照を保ちつつ、main の ADR-0132/0133/0134 の参照・受信箱の行を反映し、撤去された cache server の行を削除した。`docs/ops/nextest.md` は両側の趣旨を保持し、撤去済み sccache への参照を除いた。
- `agent-docs/PROGRESS.md` は既存行を残し、main が旧 `docs/PROGRESS.md` に加えた節を末尾へ移した。旧 `docs/PROGRESS.md` と `docs/DESIGN.md` は復活していない。新しい節の task 別文書への移動は後続の `migrate-docs` で行う。

| コマンド | exit | 結果 |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | 差分なし |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | 警告なし |
| `cargo test -p task-worker --test browser_launcher_ptrace` | 101 | 5 passed、1 failed。`launcher_chrome_denies_daemon_uid_ptrace` は Chrome PID がこの run の `/proc` に現れず失敗。従前の `Protocol` エラーは出ていない |
| `cargo test -p task-worker --test browser_launcher_ptrace launcher_chrome_denies_daemon_uid_ptrace -- --exact --nocapture` | 101 | 単独再実行も同じ `/proc` の Chrome PID 不可視で失敗 |

実 launcher 試験は run の PID namespace から host 側 Chrome を観測できない環境制約とみられる。host の設定・daemon・launcher は変更していない。ログはこの run の成果物ディレクトリに `browser_launcher_ptrace.log` と `browser_launcher_ptrace_exact.log` として残した。

## sync-main-2 migrate-docs

2026-10-03（task 01M3Z8CXYG1J6BZCQ87YS3FC67、WorkUnit migrate-docs、土台 `67d4e807` = main `40189604` 取り込み後）。merge-latest で main から旧配置に入ったものを ADR-0128 D6 で新配置へ移した。コードの挙動は変えていない。

- ADR: `docs/adr/` の 0129・0132・0133・0134・0135・0138 を `agent-docs/adr/` へ同名で `git mv`。`docs/adr/` には README.md だけが残る。
  0128 超えの番号は振り直さず、`check-adr-numbers.sh` の `ALLOWED_OVER_LAST` に完全なファイル名で書いた（ADR-0128 末尾の付記 2026-10-03）。
- 進捗: `docs/progress/` の 5 本を移した（`docs/progress/` には README.md だけが残る）: `phase-inbox-notifications.md` → `2026-10-02-inbox-notifications.md`（front matter を足した）、
  `provider-llm-source-inventory.md` → `2026-10-02-provider-llm-source/inventory.md`、`reflink-target-experiment.md`・`sccache-sync-recheck.md`・`seed-refresh.md` → `2026-10-02-host-sccache-reflink/` の下。
- `docs/testing/time-dependent-waits.md`（task 01M3ZCXG… の走査記録）は agent 側と分類し `2026-10-02-deterministic-time-waits/inventory.md` へ。`docs/ops/`・`docs/api/v1/` の新しい人向け 5 本はそのまま（tsv に human 行）。tsv の末尾に 17 行を足した。
- `agent-docs/PROGRESS.md`: main が足した 816 行（途中の 2 か所と末尾 792 行）を task ごとの進捗ファイルへ移し、本文を merge-base `c4885655` の版に戻した（D6 の凍結）。移した先:
  `2026-10-01-browser-prod-admission.md`（新）、`2026-10-02-deterministic-time-waits.md`（新）、`2026-10-02-host-sccache-reflink.md`（新）、`2026-10-02-provider-llm-source.md`（新）、
  `2026-10-02-web-follow-health-gate.md`（新）、`2026-10-02-blocked-repair-replan-loop.md`（新）、既存の `2026-10-02-browser-launcher.md`・`2026-10-02-skills-native-delivery.md`・`2026-10-02-inbox-notifications.md` の末尾。
  落としていないことの確認: main が足した空でない 661 行を 1 行ずつ `grep -qxF` で `agent-docs/progress/` の全ファイルに照合し、見つからない行 0。
- 参照: 移した ADR・進捗・`docs/ops/*` の相対リンク、`scripts/host-sccache/*` のコメント（`agent-docs/adr/0129-…`）、`scripts/selfdeploy/install-units.sh` のコメント（`docs/ops/web-parallel-operation.md`）を新パスへ直した。
  main に無い ADR-0136（`0136-local-hot-data-layout.md`）へのリンク 3 か所はリンクを外し「main 未取り込み」と書いた。移した本文中の当時のコマンド記録（`grep … docs/adr/0129-…` など）は記録なので変えていない。

### build cache 判定（取り込み後）: 残っている

ADR-0125 の状態待ち（`run_until_state`、`STATE_WAIT_GUARD` 60 秒、`crates/task-dispatch/src/dispatcher/tests/mod.rs:2643-2661`）は main で入ったが、sccache 系 helper と build_cache.rs の多くは tick 回数で止まる `run_until_idle(&mut d, 60)` の直後に `Status::Done` を assert したまま。

- `crates/task-dispatch/src/dispatcher/tests/mod.rs:2990-2991`（`run_scratch_env`。`build_cache.rs:525` の `scratch_runs_get_target_and_cargo_tuning_but_no_sccache` が使う）— 残っている。
- `crates/task-dispatch/src/dispatcher/tests/build_cache.rs`: 残っている — 24-25（`the_preamble_notes_the_shared_build_cache_when_enabled`）、54-55（`the_preamble_does_not_note_the_shared_build_cache_when_disabled`）、
  96-98（`shared_build_cache_sets_cargo_target_dir_for_a_local_git_worktree_on_the_host`）、141-143（`shared_build_cache_disabled_does_not_set_cargo_target_dir`）、
  733-734（`scratch_on_nfs_falls_back_to_build_cache_dir`）、822-823（`run_start_adopts_a_finished_target_that_predates_the_checkout`）。
- 直っている: 269-273（`parallel_work_units_get_their_own_cargo_target_dir_and_it_is_removed_when_done`）、396-400・489-493（`every_cargo_path_uses_the_scratch_target_dir`）、325・329 は `run_until_state` で状態待ち。

### 文書検査（migrate-docs 後）

| コマンド | exit | 結果 |
|---|---:|---|
| `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok`（移動前は 41 件の壊れた参照） |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 | `check-doc-layout: ok` |
| `sh scripts/dev/check-adr-numbers.sh` | 0 | `check-adr-numbers: ok (122 files)`（移動前は 0128 超え 6 件で違反。一時ファイル `0139-x.md` を置くと exit 1 で違反を出すことも確かめ、消した） |
| `sh scripts/dev/progress-index.sh --check` | 0 | `progress-index --check: ok` |

未解決: Task の受け入れ条件 3（`celeris/01M3YBGM…` との merge-base からの差分が agent-docs と dispatcher/tests だけ）は、merge-latest で main を取り込んだ時点で 269 file の差分があり成り立たない（本 WU の前から）。条件の基点の見直しが要る。

## sync-main-2 gate: 検証

2026-10-03（WorkUnit gate-run、HEAD `09d2ddd4`）。指定 gate を foreground で実行した。

| コマンド | exit | 結果 |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | 差分なし |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | 警告なし |
| `timeout 3000s cargo test --workspace --no-fail-fast` | 101 | 20 targets failed。task-worker lib は 722 passed / 12 failed / 4 ignored、task-core 653 passed、task-dispatch 516 passed、task-ops 436 passed。失敗対象・件数: e2e cluster_scenarios 2/6、delegation_scenarios 5/5、multi_account_scenarios 3/3、phase7_scenarios 5/5、plan_scenarios 2/2、scenarios 4/4、worker_db_read_only 1/1; task-api browser_h3_injection 1/3、browser_restore_deliver 1/1、browser_restore_live_session 1/1; task-worker lib 12/734、browser_cdp_sink 1/2、browser_egress_relay 1/2、browser_h3_wire 1/2、browser_injection_attacks 1/2、browser_injection_wire 1/3、browser_launcher_ptrace 1/6、browser_restore_deliver 4/4、browser_runtime_isolated 4/9、browser_runtime_supervisor 1/4。失敗名は直下の「失敗の詳細」を参照 |
| `timeout 300s cargo test -p e2e --test cluster_scenarios -- --nocapture` | 101 | 4 passed / 2 failed。`unknown_cluster_is_unroutable` と `missing_control_master_records_cluster_unavailable_and_does_not_block_idle` は daemon 起動時の db_guard user namespace probe が `Operation not permitted` |
| `timeout 300s cargo test -p task-worker --test browser_launcher_ptrace launcher_chrome_denies_daemon_uid_ptrace -- --exact --nocapture` | 101 | 0 passed / 1 failed。`launcher_chrome_denies_daemon_uid_ptrace`: launcher は起動し isolation は確認できたが、20 秒間 Chrome PID が `/proc` に現れず。workspace run の失敗と同じ |

失敗の詳細（workspace run）:

- `e2e/cluster_scenarios`: `unknown_cluster_is_unroutable`, `missing_control_master_records_cluster_unavailable_and_does_not_block_idle`。
- `e2e/delegation_scenarios`: `when_the_parent_cannot_retry_it_asks_a_human_instead_of_failing`, `non_aggregate_lead_completes_after_children_without_another_run`, `a_failed_child_makes_the_parent_retry_instead_of_inheriting_the_failure`, `lead_delegates_children_waits_for_them_and_aggregates_once`, `on_child_failure_ignore_keeps_the_old_behaviour`。
- `e2e/multi_account_scenarios`: `task_without_a_matching_provider_does_not_block_until_idle`, `second_account_runs_the_overflow_when_the_first_is_at_capacity`, `throttled_account_falls_back_to_the_next_account`。
- `e2e/phase7_scenarios`: `human_check_asks_again_after_a_retry_and_orphaned_approvals_are_cancelled`, `answer_is_delivered_in_context_answers_and_cli_checks_complete_the_task`, `persistent_provider_failure_stops_after_max_requeues`, `provider_failure_requeues_without_consuming_attempts`, `cancel_is_limited_to_non_terminal_tasks_and_failures_cancel_dependents`。
- `e2e/plan_scenarios`: `celerisctl_plan_generates_children_that_complete_after_human_approval`, `invalid_plan_is_retried_once_with_prior_review`。
- `e2e/scenarios`: `command_check_fails_once_then_passes_after_retry`, `three_tasks_with_dependency_all_done_at_concurrency_two`, `worker_crash_without_terminal_message_fails_after_retries`, `expired_lease_is_reclaimed_and_task_completes`。
- `e2e/worker_db_read_only`: `a_codex_worker_run_cannot_write_the_daemon_db_but_can_read_it_with_celerisctl`。
- `task-api/browser_h3_injection`: `production_h3_injects_once_without_exposure`; `browser_restore_deliver`: `restore_enters_observation_stop_until_session_end`; `browser_restore_live_session`: `restore_http_binds_to_real_isolated_session_and_never_opens_on_refusal`。
- `task-worker` lib: browser の実 runtime 7 件（`production_action_path_reaches_fixture_through_real_browser_and_egress`, `launch_uses_generated_policy_and_binds_its_hash_to_the_run`, `opencode_and_claude_share_supervised_browser_lifecycle_artifacts_and_cleanup`, `actions_and_domains_outside_task_policy_are_stopped_before_the_substrate`, `specialist_wraps_existing_harness_and_runs_same_browser_task`, `execution_fallback_uses_fresh_session_and_refuses_without_conformance`, `cleanup_failure_marks_browser_failed_instead_of_reporting_completion`）と db_guard 5 件（`ssh_config_includes_still_parse_inside_the_namespace`, `a_nested_user_namespace_cannot_undo_the_read_only_mount`, `the_db_and_its_wal_files_are_read_only_but_siblings_stay_writable`, `probe_confirms_the_db_is_not_writable_inside`, `the_guarded_process_has_no_capabilities_and_keeps_its_uid`）。
- `task-worker/browser_cdp_sink`: `real_browser_injection_receipt_and_origin_guards`; `browser_egress_relay`: `fixture_reachable_only_through_per_connection_egress_proxy`; `browser_h3_wire`: `real_broker_browser_injection_receipt_and_origin_guards`; `browser_injection_attacks`: `real_browser_injection_attack_matrix`; `browser_injection_wire`: `real_broker_browser_injection_receipt_and_origin_guards`。
- `task-worker/browser_launcher_ptrace`: `launcher_chrome_denies_daemon_uid_ptrace`（単体再実行も失敗。Chrome PID の `/proc` 可視性）。`browser_restore_deliver`: `live_session_delivers_restored_state_over_its_own_cdp_pipe`, `restored_session_refuses_agent_observation`, `supervisor_entry_delivers_restored_state_to_controller_cdp_under_harness_admission`, `identity_restore_sameuid_rejected_in_production`。
- `task-worker/browser_runtime_isolated`: `probe_inside_runtime_cannot_reach_host_sockets_or_network`, `restart_reaps_recorded_runtime_and_ignores_stale_records`, `real_browser_in_runtime_facts_and_restore_refused_on_same_uid`, `controller_kill_leaves_no_runtime_processes`; `browser_runtime_supervisor`: `runtime_processes_do_not_survive_controller_kill_restart_or_stop`。

失敗ログは `unshare`/bwrap の `Operation not permitted` または `isolated_runtime_unavailable`、e2e では daemon の db_guard userns probe が同じく `Operation not permitted`。従って失敗は当該 task の文書変更と無関係な sandbox 制約として記録する。対象失敗ファイル群について `git diff --stat $(git merge-base HEAD main) HEAD -- <paths>` は `crates/task-worker/tests/browser_runtime_isolated.rs | 2 +-, 1 file changed` のみ（この task の既存1行差分）で、上記 e2e/browser/db_guard 失敗対象ファイルには差分がない。

記録後の文書検査は下記 4 本を再実行し、全て exit 0。

| コマンド | exit | 結果 |
|---|---:|---|
| `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok` |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 | `check-doc-layout: ok` |
| `sh scripts/dev/check-adr-numbers.sh` | 0 | `check-adr-numbers: ok (122 files)` |
| `sh scripts/dev/progress-index.sh --check` | 0 | `progress-index --check: ok` |
