---
title: 戦略層 v1 子 6 ts-checkpoint-rule — control-ts の規則 pc/checkpoint と評価の結果の写し
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: ts-checkpoint-rule
status: done
updated: 2026-11-23
completed: 2026-11-23
---
# ts-checkpoint-rule（WorkUnit）

完了日: 2026-11-23。
`control-ts/src/domains/project_controller.ts` に、snapshot の `checkpoints` を読む規則 `pc/checkpoint/evaluate` と `pc/checkpoint/act` を足した。
設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md) の D5.1〜D5.3・D6。
基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

Rust 側の 2 つの穴（写していない評価がある間、guard の提案を Rust が落とす／continue の写しが写し済みにならない）を見つけた。どちらもこの WU の write set の外にあるので、「未解決事項」と「提案」に書いた。

## 実装前 brief

1. 根本原因: Rust（controller-checkpoint）は snapshot に `checkpoints` を出し、`evaluate` と `pc/checkpoint/act` を検証する。しかし base の TS は `checkpoints` を未知の欄として throw し、evaluate も act も返さなかった。
2. 責務 owner: 方針は `control-ts/src/domains/project_controller.ts` の `decide`。表（allowed）と検証は Rust の `task_core::parent_checkpoint::allowed_actions`・`project_proposal::validate` が持つ。
3. 既存の再利用先: `decide` の `out`・`replan`（CoS 入力の日の上限で待つ）、no-progress の streak、規則 7 の in-flight の root の集合。行動は Rust が snapshot に入れた `allowed` から選び、TS で表を作り直さない。
4. 選択肢: act の置き場を (a) ADR D5.1 の字句どおり evaluate の直後（既存の待ちの規則より後）にする案と、(b) paused・no-progress の直後にする案を比べた。(a) では、写していない評価がある間に milestone の記録や起票が先に出る。Rust はそれを `checkpoint_action_mismatch` で毎回落とし、act に届かないまま止まる。そこで (b) を採った。evaluate は D5.1 どおり人の待ちの規則の後、root の見守り（規則 7）の前に置いた。
5. 新しい状態・分岐・fallback・port: 新しい状態は無し（Rust が導いた値を読むだけ）。分岐は規則 1c（act）と 6b（evaluate）。`allowed` がどの行にも当たらないときは fallback を置かず、入力の誤りとして throw する（Rust は空の `allowed` を作らない）。
6. 撤去対象: 無し（旧い経路は無い）。
7. 不変条件: 下の試験に書いた (1)〜(4)。strategy root の無い snapshot の判断は base と同じ（golden 15/15、既存の 21 件）。

## 規則の順（`decide`）

1 paused（`pc/project-not-active`）→ 1b no-progress（`pc/no-progress`。streak に `evaluate` を足した。Rust の `runaway_guard` と同じ D6）→ **1c `pc/checkpoint/act`**（replan の行は CoS 入力の日の上限で `pc/checkpoint/act/daily-limit` の wait）→ 2 outcomes-invalid → 3 no-outcomes → 4 milestone → 5 achieved → 6 human-wait → **6b `pc/checkpoint/evaluate`** → 7 root-in-flight → …

- act の行動: `allowed` に replan があればそれを返す（reason_code は表の値。context_refs は `task:<root>`・記録・戦略の参照だけ。評価の文は写さない。I11）。次に escalate を見る（`question_key` は表の `checkpoint:<判断 id>`、category `explicit_human`）。最後に continue を見る（規則 7 と同じ in-flight の root を見守る）。signal は `checkpoint` = 判断 id。
- evaluate: 実行中の評価が無く、due の trigger がある root のうち、先頭の trigger の `at`（20 桁の event id）が最も小さい root を 1 つ選ぶ。`at` が null の root は後に回す。trigger は Rust の上限の 8 件まで入れる。

## 試験（`control-ts/src/domains/project_controller.test.ts`、名前は `project_controller_checkpoint_`）

実装前の実行: `npx vitest run src/domains/project_controller.test.ts` は 9 件 fail、21 件 pass（新しい 9 件がすべて落ちた。`checkpoints` を未知の欄として throw した、evaluate・act を返さなかった）。

| 試験 | 守る不変条件 |
|---|---|
| `..._parse_accepts_the_rust_section` | Rust の `checkpoints` を読み、配列でなければ落とす。key が無ければ base と同じ判断 |
| `..._evaluates_the_oldest_due_root_before_watching` | 実行中の root を選ばない。最も古い due の root を 1 つ選ぶ。root が走っていても規則 7 より先 |
| `..._caps_triggers_at_the_rust_limit` | trigger は 8 件まで |
| `..._pivot_replans_with_references_only` | pivot・識別実験 → 表の reason_code の replan。refs は 3 種だけで、評価の文を写さない |
| `..._ask_human_and_invalid_escalate_once_per_checkpoint` | ask_human・invalid → `checkpoint:<id>` の escalate |
| `..._on_track_continue_watches_the_roots` | continue の行 → continue |
| `..._act_precedes_milestones_and_launches` | 写していない評価があれば milestone より act が先。写した後は既存の規則に戻る |
| `..._comes_after_paused_no_progress_and_the_daily_cos_limit` | 受け入れ条件 1: paused・no-progress（evaluate を数える）・CoS 入力の日の上限が checkpoint より先 |
| `..._evaluate_waits_for_open_human_decisions` | 人の待ちの間は evaluate しない。due も写していない評価も無ければ既存の判断 |

既存の試験でこれらを検出できなかった理由: base の TS の試験は `checkpoints` を持つ snapshot を 1 件も作らない。

## 検査（提出時の HEAD で実行）

- `cd control-ts && pnpm run typecheck`: exit 0。
- `cd control-ts && pnpm test`: exit 0。vitest 143 件 pass（6 file）、node --test 47 件 pass。
- `cd control-ts && node scripts/compare.mjs project_controller`: 15/15 matched。Rust の dump（invariants.jsonl）は変えていない（strategy root の無い snapshot の形は同じ）。
- `cargo nextest run -p task-core -p task-ops -p task-dispatch -p celeris -p celeris-script -E 'test(/project_controller|checkpoint|script_fixtures|script_host_runner/)'`: exit 0。141 件 pass（Rust の `project_controller_checkpoint_` 3 件を含む）。
- `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris -E 'binary(project_controller_e2e) | binary(project_cos_shadow) | binary(project_controller_replay)'`: exit 0。19 件 pass（この変更の TS から作った本物の bundle を V8 で動かす。bundle が無ければ落ちる設定）。
- TS の出力と Rust の検証の突き合わせ（commit しない一時の example。golden の snapshot に `checkpoints` を足した 12 例を TS の `decide` に通し、`parse_and_validate` に掛けた）: evaluate・act の 7 例はすべて ok。guard の 5 例の結果は次のとおり。
  - `checkpoints` に due だけがある paused・no-progress は ok（wait・escalate）。
  - 写していない評価がある paused・no-progress・日の上限の 3 例は `invalid ["checkpoint_action_mismatch"]`。下の「未解決事項」1。
- `bash scripts/dev/test-affected.sh`: exit 0（ts-verify。cargo 0 回。golden は lane_rules 19683・route_priority 2026・execution_gate 2337・project_controller 15・cos_digest 22 と 2114 がすべて matched）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の出力は 2 file（`project_controller.ts`・`project_controller.test.ts`）。commit 後はこの進捗も入る。

## 行数（`git diff --numstat $CELERIS_WU_BASE`）

- production（TS）: `project_controller.ts` +137 −10（純増 127）。型 4 つ（Rust の `SnapshotCheckpoint`・`SnapshotEvaluation` と同じ形）と、規則 2 つ。純増の理由は、新しい行動 `evaluate` と、写していない評価の act の規則を足したこと。
- test: `project_controller.test.ts` +257。
- Rust・store・migration・events の差分 0。

## 親の目的への寄与

- 確認済み: Rust が snapshot に出す checkpoint の信号から、TS が evaluate を選び、評価の結果を Rust の表どおりの行動（continue・replan の `checkpoint_experiment`／`checkpoint_pivot`・escalate）に写す。Rust の検証も通る（上の突き合わせ）。改善 C（途中で独立評価して pivot・再計画する）の controller の判断の段がつながった。
- 未確認: 評価の reviewer run の起動・決着（controller-checkpoint の残り）を含む端から端までの shadow ケース（second-gate-shadow の WU）。

## 未解決事項

1. 写していない評価がある間、Rust の `validate_checkpoint_act` が guard の提案を `checkpoint_action_mismatch` で落とす。落ちるのは paused の wait、`pc/no-progress` の escalate、CoS 入力の日の上限の wait で、上の突き合わせで再現した。結果は `invalid_output`（判断は none・phase waiting）になる。paused と日の上限では実質は待ちと同じだが、判断の記録は invalid になる。no-progress では人への escalate が出ず、guard が人に知らせないまま止まる。TS 側では直せない（表に無い行動は全部落ちる）。owner は `crates/task-core/src/project_proposal.rs`（controller-checkpoint の write set）。
2. continue の行（`on_track`・`at_risk` で `continue`）を continue・wait で写した判断は効果が無く、`effect_state` が `none` になる。D5.3 と `task_ops::project_snapshot::checkpoints` の「写し済み」は `pending`・`applied`・`suppressed` だけを数えるので、その評価はいつまでも写していない評価のまま残る。すると Rust は act 以外の行動をすべて落とし、TS は act の continue を出し続ける。root が終わっても次の Outcome の起票・milestone の記録に進めない。コードを読んで導いた結論で、試験での再現はしていない（未確認）。owner は `crates/task-ops/src/project_snapshot.rs` と ADR D5.3。

## 提案

- 未解決事項 2: 「写し済み」を、rule `pc/checkpoint/act`・signal `checkpoint` = 判断 id の判断のうち `effect_state` が `failed` でないもの（`none` を含む）に変える。変える場所は ADR D5.3 と `project_snapshot::checkpoints` の `mapped`。
- 未解決事項 1: `validate_checkpoint_act` で、rule が act でない `wait` と `escalate` は通す。どちらも計画を進めない（起票・CoS 入力・到達の記録を書かない）ので、D5.2 の不変条件「off_track・unknown から continue は出ない」は保たれる。写していない評価は残り、guard が外れた後の判断で act が写す。Rust に TS の rule id を書き込まずに済む。
- ADR D5.1 の付記: act（2）は paused・no-progress の直後に置く（Rust が act 以外を落とすため）。evaluate（1）は D5.1 の位置のままにする。この順を ADR に書く。
- 上の 3 件は integrate-controller か second-gate-shadow の再計画で、controller-checkpoint の write set の WU に回す。
