---
title: 戦略層 v1 子 5 cos-skill — CoS の skill に Strategy Review の手順・strategy の欄・拒否の code を書く
tasks: [01M7XQ9QQXY1DY56EXE0TT8X6W]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# cos-skill（WorkUnit。子 5 の段 gate）

完了日: 2026-11-22。設計の正本は [ADR 2026-11-22-strategy-review-critic](../../adr/2026-11-22-strategy-review-critic.md) の D6。
基準はこの WU の `$CELERIS_WU_BASE`。検査はすべて提出時の HEAD で実行した。

## 結論

- `config/skills/cos-operator/strategy-review.md`（新、93 行）に ADR D6 の CoS の手順を書いた: いつ使うか（抽象的な依頼の開始時・人が戦略の比較を求めたときだけ。
  明確なバグ修正・人が手順を指定した変更・低抽象度の task は `strategy` の無い `task.create` のまま。迷えば人に聞く）、手順 (1)〜(6)、A〜H 型の計画を最初に起票しないこと、
  `task.create` の欄 `strategy {task_id, name, sha256, candidate}` と実験 task の例、子 2 の 7 つと `strategy_not_applicable`・子 5 の 4 つの 422 の code ごとの次の手、
  CoS・operator・mcp の回答を人の承認に数えないこと、新しい `strategy_id` を立てて却下を避けないこと。
- `SKILL.md` の場面の表に 1 行（`[strategy-review.md](strategy-review.md)`）を足し、`metadata.version` を 7 → 8 に上げた。`SOURCE.md` の Notes に付記を 1 行足した。
- `operations.md` の登録表・`crates/`・`web/` は変えていない。chat の propose の案の説明（ADR D6 の 2 点目、`cos_chat.rs`）は contract-review が済ませた。

## 実装前 brief

1. 根本原因: CoS の skill に Strategy Review の手順が無く、CoS は抽象的な依頼でも大きな計画を `strategy` 無しで起票できる（gate は `strategy` の無い起票を止めない。ADR「未確認と残る穴」）。
2. 責務 owner: CoS の手順は `config/skills/cos-operator/`（ADR D6・D8 の write set）。
3. 既存の再利用先: SKILL.md の場面の表と参照 file の分け方（version 3 以降）、operations.md の「共通の形」「起票」、子 2 ADR D2.2・D4 の code の定義。
4. 選択肢: (a) operations.md の起票節に書き足す → 登録表を読む試験の file を膨らませ、通常の起票の説明に戦略の手順が混ざるので採らない。
   (b) 新しい参照 file に分け、SKILL.md の表から辿らせる → 採用（ADR D6 の指定、通常の起票では読まれない）。
5. 新しい状態・分岐・fallback・port: 無し（文書だけ）。
6. 撤去対象: 無し（手順を足すだけ。置き換える旧い手順は無い）。
7. 不変条件: operations.md の登録表は `ALLOWED` と一致したまま（`cos_operator_skill_table_matches_allowed`）、celerisctl の operations.md の解析も変わらない、
   SKILL.md の sibling link はすべて実在する（daemon の skill gate `task_ops::cos_skill_gate::check` の規則）。

## 受け入れ条件ごとの証拠（提出時の HEAD で実行）

| 条件 | コマンド | 結果 |
|---|---|---|
| 0. strategy-review.md に手順・欄・code、SKILL.md から link | `grep -n strategy-review config/skills/cos-operator/SKILL.md`、SKILL.md の sibling link を gate と同じ規則で抜いて存在を確かめる python | 表の 1 行あり。link 8 件、missing 0 |
| 1. version が上がり SOURCE.md に付記 | `grep -n 'version' config/skills/cos-operator/SKILL.md`、`git diff "$CELERIS_WU_BASE" HEAD -- config/skills/cos-operator/SOURCE.md` | `version: "8"`。付記 1 行 |
| 2. skill の file を読む既存の試験 | `cargo nextest run -p celerisctl` | exit 0、199 passed |
| 2. 同上 | `cargo nextest run -p task-api -p task-worker -E 'test(/cos_operator_skill_/)'` | exit 0、2 passed（`cos_operator_skill_table_matches_allowed`・`cos_operator_skill_documents_the_registered_project_deploy_scope`） |
| 2. 同上（skill gate） | `cargo nextest run -p task-ops -p task-dispatch -E 'test(/cos_skill_gate\|skill_gate/)'` | exit 0、3 passed |
| 3. crates/・web/ を変えていない | `git diff --name-only "$CELERIS_WU_BASE" HEAD`、`sh "$CELERIS_WU_SCOPE_PATHS"` | `config/skills/cos-operator/` の 3 file と、この進捗 file だけ |
| repo の検査 | `bash scripts/dev/test-affected.sh --list` | exit 0、対象 0（変更は `config/` と `agent-docs/` だけで crate を含まない。そのため上の試験を個別に走らせた） |
| repo の検査 | `cargo clippy --workspace -- -D warnings` | exit 0 |
| repo の検査 | `cargo fmt --all -- --check` | exit 0 |

## LoC

production の code（Rust・TS）0 行、試験 0 行。skill の文書 +96 行・−1 行（strategy-review.md +93、SKILL.md +2/−1、SOURCE.md +1）。この進捗 file は別。

## commit

- strategy-v1 5 cos-skill: CoS の skill に Strategy Review の手順・strategy の欄・拒否の code を書く（strategy-review.md、SKILL.md の索引と version 8、SOURCE.md の付記）
- strategy-v1 5 cos-skill: WU の進捗

## 本番への移行（人が行う）

skill の正本は repo の `config/skills/`。release の昇格で `scripts/selfdeploy/sync-skills.sh` が KB の `skills/` に写すなら、追加の操作は要らない。手で入れるときは次を人が実行する。

```sh
celerisctl skills import config/skills --name cos-operator --name cos-inbox-triage --root <kb_root>
```

確認: KB の `skills/cos-operator/strategy-review.md` があり、`skills/cos-operator/SKILL.md` の `metadata.version` が `"8"`。SKILL.md が strategy-review.md を link するので、
file を写さずに SKILL.md だけ入れると daemon の skill gate（`cos_skill_gate::check`）が missing として CoS run の claim を止める。必ず dir ごと取り込む。
`[cos.triage] policy_version` は triage skill（`cos-inbox-triage`、版 13）と比べる値で、cos-operator の版 8 とは比べないので config の変更は要らない。

## 未確認

- CoS が実際の抽象的な依頼でこの手順どおりに起票するか（LLM の振る舞い）は、子 6・子 8 の pilot で確かめる。
- 実験段階・実行段階の gate（`strategy_experiment_phase` ほか 4 つの code）は並行の WU experiment-gate が実装する。skill の code の説明はその ADR D5 の表に合わせた。
  gate の統合後に code の名前が変われば、この file を直す。

## 提案

- `cos-inbox-triage` の skill（この WU の write set の外）に、決定 `strategy-approve-v<n>` を一次対応で answer せず escalate する規則を足す。
  CoS の回答は gate が承認に数えないので害は無いが、受信箱で人の承認が遅れる。
