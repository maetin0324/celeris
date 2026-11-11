---
title: 健全性v2 G4-R1 fix-review-gap — review 方針の無い汎用 repo の候補を final reviewer に渡す
tasks: [01M74X0XAN0MCFJ4KZ3VVFT2QA]
status: running
updated: 2026-11-11
---
# 健全性v2 G4-R1 fix-review-gap — review 方針の無い汎用 repo の候補を final reviewer に渡す

完了日: （未完了。unit record が書く）

task 01M74X0XAN0MCFJ4KZ3VVFT2QA の進捗。工程は contract → input-owner → request-atomic → request-integrated → record。基準は WorkUnit では `$CELERIS_WU_BASE`、task では `git merge-base HEAD main`。

## 工程 1: contract（Absent の final review 入力契約を改定する）
コードは変えていない。実装の前に契約と実装前 brief を ADR に書いた。

### 変更
- `phase 1: G4-R1 の契約を ADR に書く` — 次の 3 file。
  - 新 ADR [2026-11-11-health-generic-review-input](../../adr/2026-11-11-health-generic-review-input.md): 実装前 brief（7 項目）、R1 Absent の材料（`present` と同じ構築、`owner_rows`・`invariants` だけ空、I7 が及ぶ）、R2 再利用の鍵（4 つ目は選んだ検査器の `definition_digest(&[], Some(&checker))`、base の profile は `target_sha` で束縛）と変更時の失効の表、R3 変えない契約、R4 撤去の表、R5 不変条件と試験、R6 計画の範囲外の path、本番への移行。
  - [ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md): 状態の行、D4.4 と付記「policy の関門の fail closed」の `Absent` の項に改定の注記、末尾に付記「2026-11-11、G4-R1 の改定」。
  - [ADR 2026-11-10 health-generic-fixtures](../../adr/2026-11-10-health-generic-fixtures.md): 付記 (e) に訂正、状態の記録に 1 行。

### 再利用の鍵と失効（受け入れ条件 1 の要約）
鍵は `(repo_id, target_sha, reviewed_sha, definition_digest(&[], Some(&checker)))`。`present`・`absent` で同じ。
- candidate が変わる → `reviewed_sha` が変わり作り直す。
- base が変わる・base の profile が変わる → `target_sha` が変わり作り直す。
- 検査器が変わる（base の検査器、同梱の検査器、組み込み generic の中身） → `checker.sha256` が変わり作り直す。
- 候補が profile・baseline・検査器を変える → `reviewed_sha` で作り直す。値は base の版から作り、`policy.changed = true` で人の承認（I8）。
- policy の状態が不明の材料は再利用しない（今のまま）。

### 証拠（工程 1 の提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
変えたのは文書だけなので、`test-affected.sh` は軽い経路（docs のみ）を通った。Rust の試験は走っていない（対象の crate が無い）。
- `bash scripts/dev/test-affected.sh --list` → exit 0。対象の crate・package は 0 件。
- `bash scripts/dev/test-affected.sh` → exit 0。「試験対象の変更なし（docs のみ等）」。repo_health_gate の試験 7 件は OK。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。差分なし。
- `bash scripts/dev/check-health.sh --base $CELERIS_WU_BASE` → exit 0。verdict pass、mode doc-only、fail 級 0、new 0・worsened 0。`policy.changed` は pass。
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）。
- `bash scripts/dev/check-adr-numbers.sh` → exit 0（420 file）。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0。
- `bash scripts/dev/check-doc-links.sh` → exit 0。
- `python3 scripts/dev/check-architecture-map.py` → exit 0（999 path）。
- `python3 scripts/dev/check-human-text.py` → exit 0（指摘 0 件）。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は、`agent-docs/adr/` の 3 file とこの進捗の 1 file だけ。検査器・profile・baseline・TaskStore・campaign は変えていない。

## 工程 2: input-owner（共通構築で汎用候補を渡し旧省略経路を撤去する）
状態: 実装と commit は済み。範囲外の試験 12 件が落ちるので、unit の check（`test-affected.sh`・`health_review_policy_`）は不合格。範囲外の file を直す必要がある（ADR の R6 の見込みどおり）。1 回目の run は plan_issue と書いたが done を返したので、celeris が check を走らせて不合格になった。2 回目の run（下の「再確認」）で同じ 12 件を確かめ、`yield.plan_issue` で返した。

### 変更
- `phase 2: Absent の材料を共通構築に合流し旧省略経路を撤去する`
  - `crates/task-dispatch/src/health_review/input.rs`: `HealthInputWork::repo_input` は `Present`・`Absent` を同じ手順で作る（R1）。`Absent` は `owner_map`・`invariants` が無いので `owner_rows`・`invariants` が空。再利用の鍵の 4 つ目は `definition_digest(&[], Some(id))`（`id` は `gate::with_applicable_checker` が選んだ検査器。R2）。report は今までどおり `run_report`（`gate::check_with`）が `evaluated` と共有する。`absent_policy`・`POLICY_ABSENT`・module doc の旧い説明を消した。内側の `Result<Result<_, Skip>, String>` は `Result<_, String>` になった。
  - `crates/task-dispatch/src/health_review/input/tests.rs`: `health_review_input_policy_absent_is_skipped` を消し、次の 4 件に置き換えた。stub の検査器を書く `write_stub` を切り出した。
    - `health_review_input_policy_absent_builds_material_without_policy_values`（Absent の材料・鍵・Built 1 回・report の共有）
    - `health_review_input_reuse_is_bound_to_candidate_base_and_policy`（candidate・base・base の方針（Absent → Present）の変更で作り直し、同じ candidate のやり直しは再利用）
    - `health_review_input_policy_absent_failed_report_is_not_reused`（Absent の report の故障は不明の材料として Built に記録し、再利用しない）
    - `health_review_input_reuse_is_bound_to_the_checker_version`（同梱の検査器の版の変更で作り直し、`Unavailable`・方針を読めない repo は不明の policy で Skipped）
  - `crates/task-dispatch/src/dispatcher/tests/health_review_input.rs`: `health_review_input_policy_absent_is_skipped` を `health_review_input_policy_absent_reaches_the_reviewer_request` に置き換えた（adapter が受けた `ReviewRequest.health` に diff・候補の finding とその path・policy が入り、`owner_rows`・`invariants` が空、Skipped なし）。`built()` の「鍵 = 方針の digest」の assert を外し、`Present`・`Absent` の両方で鍵が base の検査器の `definition_digest` であることを確かめる。
  - `crates/task-dispatch/src/dispatcher/tests/health_generic.rs`: 4 試験 `health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_reaches_reviewer` の `should_panic` を外した。候補の evidence の path の assert は残し、Skipped が無いことと `owner_rows`・`invariants` が空であることの assert を足した。doc comment の G4-R1 の説明を不変条件の説明に替えた。
- 各試験の doc comment に、守る不変条件と既存の試験で検出できない理由を書いた。

### 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(health_review_input_) | test(health_generic_review_input_) | test(health_review_policy_)'` → exit 100。50 件のうち 48 件合格、2 件不合格（下の (A)）。4 件の `…_reaches_reviewer` と新しい 5 件は合格。
- `bash scripts/dev/test-affected.sh` → exit 100。nextest 3747 件のうち 3735 件合格、12 件不合格（下の (A)・(B)）、11 件 skip。repo-health は verdict pass・exit 0・new 0・worsened 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は `expected_write_paths` の 5 file だけ。検査器・profile・baseline・TaskStore・campaign は変えていない。

### 範囲外で落ちる試験（plan_issue の中身）
どれも fail closed は保たれている。落ちる理由は、試験が旧い契約（`Absent` では材料を作らない）を固定している（A）か、偽の reviewer が `review.json` に `health` を書かない（B）かのどちらか。
- (A) 旧い契約を固定する試験（2 file、3 件）
  - `crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs`
    - `health_review_policy_unknown_absent_policy_change_needs_approval`: 「`HealthReviewInputBuilt` を作らない」の assert（R4 の撤去対象）。承認の子は作られている。直し方: `HealthReviewInputBuilt` が 1 件あり、`input.policy.changed = true` であることを assert する。
    - `health_review_policy_unknown_candidates_wait_for_a_human`: `(Absent, Missing)` の場合に、本文に「policy の状態を決められない」（旧い `POLICY_ABSENT` の理由の文）を期待している。承認の子は作られ、本文は「policy state unknown: repo_health.py が report を書かなかった」になる。直し方: 期待する文を「repo_health.py が report を書かなかった」にする。
  - `crates/task-dispatch/src/health_review/scenario_tests.rs` の `health_review_scenario_normal_change`: 「no health without [review]」（`seen == [None]`）を固定している。直し方: `Absent` の材料（`owner_rows`・`invariants` が空）が渡ることを assert する。
- (B) 共通の stub の検査器（`source = absent`）の下で、`health` を書かない偽の reviewer（R1.4・I7 で不合格になる）を使う試験（4 file、9 件）
  - `crates/task-dispatch/src/dispatcher/tests/target_sync.rs`: `integration_repair_resumes_review_on_latest_target`、`reuse_pass_delivery_reaches_merge`、`reuse_pass_records_candidate`、`reviewer_pass_is_reused_when_target_advances_without_overlap`、`reviewer_runs_again_when_target_advances_touching_the_task_files`
  - `crates/task-dispatch/src/dispatcher/tests/health_enforce.rs`: `review_repair_not_evaluated_reviewer_does_not_block_a_health_repair`（log に「health verdict missing or invalid in review.json」）
  - `crates/task-dispatch/src/dispatcher/tests/phase_effect_ab/review_sync.rs`: `phase_effect_ab_review_sync_phase2_integration_repair_keeps_attempts`、`phase_effect_ab_review_sync_pre_review_sync_reduces_runs`
  - `crates/task-dispatch/src/dispatcher/tests/phase_effect_ab/stale_priority.rs`: `phase_effect_ab_stale_priority_syncs_the_long_stale_task_first`
  - 直し方（R6）: 偽の reviewer の `review.json` に `"health":{"findings":[]}` を足す。I7 を弱める、stub の検査器の方針を変えて `Absent` を避ける、は採らない。
- 数え直し: 試験 file は 6 個（(A) 2 個・(B) 4 個）。1 回目の checkpoint の「7 file」は誤り。ほかに doc comment だけの範囲外が 2 個ある（下の「未解決（工程 2）」）。

### 再確認（2 回目の run。提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
コードは変えていない。1 回目の run の check の不合格が範囲外の試験だけによることを確かめた。
- `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(health_review_input_) | test(health_generic_review_input_) | test(health_review_policy_) | test(target_sync::) | test(health_enforce::review_repair_not_evaluated) | test(phase_effect_ab_review_sync_) | test(phase_effect_ab_stale_priority_) | test(health_review_scenario_normal_change)'` → exit 100。86 件のうち 74 件合格、12 件不合格。不合格は上の (A)・(B) の 12 件と一致する。範囲内の試験（`health_review_input_`・`health_generic_review_input_`・4 件の `…_reaches_reviewer`）は全件合格。
  - `target_sync.rs` の偽の reviewer（233 行・1087 行）は `{"verdicts":[…]}` だけを書き、`health` を書かない。`health_enforce.rs` の失敗の log は `health verdict missing or invalid in review.json`（`health_review::verdict::HEALTH_VERDICT_INVALID`）。
  - `scenario_tests.rs:346` は `baseline.final_reviews() == vec![None]`（`Absent` では材料を渡さない旧い契約）を固定している。
  - `unknown_tests.rs:320` は `(Absent, Missing)` の本文に旧い `POLICY_ABSENT` の文「policy の状態を決められない」を、`:421` は `HealthReviewInputBuilt` が無いことを期待している。
- 範囲内だけで通す手（`POLICY_ABSENT` の文を残す、stub の検査器を `present` にする、I7 を弱める）は、ADR の R4・R1 に反する回避策なので採らない。

## 未解決（工程 2）
- 上の 12 件（試験 file 6 個）が `expected_write_paths` の外にある。input-owner の `expected_write_paths` と範囲 check の正規表現に、次の 6 file を足す必要がある（plan_issue）。`crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs`、`crates/task-dispatch/src/health_review/scenario_tests.rs`、`crates/task-dispatch/src/dispatcher/tests/target_sync.rs`、`crates/task-dispatch/src/dispatcher/tests/health_enforce.rs`、`crates/task-dispatch/src/dispatcher/tests/phase_effect_ab/review_sync.rs`、`crates/task-dispatch/src/dispatcher/tests/phase_effect_ab/stale_priority.rs`。別の unit に分けると、input-owner の check（`test-affected.sh`）が先に落ちるので、同じ unit の範囲に足す。
- `task_core::design_review::ReviewPolicy::definition_sha256` の doc comment は「health の材料の再利用の鍵に入る」のまま。鍵には使わなくなった（R2.3）。`crates/task-core/src/model/events.rs` の `HealthReviewInputSkipped` の doc comment（R4）と合わせて範囲外。

## 未解決
- 計画の後続 unit の範囲（`expected_write_paths`）に無いが変える必要のある path がある（ADR の R6）。`crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs` の `health_review_policy_unknown_absent_policy_change_needs_approval` は Absent で `HealthReviewInputBuilt` を作らないことを assert していて、input-owner の変更で落ちる。共通の stub の検査器は `source = absent` を返すので、`health` を書かない偽の reviewer を持つ dispatcher 試験が I7 で落ちる（工程 2 で確認。落ちたのは `target_sync.rs`・`health_enforce.rs`・`phase_effect_ab/review_sync.rs`・`phase_effect_ab/stale_priority.rs` で、契約の工程の候補に挙げた `work_units.rs` は落ちなかった）。`crates/task-core/src/model/events.rs` の doc comment も古くなる。

## 提案
- input-owner の範囲に、「未解決（工程 2）」の試験 file 6 個を足す。直し方は「範囲外で落ちる試験」の (A)・(B) のとおりで、I7・stub の検査器・`POLICY_ABSENT` の撤去は変えない。doc comment だけの `crates/task-core/src/model/events.rs`（`HealthReviewInputSkipped`）と `task_core::design_review::ReviewPolicy::definition_sha256` も同じ unit か record の範囲に足す。
