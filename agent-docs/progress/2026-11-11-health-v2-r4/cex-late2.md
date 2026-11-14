---
title: "健全性v2 R4 cex-late2: 段の repair・replan の後に届く旧い統合・旧い review の合格の反例試験と guard の確認"
tasks: [01M7DYMCVS1PMDK2N0WW3ME9T8]
status: running
updated: 2026-11-14
---
# 健全性v2 R4 cex-late2

`cex-late.md` の「残り」1〜3 を終える task。WU ごとに節を分ける。この節は WU `late-integ`（段の repair の後に届く旧い統合の
完了）の結果。基準は `$CELERIS_WU_BASE`。本体（`cfg(test)` の外）は変えていない。

## late-integ: 段の repair の後に届く旧い統合の完了

### 試験

| 試験 | 守る不変条件（要約） | 結果 |
|---|---|---|
| `cex_late_integration_after_stage_repair_is_dropped` | 段 `build` の統合の検査が走っている間に段の repair（planner の replan が足す `kind = repair` の unit `repair-build-1`。`RepairScheduled{origin: planner}`）が先に確定したら、旧い統合（repair の前の HEAD の候補）の完了は捨てる。repair の done の前は `PhaseIntegrated` を付けず、統合 WU を done にせず、Task を Reviewing・Done に進めない。`PhaseIntegrated` は repair の done の後に、repair の commit（`r.txt`）を含む HEAD にだけ付く。Task は repair を含む候補で Done | 合格（4 本の file 全体も合格） |

順序の作り方（`crates/task-dispatch/src/dispatcher/tests/cex_late_completion.rs`）:

1. `adopt_blocking_plan` で段 `build`（WU `a`・`b`）を採用する。`a` の検査は統合の作業ツリーで最初に走ったときだけ、
   試験が `<marks>/open` を置くまで止まる。
2. `run_until_state` で `<marks>/first` と `IntegrationCheckStarted` を待つ（統合 WU は running）。
3. `task_ops::execution::replan`（`PlanOrigin::Planner`）で段 `build` に `kind = repair` の unit を足す。
   `RepairScheduled{key: repair-build-1, origin: planner}` が記録されたことを assert する。
4. `<marks>/open` を置いて旧い統合の完了を届ける。repair の run は fake adapter の gate（`holding`）で止める。
5. `run_until_state` で repair の run の開始を待つ。統合の process が残る間は Task の WU を起こさない（`dispatch_parallel_work_units`
   と D1.7 の Ready 戻しが `is_integrating` を見る）ので、repair の run の開始は旧い統合の完了の処理の後。ここで「repair の
   done の前」の assert をする。
6. gate を開け（`notify_one`。permit は待ちの前に出しても残る）、Task が終端になるまで待ち、event の順と HEAD を assert する。

既存の試験で検出できない理由: replan の変種（`cex_late_review_and_integration_after_replan_are_dropped`）は葉の WU を足し、
`PhaseIntegrated.merged` の key で新旧の統合を見分けられる。repair の unit は Task の worktree で走り merge の item に
ならないので、key では旧い統合と新しい統合を見分けられない。この試験は HEAD に repair の commit があるかで見分ける。

### guard の一時変更と結果

| 一時変更（本体） | 結果 |
|---|---|
| `phase_integration.rs` の `on_integration_finished` の古い結果の破棄 `if task.status != Status::Running \|\| integ.status != task_core::WorkUnitStatus::Running` から `\|\| integ.status != …Running` を外す | `cex_late_integration_after_stage_repair_is_dropped` が落ちた: 「旧い統合の完了が repair の done の前に PhaseIntegrated を付けた: repair_done=None integrated=[["a", "b"]] integ_done=Some(32) reviewing=None done=None status=Running」。同じ一時変更で既存の `cex_late_review_and_integration_after_replan_are_dropped` も落ちた（integrated=[(33, ["a", "b"])]、c_done=Some(39)）。他の 2 本は合格 |

一時変更は `git checkout -- crates/task-dispatch/src/dispatcher/phase_integration.rs` で戻した（commit していない）。戻した後、
4 本すべて合格。

### 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-dispatch -E 'test(/cex_late_completion::/)'`: 4 本すべて合格、exit 0。
- 上の一時変更の下で同じコマンド（`--no-fail-fast`）: 4 本中 2 本が失敗（上の表）、`error: test run failed`。戻した後は 4 本合格。
- `cargo nextest run -p task-dispatch -E 'test(=dispatcher::tests::cex_late_completion::cex_late_integration_after_stage_repair_is_dropped)'` を 3 回: 3 回とも合格（各 約 1.1 秒）。
- `bash scripts/dev/test-affected.sh`（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）: exit 0、3944 本合格・11 本 skip（nextest の Summary）、doc test も合格。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0（差分無し）。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: `crates/task-dispatch/src/dispatcher/tests/cex_late_completion.rs` と、この file だけ。

### 変更（commit の件名）

- `late-integ: 段の repair の後に届く旧い統合の完了の反例試験を足し、guard を外すと落ちることを確かめる`

### 未解決事項

- 無し（反例は今の実装で成り立った）。
