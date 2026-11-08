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
- (5) policy: 候補の profile・検査器は判定に使わない。policy の file と他の file を混ぜた候補は `policy_mixed`（replan）、policy の file だけの候補は reviewer の policy verdict と人の承認（`Approval` の子 task）で通す。
- (6) 試験: `health_review_` の試験で通常変更（追加の run なし・health 文脈だけ）と高 risk 変更（設計 review が起き、blocking で不合格）を流れ全体で区別した（`crates/task-dispatch/src/health_review/scenario_tests.rs`）。architecture-map の「planner/reviewer の起動・判定」と「健全性の review」の行を更新した。
- dispatcher から LLM は呼ばない。新しい review エンジン・新旧切替 flag・config 欄は足していない。

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
- close（この WorkUnit）: 「G2 close: architecture-map の行・ADR の付記・task の進捗」。`docs/architecture-map.md` の 2 行（「planner/reviewer の起動・判定」と「健全性の review」）に関連試験と不変条件（I3・I4・I5・I7・I8・I9）を書き、ADR に付記を足し、この file を書いた。コードは変えていない。

## 人の承認と main への取り込みの経緯
- ADR D6.6 のとおり、`scripts/dev/repo-health/` と `scripts/dev/check-health.sh` を変える U2 は policy の file だけの branch で、人の承認で通した。main の base には D6.2 の扱いがまだ無く、policy の file を変えた branch は必須検査 `test-affected.sh`（先頭で `check-health.sh` を呼ぶ）が `policy.changed` で落ちるためである。
- 前段: profile の `[review]` 節は別 task 01M6VCXJCX の branch で、人が main に merge した（main の件名「integrate 01M6VCXJCX: profile の [review] 節（policy の branch、人の承認済み。ADR health-review-integration D6.6）」）。
- 子 task review-policy-cli（01M6VVB2X5）の 1 回目の run は実装・試験・進捗を commit し、決定 `policy-cli-merge`（推奨は merge）で人の merge を待った。人が main に merge した（main の件名「integrate 01M6VVB2X5: --print-review-policy と D6.2（policy の branch、人の承認済み。ADR health-review-integration D6.6）」）。2 回目の run は main を取り込まずに進捗だけを commit し、`test-affected.sh`・`check-health.sh` が exit 0 になることを確かめて done にした。その後、base 段の統合（「integrate wu/review-policy-cli (phase base)」）でこの task の branch に入った。
- 人の merge で main に先に入った commit は、子 task の branch にあった「G2 review-policy-cli: --print-review-policy と policy の file だけの変更の扱い（D2.3・D6.2・D7.3）」「G2 review-policy-cli: 進捗（実装・検査結果・取り込み待ち）」「G2 review-policy-cli: 進捗の main..HEAD の commit 数を直す」の 3 件。`git merge-base HEAD main` は 3 件目になり、ほかの WorkUnit の commit（Rust を含む）はまだ main に無い（提出時の HEAD で `git log` を確かめた）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
EVIDENCE_PLACEHOLDER

## 未解決
- prompt の 2 点はこの task の範囲外で、まだ無い（prompts task 01M6VTFGWFVE57YXDKHR2E436G の範囲）。提出時の HEAD の `crates/task-worker/src/claude_code/prompt.rs` で確かめた。
  - D3.4: `build_execution_plan_prompt` に brief の書き方と review policy の class 一覧の節が無い。planner は brief を書く指示を受けないので、高 risk の class の path を書く計画は `DesignBriefMissing` の検証失敗と再試行の文で初めて brief を求められる。
  - D5.4: reviewer prompt の Result 節（`## Result`）は旧い形 `{"verdicts":[{"criterion","pass","reason"}]}` だけを指示し、`health`・`design` の形と 7 種の表が無い。reviewer prompt には health 文脈と設計 review の依頼を描く節も無い。reviewer が `health` を書かなければ、`[review]` 節のある repo（Celeris 自身）の final review は I7 で不合格になり、設計 review は `design` が無いので不合格 → 再試行 → 2 回目で `PlanGate` になる。この branch を prompts task より先に本番へ出さない。
- replan の planner の試行が `execution-plan.json` を書かずに終わると、dispatcher は前の試行が残した同じ file を読み、同じ計画を版 2 として採用し直す（scenario で見つけた。試験の adapter は file を消して避けた）。
- 設計 review の run には task 木の token 予算の watch が付いていない。設計 review の待ち・不合格の findings を人に見せる表示（task の待ちの理由・web の計画の節）が無く、`WorkerProgress` と `DesignReviewed` の event だけ。
- planner run に渡す方針は task の先頭の repo の 1 つだけ（`ExecutionPlannerContext.review_policy` が 1 つ）。計画の採用前の判定は repo ごとに読む。
- 計画の write path が class より広い接頭辞（例 `crates/task-core/`）だと、`re.search` の意味で class（例 `^crates/task-core/src/store/`）に合わず、高 risk にならない。
- 本物の `repo_health.py` はこの repo の全体で約 80 秒かかった（health-input の手元の計測 1 回）。その分 reviewer run の起動が遅れる。上限 600 秒は計測 1 回からの見積もり。
- `policy_mixed` は `review.json` を読めた review にだけ足す。`Check::Reviewer` の条件が無い task は health 文脈を作らないので、policy の変更を見つけず人の承認も求めない。
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
