---
title: 戦略層 v1 子 5 contract-review — task_core::strategy に識別実験・期待情報価値・停止条件・保留する前提と却下した候補の規則
tasks: [01M7XQ9QQXY1DY56EXE0TT8X6W]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# contract-review（WorkUnit。子 5 の段 impl）

完了日: 2026-11-22。設計の正本は [ADR 2026-11-22-strategy-review-critic](../../adr/2026-11-22-strategy-review-critic.md) の D1・D2.4・D3.7（gate の規則）・D4・D5。
基準はこの WU の `$CELERIS_WU_BASE`。検査はすべて提出時の HEAD で実行した。

## 結論

- `celeris.strategy/1` の候補に `state`・`rejected_by`・`experiment`・`expected_information_value`・`stop_when`、文書に `held_assumptions` を必須で足した（D1）。
  候補の数は 1〜5 から 2〜8（`open` は 3 まで、`selected` の無い版は `open` 2〜3）に変えた。違反は `strategy_invalid`。
- 却下した候補は次の版で消せず・`open` に戻せず・書き換えられず・選べない（D4.1、`strategy_revisit_rejected`）。`validate_document` の検査の表に入れたので Record（chat の案）と Execute の両方に掛かる。
- 実行の入口の純関数 `task_core::strategy::check_execution(doc, &ExecutionFacts)` を `strategy/review.rs` に置いた。`CheckContext` は変えていない。
  `strategy_experiment_phase`（D2.4 の 1〜6）・`strategy_uncritiqued`（D3.7）・`strategy_revisit_rejected`（`candidate`）・`strategy_failure_unread`（D4.2）を全部返す。store は読まない。
  gate（experiment-gate の WU）が `ExecutionFacts` を store から集めて呼ぶ。この WU では gate から呼んでいない。
- 新しい試験 `strategy_review_contract_` は 13 件。既存の `strategy_contract_` 20 件と合わせて 33 件が pass する。

## 実装前 brief

1. 根本原因: 文書に識別実験・却下・停止条件の欄が無く、入口に実験段階・失敗の既読の規則が無い。そのため承認済みの版なら固定の大きな計画も通る。
2. 責務 owner: 文書の規則は `task_core::strategy`（`validate_document` の検査の表）。入口の規則は同じ module の兄弟 `strategy/review.rs` の `check_execution`。
3. 既存の再利用先: `validate_document` の `CHECKS` の表と ablation の形。`Input`（前の版の鎖）と `push`。上限は `TEXT_MAX_CHARS`・`LIST_ITEM_MAX_CHARS`・`is_id`。実行の形は `task_core::ExecutionMode`。
4. 選択肢: (a) 規則を `mod.rs` に足す → 1,385 行の file がさらに膨らむので採らない。(b) `CheckContext` に事実を足す → chat の案の検査（子 3）の呼び出しが変わるので採らない（ADR D5）。
   採用: 型の欄は `mod.rs` の `Candidate`・`StrategyDocument` に、新しい型と規則は `review.rs` に置き、`CHECKS` に 3 行を足した。
5. 新しい状態・分岐・fallback・port: 状態は足していない（段階は `selected` の有無から導く `experiment_phase`）。`Rule` に 4 つの variant を足した。fallback は無い。旧い形の文書を読む分岐も無い（本番に文書 0 件、ADR D1）。
6. 撤去対象: 候補の上限 `CANDIDATES_MAX = 5`（下限 1）を `CANDIDATES_MIN = 2`・`CANDIDATES_MAX = 8` に置き換えた。`check_limits` の文と件数の closure を module の関数 `limit_text`・`limit_count` にして `review.rs` と共有した。
   `check_refute` の鎖の 2 版の組の作り方を `Input::adjacent_versions` にまとめ、`check_revisit` と共有した（同じ仕様の二重の実装を避けた）。
7. 不変条件: 下の「守る不変条件と試験」。

## 守る不変条件と試験（`crates/task-core/src/strategy/tests/review.rs`）

既存の `strategy_contract_*` は子 2 の規則しか見ないので、下の規則の違反を検出できない（子 2 の型は新しい欄を知らず、入口の規則も無い）。

| 試験 | 守る不変条件 |
|---|---|
| `strategy_review_contract_baselines_accepted` | v1（実験段階、open 2）・v2-keeps-refute（v1 の実験を読んで選んだ版）・v3 が通る。候補 8 件（open 2・rejected 6）は上限の内 |
| `…_missing_fields_rejected` | `held_assumptions` の欠けは `Required`、候補の `state`・`experiment`・`expected_information_value`・`stop_when` の欠けは `strategy_invalid` |
| `…_field_limits_rejected` | 17 種の上限（文の長さ・空、`stop_when` 1〜8、`discriminates` 1〜8、`held_assumptions` 0〜16、候補 2〜8、open 3 まで、selected の無い版の open 2〜3、`rejected_by` 1〜16）がちょうど `Limit` |
| `…_shape_rules_rejected` | open の `rejected_by`、support・無い証拠を指す `rejected_by`、予算が `cost` を超える・壁時計 0、無い仮説・候補の仮説を含まない `discriminates`、`held_assumptions` の id の重複・形がちょうど `Shape` |
| `…_rejected_removed_next_version_is_revisit` | v3 が v2 の却下した候補を消す・open に戻す・書き換えると、ちょうど `strategy_revisit_rejected` |
| `…_selected_rejected_is_revisit` | 却下した候補を `selected` にした版（同じ版・次の版）は `strategy_revisit_rejected` |
| `…_rejected_candidate_task_is_revisit` | 却下した候補の task（atomic・compound）は `strategy_revisit_rejected` |
| `…_execution_compound_in_experiment_phase` | 実験段階の版で compound・execution 省略は `strategy_experiment_phase` |
| `…_experiments_up_to_open_candidates` | 実験段階の版の実験 task は open の候補ごとに 1 件（2 件目・candidate 無し・文書に無い候補・予算超えの `max_wall_secs` は拒否） |
| `…_selected_needs_terminal_experiments` | 選んだ版は前の版の実験が 1 件以上あり全部終端のときだけ通る（実験無し・未終端・自分の版の task だけ・v1 で先に選んだ版は拒否） |
| `…_uncritiqued_source_not_done_or_not_subject` | 文書の task が done でない・Critic の対象でない版は `strategy_uncritiqued` |
| `…_failure_unread_terminal_experiment` | 前の版の終端の実験・本実装の task を証拠が参照しない版は `strategy_failure_unread`。参照した v3 は通る |
| `…_ablation` | 基準 B2（v2-keeps-refute・compound・v1 の実験 D・H が終端で参照済み）から 1 か所だけ変えた N-phase・N-crit・N-revisit・N-unread が、ちょうどその code を返す |

ablation の出力（提出時の HEAD、`--no-capture` 相当の stdout）:

| 負の状態 | B2 から変えた 1 か所 | 返した code |
|---|---|---|
| N-phase | v1 の実験 H が終端でない | `{"strategy_experiment_phase"}` |
| N-crit | 参照した版が Critic の対象にない | `{"strategy_uncritiqued"}` |
| N-revisit | 却下した continue-pilots の task | `{"strategy_revisit_rejected"}` |
| N-unread | v1 の終端の実験 Z を v2 が参照しない | `{"strategy_failure_unread"}` |

## 変更前に新しい試験が落ちた出力

試験と fixture を先に commit した（件名「strategy-v1 5 contract-review: strategy_review_contract_ の試験と fixture を先に足す（規則は未実装）」）。
この commit は新しい欄の serde の型・`Rule` の 4 つの variant・空の `Vec` を返す `check_execution` だけを含み、規則（検査の関数と候補の数の変更）は含まない。

`cargo nextest run -p task-core --no-fail-fast -E 'test(/strategy_review_contract_|strategy_contract_/)'`（exit 100）:

```
Summary [   0.063s] 33 tests run: 21 passed, 12 failed, 1399 skipped
FAIL strategy_review_contract_experiments_up_to_open_candidates   left: {} right: {"strategy_experiment_phase"}  (cut-min-edges)
FAIL strategy_review_contract_uncritiqued_source_not_done_or_not_subject  left: {} right: {"strategy_uncritiqued"}
FAIL strategy_review_contract_failure_unread_terminal_experiment  left: {} right: {"strategy_failure_unread"}
FAIL strategy_review_contract_execution_compound_in_experiment_phase  left: {} right: {"strategy_experiment_phase"}  (Some(Compound))
FAIL strategy_review_contract_selected_needs_terminal_experiments  left: {} right: {"strategy_experiment_phase"}  (no experiments)
FAIL strategy_review_contract_field_limits_rejected  left: {} right: {Limit}  (experiment.content: [])
FAIL strategy_review_contract_ablation  left: {} right: {"strategy_experiment_phase"}  (N-phase)
FAIL strategy_review_contract_rejected_candidate_task_is_revisit  left: {} right: {"strategy_revisit_rejected"}  (Atomic)
FAIL strategy_review_contract_selected_rejected_is_revisit  left: {} right: {"strategy_revisit_rejected"}
FAIL strategy_review_contract_rejected_removed_next_version_is_revisit  left: {} right: {RevisitRejected}  (removed: [])
FAIL strategy_review_contract_shape_rules_rejected  left: {} right: {Shape}  (open with rejected_by: [])
FAIL strategy_review_contract_baselines_accepted  [Violation { rule: Limit, path: "candidates", detail: "8 件で範囲 1〜5 の外" }]
```

21 件の pass は既存の `strategy_contract_` 20 件（新しい fixture で通る）と `strategy_review_contract_missing_fields_rejected` である。
後者は欄の欠けを serde の型（この commit に入れた）が拒否するので、規則の実装の前から通る。base の型では新しい欄を持つ fixture 自体が `deny_unknown_fields` で読めない。

## 提出時の HEAD の検査

| 条件 | コマンド | 結果 |
|---|---|---|
| 0: `strategy_review_contract_` が 6 件以上 | `cargo nextest list -p task-core \| grep -cE 'strategy_review_contract_[a-z0-9_]*$'` | 13 |
| 0: 既存と共に全部 pass | `cargo nextest run -p task-core -E 'test(/strategy_review_contract_\|strategy_contract_/)'` | exit 0、33 tests run: 33 passed |
| 2: test-affected | `bash scripts/dev/test-affected.sh` | exit 0。Rust 8655 tests run: 8655 passed（31 skipped）、web の unit 745 passed（105 file）、`repo_health_gate: all checks passed`、test-ts-verify 20 passed |
| 2: clippy | `cargo clippy --workspace -- -D warnings` | exit 0（vendor の `deno_core` の既存の deprecated の警告 1 件だけ） |
| 2: fmt | `cargo fmt --all -- --check` | exit 0 |
| 範囲 | plan の範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`） | 範囲外の path 0 件 |

## 他の crate・prompt・API schema の合わせ方

- fixture: `baseline-v1.json`・`neg-*.json` は D1 の欄を足し、候補 2 件を open・selected 無し（実験段階）にした。`v2-keeps-refute.json` は v1 の実験の結果を読んで
  continue-pilots を `rejected_by: ["e-pilot-refute"]` で却下し、cut-min-edges を選んだ版にした（`change_reason` もそう書いた）。`v2-drops-refute.json` は候補を v1 のまま残す（反証の削除だけが違反）。
  `supersedes.sha256` は新しい v1 の sha256。task-api の `strategy_gate` の試験と task-dispatch の `discussion_files` の試験は `include_str!` でこの fixture を読み、変えずに通る（test-affected で確かめた）。
- 他の crate の試験に、fixture を使わずに書いた strategy の文書は無い（`grep -rln 'reevaluate_when\|decision_method' crates web/src` が `crates/task-core/src/strategy/` の外に 0 件）。
- CoS chat の prompt: `crates/task-worker/src/cos_chat.rs` の propose の案の説明（`discussion_files_guide`）に D1 の欄と候補の数・却下の規則を 1 項足した。
- API schema: `StrategyDocument` は `docs/api/v1/`・`web/api/generated/` に載っていない（同じ grep で 0 件）ので、生成物は変わらない。`StrategyRef.candidate` は experiment-gate の WU が足す。

## LoC（`git diff --numstat "$CELERIS_WU_BASE" HEAD`）

| 区分 | file | 追加 | 削除 | 差 |
|---|---|---|---|---|
| production Rust | `strategy/mod.rs`（1,385 → 1,439 行） | 92 | 38 | +54 |
| production Rust | `strategy/review.rs`（新） | 440 | 0 | +440 |
| production Rust | `task-worker/src/cos_chat.rs`（prompt の文） | 5 | 1 | +4 |
| test Rust | `strategy/tests/review.rs`（新）・`strategy/tests.rs` | 647 | 0 | +647 |
| test の fixture（JSON） | `strategy/fixtures/*.json` 6 件 | 246 | 15 | +231 |

production は +498 行の純増。理由: ADR が求める新しい欄 6 つと規則（D1 の形と上限、D4.1、`check_execution` の 4 つの code）を足す WU で、消せる旧い経路は候補の上限 1〜5 だけだったため。
新しい Manager・trait・状態機械・DB 表・event の種類・migration は足していない（`Rule` の variant 4 つ、型 6 つ、純関数）。

## 親の目的への寄与（確認済み・推定・未確認）

- 確認済み（試験）: 改善 A の「最初から巨大な固定計画を走らせない」「却下した戦略に戻らない」「失敗の記録を読む」を、文書と入口の純関数の規則として決定的に判定できるようになった（13 件の試験と ablation）。
- 未確認: 実際の `/cos/operations` の `task.create` での拒否は experiment-gate の WU が `check_execution` を gate から呼ぶまで効かない。Critic の判定は critic-review の WU。shadow ケースは shadow-case の WU。
- 推定: v1 を実験段階に固定したので、後続の gate の試験（子 2 の `strategy_gate_` の基準）は ADR D7.4 どおり `execution: atomic` と `candidate` を足す必要がある。

## 未解決・提案

- experiment-gate の WU への申し送り: `ExecutionFacts.experiments` は鎖の全版（v1〜v）の X を `version` 付きで渡す。`check_execution` は `version == doc.version` を X(v)、`version < doc.version` を前の版の実験として扱う。
  gate は `validate_document` で文書を検査した後に `serde_json::from_slice::<StrategyDocument>` で読んで `check_execution` を呼ぶ（`validate_document` は文書を返さない）。
- 名前を変えて却下した approach を出し直すことは決定的に見分けない（ADR の未確認のまま。Critic の C5 が受け持つ）。
