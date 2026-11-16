---
title: 健全性v2 V0 generic — Celeris でない fixture repo で health の loop を HEAD の検査器で 1 周させる
tasks: [01M6TYVJT97XH88TEEAVRD7JK6]
status: done
updated: 2026-11-16
completed: 2026-11-16
---

# 健全性v2 V0 generic

## 結論
- (1) Python の試験 `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py'` は 99 件が全部 ok で exit 0。expectedFailure は 0 件、skip は 0 件。G1・G2 の expectedFailure は task 01M76BYGKY7R8SBZ22AZGJ72SP で外れ、HEAD の `scripts/tests/test_repo_health*.py` に `expectedFailure`・`skip` の指定は 1 つも無い（grep で 0 件）。
- (2) dispatcher の経路 `cargo nextest run -p task-dispatch -E 'test(/health_/)'` は 238 件が全部 pass で exit 0（filter の外の 1662 件は skip、`#[ignore]` の health 試験は 0 件）。`health_generic_flags_do_not_change_outcome` を含む。
- (3) 手の 1 周 `bash agent-docs/progress/2026-11-15-health-v2-v0/generic-loop.sh` は exit 0。1 つの fixture repo（ts）で、境界違反 exit 1 → 期限切れの例外 exit 1 → 重複は review の候補で exit 0 → 直した変更 exit 0（verdict pass）の順になった。順が崩れると script は exit 1 で止まる（期待を 1 つ変えた写しで exit 1 を確かめた）。
- objective との違いが 1 つある。重複の変更は、検査器だけでは fail にならない。dup.* は検査器の契約で review の候補（severity warning、fail 級にしない）で、不合格にするのは候補を根拠にした reviewer の blocking の判定である。検査器・policy は変えないので、script は重複の段で「exit 0 で dup.contract の new の候補が 1 件出て reviewer に渡る」ことを確かめる。reviewer の blocking で review が不合格になり task が Done にならないことは、(2) に入る `g4_r1_review_request_ts_dup_contract_fails_review` を含む 8 件（ts・rust・組み込み generic・integrated の dup.contract と dup.state）が stub の reviewer で確かめている。
- 検査器・fixture・policy（`scripts/dev/repo-health/`・`scripts/tests/`）の差分は WU の base から 0 行。

## 基準と環境
- HEAD: WU の base と同じ tree（件名「integrate wu/targets (phase measure)」）。この WU が足したのは `generic-loop.sh` とこの md だけ。
- host: home-dev。Python 3.13.3、git 2.48.1、cargo-nextest 0.9.146。CARGO_TARGET_DIR は celeris が渡した WU の scratch。
- 実行: 2026-11-16T00:14Z〜00:27Z。外部ネットワーク・LLM・実ユーザーの repo は使っていない。fixture repo は `$TMPDIR` の下に作り、script の終わりに消える。

## (1) Python の試験
`python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health*.py' -v` → `Ran 99 tests in 48.247s`、`OK`、exit 0。

| module | 件数 |
|---|---|
| test_repo_health_review_policy | 19 |
| test_repo_health_core | 18 |
| test_repo_health_dup | 15 |
| test_repo_health_generic_bypass | 10 |
| test_repo_health_structure | 10 |
| test_repo_health_archmap | 7 |
| test_repo_health_generic | 7 |
| test_repo_health_exception_expiry | 5 |
| test_repo_health_contract | 4 |
| test_repo_health_wrapped_checks | 4 |

- expectedFailure 0 件、unexpected success 0 件、skip 0 件。`grep -nE 'expectedFailure|skipTest|skipIf|skipUnless|unittest\.skip' scripts/tests/test_repo_health*.py` は 0 行。

## (2) dispatcher の経路
`cargo nextest run -p task-dispatch -E 'test(/health_/)'` → `Starting 238 tests across 2 binaries (1662 tests skipped)`、`238 tests run: 238 passed, 1662 skipped`（23.5 秒、build を含む壁時計は 2 分 29 秒）、exit 0。
`cargo nextest list -p task-dispatch --run-ignored only -E 'test(/health_/)'` は 0 件（filter に当たる `#[ignore]` は無い）。

filter は module の path にも当たるので、`dispatcher::tests::health_generic` の `g4_r1_*` も入る。主な module の件数（`cargo nextest list` の 238 件から数えた）:

| module | 件数 |
|---|---|
| dispatcher::tests::health_generic | 33 |
| health_review::verdict::tests | 15 |
| health_review::input::tests | 11 |
| dispatcher::tests::health_review_input | 11 |
| health_review::policy::tests | 10 |
| health_review::gate::tests | 10 |
| dispatcher::tests::health_limits_budget | 10 |
| dispatcher::tests::health_enforce | 10 |
| dispatcher::tests::health_limits_integration | 9 |
| health_review::limits::tests | 8 |
| ほか 23 module | 111 |

一般の repo の loop に関わる試験（全部 pass）:
- `health_generic_flags_do_not_change_outcome`: flag・案件名・repo 名で判定が変わらない。
- `health_generic_{wu_check,direct_review}_{ts,rust}_{good_passes,dep_inversion_routes_to_repair}` の 8 件: 境界違反は repair に回り、good は通る。
- `health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_{does_not_fail,reaches_reviewer}` の 8 件: 重複の候補は検査器では不合格にせず、reviewer の入力に届く。
- `g4_r1_review_request_*_{dup_contract,dup_state}_fails_review` の 8 件（組み込み generic と integrated を含む）: 候補を根拠にした reviewer の blocking で review が不合格になり、task は Done にならない。

## (3) 手の 1 周（generic-loop.sh）
呼び方: `bash agent-docs/progress/2026-11-15-health-v2-v0/generic-loop.sh`（引数なし）。実行は 1 秒弱で、2 回の出力は同じだった（commit の日付を固定している）。

script がすること:
1. `python3 -I scripts/tests/repo_health_fixtures.py build ts dep-inversion $TMPDIR/…/orchard` で fixture を作る。main は core・auth・app の 3 層と fixture の profile、candidate は core が app を import する変更。
2. 各段で HEAD の `scripts/dev/repo-health/repo_health.py --repo <fixture> --base main --head candidate --policy-version 1 --out <report>` を当て、report を `validate_report.py` で検証し、exit code・status=fail の rule・fail 級の finding・期限切れの例外・dup.contract の候補を期待と比べる。どの段も error・unknown・unsupported の rule は 0 件、`policy.changed` は false のはず。
3. git は呼び出し側の `GIT_*`・global/system の設定・hook を引き継がない（fixture の `git()` と同じ隔離）。

| 段 | fixture repo への変更 | checker exit | verdict | status=fail の rule | 要点 |
|---|---|---|---|---|---|
| 1 boundary | candidate のまま（`src/core/inverted.ts` が `../app/index` を import） | 1 | fail | deps.violation | `FAIL deps.violation src/core/inverted.ts:1: 層 core … が層 app … に依存する（import）。profile が層 core に許す依存先: なし`。fail_grade=1 |
| 2 expired | main が段 1 の finding の fingerprint に例外（`expires = 2026-02-01`）を 2026-01-15 の commit で置き、main は 2026-03-02 に進む。candidate を main に rebase | 1 | fail | deps.violation | `例外は期限切れ: expires=2026-02-01 基準日=2026-03-02 scripts/dev/repo-health/profiles/default.toml:44`。finding は exception=null・expired_exception あり。policy.changed は pass（policy は base 側で読む） |
| 3 duplicate | 逆依存を外し、`src/app/session_view.ts` に core の Session と同じ field（id・owner・state）の SessionView を足す | 0 | pass | なし | dup.contract の new の候補が 1 件（warning、candidate=true、evidence は `src/app/session_view.ts` と `src/core/session.ts`）。fail_grade=0、reviewable=true |
| 4 fixed | SessionView をやめ、core の Session を import して使う | 0 | pass | なし | deps.violation・dup.contract の finding は 0 件、fail_grade=0 |

- 各段で pass だった rule: deps.cycle・size.responsibility-growth・dup.contract・dup.state・dup.public-surface・link.unlinked・dup.test-fixture・migration.remnant・policy.changed（段 1・2 は deps.violation が fail、段 3・4 は pass）。arch-map.* の 6 件と wrapped checks の 5 件（doc.links・adr.numbers・progress.index・human-text・source-size）は fixture の profile に設定が無く not_applicable。
- 順が崩れたときの確認: 段 4 の期待を「exit 1、fail は deps.violation」に変えた写しを走らせると、`step 4 fixed: exit は 1 のはず（checker 0、report 0）` を出して exit 1 で止まった。

## 検査器・fixture・policy の差分
- `git diff --exit-code "$CELERIS_WU_BASE" -- scripts/dev/repo-health scripts/tests` → 差分 0 行、exit 0。
- script が書く profile（`scripts/dev/repo-health/profiles/default.toml`）は `$TMPDIR` の fixture repo の中の file で、この repo の profile ではない。

## 提出時の HEAD での検査
- `bash scripts/dev/test-affected.sh` → exit 0（`--list` は空。変更は agent-docs だけで、check-health は doc-only として verdict=pass exit=0）。
- `cargo clippy --workspace -- -D warnings` → exit 0（53.6 秒）。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は `generic-loop.sh` と `generic.md` の 2 件だけ。

## 未解決事項
- 手の 1 周は reviewer を動かさない（LLM を使わない）。重複の候補が review で不合格になる部分は、(2) の stub の reviewer の試験だけが確かめている。
- rust・multi の fixture では手の 1 周を回していない。rust の境界違反・重複は (1) の `test_repo_health_generic`・(2) の `*_rust_*` の試験が確かめている。

## 提案
- 計画の objective は「重複の変更で fail」と書いたが、検査器の契約（dup.* は review の候補）とは合わない。close の報告では「重複は候補として reviewer に渡り、reviewer の blocking で不合格になる」と書くよう提案する。検査器で重複を fail にする変更は、この WU の範囲の外（検査器・policy を変えない）で、行っていない。
