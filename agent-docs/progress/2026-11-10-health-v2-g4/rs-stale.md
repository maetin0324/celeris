---
title: 健全性v2 G4 rs-stale — candidate・base・policy の変更と reviewer の合格の再利用で古い合格を使わず、baseline 改変で合格しない
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: blocked
updated: 2026-11-10
---
# 健全性v2 G4 rs-stale — candidate・base・policy の変更と reviewer の合格の再利用で古い合格を使わず、baseline 改変で合格しない

完了日: 未完了（未解決 G4-S1 を plan_issue で申告した）

`crates/task-dispatch/src/dispatcher/tests/health_generic_stale.rs` に 6 試験を足した。5 試験は合格、1 試験（`health_generic_reviewer_reuse_health_fail_routes_to_repair`）は欠け G4-S1 の assert の文言に限った `should_panic` で、期待を弱めずに残した。設計は [ADR 2026-11-10 health-generic-fixtures](../../adr/2026-11-10-health-generic-fixtures.md) の (3) の (e)・(f) の行と D3.1。基準は `$CELERIS_WU_BASE`。本体・検査器・policy・baseline・builder は変えていない。

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

## 試験と結果（提出時の HEAD で実行）
| 試験 | 確かめること | 結果 |
|---|---|---|
| `health_generic_stale_head_change_reevaluates` | ts・rust の good の Pass の後に、repair が同じ base の dep-inversion の fixture の候補の commit を head に重ねる。2 回目の health は前の Pass を写さず（`reused_from` 無し、head の tree が違う）、本物の同梱の検査器が fail 級の `deps.violation`（rust は `deps.cycle` も）で Fail。修復の後に done | 合格 |
| `health_generic_stale_base_change_reevaluates` | ts・rust の good の Pass の後に main が tree の同じ空の commit で進む。同期の後の health は、head の tree が前と同じでも前の Pass を写さず、新しい base で走り直して Pass。tree だけで再利用を決める欠けがあれば落ちる | 合格 |
| `health_generic_stale_policy_change_reevaluates` | ts の good の Pass の後に main が base 側の profile で層 `app` の許可を空にする。同期の後の health は新しい base の profile で走り直し（policy の指紋が変わる）、良い変更の `app` → `core` の import が fail 級の `deps.violation` で Fail。review の repair（class `health:deps.violation`）の後に done。rust は良い変更が `Cargo.toml` の依存を足さないので対象外 | 合格 |
| `health_generic_reviewer_reuse_reevaluates_health_on_new_head` | reviewer の合格の後に検査の最中に main が進む（`target_sync.rs` と同じ流れ）。`ReviewerPassReused` が 1 件で、health は飛ばさず再利用先の `(target, reviewed)` で走り直す（`reused_from` 無し）。main が文書だけ進む ts・rust は Pass で done。main が層 `app` の許可を空にする ts は Fail で、再利用の合格で done にしない | 合格 |
| `health_generic_reviewer_reuse_health_fail_routes_to_repair` | 上の Fail の場合が、ADR (3) の (g) の行のとおり review の repair（class `health:deps.violation`）に回り、修復の後に done になる | `should_panic`（G4-S1） |
| `health_generic_baseline_weakening_not_pass` | ts・rust で、(1) baseline-rewrite・policy-exclude の候補（dep-inversion に改変を重ねたもの）は base の profile で評価され、fail 級は dep-inversion の rule と `policy.changed` で Fail（review の repair）。(2) 同じ改変の policy の file だけの候補（fixture の head から改変の path だけを取り出す）は health の項目が PolicyOnly（合格）でも、policy の関門が承認の子 task（objective に policy の file）を作り、task は reviewing のまま止まる。(3) history-smuggle は 2 commit の候補の途中の commit を base にせず、base との差分で dep-inversion の Fail。(4) dep-inversion の候補が自分の `scripts/dev/repo-health/repo_health.py`（何もせず exit 0）を足しても、証跡の検査器は同梱のもので、合格にならない | 合格 |

ADR (3) の表との差:
- (4) の verdict は Fail ではなく Error だった。同梱の検査器が候補の差分の `.py` を評価し、fixture の profile の adapter に python が無いので、構造の rule が `unsupported:<rule>(adapter_missing:python)` になる（検査器の契約どおり。合格にはならない）。試験は「Fail か Error で、合格でなく、証跡の検査器が同梱のもの」を確かめる。
- reviewer の合格の再利用の経路で health が Fail になるのは、再利用の後に base 側が変わる場合だけ。候補が自分で層を逆向きにすると、1 回目の review で決定的な項目（health）が落ちるので reviewer run が起きず（`review.rs` の ADR-0007 D5）、再利用する合格が無い。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(health_generic_)'`: exit 0。25 tests run、25 passed（rs-paths の 19 と rs-stale の 6。`should_panic` は rs-paths の 4 件と G4-S1 の 1 件）。rs-stale の 6 試験は合わせて約 50 秒（1 回の review の流れは 1〜2 秒）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0（nextest 3731 tests passed・11 skipped、Python の unittest 94 件 OK・expected failures=2、check-health の台本 7 件 OK）。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"`: exit 0（verdict=pass、fail_grade=0、new=0）。
- `sh scripts/dev/progress-index.sh --check`: exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更 path は `dispatcher/tests/health_generic.rs`・`health_generic_stale.rs`・`mod.rs` と、この進捗 file。
- `git diff --exit-code "$CELERIS_WU_BASE" -- scripts/dev/repo-health scripts/dev/check-health.sh scripts/tests size-baseline.json`: exit 0（検査器・policy・baseline・builder に差分なし）。
- 参考: `cargo clippy --workspace --all-targets -- -D warnings` は `task-core` の lib test の既存の `useless conversion`（2 件）で落ちる。この unit は `task-core` を変えていない。

## 未解決
- G4-S1（plan_issue で申告）: `Check::Reviewer` のある task では、health の Fail が review の repair に回らず task が failed になる。
  - 再現: `python3 -I scripts/tests/repo_health_fixtures.py build ts good <dest>`。task に `Check::Reviewer` を足し、reviewer の合格の後に検査の最中に main の `scripts/dev/repo-health/profiles/default.toml` の層 `app` の `allow` を空にする（試験 `health_generic_reviewer_reuse_health_fail_routes_to_repair`）。
  - 期待（ADR (3) の (g) の行）: health の Fail（`deps.violation`）が review の repair（`RepairScheduled` の class `health:deps.violation`、origin Review）に回り、修復の後に done。
  - 実際: `ReviewerPassReused` の後に health は Fail（同梱の検査器、`reused_from` 無し）だが、reviewer の項目は「not evaluated: a deterministic check failed」の不合格になり（`crates/task-dispatch/src/review.rs` の決定的な条件が全部合格のときだけ reviewer を評価する分岐）、`task_core::classify_review_failure` は修復できない reviewer の不合格（class 無し）と health の不合格の混在を Substantive にする。`RepairScheduled` は 0 件で、`Transitioned { to: Failed, reason: "review_fail" }`。
  - 範囲: 再利用に限らない（未確認）。同じ分岐は reviewer run の前にも当たるので、`Check::Reviewer` のある task で候補が fail 級の finding を入れたときも、health の Fail は repair に回らず failed になると読める。rs-paths の `direct_review_*_dep_inversion_routes_to_repair` は `Check::Reviewer` を持たないので、この場合を通らない。
  - 直し方の案（どれも rs-stale の範囲外）: (a) `review_verdict` の分類で「not evaluated: a deterministic check failed」の reviewer の不合格を数えず、決定的な項目の class だけで repair を決める（owner は `task_core::execution::classify_review_failure` の呼び出し側の `review_verdict.rs`）。(b) `review.rs` が決定的な項目の不合格のときに reviewer の項目を不合格の verdict にせず、評価しなかった印にする。
  - 直ったら、`should_panic` を外す（外さないと試験が落ちるので、外し忘れは分かる）。

## 提案
- G4-S1 は (a) を推す。reviewer を評価しなかったことは不合格の原因ではなく、決定的な不合格の結果なので、分類は原因の class で決めるのが D16 の表の意図に合う。
