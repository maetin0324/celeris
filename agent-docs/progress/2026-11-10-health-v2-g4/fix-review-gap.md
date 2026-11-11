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
状態: 完了。実装と、範囲外で落ちた試験 12 件（試験 file 6 個）の直しを commit した。unit の check はすべて exit 0（下の「範囲外の試験の直し」）。経緯: 1 回目の run は範囲外の 12 件が落ちたまま done を返し、check が不合格になった。2 回目の run は同じ 12 件を確かめて `yield.plan_issue` で返した。replan で input-owner の範囲に 6 file が入り、3 回目の run で直した。

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

### 範囲外の試験の直し（3 回目の run。replan 後）
- `phase 2: 旧い契約を固定する試験 3 件を新しい契約に替え、偽の reviewer 9 件に health を書かせる` — 次の 6 file。どれも試験だけで、本体の code・I7（`health_review/verdict.rs`）・stub の検査器・`POLICY_ABSENT` の撤去は変えていない。
  - (A) `crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs`
    - `health_review_policy_unknown_absent_policy_change_needs_approval`: 「`HealthReviewInputBuilt` を作らない」の assert を、`HealthReviewInputBuilt` が 1 件あり `input.policy.changed = true`、`owner_rows`・`invariants` が空であることの assert に替えた。承認の子の assert（本文の `policy files:`）は残した。
    - `health_review_policy_unknown_candidates_wait_for_a_human`: `(Absent, Missing)` の期待文を「repo_health.py が report を書かなかった」にした。不明の理由（`could not be determined`・`policy state unknown: `・candidate の SHA）と、同じ candidate で子を作り直さない assert は変えていない。
  - (A) `crates/task-dispatch/src/health_review/scenario_tests.rs` の `health_review_scenario_normal_change`: `baseline.final_reviews() == vec![None]` を、`[review]` の無い repo の final review が受けた `ReviewRequest.health` に diff（変更 path）・findings（new 1 件）・policy（`source = base`・`changed = false`）が入り、`owner_rows`・`invariants` が空で、task が Done になることの assert に替えた。
  - (B) 偽の reviewer の `review.json` に `"health":{"findings":[]}` を足した（9 件）。`dispatcher/tests/target_sync.rs` の `ReviewShaAdapter` と `CountingReviewer`（5 件）、`dispatcher/tests/health_enforce.rs` の `review_repair_not_evaluated_reviewer_does_not_block_a_health_repair`（1 件）、`dispatcher/tests/phase_effect_ab/review_sync.rs` の `SyncScenarioAdapter`（2 件）、`dispatcher/tests/phase_effect_ab/stale_priority.rs` の `ReviewAdapter`（1 件）。
- 試験ごとの不変条件と変えた理由は doc comment・注釈に書いた。(A) は旧い契約（absent では材料を作らない）を ADR 2026-11-11-health-generic-review-input の R4 で撤去したので、新しい契約（R1）の中身を確かめる形に替えた。守る不変条件（I8・D6.4 の fail closed、`policy.changed` の承認）は変えていない。(B) は I7（ADR 2026-11-07-health-review-integration D5.2）が health の材料を受けた review に health の判定を求め、R1 で absent の repo も材料を受けるようになったので、偽の reviewer も空の判定を書く（R6）。各試験が守る不変条件（target の同期・reviewer の合格の再利用・health の repair・review の同期の回数・stale の優先順）は変えていない。
- task の `expected_write_paths` の外の 5 file（`dispatcher/tests/health_review_input.rs`・`target_sync.rs`・`health_enforce.rs`・`phase_effect_ab/review_sync.rs`・`phase_effect_ab/stale_priority.rs`）を変えた理由: task の受け入れ条件 2（提出時の affected tests の合格）を満たすには、`test-affected.sh` が走らせるこれらの試験を新しい契約に合わせる必要がある（ADR の R6 の見込みどおり）。`health_review_input.rs` は工程 2 の実装の commit で旧試験を置き換えた file（計画の input-owner の範囲）、残りの 4 file は (B) の偽の reviewer の file。

### 証拠（3 回目の run。提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(health_review_input_) | test(health_generic_review_input_) | test(health_review_policy_) | test(health_review_scenario_) | test(target_sync::) | test(health_enforce::) | test(phase_effect_ab_)'` → exit 0。103 件すべて合格。
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 3747 件すべて合格、11 件 skip。repo-health は verdict pass・exit 0・new 0・worsened 0。repo_health_gate の試験 7 件は OK。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。差分なし。
- `! git grep -n -e POLICY_ABSENT -e 'fn absent_policy' -- crates/`、`health_generic.rs` に `#[should_panic` が無く `_reaches_reviewer()` が 4 件、`git diff --quiet $(git merge-base HEAD main) -- …/verdict.rs …/verdict …/stub_checker.rs …/stub_checker crates/task-core/ crates/campaign/ scripts/dev/repo-health/ scripts/dev/check-health.sh scripts/dev/source-size-baseline.json`（計画の check 3）→ exit 0。
- 範囲（計画の check 4）→ exit 0。変更 path は input-owner の `expected_write_paths` の 11 file の中だけ。検査器・profile・baseline・TaskStore・campaign は変えていない。
- `sh scripts/dev/progress-index.sh --check` → exit 0。

### timeout 後の再検証（今回の提出時の HEAD。基準は `$CELERIS_WU_BASE`）
- 前回の daemon check は `bash scripts/dev/test-affected.sh` が 1200 秒で timeout した。今回、同じ cwd・同じコマンドを再実行し exit 0 で完走した。直接原因は未特定で、timeout は再現していない。health 本体と検査器自身の試験にも時間がかかり、Rust の試験開始まで約 10 分を要した。
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 3747 件すべて合格（321.267 秒）、11 件 skip。対象 package の doc test も合格。health は verdict pass、fail 級 0・new 0・worsened 0。検査器自身の 94 試験は OK（expected failures 2 件）、repo_health_gate は 7 件すべて OK。
- 偽 reviewer を使う 9 試験それぞれの doc comment に、守る同期・再利用・修復の不変条件と、Absent の材料を受ける偽 reviewer にも I7 の health 判定が要るため変更したこと（ADR R1・R6）を追記した。動作・assert は変えていない。
- `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(health_review_input_) | test(health_generic_review_input_) | test(health_review_policy_) | test(health_review_scenario_) | test(target_sync::) | test(health_enforce::) | test(phase_effect_ab_)'` → exit 0。103 件すべて合格（24.394 秒）。
- `cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`、`sh scripts/dev/progress-index.sh --check`、`git diff --check` → いずれも exit 0。
- 撤去・禁止対象無変更の check → exit 0。`crates/` に旧定数・旧関数なし、4 件の通常試験を維持、verdict・stub 検査器・task-core・campaign・検査器・baseline は無変更。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0。出力の 11 path は許可範囲内。
- commit 件名: `phase 2: 偽 reviewer の試験ごとに不変条件と修正理由を記録する`。

### 再提出の検証（run の時間切れ後。提出時の HEAD で実行）
基準は `$CELERIS_WU_BASE`。既存の修正 12 件と試験ごとの doc comment は要求を満たしていたため、今回のコード変更は無し。前回の check の 1200 秒 timeout に対し、同じ cwd で同じコマンドを再実行した。timeout の原因は未特定で、今回は再現しなかった。
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 3747 件すべて合格（319.679 秒）、11 件 skip。対象 package の doc test も合格。health は verdict pass・fail 級 0・new 0・worsened 0。検査器自身の 94 試験は OK（expected failures 2 件）、repo_health_gate は 7 件すべて OK。
- `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(health_review_input_) | test(health_generic_review_input_) | test(health_review_policy_) | test(health_review_scenario_) | test(target_sync::) | test(health_enforce::) | test(phase_effect_ab_)'` → exit 0。103 件すべて合格（30.027 秒）。Absent の材料・旧契約の撤去・policy gate と report 故障の回帰・偽 reviewer の修正を確認した。
- `cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check` → exit 0。
- 計画の check 3（旧定数・旧関数なし、4 件の通常試験、禁止対象の差分なし）と check 4（許可された 11 path の範囲）→ exit 0。I7・stub 検査器・task-core・campaign・profile・baseline は変えていない。
- `sh scripts/dev/progress-index.sh --check`、`git diff --check` → exit 0。
- commit 件名: `phase 2: 時間切れ後の affected tests 再検証を記録する`。

## 工程 3: request-atomic（adapter 受信内容と atomic review の合否を固定する）
状態: 完了。変えたのは `crates/task-dispatch/src/dispatcher/tests/health_generic.rs`（試験だけ）とこの進捗。本体の code・検査器・profile・baseline・TaskStore・campaign は変えていない。

### 変更
- `phase 3: adapter が受けた ReviewRequest.health と atomic の final review の合否を固定する`
  - 既存の `review_flow`・`configure_real_bundled_checker`・`build_fixture` を使い回した。`ReviewSetup.reviewer` を `bool` から `Reviewer`（`None`・`Pass`・`Candidates`）に替え、`without_profile` を足した。既存の試験の動きは変えていない。
  - `build_fixture_without_profile`: builder の CLI は profile の有無を選べないので、同じ builder の `build(..., with_profile=False)` を `python3 -I -c` で呼ぶ。fixture の内容は写していない。builder の出力の読み取りは `run_builder` に寄せた（`build_fixture` と共用）。
  - `CandidateReviewer`（試験の中だけの adapter。本体に hook を足していない）: reviewer run が受けた `req.context.review.health` を記録する。受けた `findings.new` のうち `dup.contract` には `duplicate_implementation`、`dup.state` には `dual_source_of_truth` の blocking の finding を書く（path は候補の path、reason は候補の fingerprint を引く）。候補が無ければ空の health の判定で合格にする。fixture と作業ツリーは読まない。
  - 試験 9 件（どれも通常の成功試験。`should_panic` は 0 件）。
    - `g4_r1_review_request_{ts,rust}_{dup_contract,dup_state}_fails_review`（4 件）と `g4_r1_review_request_builtin_generic_{dup_contract,dup_state}_fails_review`（profile の無い ts の dup-contract と rust の dup-state。2 件）。adapter が受けた材料について次を確かめる: repo_id、`target_sha` = fixture の base、`reviewed_sha` = 候補、diff の path が候補の変更 file の全部と一致、`owner_rows`・`invariants` が空、`policy.source` = `base`（profile あり）か `builtin`（profile 無し）、`changed = false`、report の verdict `pass` で `reviewable`、fixture の期待の rule の候補 1 件（warning・new、path は候補の変更 file）。同じ候補の `HealthReviewInputBuilt` の `input` は受けた材料と一致する（event は照合だけに使う）。さらに、reviewer の blocking の不合格（`health:<kind>`、根拠あり）で review が落ち、検査器の health の項目は合格のまま、`review_pass` の遷移は無く、reviewer run は 1 回で、task は Failed になる（`max_retries = 0`）。
    - `g4_r1_review_request_{ts,rust}_good_is_done` と `g4_r1_review_request_builtin_generic_good_is_done`（3 件）。同じ材料の検査に加え、受けた材料に二重化の候補（`dup.contract`・`dup.state`）が無い。reviewer run は 1 回で、不合格の verdict は無く、`review_pass` で Done になる。
  - 試験ごとの不変条件と既存の試験で足りない理由は doc comment と節の注釈に書いた。要点: 既存の `…_reaches_reviewer` は `HealthReviewInputBuilt` の event だけを見る。材料が reviewer の run の `RunContext` に載ったこと（配送）と、受けた候補による reviewer の不合格が task の状態を止めることは確かめない。`health_review_input_policy_absent_reaches_the_reviewer_request` は stub の検査器の固定の report で、本物の検査器の候補と組み込み generic を通らない。

### 変異の確認（commit していない一時の変更）
- `CandidateReviewer` が受けた材料を無視する（`review_json(None)`）ように一時的に変えて流した。候補の 6 件は「the reviewer's blocking health:… must fail the review」で落ち、good の 3 件は合格した（9 件中 6 件不合格）。材料が reviewer に届かない不具合を、この試験が検出できることを確かめた。変更は元に戻した。

### 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(g4_r1_review_request_) | test(health_generic_review_input_)'` → exit 0。17 件すべて合格（新しい 9 件と既存の 8 件）。
- `bash scripts/dev/test-affected.sh` → exit 0（836 秒）。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。nextest 3756 件すべて合格、11 件 skip。health は verdict pass・fail 級 0・new 0・worsened 0。検査器自身の試験は OK（expected failures 2 件）、repo_health_gate は 7 件すべて OK。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 範囲（計画の check 4）→ exit 0。変更 path は `health_generic.rs` とこの進捗だけ。
- 参考（必須の検査ではない）: `cargo clippy -p task-dispatch --tests -- -D warnings` は既存の 4 件（`dispatch_ready_phases.rs`・`health_enforce.rs`・`cos_chat_root_done.rs`・`cos_slot_wait.rs`）で exit 101。`health_generic.rs` の指摘は 0 件。

## 未解決（工程 2）
- 範囲外で落ちた 12 件は直した。残りは注釈だけで、どれも動作に影響しない。
  - `crates/task-core/src/model/events.rs` の `HealthReviewInputSkipped` の doc comment と、`task_core::design_review::ReviewPolicy::definition_sha256` の doc comment（「health の材料の再利用の鍵に入る」。R2.3 で鍵に使わなくなった）が古い。task の受け入れ条件が `crates/task-core/` の無変更を求めるので変えていない。
  - `crates/task-dispatch/src/health_review/verdict.rs` の module doc の「`health` を渡していない review（`[review]` の無い repo、WU の review）」が古い。今は `[review]` の無い repo にも渡す。計画の check 3 が `verdict.rs` の無変更を求めるので変えていない。

## 未解決
- 無し（工程 2 の注釈 3 か所は「提案」に回した）。

## 提案
- 古い注釈 3 か所（`events.rs` の `HealthReviewInputSkipped`、`ReviewPolicy::definition_sha256`、`health_review/verdict.rs` の module doc）を、`crates/task-core/` と `verdict.rs` を変えてよい別の task で直す。どれも説明だけの変更。
