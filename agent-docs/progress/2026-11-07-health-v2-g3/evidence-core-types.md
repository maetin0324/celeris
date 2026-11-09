---
title: 健全性v2 G3 evidence core-types — 適用判定の関数と HealthEvidence の型、event の任意欄 health
tasks: [01M6YJF7HJ9J0EQHRM5SN5TWV4]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 evidence core-types — 適用判定の関数と HealthEvidence の型、event の任意欄 health

完了日: 2026-11-09

WorkUnit `core-types`（task 01M6YJF7HJ9J0EQHRM5SN5TWV4）。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D1.1・D2.1〜D2.3 を task-core に実装した。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。束縛・再利用・照合（D2.4〜D2.7）は後続の unit `core-binding`、起動の口と verdict（D1.3・D3.1）は `checker-gate`、経路への配線は `loop-points` が行う。

## 変更
- 件名「G3 evidence core-types: 適用判定・HealthEvidence・event の任意欄 health と schema の再生成」の commit
  - `crates/task-core/src/required_checks/health.rs`（新規。`required_checks.rs` には `pub mod health;` だけを足した）
    - 定数: `HEALTH_CHECK_ID`（`celeris:repo-health/1`）・`CHECKER_DIR`・`CHECKER_SCRIPT`・`SOURCE_SIZE_REPORT`・`CHECKER_SIDE_FILES`・`PROFILES_DIR`・`MAX_BLOCKING`（50）。検査器の path の正本はここ。`health_review/policy.rs` の同名の定数は unit `checker-gate` が消してこれを使う。
    - 型: `CheckerSource`（`base`・`bundled`）・`CheckerId { source, sha256 }`・`HealthRepoFacts`・`NotApplicableReason`（`not_a_git_repo`・`remote_workspace`）・`UnavailableReason`（`checker_missing`・`facts_unreadable:<理由>`）・`HealthApplicability`（`Applies`・`NotApplicable`・`Unavailable`、`reason()` で証跡に残す文）・`HealthVerdict`（`pass`・`fail`・`error`・`policy_only`・`not_applicable`）・`HealthEvidence`・`PolicyIdentity`。
    - 関数: 純粋関数 `applicability(&HealthRepoFacts)`、事実を集める `repo_facts(integrated_by_celeris, base_sha, bundled, git)`、base に検査器があるかの判定 `base_has_file(base_sha, path, git)` と `base_checker(base_sha, git)`、指紋 `checker_sha256(checker_tree, source_size_report_blob)`・`policy_sha256(&PolicyIdentity)`。`git` は `candidate_required_checks` と同じ形の closure（`FnMut(&[&str]) -> Result<String, String>`）。
    - 適用の順序: remote workspace → 読めない事実 → Git 管理でない → base の検査器 → 同梱の検査器 → どちらも無い（`checker_missing`）。base の有無を確かめられないときは、同梱の検査器があっても `Unavailable` にする（base を優先する J3 を守れないため）。
    - 入力に案件名・repo 名・`self_improve`・selfdeploy・task の mode は無い。`repo_facts` が git に渡すのは base の sha と固定の検査器の path だけ（試験で確かめる）。
  - `crates/task-core/src/model/events.rs`: `PhaseCheckResult`・`WorkUnitCheckFinished`・`IntegrationCheckFinished` に任意欄 `health: Option<Box<HealthEvidence>>`（`serde(default)`、`None` は出力しない）。新しい Event の種類は足していない。
  - 既存の構築箇所に `health: None` を足した（挙動は変えない）: `task-dispatch` の `dispatcher/phase_integration.rs`（WU・段の統合の検査の終わりの event 2 か所と `PhaseIntegrated.checks[]`）・`review/required.rs`（2 か所）、試験の `task-core` `required_checks/tests.rs`・`model_router/feedback/tests.rs`、`task-ops` `view/tests.rs`、`celeris` `delivery/tests.rs`、`task-api` `src/handlers/task_io.rs`（試験 module の中）・`src/query/tests.rs`・`tests/files.rs`。
  - schema の再生成: `UPDATE_SCHEMA=1 cargo test -p task-core --lib schema`・`UPDATE_SCHEMA=1 cargo test -p task-api committed_schema_matches_generated` で `docs/api/v1/event.schema.json`・`docs/api/v1/api-v1.schema.json`、`node web/scripts/gen-types.mjs` で `web/api/generated/schema.json`・`types.ts`（`HealthEvidence`・`CheckerId`・`CheckerSource`・`HealthVerdict` と 3 か所の `health?`）。
- 件名「G3 evidence core-types: health の無い検査結果は PhaseCheckResult::new で作る」の commit
  - `PhaseCheckResult::new(cmd, pass, summary)`（`health` は `None`）を足し、`review/required.rs` の 2 か所と `PhaseIntegrated.checks[]` の組み立てをこれに替えた。`dispatcher/phase_integration.rs` に `health: None` の行を足すと、repo health の `size.responsibility-growth`（単位 `crates/task-dispatch/src/dispatcher` は閾値 8000 行に対し 35166 行）が worsened で fail になった。構築を関数にして単位の行数を base と同じにした。

## event の正本の決定
- health の証跡の正本は経路ごとに 1 つにした。task の review は `RequiredChecksEvaluated.results[].health`、WU の check は `WorkUnitCheckFinished.health`、段の統合は `IntegrationCheckFinished.health`。
- `PhaseIntegrated.checks[]` は `PhaseCheckResult` を使うので型の上では `health` を持つ。しかし `dispatcher/phase_integration.rs` は常に `None` で書き、誰も読まない（段の統合の証跡は `IntegrationCheckFinished` にあり、2 つ目の正本を作らない。規約 4）。これを `PhaseCheckResult` の doc comment と、`phase_integration.rs` の構築箇所の comment に書いた。読まないことの試験（`health_evidence_phase_integrated_health_ignored`）は読む側を作る unit `core-binding` が置く。

## ADR とのずれ
- `HealthRepoFacts` に欄 `unreadable: Option<String>` を足した（D1.1 は 4 欄）。D1.1 は `applicability` の結果に `facts_unreadable:<理由>` を挙げるが、4 欄の事実からは読めなかったことが分からない。`repo_facts` が `Result` を返して呼び出し元が `Unavailable` に写す形にすると、全経路が同じ写し方を書くことになる（J1 の「1 つの関数」が崩れる）。そこで読めなかった理由を事実に載せ、判定は `applicability` だけが行う形にした。
- `repo_facts` の引数に `integrated_by_celeris` を足し、順を `(integrated_by_celeris, base_sha, bundled, git)` にした（D1.1 は `repo_facts(base_sha, git, bundled)`）。remote workspace には手元に統合する作業ツリーが無いので git を呼ばない。closure を最後に置くのは `candidate_required_checks` と同じ。
- Git 管理の判定: `git rev-parse --show-toplevel` が失敗したら `git --version` で git が動くかを確かめ、動かなければ `Unavailable(facts_unreadable)` にした。rev-parse の失敗だけでは「repo でない」と「git が起動できない」を区別できず、後者を `not_a_git_repo` にすると fail open になるため。
- `HealthEvidence.checker` は `Option<CheckerId>` にした（D2.1 は `CheckerId`）。`NotApplicable` と、検査器の無い `Error`（`checker_missing`）には使った検査器が無く、偽の値を置かないため。report から作る欄（`head_tree`・`policy_sha256`・`report_sha256`・`tool_version`）は作れなければ空文字列にした。
- `HealthEvidence` に `reviewable`（report の `summary.reviewable`。D2.7）を足した（unit の objective どおり。D2.1 の欄の列挙には無い）。
- event の欄は `Option<Box<HealthEvidence>>` にした。`Box` でないと `Event` が 400 byte を超え、`clippy::large_enum_variant` が `task-ops` の `PlanGatePlan` で落ちる（全 event の大きさが増える）。JSON と schema の形は `Box` の有無で変わらない。
- `checker_sha256` の式: domain `celeris.repo-health.checker/1` の後に `CHECKER_DIR`・tree id・`SOURCE_SIZE_REPORT`・blob id を長さ付きで入れる。base に `source-size-report.py` が無ければ blob の代わりに `absent` を入れる（D2.3 は blob が有る場合だけを書いている）。同梱の版は unit `bundle-release` の `CHECKER_TREE` の 2 行（`repo-health <tree id>`・`source-size-report <blob id>`）の値を渡せば同じ値になる。
- `policy_sha256` は D2.3 の順（`policy.version`・`policy.source`・`inputs.profile.resolved`・`inputs.profile.source`・profiles の tree id か `builtin`・`policy.files[].base_blob`）で、`base_blob` が `null` のときは `absent` を入れる。path は入れていない（D2.3 どおり）。
- 試験の prefix は ADR の例（`health_evidence_*_change_invalidates_pass` など）のうち、束縛・失効の試験を unit `core-binding` に残し、この unit は適用判定・指紋・event の互換の試験を置いた。

## 試験（`crates/task-core/src/required_checks/health_tests.rs`、12 件）
既存の試験（`required_checks/tests.rs`）は cmd 単位の合否と束縛（repo・target・reviewed）だけを見ており、適用判定・検査器の版・policy の指紋・event の `health` 欄はこれまで存在しなかったので、どの不具合も既存の試験では検出できない。各試験が守る不変条件は次のとおり。
- `health_evidence_applicability_ignores_project_and_repo_name`: J1。`HealthRepoFacts` を `..` なしで分解するので、案件・repo 名・mode の欄を足すと compile が落ちる。Celeris の profile を持つ repo と generic の repo が同じ結果になり、`repo_facts` が git に渡す引数が base と固定の path だけであることを確かめる。
- `health_evidence_non_git_repo_is_not_applicable`: Git 管理でない作業場所は `NotApplicable(not_a_git_repo)` で、base を見に行かない。同梱の検査器があっても当てない。
- `health_evidence_remote_workspace_is_not_applicable`: remote workspace は `NotApplicable(remote_workspace)` で git を 1 回も呼ばない。事実が検査器を主張しても当てない（統合しない作業ツリーに証跡を作らない。D1.1）。
- `health_evidence_bundled_checker_applies_when_the_base_has_none`: base に検査器が無いと確かめられた repo は同梱の検査器（`Bundled`）で `Applies`（Celeris 以外の repo にも当たる。D1.4）。
- `health_evidence_base_checker_wins_over_bundled`: base と同梱の両方があれば base（J3）。
- `health_evidence_no_checker_anywhere_is_checker_missing`: どちらも無ければ `Unavailable(checker_missing)` で合格にしない（J4）。
- `health_evidence_unreadable_facts_are_unavailable`: git が動かない・base を一覧できない・一覧にある検査器が読めない・tree id が読めない・tree id が壊れている・side file が読めない、の 6 つがすべて `facts_unreadable:` になり、同梱の検査器に落ちない（fail closed）。`base_has_file` は無いと確かめられたときだけ false。
- `health_evidence_repo_without_profile_still_applies`: profile の無い repo でも `Applies`（base の検査器でも同梱でも）。profile の無い policy は `builtin` として指紋が作れ、profile のある policy と違う値になる（契約 D2）。
- `health_evidence_checker_sha256_is_one_formula_for_base_and_bundled`: git から読んだ base の版が、`CHECKER_TREE` の値から計算する式と同じ値になる。tree・side file・side file の有無で値が変わり、長さ付きなので欄の境目をずらしても同じ値にならない（J2 の検査器の版）。
- `health_evidence_policy_sha256_changes_with_each_input`: policy の 6 つの入力（版・source・profile 名・profile の source・profiles の tree・base_blob の列の順・値・数）のどれが変わっても値が変わる（J2 の policy hash）。
- `health_evidence_old_event_json_without_health_reads`: `health` 欄の無い旧い `WorkUnitCheckFinished`・`IntegrationCheckFinished`・`RequiredChecksEvaluated`・`PhaseIntegrated` の JSON が読め、`health` は `None` で、書き戻しても JSON が変わらない（events は追記だけで、旧い event を書き換えずに読む。J8）。
- `health_evidence_round_trips_in_check_events`: `health` のある証跡が review と WU の event で往復し、verdict・source が snake_case で出る。空の任意欄は出力しない。

## 範囲の外の path
- `crates/task-api/src/handlers/task_io.rs`（試験 module の中だけ）・`crates/task-api/src/query/tests.rs`・`crates/task-api/tests/files.rs` は B0 の G3 の `write_set` と ADR (8) の表に無い。これらは `IntegrationCheckFinished`・`WorkUnitCheckFinished` を struct literal で組み立てる試験で、欄を足すと compile が落ちるので `health: None` だけを足した（unit の計画の範囲 check が許す path）。
- `crates/task-dispatch/src/review/required.rs`・`dispatcher/phase_integration.rs` は `health: None` を足しただけで、health の項目の評価はしていない（配線は `loop-points`）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo test -p task-core --lib health_evidence_` → exit 0、`test result: ok. 12 passed`
- `cargo test -p task-core --lib` → exit 0、`1293 passed; 0 failed`
- `cargo test -p task-api committed_schema_matches_generated` → exit 0、1 passed。`node web/scripts/gen-types.mjs` の前後で `web/api/generated` の差分が同じ
- `cargo test --workspace --no-run` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0。check-health は `verdict=pass exit=0 fail_grade=0 worsened=0`、nextest は `7993 tests run: 7993 passed, 23 skipped`、web は `Test Files 104 passed`・`Tests 711 passed`、`test-ts-verify: 20 passed, 0 failed`
- `sh scripts/dev/progress-index.sh --check` → exit 0

## 未解決
- 無し（束縛・再利用・照合は `core-binding`、起動の口は `checker-gate` の範囲）。

## 提案
- `core-binding` は `definition_digest` の v3 で、長さ付きの sha256 を `health.rs` の `domain_sha256`（`pub(super)`）で計算すると、同じ形の hash の実装が 2 つにならない（規約 2）。
- 経路（`loop-points`）は `NotApplicable`・`Unavailable` の理由を `HealthApplicability::reason()` で証跡の `reason` に写すと、理由の文が経路ごとに食い違わない。

## 本番への移行
- 無し。event の任意欄を足しただけで、旧い event はそのまま読める。config・表の変更は無い。
