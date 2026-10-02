# ADR-0117: reviewer に人の決定・回答と決定的 check の結果を渡す

---
tasks: [01M3WZ1GFBPQ8T699ZF3Y66SJ3]
---

- 日付: 2026-10-02
- 状態: 採用（D1・D2 は後続の WorkUnit で実装する。D3 は見送り）
- 関連: [ADR-0007](0007-phase5-planner-and-reviewer.md)（D5 Reviewer run）、[ADR-0010](0010-phase7-hardening.md) D3（answers）、[ADR-0072](0072-task-execution-decomposition.md) D14/D16（WU checks・repair hint）、[ADR-0079](0079-recursive-task-decomposition.md) D7（decisions）

## 背景

web root task の final review（root v3・v5）で、次の 2 つの誤った不合格が出た。

1. **人の決定を知らない reviewer。** 人は decision `adr-place` に (a)「ADR は docs/adr に置く」と答え、変更範囲を広げた。reviewer はこれを知らされず、「人の判断による追加でも、この criterion の厳密な変更範囲は満たさない」として不合格にした。原因は、`build_review_prompt`（`crates/task-worker/src/claude_code/prompt.rs`）が answers も decisions も出力しないことにある。`answers_section` には「Review プロンプトには使わない」という注記が付いている。さらに `task-dispatch/src/review.rs` の `run_reviewer` は `answers: vec![]` を渡し、`ReviewRequest`（`crates/task-worker/src/protocol.rs`）は `summary` / `evidence` / `criteria` しか持たない。
2. **決定的 check より worker の記録を信じる reviewer。** `review_task` は `Check::Command` / `ArtifactExists` などの決定的な条件を先に workspace で実行し、すべて合格したとき（`deterministic_ok`）だけ reviewer を起動する。つまり reviewer が動く時点で決定的な条件は合格済みである。それなのに reviewer は、worker が自己申告した evidence にあった「exit 101」（sandbox 内で失敗した時点の記録）を根拠に、同じ事実に関わる reviewer criterion を不合格にした。合格した結果は reviewer に渡されていない。

## 決定

### D1: 人の決定と回答を ReviewRequest に載せ、優先するよう指示する

- `ReviewRequest` に次の 2 つの欄を追加する。
  - `decisions: Vec<ReviewDecision>`: 対象 task と root までの祖先 task で `answered` になっている decision。各要素は `task_id`（出した節点）、`key`、`question`、選んだ選択肢（`option` の key と `label`）、`note`（任意）を持つ。`withdrawn` と未回答のものは含めない。dispatcher は `TaskStore::decisions_list(Some(root_id))` から取り出し、`DecisionRow.task_id` が対象 task かその祖先であるものだけを残す（兄弟の決定は混ぜない）。並び順は `answered_at` の昇順とする。
  - `answers: Vec<Answer>`: 対象 task（と祖先）の `question` に人が返した回答。既存の `answers_from_events` を使う。`RunContext.answers` は worker run 用の意味を持つので流用せず、`ReviewRequest` 側の欄にする。
- `build_review_prompt` には、どちらかが空でなければ `## Human decisions and answers (authoritative)` 節を出す。指示文の要点は次のとおり。
  - 人の決定と回答は受け入れ条件の解釈より優先する。決定で範囲・場所・方式が変わったときは、変わった範囲を正しい範囲として判定する。
  - 人の決定に従った追加・変更（範囲の拡大を含む）を不合格の理由にしてはならない。
  - 決定の範囲を超えた変更は、従来どおり criterion に照らして判定する。
- 両方とも空のときは節を出さない。これにより、既存のプロンプトとバイト単位で同じになる。
- worker 用の `answers_section` は従来どおり worker プロンプト専用とする。注記は「review では `ReviewRequest.answers` を別の節で出す」に改める。

### D2: 決定的 check の verdict を渡し、反証には reviewer 自身の実行結果を求める

- `ReviewRequest` に `checks: Vec<ReviewCheckResult>` を追加する。中身は、同じ review で reviewer より先に出た決定的な verdict である。対象は task.acceptance の `Command` / `ArtifactExists` / `KnowledgePage` / `Human`、WU の `repo_checks`、暗黙の条件（plan / research など）。各要素は `criterion`（acceptance のインデックス。暗黙の条件なら `None`）、`kind`、`cmd`（Command のときだけ）、`pass`、`reason` の末尾を持つ。`reason` は末尾 1000 文字までに切り詰める。出力の終わりに exit と要点があるためである。
- `build_review_prompt` には `## Deterministic checks already executed by celeris (authoritative)` 節を出す。指示文の要点は次のとおり。
  - これらは worker の自己申告ではなく、celeris がこの workspace で実行した結果であり、worker の evidence より優先する。
  - 合格した check と矛盾する理由（例: 同じコマンドが「exit 101 で失敗した」）で不合格にするには、reviewer 自身が今そのコマンドを実行し、その実行コマンドと出力の要点を `reason` に書く。worker の記録・summary・evidence だけを根拠に不合格にしない。
  - 自分で実行できない場合（sandbox で書き込めない、時間がかかりすぎるなど）は、合格した check の結果を正とする。
- `checks` が空なら節を出さない（後方互換）。

### D3: 決定の回答で受け入れ条件の文言を改める入口は、本 task では実装しない

- 決定の `note` から `PATCH acceptance` を提案する入口（GUI のボタンや提案 event）は作らない。
- 理由は次のとおり。
  1. D1 によって、reviewer は決定を見て解釈を広げる。今回の誤判定はこれで解消できる。
  2. acceptance の文言の書き換えは、人の承認が要る後戻りしにくい操作である。decision の自由記述 note から機械的に作ると、意図しない条件の緩和が起きうる。
  3. task の acceptance は decision で書き換わらないという現在の契約（final review は元の条件で再評価する）を崩すには、別の設計判断が要る。
- 将来の入口案: decision に回答するときに「acceptance の修正案」を任意で付けられるようにする（`DecisionAnswer.acceptance_patch` の提案）。その場合も、GUI が差分を見せ、人が確定したときにだけ既存の `PATCH /api/v1/tasks/{id}` で acceptance を更新し、`AcceptanceChanged` の event を残す。LLM がこれを自動で適用する経路は作らない。

## 後方互換

- 新しい欄（`decisions` / `answers` / `checks`）には、いずれも `#[serde(default, skip_serializing_if = "Vec::is_empty")]` を付ける。古い worker は新しい欄を無視し、新しい worker は欄の無い古い要求を空として読む。このため `PROTOCOL_VERSION` は上げない（v4 の追加のみの規約に従う）。JSON schema は `UPDATE_SCHEMA=1` で再生成する。
- 欄が空のときのプロンプトは、従来と同じになる。

## テスト方針

- task-worker:
  - `build_review_prompt` の単体試験を置く。決定（question、選択肢の label、note）、回答、check の結果（cmd、pass、reason の末尾）がそれぞれの節に出ることを確かめる。
  - 「人の決定に従った範囲の拡大を不合格の理由にしない」指示と、「合格 check への反証には自分の実行結果を書く」指示の文言が入ることも確かめる。
  - 欄が空のときは節が出ないことを確かめる。
  - 欄を持たない旧 JSON の `ReviewRequest` が deserialize できることを確かめる。
- task-dispatch:
  - 祖先の answered decision は入り、兄弟の decision・未回答・withdrawn は入らないことを確かめる。
  - 先に出た決定的 verdict が `checks` に写ることを確かめる。
  - reason が切り詰められることを確かめる。
  - stub adapter で受け取った `RunRequest.context.review` を検査する。
- LLM を呼ぶ実機の確認はしない（外部ネットワークに出ない）。
