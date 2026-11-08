---
title: 健全性v2 G2 dispatch — risk 判定・高 risk 設計 review・final review の health 文脈と構造化 verdict
tasks: [01M6VTFG9K5HEE817EW05JASMZ]
status: done
updated: 2026-11-08
completed: 2026-11-08
---
# 健全性v2 G2 dispatch — risk 判定・高 risk 設計 review・final review の health 文脈と構造化 verdict

完了日: 2026-11-08

task 01M6VTFG9K5HEE817EW05JASMZ の記録。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../adr/2026-11-07-health-review-integration.md) の D2.3・D3〜D6 で、ADR との違いは同じ ADR の「付記（2026-11-08、dispatch の実装）」に書いた。基準は `git merge-base HEAD main`（WorkUnit の検査は `$CELERIS_WU_BASE`）。各 WorkUnit の詳細は `dispatch/<key>.md`。

## 結論
- (1) 方針の読み込み: `health_review/policy.rs` が target（base）の commit の検査器で `--print-review-policy` を起こし、planner run の起動時に `ExecutionPlannerContext.review_policy` に渡す。
- (2) 設計 review: 計画の採用前に `assess_risk`（task-core の純粋関数）で repo ごとに risk を判定し、`high`・`unknown` の計画だけ既存の reviewer run で設計 review を起こす。不合格は planner の replan、2 回続けば `PlanGate`。`none`・`low` は追加の run なしで今までどおり採用する。
- (3) health 文脈: final review の reviewer run を起こすときに candidate SHA ごとに 1 回作り、`ReviewRequest.health` に入れる。同じ `(repo_id, target_sha, reviewed_sha, definition_sha256)` の `HealthReviewInputBuilt` があれば作り直さない。reviewer 再利用・WU の check・WU の review・tick では作らない。
- (4) health verdict: blocking の finding を不合格の verdict にし、既存の `classify_review_failure` で局所修復か replan に写す。advisory は記録だけ。health を渡した review で `health` が無い・壊れていれば不合格。
- (5) policy: 候補の profile・検査器は判定に使わない。policy の file と他の file を混ぜた候補は `policy_mixed`（replan）、policy の file だけの候補は reviewer の policy verdict と人の承認（`Approval` の子 task）で通す。policy の関門は fail closed にした。policy の状態を決められない candidate（base の方針を読めない・report を作れない）は不明として人の承認を求め、`[review]` が無いが検査器のある repo と `Check::Reviewer` の無い task（reviewer run の無い review）も関門に掛ける（下の「final review の差し戻しと修正」）。
- (6) 試験: `health_review_` の試験で通常変更（追加の run なし・health 文脈だけ）と高 risk 変更（設計 review が起き、blocking で不合格）を流れ全体で区別した（`crates/task-dispatch/src/health_review/scenario_tests.rs`）。architecture-map の「planner/reviewer の起動・判定」と「健全性の review」の行を更新した。
- dispatcher から LLM は呼ばない。新しい review エンジン・新旧切替 flag・config 欄は足していない。

## final review の差し戻しと修正
1 回目の final review は、受け入れ条件 3（候補が policy を弱めて自分を通せない・承認の無い fallback を足さない）を不合格にした。技術的な差し戻しで、needs-human ではない。理由は policy の関門（ADR の I8・D6.4）が次の 3 つの場合に fail open になることだった。policy の file だけの候補は D6.2 で `check-health.sh` が exit 0 になるので、どの場合も LLM の reviewer の合格だけで、人の承認なしに通りえた。
- (a) report の失敗: base の検査器が report を作れない（report を書かない・600 秒の時間切れ・壊れた report）と、`input.rs` の `unavailable_report` が policy を `{source: "unavailable", changed: false, files: []}` として reviewer に渡し、`HealthReviewInputBuilt` を記録しなかった。`verdict.rs` は `policy.changed` しか見ず、`policy_review.rs` は `HealthReviewInputBuilt` しか読まないので、policy verdict も人の承認も求めなかった。
- (b) 方針を読めない: final review で base の方針が `Unavailable` の repo は `HealthReviewInputSkipped` を残すだけで、この event を読む判定は無かった。repo が 1 つなら `ReviewRequest.health` が `None` になり、従来の review になった。
- (c) `Check::Reviewer` の無い task: health 文脈を作らず、policy の承認も求めなかった（close の進捗の未解決にあった穴）。

replan v3 で段 failclosed（policy-unknown → policy-no-reviewer）と段 record（reclose）を足して直した。
- policy-unknown が (a)(b) を直した。base に検査器がある repo の candidate は、policy の状態を candidate ごとに event に残す（reviewer に材料を渡した repo は `HealthReviewInputBuilt.input.policy`、渡さなかった repo は `HealthReviewInputSkipped` に serde default で足した `target_sha`・`reviewed_sha`・`policy`）。決められなければ `source = "unavailable"`（不明）として、D6.4 の人の承認を求める。不明には reviewer の policy verdict を求めない。方針が `Absent`（`[review]` が無い）で検査器のある repo も report を 1 回走らせて関門に掛ける。
- policy-no-reviewer が (c) を直した。reviewer run の無い final review では、`health_job` が `policy_only` の仕事を作り、base の検査器の report で candidate の policy の状態だけを決めて `HealthReviewInputSkipped` に残す。`policy_review_gate` は `Check::Reviewer` の条件が無い task にも掛かり、verdict は acceptance の外の criterion（この review の verdict の最大の次）に付ける。
- reclose（この WorkUnit）が ADR の付記・architecture-map の 2 行・この file を新しい扱いに合わせた。

## 変更（WorkUnit ごと。commit は件名）
- 子 task review-policy-cli（01M6VVB2X5EBVCYB655QJVHCN8。policy の file だけの branch）: 「G2 review-policy-cli: --print-review-policy と policy の file だけの変更の扱い（D2.3・D6.2・D7.3）」「G2 review-policy-cli: 進捗（実装・検査結果・取り込み待ち）」「G2 review-policy-cli: 進捗の main..HEAD の commit 数を直す」「G2 review-policy-cli: 進捗（main への取り込みの後の確認、done）」。記録は [review-policy-cli](dispatch/review-policy-cli.md)。
- risk-events: 「G2 risk-events: assess_risk・設計 review と health の event・設計 review の wire 型」「G2 risk-events: design_review を task-core の module の循環から外す」「G2 risk-events: web の realtime の event 種類に設計 review と health の 4 種を足す」「G2 risk-events: 設計 review の依頼と brief を Box にして RunContext を大きくしない」と進捗の 4 件（「進捗に Box・web の変更と提出時の検査を書く」「進捗に範囲 check の不備（Cargo.lock・web/api/realtime）を書く」「進捗に範囲 check の再申告を書く」「進捗を done にして直した範囲 check と提出時の検査を書く」）。記録は [risk-events](dispatch/risk-events.md)。
- policy-load: 「G2 policy-load: base 側の検査器で review 方針を読み planner の RunContext に渡す」「G2 policy-load: 再起動後の統合やり直しの試験で d1 の統合の終わりを待つ（負荷で merge が重なり Blocked になる）」と進捗の 3 件。記録は [policy-load](dispatch/policy-load.md)。
- design-gate: 「G2 design-gate: 計画の採用前の risk 判定と高 risk だけの設計 review の起動」「G2 design-gate: 試験の仮の path を docs/ で始めない（doc.links が link と読む）」「G2 design-gate: 進捗（変更・判断・証拠・未解決）」。記録は [design-gate](dispatch/design-gate.md)。
- health-input: 「G2 health-input: final review の health 文脈を candidate SHA ごとに 1 回作り ReviewRequest.health に渡す」「G2 health-input: 試験の仮の owner 地図の path を docs/ の外にする（doc-links の検査が link と読む）」「G2 health-input: 進捗（変更・証拠・未解決・提案）」。記録は [health-input](dispatch/health-input.md)。
- health-verdict: 「G2 health-verdict: review.json の health verdict を読み、blocking を不合格にして repair/replan に写す」「G2 health-verdict: 子待ちの親の試験 2 件を tick の回数でなく状態で待つ（負荷で 2 回目の run が 1.2 秒に間に合わない）」と進捗の 2 件。記録は [health-verdict](dispatch/health-verdict.md)。
- base 段の統合: 「integrate wu/review-policy-cli (phase base)」「integrate wu/design-gate (phase base)」「Merge branch 'celeris-wu/01M6VTFG9K5HEE817EW05JASMZ/health-input' into celeris/01M6VTFG9K5HEE817EW05JASMZ」（repair WU merge-base-health-input が `policy.rs` の衝突を解いた merge。`with_base_checker` の戻りを `Result<Option<T>, String>` にし、base に検査器が無いときを `None` にした）「integrate wu/health-verdict (phase base)」。
- design-verdict: 「G2 design-verdict: 設計 review の結果で計画を採用・replan・人に回す（events だけで待つ）」「G2 design-verdict: 進捗（変更・判断・証拠・未解決・提案）」。記録は [design-verdict](dispatch/design-verdict.md)。
- policy-review: 「G2 policy-review: policy の変更を別 review にする（policy_mixed・policy verdict の必須化・人の承認）」「G2 policy-review: 進捗（変更・判断・証拠・未解決・提案）」。記録は [policy-review](dispatch/policy-review.md)。
- follow 段の統合: 「integrate wu/design-verdict (phase follow)」「integrate wu/policy-review (phase follow)」。
- scenario: 「G2 scenario: 通常変更と高 risk 変更を流れ全体で区別する試験と本物の --print-review-policy の読み込みの試験」「G2 scenario: 進捗（変更・判断・証拠・未解決・提案）」。記録は [scenario](dispatch/scenario.md)。
- close: 「G2 close: architecture-map の行・ADR の付記・task の進捗」「G2 close: 進捗に提出時の検査を書く」。`docs/architecture-map.md` の 2 行（「planner/reviewer の起動・判定」と「健全性の review」）に関連試験と不変条件（I3・I4・I5・I7・I8・I9）を書き、ADR に付記を足し、この file を書いた。コードは変えていない。
- verify 段の統合: 「integrate wu/close (phase verify)」。
- policy-unknown（差し戻し (a)(b) の修正）: 「G2 policy-unknown: policy の状態を決められない candidate を不明として記録し、人の承認なしに合格させない」「G2 policy-unknown: 進捗（変更・判断・証拠・未解決・提案）」。試験は prefix `health_review_policy_unknown_` の 6 件（`health_review/policy_review/unknown_tests.rs` に 5 件、`dispatcher/tests/health_review_input.rs` に流れ全体の 1 件）。記録は [policy-unknown](dispatch/policy-unknown.md)。
- policy-no-reviewer（差し戻し (c) の修正）: 「G2 policy-no-reviewer: reviewer run の無い final review にも policy の関門を掛ける」「G2 policy-no-reviewer: 進捗（変更・判断・証拠・未解決・提案）」。試験は prefix `health_review_policy_no_reviewer_` の 7 件（`health_review/policy_review/no_reviewer_tests.rs` に 5 件、`dispatcher/tests/health_review_input.rs` に流れ全体の 2 件）。記録は [policy-no-reviewer](dispatch/policy-no-reviewer.md)。
- failclosed 段の統合: 「integrate wu/policy-no-reviewer (phase failclosed)」（policy-unknown の 2 件はその前に同じ branch に入っている）。
- reclose（この WorkUnit）: 「G2 reclose: ADR の付記・architecture-map の行・task の進捗を policy の関門の fail closed に合わせる」。ADR の付記に節「policy の関門の fail closed（final review の差し戻しの修正）」を足し、D4.3 の段落（reviewer に `verdict = error` を渡す扱い）を不明の記録と人の承認に合わせて書き直し、試験の件数を直した。architecture-map の 2 行の関連試験に `crates/task-dispatch/src/health_review/policy_review/{unknown_tests,no_reviewer_tests}.rs` を足し、不変条件 I8 を fail closed の範囲に広げた。この file を更新した。コードは変えていない。

## 人の承認と main への取り込みの経緯
- ADR D6.6 のとおり、`scripts/dev/repo-health/` と `scripts/dev/check-health.sh` を変える U2 は policy の file だけの branch で、人の承認で通した。main の base には D6.2 の扱いがまだ無く、policy の file を変えた branch は必須検査 `test-affected.sh`（先頭で `check-health.sh` を呼ぶ）が `policy.changed` で落ちるためである。
- 前段: profile の `[review]` 節は別 task 01M6VCXJCX の branch で、人が main に merge した（main の件名「integrate 01M6VCXJCX: profile の [review] 節（policy の branch、人の承認済み。ADR health-review-integration D6.6）」）。
- 子 task review-policy-cli（01M6VVB2X5）の 1 回目の run は実装・試験・進捗を commit し、決定 `policy-cli-merge`（推奨は merge）で人の merge を待った。人が main に merge した（main の件名「integrate 01M6VVB2X5: --print-review-policy と D6.2（policy の branch、人の承認済み。ADR health-review-integration D6.6）」）。2 回目の run は main を取り込まずに進捗だけを commit し、`test-affected.sh`・`check-health.sh` が exit 0 になることを確かめて done にした。その後、base 段の統合（「integrate wu/review-policy-cli (phase base)」）でこの task の branch に入った。
- 人の merge で main に先に入った commit は、子 task の branch にあった「G2 review-policy-cli: --print-review-policy と policy の file だけの変更の扱い（D2.3・D6.2・D7.3）」「G2 review-policy-cli: 進捗（実装・検査結果・取り込み待ち）」「G2 review-policy-cli: 進捗の main..HEAD の commit 数を直す」の 3 件。`git merge-base HEAD main` は 3 件目になり、ほかの WorkUnit の commit（Rust を含む）はまだ main に無い（提出時の HEAD で `git log` を確かめた）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
reclose の WorkUnit の提出時の HEAD（件名「G2 reclose: …」の commit。差分は文書 3 file だけ）で実行した。main は取り込んでいない。`git merge-base HEAD main` は review-policy-cli の 3 件目の commit（件名「G2 review-policy-cli: 進捗の main..HEAD の commit 数を直す」）のままで、この task のほかの commit（Rust を含む）は main に無い（`git log <merge-base>..main` に件名「G2 …」の commit は 0 件）。
- `bash scripts/dev/test-affected.sh` → exit 0。先頭の `check-health.sh` は `mode=full`・base は `git merge-base HEAD main`・`verdict=pass`・`new=0`・`worsened=0`・`existing=718`。arch-map の 6 rule は全部 pass（missing_row 1・missing_test 77・wrong_owner 32・missing_invariant 32・stale 0・wrong_path 0 で、close の時と同じ数）。この WorkUnit の差分は文書だけなので「試験対象の変更なし（docs のみ等）」。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo nextest run -p task-dispatch -p task-core -p task-worker -E 'test(/health_review_/)' --no-fail-fast` → exit 0、81 tests run: 81 passed（task-dispatch 68・task-core 9・task-worker 4。`cargo nextest list` で数えた）。このうち `health_review_policy_unknown_` が 6 件、`health_review_policy_no_reviewer_` が 7 件。流れ全体の `health_review_scenario_normal_change`・`health_review_scenario_high_risk_change`・`health_review_scenario_real_cli` を含む。
- `python3 scripts/dev/check-architecture-map.py` → exit 0（「OK: 956 件のパスを確認した」。close の時の 954 件に、2 行に足した `policy_review/{unknown_tests,no_reviewer_tests}.rs` の 2 件が増えた）。
- `sh scripts/dev/check-doc-links.sh` → exit 0（ok）。`sh scripts/dev/check-adr-numbers.sh` → exit 0（407 files）。`sh scripts/dev/progress-index.sh --check` → exit 0（ok）。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0（ok）。`python3 scripts/dev/check-human-text.py` → exit 0（0 findings）。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は `agent-docs/adr/2026-11-07-health-review-integration.md`・`docs/architecture-map.md`・この file の 3 つだけ。

## 未解決
- prompt の 2 点はこの task の範囲外で、まだ無い（prompts task 01M6VTFGWFVE57YXDKHR2E436G の範囲）。提出時の HEAD の `crates/task-worker/src/claude_code/prompt.rs` で確かめた。
  - D3.4: `build_execution_plan_prompt` に brief の書き方と review policy の class 一覧の節が無い。planner は brief を書く指示を受けないので、高 risk の class の path を書く計画は `DesignBriefMissing` の検証失敗と再試行の文で初めて brief を求められる。
  - D5.4: reviewer prompt の Result 節（`## Result`）は旧い形 `{"verdicts":[{"criterion","pass","reason"}]}` だけを指示し、`health`・`design` の形と 7 種の表が無い。reviewer prompt には health 文脈と設計 review の依頼を描く節も無い。reviewer が `health` を書かなければ、`[review]` 節のある repo（Celeris 自身）の final review は I7 で不合格になり、設計 review は `design` が無いので不合格 → 再試行 → 2 回目で `PlanGate` になる。この branch を prompts task より先に本番へ出さない。
- replan の planner の試行が `execution-plan.json` を書かずに終わると、dispatcher は前の試行が残した同じ file を読み、同じ計画を版 2 として採用し直す（scenario で見つけた。試験の adapter は file を消して避けた）。
- 設計 review の run には task 木の token 予算の watch が付いていない。設計 review の待ち・不合格の findings を人に見せる表示（task の待ちの理由・web の計画の節）が無く、`WorkerProgress` と `DesignReviewed` の event だけ。
- planner run に渡す方針は task の先頭の repo の 1 つだけ（`ExecutionPlannerContext.review_policy` が 1 つ）。計画の採用前の判定は repo ごとに読む。
- 計画の write path が class より広い接頭辞（例 `crates/task-core/`）だと、`re.search` の意味で class（例 `^crates/task-core/src/store/`）に合わず、高 risk にならない。
- 本物の `repo_health.py` はこの repo の全体で約 80 秒かかった（health-input の手元の計測 1 回）。その分 reviewer run の起動が遅れる。上限 600 秒は計測 1 回からの見積もり。
- `policy_mixed` は `review.json` を読めた review にだけ足す。reviewer run の無い review で policy の file とほかの file を混ぜた候補は、`policy_mixed` の replan でなく人の承認を求める（人が却下すれば replan）。
- 材料作りの非同期 task が panic した（`spawn_blocking` の失敗）ときは event が残らず、policy の関門が掛からない（reviewer の有無に依らない）。
- 同期の無い repo（同期を省いた・失敗した）と remote workspace の repo は candidate の SHA が決まらないので、policy の状態の記録も関門も無い（ADR の D4.4 のとおり）。
- `Check::Reviewer` の無い task の final review は、base に検査器がある repo では repo health の実行時間（この repo で約 80 秒、上限 600 秒）だけ遅れる。reviewer run の無い review では、関門のために repo health の全体を走らせているため。
- policy の承認の子 task の受信箱での見え方（本文の表示）は画面で確かめていない（未確認）。
- `cargo clippy -p task-dispatch --tests` は base からある `dispatcher/tests/cos_slot_wait.rs:172` の lint で落ちる（必須の `cargo clippy --workspace -- -D warnings` は通る）。

## 提案
- prompts task（01M6VTFGWFVE57YXDKHR2E436G）の D3.4・D5.4 を、この task と同じ親の branch に入れてから親の final review と配送に進める。
- planner の replan の run を起こす前に、前の試行の `artifacts/execution-plan.json` を退ける（`execution-plan.rejected.*` と同じ扱い）か、run の開始より古い file を読まない unit を G2 の後に置く。
- 設計 review の待ちの表示（`TaskWait`）と、不合格の findings を web の task 画面の計画の節に出す unit を G2 の後に置く。
- reason に `path` を足す（`{tag}: health:<kind>: <path>: <reason>`）かを ADR で決める。repair WU が直す場所を探す手間が減る。
- 決定的な検査で止まった review にも `policy_mixed` を足すか（health 文脈は検査の前に作っているので、reviewer を起こさない分岐で足せる）を ADR で決める。
- write path が class の path の接頭辞になる場合も合ったとみなすかを ADR で決める。
- D6.2 の対象に profile の `doc_only.paths`（進捗・文書）と検査器の試験を足す変更は、別の policy の branch で行う（review-policy-cli の提案）。
- `--print-review-policy` の出力（`task_core::design_review::ReviewPolicy`）は今 `owner_map`・`high_risk`・`invariants`・`definition_sha256` だけで、`policy_paths` を持たない。`--print-review-policy` が `policy_paths` を出せば、reviewer の無い review では diff の path と照合するだけで policy の変更を決められ、repo health の全体を走らせずに済む（上の未解決の待ち時間が無くなる）。検査器の出力を変えるので、別の policy の branch（D6.6 と同じく人の承認）で行う。
- 材料作りの panic を fail closed にする（`HealthInputJob::attach` が失敗したとき、candidate ごとに不明の `HealthReviewInputSkipped` を残す）変更を、試験用の失敗の注入点と一緒に別の WorkUnit で入れる（policy-no-reviewer の提案）。
- 方針が `Absent` の repo でも、policy の file を変えた candidate を reviewer に知らせる（health 文脈に policy の状態だけを載せる）かを ADR で決める（policy-unknown の提案）。今は人の承認だけで決めている。
