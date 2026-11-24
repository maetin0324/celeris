---
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: dedup-api
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 7 dedup-api: 決定の記録・最後の回答・manifest の違反の型の重複をなくす

## 結論
- final review（v3）の助言（規約 2 `duplicate_implementation`、repo health の `dup.contract`）の 3 件を直した。挙動は変えていない。
  1. `task_core::strategy::ApprovalRecord` と `task_core::publish::SignoffRecord` を消し、`task_core::decision::DecisionRecord`（key・問い・状態・最後の回答）の 1 つにした。
  2. 最後の `DecisionAnswered` の `(option, by)` を引く照会を `task_core::decision::DecisionRow::last_answer` の 1 つにした。
     承認の照合（option が `approve` で主体が人）は `task_core::strategy::approving_human` の 1 つにし、子 2 の照合と概要の O2 が同じ関数を呼ぶ。
  3. `task_api::publish::ManifestViolationView` を消し、`task_core::publish::ManifestViolation`（`code` を `String`、`JsonSchema` を付けた）を wire に使う。
     wire の名は `ManifestViolationView` から `ManifestViolation` に変わった。生成物（`docs/api/v1/api-v1.schema.json`・`web/api/generated/`）を再生成した。
- 共有の関数・型は `crates/task-core/src/decision.rs` と `crates/task-core/src/strategy/mod.rs` に置いた。`store/`・`model/events.rs` は触っていない。
- 新しい表・migration・event の種類・Manager・trait・状態機械は足していない。Project.request と Roadmap は書き換えていない。本番昇格・publish はしていない。

## 実装前 brief
1. 根本原因: 子 2（戦略の承認）と子 7（publish の署名・概要）が、決定の行と最後の回答を読む型・照会・承認の照合をそれぞれ書いた。
   manifest の違反は core の型の `code` が `&'static str` で `JsonSchema` を持たないため、API 側が写しの型を作った。
2. 責務 owner: 決定の行の型と畳み込みは `task_core::decision`（`DecisionRow::apply_answer` が最後の回答を行に写す）。承認の照合は子 2 の
   `task_core::strategy`（D2.2）。manifest の検査の結果は `task_core::publish`。
3. 既存の再利用先: `DecisionRow::request.answer`（store が `DecisionAnswered` と同じトランザクションで `apply_answer` で写す。revise も上書きする）、
   `strategy::human_approval`（D2.2 の 1〜5）、`envelope::human_approved`（R10）。
4. 選択肢: 最後の回答の照会は (a) task の event を走査する関数を共有する、(b) 決定の行に写した回答を読む、の 2 案。
   (b) を採った。行は event と同じトランザクションで畳み込まれる派生（正本は event のまま）で、publish の読み取りは既に行を読んでいた。
   (a) は strategy_gate が承認の照合のたびに task の全 event を読み直す形を残す。型の置き場は `strategy`（publish が名前の違う意味の
   型を使う）と `decision` を比べ、決定の行の型と同じ所に置いた（strategy・publish はどちらも既に `decision` を使っている。新しい module の依存の組は無い）。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: `strategy::ApprovalRecord`、`publish::SignoffRecord`、`task_api::publish::ManifestViolationView`、
   `task_api::cos::strategy_gate::last_answer`、`task_ops::project_overview::Events::last_answer`、承認の照合の写し 3 か所
   （strategy の `human_approval` の match、`envelope::human_approved` の `matches!`、strategy_gate の `human_approvals`、概要の O2 の filter）。
7. 不変条件: 署名・承認・案の採否・概要の結果は同じ（下の 5 接頭辞の試験と schema の一致の試験が変更なしで通る）。最後の回答は revise で
   上書きされ取り下げの後も残る（新しい試験 `row_last_answer_is_the_latest_answer_and_survives_withdrawal`）。

## 消した型・関数と、置き換えた呼び出し元
| 消したもの | 置き換え先 | 呼び出し元 |
|---|---|---|
| `task_core::strategy::ApprovalRecord` | `task_core::decision::DecisionRecord` | `strategy::CheckContext::approval`、`strategy::envelope::{EnvelopeContext::approval, human_approved, outbound_allowed}`、`discussion_exchange`、`task_api::cos::strategy_gate::approval`、試験（strategy・envelope・discussion_exchange・celeris-mcp の `mcp_external_scope_mcp_answer_not_strategy_approval`） |
| `task_core::publish::SignoffRecord` | `task_core::decision::DecisionRecord`（`DecisionRecord::from_row`） | `publish::signoff_state`、`task_api::publish::signoff_view`、`publish/tests.rs` |
| `task_api::cos::strategy_gate::last_answer`（event の走査） | `DecisionRow::last_answer` | `strategy_gate::approval`（`DecisionRecord::from_row`）、`Reader::human_approvals` |
| `task_ops::project_overview::Events::last_answer`（event の走査） | `DecisionRow::last_answer` | 概要の O2 `strategy_series` |
| publish の読み取りの `row.request.answer` の写し | `DecisionRow::last_answer` | `task_api::publish::signoff_view` |
| 承認の照合 `option == APPROVE_OPTION && is_human_principal(by)` の写し | `task_core::strategy::approving_human` | `strategy::Input::human_approval`、`envelope::human_approved`、`strategy_gate::Reader::human_approvals`、概要の O2 |
| `task_api::publish::ManifestViolationView` | `task_core::publish::ManifestViolation`（`code: String`、`JsonSchema`） | `ManifestView::violations`、`manifest_status`、`check_before_sign` の 422 の `violations` |

## 受け入れ条件の確認（提出時の HEAD で実行）
- 条件 0（`SignoffRecord`・`ManifestViolationView` の定義が crates に無い）: `git grep -nE '\b(SignoffRecord|ManifestViolationView|ApprovalRecord)\b' -- crates` → exit 1（0 件）。
- 条件 1（最後の回答の照会の関数定義が crates/ に 1 つ）: `git grep -nE 'fn last_answer\b' -- crates` → 1 件（`crates/task-core/src/decision.rs` の `DecisionRow::last_answer`）。
- 条件 2（既存の 5 接頭辞の試験と schema の一致の試験）:
  `cargo nextest run -p task-core -p task-ops -p task-api -p celeris-mcp -E 'test(/publish_manifest_|publish_signoff_|proposal_review_|project_overview_|cos_ops_registry_/) | test(/committed_schema_matches_generated$/)' --no-tests=fail`
  → exit 0、90 passed。接頭辞ごとに publish_manifest_ 15・publish_signoff_ 17・proposal_review_ 23・project_overview_ 23・cos_ops_registry_ 5 passed。
  `cargo nextest run --workspace -E 'test(/committed_schema_matches_generated$/)'` → 8 passed。

## 試験
- 足した試験: `task_core::decision::tests::row_last_answer_is_the_latest_answer_and_survives_withdrawal`。守る不変条件は「最後の回答の唯一の照会は
  revise の後の回答を返し、取り下げの後も残る。`DecisionRecord::from_row` も同じ値」。strategy_gate と概要は event の走査から行の読み取りに替えたので、
  行の値が最後の `DecisionAnswered` と同じであることを固定する。既存の `row_folds_answer_and_withdrawal` は 1 回目の回答の option だけを見て、主体・revise・取り下げの後を見ない。
- 試験は消していない・弱めていない。既存の試験の変更は型の名前の付け替えと、`codes()` が `&'static str` でなく `&str` を返すことだけ。

## 検査（提出時の HEAD で実行）
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh`（基点 `$CELERIS_WU_BASE`）→ exit 0。Rust 8,902 passed・34 skipped、web の tsc -b exit 0・vitest 771 passed・
  node --test 110 passed、ts-verify 20 passed。repo-health は verdict=pass・fail_grade 0（基点 merge-base main）。
- repo-health の `dup.contract`・`dup.public-surface` に `ApprovalRecord`・`SignoffRecord`・`ManifestViolationView`・`last_answer` の finding は無い
  （drilldown-api の時点では新しい finding として出ていた）。残る new・worsened の 5 件は名前だけが同じ別の意味の型・関数
  （`Target`・`Manifest`・`Entry`・`idempotency_key`・`validate_manifest`。campaign・routing-eval・script-runtime・strategy と publish の key の書式）で、この WU の範囲の外。

## 測定（この WU の差分。基準は `$CELERIS_WU_BASE`、`git diff --numstat`）
- production の Rust: +90 −122（純減 32 行）。
- 試験: 既存の試験の付け替え +20 −20、新しい試験 1 件。
- 生成物（schema 2 件・types.ts）: +10 −8。

## commit（件名）
- strategy-v1 7 dedup-api: 決定の記録・最後の回答・manifest の違反の型を 1 つにする
- strategy-v1 7 dedup-api: 最後の回答の試験と WU の進捗を書く

## 未確認・引き継ぎ
- wire の名の変更（`ManifestViolationView` → `ManifestViolation`）は生成物だけに効く。`web/features`・`web/e2e` はこの型の名前を使っていない（git grep で 0 件）。
- 子 4 の ADR `agent-docs/adr/2026-11-22-external-llm-discussion-exchange.md` は当時の型の名前 `ApprovalRecord` を書いている。ADR は当時の記録として残した。
  今の名前への読み替えは reclose-audit で ADR の保証の表と合わせて見る（提案）。
- `docs/api/v1/api.md` の §2 の表と拒否の code の記述は api-doc の WU の範囲で、この WU は触っていない。

## 親の目的への寄与
- 確認済み: observability（概要の O2・publish の署名）の読み取りは、子 2 の承認の照合と同じ関数を通る。概要と strategy の gate が
  「承認済み」を別の規則で数えることが無くなった（同じ `approving_human`）。
- 推定: 型と照会が 1 つになったので、承認・署名の規則を変えるときに直す所は 1 か所になる。
- 未確認: 本番の大きさの DB での概要の応答時間（event の走査を減らした効果は測っていない）。
