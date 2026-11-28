---
tasks: [01M7XQ9PS0AFKKMRJ7P5ZMKQQE]
wu: r1-core
status: done
updated: 2026-11-28
completed: 2026-11-28
---
# 戦略層 v1 親 r1-core: R1（K = 3）を parent_checkpoint の純関数と controller の snapshot に載せる

## 結論

- 前提の確認: `r1-bench.json` は `falsified: false`（`inconclusive: true`）なので作業した。
- `task_core::parent_checkpoint::stall`（純関数）と定数 `STALL_K = 3`、snapshot の欄 `SnapshotCheckpoint.stall`（`SnapshotStall`）を足した。設計は [ADR 2026-11-27-r1-stall-shadow](../../adr/2026-11-27-r1-stall-shadow.md)。
- Rust の K = 3 の発火は r1-result.json の 19 点と一致する。r1-bench は点が 0 なので、trial の終わりの状態だけを比べた（44 件中 42 件が一致。a9 の C08 の new の 2 件は Rust が発火し、記録は除外のため空）。
- store・migrations・events の module・config・`docs/api/script-host/`・`control-ts/` は変えていない。TS の型と規則 `pc/r1-stall` は WU r1-ts が足す。本番昇格・release.sh はしていない。

## 実装前 brief

1. 根本原因: R1 の規則が Python（`r1_rule.py`）にしか無く、controller の snapshot に R1 の値が無い。TS の規則が R1 を読めない。
2. 責務 owner: 規則は `task_core::parent_checkpoint`（strategy root の checkpoint の純関数）、snapshot への載せ方は `task_ops::project_snapshot::checkpoints`、到達の数え方は `task_ops::project_controller_view::recorded_outcomes`。
3. 既存の再利用先: `TriggerFacts`・`ChildFacts`・`Status::is_terminal`（`triggers` と同じ）、`recorded_outcomes` と `pp::unverified_outcomes`（D6）、`store.events_since`・`store.roadmap_outcome_events`。
4. 選択肢: (a) 到達の順を時刻で比べるため子の遷移の `at` を時刻にする → `triggers` の `at`（TS に渡る wire）の意味が変わるので採らない。(b) 到達の時刻を「その時刻までに終端になった直下の子の終端の event の順の最大」に直して、子の `at`（event の順）と同じ順で比べる → 採用。store に関数を足さずに済む。
5. 新しい状態・分岐・fallback・port: 状態は無し（毎回導く）。時刻を読めない終端の子・到達があれば、その root の `stall` を `None` にする分岐を 1 つ足した（推測で補わないため）。
6. 撤去対象: 無し（置き換える旧い仕組みが無い）。`recorded_outcomes` は `recorded_outcome_times` の鍵を返す形にし、到達の規則は 1 か所に保った。
7. 不変条件: K = 3 の境界・到達で連続が 0 に戻る・strategy root 以外は値なし・Python と同じ発火（試験 `parent_checkpoint_stall_` 6 件）。既存の到達と checkpoint の試験はそのまま通る（test-affected）。

## 変更

- `crates/task-core/src/parent_checkpoint.rs`: `STALL_K`・`OutcomeReach`・`Stall`・`stall`。
- `crates/task-core/src/project_proposal.rs`: `SnapshotCheckpoint.stall` と `SnapshotStall`。
- `crates/task-ops/src/project_snapshot.rs`: `checkpoints` に引数 `reach: &OutcomeReachTimes` を足し、`reach_in_event_order` で到達の時刻を event の順に直して `stall` を載せる。
- `crates/task-ops/src/project_controller_view.rs`: `recorded_outcome_times`（Outcome ごとの到達の時刻）と `outcome_reach_times`（D6 の到達の時刻と Outcome の数）。`recorded_outcomes` はその鍵。
- `crates/task-dispatch/src/dispatcher/project_decider_ts.rs`: `checkpoint_section` が `outcome_reach_times` を渡す。
- 試験: `crates/task-core/src/parent_checkpoint/stall_tests.rs`（5 件）、`crates/task-ops/src/project_snapshot/stall_tests.rs`（1 件）。task-api の試験 3 file は `checkpoints` の新しい引数に既定値を渡すだけ。
- 文書: ADR 2026-11-27-r1-stall-shadow、`docs/architecture-map.md` の parent_checkpoint の行。

## 経緯（範囲の check と計画 v3）

- 1〜3 回目の run（2026-11-27）は、範囲の check（計画の check 4）の許可リストに `crates/task-ops/src/project_controller_view.rs` と task-api の試験 3 file（`parent_checkpoint_{shadow,regression,pivot_chain}.rs`）が無く、範囲外として落ちた。
  - task-api の試験 3 file は `checkpoints` を直接呼ぶので、引数 `reach` を足すと呼び出しを 1 行ずつ直す必要がある。旧い引数の形の wrapper は互換の分岐になるので置かない。
  - `project_controller_view.rs` は `recorded_outcomes` の owner。到達の時刻をこの file の外で読むと、記録を選ぶ規則の二重実装になる。また `project_controller_view` は `project_snapshot` を use するので、逆向きに呼ぶと module の循環になる。
  - 3 回目の run は plan_issue を申告した。2 回目の run の申告は result.json に summary も書いたため done として扱われ、計画に届かなかった。
- 計画 v3（人が 2026-11-28 00:08Z ごろに承認）は許可リストにこの 4 path を足した。4 回目の run（この記録）はコードを変えず、check を流し直して検査の節を書き直しただけ。

## 試験と検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

計画 v3 の check 0〜4 を 4 回目の run（2026-11-28）で流し直した。結果はすべて exit 0。

- check 0 `r1-bench.json` の `falsified` が `false`: exit 0（`inconclusive: true`）。
- check 1 `cargo nextest run -p task-core -E 'test(/parent_checkpoint_stall_/)'`: exit 0、5 passed（`k_boundary`・`reach_resets_streak`・`non_strategy_root_is_none`・`r1_result_points_match`・`r1_bench_trials_match`）。
  - あわせて `cargo nextest run -p task-ops -E 'test(/parent_checkpoint_stall_/)'`: exit 0、1 passed（`snapshot_field_from_store`）。
- check 2 `bash scripts/dev/test-affected.sh && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check`: すべて exit 0。
  - test-affected: 対象 14 crate の nextest 8,944 passed・35 skipped、doc test は失敗 0、web の tsc -b 通過・vitest 108 file 774 passed・node --test 110 passed、ts-verify 20 passed、script bundle の SKIP 0 件、repo-health verdict=pass・fail_grade=0・new 0・worsened 0。
  - 前の版で 1 回 exit 1 になった（source-size の inline の試験 module `checkpoint_tests` が 343 行で上限 300 を超えた）ので、snapshot の試験を `project_snapshot/stall_tests.rs` に移した。今回は落ちていない。
  - clippy: exit 0（警告は vendor の deno_core の既存の 1 件だけ）。fmt: exit 0。
- check 3 ADR の存在・`check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check`: exit 0（doc-links ok、adr-numbers ok 487 files、progress-index ok）。
- check 4 範囲（v3 の許可リスト）: exit 0。`sh "$CELERIS_WU_SCOPE_PATHS"` の 14 path に範囲外は無い。
- `node control-ts/scripts/gen-host-types.mjs --check`: exit 0（生成型は変わらない）。
- `git diff --quiet $CELERIS_WU_BASE HEAD -- crates/task-core/src/store crates/task-core/migrations crates/task-core/src/model/events.rs crates/celeris/src/config docs/api/script-host control-ts`: exit 0（store・migrations・events の module・config・schema・control-ts の差分 0）。

## LoC（base の script と toml を `git show` で取り出し、`--exclusive-classes` で base と HEAD に当てた）

| 欄 | base | HEAD | 差 |
|---|---|---|---|
| production 全言語（product_only） | 394,123 | 394,335 | +212 |
| production Rust | 346,942 | 347,154 | +212 |
| production TypeScript | 36,038 | 36,038 | 0 |
| own_total_with_tests | 1,090,145 | 1,090,662 | +517 |

純増の理由: 新しい規則（純関数）と snapshot の欄・到達の時刻の読み取りを足し、置き換える旧い仕組みが無い。

## 未確認

- 到達で連続が 0 に戻ることは合成の入力の試験だけで確かめた。実の記録での確認は shadow-bench が最初になる（ADR D4）。
- 本番の DB では Roadmap・Outcome の記録が 0 件なので、本番の snapshot では `outcome_reached` が常に偽になる見込み（未確認）。
- `docs/api/script-host/` の schema に snapshot の型は無い（objective の前提と違う）。schema と生成型は変えていない（ADR D2）。

## 提案

- WU r1-ts は `control-ts/src/domains/project_controller.ts` の `SnapshotCheckpoint` に `stall?: { k; terminal_children; no_progress_streak; outcome_reached; escalate; at }` を足して読む。`at` は `due[].at` と同じ大域の event id の 20 桁。
