---
title: 戦略層 v1 子 6 controller-launch — 親の途中評価の reviewer run の起動・決着と task-dispatch の試験
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: controller-launch
status: done
updated: 2026-11-23
---
# controller-launch（WorkUnit）

完了日: 2026-11-23

状態: 終了（run 1 で起動・決着を実装、run 2 で試験 11 件・ADR と運用文書の直し・検査、run 3 で進捗と提出時の検査）。
設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md) の D2.3・D3・D11。
基準はこの WU の `$CELERIS_WU_BASE`（repo health と test-affected は `git merge-base HEAD main` も見る）。検査は提出時の HEAD で実行した。

## 実装前 brief

1. 根本原因: controller-checkpoint の run 2 が done と読まれ、`RequestEvaluation` の効果（何も書かず `applied`）の後に評価の reviewer run を起こす段と、終わりの決着が無かった。
   症状は「`evaluate` の判断が `applied` になっても reviewer run も最終の記録も出ない」（下の実装前の失敗）。
2. 責務 owner: run の起動・実行・終わりの記録は `health_review/design_gate.rs` の共通の起動口。起動の条件・試行の記録・決着は `crate::review::parent_evaluation`（新）。
   tick の呼び出しは `dispatcher/project_wake.rs` の `tick_project_controller`。
3. 既存の再利用先: `start_side_review`・`on_side_review_finished`・`holds_side_review`、`supervisor.has_task_run`、`tree_limit_hold_for`、
   `parent_checkpoint::{checkpoint_states, check_evaluation, ParentEvaluationRecord, attempt_artifact_name, attempt_of, evaluation_decision_id}`、
   `strategy_reads::strategy_task_creates`、`project_decider_ts::decision_proposal`、`token_budget::tree_tokens_used`。試験は `tests/project_cos.rs` の fake の TS（`FakeTs`）と `TsProjectDecider::with_fixed_script`。
4. 選択肢: (a) `Dispatcher` に method を足す（method の数が閾値の上で worsened）、(b) dispatcher の子の新しい file（dispatcher module の本体の行が増える）、
   (c) `crate::review` の自由関数が `&mut Dispatcher` を受けて起動口を呼ぶ。(c) を採った。`tree_limit_hold_for` は `dispatcher` の中だけで見える method なので、
   可視性を変えず（`dispatcher/tree_units.rs` は範囲外）`project_wake.rs` の呼び出しが関数（`parent_evaluation::TreeHold`）として渡す。
5. 新しい状態・分岐・fallback・port: 新しい状態は無し（実行中・試行の数・評価済みは判断の行と成果物の name から毎回導く）。分岐は `SideReviewKind::ParentEvaluation(Subject)`
   （判断の id と起動の時点の試行の上限）。可視性の変更: `design_gate` の mod・`SideReviewKind`・`start_side_review`・`Dispatcher.supervisor`・`decision_proposal` を `pub(crate)`（行は足さない）。
6. 撤去対象: 無し（新しい経路の追加だけ）。行数の相殺のため `decide_exhausted_notice` を `dispatcher/project_wake.rs` から `crate::project_controller` に移した（中身は同じ）。
7. 不変条件と試験: 下の「試験」の表。

## 変更

- `strategy-v1 6 controller-launch: 親の途中評価の run の起動・試行の記録・決着を review/parent_evaluation.rs の自由関数に置き、共通の起動口の種類と終わりの腕・holds_side_review の評価の半分を足す`
  — `launch`（mode `enabled` の案件の実行中の `evaluate` の判断ごとに、`has_task_run(root)` が偽・木の上限が止めない・試行 < 上限・strategy root のときだけ `start_side_review`、
  run id が返れば試行の記録 `parent-evaluation/<id>.attempt-<n>.json` を `ArtifactProduced`〈kind `parent_evaluation_attempt`〉で書く。上限で run が無ければ `invalid` の最終の記録）、
  `settle`（`review.json` の `parent_evaluation` を `check_evaluation` に通し、`checked` か上限の `invalid` で最終の記録 `parent-evaluation/<id>.json`）、`in_flight`（`holds_side_review` の評価の半分）。
- `strategy-v1 6 controller-launch: 親の途中評価の起動・決着の試験 project_controller_checkpoint_ を足し、木の上限の判定を project_wake.rs から渡して tree_units.rs の変更を戻す`
- `strategy-v1 6 controller-launch: ADR の D2.3・D3.4・D11 の置き場と運用文書の checkpoint の手順を実装に合わせて直す` — ADR は本文をその場で直した（付記なし）。
  `docs/ops/project-controller.md`: 評価の run が起きる時点・試行の上限での `invalid`・shadow で発火済みの trigger は `enabled` にしても評価しないこと・mode を戻したときの走っている run の扱い・確認の手順。
- `strategy-v1 6 controller-launch: final review の run の試験の runs の行を SQL で置き、run_index_start を持つ file を増やさない`
- 進捗（この file と controller-checkpoint.md の status）。

## 試験（`crates/task-dispatch/src/dispatcher/tests/project_checkpoint.rs`、`tests/project_cos.rs` の `#[path]` の子 module）

proposal は decider の差し替え（`TsProjectDecider::with_fixed_script` + fake の TS が snapshot の checkpoint から `evaluate`・guard の `escalate`・`wait` を返す）で与え、
Rust の検証・効果・起動口は本物を通す。評価の run の adapter は `review.json` を書いて `done` で終える偽物（LLM・外部ネットワークなし）。
task-core・task-ops の既存の試験は判断の行と成果物の name を手で置くので、起動の段が無くても通る（run 2 の誤読を検出できなかった）。

| 試験 | 守る不変条件 | 実装前 |
|---|---|---|
| `..._fires_one_run_and_settles` | `children:2` で `evaluate` が `applied`、同じ tick で評価の run 1 回と `attempt-1`、走る間 `holds_side_review` が真、終わりで最終の記録（checked）、request に `parent_checkpoint` の節 | fail |
| `..._effect_once_is_idempotent` | run の間・後の wake で `evaluate`・試行・reviewer run・runs の reviewer の行が 1 のまま | fail |
| `..._crash_boundaries_start_one_run` | B4（tx1 の後）・B6（`applied` の後）で落ちて別 instance で再開しても、判断 1 件 `applied`・試行 1・run 1・最終の記録 1 | fail |
| `..._no_strategy_root_or_disabled_starts_no_run` | strategy の無い root（`evaluate` は検証で落ちる）と controller の無い案件で reviewer run・runs の行・成果物が 0 | pass（否定の試験） |
| `..._shadow_starts_no_reviewer_run` | shadow で `evaluate` は `suppressed` 1 件、reviewer run・試行 0 | pass（否定の試験） |
| `..._suppressed_due_does_not_open` | `suppressed` は発火済みで実行中にならない。`enabled` の後の次の trigger（`children:4`）の評価は阻まれず run 1 回 | fail |
| `..._no_progress_escalates_while_unmapped` | 写していない評価がある間、TS の `pc/no-progress` の `escalate` が検証を通って `applied`、評価の run は増えない | fail |
| `..._final_review_run_is_not_an_attempt` | root の final review の run（`WorkerStarted`・runs の reviewer の行）を試行に数えず `attempt-1` | fail |
| `..._waits_while_root_has_local_run` | root の run が手元にある間は起こさず、無くなった次の tick で起こす | fail |
| `..._attempt_limit_records_invalid` | 節の無い `review.json` は上限の前は最終の記録を書かず再試行、上限（2）で `invalid`、写しの表は `escalate` だけ | fail |
| `..._limit_without_run_records_invalid` | 上限に達して run が無いとき、起動の段が `invalid` の最終の記録を書き run を起こさない | fail |

実装前の失敗の記録: 実装を run 1 で先に置いたため、試験を先に置けなかった。代わりに `project_wake.rs` の `parent_evaluation::launch(...)` の呼び出しを外した状態
（起動の段が無い状態。終わりの腕はこの段からしか届かない）で `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(/project_controller_checkpoint_/)'` を走らせ、
11 件中 9 件 fail・2 件 pass（上の表の否定の試験）だった。fail の例: `fires_one_run_and_settles` は試行の記録が `[]`（期待 `attempt-1`）、
`limit_without_run_records_invalid` は最終の記録が `[]`。呼び出しを戻すと 11 件 pass。

## 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(/project_controller_checkpoint_/)'` → exit 0（11 passed）
- `cargo nextest run -p task-dispatch -p task-core --no-fail-fast -E 'test(/parent_checkpoint_contract_|health_review_|reviewer_side_run_|project_controller_/)'` → exit 0（161 passed）
- `bash scripts/dev/test-affected.sh` → exit 0（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch、4002 passed・11 skipped。repo health verdict pass、
  `size.responsibility-growth`・`deps.cycle` とも pass）
- `python3 scripts/dev/repo-health/repo_health.py --repo . --base $CELERIS_WU_BASE --head worktree --policy-version 1 --out …` → exit 0、new・worsened 0 件（全規則）。
  `git merge-base HEAD main` を基準にすると new 8・worsened 2 件で、すべて optional の `dup.contract`（先行 WU の `crates/task-core/src/parent_checkpoint.rs` を含む組）。この WU の file は含まない。
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- 計画の store・events・LLM の client の check（store・migration・`model/events.rs`・`store/events.rs`・`cos/operations.rs` の差分 0、`strategy_reads.rs` に書き込みの SQL なし、
  `UPDATE projects`・`INSERT INTO roadmap`・LLM の client の語なし）→ exit 0。`run_index_start` を持つ `crates/task-dispatch/src` の file: base 15、HEAD 15。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` と計画の正規表現）→ 範囲外 0 件。
- `sh scripts/dev/progress-index.sh --check` → exit 0

## 測定（`git diff --numstat $CELERIS_WU_BASE HEAD`）

- production の Rust: +542 / -46（純増 +496）。内訳は `review/parent_evaluation.rs` +474（新。doc comment を含む）、`design_gate.rs` +15/-5、`project_controller.rs` +33（移した
  `decide_exhausted_notice`）、`project_wake.rs` +12/-35、`dispatcher.rs` +2/-2（可視性だけ）、`review.rs` +2、`project_decider_ts.rs` +1/-1。
  純増の理由: 起動の条件・試行の記録・決着・入力の節の組み立ては今まで無かった処理で、起動・実行・終わりの記録は共通の起動口を使い写していない。
- 試験: +742（`project_checkpoint.rs` +738、`project_cos.rs` の `mod` の行 +4）。
- 文書: ADR +12/-7、運用文書 +9/-3、進捗。
- dispatcher module（`dispatcher.rs`・`dispatcher/`・`#[path]` の `design_gate.rs`）の本体の行は base より少ない（run 1 の repo health で 32218 → 32200。run 2 の `TreeHold` の呼び出しで約 2 行増えたが、提出時の `size.responsibility-growth` は pass）。
  `Dispatcher` に method を足していない。`dispatcher.rs` の行数は変わらない（可視性の 2 行だけ）。

## 親の目的への寄与

- 確認済み（試験）: 改善 C・D の「途中で独立の Evaluator（既存の reviewer run）が親の目的を判定する」の起動と決着が、Project Controller の `evaluate` の判断から
  共通の起動口を通って実際に起きる。評価の結果は Rust の `check_evaluation` を通って最終の記録になり、LLM の自己申告だけで `checked` にならない（節が無ければ `invalid`）。
  strategy の無い task・shadow・controller の無い案件で reviewer run は増えない（低抽象度の task の回帰）。
- 未確認: 実際の LLM の reviewer が書く `parent_evaluation` の質（live-evaluator-sample の WU）、第二ゲートの shadow ケースでの pivot（second-gate-shadow の WU）、本番の挙動。

## 未解決

- 入力の節の `goal.outcomes`（承認済み Roadmap の Outcome と到達）と子の `criteria` は空で渡している。`facts.goal_reached` は false 固定で、commit の参照の実在は確かめない
  （解決しない参照は支持に数えないので、評価が楽観に倒れる向きの誤りは無い）。D6 の値の読み口（`task_ops::project_controller_view::recorded_outcomes` は snapshot を要る）を決めて入れるのは後の WU。
- 上限で run が無いときの `invalid` の最終の記録は `ArtifactProduced.run_id` が空文字になる。
- 試験の作成中、root の runs の行の `status` に不正な値（`done`）を置くと、その案件の `evaluate` の判断が出なかった。原因は未確認（この WU では変えていない）。

## 提案

- integrate-controller か close-gate で、`goal.outcomes`・子の `criteria`・`goal_reached` を D6 の読み口から入れる WU を置く（今は評価器に親の Outcome の一覧が届かない）。
