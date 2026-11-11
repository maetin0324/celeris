---
title: 健全性v2 G4-R1 fix-review-gap — review 方針の無い汎用 repo の候補を final reviewer に渡す
tasks: [01M74X0XAN0MCFJ4KZ3VVFT2QA]
status: done
updated: 2026-11-11
---
# 健全性v2 G4-R1 fix-review-gap — review 方針の無い汎用 repo の候補を final reviewer に渡す

完了日: 2026-11-11

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

## 工程 4: request-integrated（統合後 final review・複数 repo・再利用を検証する）
状態: 完了。変えたのは試験 2 file（`crates/task-dispatch/src/dispatcher/tests/health_generic.rs`・`health_review_input.rs`）とこの進捗。本体の code・検査器・profile・baseline・TaskStore・campaign は変えていない。本体の変更は要らなかった（owner 修復の申告は無し）。

### 変更
- `phase 4: WU 統合後の final review・複数 repo・Absent の再利用と policy の関門で adapter が受けた材料と合否を固定する`
  - `health_generic.rs`: 工程 3 の `CandidateReviewer` に、WU の run で fixture の候補を作業ツリーに入れる `apply` と、reviewer でない run が `ReviewRequest.health` を受けたかの記録 `work_runs` を足した（試験の中だけ。本体に hook は足していない）。工程 3 の受信の検査を `delivered`・`assert_repo_input`（repo ごとの repo/base/head/diff/空の owner_rows・invariants/policy/report と、同じ repo・候補の `HealthReviewInputBuilt` との照合）・`assert_candidate_fails_review`・`assert_no_candidate` に分け、atomic・統合後・複数 repo で共用した。`multi_repo_flow` は reviewer の有無を選べる形にした（既存 2 試験の動きは同じ）。
  - `health_review_input.rs`: stub の検査器の本文を `checker_script` に切り出し、候補の file の中身を選べる `review_flow_with` を足した（既存の `review_flow_at` はこれを呼ぶ）。
  - 新しい試験 7 件（どれも通常の成功試験。`should_panic` は 0 件）。不変条件と既存の試験で足りない理由は各試験の doc comment に書いた。
    - `g4_r1_review_request_integrated_ts_dup_contract_fails_review`・`g4_r1_review_request_integrated_builtin_generic_dup_state_fails_review`: 採用済みの計画（1 段 1 WU、check あり）の task で、WU の run が候補を入れ、WU の check（health の証跡あり）と段の統合の後に final review の reviewer が受けた材料を見る。WU の run は `ReviewRequest.health` を受けず、`HealthReviewInputBuilt` は 1 件で final review の reviewer run の後・その run の分。受けた材料は repo・base（= fixture の base）・候補（= 統合後の task の branch の HEAD。fixture の候補を祖先に持つ）・diff（= fixture の候補が変えた path の全部）・空の `owner_rows`/`invariants`・`policy.source`（`base`/`builtin`）を持ち、候補の finding の path は候補が変えた file。受けた候補を根拠にした reviewer の blocking で review は不合格、task は Failed（`max_retries = 0`）。
    - `g4_r1_review_request_integrated_good_is_done`: 同じ流れの良い変更で、材料に二重化の候補は無く、不合格の verdict も無く Done。
    - `g4_r1_review_request_multi_repo_candidate_fails_review`: 2 repo（ts の dup-contract・rust の good）の final review で、reviewer は 2 repo の材料を 1 回の run で受け、repo ごとに repo_id・base・head・diff はその repo の fixture のもの。evidence の path を持つ finding はその repo の候補の tree にある file で、もう一方の repo の変更 path を指さない。dup-contract の候補は ts の材料にだけあり、それによる reviewer の blocking で task 全体は Failed（rust は良くても Done にならない）。
    - `g4_r1_review_request_multi_repo_good_is_done`: builder の multi の良い変更で、2 repo とも候補なしの材料を受けて Done。
    - `g4_r1_review_request_absent_reused_on_reviewer_retry`（`health_review_input.rs`）: 方針が `Absent` の repo で reviewer run が一時的に失敗してやり直しても、2 回の reviewer run が同じ材料を受け、`HealthReviewInputBuilt` は 1 件、材料の report は 1 回だけ走る。
    - `g4_r1_review_request_absent_not_built_when_the_reviewer_pass_is_reused`: 方針が `Absent` の repo で target の進みが候補と重ならず reviewer の合格を再利用した再 review（`ReviewerPassReused`）では材料を作らない（1 回目の candidate の 1 件だけ、reviewer run は 1 回）。
    - `g4_r1_review_request_absent_candidate_policy_is_not_trusted`: 方針が `Absent` の repo で、候補が検査器（`repo_health.py`。方針の出どころ）を `[review]` を持つ版に書き換えても、材料は base の検査器から作り（鍵は base の検査器の版、候補の検査器は 1 度も走らない）、reviewer が受けた `owner_rows`・`invariants` は空。report の `policy.changed = true` の候補は、reviewer が `health.policy` の判定を書かないので review が不合格（「health.policy verdict missing in review.json for the changed policy files」）で Failed になり、Done にならない。
- 観察（試験を作る途中で確かめた事実）: 案件の repo（`task.repos`）を持たない task（`parallel_task` の既定。workspace だけの task）は、final review で `HealthReviewInputBuilt`・`HealthReviewInputSkipped` のどちらも残さず、reviewer は材料を受けなかった。review 前の同期（ADR-0118）と材料が案件の repo ごとだからで、統合後の試験は atomic の試験と同じく案件の repo を持たせた。これが意図した契約かは未確認（下の「提案」）。

### 変異の確認（commit していない一時の変更）
- `CandidateReviewer` が受けた材料を無視する（`review_json(None)`）ように一時的に変えて、統合後と複数 repo の 5 件を流した。候補の 3 件（`…_integrated_ts_dup_contract_…`・`…_integrated_builtin_generic_dup_state_…`・`…_multi_repo_candidate_…`）は「the reviewer's blocking health:… must fail the review」で落ち、good の 2 件は合格した（exit 100、5 件中 3 件不合格）。変更は `git checkout` で戻した。

### 再利用・失効・policy の関門の対応表（受け入れ条件 1）
鍵は `(repo_id, target_sha, reviewed_sha, definition_digest(&[], Some(&checker)))`（ADR 2026-11-11-health-generic-review-input R2）。「新」は工程 4 で足した試験、「単体」は `health_review::{input,policy,policy_review}` の module の試験（dispatcher を通らない）。「—」はその列の専用の試験が無い（鍵は `Present`・`Absent` で同じ）。

| 事象 | 期待する動き | 試験（`Present` の repo） | 試験（`Absent` の repo・汎用 fixture） |
|---|---|---|---|
| 同じ candidate で reviewer run をやり直す | 記録した材料を使い、同じ `ReviewRequest.health` を渡す。report は 1 回 | `health_review_input_reused_on_reviewer_retry`、単体 `health_review_input_built_once_per_candidate` | 新 `g4_r1_review_request_absent_reused_on_reviewer_retry`、単体 `health_review_input_reuse_is_bound_to_candidate_base_and_policy` |
| reviewer の合格を再利用した再 review | 材料を作らない（1 回目の candidate の分だけ） | `health_review_input_not_built_when_the_reviewer_pass_is_reused` | 新 `g4_r1_review_request_absent_not_built_when_the_reviewer_pass_is_reused` |
| candidate が変わる | 作り直す | — | 単体 `health_review_input_reuse_is_bound_to_candidate_base_and_policy` |
| base（target）が変わる | 作り直す | — | 単体 `health_review_input_reuse_is_bound_to_candidate_base_and_policy` |
| base の方針が変わる（`Absent` → `Present`） | 作り直す（base の profile は `target_sha` で束縛） | — | 単体 `health_review_input_reuse_is_bound_to_candidate_base_and_policy` |
| 検査器の版が変わる | 作り直す | — | 単体 `health_review_input_reuse_is_bound_to_the_checker_version` |
| report の故障（policy 不明） | 不明として記録し、再利用しない | 単体 `health_review_input_failed_report_is_recorded_as_unknown` | 単体 `health_review_input_policy_absent_failed_report_is_not_reused` |
| 候補が方針・検査器を書き換える | 方針は base の版から作る（候補の値を信用しない）。`policy.changed` は fail closed | 単体 `health_review_policy_base_side_wins_over_candidate_and_worktree`、単体 `health_review_policy_review_missing_policy_verdict_fails` | 新 `g4_r1_review_request_absent_candidate_policy_is_not_trusted` |
| policy の状態が不明（report 無し・`Unavailable`） | 合格にしない（Blocked・承認待ち・Skipped の `unavailable`） | `health_review_policy_unknown_flow_missing_report_is_not_a_pass` | `health_review_policy_unknown_candidates_wait_for_a_human`、単体 `health_review_input_reuse_is_bound_to_the_checker_version`（`Unavailable`） |
| reviewer 無し（policy-only）・通常の変更 | 材料を作らず reviewer にも渡さない。policy の状態を Skipped に残し、承認なしで Done | `health_review_policy_no_reviewer_normal_change_passes_without_approval` | — |
| reviewer 無し・policy の file の変更 | 承認の子を作り reviewing で待つ。人の承認で Done | `health_review_policy_no_reviewer_policy_file_candidate_needs_a_human` | `health_review_policy_unknown_absent_policy_change_needs_approval` |
| WU の check・WU の run | 材料を作らず、WU の run は `ReviewRequest.health` を受けない。作るのは final review の reviewer run だけ | `health_review_input_not_built_for_work_units`（材料 0 件でも合格する弱い形） | 新 `g4_r1_review_request_integrated_*`（3 件） |
| 複数 repo | repo ごとにその repo の base/head/diff/finding。1 repo の候補の不合格で全体が止まる | — | 新 `g4_r1_review_request_multi_repo_*`（2 件） |

### 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(g4_r1_review_request_) | test(health_review_input_) | test(health_review_policy_) | test(health_enforce_reviewer_reuse_) | test(health_enforce_pre_review_sync_) | test(health_generic_)'`（計画の check 1）→ exit 0。91 件すべて合格（16.6 秒）。新しい 7 件と、上の表の既存の試験を含む。
- `bash scripts/dev/test-affected.sh` → exit 0（836 秒、load average 約 20〜48）。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。nextest 3764 件すべて合格、11 件 skip。対象 package の doc test も合格。health は verdict pass・fail 級 0・new 0・worsened 0。検査器自身の 94 試験は OK（expected failures 2 件）、repo_health_gate は 7 件すべて OK。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- 範囲（計画の check 4）→ exit 0。変更 path は `health_generic.rs`・`health_review_input.rs`・この進捗だけ。
- 実行した HEAD: `test-affected.sh` は commit『phase 4: WU 統合後の final review・複数 repo・Absent の再利用と policy の関門で adapter が受けた材料と合否を固定する』の時点で実行した。その後の commit『phase 4: request-integrated の試験・対応表・検査結果を進捗に記録する』は `health_generic.rs` の doc comment 2 か所（既存の `health_review_input_not_built_for_work_units` の不足の説明を、読んで確かめた事実に直した）とこの進捗だけを変えた。提出時の HEAD で check 1（91 件合格）・clippy・fmt・`progress-index.sh --check`・`git diff --check`・範囲の check を再実行し、すべて exit 0。
- `sh scripts/dev/progress-index.sh --check`・`python3 scripts/dev/check-human-text.py` → exit 0。

## 工程 5: record（最終 HEAD の受け入れ証拠を記録する）
状態: 完了。コードは変えていない。変えたのは [ADR 2026-11-11-health-generic-review-input](../../adr/2026-11-11-health-generic-review-input.md)（状態を「実装済み」に、付記 (a)〜(e)、状態の記録 1 行）とこの進捗だけ。

### 変更
- `phase 5: 提出時の HEAD の受け入れ証拠を記録し ADR を実装済みにする`

### この task の commit（件名。task の最初の commit の親から HEAD まで、merge commit を除く）
- phase 1: G4-R1 の契約を ADR に書く（Absent の材料を共通構築に、再利用の鍵を選んだ検査器に束縛）
- phase 1: 契約の工程の検査結果を進捗に記録する
- phase 2: Absent の材料を共通構築に合流し旧省略経路を撤去する
- phase 2: input-owner の検査結果と範囲外の失敗を進捗に記録する
- phase 2: input-owner の範囲外の失敗 12 件を再確認し plan_issue を進捗に記録する
- phase 2: 旧い契約を固定する試験 3 件を新しい契約に替え、偽の reviewer 9 件に health を書かせる
- phase 2: 範囲外の試験の直しと検査結果を進捗に記録する
- phase 2: 偽 reviewer の試験ごとに不変条件と修正理由を記録する
- phase 2: 時間切れ後の affected tests 再検証を記録する
- phase 3: adapter が受けた ReviewRequest.health と atomic の final review の合否を固定する
- phase 3: request-atomic の試験と検査結果を進捗に記録する
- phase 4: WU 統合後の final review・複数 repo・Absent の再利用と policy の関門で adapter が受けた材料と合否を固定する
- phase 4: request-integrated の試験・対応表・検査結果を進捗に記録する
- phase 4: 検査を実行した HEAD を進捗に書く
- phase 5: 提出時の HEAD の受け入れ証拠を記録し ADR を実装済みにする

### 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- 基準について: この WorkUnit の `$CELERIS_WU_BASE` は開始時の snapshot の commit で、そこからの code の差分は無い。そのため `bash scripts/dev/test-affected.sh` をそのまま走らせると docs だけの軽い経路になる。task 全体の証拠にするため、`CELERIS_WU_BASE=` を空にして基準を `git merge-base HEAD main` にした。対象（`--list`）は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
- 実行した HEAD: 規定 3 検査と試験は、この工程の最初の HEAD（commit『integrate wu/request-integrated (phase request-integrated)』。最後の同期の後）で実行した。この工程の commit が変えたのは ADR とこの進捗の 2 file だけ（code の差分 0）。文書 3 検査・範囲の check・監査の check は、この工程の commit の後の HEAD で再実行した。
- `CELERIS_WU_BASE= bash scripts/dev/test-affected.sh` → exit 0（567 秒。load average の 1 分平均は開始時 12、終了時 14）。nextest 3764 件すべて合格（298.5 秒、slow 14 件）、11 件 skip。対象 package の doc test も合格。health（repo-health）は verdict pass・exit 0・fail 級 0・new 0・worsened 0。検査器自身の 94 試験は OK（expected failures 2 件）、repo_health_gate は 7 件すべて OK、script bundle の SKIP は 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。差分なし。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`（文書 3 検査）→ 下の「文書検査」。

### 受け入れ条件ごとの証拠（task）
| 条件 | 実行したコマンド | 結果 |
|---|---|---|
| 0. 4 件が通常の成功試験になり、adapter が受けた `ReviewRequest` で候補の配送と reviewer の拒否を確かめる。`Absent`・Unknown・再利用・汎用 fixture の回帰が全件合格 | `cargo nextest run -p task-dispatch -E 'test(g4_r1_review_request_) \| test(_reaches_reviewer)'` | exit 0。21 件すべて合格（`g4_r1_review_request_` 17 件と `health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_reaches_reviewer` 4 件） |
| 0. （回帰） | `cargo nextest run -p task-dispatch --no-fail-fast -E 'test(g4_r1_review_request_) \| test(health_generic_) \| test(health_review_input_) \| test(health_review_policy_) \| test(health_review_scenario_) \| test(target_sync::) \| test(health_enforce::) \| test(phase_effect_ab_)'` | exit 0。140 件すべて合格（17.96 秒） |
| 1. `should_panic` で成功を偽装していない | `grep -c should_panic crates/task-dispatch/src/dispatcher/tests/health_generic.rs`、task の最初の commit の親から HEAD までの `git diff -- crates/` の `should_panic` の行 | `health_generic.rs` に 0 件。diff で消えた行 6（属性 `#[should_panic(expected = "no HealthReviewInputBuilt for the candidate")]` 4 件と、それを説明する doc comment 2 行）、足した行 0。`git diff $(git merge-base HEAD main) HEAD -- crates/` にも `should_panic` を足す行は無い |
| 2. 提出時の affected tests と health が合格 | `CELERIS_WU_BASE= bash scripts/dev/test-affected.sh` | exit 0。上の「証拠」のとおり |
| 3. Rust の静的検査と書式 | `cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check` | どちらも exit 0 |
| 4. ADR と進捗に契約の改定・撤去・不変条件・試験証拠があり、検査器・baseline を変更していない | ADR 2026-11-11-health-generic-review-input（R1〜R6、付記）、この進捗の工程 1〜5、下の監査の check | ADR は「実装済み」。監査の check は exit 0 |

### 受け入れ条件ごとの証拠（この WorkUnit）
- 0. 提出時の HEAD の検査結果と受け入れ条件ごとの証拠: 上の「証拠」と表。
- 1. 禁止範囲の無変更と旧経路の撤去（下の「監査」）。
- 2. ADR の状態を「実装済み」にし、付記 (a)〜(e) に実装との食い違いを書いた。範囲外の path は下の「task の `expected_write_paths` の外で変えた path」、task-core の注釈の撤去の条件と責任者は「提案」。

### 監査（禁止範囲の無変更と旧経路の撤去）
- `! git grep -n -e POLICY_ABSENT -e 'fn absent_policy' -- crates/ && git diff --quiet $(git merge-base HEAD main) -- crates/task-dispatch/src/health_review/verdict.rs crates/task-dispatch/src/health_review/verdict crates/task-dispatch/src/health_review/stub_checker.rs crates/task-dispatch/src/health_review/stub_checker crates/task-core/ crates/campaign/ scripts/dev/repo-health/ scripts/dev/check-health.sh scripts/dev/source-size-baseline.json`（計画の check）→ exit 0。旧い定数・関数は無く、I7（`verdict.rs`）・stub の検査器・task-core（TaskStore を含む）・campaign・検査器・baseline に差分は無い。
- `git diff --name-only $(git merge-base HEAD main) HEAD | grep -E 'repo-health|profile|baseline|\.celeris|health/'` → 出力なし。profile・baseline も変えていない。
- 旧い省略の試験: `git grep -n -e policy_absent_is_skipped -e absent_policy -- crates/` の出力は 6 行で、どれも撤去対象ではない。2 行は置き換えた試験の doc comment が旧い試験の名前を挙げて「既存の試験で足りない理由」を書いた行。1 行は `health_review_policy_unknown_absent_policy_change_needs_approval`（新しい契約の assert に替えた既存の試験。`policy_only` の経路）と、それを引く doc comment 1 行。残りの 2 行は risk の判定の試験 `health_review_risk_absent_policy_is_none`（task-core）と `health_review_design_gate_absent_policy_adopts_as_before`（ADR の R1.5 で変えないと決めた設計 review の契約）。
- `should_panic`: 上の表の条件 1。
- task の差分（task の最初の commit の親から HEAD まで）は 14 file。code は `crates/task-dispatch/` の中だけで、`crates/task-core/`・`crates/campaign/`・`scripts/`・`docs/architecture-map.md` は含まない。

### task の `expected_write_paths` の外で変えた path
task の `expected_write_paths` は `crates/task-dispatch/src/health_review/`・`dispatcher/tests/health_generic.rs`・`dispatcher/tests/health_generic_stale.rs`・`dispatcher/tests/mod.rs`・ADR 3 file・`agent-docs/progress/2026-11-10-health-v2-g4/`。`health_review/policy_review/unknown_tests.rs` と `health_review/scenario_tests.rs` は `crates/task-dispatch/src/health_review/` の中にある。外で変えたのは次の 5 file（どれも試験だけ）。
| path（`crates/task-dispatch/src/dispatcher/tests/` の下） | 変えた内容 | 理由 |
|---|---|---|
| `health_review_input.rs` | 旧い試験 `health_review_input_policy_absent_is_skipped` を `health_review_input_policy_absent_reaches_the_reviewer_request` に置き換え、`built()` の鍵の assert を新しい鍵に替えた（工程 2）。`g4_r1_review_request_absent_*` 3 件と、土台の `checker_script`・`review_flow_with` を足した（工程 4） | 旧い試験は ADR R4 の撤去対象で、この file にあった。dispatcher を通る偽の reviewer（`HealthReviewer`）の土台もこの file にある。計画の unit input-owner と request-integrated の範囲には入っていた |
| `target_sync.rs` | 偽の reviewer（`ReviewShaAdapter`・`CountingReviewer`）の `review.json` に `"health":{"findings":[]}` を足した（5 件の試験） | R1 で `Absent` の repo にも材料が渡り、I7 が health の判定を求めるので、health を書かない偽の reviewer の試験が落ちた。task の受け入れ条件 2（affected tests の合格）を満たすため。replan で input-owner の範囲に入れた（ADR の R6・付記 (a)） |
| `health_enforce.rs` | 同上（`review_repair_not_evaluated_reviewer_does_not_block_a_health_repair` の 1 件） | 同上 |
| `phase_effect_ab/review_sync.rs` | 同上（`SyncScenarioAdapter`、2 件） | 同上 |
| `phase_effect_ab/stale_priority.rs` | 同上（`ReviewAdapter`、1 件） | 同上 |
- どの試験も assert と守る不変条件（target の同期・reviewer の合格の再利用・health の repair・review の同期の回数・stale の優先順）は変えていない。I7 を弱める・stub の検査器の方針を変える回避策は採っていない。
- ADR の R6 が挙げた `dispatcher/tests/work_units.rs` は落ちず、変えていない（付記 (a)）。

### 試験の対応表
- 再利用・失効・policy の関門の対応表は工程 4 の「再利用・失効・policy の関門の対応表」。ADR の R5 の不変条件と試験の対応は次のとおり（どれも上の 140 件に入り、合格）。

| R5 の不変条件 | 試験 |
|---|---|
| `Absent` の材料は diff・findings・policy を持ち、owner_rows・invariants は空。Built は 1 回 | 単体 `health_review_input_policy_absent_builds_material_without_policy_values`、dispatcher `health_review_input_policy_absent_reaches_the_reviewer_request` |
| 再利用の鍵と失効、不明の材料は再利用しない | 単体 `health_review_input_reuse_is_bound_to_candidate_base_and_policy`・`health_review_input_reuse_is_bound_to_the_checker_version`・`health_review_input_policy_absent_failed_report_is_not_reused`、`g4_r1_review_request_absent_reused_on_reviewer_retry` |
| adapter が受けた `ReviewRequest.health` に repo・base・head・diff・候補の finding・空の owner_rows/invariants | `g4_r1_review_request_{ts,rust}_{dup_contract,dup_state}_fails_review`・`g4_r1_review_request_builtin_generic_{dup_contract,dup_state}_fails_review`（6 件） |
| 候補を見た reviewer の blocking で Done にならず、good は Done | 上の 6 件と `g4_r1_review_request_{ts,rust,builtin_generic}_good_is_done`（3 件） |
| WU の統合後・複数 repo でも届き、片方の候補で全体が止まる。WU の run は材料を受けない | `g4_r1_review_request_integrated_ts_dup_contract_fails_review`・`g4_r1_review_request_integrated_builtin_generic_dup_state_fails_review`・`g4_r1_review_request_integrated_good_is_done`・`g4_r1_review_request_multi_repo_candidate_fails_review`・`g4_r1_review_request_multi_repo_good_is_done`、既存 `health_review_input_not_built_for_work_units` |
| 4 試験 `…_reaches_reviewer` が通常の成功の試験として通る | `health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_reaches_reviewer`（4 件） |
| Unavailable・report の故障・policy_only・合格の再利用・policy の関門の fail closed は変わらない | `health_review_policy_unknown_*`・`health_review_policy_no_reviewer_*`・`health_review_input_failed_report_is_recorded_as_unknown`・`health_review_input_not_built_when_the_reviewer_pass_is_reused`・`g4_r1_review_request_absent_not_built_when_the_reviewer_pass_is_reused`・`g4_r1_review_request_absent_candidate_policy_is_not_trusted` |

### 文書検査と範囲（この工程の commit の後の HEAD で実行）
commit『phase 5: 提出時の HEAD の受け入れ証拠を記録し ADR を実装済みにする』の後に実行した。その後の commit『phase 5: 文書検査の結果を進捗に書く』はこの節と「未解決」の 1 行だけを変え、同じ検査を再実行した。
- `sh scripts/dev/check-doc-links.sh` → exit 0（ok）。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（ok、420 file）。
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）。
- 参考: `python3 scripts/dev/check-human-text.py` → exit 0（11 file、指摘 0 件）。`python3 scripts/dev/check-architecture-map.py` → exit 0（999 path）。`git diff --check` → exit 0。
- 監査の check（上の「監査」の 1 行目）→ exit 0。
- 範囲（計画の scope の check。`sh "$CELERIS_WU_SCOPE_PATHS"`）→ exit 0。変更 path は ADR 2026-11-11-health-generic-review-input とこの進捗の 2 file だけ。
- WorkUnit の基準（`$CELERIS_WU_BASE`）の `bash scripts/dev/test-affected.sh` → exit 0（125 秒）。「試験対象の変更なし（docs のみ等）」の経路で、repo-health は verdict pass・new 0・worsened 0、検査器自身の 94 試験は OK（expected failures 2 件）、repo_health_gate は 7 件すべて OK。

## 未解決（工程 2）
- 範囲外で落ちた 12 件は直した。残りは注釈だけで、どれも動作に影響しない。
  - `crates/task-core/src/model/events.rs` の `HealthReviewInputSkipped` の doc comment と、`task_core::design_review::ReviewPolicy::definition_sha256` の doc comment（「health の材料の再利用の鍵に入る」。R2.3 で鍵に使わなくなった）が古い。task の受け入れ条件が `crates/task-core/` の無変更を求めるので変えていない。
  - `crates/task-dispatch/src/health_review/verdict.rs` の module doc の「`health` を渡していない review（`[review]` の無い repo、WU の review）」が古い。今は `[review]` の無い repo にも渡す。計画の check 3 が `verdict.rs` の無変更を求めるので変えていない。

## 未解決
- 無し。工程 2 の古い注釈 3 か所は「提案」に回した。この task の受け入れ条件と計画の check は満たしている（工程 5）。
- この WorkUnit の daemon の check の `bash scripts/dev/test-affected.sh` は、基準が `$CELERIS_WU_BASE`（開始時の snapshot）なので docs だけの軽い経路を通る（工程 5 で exit 0 を確かめた）。task 全体の affected tests は、工程 5 で基準を `git merge-base HEAD main` にして実行した（exit 0）。

## 提案
- 案件の repo（`task.repos`）を持たない task（workspace だけの task）の final review は、review 前の同期が無く health の材料も作らない（工程 4 の観察）。`NOT_SYNCED` の Skipped も残らないので、材料が無いことが event に出ない。意図した契約かを ADR 2026-11-07-health-review-integration の D4 の担当（G4 の親 task）が決め、意図どおりなら D4.4 に明記し、そうでなければ別の task で直す。
- 残した古い注釈（どれも説明の文だけで、動作に影響しない）。
  - task-core の doc comment 2 か所: `crates/task-core/src/model/events.rs` の `HealthReviewInputSkipped`（「review 方針が `absent` など」。`Absent` は今は Built になる）と、`crates/task-core/src/design_review.rs` の `ReviewPolicy::definition_sha256`（「health の材料の再利用の鍵に入る」。ADR R2.3 で鍵に使わなくなった）。
    - 撤去の条件: `crates/task-core/` の変更を許す後続 task で直す（この task は受け入れ条件で `crates/task-core/` の無変更を求められていた）。
    - 撤去の責任者: G4 の親 task（この task の親の「[健全性v2 G4] 非 Celeris プロジェクトで汎用性とバイパス耐性を実証」、task 01M6TYVJSXBZMSBC0YGAT2TTX0）。後続 task を起票するか、G4 の後続の unit に入れる。
  - `crates/task-dispatch/src/health_review/verdict.rs` の module doc（「`health` を渡していない review（`[review]` の無い repo、WU の review）」）。撤去の条件は `verdict.rs` の変更を許す後続 task（この task は計画の check で無変更を求められていた）。責任者は同じく G4 の親 task。

## 本番への移行
- 無し。config・DB（migration）・検査器・profile・baseline は変えていない。人が実行する手順は無い。
- 配備の後の動き（ADR 2026-11-11-health-generic-review-input の「本番への移行」と R2.8）: `[review]` の無い profile か組み込み generic の repo で `Check::Reviewer` の条件を持つ task の final review は、reviewer に health の材料を渡し、`review.json` の `health` を求める。本番の events に残る旧い鍵の `HealthReviewInputBuilt` は新しい鍵に一致しないので、review 中の candidate は材料を 1 回だけ作り直す（events は書き換えない）。reviewer の prompt が材料の分だけ長くなる費用の増分は未測定。
