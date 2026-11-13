---
title: 健全性v2 G4 rs-paths — 本物の検査器と fixture で dispatcher の経路・review の材料・複数 repo・flag の非依存
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: blocked
updated: 2026-11-10
---
# 健全性v2 G4 rs-paths — 本物の検査器と fixture で dispatcher の経路・review の材料・複数 repo・flag の非依存

完了日: 未完了（未解決 G4-R1 を plan_issue で申告した）

`crates/task-dispatch/src/dispatcher/tests/health_generic.rs` に 19 試験を足した。15 試験は合格、4 試験（`*_reaches_reviewer`）は欠け G4-R1 の assert の文言に限った `should_panic` で、期待を弱めずに残した。G4-R1 は「候補の finding（`dup.*`）は本物の検査器が検出するが、`[review]` 節の無い repo では final review の材料に入らない」。設計は [ADR 2026-11-10 health-generic-fixtures](../../adr/2026-11-10-health-generic-fixtures.md) の (3)・D3.1〜D3.3。基準は `$CELERIS_WU_BASE`。本体（dispatcher の試験以外）・検査器・policy・baseline は変えていない。

## 実装前 brief
1. 根本原因: 本物の検査器の判定（層の宣言・候補の finding・evidence の path）が、dispatcher の経路の結果（repair・review の材料・統合の停止）に届くことを確かめる試験が無い。`health_enforce_non_celeris_*` は stub の検査器を使い、`test_repo_health_generic*.py` は検査器だけを fixture に当てている。
2. 責務 owner: 経路は `task_dispatch::health_review`（`gate`・`check_item`・`review_item`・`input`）と dispatcher の WU の check・review。試験は `dispatcher/tests/health_generic.rs`（ADR D3.1）。
3. 既存の再利用先: `health_enforce.rs` の `non_celeris_dispatcher`・`health_items`・`unit`・`with_check`・`RepairActionAdapter`、`target_sync.rs` の `reviewing_task`、`mod.rs` の `parallel_task`・`adopt_v2_plan`・`ParallelWuAdapter`・`project_with_repos`・`git_task`、`health_review/scenario_tests.rs` の `copy_checker`（本物の検査器の写し。`pub(super)` にした）、builder の CLI。
4. 選択肢: 検査器を写す関数を新しく書く案は採らなかった。`copy_checker` と同じ仕事の二重実装になるため。`copy_checker` を `pub(super)` にして使い、release と同じく `profiles/` を除いた。fixture の内容を Rust に写す案も採らなかった（ADR の K2）。builder の CLI を呼び、evidence の照合は `git diff --name-only base head` で取った path で行う。
5. 新しい状態・分岐・fallback・port: 無し（試験の helper だけ）。
6. 撤去対象: 無し。
7. 不変条件: 下の「試験と結果」の表。

## 変更
- `phase 1: test dispatcher health paths on generic fixtures with the real checker`
  - `crates/task-dispatch/src/dispatcher/tests/health_generic.rs`（新規）: 試験 19 件と、後続の stale・limits が `pub(super)` で使う helper。helper は `configure_real_bundled_checker()`（作業ツリーの `scripts/dev/repo-health/` と `scripts/dev/source-size-report.py` を一時 dir に写し、`profiles/` を除き、`CHECKER_TREE` に HEAD の tree id と blob id を書いて同梱の検査器に設定）、`build_fixture(kind, variant, dest)`（`python3 -I <CARGO_MANIFEST_DIR>/../../scripts/tests/repo_health_fixtures.py build …` の末尾の JSON を読む）、`apply_candidate`（worktree を fixture の候補へ `merge --ff-only`）、`restore_base`（tree を fixture の base に戻して commit。修復の代わり）、`Fixture`。
  - `crates/task-dispatch/src/dispatcher/tests/mod.rs`: `mod health_generic;` の 1 行。
  - `crates/task-dispatch/src/health_review/scenario_tests.rs`: `copy_checker` を `pub(super)` にした（1 語）。

## 試験と結果（提出時の HEAD で実行）
| 試験 | 確かめること | 結果 |
|---|---|---|
| `health_generic_wu_check_{ts,rust}_good_passes` | WU の check が本物の同梱の検査器で Pass。base は WU の base（= fixture の base）、head は候補。checks の不合格にしない。WU は done | 合格 |
| `health_generic_wu_check_{ts,rust}_dep_inversion_routes_to_repair` | 層を宣言した profile での層の逆向きが Fail。checks の不合格の detail は `health:<fingerprint>（evidence: <候補が変えた path>）`。retry が直した HEAD は Pass。replan・質問なし | 合格 |
| `health_generic_direct_review_{ts,rust}_good_passes` | 計画なしの atomic の task の direct review でも health が当たって Pass。repair なしで done | 合格 |
| `health_generic_direct_review_{ts,rust}_dep_inversion_routes_to_repair` | direct review の Fail が review の repair（`RepairScheduled`、origin Review）に回る。修復の後の再 review で Pass、done | 合格 |
| `health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_does_not_fail` | 候補の finding が report に 1 件（candidate=true・warning・new・exception null。evidence に候補が変えた path）。health は Pass で、reviewer の合格で done | 合格 |
| `health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_reaches_reviewer` | 候補の finding が `HealthReviewInputBuilt` の `input.findings.new` に path 付きで入る | `should_panic`（G4-R1） |
| `health_generic_multi_repo_each_repo_checked` | builder の multi good（ts と rust の 2 repo）で、2 つの repo それぞれに health の項目があり（base・head はその repo の fixture）、どちらも Pass で done | 合格 |
| `health_generic_multi_repo_second_repo_violation_blocks_integration` | 1 つ目の repo（ts good）は Pass でも、2 つ目の repo（rust dep-inversion）の Fail が review を止めて repair に回す。class は 2 つ目の repo の rule だけ。`review_pass` は `review_repair` の後。修復の後に done | 合格 |
| `health_generic_flags_do_not_change_outcome` | 下の「flag の実名」の 5 つの欄を 1 つずつ変えた場合と全部変えた場合で、結果が既定の task と同じ。比べるのは評価ごとの判定と fail 級の fingerprint、review の repair の class、最後の状態。ts dep-inversion（Fail）と rust good（Pass）で確かめた | 合格 |

ADR (2) の表との差:
- rust の dep-inversion は、層を宣言した profile でも `deps.violation` に加えて `deps.cycle` が fail 級になる（core の `Cargo.toml` が app に依存し、app は core に依存する Cargo の循環）。検査器の実測どおりで、試験は kind ごとの fail 級の rule の集合を明示した（ts は `deps.violation`、rust は `deps.cycle`・`deps.violation`）。そのため rust の review の repair の class は `health:deps.cycle,health:deps.violation`（bucket を `,` で並べたもの）になる。
- 複数 repo の止まる試験は、builder の multi ではなく ts good と rust dep-inversion を別々に作った 2 repo を使った。multi は 2 つの repo に同じ variant を入れるので、「1 つ目は通り、2 つ目だけが止める」を作れない。multi の kind は `health_generic_multi_repo_each_repo_checked` で使った。

## flag の実名（ADR D3.3）
| ADR の flag | 試験で変えた欄 | 値 |
|---|---|---|
| (i) 案件名 | `Project.title`（`TaskStore::project_set_text`）と `Project.slug`（`project_set_slug`） | `celeris` |
| (ii) repo id | `ProjectRepo.name` と `RepoRef.name`。`RepoId` は ULID で名前を持たないので変えられない | `celeris` |
| (iii) task の mode | `Task.mode` | `TaskMode::Prototype` |
| (iv) selfdeploy | `DispatchConfig.releases_dir`（`[selfdeploy] releases_dir`）。親の `current/manifest.json` に fixture の base の sha を書いた（worktree の base は変わらない） | 設定あり |
| (v) 案件の `self_improve` | `self_improve` という欄はコードに無い（`git grep self_improve` は注釈と ADR だけ）。案件ごとの自動化の欄 `Project.auto_integrate`（`project_set_auto_integrate`）を立てた | `true` |

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(health_generic_)'`: exit 0。19 tests run、19 passed（うち 4 件は `should_panic` の `*_reaches_reviewer`）。所要約 12 秒（1 試験は 1〜2 秒）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更 path は `crates/task-dispatch/src/dispatcher/tests/health_generic.rs`・`dispatcher/tests/mod.rs`・`health_review/scenario_tests.rs` と、この進捗 file。
- `git diff --exit-code "$CELERIS_WU_BASE" -- scripts/dev/repo-health scripts/dev/check-health.sh scripts/tests size-baseline.json`: exit 0（検査器・policy・baseline・builder に差分なし）。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` と `bash scripts/dev/test-affected.sh`: 結果は報告本文に書く（この file を足した後の HEAD で実行する）。

## 未解決
- G4-R1（plan_issue で申告）: `[review]` 節の無い repo では、候補の finding が final review の材料に入らない。
  - 再現: `python3 -I scripts/tests/repo_health_fixtures.py build ts dup-contract <dest>`（ts dup-state、rust の dup-contract・dup-state も同じ）。task に `Check::Reviewer` を足して final review を流す（試験 `health_generic_review_input_*_reaches_reviewer`）。
  - 期待（ADR (3) の `review_input_`）: `HealthReviewInputBuilt` の `input.findings.new` に `dup.contract`（`dup.state`）が、候補が変えた path 付きで 1 件入る。
  - 実際: `HealthReviewInputBuilt` は 0 件で、`HealthReviewInputSkipped` の reason は `review 方針が absent（base の profile に [review] が無い）`（`health_review::input::POLICY_ABSENT`）。候補の finding は report（task dir の `runs/health/`）にはあり（`*_does_not_fail` が確かめる）、health の判定は Pass。reviewer には届かない。
  - 範囲: fixture の profile は `[review]` を持たない。組み込み generic の profile（profile の無い repo）も `[review]` を持たないので、Celeris 以外の多くの repo で同じことが起きる。
  - 直し方の案（どれも rs-paths の範囲外）: (a) builder の fixture の base の profile に `[review]`（`invariants` など）を足し、「設計 review を選んだ汎用 repo」を試す（`scripts/tests/repo_health_fixtures.py`）。(b) `health_review::input` が、方針が `absent` でも diff・findings・policy の材料を作る（owner 地図と不変条件は空）。G2 の ADR（health-review-integration D4）の変更になる。目的「Git 管理下の repo 一般に適用する」に合うのは (b)。
  - 直ったら、4 試験の `should_panic` を外す（外さないと試験が落ちるので、外し忘れは分かる）。

## 提案
- G4-R1 は (b) を推す。(a) だけでは、profile の無い汎用 repo で reviewer が候補の finding を受け取れないままになる。
