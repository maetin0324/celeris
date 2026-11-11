---
title: 健全性v2 G4 rs-stale — candidate・base・policy の変更と reviewer の合格の再利用で古い合格を使わず、baseline 改変で合格しない
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: done
updated: 2026-11-11
---
# 健全性v2 G4 rs-stale — candidate・base・policy の変更と reviewer の合格の再利用で古い合格を使わず、baseline 改変で合格しない

完了日: 2026-11-11

`crates/task-dispatch/src/dispatcher/tests/health_generic_stale.rs` に 6 試験を足し、6 試験とも `should_panic` 無しで合格する。前回の run は欠け G4-S1 を見つけて `health_generic_reviewer_reuse_health_fail_routes_to_repair` を `should_panic` にし、plan_issue で申告した。unit fix-s1 が G4-S1 を直した後、この run は前回の 3 commit を cherry-pick し、`should_panic` を外して ADR (3) の (g) の期待のまま合格させた（下の「G4-S1（fix-s1 で解決）」）。設計は [ADR 2026-11-10 health-generic-fixtures](../../adr/2026-11-10-health-generic-fixtures.md) の (3) の (e)・(f) の行と D3.1。基準は `$CELERIS_WU_BASE`。本体・検査器・policy・baseline・builder は変えていない。

## 実装前 brief
1. 根本原因: 本物の検査器の合格が dispatcher の流れの中で古くなる場合（候補の head・main・base 側の profile が変わる、reviewer の合格を再利用する）と、baseline・policy の改変・履歴の細工・候補が持ち込んだ検査器を、本物の検査器と fixture で確かめる試験が無い。`health_review/reuse_tests.rs` は stub の検査器に `review_item::evaluate_all` を直接当て、`target_sync.rs` の再利用の試験は health の当たらない repo で、`health_generic.rs` は 1 回の候補の判定だけを見る。
2. 責務 owner: 経路は `task_dispatch::health_review`（`review_item` の再利用・`gate` の PolicyOnly）、dispatcher の review（`review_verdict`・main 同期・`policy_review_gate`）。試験は `dispatcher/tests/health_generic_stale.rs`（ADR D3.1）。
3. 既存の再利用先: `health_generic.rs` の helper（`configure_real_bundled_checker`・`build_fixture`・`apply_candidate`・`restore_base`・`changed_paths`・`review_health`・`assert_pass`・`assert_dep_inversion_fail`・`blocking_rules`・`dep_inversion_rules`・`repair_classes`・`REVIEW_PASS`）を `pub(super)` にして使った。`target_sync.rs` の `reviewing_task`・`set_check`・`bump_main_writing`、`health_enforce.rs` の `non_celeris_dispatcher`、`mod.rs` の `ParallelWuAdapter`（reviewer の返答と `repair-<n>` の action）。
4. 選択肢: `health_generic.rs` の `review_flow` に欄を足す案は採らなかった（並行の rs-limits と同じ file を大きく変えると統合で衝突しやすい。可視性の 8 か所だけを変えた）。fixture の内容を Rust に写す案も採らなかった（ADR の K2）。変化は別の fixture の commit の取り込み（`git fetch`・`cherry-pick`）、fixture の head の file の取り出し（`git checkout <head> -- <path>`）、main の profile の `sed` で作った。
5. 新しい状態・分岐・fallback・port: 無し（試験の helper だけ）。
6. 撤去対象: 無し。
7. 不変条件: 下の「試験と結果」の表。

## 流れの作り方
- 「良い変更の Pass の後」の変化は、1 回目の review の review の repair で起こす。acceptance[0] を `test -e <flag> && git merge-base --is-ancestor main HEAD` にし、1 回目はこの command だけが落ちる（health の項目は記録される）。merge-base の語を持つ command の不合格は局所の repair（MergeBase。ADR-0074 D6.2）に分類される。`repair-1` が変化を起こして flag を立て、それ以後の repair は作業ツリーを main の tree に戻す。
- 検査の最中に main を進める流れ（`target_sync.rs` の `bump_main_writing`）は、古くなった 1 回目の review の health の項目を記録しない（同期をやり直す）。そのため「前の Pass の後の変化」には使えず、reviewer の合格の再利用の試験だけに使った（reviewer の合格は 1 回目に記録される）。
- `test -e <flag>` だけの command の不合格は Substantive（`classify_command` の語の表に無い）で、review の repair に回らない。

## 変更
- `phase 1: test stale health verdicts and baseline weakening on generic fixtures`
  - `crates/task-dispatch/src/dispatcher/tests/health_generic_stale.rs`（新規）: 6 試験と helper（`review`・`Setup`・`narrow_app_layer`・`advance_main_empty`・`sh`・`assert_fresh`・`reused_bindings`）。
  - `crates/task-dispatch/src/dispatcher/tests/health_generic.rs`: helper 8 個を `pub(super)` にした（本文は変えていない）。
  - `crates/task-dispatch/src/dispatcher/tests/mod.rs`: `mod health_generic_stale;` の 1 行。
- `phase 1: keep the fixture note path out of the doc link check`
  - fixture の repo の文書の path を `concat!("docs", "/notes.md")` の const にした。文字列のままだと `doc.links`（`scripts/dev/check-doc-links.sh`）がこの repo の壊れた link として fail 級にする（builder の `Path('docs') / 'guide.md'` と同じ扱い）。
- `phase 1: record rs-stale tests and gap G4-S1`（進捗の記録。前回の run）
- `phase 1: drop should_panic from the G4-S1 test on top of fix-s1`
  - `health_generic_reviewer_reuse_health_fail_routes_to_repair`: `should_panic` と欠けの文言の const `G4_S1` を外し、doc comment を回帰の試験の説明（fix-s1 の `try_review_repair` が `review::REVIEWER_NOT_EVALUATED_REASON` の項目を分類から外す）に直した。assert（`RepairScheduled` の class が `health:deps.violation` だけ、最後は done）は前回のまま。
  - `health_generic_reviewer_reuse_reevaluates_health_on_new_head` の policy を狭める場合: 「done にならない」の assert は fix-s1 の後は成り立たない（repair の後に done になる）ので、「done への遷移は health の review の repair（origin Review）の後」に直した。再利用の合格で done にしない、という確かめる性質は変えていない。

## 試験と結果（提出時の HEAD で実行）
| 試験 | 確かめること | 結果 |
|---|---|---|
| `health_generic_stale_head_change_reevaluates` | ts・rust の good の Pass の後に、repair が同じ base の dep-inversion の fixture の候補の commit を head に重ねる。2 回目の health は前の Pass を写さず（`reused_from` 無し、head の tree が違う）、本物の同梱の検査器が fail 級の `deps.violation`（rust は `deps.cycle` も）で Fail。修復の後に done | 合格 |
| `health_generic_stale_base_change_reevaluates` | ts・rust の good の Pass の後に main が tree の同じ空の commit で進む。同期の後の health は、head の tree が前と同じでも前の Pass を写さず、新しい base で走り直して Pass。tree だけで再利用を決める欠けがあれば落ちる | 合格 |
| `health_generic_stale_policy_change_reevaluates` | ts の good の Pass の後に main が base 側の profile で層 `app` の許可を空にする。同期の後の health は新しい base の profile で走り直し（policy の指紋が変わる）、良い変更の `app` → `core` の import が fail 級の `deps.violation` で Fail。review の repair（class `health:deps.violation`）の後に done。rust は良い変更が `Cargo.toml` の依存を足さないので対象外 | 合格 |
| `health_generic_reviewer_reuse_reevaluates_health_on_new_head` | reviewer の合格の後に検査の最中に main が進む（`target_sync.rs` と同じ流れ）。`ReviewerPassReused` が 1 件で、health は飛ばさず再利用先の `(target, reviewed)` で走り直す（`reused_from` 無し）。main が文書だけ進む ts・rust は Pass で done。main が層 `app` の許可を空にする ts は Fail で、done への遷移は health の review の repair の後（再利用の合格で done にしない） | 合格 |
| `health_generic_reviewer_reuse_health_fail_routes_to_repair` | 上の Fail の場合が、ADR (3) の (g) の行のとおり review の repair（`RepairScheduled` の class `health:deps.violation` だけ）に回り、修復の後に done になる（G4-S1 の回帰の試験） | 合格（`should_panic` 無し） |
| `health_generic_baseline_weakening_not_pass` | ts・rust で、(1) baseline-rewrite・policy-exclude の候補（dep-inversion に改変を重ねたもの）は base の profile で評価され、fail 級は dep-inversion の rule と `policy.changed` で Fail（review の repair）。(2) 同じ改変の policy の file だけの候補（fixture の head から改変の path だけを取り出す）は health の項目が PolicyOnly（合格）でも、policy の関門が承認の子 task（objective に policy の file）を作り、task は reviewing のまま止まる。(3) history-smuggle は 2 commit の候補の途中の commit を base にせず、base との差分で dep-inversion の Fail。(4) dep-inversion の候補が自分の `scripts/dev/repo-health/repo_health.py`（何もせず exit 0）を足しても、証跡の検査器は同梱のもので、合格にならない | 合格 |

ADR (3) の表との差:
- (4) の verdict は Fail ではなく Error だった。同梱の検査器が候補の差分の `.py` を評価し、fixture の profile の adapter に python が無いので、構造の rule が `unsupported:<rule>(adapter_missing:python)` になる（検査器の契約どおり。合格にはならない）。試験は「Fail か Error で、合格でなく、証跡の検査器が同梱のもの」を確かめる。
- reviewer の合格の再利用の経路で health が Fail になるのは、再利用の後に base 側が変わる場合だけ。候補が自分で層を逆向きにすると、1 回目の review で決定的な項目（health）が落ちるので reviewer run が起きず（`review.rs` の ADR-0007 D5）、再利用する合格が無い。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(health_generic_stale_) | test(health_generic_reviewer_reuse_) | test(health_generic_baseline_weakening_not_pass)'`: exit 0。6 tests run、6 passed。`grep -c should_panic crates/task-dispatch/src/dispatcher/tests/health_generic_stale.rs` は 0。
- `cargo nextest run -p task-dispatch -E 'test(health_generic_)'`: exit 0。25 tests run、25 passed（rs-paths の 19 と rs-stale の 6。`should_panic` は rs-paths の G4-R1 の 4 件だけ）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0（nextest 3734 tests passed・11 skipped、Python の unittest 94 件 OK・expected failures=2、check-health の台本 7 件 OK）。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"`: exit 0（verdict=pass、fail_grade=0、new=0）。
- `sh scripts/dev/progress-index.sh --check`: exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更 path は `dispatcher/tests/health_generic.rs`・`health_generic_stale.rs`・`mod.rs` と、この進捗 file。
- `git diff --exit-code "$CELERIS_WU_BASE" -- scripts/dev/repo-health scripts/dev/check-health.sh scripts/tests size-baseline.json`: exit 0（検査器・policy・baseline・builder に差分なし）。`git diff --stat "$CELERIS_WU_BASE" -- crates ':!crates/task-dispatch/src/dispatcher/tests'` は空（本体のコードに差分なし）。

## G4-S1（fix-s1 で解決）
- 欠け: `Check::Reviewer` のある task で health の Fail が review の repair に回らず、task が failed（`review_fail`）になった。決定的な項目が落ちると reviewer の項目が「not evaluated: a deterministic check failed」の不合格になり、`task_core::classify_review_failure` が修復できない reviewer の不合格と health の不合格の混在を Substantive にしていた。
- 再現: `python3 -I scripts/tests/repo_health_fixtures.py build ts good <dest>`。task に `Check::Reviewer` を足し、reviewer の合格の後に検査の最中に main の `scripts/dev/repo-health/profiles/default.toml` の層 `app` の `allow` を空にする（試験 `health_generic_reviewer_reuse_health_fail_routes_to_repair`）。
- 解決: unit fix-s1（ADR-0072 D16 付記 G4-S1）。`Dispatcher::try_review_repair`（`crates/task-dispatch/src/dispatcher/review_verdict.rs`）が、決定的な不合格があるとき reason が `review::REVIEWER_NOT_EVALUATED_REASON` の `Check::Reviewer` の項目を分類から外す。
- 確認: この run の HEAD（fix-s1 の上）で、上の試験が `should_panic` 無しで合格する。`RepairScheduled`（origin Review）の class は `health:deps.violation` だけで、修復の後に done。

## 未解決
- 無し（rs-stale の範囲）。rs-paths の G4-R1（`[review]` 節の無い repo で候補の finding が final review の材料に入らない）は rs-paths の記録のとおり別に扱う。
