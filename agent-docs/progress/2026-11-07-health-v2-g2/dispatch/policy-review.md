---
title: 健全性v2 G2 dispatch policy-review — policy の変更を別 review にする（policy_mixed・policy verdict・人の承認）
tasks: [01M6VTFG9K5HEE817EW05JASMZ]
status: done
updated: 2026-11-08
---
# 健全性v2 G2 dispatch policy-review — policy の変更を別 review にする（policy_mixed・policy verdict・人の承認）

完了日: 2026-11-08

WorkUnit `policy-review`。基準は `$CELERIS_WU_BASE`（範囲 check）と `git merge-base HEAD main`（check-health・test-affected）。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../../adr/2026-11-07-health-review-integration.md) の D6.1・D6.3・D6.4・I3・I8。

## 変更

件名「G2 policy-review: policy の変更を別 review にする（policy_mixed・policy verdict の必須化・人の承認）」の commit。

- `crates/task-dispatch/src/health_review/verdict.rs`（前の段の health-verdict の写像に足した）: `Verdict::from_review_json` の 2 番目の引数を `health_requested: bool` から `Option<&HealthReviewInput>`（reviewer に渡した `ReviewRequest.health`）に変えた。
  - D6.3・I7: `policy.changed` の repo がある review で `health.policy` が無ければ `Err`（全 criterion を不合格。reason は `{tag}: health.policy verdict missing in review.json for the changed policy files: <files>`）。`approved = false` は `{tag}: health:policy: <files>: <reason>` の substantive の不合格。`approved = true` は合格の記録 `{tag}: health:policy (approved by reviewer; human approval required): …` だけで、合否は人の承認で決まる。
  - D6.1: `policy_mixed(repo)`。`policy.changed` の repo の変更 path に policy の file 以外があれば `{tag}: health:policy_mixed: <repo>: policy files [...] changed together with other files [...] (+N more); move the policy change into its own task` の substantive の不合格を足す。`diff.paths` は上限 40 件で切り詰めるので、`diff.files` の総数から見えない path の数を数え、見えない policy の file の数を引く。
  - policy の変更が無い review（`policy.changed` の repo が無い）は従来どおりで、reviewer が `policy` を書いても使わない。
- `crates/task-dispatch/src/health_review/policy_review.rs`（新規。`dispatcher::child_tasks` の子 module として `#[path]` で読む）: D6.4・I8 の人の承認。
  - `policy_review_gate`（`on_review_finished` が `all_pass` を数える前に 1 行で呼ぶ）: reviewer が判定した candidate の `HealthReviewInputBuilt` を events から引く（reviewer run があればその review の同期の `(target_sha, reviewed_sha)`、reviewer の合格を再利用した review は `ReviewerPassReused` の元の candidate）。`policy.changed` で、ほかの判定が全部合格なら、candidate ごとの `Approval` の子（title `Policy change approval: <repo_id> @ <sha12>`）を探す。無ければ既存の `create_human_approval_child` で作る（`ApprovalRequested` → 受信箱。本文に policy の file・diff の要約・reviewer の承認の理由）。子が終端でない間は、既存の「全 pass でも子が終端でなければ判定だけ記録して reviewing のまま待つ」で待つ。人の承認（done）は `human(approval task <id>): health:policy approved by a human: <files>` の合格、却下（failed）・取り消し（cancelled）は `health:policy: rejected by a human<note>` の substantive の不合格（→ `Trigger::ReviewFail`、replan）を足す。
  - `policy_approval_pending`（`resolve_human_approvals` の先頭）: 承認の子が終端でない間は review を始めない。`Check::Human` の待ちと同じく `awaiting_human` に入り、daemon の再起動の後も SQLite の子の状態だけで待つ。
  - `policy_rereview`（`settle_awaiting_children`）: 承認の子が終端になった task は直接 `ReviewPass` にせず review をやり直す。やり直しの review で上の gate が人の判断を verdict にする（reviewer の合格は ADR 2026-10-27 D2 で再利用される）。既存の子待ちは `Event::Delegated` の子の失敗しか見ないので、これが無いと却下でも `ReviewPass` になる。
  - I3: 判定に使う policy の状態は base 側の検査器で作った `HealthReviewInputBuilt` だけから読む。candidate の profile・検査器は読まない。LLM は呼ばない。
- 配線（`dispatcher/` は行を増やしていない）:
  - `dispatcher/child_tasks.rs`: `mod policy_review`（`#[path]`）、`resolve_human_approvals` の先頭の待ち、`settle_awaiting_children` の分岐、`create_human_approval_child` の引数を `idx` から `objective` に変えた。`resolve_human_approvals` の `match` を `(pass, reason)` を返す式にまとめて相殺（30 行増・30 行減）。
  - `dispatcher/review_verdict.rs`: `on_review_finished` に gate の呼び出し 1 行。同じ関数で 3 回あった `let review_run = completed_review_run.clone().unwrap_or_else(..)` を 1 回にまとめて相殺（7 行増・11 行減）。
  - `review.rs`: `run_reviewer_inner` の 2 行を書き換えただけ（`health` を bool から材料の写しへ）。行数は同じ。
  - `dispatcher/tests/mod.rs`: 試験の helper 3 つ（`new_task`・`dispatcher`・`done_adapter`）を `pub(super)` にした（新しい試験から使う）。
- 試験 `crates/task-dispatch/src/health_review/policy_review/tests.rs`（prefix `health_review_policy_review_`、6 件）:
  - `mixed_paths_replan`（受け入れ条件 0）: policy の file と `crates/x/src/lib.rs` を混ぜた candidate は reviewer が承認しても policy_mixed の不合格 1 件で `classify_review_failure` が `Substantive`。切り詰めで見えない他の file（+2）も混在と数え、policy の file だけ（見えない policy の file を含む）は混在でない。
  - `missing_policy_verdict_fails`: `health.policy` 欠落で `Err`、`approved = false` で substantive、`approved = true` は全部合格（条件の最後の verdict は reviewer 自身の判定）。
  - `approved_waits_for_human`（受け入れ条件 1）: 全部合格の outcome でも `Approval` の子（Ready・`ApprovalRequested`・本文に file と diff）を作り、`pending_children` が 1、`resolve_human_approvals` が `None`（review を始めない）。同じ candidate で子を作り直さない。人の承認の後は `policy_rereview` が true、reviewer の合格を再利用した review で「approved by a human」の合格が付く。
  - `human_rejection_replans`: 却下で substantive の不合格。SHA の変わった candidate（`c2`）には新しい子を作って待つ。
  - `no_policy_change_is_unchanged` と `without_policy_change_keeps_previous_behavior`: policy の変更が無い review は子を作らず verdict も変えない。ほかの判定が落ちている review では人に聞かない。
  - 既存の `health_review_verdict_` の 8 件は、helper で policy の変更の無い材料を渡すように 1 か所だけ直した。

## ADR との違い（判断）

- 人の承認の子の title は `human_approval_title`（試行回数で鍵を作る）でなく candidate の SHA で作った。承認は candidate ごと（I8）で、同じ candidate の再 review では同じ子を使い、SHA が変われば新しい承認を求める。`task-ops` の `parse_human_approval_title` とは接頭辞が違うので、`Check::Human` の criterion と取り違えない。
- 待ちの入口は `Check::Human` の既存の経路（`Approval` の子・`ApprovalRequested`・`resolve_human_approvals` の延期）と、`on_review_finished` の既存の子待ちを使った。新しい event の種類・状態は足していない。
- 人の却下の不合格の reason は `reviewer(<id>):` でなく `human(approval task <id>):` で始める。CoS の `review_rework`（reviewer の技術的差し戻しだけを自動再開する）の対象にしないため。

## 証拠（提出時の HEAD で実行）

実装の commit の HEAD で実行した。その後の commit はこの進捗 file だけを変える（`sh scripts/dev/progress-index.sh --check` と check-health はその後の HEAD でも実行）。

- `cargo nextest run -p task-dispatch --lib -E 'test(health_review_)'` → exit 0、47 passed（うち `health_review_policy_review_` 6 件: 受け入れ条件 0 は `mixed_paths_replan`、条件 1 は `approved_waits_for_human`、ほか `missing_policy_verdict_fails`・`human_rejection_replans`・`no_policy_change_is_unchanged`・`without_policy_change_keeps_previous_behavior`。`health_review_verdict_` 8 件も合格）
- `bash scripts/dev/test-affected.sh` → exit 0（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）。check-health は verdict=pass・new=0・worsened=0。nextest 3,546 件 passed（11 skipped、270.9 秒）。doc test も通った。script bundle SKIP 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 範囲 check: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は `dispatcher/child_tasks.rs`・`dispatcher/review_verdict.rs`・`dispatcher/tests/mod.rs`・`health_review/policy_review.rs`・`health_review/policy_review/tests.rs`・`health_review/verdict.rs`・`health_review/verdict/tests.rs`・`review.rs`（どれも `crates/task-dispatch/src/` の下）とこの進捗 file。計画の許可 path の外は無い。

## 未解決

- policy_mixed は reviewer run の結果（`review.json`）を読めたときだけ足す。決定的な検査が落ちて reviewer を起こさなかった review（Celeris では check-health の `policy.changed` で必須検査が落ちる）には足さない。その不合格は暗黙の条件の失敗として既に substantive（修復しない）なので replan に回る。`review.json` が読めない・health の欄が壊れているときも、全条件の不合格だけで policy_mixed は付かない。
- `Check::Reviewer` の条件が無い task（reviewer run の無い review）は health の材料を作らないので、policy の変更を見つけられず人の承認も求めない。
- 承認の子が終端になった後の review のやり直しは、子待ち（`awaiting_children`）からの 1 回だけで、ほかの委譲した子の待ちと重なったときも 1 回やり直す（判定は変わらない）。
- 流れ全体（reviewer run → 子待ち → 人の承認 → 再 review → done）は試験の単位を gate と待ちの関数に分けて確かめた。`on_review_finished` を通す流れ全体の試験は scenario の unit。
- 受信箱での見え方（Approval の子の本文の表示）は画面で確かめていない（未確認）。

## 提案

- close の unit で ADR に付記する: D6.4 の承認の子の title は candidate の SHA で鍵を作る、却下の reason は `human(approval task <id>):`、待ちは既存の子待ちと `resolve_human_approvals` の延期を使う、承認の子が終端になったら review をやり直す。
- 決定的な検査で止まった review にも policy_mixed を足すか（health の材料は検査の前に作っているので、`review_task` の reviewer を起こさない分岐で足せる）を ADR で決める。
