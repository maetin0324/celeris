---
title: 健全性v2 G3 limits repeat — health の同じ指摘の往復を判定する純粋関数 health_review::limits
tasks: [01M70C8NHEANARRKZD9RGKZ87D]
status: done
updated: 2026-11-09
completed: 2026-11-09
---
# 健全性v2 G3 limits repeat — health の同じ指摘の往復を判定する純粋関数 health_review::limits

WorkUnit `repeat`（task 01M70C8NHEANARRKZD9RGKZ87D）。[ADR 2026-11-08](../../adr/2026-11-08-health-enforcement-loop.md) の D4.3 の判定だけを作った。経路への配線（Blocked・木の root の受信箱）は段 wire が持つ。基準は `$CELERIS_WU_BASE`。

結論: `crates/task-dispatch/src/health_review/limits.rs` の `judge(events, scope)` が、範囲ごとに `Repeat{fingerprints, set_sha256}` か `Proceed` を返す。I/O・LLM・時計を持たない。人の再開の判定は `task_ops::plan_gate::is_human_resume` の 1 か所にあり、`stalled_replans` と `limits` が共有する。`health_limits_repeat_` の試験 8 件と `plan_gate::stall` の試験 7 件が合格した。

## 変更（commit の件名）
- 「G3 limits repeat: health の同じ指摘の往復を判定する health_review::limits と、人の再開の判定 is_human_resume の共有」
  - `crates/task-ops/src/plan_gate/stall.rs`: `pub fn is_human_resume` を足した。中身は既存の `is_human_replan_request`（人の replan の依頼・`answer`）に、`Transitioned.reason` が `Trigger::Interrupt.reason()`（`comment`）・`Trigger::Reopen.reason()`（`reopen`）のものを足した判定。`stalled_replans` はこれで連なりを切る。`pending_integration_auto_replan` は今の `is_human_replan_request` のまま。`crates/task-ops/src/plan_gate.rs` から公開した。
  - `crates/task-dispatch/src/health_review/limits.rs`（新規。`health_review.rs` で `pub mod limits` と宣言。`pub` にしたのは、配線の前の段で crate の中に呼び出し元が無く、`pub(crate)` だと dead_code になるため）:
    - `HealthScope::{WorkUnit(key), Phase(段の key), Review}` と `RepeatVerdict::{Proceed, Repeat{fingerprints, set_sha256}}`。
    - 評価と修復の読み方（範囲ごと）:

      | 範囲 | 評価（health の証跡の正本。D2.2） | 修復・再試行 |
      |---|---|---|
      | WU の key | 同じ key の `WorkUnitCheckFinished.health` | 同じ key の `WorkUnitChecksFailed`（daemon が run を WU の retry にすり替えた記録） |
      | 段の key | `IntegrationCheckFinished.health`（key が `task_core::integrate_key(段)`） | `RepairScheduled{origin: integration}` |
      | task の review | `RequiredChecksEvaluated.results[]` の `cmd = celeris:repo-health/1` の `health` | `RepairScheduled{origin: review}` と `Transitioned{reason: review_fail}`（atomic の task の review の不合格の再実行） |
    - 判定: 最後の `is_human_resume` の後の events だけを読む。repo ごと（`HealthEvidence.repo_id`）に、今の評価（その repo の最後の証跡）が Fail で集合が空でなく、(1) 最後の修復の前の測れた評価と同じ集合が残る、または (2) 前のどれかの修復で消えた fingerprint（修復の直前の評価にあり、直後の評価に無い）が今の集合にある、なら Repeat。往復した repo の今の集合を合わせ、昇順・重複なしにして、改行でつないだ byte 列の sha256 の hex を `set_sha256` にする。
    - Error・NotApplicable の行は判定の外: 今の評価なら Proceed、前の評価としては読み飛ばす（測れなかった行で fingerprint が消えたと数えない）。PolicyOnly は集合を持つ評価として数え、repair に回らないので今の評価なら Proceed。
  - 新しい状態・config・fallback・互換分岐は無い。`crates/task-dispatch/src/dispatcher.rs` は変えていない（2784 行のまま。check-health の source-size は pass）。

## 判断
- `stalled_replans` と別の仕様にした。`stalled_replans` は同じ `(key, 失敗の署名)` の自動 replan が、変更のある commit を挟まずに続く回数を数える。health の修復 WU は必ず変更を commit するので、その進展の判定では往復を検出できない。ここでは修復の前後の blocking の fingerprint の集合を比べる。
- 「前の修復で消えた」は、最後の修復だけでなく連なりの中のどの修復も見る。最後の修復だけを見ると a→b→a の往復（1 回目の修復で a が消えて b が出て、2 回目で b が消えて a が戻る）を検出できない（試験 `health_limits_repeat_vanished_fingerprint_reappears`）。
- 「同じ集合が残る」は集合の一致にした。直らずに新しい指摘が増えた場合（{a} → {a, b}）は Proceed で、修復の上限（D4.1 の `max_repairs`・`max_repairs_per_class`）で止まる。
- review は repo ごとに証跡を残すので、repo ごとに比べる。片方の repo の Error で他方の fingerprint が消えたと数えないため。
- 段の修復は origin が integration の `RepairScheduled` をすべて数える。修復の event は段の key を持たず（repair WU の key の接頭辞は段の検査の修復と merge の衝突の修復で違う）、段の障壁があるので、同じ段の 2 つの評価の間に別の段の統合の修復は入らない。

## 契約の変更（`stalled_replans`）
- 2026-11-09 から、進展の無い自動 replan の連なりは、人の replan の依頼・回答に加えて、人の comment（running・reviewing の task への comment の `Transitioned{reason: comment}`）と reopen（`Transitioned{reason: reopen}`）でも切れる。人が手を入れた後の自動 replan は新しい連なりとして 1 から数える。
- 既存の試験との食い違いは無かった。`crates/task-dispatch/src/dispatcher/tests/stalled_replan.rs` の 3 件と `plan_gate::stall` の既存 6 件は comment・reopen を含まず、変更の後も合格した。
- 足した試験 `a_human_comment_or_reopen_resets_the_count` は、comment・reopen の後の数が 0 になること、再開の後の同じ失敗が 1 から数え直されること、人でない遷移（`review_fail`）では切れないことを確かめる。既存の `a_human_replan_request_resets_the_count` は replan の依頼だけを見るので、comment・reopen の抜けを検出できない。

## 試験（守る不変条件。`crates/task-dispatch/src/health_review/limits_tests.rs`）
events を組んで渡すだけで、時計・I/O・検査器・LLM を使わない（CPU を焼く負荷も無い）。既存の `plan_gate::stall` の試験は `(key, 失敗の署名)` と commit の有無だけを見るので、fingerprint の集合の往復を検出できない。
- `health_limits_repeat_same_set_after_repair_repeats`: 3 つの範囲で、修復の後も同じ集合が残れば Repeat。review では `RepairScheduled{origin: review}` と `review_fail` の再実行のどちらも修復として数える。
- `health_limits_repeat_vanished_fingerprint_reappears`: a→b→{a, c} は Repeat（上げる集合は今の集合全体）。修復を挟まずに消えた fingerprint の再出現は Proceed。
- `health_limits_repeat_different_set_proceeds`: 別の集合・一部が直った・増えただけ・今が Pass は Proceed。
- `health_limits_repeat_first_evaluation_proceeds`: events が空・初回・修復を挟まない同じ集合の再評価・今の評価の後に積まれた retry は Proceed。
- `health_limits_repeat_set_sha_does_not_depend_on_order`: blocking の順と重複に依らず同じ `fingerprints` と `set_sha256`。値は `sha256("fp-a\nfp-b\nfp-c")`。
- `health_limits_repeat_human_resume_resets`: Repeat の後の comment・answer・reopen の直後と、その後の最初の評価は Proceed。再開の後にまた修復して同じ集合が残れば、新しい連なりの Repeat。`review_fail` の遷移では切れない。
- `health_limits_repeat_error_rows_are_outside`: 今の評価が Error なら Proceed。前の Error を Pass と扱わない（{a} → Error → {a, c} は Proceed）。Error・NotApplicable を挟んでも同じ集合が残れば Repeat。今の評価が PolicyOnly なら Proceed。
- `health_limits_repeat_scopes_and_repos_are_separate`: 別の WU の retry・別の範囲の評価・origin の違う修復を混ぜない。review は repo ごとに比べ、往復した repo の集合だけを上げる。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/health_limits_repeat_/)'` → exit 0、8 tests run: 8 passed。
- `cargo nextest run -p task-ops --no-tests=fail -E 'test(/plan_gate::stall::/)'` → exit 0、7 tests run: 7 passed（`a_human_comment_or_reopen_resets_the_count` を含む）。
- `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/stalled_replan/)'` → exit 0、3 tests run: 3 passed。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-campaign・celeris-mcp・celerisctl・llm-proxy・task-api・task-dispatch・task-ops・task-worker。nextest は 6520 tests run: 6520 passed、23 skipped。doc test は全部 ok。web は task-ops の変更で型検査（tsc -b）と unit 試験を走らせ、vitest は 104 files・711 tests passed。check-health は `repo_health_gate: all checks passed`（source-size pass、required findings=0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` と `git diff --name-only "$CELERIS_WU_BASE"..HEAD` → 変えた path は `crates/task-dispatch/src/health_review.rs`・`health_review/limits.rs`・`health_review/limits_tests.rs`・`crates/task-ops/src/plan_gate.rs`・`plan_gate/stall.rs` と、この記録だけ。

## 未解決
- 配線は段 wire（integration-wire・review-wire）が持つ。呼ぶ時点は「今の評価が Fail で、repair を起こす前」を想定している。今の評価の後に積まれた修復の event（WU の `WorkUnitChecksFailed` は `WorkerFinished` と同じトランザクション）は判定を変えないので、その event の前でも後でも呼べる。
- WU の check の範囲は、同じ key の `WorkUnitChecksFailed` を修復とみる。health でない check の不合格による retry も修復として数えるが、その retry の後も health の集合が同じなら往復なので、判定は変わらない。
- 段の範囲は、replan で同じ段の統合が作り直されると、間の別の段の統合の修復も修復として数える。その後も同じ集合が残るなら往復とみてよいので、今は直さない。

## 提案
- 無し。
