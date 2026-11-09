---
title: 健全性v2 G3 evidence — 適用判定の関数と、health の結果を verified-candidate の束縛に結び付ける
tasks: [01M6YJF7HJ9J0EQHRM5SN5TWV4]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 evidence — 適用判定の関数と、health の結果を verified-candidate の束縛に結び付ける

完了日: 2026-11-09

task 01M6YJF7HJ9J0EQHRM5SN5TWV4（G3 の段 evidence）の記録。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の (1) 適用判定と (2) 証跡を実装した。各経路への配線は後続の loop-points が行う。基準は `git merge-base HEAD main`。ADR の付記と状態（`状態: 提案`）は G3 の closeout が書くので、この記録では変えていない。

## 各 unit の記録
| unit | 内容 | 記録 |
|---|---|---|
| core-types | 適用判定 `applicability`・事実 `repo_facts`・`HealthEvidence`・指紋（`checker_sha256`・`policy_sha256`）、event の任意欄 `health` と schema・web の生成型の再生成 | [evidence-core-types.md](evidence-core-types.md) |
| bundle-release | release.sh が検査器を `<release>/repo-health/` に同梱し `CHECKER_TREE` を書く | [evidence-bundle-release.md](evidence-bundle-release.md) |
| checker-gate | 実行の口 `health_review::gate`（report の起動を 1 つにし、D3.1 の判定と同梱の検査器） | [evidence-checker-gate.md](evidence-checker-gate.md) |
| core-binding | `definition_digest` v3、`reusable_health`、`verify_required_checks` の `HealthRequirement` | [evidence-core-binding.md](evidence-core-binding.md) |
| bundle-path | daemon が release dir の同梱の検査器を求めて gate に設定する | [evidence-bundle-path.md](evidence-bundle-path.md) |
| delivery-verify | delivery の照合で適用判定を呼び、health の証跡を必須にする | [evidence-delivery-verify.md](evidence-delivery-verify.md) |
| record | この記録と提出時の検査（コードは変えていない） | この file |

## commit（件名）
- 「G3 evidence bundle-release: release に repo health の検査器を同梱し CHECKER_TREE を書く」
- 「G3 evidence core-types: 適用判定・HealthEvidence・event の任意欄 health と schema の再生成」
- 「G3 evidence core-types: health の無い検査結果は PhaseCheckResult::new で作る」
- 「G3 evidence core-types: 進捗に検査の結果を書く」
- 「G3 evidence core-binding: definition_digest v3・reusable_health・delivery の health の照合」
- 「G3 evidence core-binding: 進捗に test-affected の結果を書く」
- 「G3 evidence checker-gate: 実行の口 health_review::gate（report の起動を 1 つにし、D3.1 の判定と同梱の検査器）」
- 「G3 evidence checker-gate: 進捗に試験ごとの不変条件・ADR とのずれ・検査の結果を書く」
- 「G3 evidence bundle-path: daemon が release dir の同梱の検査器を求めて gate に設定する」
- 「G3 evidence delivery-verify: delivery の照合で適用判定を呼び health の証跡を必須にする」
- 段の統合の merge commit「integrate wu/<key> (phase core)」・「integrate wu/<key> (phase wire)」
- 「G3 evidence record: evidence の記録と提出時の検査」（この記録）

## health_evidence_ の試験（51 件）
各試験が守る不変条件と、既存の試験で検出できない理由は各 unit の記録にある。task の受け入れ条件 2（6 件以上）に対し 51 件。

### 束縛の各欄の失効（J2・D2.6）
`crates/task-core/src/required_checks/tests.rs`（core-binding）
- head（候補の tree）: `health_evidence_head_change_invalidates_pass`
- base SHA: `health_evidence_base_change_invalidates_pass`
- policy hash: `health_evidence_policy_change_invalidates_pass`
- checker 版: `health_evidence_checker_change_invalidates_pass`、`health_evidence_definition_digest_changes_with_checker_version`
- repo identity: `health_evidence_repo_change_invalidates_pass`
- report digest: `health_evidence_report_digest_change_invalidates_pass`
- 定義指紋の版: `health_evidence_definition_digest_is_v3_with_checker`、`health_evidence_v2_definition_digest_is_invalid`

delivery の照合（`crates/celeris/src/delivery/health_evidence_tests.rs`、delivery-verify）
- `health_evidence_delivery_rejects_evidence_for_another_tree`（候補 `reviewed_sha` の tree と違う証跡）
- `health_evidence_delivery_rejects_another_checker_version`（今の検査器と違う版の証跡・指紋）
- `health_evidence_delivery_applies_requires_the_health_item`（health 導入前の証跡・項目の欠け・証跡の無い項目）

### 同じ束縛での再利用（D2.5）
- `health_evidence_same_tree_reuses_pass`: 同じ tree の別 commit でも 3 つの正本のどれの合格も写せる。写した証跡も写せる。
- `health_evidence_reusable_passes_never_copies_health`: health の再利用の口は `reusable_health` の 1 つ。
- `health_evidence_phase_integrated_health_ignored`: `PhaseIntegrated.checks[].health` は読まない。
- `health_evidence_gate_pass_binds_each_field`（`crates/task-dispatch/src/health_review/gate_tests.rs`）: 証跡が repo_id・base/head の SHA・head の tree・policy の指紋・検査器・report の sha256・tool.version・reviewable に結び付く。

### error・unknown・fail・policy_only を合格にも再利用にもしない（J4）
- `crates/task-core/src/required_checks/tests.rs`: `health_evidence_error_is_neither_passed_nor_reused`、`health_evidence_unknown_is_neither_passed_nor_reused`、`health_evidence_unsupported_is_neither_passed_nor_reused`、`health_evidence_fail_is_neither_passed_nor_reused`、`health_evidence_policy_only_is_not_passed_without_bound_approval`、`health_evidence_dirty_report_is_not_reused_or_delivered`、`health_evidence_applies_requires_health_item`
- `crates/task-dispatch/src/health_review/gate_tests.rs`: `health_evidence_gate_exit2_is_error`、`health_evidence_gate_missing_or_broken_report_is_error`、`health_evidence_gate_exit1_is_fail_with_sorted_blocking`、`health_evidence_gate_policy_only`、`health_evidence_gate_policy_mixed_is_fail`、`health_evidence_gate_ignores_candidate_checker`
- `crates/celeris/src/delivery/health_evidence_tests.rs`: `health_evidence_delivery_rejects_error_and_fail_evidence`、`health_evidence_delivery_unavailable_is_rejected`
- `crates/task-core/src/required_checks/health_tests.rs`: `health_evidence_no_checker_anywhere_is_checker_missing`、`health_evidence_unreadable_facts_are_unavailable`

### Celeris でない repo（profile 無しで generic）と、適用判定が名前で分岐しないこと（J1）
- `crates/task-core/src/required_checks/health_tests.rs`: `health_evidence_applicability_ignores_project_and_repo_name`、`health_evidence_repo_without_profile_still_applies`、`health_evidence_bundled_checker_applies_when_the_base_has_none`、`health_evidence_base_checker_wins_over_bundled`、`health_evidence_non_git_repo_is_not_applicable`、`health_evidence_remote_workspace_is_not_applicable`
- `crates/task-dispatch/src/health_review/gate_tests.rs`: `health_evidence_gate_no_profile_policy_is_builtin`、`health_evidence_gate_bundled_checker`
- `crates/celeris/src/delivery/health_evidence_tests.rs`: `health_evidence_delivery_non_celeris_repo_passes_with_bundled_pass`、`health_evidence_delivery_prefers_the_base_checker_over_the_bundled_one`、`health_evidence_delivery_prototype_still_requires_health`

### 指紋の式・event の互換・同梱の所在
- `crates/task-core/src/required_checks/health_tests.rs`: `health_evidence_checker_sha256_is_one_formula_for_base_and_bundled`、`health_evidence_policy_sha256_changes_with_each_input`、`health_evidence_old_event_json_without_health_reads`、`health_evidence_round_trips_in_check_events`
- `crates/celeris/src/daemon/repo_health_path_tests.rs`: `health_evidence_bundled_checker_tree_gives_base_formula`、`health_evidence_bundled_missing_or_broken_checker_tree_is_none`、`health_evidence_bundled_without_release_is_none`
- shell の試験 `scripts/selfdeploy/tests/repo_health_bundle.sh`（prefix の外。release の同梱と `CHECKER_TREE`）

## ADR とのずれ
closeout が ADR の付記に写すための一覧。詳しい理由は各 unit の記録の「ADR とのずれ」にある。
1. `CHECKER_TREE` は 2 行（bundle-release）。D1.4 は「その commit の tree id を書く」とする。実装は `repo-health <scripts/dev/repo-health の tree id>` と `source-size-report <scripts/dev/source-size-report.py の blob id>` の 2 行を書く。D2.3 の `checker.sha256` の式の入力をそのまま置き、git の無い release dir から base の版と同じ値を計算するため。写す file は「`profiles/` を除く全部」で、D1.4 の列挙の後に足された `ci.sh` も入る。
2. report digest は再利用の鍵に入れず、同じ鍵の最新の証跡が決める（core-binding）。D2.5 の鍵は `(repo_id, base_sha, head_tree, policy_sha256, checker.sha256)` で report の digest を含まない。同じ鍵で後から別の report の証跡が記録されると、前の Pass は返らない（後が Pass なら後を返す）。delivery の照合は `report_sha256` が空でないことを求める。
3. `PhaseIntegrated.checks[].health` は読まない（core-types・core-binding）。型の上では欄を持つが、段の統合は常に `None` で書く。正本は `RequiredChecksEvaluated.results[].health`・`WorkUnitCheckFinished.health`・`IntegrationCheckFinished.health` の 3 か所（規約 4）。
4. PolicyOnly は検査器の `health.engine.policy_review_files` で判定する（checker-gate）。Rust に判定を書き直さず、同じ検査器の python を `check-health.sh` と同じ呼び方で呼ぶ。変更 path は `git diff --no-renames --name-only -z <base> <head>` で渡す。呼び出しが失敗したら Error（`policy_review_failed`）。delivery は PolicyOnly を通さない。D2.6 の「G2 の `policy_review_gate` の承認があれば通す」は、承認を `(repo, target_sha, reviewed_sha)` に結び付ける event が無く、純粋関数から判定できないため（core-binding の提案 1）。
5. (8) に無い path（B0 の G3 の `write_set` にも無いもの）。
   - `scripts/selfdeploy/lib.sh`: 関数 `sd_bundle_repo_health`。`sd_bundle_selfdeploy_scripts` と同じく lib.sh に置き、release.sh を起こさずに試験から呼べる形にした（bundle-release）。
   - `crates/celeris/src/daemon.rs`: `mod` の行 2 つ（`release_dir`・`repo_health_path`）。(8) の `^crates/celeris/src/daemon/[a-z_]+\.rs$` は module root を含まない（bundle-path）。
   - `crates/task-dispatch/src/health_review.rs`: `pub mod gate;` の 1 行。(8) の `health_review/[a-z_]+` は module root を含まない（checker-gate）。
   - task-api の試験 file: `crates/task-api/src/handlers/task_io.rs`（試験 module の中だけ）・`crates/task-api/src/query/tests.rs`・`crates/task-api/tests/files.rs`。event を struct literal で作る試験で、`health: None` だけを足した（core-types）。
   - (8) には無いが B0 の `write_set` にある path: `crates/celeris/src/delivery.rs` と `delivery/` の試験、`crates/task-dispatch/src/dispatcher/phase_integration.rs`、`dispatcher/tests/` の 5 本、`crates/task-core/src/model_router/feedback/tests.rs`、`crates/task-ops/src/view/tests.rs`。
6. その他の小さなずれ: `HealthRepoFacts` に `unreadable` を足し、`repo_facts` の引数を `(integrated_by_celeris, base_sha, bundled, git)` にした。`HealthEvidence.checker` は `Option`、`reviewable` を足した。event の欄は `Option<Box<HealthEvidence>>`。`run_health_check` の先頭に `repo_id`。`definition_digest` は cmd の数を先に入れる。`HealthRequirement::Applies` は `reviewed_tree` を持つ。delivery は `Unavailable` を「結果の列の error」ではなく再 review の理由として返す。daemon は `CHECKER_TREE` を gate の `read_bundled_checker` で読み、release dir の式を `release_dir.rs` の 1 つにした。

## loop-points への引き継ぎ
### G2 の「base に検査器が無ければ当てない」経路の撤去
- 残っている所: `crates/task-dispatch/src/health_review/input.rs` の 2 か所（G2 の材料と、`policy_review.rs` の関門が読む policy の状態）と、`health_review/policy.rs` の `load_review_policy`。3 か所とも `with_base_checker(.., CheckerSite::Base, ..)` を呼び、base に検査器が無いと `None`（当てない）になる。checker-gate は起動の口を `with_base_checker` の 1 つにしたが、この 3 か所の挙動は変えていない。
- 撤去の条件: loop-points が D1.2 の経路を配線するのと同じ変更で、3 か所を `repo_facts`（`bundled` は `gate::bundled_checker().map(|b| &b.id)`）→ `applicability` → gate の検査器の選び方（`CheckerSource` から `CheckerSite` への写し。今は `gate.rs` の中）に付け替える。`Unavailable` は当てないのではなく合格にしない扱いにする。付け替えた後は `grep -n 'CheckerSite::Base' crates/task-dispatch/src/health_review/input.rs crates/task-dispatch/src/health_review/policy.rs` が 0 件になる（gate の中の写しだけが残る）。
- 撤去の責任者: G3（task 01M6TYVJSWEY3ZE50S02ETYDNM）の段 enforce の unit loop-points。closeout が上の grep で撤去を確かめる。

### 各経路の配線（ADR D1.2）
今の状態と、loop-points がすることを経路ごとに書く。delivery 以外は health の項目をまだ作らない。
1. atomic/direct の task の review: `review/required.rs` の `evaluate` は `PhaseCheckResult::new`（health なし）で結果を作り、指紋は `definition_digest(cmds, None)`。loop-points は適用判定を呼び、`Applies` なら `run_health_check` の証跡を `HEALTH_CHECK_ID` の項目として足し、指紋を `definition_digest(cmds, Some(&checker))` にする。`NotApplicable`・`Unavailable` の理由は `HealthApplicability::reason()` で写す。
2. WU の check: `dispatcher/phase_integration.rs` の `WorkUnitCheckFinished` は `health: None`。base は `wu.base_commit`、head は WU の HEAD で `run_health_check` を呼ぶ。dirty な作業ツリーの report（`reviewable == false`）は WU の合否には使うが再利用・delivery には使わない（D2.7）。
3. 段の統合: 同じ file の `IntegrationCheckFinished` は `health: None`。base は段の基点、head は統合の HEAD。`PhaseIntegrated.checks[]` は `None` のまま（ずれ 3）。
4. review の再利用: `dispatcher/review_spawn.rs` の `reviewer_reuse` は `reusable_passes`（`definition_digest(required, None)` で比べ、health の項目を返さない）だけを使う。`reusable_passes` に health の要求を渡す引数を足し、health は `reusable_health` で引く。返った証跡を今の `(base_sha, head_sha)` に束縛し直し `reused_from` を書くのは呼び出し側（core-binding の未解決）。
5. review 前の main 同期・rebase: 同期の後の `(target_sha, reviewed_sha)` で評価し直す。同期で head の tree が変われば `reusable_health` は前の証跡を返さない。
6. 統合の修復の後: 修復の後は 1 か 3 の経路に戻り、新しい head で評価する。
7. delivery: 配線済み（delivery-verify）。`tick` が `bundled_checker()` を 1 回読み、`check_required_evidence` が `target_sha` の事実で適用判定を呼ぶ。health の証跡の `base_sha` を `target_sha` と比べるかは配線が決める（core-binding のずれ 6）。

### そのほかの引き継ぎ
- 今は review が health の項目を記録しないので、delivery-verify の後は Git 管理のどの repo の配送も health の証跡が無く再 review に戻る。G3 の branch を main に取り込むのは loop-points の後にする（delivery-verify の未解決）。
- delivery-verify の提案: loop-points の試験 `health_gate_path_delivery` で、review が記録した証跡がそのまま `check_required_evidence` を通ることを 1 本で確かめる（今の delivery の試験は証跡を手で作る）。
- checker-gate の提案: `input.rs` の report を `run_health_check` の report と共有し、同じ candidate で検査器を 2 回起こさない。
- PolicyOnly を delivery で通すなら、G2 の承認（`Approval` の子 task の done）を `(repo, target_sha, reviewed_sha)` に結び付けて親の task の events に残す。方式は loop-points の設計で決め、ADR の付記に書く（core-binding の提案 1）。
- `dup.contract` の warning: task-core の `UnavailableReason` が `route_select/availability.rs` の型と同名。合否には使わない。名前を分けるかは closeout か loop-points で決める。
- closeout: `daemon.rs` の module の表に `release_dir`・`repo_health_path` の行を足す（bundle-path の提案）。

## 本番への移行
- 必須検査の証跡の定義指紋を v3（domain `celeris.required-checks/3`）に上げたので、この変更を含む release を昇格した時点で、`MergeQueued` の配送は手持ちの証跡が一致しなくなり 1 回だけ再 review になる（ADR の「本番への移行」と ADR 2026-10-23 の移行と同じ）。人の操作は要らない。
- 同梱の検査器は、bundle-release と bundle-path を含む最初の release から `<release>/repo-health/` に入る。それより前の release の daemon は、base に検査器の無い repo を `Unavailable(checker_missing)` にする。evidence と loop-points は同じ release に入れる（G3 を main に取り込むのは loop-points の後。release は main の HEAD から作る）。
- 昇格の後の確認（人が行う）: daemon の起動の log に `repo health: bundled checker` の行（dir と `checker_sha256`）が出ることを確かめる。出ない・warn が出るときは、`<release>/repo-health/CHECKER_TREE` があるかを確かめる。
- config・DB の表・migration の変更は無い。event には任意欄を足しただけで、旧い event はそのまま読める。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
この unit はコードを変えていない（変えた path はこの file だけ。`sh "$CELERIS_WU_SCOPE_PATHS"` の出力もこの file の 1 行）。下の検査は「G3 evidence record: evidence の記録と提出時の検査」の commit で実行した。その後の commit は、この節を書いた差分だけ。
- `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh` → exit 0（607 秒）。基準は `git merge-base HEAD main` で、task 全体の変更が対象。
  - 対象: crate 14 個（celeris・celeris-campaign・celeris-credentiald・celeris-mcp・celeris-script・celerisctl・e2e・llm-proxy・routing-eval・task-api・task-core・task-dispatch・task-ops・task-worker）、web の型検査と unit 試験、test-ts-verify。
  - check-health: `verdict=pass exit=0 reviewable=false findings=718 fail_grade=0 new=1 worsened=0 existing=717`。new の 1 件は warning の `dup.contract`（`required_checks/health.rs` の `UnavailableReason` と `route_select/availability.rs` の同名の型。core-types の変更で、合否には使わない。「loop-points への引き継ぎ」に書いた）。必須 rule はすべて pass、`policy.changed` も pass。
  - `repo_health_gate: all checks passed`（ci.sh の場合を含む 7 項目）。
  - nextest: `8032 tests run: 8032 passed (10 slow), 23 skipped`。web: `Test Files 104 passed`・`Tests 711 passed`。`test-ts-verify: 20 passed, 0 failed`。
- `cargo nextest run -p task-core -p task-dispatch -p celeris -E 'test(/health_evidence_/)'` → exit 0、`51 tests run: 51 passed`。
- `cargo clippy --workspace -- -D warnings` → exit 0（vendored `deno_core` の既存の warning 1 件だけ）。
- `cargo fmt --all -- --check` → exit 0。
- 文書の検査 3 本: `bash scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → `check-doc-layout: ok`、`sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`（3 本とも exit 0。この節を書いた後の HEAD でも再実行した）。

## 未解決
- 無し（経路への配線は loop-points の範囲）。

## 提案
- bundle-release の提案を引き継ぐ: `scripts/selfdeploy/tests/*.sh` の release を通す試験は、どの検査からも呼ばれていない。release gate か test-affected.sh（`scripts/selfdeploy/` が変わったとき）から呼ぶ。
