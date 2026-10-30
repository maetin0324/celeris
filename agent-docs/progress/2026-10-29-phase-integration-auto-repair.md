---
title: 段の統合の衝突を人を待たずに自動で解消する
tasks: [01M6320TM5XCF2QA8XXTT57BRE]
status: done
updated: 2026-10-29
---
# 段の統合の衝突を人を待たずに自動で解消する

完了日: 2026-10-29

ADR: [2026-10-29-phase-integration-auto-repair](../adr/2026-10-29-phase-integration-auto-repair.md)

## 変更
- `phase 1: 段の統合の衝突を merge の repair WU で自動に解き、尽きたときだけ人に回す`
  - `task-dispatch` `phase_integration.rs`: 衝突（auto_resolve の `NeedsHuman` を含む）は統合の依頼にせず merge の repair WU に渡す。
    repair の objective に依頼の markdown（両側の commit・理由）を足す。上限は衝突した WU ごとに 2 回（`MAX_MERGE_REPAIRS_PER_UNIT`）。
    上限に達したときだけ、理由「自動の解消が失敗: …」を付けた統合の依頼を記録する。
  - `phase_integration/merge_repair.rs`（新）: merge の repair WU の数え方と、失敗時に replan せず `cancelled` にして統合 WU を
    `pending` に戻す `retire_if_failed`（`worker_finish.rs` から 1 行で呼ぶ）。
  - `task-ops` `integrate_task/phase.rs`（新）: 依頼で止まった統合 WU の特定・再開（受信箱の回答 API と共有）、古い依頼を自動の
    解消へ戻す `retry_unrepaired_requests`。`celeris` の stranded sweep から呼ぶ。
  - `task-api` `inbox_notifications.rs`: 段の依頼の回答を共有関数に置き換えた。
  - ADR 新規 1 本、ADR 2026-10-28-cos-integrate-task と 2026-10-02-parallel-integration-auto-resolve に付記、`cos-inbox-triage` の規則。
- `phase 1: 長い TMPDIR と共有 process で落ちる celeris の試験を直し、進捗を書く`
  - `browser_doctor_tests.rs`: unix socket を短い dir（`/tmp` の下の tempdir）に置く（run の TMPDIR は 93 字で `SUN_LEN` を超える）。
  - `classify_stage/tests.rs`: `set_default` の後に `tracing::callsite::rebuild_interest_cache()`（同じ process の他の試験が
    callsite の interest を無効で覚えていると log が出ない）。
- `phase 1: 深い TMPDIR で socket を bind できない試験と tracing の interest 競合を直す`（attempt 2。前回は `cargo test --workspace` が
  credentiald の broker 試験で落ちて不合格）
  - unix socket を置く試験の一時 dir を `short_tempdir()`（TMPDIR が 40 字を超えるときは `/tmp` の下）に寄せた:
    `task-worker` は `test_support::short_tempdir`、`celeris-credentiald`・`task-api`・`task-worker` の結合試験は各 file の同名 helper、
    `celerisctl` と `task-api` `browser_waits` は `/tmp` の tempdir。
  - `classify_stage/tests.rs`: dispatcher が 1 つだけだと tracing-core は callsite の interest を登録した thread の既定で決める
    （`has_just_one`）。subscriber の無い別 thread が先に登録すると無効で固まるので、2 つ目の `Dispatch` を試験の間だけ生かす。
- `phase 1: python の browser 試験にも短い TMPDIR を渡す`: `python_transport_security_regressions_are_part_of_workspace_gate` が
  起動する python の unittest に短い `TMPDIR` を渡す（`AF_UNIX path too long`）。

## 試験
- `dispatcher::tests::work_units::phase_integration_conflict_is_resolved_by_a_merge_repair_without_a_person`（新）: 内容衝突 →
  repair WU が両側を残して解く → 統合再開 → 最終 review（検査と reviewer の合格）→ done。統合の依頼は出ない。
- `phase_integration_request_is_the_only_inbox_item`（改）: repair WU が 2 回失敗 → replan せず統合の依頼 1 件、理由と受信箱の本文に
  「自動の解消が失敗」と repair WU の key が出る。
- `task-ops` `sweep_hands_unrepaired_phase_requests_back_to_the_automatic_repair`・
  `sweep_leaves_requests_after_the_automatic_repair_and_delivery_requests`（新）。
- `phase 1: SQLite の memory 統計を load 時に切り、cargo test の futex 競合をなくす`（attempt 3。前回は `cargo test --workspace` が
  評価器の上限 1200 秒で打ち切られた。手元でも 1772 秒）
  - 1 process の試験 thread 群が bundled SQLite の memory 統計の mutex で競合していた（task-dispatch の unit 試験で kernel 時間 6563 秒）。
    `task-core` `sqlite_process.rs`（新）が `.init_array` で `SQLITE_CONFIG_MEMSTATUS` を 0 にする。ADR
    [2026-10-29-sqlite-memstatus-off](../adr/2026-10-29-sqlite-memstatus-off.md)。
  - task-core の unit 試験 148 → 12 秒、task-dispatch 375 → 37 秒、`cargo test --workspace` 1772 → 829 秒。

- `phase 1: 空の KB の根で cwd に index.json を書かないようにし、誤って入れた index.json を消す`（attempt 4。前回は repo の根に
  `index.json`（KB 索引の派生物、items 0 件）を commit して差し戻し）
  - 原因: `CELERIS_KNOWLEDGE_ROOT=`（空）で `celerisctl knowledge search` を repo の根で走らせると、根が `""` になり
    `ensure_index` が cwd に `index.json` を書く（手元で再現）。前回 run の手元の操作で出た file とみられる。どの試験も
    `CELERIS_KNOWLEDGE_ROOT` を空にしていない。
  - `task-ops` `knowledge.rs`: `resolve_root` は空の path（flag・env・config）を指定なしとして扱う。`reindex` は空の根を拒む。
    試験 `an_empty_root_writes_no_index`（新）と `the_root_comes_from_the_flag_then_the_env_then_the_config`（改）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo test --workspace`（run の TMPDIR、93 字のまま）→ exit 0、失敗 0（`test result: ok` 326 行）、829 秒
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 6729 passed、21 skipped）
- `cargo clippy --workspace -- -D warnings` → exit 0（`--all-targets` も exit 0）
- `cargo fmt --all -- --check` → exit 0
- 試験後に `/tmp/cz*` の残りは 0 件

## 本番への移行
- config・migration の変更は無い。配備（release・promote）は人が行う。
- 昇格後、最初の stranded sweep（起動直後）が依頼 01M62ZEN0WPZ0YY64N8YGFGPPF（task 01M62S8XWCZDCY2ZBVQ35176MH の `integrate-fix`）を
  `retry`（by daemon）で閉じ、統合をやり直す。衝突は repair WU `merge-fix-<unit>` が解く。確認: task の events に
  `IntegrationAnswered { answer: "retry", by: "daemon" }` と `RepairScheduled { class: "merge_conflict" }` が出ること。
- `config/skills/cos-inbox-triage` の変更は本番 KB の `skills/` に人が取り込む。policy_version は 6 のまま。

## 未解決
- 木の親 task（子が kind task の unit）の段の統合で repair WU が走る経路は、コードを読んで同じ経路と判断した。実 daemon での確認は未実施。

## 提案
- run の TMPDIR を短くする（例 `runs/<run>/t`）。今回は試験側を短い dir に寄せたが、新しい socket 試験が `tempfile::tempdir()` を
  使うと同じ失敗が戻る。
