---
title: 健全性v2 G3 limits review-wire — task の review の health の Error・往復・修復の上限の使い切りを Blocked と受信箱の質問で止める
tasks: [01M70C8NHEANARRKZD9RGKZ87D]
status: done
updated: 2026-11-09
completed: 2026-11-09
---
# 健全性v2 G3 limits review-wire — task の review の health の Error・往復・修復の上限の使い切りを Blocked と受信箱の質問で止める

WorkUnit `review-wire`（task 01M70C8NHEANARRKZD9RGKZ87D）。[ADR 2026-11-08](../../adr/2026-11-08-health-enforcement-loop.md) の D3.4・D4.3・D4.4 を task の review の経路（`Dispatcher::try_review_repair`）に配線する。基準は `$CELERIS_WU_BASE`。

結論: 完了。review の health の Error・Unavailable、同じ指摘の往復、修復の上限の使い切りを `Trigger::ReviewBlocked`（attempts 不変）と受信箱の質問 1 件で止め、repair・再実行・replan・review_rework を起こさない。試験 7 件 `health_limits_review_*` が合格し、提出時の HEAD で範囲 check・試験の check・fmt と clippy・test-affected の 4 つが exit 0。

## 実装前 brief
1. 根本原因: task の review の不合格は `try_review_repair` が `None` を返すと呼び出し側が `Trigger::ReviewFail`（`retry_or_fail`）に進む。health の Error・Unavailable は修復できない種類なので `ReviewFail` に落ち、retry（atomic は再実行、compound は最終レビューの後の replan）か、retry 切れで Failed → CoS の review_rework になる。health の Fail が修復の上限に達したときも同じ経路に落ち、同じ指摘の往復は判定されていない。
2. 責務 owner: 状態遷移は `task_core::transition`。review の不合格の扱いの判定は `dispatcher::review_verdict::try_review_repair`。health の止め方（質問・root への集約）は health_review の側（`check_item.rs` と同じく dispatcher の子 module として `#[path]` で読む file）。往復の判定は `health_review::limits::judge`（変えない）、root への追記は `task_ops::inbox::health_repeat::record_findings_repeated`（escalation）。
3. 既存の再利用先: 質問の出し方は `escalate_failed_children`（`record_question_approval` と `QuestionRaised`）。再開は既存の `Trigger::Answer`（comment・answer）。health の Error の見分けは `crate::review::HEALTH_CHECK_ERROR`。修復の数え方は `check_item::repair_count`（rule ごと `health:<rule_id>`）。試験の土台は `health_enforce.rs` の stub の検査器と `target_sync::reviewing_task`。
4. 選択肢: (a) 既存の `ChildFailed`（reviewing → blocked）を使い回す案は attempts を進める分岐と reason（`child_failed`）を持ち、replay と planner の理由の読み取りがそれを子の失敗と読むので採らない。(b) Trigger を 1 つ足す（採用）。(c) 往復の判定を review の評価の時点（`review_item::evaluate_all`）で済ませて結果を `ReviewOutcome` で運ぶ案は、型の欄と `Verdict` の作り方を広く変えるので採らない。(d) helper を `Dispatcher` の method にする案は check-health の `size.responsibility-growth`（`Dispatcher` の method 数・dispatcher module の行の合計）と `deps.cycle`（`dispatcher` → `health_review` の新しい辺）で落ちたので、free 関数にして health_review の下の file に置く。
5. 新しい状態・分岐・fallback・port: `Trigger::ReviewBlocked`（`Reviewing → Blocked`、attempts 不変、reason `review_blocked`）。状態（Status）・config の欄・event の種類・API の欄は足さない。
6. 撤去対象: health だけの不合格の「`try_review_repair` の `None` → `ReviewFail` → `retry_or_fail`（atomic の再実行・compound の replan・retry 切れの Failed → review_rework）」の経路。health の Error の `ReviewFail` の経路。どちらも `try_review_repair` が `ReviewBlocked` で止めるので通らなくなった（専用のコードは無く、通らなくなったことを試験で固定した）。
7. 不変条件: health の Error・Unavailable は合格にも修復にもならない。同じ指摘の往復は 2 回目の repair を作らず root に 1 件。health だけの不合格が上限を使い切ると attempts は進まず、run・repair・replan を起こさない。health でない失敗が混ざる不合格は今どおり `ReviewFail`。止めた task は comment で再開する。試験は下の 7 件。

## 変えた契約
- health の Error・Unavailable（`health_check_error: <理由>`）の review: 以前は `ReviewFail`（`max_retries` が 0 なら Failed → CoS の review_rework）。今は `ReviewBlocked` で Blocked、受信箱に `health_check_error: <理由>` の質問 1 件。task の種類を問わず、他の不合格より先に見る。
- health だけの不合格が `max_repairs`・`max_repairs_per_class`（`health:<rule_id>`）を使い切った review: 以前は `ReviewFail`（attempts+1 の再実行、compound は replan、retry 切れで Failed → review_rework）。今は `ReviewBlocked` で Blocked、受信箱に `health_repair_cap: …`（bucket・bucket の修復の数・`max_repairs_per_class`・修復の合計・`max_repairs`・指摘）の質問 1 件。
- 同じ指摘の往復（`limits::judge` の範囲 review が `Repeat`）: 以前は上限まで repair を作り続けた。今は repair を作らず Blocked、`health_findings_repeated: …` の質問を出し、木の root に `HealthFindingsRepeated`（位置 review）を追記する（root の受信箱の `health_repeat` 1 件）。
- 既存の試験 `health_review_policy_unknown_flow_missing_report_is_not_a_pass`（`dispatcher/tests/health_review_input.rs`）は Error の review を Failed と確かめていた。契約の変更に合わせて Blocked に直した（v2 の計画で範囲に足した。下の「範囲の決着」）。`health_enforce_direct_review_*`（`review_item_tests.rs`）は `review_task` の項目だけを見て遷移を見ないので、変える必要は無かった。

## 変更（commit の件名）
- 「G3 limits review-wire: review の health の Error・往復・修復の上限の使い切りを ReviewBlocked と受信箱の質問で止める」
  - `crates/task-core/src/transition.rs`: `Trigger::ReviewBlocked`。`transition/tests.rs` の表の試験に足した（4 kinds × 8 statuses × 35 triggers）。
  - `crates/task-dispatch/src/review/required.rs`: `Verdict::health_check_error`（acceptance の外の health の Error・Unavailable の reason）。
  - `crates/task-dispatch/src/dispatcher/review_verdict.rs`: 呼び出し側の `task.kind == Execute` の条件を `try_review_repair` の中（Error の判定の後）に移し、Error・往復・上限の使い切りの 3 か所で止める。
  - `crates/task-dispatch/src/dispatcher/tests/health_enforce.rs`: stub の検査器の fingerprint を作業ツリーの `FINDING` から読めるようにした（無ければ今までと同じ `deps.violation:0000000000000001`）。修復の WU の後に任意の処理を走らせる `RepairActionAdapter`（以前の `RepairCommitAdapter` を一般化）と、`commit_base_checker`・`non_celeris_dispatcher`・`commit_fixed` を `pub(super)` にした。
  - `crates/task-dispatch/src/dispatcher/tests/health_limits_review.rs`（新規）と `tests/mod.rs` の末尾の 1 行。
- 「G3 limits review-wire: transition の Blocked → Ready の同形の分岐を 1 つにまとめ、関数の行を基準より減らす」: check-health の `size.responsibility-growth`（`fn transition` が閾値 200 行を超えたまま 455 → 465 行に伸びた）を直すため、本体の同じ `Answer`・`PhaseResume`・`BrowserResume`・`ClusterJobResume`・`BrowserPrereqResume` の 5 分岐を 1 つにした（動作は同じ。表の試験が全組で確かめる）。`fn transition` は 425 行（基準 452 行）。
- 「G3 limits review-wire: health の止め方を free 関数の module にまとめ、Dispatcher の method と dispatcher の行を増やさない」: 下の「範囲の拡張の後の形」の module を暫定で `review_verdict.rs` の inline module `review_limits` にした。`try_review_repair` の `failing` の組み立てを短くし、空の `failing` の早期 return を外した（空なら `classify_review_failure` が `Substantive` を返すので動作は同じ）。
- 「G3 limits review-wire: health の止め方を health_review/review_limits.rs に移して #[path] で読み、health_review_input の Error の試験を Blocked に直す」: inline module の本体を `crates/task-dispatch/src/health_review/review_limits.rs` に移し、`review_verdict.rs` は `#[path = "../health_review/review_limits.rs"] mod review_limits;` で読む（`check_item.rs`・`policy_review.rs` と同じ形）。`health_review_input.rs` の 1 行を `Status::Blocked` にした。2 file は v2 の計画で範囲に足された（下の「範囲の決着」）。

## 試験（守る不変条件）
既存の試験は review の項目と修復の 1 往復だけを見て、Error・往復・上限の使い切りの後の遷移を通らない。`health_limits_repeat_*` は判定の純粋関数、`health_limits_inbox_*` は受信箱の導出だけを見る。ここは dispatcher の流れで見る。時計は使わず、`run_until_idle`（出来事が尽きるまで tick を回す）で決まる。外部ネットワークに出ず、LLM を呼ばない。
- `health_limits_review_error_blocks_without_repair`: 検査器の exit 2 で Blocked、attempts 0、`review_blocked` 1 回、その後の run なし、repair なし、受信箱の質問 1 件（`health_check_error: `）。
- `health_limits_review_unavailable_is_not_pass`: base にも同梱にも検査器が無いと `health_check_error: checker_missing` で Blocked、合格にしない。
- `health_limits_review_same_fingerprint_blocks`: 修復の後に同じ fingerprint が残ると repair は 1 回だけで Blocked、root（自分）に `HealthFindingsRepeated`（位置 review、fingerprint 1 件）、受信箱の `health_repeat` 1 件、質問 1 件。
- `health_limits_review_repair_cap_blocks_without_rerun`（atomic、`max_repairs_per_class` = 1）: 指摘が修復ごとに変わっても上限で Blocked、attempts 0、新しい run も repair も無く、質問 1 件に bucket と回数。
- `health_limits_review_compound_repair_cap_does_not_replan`（計画あり、`max_repairs` = 1、`max_retries` = 2）: `AutoReplanRequested` も `ExecutionPlanned` も増えず、計画の版は 1、planner の run なし。
- `health_limits_review_mixed_failure_keeps_review_fail`: health の Fail と acceptance の `false` が混ざると今どおり `ReviewFail`（Failed、attempts 1）、質問なし。
- `health_limits_review_blocked_resumes_on_comment`: Error で止めた task に環境を直して comment すると `answer` で Ready（attempts 0）、次の review で Pass になり Done。

## 証拠（提出時の HEAD で実行。最後の同期の後に再実行する）
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` を計画 v2 の正規表現で絞る）→ exit 0、範囲外の path なし。
- `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/health_limits_/) | test(/health_enforce_/)'`（WU の check 1。7 件の `fn` の grep を含む）→ exit 0、`65 tests run: 65 passed, 2891 skipped`。
- `cargo fmt --all -- --check && cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0。nextest `8110 tests run: 8110 passed (10 slow), 23 skipped`。web の vitest `Test Files 104 passed`・`Tests 711 passed`。check-health `repo-health: verdict=pass exit=0 fail_grade=0 new=2 worsened=0`（new の 2 件は warning の `dup.contract`（`UnavailableReason`）と `dup.public-surface`（`fn judge`）で、どちらも前の unit の変更）。
- 参考: `bash scripts/dev/check-health.sh` を inline module の形（「health の止め方を free 関数の module にまとめ…」の commit）で実行すると exit 1。`deps.cycle` が 3 単位 → 4 単位（`dispatcher` → `health_review` の辺で health_review が base の循環 {dispatcher, review, undeclared_artifacts} に入る）、`size.responsibility-growth` の dispatcher module の本体の行が 35163 → 35206。今の形では両方とも基準のまま（worsened 0）。

## 範囲の決着
- 最初の計画の範囲には `crates/task-dispatch/src/health_review/review_limits.rs` と `crates/task-dispatch/src/dispatcher/tests/health_review_input.rs` が無く、`yield.plan_issue` で申告した。計画 v2 が key と依存を保ったまま、2 path を範囲 check の正規表現に足した。
  - `review_limits.rs`: health の止め方（`stop_on_error`・`stop_on_repeat`・`stop_on_cap`・`block`）を free 関数で置き、`review_verdict.rs` が `#[path = "../health_review/review_limits.rs"] mod review_limits;` で読む（`check_item.rs`・`policy_review.rs` と同じ形）。dispatcher の file に置くと上の 2 つの finding で check-health が落ちる。範囲の中の `review_item.rs` は health_review の module なので、dispatcher から呼ぶと同じ辺ができる。
  - `health_review_input.rs`: `health_review_policy_unknown_flow_missing_report_is_not_a_pass` の `Status::Failed` を `Status::Blocked` にした（D3.4 の契約の変更。他の確かめは変えていない）。

## 未解決
- 無し（受信箱の件数と CoS の扱いは「提案」に書いた）。

## 提案
- 受信箱: 往復で止めた源の task は Blocked なので、root の `health_repeat` とは別に源の質問の項目が出る（escalation の設計では人は源への comment で再開する）。root が源と同じ task でも 2 件になる。源が往復の源である間は質問の項目を出さない絞り込みを task-ops の受信箱に足すかを、record の unit で決めるとよい。
- CoS の一次対応: `health_check_error:`・`health_repair_cap:`・`health_findings_repeated:` の質問に CoS が自由記述で答えると、環境や指摘が直らないまま再開して同じ理由でまた止まりうる（止まるたびに質問 1 件で、修復・再実行は増えない）。CoS の方針でこの 3 つを人に上げる規則を置くかを、budgets か record の unit で決めるとよい。

## 本番への移行
- config・表・migration の変更は無い。event の `Transitioned.reason` に `review_blocked` が増える（replay は attempts を進めない理由として読む。`task_ops::replay` の変更は要らない）。
