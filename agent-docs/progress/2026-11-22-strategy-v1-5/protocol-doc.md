---
title: 戦略層 v1 子 5 protocol-doc — worker-protocol.md に ReviewRequest.strategy_critic・ReviewOutput.critic と暗黙の条件 strategy_critic の規則を書く
tasks: [01M7XQ9QQXY1DY56EXE0TT8X6W]
wu: protocol-doc
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# protocol-doc（WorkUnit。子 5 の final review の不合格（条件 4）の修正）

完了日: 2026-11-22。設計の正本は [ADR 2026-11-22-strategy-review-critic](../../adr/2026-11-22-strategy-review-critic.md) の D3.3〜D3.6・D8。
基準はこの WU の `$CELERIS_WU_BASE`。検査はすべて提出時の HEAD で実行した。コード・schema・試験・ADR は変えていない。

## 結論

- 子 5 の final review（reviewer 01M823J2NSDJF7JQT0KGG28HXZ）は条件 4 を不合格にした。理由は、critic-review が足した wire の欄
  （`ReviewRequest.strategy_critic`・`ReviewOutput.critic`）と暗黙の条件 `strategy_critic` の規則を `docs/protocol/worker-protocol.md` が載せていないこと
  （ADR D8 は critic-review の書く場所にこの file を挙げていたが、critic-review は `worker-protocol.schema.json` だけを直した）。
- この WU は `docs/protocol/worker-protocol.md` だけを直した:
  - `context.review` の表（§3.1）と §10.2 の `ReviewRequest` の列挙の 2 か所を
    `{summary, evidence, criteria, decisions?, answers?, checks?, policy_violations?, health?, design?, diff_design?, strategy_critic?}` にした。
  - §10.2 の欄の説明の文に「`strategy_critic` は戦略の文書を作った task の final review にだけ渡す Critic の材料」を足した。
  - §10.2 の「final review の差分の設計の節」の後に節「戦略の文書の Critic（ADR 2026-11-22-strategy-review-critic D3.3〜D3.6）」を足した。
    中身: 対象の導き方（D3.1）と reviewer run を起こす条件（D3.2）、入力 `StrategyCriticRequest {criterion_idx, subjects[{name, path, sha256, strategy_id, version}], rubric[{id, text}]}`
    と固定の基準 C1〜C5、`None` なら `request.json` と prompt が base と同じこと、出力 `ReviewOutput.critic` の形
    `{subjects[{name, sha256, candidates[{id, defects, falsification{ok,reason}, experiment{ok,reason}}], comparison{ok,reason}, revisit{ok,reason}}]}`、
    `verdicts` と並べた 1 行の例（`"critic":{"subjects":[` で始まる）、dispatcher（`task_dispatch::review::critic`）が形だけを照合して LLM を呼ばないこと、
    不合格になる場合（reviewer run 無し・判定されなかった・`critic` 節の欠け・形の崩れ・対象や候補 id の食い違いと重複・対象の衝突・文書の書き換え・
    `reason` の空や 2,000 字超・どれか 1 つの `ok=false`）。
- 節の文は実装（`crates/task-dispatch/src/review/critic.rs` の `problems`・`document_candidates`、`crates/task-dispatch/src/review.rs` の `not_evaluated`・
  `without_reviewer_run`・`fail_all`、`crates/task-core/src/strategy_critic.rs` の `CRITIC_RUBRIC`、`crates/task-worker/src/change_rules/review.rs` の
  `CRITIC_VERDICT_LINES`）と schema の定義を読んで合わせた。

## 変更（commit の件名）

- `strategy-v1 5 protocol-doc: worker-protocol.md に ReviewRequest.strategy_critic・ReviewOutput.critic と暗黙の条件 strategy_critic の規則を書く`
- `strategy-v1 5 protocol-doc: WU の進捗と親の進捗（WorkUnit の表・検査・review の助言 2 件の提案）`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

| 受け入れ条件 | 実行したこと | 出力の要点 |
|---|---|---|
| 0. 列挙 2 か所が `strategy_critic?` を含む | `grep -c 'diff_design?, strategy_critic?}' docs/protocol/worker-protocol.md` | `2`（106 行の表と 747 行の §10.2） |
| 1. Critic の節（入力・出力・例・不合格の規則） | `grep -n '戦略の文書の Critic\|"critic":{"subjects":\[' docs/protocol/worker-protocol.md`、例の JSON を `python3 -I` の `json.loads` で読む | 節の見出しと例の行がある。例は JSON として読め、schema の `ReviewOutput`・`CriticVerdictOut` の必須の欄（`verdicts`・`subjects`・`name`・`sha256`・`candidates`・`id`・`falsification`・`experiment`・`comparison`・`revisit`・`ok`・`reason`）を全部持つ |
| 2. schema の Critic の 7 定義の欄名が全部載る | `python3 -I` で `worker-protocol.schema.json` の `$defs` の `CriticCandidateVerdict`・`CriticCheckOut`・`CriticRubricItem`・`CriticSubjectVerdict`・`CriticVerdictOut`・`StrategyCriticRequest`・`StrategyCriticSubject` の定義名と `properties` の欄名を worker-protocol.md から探す | 欠け 0（定義名 7・欄名 18 とも載る） |
| 3. 進捗に修正・check の結果・助言 2 件の撤去の条件と判断者 | この file と親の進捗 [2026-11-22-strategy-v1-5.md](../2026-11-22-strategy-v1-5.md)（WorkUnit の表の protocol-doc の行、「検査」の節、「提案」の節） | 書いた |
| 4. crates/・schema・ADR を変えていない | `git diff --name-only $CELERIS_WU_BASE HEAD`、`sh "$CELERIS_WU_SCOPE_PATHS"` | 変えた file は `docs/protocol/worker-protocol.md`・この file・親の進捗の 3 つだけ |

検査（提出時の HEAD で実行）:

- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (478 files)`）。
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。
- `bash scripts/dev/test-affected.sh` → exit 0（`repo_health_gate: all checks passed`、`test-affected: 試験対象の変更なし（docs のみ等）`）。
- `cargo clippy --workspace -- -D warnings` → exit 0（vendor の `deno_core` の既存の deprecated の警告 1 件だけ）。
- `cargo fmt --all -- --check` → exit 0。

## 未解決

- 文書の節は手で書いた写しで、schema や実装と文字列で照合する試験は無い（`worker-protocol.md` はどの試験にも埋め込まれていない）。
  欄名の載り方は上の条件 2 の手順で確かめた。
- review の助言 2 件（gate の `write_tx` の中の生の SQL、`bare_sha` の 3 つの写し）はコードの変更なので、この WU では直していない。撤去の条件と判断者は
  親の進捗の「提案」の節に書いた。

## 提案

- `worker-protocol.md` の §10.2 の各節（`health`・`design`・`diff_design`・Critic）の欄名を schema の `$defs` と照合する doc の検査を
  `scripts/dev/` に置けば、今回のような載せ忘れを review の前に決定的に見つけられる。要否は人が決める。
