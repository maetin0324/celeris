---
tasks: [01M7XQ9PS0AFKKMRJ7P5ZMKQQE]
---
# ADR 2026-11-27: R1 — 子の終端の数と no-progress の連続から決定的に escalate を導く純関数を parent_checkpoint に置き、controller の snapshot に載せる（本番は shadow のまま）

- 日付: 2026-11-27
- 状態: Accepted
- 関係: 戦略層 v1 親（task 01M7XQ9PS0AFKKMRJ7P5ZMKQQE）の計画 v2 の WU r1-core。人の決定（2026-11-27、初期戦略の replan は R1「決定的な下限を主にする」。after-r1 は b replan、2026-11-27 22:17Z ごろ）。
  [ADR 2026-11-22-parent-checkpoint-evaluator](2026-11-22-parent-checkpoint-evaluator.md)（D1 の strategy root・`triggers`・D5.2 の行動の表・D6 の到達）、
  [ADR 2026-11-04-project-controller](2026-11-04-project-controller.md)（wake→snapshot→TS decision→Rust gate→effect、shadow の mode）。
  識別実験の記録: `agent-docs/progress/2026-11-27-strategy-v1/r1-replay.md`・`r1-result.json`（r1-replay）、`r1-bench.md`・`r1-bench.json`（r1-bench）。

## 背景

- 子 9 の最終報告で、親の目的（改善 A〜D と observability）は未達だった。子 8 の pilot では、LLM の親の評価器 24 件のうち pivot・discriminating_experiment・off_track は 0 件だった。
- 人は R1 を選んだ: D6 の到達（Outcome 未達）・子の終端の数・no-progress の連続から決定的な規則で escalate を出す。LLM の評価器は識別実験の案を書くだけにする。子は増やさない。本番の controller は shadow のまま。
- R1 の規則は Python（`r1_rule.py` の `streak_after_reach`・`escalate`）にしか無く、controller の経路に載っていない。

## 決定

### D1: 純関数 `parent_checkpoint::stall`

- 入力は `TriggerFacts`（`triggers` と同じ型。root の直下の子の `ChildFacts` と strategy の有無）と `OutcomeReach`（今の Outcome の数 `required` と、D6 で到達した Outcome ごとの到達の順 `reached_at`）。
- 出力 `Stall`:
  - `terminal_children`: 最後の遷移が `Status::is_terminal` の直下の子の数（`triggers` と同じ数え方）。
  - `no_progress_streak`: 最後の到達（`reached_at` の最大）より後に終端になった子の数。到達が無ければ `terminal_children`。
  - `outcome_reached`: Outcome が 1 件以上あり、全部が到達のときだけ真（子 8 の `goal_reached`・D6 と同じ定義）。
  - `escalate`: `outcome_reached` が偽 かつ `terminal_children >= K` かつ `no_progress_streak >= K`。
  - `at`: `escalate` が真になった event の順（最後の到達より後に終端になった子の K 個目の終端の `at`）。偽なら `None`。
- strategy root でない root（`strategy` が `None`）は `None`。状態を持たず、I/O も LLM も無い。
- K は定数 `STALL_K = 3`。config の欄にしない（D3）。

### D2: snapshot の欄 `SnapshotCheckpoint.stall`

- `task_ops::project_snapshot::checkpoints` が strategy root ごとに `stall` を計算し、`SnapshotStall { k, terminal_children, no_progress_streak, outcome_reached, escalate, at }` を載せる。
- 到達の読み取りは既存の `recorded_outcomes` の 1 つの実装から取る。`project_controller_view::recorded_outcome_times` が到達を数える規則（Roadmap の `reached`・`mark_milestone` の判断）はそのままで、Outcome ごとに時刻を返す。`recorded_outcomes` はその鍵を返すだけにした（同じ規則を 2 つに書かない）。
  - Roadmap の到達の時刻: その Outcome の最後の状態の記録（`project_roadmap_outcome_events` の `kind = 'status'`）の `created_at`。
  - 判断の到達の時刻: 数えた `mark_milestone` の判断の行の `created_at`。
  - 同じ Outcome に複数あれば早い方。
- `project_controller_view::outcome_reach_times` は、今の Outcome のうち記録があり `pp::unverified_outcomes` に入らないもの（D6 の到達）の時刻を集める。dispatcher の `checkpoint_section` がこれを `checkpoints` に渡す（`project_snapshot` から `project_controller_view` を import しない。逆向きの依存は既にある）。
- 順の揃え方: 子の遷移の `at` は大域の event id の 20 桁（`triggers` と同じ）で、到達の記録は別の表にある。そこで到達 1 件の順を「その時刻までに終端になった直下の子の終端の event の順の最大（無ければ空文字列）」に直してから `stall` に渡す。
  - 終端の子の時刻は、既存の `events_since(id - 1, 1)` で同じ id の行の `ts` を引く（store に関数を足さない）。
  - 子の終端と到達が同じ時刻なら、子は到達より前に数える（Python の `t > last` と同じ）。
  - 時刻を読めない終端の子か到達があれば、その root の `stall` は `None` にする（推測で補わない）。
- TS に渡す snapshot の型は `docs/api/script-host/host-v1.schema.json` に無い（schema は Host API の DTO だけ。snapshot の型は `control-ts/src/domains/project_controller.ts` に手で書いてある）。この WU は schema と生成型 `control-ts/src/host.d.ts` を変えない。TS の入力の検査は `checkpoints` の中の欄を見ないので、欄を足しても今の TS の規則は変わらない。TS の型と規則 `pc/r1-stall` は WU r1-ts が足す。

### D3: K = 3 の根拠（事前に固定。結果を見て変えていない）

- r1-replay（子 0 の replay 12 点・研究ケース 7 点）で K = 3 は、replay で C-done、研究で 11.17 に初めて発火する。誤検知は replay 2 点（C-done・D-mid）、研究 3 点（11.17・11.18・11.19）、見逃しは 0 点。人の反証条件（A-done〜C-done で 2 回以上の発火、または F-done までに発火無し）には当たらない（`falsified: false`）。
- 感度（判定に使わない）: K = 2 は C-mid・C-done の 2 点で発火し、反証条件の前半に当たる。K = 4 は replay で D-done に初めて発火し、検知してよい最初の点 E-mid より後になる。
- r1-bench（long-horizon の記録 44 trial）は全 trial を除外した（root の終端の時刻と Outcome ごとの到達の時刻が facts に無い）。判定は inconclusive（`falsified: false`）で、K の根拠を足していない。
- Rust の試験 `parent_checkpoint_stall_r1_result_points_match` は r1-result.json の 19 点の値で、K = 3 の発火が Python と一致することを確かめる。`parent_checkpoint_stall_r1_bench_trials_match` は r1-bench の trial の終わりの状態だけを比べる。44 件中 42 件は非発火で一致する。a9 の C08 の new の 2 件（終端の root 4・`achieved_at` 無し）は Rust が発火し、記録は除外のため空で一致しない。r1-bench には点ごとの値が無いので、これは no-progress の部分（到達で連続が 0 に戻ること）の実の記録での確認にならない。

### D4: 反証条件

- R1 の no-progress の部分は、合成の入力の試験（`parent_checkpoint_stall_reach_resets_streak`・`parent_checkpoint_stall_snapshot_field_from_store`）でしか確かめていない。実の記録で最初に試せるのは計画 v2 の shadow-bench（long-horizon bench C01〜C10 に腕 shadow を足す）。
- 次のどちらかに当たれば R1 を本番の shadow に進めず、人の決定どおり R3 に移る:
  - F1: 途中到達のある trial で、到達の直後の `no_progress_streak` が 0 にならない到達がある。
  - F2: 必要な Outcome に全部到達した trial のうち、到達より前に K = 3 で発火した trial が 2 件以上ある。
- 途中到達のある trial が 3 件未満なら inconclusive とし、反証にしない（r1-bench と同じ）。

### D5: 本番は shadow のまま

- この ADR が足すのは純関数と snapshot の欄だけで、TS の規則・判断・効果は足さない。WU r1-ts の規則 `pc/r1-stall` も controller の mode に従い、本番の controller は shadow のままにする（効果を書かない）。mode・config の欄・本番 config は変えない。
- 本番への昇格・release.sh・canary・HPC・外部公開は人だけが行う。

### D6: 評価器の出口（D5.2 の行動の表）は今回変えない

- R1 は決定的な下限で、評価器の判定を経ずに escalate（人への質問）を出す。評価器の出口を変えなくても R1 は働く。
- 子 8 の pilot で評価器の 24 件の記録に pivot・discriminating_experiment・off_track が 0 件で、出口をどう変えればよいかを決める根拠が無い。
- 人の決定は「LLM の評価器は識別実験の案を書くだけにする」で、出口の変更は別の設計と人の承認が要る。この WU の範囲（store・config・新しい状態機械を足さない）を超える。

## 守る不変条件

- 状態の正本は増やさない: `stall` は store の event と既存の到達の記録から毎回導き、保存しない。新しい表・列・event の種類・migration・config の欄・Manager・trait・状態機械は無い。
- LLM を dispatcher・TS 決定器に入れない: `stall` は純関数。
- strategy の無い root と低抽象度の task は変わらない: `stall` は strategy root の checkpoint にだけ載る（`parent_checkpoint_stall_non_strategy_root_is_none`）。
- 到達の数え方は 1 つ: `recorded_outcomes` は `recorded_outcome_times` の鍵で、既存の到達の試験がそのまま通る。

## 撤去

- 無し。R1 の Python（`r1_rule.py`・`r1_bench.py`）は識別実験の記録として残し、controller の経路では使わない。
