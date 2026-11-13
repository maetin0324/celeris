---
title: "健全性v2 R4 own-coord-a: ExecutionCoordination を作り、write set・同期・依存・資格の待ちの field を移す"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-13
---
# 健全性v2 R4 own-coord-a

完了日: 2026-11-13

## 概要

- ADR 2026-11-11 dispatcher-state-ownership D1 の表で unit 列が own-coord-a の 14 field（`browser_ledger`・`browser_prereq_rescan`・`cluster_waiting`・
  `credential_busy`・`credential_held`・`integrated_deps`・`review_resync_halted`・`review_sync_queue`・`review_sync_seq`・`run_write_bases`・
  `write_reservations`・`write_set_hold_seq`・`write_set_waits`・`wu_prepare_failures`）を `Dispatcher` から消し、
  `crates/task-dispatch/src/execution_coordination.rs` の `ExecutionCoordination`（private field 14 件）に移した。`Dispatcher` の field 名は `coord`。
  D1 の表と実装は合っていた（ADR は変えていない）。
- 値の型 `WriteReservation`・`WriteSetWait`・`reservations_overlap`・`WRITE_SET_STARVATION_TICKS`（`write_set_gate.rs` から）、`ReviewSyncWait`
  （`stale_priority.rs` から）、`RunWriteBase`（`write_set_record.rs` から）、`WuPrepareFailures`（`dispatcher.rs` から）も同じ file に移した
  （owner が `dispatcher` module を使わないため。D2 の 3）。旧い定義は残していない。
- run の集合は持たない（D1「run の集合と lease の照合」）。write set の blocker の絞り込み（`admit_write_set`）と予約の刈り込み
  （`prune_write_set_state`）は、入口が `RunSupervisor::run_keys` で渡す実行中の run の鍵だけを見る。tick の番号も引数で受ける。
- D6: write-set gate の ablation は `Dispatcher::write_set_gate_disabled`（`TestHooks` の `ablation.disable_write_set_gate` を読む入口。
  本番は `false`）が読み、`ExecutionCoordination::admit_write_set` の引数 `gate_disabled` に渡す。owner の struct に `#[cfg(test)]` の field は無い。
  stale 優先の ablation（`disable_stale_priority`）は D1 の表のとおり own-coord-b が扱う（`order_review_sync_queue` の入口に残した）。
- 判定は変えていない: write set の予約の排他（同じ blocker の選び方・`RunKey` の順・先取りの規則・容量切れの引き継ぎ）、stale sync の優先
  （`stale_priority_cmp` は不変、待ち行列の出入りを owner の method に移しただけ）、credential の待ち（`credential_gate.rs` は field の操作を
  `hold_credential`・`release_credential_hold` に置き換えただけ）、browser の待ち（台帳の見張りの差し替え・再読込・見直しの予約を method に置き換えただけ）。
- 入口に残したもの: `write_set_gate.rs` の `admit_write_set`（`dispatch_one` の write-set gate の入口。予約を作り、ablation と実行中の run の鍵を
  引数にして owner に判定させ、`Hold` なら `TaskWait::WriteSet` を記録する）・`write_reservation_for`（store の hint から予約を作る）・`write_set_gate_disabled`、`stale_priority.rs` の
  `order_review_sync_queue`（behind の測定）・`note_review_sync_offer`（`awaiting_human` を読んで owner に bool で渡す。`awaiting_human` は own-coord-b）、
  `browser_prereq.rs`・`credential_gate.rs` の store の操作。
- `dispatcher/tests/structure_guard.rs` の `OWNER_STRUCTS` に `ExecutionCoordination` を足した。`structure_guard_execution_coordination` が
  D2 の 1〜3・7（field が private で 14 件、dispatcher の本体を受けない・`dispatcher` module と `crate::review` を使わない、`RunEntry`・`RunSupervisor<` を
  持たない）を source から確かめる。
- `dispatcher.rs` の module map に `execution_coordination` の行（層 owner）を足した。移した module の旧い行は無かった（`write_set_gate`・`stale_priority`・
  `browser_prereq`・`credential_gate`・`write_set_record` は map に行を持たない）。ADR-0082 に付記、`docs/architecture-map.md` に owner の行を足し、
  write-set 予約 gate の行の entry point に `execution_coordination.rs` を足した。
- 基準は `$CELERIS_WU_BASE`。

## 実装前 brief

1. 根本原因: write set の予約・review 前 sync の待ち行列・依存の統合・WU の用意の失敗・credential・browser・クラスタの待ちが `Dispatcher` の field で、
   `dispatch_run.rs`・`work_units.rs`・`review_spawn.rs` ほかどの module でも直接書けた。write set の blocker と刈り込みは `self.supervisor.runs()` を
   直に読み、gate の ablation は `impl Dispatcher` の method の中の `#[cfg(test)]` 分岐が判断と同じ関数にあった。
2. 責務 owner: `ExecutionCoordination`（`execution_coordination.rs`）。
3. 既存の再利用先: `task_ops::dep_integration::integration_wait`・`task_core::write_set::write_set_overlap`・`LedgerWatch`、既存の試験
   （`tests/{write_set_gate,stale_priority,target_sync,write_set_record,browser_ledger_gate,browser_policy_missing,work_unit_dependency_base,scope_checks,cluster_tunnel,cron_jobs}.rs`、
   phase_effect_ab::write_set、cex_* の反例試験）。
4. 選択肢: field を包んで `self.coord.write_set_waits` と書くだけ（不採用。D2 の梱包だけ）／owner に run の表の参照を持たせる（不採用。D1 が禁じる）／
   値の型と状態の操作を owner に移し、run の鍵・tick・時刻・store・ablation は引数で受ける（採用）。
5. 新しい状態・分岐・fallback・port: 無し。`WriteSetAdmission`（`Admit(予約)`・`Hold{blocker}`）は gate の結果の値で、今の `Option<RunKey>` と
   「予約を置くか」の 2 つを 1 つの戻り値にしたもの。
6. 撤去対象: `Dispatcher` の 14 field と初期化、`impl Dispatcher` の `write_set_blocker`・`note_write_set_hold`・`reserve_write_set`・
   `prune_write_set_state`・`carry_write_set_waits`・`order_write_set_starved_first`・`enter_review_sync_queue`・`refresh_browser_ledger`、
   `write_set_gate.rs`・`stale_priority.rs`・`write_set_record.rs`・`dispatcher.rs` の値の型の旧い定義、`credential_busy` の swap の写し。
7. 不変条件と試験: 下の「試験の追加と変更」。

## Run 2: 前回の check の不合格と直し

- 不合格: 計画の check `celeris:repo-health/1` が rule `size.responsibility-growth` の worsened（`dispatch_run.rs` の `fn Dispatcher::dispatch_one`）で落ちた。`Dispatcher::dispatch_one` の
  関数の行が base（`$CELERIS_WU_BASE`）の 1102 から 1108 に増えた。Run 1 で gate の結果を `match` で受け、`Hold` の待ちの記録を `dispatch_one` の中に
  書いたため。Run 1 の test-affected の中の health は main の merge-base を base にしていて、この悪化を数えなかった。
- 直し: gate の入口を `write_set_gate.rs` の `Dispatcher::admit_write_set` にまとめた（予約を作る・ablation を読む・実行中の run の鍵と tick を渡す・
  `Hold` の待ちを記録する）。`dispatch_one` は結果を受けて `Hold` で `Ok(false)` を返すだけにした。`dispatch_one` は 1089 行（base 1102 以下）。
  `Dispatcher` の method の数は 382（base 389 以下）。判定（`ExecutionCoordination::admit_write_set`）と待ちの記録の内容は変えていない。

## 試験の追加と変更

- 足した試験（`execution_coordination.rs`。doc comment に守る不変条件と既存の試験で検出できない理由を書いた）:
  - `write_set_blockers_come_from_the_running_keys_passed_in`: blocker は渡された実行中の run の鍵にある run の予約だけから選び、刈り込みは渡された鍵に
    無い予約を捨てる。ablation の `gate_disabled = true` では照合も予約もしない。dispatcher 経由の試験は run の表と予約がいつも揃うので、鍵を無視しても通る。
  - `wu_prepare_failures_count_up_to_the_limit`: 一時的な失敗は上限の手前まで待ちの記録を置き、上限で記録を消す。
  - `structure_guard_execution_coordination`: 上の概要のとおり。
- 変えた試験: field の直参照を owner の method に置き換えた（`write_set_waits()`・`review_sync_queue()`・`review_resync_halted()`・`integrated_deps()`・
  `is_cluster_waiting`・`wu_prepare_failures()`・`has_run_write_bases()`、待ちの期限だけを過ぎたことにする `expire_wu_prepare_backoff`。読み取りの
  accessor と期限の差し替えは `#[cfg(test)] impl` で、struct の field ではない）。型の import を `crate::execution_coordination` に直した。assertion は変えていない。
  `cex_review_authority.rs` の doc comment の「Dispatcher の private field」を owner の状態に直した（試験の本体は不変）。

## 証拠（提出時の HEAD で実行）

commit（件名）: 「phase 1: move write set, sync, dependency and credential waits to ExecutionCoordination」「phase 1: own-coord-a の進捗を書く」
「phase 1: write-set gate の入口を write_set_gate.rs に移し dispatch_one を base より短くする」「phase 1: own-coord-a の進捗に Run 2 の直しを書く」。

- `python3 scripts/dev/repo-health/repo_health.py --repo scripts/dev --base <base> --head worktree --policy-version 1`: base が `$CELERIS_WU_BASE`・
  `git merge-base HEAD $CELERIS_WU_TARGET`・`git merge-base HEAD main` のどれでも exit 0、verdict=pass、fail_grade=0、worsened=0。
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 3869 passed・11 skipped（402 秒）、repo-health verdict=pass fail_grade=0。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: exit 0（repo_health_gate: all checks passed）。
- 計画の check 4（D1 の表で unit 列が own-coord-a の 14 行を引き、`pub struct Dispatcher` の field に 1 件も無いこと）: 14 行、残り 0 件。
- 計画の check 5: `execution_coordination.rs` に `(impl|&mut|&) *Dispatcher` は 0 件、`structure_guard.rs` に `ExecutionCoordination` がある。
  `cargo nextest run -p task-dispatch -E 'test(structure_guard_) | test(phase_effect_ab) | test(cex_) | test(write_set) | test(execution_coordination)'`: 55 passed。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変更は `crates/task-dispatch/src/`・`agent-docs/adr/0082-dispatcher-module-split.md`・
  `docs/architecture-map.md`・この進捗だけで、範囲外 0 件。

## 未解決事項

- `awaiting_human`（stale 優先の救済の判定で読む）・`disable_stale_priority` の引数化・`checking`・`integrating`・`pending_subjects` などは
  D1 の表のとおり own-coord-b が `ExecutionCoordination` に移す。`structure_guard_execution_coordination` の field の一覧も own-coord-b が足す。

## 提案

- 無し。
