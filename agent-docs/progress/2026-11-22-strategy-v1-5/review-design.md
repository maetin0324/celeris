---
title: 戦略層 v1 子 5 review-design — Strategy Review・Critic・実験段階・却下への逆戻りの禁止の ADR
tasks: [01M7XQ9QQXY1DY56EXE0TT8X6W]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# review-design（WorkUnit。子 5 の段 design）

完了日: 2026-11-22。設計の記録は [ADR 2026-11-22-strategy-review-critic](../../adr/2026-11-22-strategy-review-critic.md)。
source（`crates/`・`web/`・`config/`）は変えていない。

## 結論

- 文書 `celeris.strategy/1` の候補に `state`（open・rejected）・`rejected_by`（反証の証拠 id）・`experiment`（内容・識別する仮説・予算）・
  `expected_information_value`・`stop_when` を、文書に `held_assumptions` を必須で足す。候補は全 2〜8 件・open 3 件まで・selected の無い版は open 2〜3 件。
  本番に文書は 0 件（2026-11-22T09:00:56Z、本番 DB を `mode=ro` で数えた）なので互換の分岐は作らない。
- 実験段階（selected の無い版）を参照する `task.create` は、`execution: atomic` で open の候補ごとに 1 件だけ通す（`StrategyRef.candidate`）。それ以外は
  422 `strategy_experiment_phase`。selected のある版は、前の版の実験 task がすべて終端のときだけ通る。
- Critic: 戦略の成果物を作った task の final review に暗黙の条件 `strategy_critic` を足し、既存の reviewer run 1 回が `ReviewRequest.strategy_critic`
  （対象・実験の記録・固定の基準 C1〜C5）を見て `ReviewOutput.critic` を書く。dispatcher は形だけ確かめる。成果物の無い task は prompt・`request.json`・
  reviewer run の数が base と同じ。`done` でない task の文書は 422 `strategy_uncritiqued`。順は人の承認 → Critic（既存の待ちの順のまま）。
- 再評価: 却下した候補は次の版で消せず選べない（422 `strategy_revisit_rejected`）。前の版の終端の実験 task を証拠が参照しない版は 422 `strategy_failure_unread`。
- 試験の prefix は `strategy_review_contract_`・`strategy_critic_`・`strategy_gate_review_`・`strategy_review_shadow_`・`strategy_review_regression_`。
  shadow は子 0 の E-mid の slice だけを fixture に写し、manifest の sha256 と cutoff を確かめ、answer key を読まない。
- 新しい DB 表・migration・event の種類・Agent サービス・Manager・trait・状態機械は足さない。

## 読んだ source と記録（base はこの WU の `$CELERIS_WU_BASE`）

- 子 2 の ADR 2026-11-21-strategy-evidence-contract、子 3 の ADR 2026-11-21-chat-discussion-mode（全文）。
- `crates/task-core/src/strategy/mod.rs`（`CheckContext`・`Rule`・`CANDIDATES_MAX = 5`・`TEXT_MAX_CHARS`・`LIST_ITEM_MAX_CHARS`）、`crates/task-api/src/cos/strategy_gate.rs`
  （`StrategyRef` は `deny_unknown_fields`、`check` の入口）、`crates/task-dispatch/src/review.rs`（`ReviewExtras`・暗黙の条件・reviewer の起動・`artifact_candidates`）、
  `crates/task-dispatch/src/dispatcher/review_spawn.rs`（`needs_reviewer_run || delivery_requires_reviewer_run`）、`crates/task-dispatch/src/health_review/verdict.rs`
  （`diff_design` の判定の前例）、`crates/task-worker/src/protocol.rs`（`ReviewRequest` は `Default` を derive、`ReviewOutput`）、
  `crates/task-dispatch/src/health_review/design_gate.rs`（`..Default::default()` で作るので触らない）、`crates/task-core/src/model.rs`（`Status`）、
  `crates/task-api/src/handlers/tasks.rs`（`NewTaskBody`）、`crates/task-worker/src/cos_chat.rs`（propose の案の説明）、`config/skills/cos-operator/`。
- 子 0 の `replay/replay.md`（E-mid の cutoff 2026-11-18T20:36:35Z、slice の中身、answer key は slice に入らない）。
- 子 4（task 01M7XQ9QK1KC34Q2A6PJ960TJB）の branch に子 3 の統合より後の commit は無い（`git log` で確かめた）。

## 本番の読み取り（書き込みなし）

`sqlite3 -readonly "file:/local/celeris/data/db/celeris.sqlite3?mode=ro"`（2026-11-22T09:00:56Z）:

| 照会 | 件数 |
|---|---|
| `artifact_produced` の `artifact.name LIKE 'strategy/%'` | 0（1.84 秒） |
| `cos_operations` の `payload_json.strategy` が null でない | 0 |
| `decisions` の `key LIKE 'strategy-approve-%'` | 0 |
| `chat_attachments` の `relative_path`・`original_name` に `strategy` | 0 |
| events の json が `celeris.strategy/1` を含む | 105（`worker_progress` 90・`execution_planned` 6・`worker_finished` 4・ほか 5。文書ではない） |

## 受け入れ条件の確かめ方（提出時の HEAD で実行）

- 条件 0（ADR に (1)〜(7)・4 つの code・5 つの prefix・不変条件・足さないもの）: `grep -c` で code と prefix を数えた（下の summary に結果）。
- 条件 1（子 2・子 3 の ADR のどの決定を使い・変えるか）: ADR の節「子 2・子 3 の ADR から使うもの・変えるもの」の表。
- 条件 2（`crates/`・`web/`・`config/` に差分が無い）: `git diff --stat "$CELERIS_WU_BASE" HEAD -- crates web config` が空。

## LoC

production・test の source の差分は 0 行（文書だけ）。

## 未解決・提案

- strategy の無い `task.create` は止めない（通常の task の経路を変えない条件のため）。CoS が strategy を付けずに大きな計画を起票する穴は、監査と子 6 の Evaluator で見つける。
  閉じるなら案件の状態で起票を止める設計を人が決める（ADR の「未確認と残る穴」）。
- 子 4 が `celeris.strategy/1` の文書を作る試験を足すと、D1 の欄が要る。先に統合した側に後の側が合わせる。
