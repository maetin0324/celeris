---
title: "健全性v2 R4 own-orchestration: ProjectOrchestration を作り、D1 の表の残りの周辺 field（保守・probe）を移す"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-13
completed: 2026-11-13
---
# 健全性v2 R4 own-orchestration

## 概要

- ADR 2026-11-11 dispatcher-state-ownership D1 の表で unit 列が own-orchestration の 7 field を `Dispatcher` から表の owner 列の置き場へ移した。
  - `cos_chat_launch`・`stagnation` → 新しい `crates/task-dispatch/src/project_orchestration.rs` の `ProjectOrchestration<L>`。停滞の判定の状態の型
    `StagnationWatch` も `tree_units.rs` から移し、field を private にした。`L` は CoS chat の起動の状態（`CosChatLaunch`。dispatcher の型なので型引数で受ける）。
  - `health_inventory`・`script_compare` → 既存の `HostMaintenance`（`HostMaintenance<C>`。`C` は dispatcher の型 `ScriptCompareState`）。Busy の印の型
    `InventoryRuns` は `health_review/inventory.rs` から `host_maintenance.rs` に移した（`inventory.rs` は re-export と `port` の関数だけ）。
  - `eval_version_probe`・`knowledge_probe`・`now_unix_fn` → 新しい `crates/task-dispatch/src/probe_ports.rs` の `ProbePorts`（本番の既定を持つ注入 port。
    `#[cfg(test)]` ではない）。`KnowledgeProbe` の定義もここへ移し、`dispatcher::KnowledgeProbe` は re-export で同じ path のまま。
- 書き込みは owner の method を通す。
  - tree_units: `configure_stagnation`（閾値の差し替え。番号・待ちの印・換算単価は保つ）、`begin_stagnation_scan(now)`（判定するかは既存の純粋関数
    `task_ops::stagnation::scan_due`。始めるなら時刻を置き、結果を「今の状態でない」にして閾値を返す）、`record_stagnation(findings)`（結果を入れ番号を 1 進める）、
    `mark_waits_known`、`set_budget_rates`。
  - cos_chat の launch: `install_launch`・`remove_launch`（verify）・`take_launch` と `restore_launch`（tick の間の取り出しと戻し）・`launch_mut`。
  - 停滞の通知の冪等性（番号は成功のたびに 1 進み、閾値を差し替えても戻さない）と CoS chat の起動の待ち（tick の取り出しと戻し、slot の待ち）は変えていない。
- owner は `Dispatcher` を受けず `dispatcher` module を使わない（D2 の 2・3）。`ProbePorts` は owner が持たず、入口が読んだ値を引数で渡す。
- `OWNER_STRUCTS`（`dispatcher/tests/structure_guard.rs`）に `ProjectOrchestration<L>`・`StagnationWatch`・`ProbePorts`・`InventoryRuns` を足し、
  `HostMaintenance` を `HostMaintenance<C>` に直した。
- 文書: `dispatcher.rs` の module map（`project_orchestration`・`probe_ports` の行を足し、`host_maintenance`・`tree_units` の行を直した。移した module の
  旧い行は無い）、ADR-0082 の付記、`docs/architecture-map.md` の owner の行 2 つ（`ProjectOrchestration`・`ProbePorts`）。ADR 2026-11-11 は変えていない。
- commit: 「phase 1: ProjectOrchestration と ProbePorts を作り、停滞・CoS chat の起動・棚卸し・TS の比較・probe の field を Dispatcher から移す」、
  「phase 1: own-orchestration の module map・ADR-0082 の付記・地図・進捗を書く」。

## WU check の不合格の直し（2 回目の run）

- 1 回目の提出の後、計画の WU check（`grep -nE '(impl|&mut|&) *Dispatcher\b' crates/task-dispatch/src/project_orchestration.rs` が 0 件であること）が
  exit 1 になった。当たったのは `structure_guard_project_orchestration` の doc comment の文言（「`&mut Dispatcher` を受けても」）で、コードではない。
- doc comment を「Dispatcher の可変参照を受けても」に書き換えた。`probe_ports.rs` の同じ文言もそろえた（check は見ないが同じ種類の文）。コードと試験の中身は変えていない。
- commit: 「phase 1: own-orchestration の structure_guard の doc comment を WU check の grep に当たらない文言にする」。
- 直した後の HEAD で実行: WU check（上の grep と `cargo nextest run -p task-dispatch -E 'test(structure_guard_) | test(cex_)'`）exit 0（36 passed）、
  `bash scripts/dev/test-affected.sh` exit 0（3875 passed、11 skipped、約 11 分）、`cargo clippy --workspace -- -D warnings` exit 0、
  `cargo fmt --all -- --check` exit 0、`check-health.sh --base "$CELERIS_WU_BASE"` exit 0、`check-architecture-map.py` exit 0、`progress-index.sh --check` exit 0。

## 受け入れ条件

- 0（D1 の表で unit 列が own-orchestration の行の field が `Dispatcher` に無い）: `pub struct Dispatcher` の本体に 7 field の名は無い（下の証拠の grep）。
- 1（`ProjectOrchestration` は `Dispatcher` を参照せず、structure_guard が数える）: `structure_guard_project_orchestration` が `&Dispatcher`・`&mut Dispatcher`・
  `impl Dispatcher`・`crate::dispatcher`・`super::dispatcher` の不在と field の private を確かめ、`OWNER_STRUCTS` に入れたので
  `structure_guard_owner_structs_have_no_test_fields` が `#[cfg(test)]` の field を数える。

## 試験の追加と変更

- 追加 `project_orchestration::tests::stagnation_generation_survives_reconfigure_and_scan_waits_for_waits`: 守る不変条件は、メモリの待ちが揃う前は判定しない・
  判定が成功するたびに番号が 1 進み閾値の差し替えで戻らない・判定を始めた後に成功しなければ結果は今の状態でない。既存の試験は store と tick を通して
  findings を見るだけで、番号が 0 に戻っても findings は同じなので検出できない。
- 追加 `project_orchestration::tests::launch_take_and_restore_keep_the_state`: tick の間に取り出した起動の状態は戻すまで見えず、戻すと同じ値が見える。
  既存の CoS chat の試験は tick の前後だけを見るので、owner の method の契約を直接は固定しない。
- 追加 `structure_guard_project_orchestration`・`structure_guard_probe_ports`: D2 の 1〜3 を source から確かめる（振る舞いの試験は field の公開や
  `&mut Dispatcher` の受け取りを検出できない）。
- 変更: 試験の field の直参照を owner の method に置き換えた（`d.stagnation.checked_at = …` → `set_stagnation_checked_at_for_test`、
  `d.stagnation.findings = …` → `set_stagnation_findings_for_test`、`d.cos_chat_launch.as_ref()` → `d.orchestration.launch()`、
  `d.health_inventory` → `d.maint.health_inventory_mut()`、`(d.now_unix_fn)()` → `d.probes.now_unix()`）。assertion は変えていない。消した試験は無い。

## 未解決

- `ScriptCompareState` の試験専用の field `next_digest_pass_id`（`#[cfg(test)]`）は今の場所に残した。`HostMaintenance` の struct には `#[cfg(test)]` の
  field は無いが、`HostMaintenance` が持つ `ScriptCompareState` の中には残る。`TestHooks` へ移すと、repo-health の size 規則（`rules/size_growth.py`）の
  test mask が、それまで数えていなかった `script_compare.rs` の `impl Dispatcher` の 12 method を数え始め、`Dispatcher` の method 数が 382 から 394 に
  見かけ上増えて `size.responsibility-growth` が fail（worsened）になった（この run で確かめた。method は増やしていない）。
- `HostMaintenance<C>`・`ProjectOrchestration<L>` は dispatcher の型を型引数で受ける（owner が `dispatcher` module を使わないため。own-coord-a・b の
  `ExecutionCoordination<S, C>` と同じ形）。`ScriptCompareState` と `CosChatLaunch` の中身の field は今も入口（`script_compare.rs`・`script_canary.rs`・
  `cos_chat/*`）が直に読み書きする。
- 本番への移行: 無し（config・DB・API は変えていない）。

## 提案

- `next_digest_pass_id` を `TestHooks` へ移す変更と、size 規則の test mask の直し（`#[cfg(test)]` の付いた struct の field の後の item を test として
  隠さない）を、policy だけの branch で先に main に入れる。直した規則で `Dispatcher` の method 数の base が 394 になってから移せば fail しない。
- `ScriptCompareState` と `CosChatLaunch` の field を private にし、入口の直の読み書きを owner の method に寄せる（record か後の task）。

## 証拠（提出時の HEAD で実行）

基準は `$CELERIS_WU_BASE`（WU の base）。

- `bash scripts/dev/test-affected.sh`: exit 0（3875 件の nextest を含む。約 12 分）。
- `cargo nextest run -p task-dispatch -E 'test(/structure_guard|project_orchestration|probe_ports|host_maintenance|stagnation|cos_chat|cos_digest|…/)'`:
  675 passed（停滞・CoS chat・棚卸し・TS の比較・probe・structure_guard の試験）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"`: exit 0（verdict=pass、fail_grade 0、new 0、worsened 0）。
- `python3 scripts/dev/check-architecture-map.py`: exit 0。`sh scripts/dev/progress-index.sh --check`: exit 0。
- `awk '/^pub struct Dispatcher \{/,/^\}/' crates/task-dispatch/src/dispatcher.rs | grep -cE '^\s+(cos_chat_launch|stagnation|health_inventory|script_compare|eval_version_probe|knowledge_probe|now_unix_fn):'`: 0。
