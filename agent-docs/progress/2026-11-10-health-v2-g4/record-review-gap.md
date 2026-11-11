---
title: 健全性v2 G4 record-review-gap — G4-R1 修正後の提出証拠と目的ごとの結果
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: done
updated: 2026-11-11
---
# 健全性v2 G4 record-review-gap — G4-R1 修正後の提出証拠と目的ごとの結果

完了日: 2026-11-11

task 01M6TYVJSXBZMSBC0YGAT2TTX0 の WorkUnit record-review-gap の記録。コードは変えていない。変えたのはこの file と [G4 のまとめ](../2026-11-10-health-v2-g4.md) の 2 file だけ。基準は task では `git merge-base HEAD main`、この WorkUnit では `$CELERIS_WU_BASE`。

## 結論
- G4-R1（`[review]` 節の無い repo で候補の finding が final review の材料に入らない）は、子 task 01M74X0XAN0MCFJ4KZ3VVFT2QA（WorkUnit fix-review-gap）が owner（`crates/task-dispatch/src/health_review/`）で直した。記録は [fix-review-gap.md](fix-review-gap.md)、契約は [ADR 2026-11-11-health-generic-review-input](../../adr/2026-11-11-health-generic-review-input.md)（実装済み）。
- 前回の review が不合格の理由にした 4 試験 `health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_reaches_reviewer` は、`should_panic` を外した通常の成功試験として合格する。`health_generic.rs` の `should_panic` は 0 件。
- G4 のまとめにあった旧い結論（「G4-R1 は未解決」「4 試験は `should_panic` で成功扱い」「未解決の 3 件はどれも合格にはならない側に倒れる」）は、G4-R1 については誤りになった。まとめを訂正した（下の「訂正した記述」）。
- 未解決は検査器の欠け G1・G2 だけ。人の決定 checker-gaps=a に従い、G4 では直さず expectedFailure のまま保留した。
- 提出時の HEAD で規定の検査はすべて exit 0（下の「証拠」）。

## ReviewRequest の捕捉と reviewer の拒否の結果
試験は子 task の工程 3・4 が足した。試験の中だけの adapter `CandidateReviewer`（`dispatcher/tests/health_generic.rs`。本体に hook は無い）が、reviewer run の受けた `req.context.review.health` を記録し、受けた候補の finding を根拠に blocking の不合格を書く。
- 捕捉した材料（atomic の final review、ts・rust の dup-contract・dup-state と profile の無い組み込み generic の 6 件）: repo_id、`target_sha` = fixture の base、`reviewed_sha` = 候補、diff の path = 候補の変更 file の全部、`owner_rows`・`invariants` は空、`policy.source` は `base`（profile あり）か `builtin`（profile 無し）、`changed = false`、report は verdict `pass`・`reviewable`、期待の rule（`dup.contract`・`dup.state`）の候補が 1 件（warning・new、path は候補の変更 file）。同じ候補の `HealthReviewInputBuilt` の `input` は受けた材料と一致する。
- reviewer の拒否の結果: 受けた候補を根拠にした reviewer の blocking（`health:<kind>`）で review は不合格、検査器の health の項目は合格のまま、`review_pass` の遷移は無く、reviewer run は 1 回、task は Failed（`max_retries = 0`）。良い変更（ts・rust・組み込み generic）は候補なしの材料を受け、Done になる。
- WU の統合後・複数 repo: WU の run は `ReviewRequest.health` を受けない。統合後の final review の reviewer が材料を受け、候補で Failed、良い変更で Done。2 repo（ts の dup-contract・rust の good）では reviewer が 2 repo の材料を 1 回の run で受け、片方の候補で task 全体が Failed になる。
- `Absent` の再利用と policy の関門: reviewer run のやり直しでは同じ材料を再利用し（Built 1 件）、reviewer の合格の再利用では材料を作らない。候補が検査器を `[review]` を持つ版に書き換えても材料は base の検査器から作り、`policy.changed = true` の候補は `health.policy` の判定が無いので Failed になる。

## (a)〜(h) の結果
「測った」は提出時の HEAD で実行した結果、「保留」は人の決定で直さないもの、「未測定」は測っていないもの。試験の詳しい対応は [G4 のまとめ](../2026-11-10-health-v2-g4.md) の「目的 (a)〜(h) と試験」。

| 目的 | 試験（Rust は `dispatcher::tests::` の下、Python は `scripts/tests/`） | 経路 | 結果 |
|---|---|---|---|
| (a) 良い変更は通る | `health_generic_wu_check_{ts,rust}_good_passes`・`health_generic_direct_review_{ts,rust}_good_passes`・`health_generic_multi_repo_each_repo_checked`・`g4_r1_review_request_{ts,rust,builtin_generic}_good_is_done`・`g4_r1_review_request_integrated_good_is_done`・`g4_r1_review_request_multi_repo_good_is_done`、Python `test_generic_good_change_passes` | TS/JS・Rust・multi・profile 無し | 測った。合格 |
| (b) 同一契約の複製・不要な状態・依存逆転の検出 | `health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_{does_not_fail,reaches_reviewer}`・`g4_r1_review_request_{ts,rust}_{dup_contract,dup_state}_fails_review`・`g4_r1_review_request_builtin_generic_{dup_contract,dup_state}_fails_review`・`health_generic_{wu_check,direct_review}_{ts,rust}_dep_inversion_routes_to_repair`、Python `test_generic_dup_contract_renamed_detected` ほか 3 件 | TS/JS・Rust・multi・profile 無し | 測った。合格。候補は reviewer に届き、reviewer の拒否で Failed |
| (c) security の二重検証・vendor/generated を削らない | Python `test_generic_security_double_check_kept`・`test_generic_vendor_generated_kept` | TS/JS・Rust・multi | 測った。合格 |
| (d) docs-only・non-Git・unsupported・tool 故障 | Python `test_generic_docs_only_not_applicable`・`test_generic_non_git_error`・`test_generic_unsupported_required_not_pass`・`test_generic_tool_failure_is_error` | TS/JS・Rust・multi | 測った。4 件合格 |
| (d) の欠け G1・G2 | Python `test_generic_checker_import_failure_is_error`（G1）・`test_generic_missing_script_unsupported`（G2） | TS/JS | 保留（人の決定 checker-gaps=a）。expectedFailure のまま |
| (e) candidate・base・policy の変更で古い合格を使わない | `health_generic_stale_{head,base,policy}_change_reevaluates`・`health_generic_reviewer_reuse_reevaluates_health_on_new_head`・`g4_r1_review_request_absent_reused_on_reviewer_retry`、Python `test_generic_binding_tracks_candidate_base_policy` | TS/JS・Rust | 測った。合格 |
| (f) baseline・policy・例外の改変と履歴の細工ですり抜けない | `health_generic_baseline_weakening_not_pass`・`g4_r1_review_request_absent_candidate_policy_is_not_trusted`、Python `test_generic_policy_weakening_fails`・`test_generic_history_smuggle_fails` | TS/JS・Rust・multi | 測った。合格 |
| (g) atomic・WU・review reuse の抜け道なし、複数 repo、flag の非依存 | `health_generic_wu_check_*`・`health_generic_direct_review_*`・`health_generic_multi_repo_*`・`health_generic_flags_do_not_change_outcome`・`health_generic_reviewer_reuse_health_fail_routes_to_repair`・`g4_r1_review_request_integrated_*`・`g4_r1_review_request_multi_repo_*`・`g4_r1_review_request_absent_not_built_when_the_reviewer_pass_is_reused` | TS/JS・Rust・2 repo | 測った。合格 |
| (h) 反復上限・同じ指摘の往復・変更なし repo の棚卸しの抑制・期限付き例外 | `health_generic_limits_repair_cap_blocks`・`health_generic_limits_repeat_escalates`・`health_generic_limits_inventory_skips_unchanged`、Python `test_repo_health_exception_expiry.py` の 5 件 | TS/JS・Rust・一時の Git repo | 測った。合格 |

- 実ユーザーの repo は変更していない。fixture は builder `scripts/tests/repo_health_fixtures.py` が `$TMPDIR` の下に作る一時の Git repo だけ。

## 測定（既存の結果。再測定していない）
出典は WorkUnit measure の成果物 `wu/measure/artifacts/g4-measurement.md`（sha256 `7de904ab77834a933968fe7f454dd697c1703f4d13c01939df006941de3dfca3`）と `g4-measurement.json`（sha256 `0443c6013026c22d39bfc0d4c32c35a820e261efa8fcbfb1464477a2c4a125a8`）、記録は [measure.md](measure.md)。有料の再測定はしていない（計画の承認の条件）。

| 項目 | 結果 |
|---|---|
| 候補の検出（dup.*） | 8/8 |
| fail 級の検出 | 48/48 |
| fail 級の適合率 | 48/62（77.4%）。差の 14 件はすべて rust の `deps.cycle` |
| 誤検出（good・security-double・vendor-generated・docs-only の fail 級） | 0 件（profile 有り 16 repo・profile 無し 16 repo） |
| 追加遅延（検査器 1 回の中央値） | ts 0.110 秒・rust 0.106 秒・multi 0.289 秒。参考: この repo に 1 回当てた実時間は 147.6 秒 |
| 費用 | 検査器は token 0・0 USD |
| reviewer の費用の増分 | 未測定。G4-R1 の修正で `[review]` の無い repo の reviewer にも材料が渡るので prompt が長くなるが、増分は測っていない |

- 限界: fixture は 1 repo 10 file 程度の小規模なもので、実在する OSS の repo での精度は未確認。候補の warning が reviewer の判断を変えるかは、試験の中だけの adapter でしか確かめていない（本物の LLM の reviewer では未確認）。`RequiredChecksEvaluated` の時刻の差も未測定。
- 測定は G4-R1 の修正の前に行った。修正は検査器・profile・baseline を変えていないので、検査器の数値（精度・誤検出・遅延・検査器の費用）は修正の前後で同じ前提のまま使える。修正が変えたのは reviewer に渡す材料だけ。

## 訂正した記述（G4 のまとめ）
- 「結論」: G4-R1 を未解決から外し、子 task が直したと書いた。未解決は G1・G2 だけ。
- 「unit の記録」: rs-paths の行の「G4-R1 は未解決のまま `should_panic` で残した」に、fix-review-gap が外したことを足した。fix-review-gap・record-review-gap・integrate-review-gap の行を足した。
- 「目的 (a)〜(h) と試験」: (b) の final review の材料の行を `should_panic`（未解決）から合格に直し、`g4_r1_review_request_*` を (a)(b)(e)(f)(g) に足した。
- 「task の受け入れ条件と試験」: 条件 0 の「(b) の final review の材料は G4-R1 で未解決」を消した。
- 「未解決の欠け」の G4-R1 の節を「解決した欠け G4-R1」に移した。
- 「証拠」: この WorkUnit の提出時の HEAD の結果に替えた（前の証拠は main の取り込みの後で G4-R1 の修正の前のもの）。
- 「未解決」「提案」: G4-R1 を消し、子 task の提案（`task.repos` の無い task の final review、古い注釈 3 か所）を足した。

## 変更
- `phase 6: record-review-gap の進捗を置く`
- `phase 6: G4-R1 修正後の目的ごとの結果と提出証拠を記録する`
- `phase 6: 規定の検査の結果を進捗に書く`
- `phase 6: run の後の check の不合格（v8 の build 環境）と再実行の結果を進捗に書く`

## run の後の check の不合格と対処（attempt 3）
- 症状: attempt 2 の run の後に celeris が走らせた計画の check 1（`cargo nextest run -p task-dispatch --lib -E 'test(/health_generic_/) | test(/health_review_input_/) | test(/g4_r1_review_request_/)'`）が、v8 の lib のコンパイルで `could not find native static library rusty_v8` になり、exit 101 で落ちた。試験は 1 件も走っていない。
- 原因: この WorkUnit の `CARGO_TARGET_DIR` は scratch seed の写し（`target-origin.json` は `origin: seed`、写し元は `target-origin.json` の `seed` が指す seed の古い世代、作成は 2026-11-11 20:35:42Z）。v8 の build script の出力 `debug/build/v8-*/output` の `cargo:rustc-link-search` が、その古い世代の `target/debug/gn_out/obj` を指していた。seed の pool には 20:46 に `current` が指した現行の世代の dir しか無く、指す先の dir は無かった。features の違う `-p task-dispatch --lib` の build で v8 の lib を作り直すときに link が落ちる。[fix-v8-env.md](fix-v8-env.md) と同じ既知の build 環境の不具合で、repo のコードの欠陥ではない。
- 対処: この WorkUnit の target で `cargo clean -p v8` を実行した（38 file、230.1 MiB を削除）。次の build で v8 の build script が `~/.cargo/config.toml` の `RUSTY_V8_ARCHIVE` から offline で作り直し、link-search は target 自身の `debug/gn_out/obj` を指すようになった（[fix-v8-env.md](fix-v8-env.md) の検査 1 の台本で `EXISTS` 1 件、`MISSING` 0 件）。repo のコード・試験・検査器・policy は変えていない。
- 注意: celeris の check が別の target（seed の新しい写し）で走り、その後に seed の世代が替わると、同じ不具合がもう一度起きうる。恒久の直しは seed の仕組みの側にある（G4 の範囲外。[G4 のまとめ](../2026-11-10-health-v2-g4.md) の提案 5）。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- 基準について: この WorkUnit の `$CELERIS_WU_BASE` は開始時の snapshot で、そこからの code の差分は無い。そのまま走らせると docs だけの軽い経路になるので、task 全体の証拠にするため `env -u CELERIS_WU_BASE` で基準を `git merge-base HEAD main` にした。対象（`--list`）は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
- 最終同期について: この WorkUnit では main を merge していない。WU の中で main を取り込むと WU の health の base に対して main の policy file の変更が `policy.changed` になるので、main の取り込みは final review の前の celeris の同期に任せる。この WorkUnit の最後の同期は、開始時の基点（commit『integrate wu/record (phase record)』）。
- 実行した HEAD: この節の結果は attempt 3 の run が、commit『phase 6: 規定の検査の結果を進捗に書く』の後の HEAD で実行した（2026-11-11 21:07〜21:21 UTC）。その後の commit『phase 6: run の後の check の不合格（v8 の build 環境）と再実行の結果を進捗に書く』は 2 つの進捗 file だけを変える（code の差分 0）。その commit の後に再実行した検査は WorkUnit の result.json の evidence に書いた。
- 計画の check 1 `cargo nextest run -p task-dispatch --lib -E 'test(/health_generic_/) | test(/health_review_input_/) | test(/g4_r1_review_request_/)'` → exit 0（1 分 19 秒。`Compiling v8 v150.4.0` を含み、link の失敗は無い）。60 件すべて合格（`health_generic_` 28 件、`health_review_input_` 15 件、`g4_r1_review_request_` 17 件。`health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_reaches_reviewer` の 4 件を含む）、1645 件 skip。
- `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh` → exit 0（726 秒。load average の 1 分平均は開始時 7.8、終了時 26）。nextest 3764 件すべて合格（467.0 秒、slow 19 件）、11 件 skip。対象 package の doc test も合格。repo-health は verdict pass・exit 0・fail 級 0・new 0・worsened 0（既存 711 件）。検査器自身の試験 94 件は `OK (expected failures=2)`（G1・G2 の 2 件だけ）、`repo_health_gate: all checks passed`、script bundle の SKIP は 0 件。
- attempt 2 の run も同じ code で test-affected を実行し、exit 0（790 秒、nextest 3764 件合格）だった。
- `grep -c should_panic crates/task-dispatch/src/dispatcher/tests/health_generic.rs` → 0。`git grep -n -e POLICY_ABSENT -e 'fn absent_policy' -- crates/` → 出力なし（旧い省略経路は無い）。
- `cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・文書検査・範囲 → 下の「規定の検査（最後の commit の後）」。
- 全体試験（nextest の全件・doc test・web e2e・実 browser/userns）は実行していない（release gate だけで走らせる規則）。

## 規定の検査（最後の commit の後）
attempt 3 の run が、上の「証拠」と同じ HEAD（commit『phase 6: 規定の検査の結果を進捗に書く』の後）で計画の check 2〜6 を実行した。その後の commit『phase 6: run の後の check の不合格（v8 の build 環境）と再実行の結果を進捗に書く』は進捗 file だけを変える（code の差分 0）。その commit の後の再実行の結果は WorkUnit の result.json の evidence に書いた。
- `bash scripts/dev/test-affected.sh`（WorkUnit の基準 `$CELERIS_WU_BASE`。計画の check 2）→ exit 0（150 秒）。`試験対象の変更なし（docs のみ等）` の軽い経路で、repo-health は verdict pass・fail 級 0・new 0・worsened 0、検査器自身の試験 94 件は `OK (expected failures=2)`、`repo_health_gate: all checks passed`。
- `cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check`（check 3）→ exit 0（23 秒）。fmt の差分なし。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check`（check 4）→ exit 0（`check-doc-links: ok`、`check-adr-numbers: ok (420 files)`、`progress-index --check: ok`）。
- `test -f …/record-review-gap.md && python3 -m unittest discover -s scripts/tests -p 'test_repo_health_generic*.py'`（check 5）→ exit 0。16 件、`OK (expected failures=2)`（G1・G2）。
- attempt 2 の run では、ほかに `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"`（exit 0、mode doc-only、verdict pass）、`python3 scripts/dev/check-human-text.py --root .`（exit 0、指摘 0 件）、`git diff --check`（exit 0）も実行した。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0。変更 path はこの file と [G4 のまとめ](../2026-11-10-health-v2-g4.md) の 2 file だけ。`git diff --quiet "$CELERIS_WU_BASE" HEAD -- crates/ scripts/` → exit 0（code・検査器の差分なし）。
- 禁止範囲: `git diff --quiet $(git merge-base HEAD main) HEAD -- crates/task-core/ crates/campaign/ scripts/dev/repo-health/ scripts/dev/check-health.sh scripts/dev/source-size-baseline.json` → exit 0（task 全体で task-core・campaign・検査器・baseline に差分なし）。

## 受け入れ条件ごとの証拠（この WorkUnit）
| 条件 | 実行したコマンド | 結果 |
|---|---|---|
| 0. G4-R1 の配送と reviewer 評価が通常の成功試験で確認され、旧い未解決の説明が訂正されている | 計画の check 1（上の nextest の filter）、`grep -c should_panic …/health_generic.rs` | exit 0、60 件合格（`…_reaches_reviewer` の 4 件と `g4_r1_review_request_` の 17 件を含む）、`should_panic` 0 件。G4 のまとめの訂正は「訂正した記述」 |
| 1. (a)〜(h) の証拠と未測定・G1/G2 の保留が区別されている | — | 「(a)〜(h) の結果」の結果の列（測った・保留）と「測定」の未測定の行 |
| 2. 最終同期後の規定検査の結果を提出時 HEAD の証拠として記録している | test-affected（基準 `git merge-base HEAD main` と `$CELERIS_WU_BASE`）・clippy・fmt・文書検査・範囲 | 上の「証拠」と「規定の検査」。すべて exit 0 |

## 未解決
- G1・G2（検査器の欠け）。人の決定 checker-gaps=a で保留。
- 未測定: reviewer の LLM の費用のうち health の材料による増分、`RequiredChecksEvaluated` の時刻の差、実在する OSS の repo での精度。
- scratch seed の世代交代で、seed から写した target の v8 の link-search が消えた path を指す不具合（G4 の範囲外）。この WorkUnit の target でも起き、`cargo clean -p v8` で直した（上の「run の後の check の不合格と対処」）。

## 本番への移行
- 無し。この WorkUnit は進捗の文書だけを変えた。
