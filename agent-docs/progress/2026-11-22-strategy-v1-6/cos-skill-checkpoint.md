---
title: 戦略層 v1 子 6 cos-skill-checkpoint — CoS の skill に checkpoint の replan（checkpoint_experiment・checkpoint_pivot）を受けたときの手順を足す
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: cos-skill-checkpoint
status: done
updated: 2026-11-23
---
# cos-skill-checkpoint（WorkUnit）

完了日: 2026-11-23。基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。
設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md) の D5・D7・D11（この WU の write set は D11 の表の行）。

## 変更

- `config/skills/cos-operator/strategy-review.md`: 節「案件の controller から checkpoint の replan を受けたとき」と小節「負の研究結果の書き方」を足し、手順 (6) からその節を指した。
  - 入力の見分け方（`理由:` が `checkpoint_experiment`・`checkpoint_pivot`、規則 `pc/checkpoint/act`、参照は id だけ）。
  - 評価の記録 `parent-evaluation/<判断 id>.json` を `GET /api/v1/tasks/<root>/artifacts`・`/artifacts/<idx>` で読み、`effective` と `violations` だけを判断に使う。根拠は R2 の supported の項目だけ。
  - 記録を `artifact` の参照で証拠に引いた新しい版（`checkpoint_experiment` は `selected` 無しの識別実験の版、`checkpoint_pivot` は反証で候補を `rejected_by` 付きで却下した版）。候補・識別実験・却下・反証の保存は子 5 の手順 (4) のまま。`idempotency_key` は `checkpoint-<判断 id>-<strategy_id>-v<n>`。
  - controller が `enabled` の案件では、`parent` の無い `task.create` が `project_controller_owns_launch`（`task_api::cos::goal_guard::check`）で断られるので、文書と実験の task を strategy root の子で起票する。
  - pivot で `selected` を書いた版の本実装は、走っている strategy root が前の版の実験 task の集合 X に入るので、root が終端になるまで `strategy_experiment_phase` で断られる（子 5 D2.3・D2.4 の 6）。
  - 人の決定にするもの（大きな戦略の変更・root の停止・却下した候補に戻す・新しい系列・高額な実験・外部公開）、変えないもの（Project.request・承認済み Roadmap・Outcome の acceptance・root の objective と acceptance・到達の記録）、no-progress guard で止まったときの扱い。
  - 負の研究結果: `effective.negative_result` が真の記録を「実行は成功・知見は前進・当初の目標は未達」と書く。反証を残し、3 軸を分けて報告し、root の status と目標は変えない。
- `config/skills/cos-operator/SKILL.md`: `metadata.version` を "8" → "9"。表の strategy-review.md の行に checkpoint の replan の場面を足した。
- `config/skills/cos-operator/SOURCE.md`: version 9 の履歴を足した。
- 本番 KB への取り込みは人が行う（`celerisctl skills import config/skills --name cos-operator --root <kb_root>`）。この WU は本番を変えていない。

## commit（件名）

1. `strategy-v1 6 cos-skill-checkpoint: strategy-review.md に checkpoint の replan を受けたときの手順を足し、cos-operator の version を 9 に上げる`

## 実装前 brief

1. 根本原因: controller の checkpoint が `checkpoint_experiment`・`checkpoint_pivot` の replan を案件 CoS に渡しても、CoS の skill に評価の記録の読み方と新しい版の作り方が無い。
2. 責務 owner: 新しい版を作るのは CoS（ADR D5.2・実装前 brief の 2「新しい版は CoS（skill）」）。手順の置き場は `config/skills/cos-operator/strategy-review.md`。
3. 既存の再利用先: 子 5 の手順 (4)（次の版）・(3)（識別実験）・拒否の code の表、`project-controller.md` の子 task の op、`GET /api/v1/tasks/{id}/artifacts`。
4. 選択肢: (a) project-controller.md に書く → D11 の write set の外で、版の規則が 2 か所に分かれるので採らない。(b) strategy-review.md に節を足し、手順 (4) を参照する → 採用。
5. 新しい状態・分岐・fallback・port: 無し（文書だけ）。
6. 撤去対象: 無し（旧い手順は無い。手順 (6) の文を新しい節に向けた）。
7. 不変条件: operations.md の登録表は変えない（`cos_operator_skill_table_matches_allowed`）。skill の入口の表が参照 file を指す（`cos_operator_skill_documents_the_registered_project_deploy_scope`）。policy_version の gate は triage skill の版だけを見るので、cos-operator の版を上げても `[cos.triage] policy_version` は変わらない（`task_ops::cos_skill_gate`）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- 受け入れ 0: `grep -c "checkpoint_experiment\|checkpoint_pivot" config/skills/cos-operator/strategy-review.md` → 下の「検査」の値。
- 受け入れ 1: `git show "$CELERIS_WU_BASE":config/skills/cos-operator/SKILL.md | grep version` → `"8"`、HEAD → `"9"`。
- 受け入れ 2: `cargo nextest run -p celerisctl` → exit 0（199 passed）。`cargo nextest run -p task-api -p task-worker -E 'test(/cos_operator_skill_/)'` → exit 0（2 passed）。
  関連: `cargo nextest run -p celeris -p task-ops -E 'test(/skill/)'` → exit 0（20 passed。skill の gate と config の試験）。
- 検査: 下の「検査」節。

## 検査

（commit の後に記入）

## LoC

- production の Rust・TS: 0 行（変更なし）。test: 0 行。
- skill の Markdown: strategy-review.md +48 行（節の追加）、SKILL.md ±1 行、SOURCE.md +1 行。純増の理由: checkpoint の replan を受ける CoS の手順は新しい場面で、置き換える旧い手順が無い。

## 親の目的への寄与（確認済み・推定・未確認）

- 確認済み: 改善 C（途中評価で pivot・再計画）の CoS 側の手順が skill にある。評価の記録の `effective` だけを判断に使い、参照の無い主張を根拠にしない手順（改善 D と「自己申告で達成にしない」）、負の研究結果の 3 軸の書き方（改善 B の反証の保存）がある。
- 推定: CoS がこの手順に従えば、pivot・識別実験の版は子 5 の gate（人の承認・Critic・却下の保存）を通る。
- 未確認: 実際の CoS run がこの節を読んで版を作るかは試していない（second-gate-shadow はスクリプトの評価で写しの経路を確かめる計画。live の CoS run は無い）。

## 未解決

- 文書の task と識別実験の task を strategy root の子で起票すると、root の subtree に入る。そのため (1) 子の終端の数の trigger（`children:<k>`）に数えられ、(2) CoS の新しい版の文書が root の subtree の文書として反証の trigger（D1.2）を発火させうる。ADR D1.2 は「新しい版を別の task が作ったときは発火しない」前提で書かれている。追加の評価は trigger の数と no-progress guard で有界だが、実測は無い。
- 子の task に `strategy` を付けた `task.create` が子 5 の gate を通ることは、gate が `parent` を見ないことをソースで確かめただけで、試験は無い（second-gate-shadow の `parent_checkpoint_shadow_d_done_pivot_new_version` で確かめる計画）。
- 本番 KB への skill の取り込みは人が行う。

## 提案

- second-gate-shadow で、pivot の新しい版の文書 task を strategy root の子として起票し、子 5 の gate の 422 と通過、および追加の checkpoint の発火の有無を確かめる。
- 上の (1)(2) が評価を増やしすぎるなら、ADR D1.2 の子の数え方から「CoS が `strategy` を付けて起票した子」と戦略の文書の task を除くかを、close-gate で人に聞く。
