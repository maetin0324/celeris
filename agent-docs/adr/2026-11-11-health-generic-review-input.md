---
tasks: [01M74X0XAN0MCFJ4KZ3VVFT2QA]
---
# ADR 2026-11-11: review 方針の無い（Absent）repo の候補も final reviewer に渡す（health-review-integration D4.4 の改定）
状態: 実装済み（2026-11-11、unit record）。実装は同じ task の unit input-owner・request-atomic・request-integrated。実装との食い違いは末尾の「付記（2026-11-11、unit record）」。

- 日付: 2026-11-11
- task: 01M74X0XAN0MCFJ4KZ3VVFT2QA（G4-R1: review 方針の無い汎用 repo の候補を final reviewer に渡す）。記録は [fix-review-gap の進捗](../progress/2026-11-10-health-v2-g4/fix-review-gap.md)。
- 改定する決定: [ADR 2026-11-07 health-review-integration](2026-11-07-health-review-integration.md) の D4.4（review 方針が `absent` の repo では材料を作らない）と、付記「policy の関門の fail closed」の `Absent` の項。[ADR 2026-11-10 health-generic-fixtures](2026-11-10-health-generic-fixtures.md) の付記（2026-11-11、unit record）の (e)（未解決 G4-R1）を訂正する。
- 土台: [ADR 2026-11-08 health-enforcement-loop](2026-11-08-health-enforcement-loop.md)（J1・J2・D1.1・D1.3。適用判定と検査器の選び方、証跡の束縛）、[ADR 2026-11-07 repo health の契約](2026-11-07-repo-health-contract.md)（D2: policy は base 側で評価、base に profile が無ければ bootstrap か組み込み generic）、[必須検査を候補に束縛する](2026-10-23-required-checks-bound-to-candidate.md)、[reviewer の合格の再利用](2026-10-27-review-resync-halt-visible-and-reviewer-reuse.md)、[破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)、[全体試験は release だけ](2026-10-20-full-tests-at-release-only.md)。
- owner 地図: [docs/architecture-map.md](../../docs/architecture-map.md) の「健全性の review」の行（owner は task-dispatch::health_review、final review の health 文脈の入口は `Dispatcher::health_job`、不変条件 I5・I7・I8）。
- 根拠: G4 の [rs-paths の進捗](../progress/2026-11-10-health-v2-g4/rs-paths.md) の未解決 G4-R1 と、[G4 の進捗](../progress/2026-11-10-health-v2-g4.md) の「提案」1。

## 結論
- review 方針が `Absent`（base の profile に `[review]` 節が無い。profile の無い repo の組み込み generic も同じ）の repo は、方針が無いだけの正常な状態として扱う。final review の材料は `Present` と同じ構築の経路（`HealthInputWork::repo_input`）で作り、`diff`・`findings`・`policy` を reviewer の `ReviewRequest.health` に入れる。`owner_rows` と `invariants` だけを空にする。
- 材料の再利用の鍵は今の `(repo_id, target_sha, reviewed_sha, definition_sha256)` の形のままにする。4 つ目の値は、`Present`・`Absent` のどちらも、適用判定が選んだ検査器の `CheckerId` から作る `task_core::required_checks::definition_digest(&[], Some(&checker))` にする。base の profile は `target_sha` で束縛する。
- 新しい型・event の種類・欄・config 欄・切替 flag は足さない。`absent_policy`・`POLICY_ABSENT` と、`Absent` を `HealthReviewInputSkipped` にする旧い試験と説明は同じ変更で消す。
- `Unavailable`、report の故障、`policy_only`（reviewer run の無い review）、reviewer の合格の再利用、policy の関門（I8）の fail closed は変えない。

## 背景
- G4 の rs-paths は、本物の検査器と isolated Git fixture（ts・rust × dup-contract・dup-state）で final review を流した。候補の finding は report に 1 件ずつ出るが、`HealthReviewInputBuilt` は 0 件で、`HealthReviewInputSkipped` の reason は `POLICY_ABSENT` だった。reviewer は候補を見ずに判定する。
- fixture の profile も組み込み generic も `[review]` を持たない。Celeris 以外の Git repo の多くは同じ状態になる。G4 の目的（Git 管理下の repo 一般に適用する）は、この repo の reviewer に候補が届かないと満たせない。
- 原因は D4.4 が `absent` を remote workspace と同じ「作らない」に入れたこと。remote workspace は candidate の SHA が無いので材料を作れない。`absent` は report も diff も作れる。作れないのは owner 地図の行と不変条件だけで、どちらも方針の値なので、方針が無ければ空が正しい。

## 実装前 brief（汎用規約の 7 項目）
1. 根本原因: `crates/task-dispatch/src/health_review/input.rs` の `HealthInputWork::repo_input` が `ReviewPolicyState::Absent` を `absent_policy` に送る。`absent_policy` は report を走らせて policy の状態だけを `HealthReviewInputSkipped` に残し、diff と findings を reviewer に渡さない。D4.4 がそう定めたことが根の原因。症状は G4 の 4 試験 `health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_reaches_reviewer` が `should_panic` で残ったことと、汎用 repo の reviewer が候補の finding を見られないことに出る。
2. 責務 owner: `health_review::input` の `HealthInputWork::repo_input`（architecture-map の「健全性の review」の行）。材料の形（`task_worker::protocol::HealthRepoInput`）、event（`task_core::Event::HealthReviewInputBuilt`）、方針の読み込み（`health_review::policy::parse_review_policy`）、検査器の選び方（`health_review::gate::with_applicable_checker`）は変えない。
3. 既存の再利用先: `gate::with_applicable_checker`（適用判定と選んだ検査器の `CheckerId`）、`gate::check_with` と `HealthInputWork.evaluated`（同じ candidate で検査器を 1 回だけ起こし、review の health の項目と report を共有する）、`diff_stat`・`summarize_report`・`HealthInputWork::record`・`find_recorded`（`Present` の構築と再利用）、`task_core::required_checks::definition_digest`（health の項目の定義指紋。`RequiredChecksEvaluated` の `definition_sha256` が既に `definition_digest(&[], Some(&checker))` を使う）、`dispatcher::policy_review` の `policy_review_gate`（`HealthReviewInputBuilt.input.policy` と `HealthReviewInputSkipped.policy` を candidate ごとに読む）。試験の土台は `crates/task-dispatch/src/dispatcher/tests/health_generic.rs` の fixture と `configure_real_bundled_checker`、`crates/task-dispatch/src/dispatcher/tests/health_review_input.rs` の偽の reviewer（`HealthReviewer`）。
4. 選択肢:
   - (a) 採用: `Absent` を `Present` と同じ構築に合流し、`owner_rows`・`invariants` を空にする。構築・再利用・report の共有が 1 か所に残り、`Absent` の repo は report をもう走らせているので検査器の追加の起動も無い。
   - (b) 採らない: fixture の profile に `[review]` を足す。profile の無い repo（組み込み generic）には効かず、fixture の builder の変更でもある。
   - (c) 採らない: `Absent` 専用の材料の関数、または protocol に `Absent` 用の欄を足す。構築の二重実装（規約 2）と共通の型への局所の欄（規約 3）になる。
   - (d) 採らない: 組み込み generic に `[review]` を足す。検査器（`scripts/dev/repo-health/health/profile.py`）の変更で、この task では禁止。Celeris 以外の repo に方針を押し付け、D3.3 の risk 判定も変わる。
   - (e) 採らない: `Absent` を `Unavailable`（不明）に寄せる。全ての汎用 repo の候補が人の承認待ちになる（付記「検査器も base の版を使う」の `Absent` の理由と同じ）。
5. 新しい状態・分岐・fallback・port: 無し。新しい event の種類・欄、config 欄、切替 flag は足さない。`ReviewPolicyState`・`HealthRepoInput`・`HealthReviewInput`・`Event`・`TaskStore` は変えない。`repo_input` の中で `Present` と `Absent` が分かれるのは、方針の値（`owner_map`・`invariants`）があるかどうかだけになる。
6. 撤去対象: (R4) の表。
7. 不変条件: (R5) の表。

## 決定

### R1 `Absent` の材料
- R1.1 `ReviewPolicyState::Absent` の repo は、`Present` と同じ `repo_input` の手順で材料を作る: 再利用の鍵で記録を引き（R2）、無ければ `diff_stat`、`run_report`（`gate::check_with`。結果は `evaluated` に入る）、`summarize_report`、`HealthInputWork::record` の順。`HealthReviewInputBuilt` を追記し、その repo を `ReviewRequest.health.repos` に入れる。
- R1.2 `Absent` の `HealthRepoInput` は `owner_rows = []`、`invariants = {}`。方針に owner 地図も不変条件も無いので空にする。候補（head）や作業ツリーの profile から補わない。`diff`・`findings`・`policy` は `Present` と同じ値の作り方（`findings.new` の上限 50 件・`diff.paths` の上限 40 件・`reviewable = false` の扱い・`policy.files`）。`brief` は `Present` と同じく採用した計画の `design_brief`。
- R1.3 wire に `Present` と `Absent` の区別を足さない。reviewer の prompt（`crates/task-worker/src/change_rules/review.rs` の `health_section`）は、空の `owner_rows` と `invariants` を既に「無し」と描く。`HealthPolicyState.source` は report の `policy.source`（`base`・`builtin`・`bootstrap`・`unavailable`）で、review 方針の有無ではない。
- R1.4 I7（D5.2）は `Absent` の repo にも及ぶ: 材料を渡した review で reviewer の `review.json` に `health` が無い・壊れていれば、全ての条件を不合格にする。reviewer が blocking の health の finding を返せば D5.3 のとおり不合格にし、局所修復か replan に写す。候補の finding（`dup.*` の candidate）は report の verdict を変えない（契約 S4）ので、候補を理由に不合格にするのは reviewer の判定だけ。
- R1.5 risk の判定（D3.3）と設計 review は変えない。`Absent` は risk `none`（理由 `review_policy_absent`）のままで、設計 review に回らない。この ADR が変えるのは final review の材料だけ。

### R2 再利用の鍵と失効
- R2.1 鍵の形は D4.3 のまま `(repo_id, target_sha, reviewed_sha, definition_sha256)`。`HealthReviewInputBuilt` の欄も変えない。
- R2.2 4 つ目の値は、`Present`・`Absent` のどちらも `definition_digest(&[], Some(&checker))`。`checker` は `gate::with_applicable_checker` が適用判定（`Applies { checker }`）で選び、`run` に渡す `CheckerId`（base の検査器か同梱の検査器）。値は `celeris.required-checks/3` の domain の sha256 で、`HEALTH_CHECK_ID` と `checker.sha256` を含む。必須検査の health の項目（`RequiredChecksEvaluated`）と同じ値なので、「この candidate をどの版の検査器で評価したか」を 1 つの定義で表す。
- R2.3 `Present` の旧い値（`--print-review-policy` の `definition_sha256`。base の profile の内容の digest）は鍵に使わない。理由: 方針は `--print-review-policy --base <target_sha>` が base の tree（profile が無ければ検査器の組み込み generic）だけから読むので、その digest は `target_sha` と検査器の版で決まる。新しい鍵が一致すれば旧い鍵も一致するので、束縛は弱まらない。旧い鍵には同梱の検査器の版が入っておらず、base に検査器の無い repo で Celeris の release が同梱の検査器を変えても古い材料を使えた。新しい鍵ではこの場合も作り直す。
- R2.4 base の profile は `target_sha` で束縛する。候補（`reviewed_sha`）の profile・baseline・検査器は鍵の値に使わない。候補がそれらを変えても `reviewed_sha` が変わるので、前の材料は使わない。方針は base の版から読むので、候補の profile は `owner_rows`・`invariants`・`high_risk` に入らない。候補が policy の file を変えた report は `policy.changed = true`（base に profile が無い初回の `bootstrap` も含む）になり、D6.4 のとおり人の承認を求める（I8）。
- R2.5 policy の状態が不明（`policy.source = "unavailable"`）の材料は、`Absent` でも再利用しない（付記「そのほかの違い」の D4.3 のまま。`find_recorded` が外す）。
- R2.6 変わったときの扱い:

| 変わるもの | 鍵のどこが変わるか | 結果 |
|---|---|---|
| candidate（head） | `reviewed_sha` | 作り直す |
| base（target） | `target_sha` | 作り直す |
| base の profile（`[review]` の追加・削除・中身、`default.toml` の有無） | `target_sha`（方針と report の policy は target の tree から読む） | 作り直す。`Absent` から `Present` に変われば `owner_rows`・`invariants` が入る |
| base の検査器（base に検査器がある repo） | `target_sha` と `checker.sha256` | 作り直す |
| 同梱の検査器（Celeris の release の更新。組み込み generic の中身の変更を含む） | `checker.sha256` | 作り直す |
| 候補の profile・baseline・検査器 | `reviewed_sha` | 作り直す。値は base の版から作り、`policy.changed = true` で人の承認（I8） |
| 方針を読めない（`Unavailable`）・report を作れない | — | 再利用しない。不明として記録し、次の review で作り直す（今のまま） |
| reviewer run の再試行・stale の再同期の後の再 review（上のどれも変わらない） | 変わらない | 記録した材料を使い、検査器を起こさない（I5） |

- R2.7 検査器の出どころ（`CheckerId.source`）は鍵に入れない。同じ `target_sha` では base に検査器があるかが決まるので出どころも決まり、`sha256` は同じ版なら base でも同梱でも同じ値（`checker_sha256`）。
- R2.8 移行: 本番の events に残る `Present` の旧い鍵の `HealthReviewInputBuilt` は新しい鍵に一致しないので、配備の後に review する candidate ごとに 1 回だけ作り直す。events は書き換えない（追記専用）。

### R3 変えない契約
- R3.1 remote workspace と同期の無い repo は今のまま作らず、`HealthReviewInputSkipped` に理由を残す（candidate の SHA が無い）。D4.4 の残りもそのまま: WU の check・WU の review・tick では作らない。
- R3.2 適用判定が `NotApplicable` の repo は何も渡さず、policy の状態も残さない。`Unavailable`（base にも同梱にも検査器が無い・事実を読めない）は今のまま `CHECKER_UNAVAILABLE` の理由と不明の policy を `HealthReviewInputSkipped` に残す。方針を読めない（`ReviewPolicyState::Unavailable`）も今のまま不明。どちらも policy の関門が人の承認を求める（I8）。
- R3.3 report の故障（report を書かない・時間切れ・壊れた report・schema 違い）は、`Present` と同じく `findings.verdict = "error"`・`policy.source = "unavailable"` の材料を `HealthReviewInputBuilt` に記録し、reviewer に渡す。再利用しない。`Absent` の repo では、これまでの「`HealthReviewInputSkipped` に不明の policy」から「`HealthReviewInputBuilt` に不明の policy」に変わる。関門の扱い（人の承認）は同じ。
- R3.4 `policy_only`（`Check::Reviewer` の無い task）は変えない: 方針を読まず、`record_policy_only` が policy の状態だけを reason `NO_REVIEWER` の `HealthReviewInputSkipped` に残す。
- R3.5 reviewer の合格の再利用（`ReviewerPassReused`）で reviewer を省いた candidate には材料を作らない。関門は元の candidate の記録を見る（`reused_from`）。
- R3.6 policy の関門（`policy_review_gate`）は変えない。`Absent` の candidate の policy の状態は、これまでの `HealthReviewInputSkipped.policy` に代わって `HealthReviewInputBuilt.input.policy` から読まれる。関門はもとから両方を読む。

### R4 撤去する旧い仕組み
| 撤去するもの | 場所 | unit |
|---|---|---|
| `HealthInputWork::absent_policy` と定数 `POLICY_ABSENT`、`repo_input` の `Absent` の早期 return | `crates/task-dispatch/src/health_review/input.rs` | input-owner |
| module doc の「方針が `absent` の repo は `HealthReviewInputSkipped`」「方針が `absent` の repo も report を走らせて決める（reviewer には渡さない）」の説明 | 同上 | input-owner |
| 試験 `health_review_input_policy_absent_is_skipped`（2 件。`Absent` を Skipped と固定する旧い契約の試験） | `crates/task-dispatch/src/health_review/input/tests.rs`、`crates/task-dispatch/src/dispatcher/tests/health_review_input.rs` | input-owner（R5 の試験に置き換える） |
| 試験 `health_review_policy_unknown_absent_policy_change_needs_approval` の「`HealthReviewInputBuilt` を作らない」assert（承認を求める assert は残す） | `crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs` | 計画の範囲外（R6） |
| 4 試験 `health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_reaches_reviewer` の `#[should_panic]` と、`review_input_candidate_reaches_reviewer` の doc comment の G4-R1 の説明 | `crates/task-dispatch/src/dispatcher/tests/health_generic.rs` | input-owner |
| `HealthReviewInputSkipped` の doc comment の「review 方針が `absent` など」 | `crates/task-core/src/model/events.rs` | 計画の範囲外（R6） |
| D4.4 の `absent` の文、付記「policy の関門の fail closed」の `Absent` の項 | [ADR 2026-11-07 health-review-integration](2026-11-07-health-review-integration.md) | contract（この unit。付記で改定を示す） |
| 付記 (e) の G4-R1 の説明 | [ADR 2026-11-10 health-generic-fixtures](2026-11-10-health-generic-fixtures.md) | contract（この unit） |

### R5 不変条件と試験
| 不変条件 | 確かめる試験（prefix） | unit |
|---|---|---|
| `Absent` の repo の材料は `diff`・`findings`・`policy` を持ち、`owner_rows`・`invariants` は空。`HealthReviewInputBuilt` を 1 回だけ記録する | `health_review_input_`（2 件の `…_policy_absent_is_skipped` を置き換える） | input-owner |
| 再利用の鍵は `definition_digest(&[], Some(&checker))`。candidate・base・base の profile・検査器のどれかが変われば作り直し、同じ candidate の再試行では検査器を起こさない。不明の材料は再利用しない | `health_review_input_` | input-owner・request-integrated |
| adapter が実際に受けた `ReviewRequest.health` に、repo・base・head・diff・候補の finding（evidence の path が候補の変更 file）・空の `owner_rows`/`invariants` がある。event や report だけで届いたとみなさない | `g4_r1_review_request_`（ts・rust × dup-contract・dup-state、profile の無い組み込み generic） | request-atomic |
| 候補を見た reviewer が blocking の health の finding を返すと task は Done にならない。good は候補の無い材料を受けて Done になる | `g4_r1_review_request_` | request-atomic |
| WU を統合した後の final review と複数 repo でも同じく届き、片方の候補で不合格になれば全体が Done にならない。WU の check・WU の review では作らない | `g4_r1_review_request_`、既存 `health_review_input_not_built_for_work_units` | request-integrated |
| 候補を見た reviewer の判定で 4 試験 `…_reaches_reviewer` が通常の成功の試験として通る（`should_panic` なし）。evidence の path の assert は残す | `health_generic_review_input_` | input-owner |
| `Unavailable`・report の故障・`policy_only`・reviewer の合格の再利用・policy の関門の fail closed は変わらない | 既存 `health_review_policy_unknown_`・`health_review_policy_no_reviewer_`・`health_review_input_failed_report_is_recorded_as_unknown`・`health_review_input_not_built_when_the_reviewer_pass_is_reused` | input-owner・request-integrated |

- 既存の試験で足りない理由: `health_review_input_` の材料の試験は方針が `Present` の stub の検査器だけで材料を確かめ、`Absent` は Skipped を固定していた。`health_generic_review_input_*_reaches_reviewer` は `HealthReviewInputBuilt` の event を見るだけで、adapter が受けた `ReviewRequest` も reviewer の判定による合否も見ない。

### R6 実装の範囲と計画への影響
- 範囲外の path は回避策を足さず、後続の unit が plan_issue で返す（task の目的のとおり）。この unit の調べで、計画の `expected_write_paths` に無いが変える必要のある path を次に挙げる。
  - `crates/task-dispatch/src/health_review/policy_review/unknown_tests.rs`: `health_review_policy_unknown_absent_policy_change_needs_approval` は「`Absent` の repo では `HealthReviewInputBuilt` を作らない」を assert する。R1 の後はこの assert が落ちる（`health_review_policy_` は input-owner の check に入っている）。
  - 共通の stub の検査器（`crates/task-dispatch/src/health_review/stub_checker.rs`。`dispatcher/tests/mod.rs` が全ての dispatcher 試験に設定する）は `source = absent` の方針と合格の report を返す。R1 の後は、同期した Git repo を持ち `Check::Reviewer` の条件がある dispatcher 試験の final review に `ReviewRequest.health` が渡り、R1.4（I7）により `health` を書かない偽の reviewer の review が不合格になる見込み。静的に見つけた候補は `crates/task-dispatch/src/dispatcher/tests/target_sync.rs`（2 か所）、`crates/task-dispatch/src/dispatcher/tests/health_enforce.rs`（`with_review` の 1 か所）、`crates/task-dispatch/src/dispatcher/tests/work_units.rs`（1 か所）。実際に落ちるかと全件の一覧は未確認（この unit は試験を走らせていない）。input-owner の `bash scripts/dev/test-affected.sh` で確かめる。直し方は偽の reviewer に `"health":{"findings":[]}` を書かせること（`health_generic.rs` の偽の reviewer は既に書く）。I7 を弱める、stub の検査器の方針を変えて `Absent` を避ける、は採らない。
  - `crates/task-core/src/model/events.rs` の `HealthReviewInputSkipped` の doc comment（R4）。path class `events` の file だが、変えるのは説明の文だけ。
  - `crates/campaign/`（dagbench の driver も同じ stub の検査器を設定する）は変更禁止。dagbench の task には `Check::Reviewer` の条件が見当たらない（`crates/campaign/src/dagbench/` を grep）ので、`policy_only` の経路のままで影響は無い見込み（未確認）。
- `docs/architecture-map.md` の「健全性の review」の行は「health 文脈は (repo_id, target_sha, reviewed_sha, definition_sha256) ごとに高々 1 回作り」「`[review]` の無い repo（base に検査器がある）…でも人の承認なしには合格しない」と書いていて、R1・R2 の後も正しい。変えない。

## 本番への移行
- config・DB・検査器・profile・baseline の変更は無い。配備の後、Celeris 以外の Git repo（`[review]` の無い profile か組み込み generic）で `Check::Reviewer` の条件を持つ task の final review は、reviewer に health の材料を渡し、reviewer の `health` の判定を求める。reviewer の prompt の health の節と Result 節は既にある（G2 の prompts task）。
- reviewer の prompt は材料の分だけ長くなる（`findings.new` は 50 件、`diff.paths` は 40 件まで）。費用の増分は未測定（有料の測定をしない）。検査器の起動の回数は変わらない（`Absent` の repo はこれまでも policy の関門のために report を 1 回走らせていた）。
- R2.8 のとおり、配備の時点で review 中の candidate は材料を 1 回だけ作り直す。

## 付記（2026-11-11、unit record）: 実装との食い違い
R1〜R5 は決めたとおりに実装した。`HealthInputWork::repo_input` は `Present`・`Absent` を同じ手順で作り、`absent_policy`・`POLICY_ABSENT` と旧い試験 2 件は消え、4 試験 `…_reaches_reviewer` は `should_panic` の無い通常の試験になった。食い違いは次の (a)〜(e)。証拠は [fix-review-gap の進捗](../progress/2026-11-10-health-v2-g4/fix-review-gap.md) の工程 2〜5。

- (a) R6 の見込みと、実際に落ちて直した試験の違い。R6 は「実際に落ちるかと全件の一覧は未確認」と書いた。input-owner の `test-affected.sh` では 12 件（試験 file 6 個）が落ち、replan で input-owner の範囲に足して直した。

| 試験 file | R6 の見込み | 実際 |
|---|---|---|
| `health_review/policy_review/unknown_tests.rs` | `health_review_policy_unknown_absent_policy_change_needs_approval` の「Built を作らない」assert | その 1 件に加え、`health_review_policy_unknown_candidates_wait_for_a_human` も落ちた（`(Absent, Missing)` の本文に旧い `POLICY_ABSENT` の文を期待していた）。2 件とも新しい契約の assert に替えた |
| `health_review/scenario_tests.rs` | 挙げていない | `health_review_scenario_normal_change` が落ちた（`[review]` の無い repo の final review に材料が無いことを固定していた）。材料が渡ることの assert に替えた |
| `dispatcher/tests/target_sync.rs` | 2 か所 | 5 件（偽の reviewer `ReviewShaAdapter`・`CountingReviewer`）。`"health":{"findings":[]}` を足した |
| `dispatcher/tests/health_enforce.rs` | `with_review` の 1 か所 | 1 件（`review_repair_not_evaluated_reviewer_does_not_block_a_health_repair`）。同じ直し方 |
| `dispatcher/tests/phase_effect_ab/review_sync.rs` | 挙げていない | 2 件（`SyncScenarioAdapter`）。同じ直し方 |
| `dispatcher/tests/phase_effect_ab/stale_priority.rs` | 挙げていない | 1 件（`ReviewAdapter`）。同じ直し方 |
| `dispatcher/tests/work_units.rs` | 1 か所 | 落ちなかった。変えていない。その試験の task は `parallel_task`（案件の repo を持たない workspace だけの task）なので、final review の前の同期が無く、health の材料を作らず I7 も及ばない |

- (b) R4 の `crates/task-core/src/model/events.rs` の `HealthReviewInputSkipped` の doc comment（「review 方針が `absent` など」）は変えていない。task の受け入れ条件と計画の check が `crates/task-core/` の無変更を求めるため。R4 に挙げていなかった `task_core::design_review::ReviewPolicy::definition_sha256` の doc comment（「health の材料の再利用の鍵に入る」。R2.3 で鍵に使わなくなった）も同じ理由で残した。`crates/task-dispatch/src/health_review/verdict.rs` の module doc（「`health` を渡していない review（`[review]` の無い repo、WU の review）」）も、計画の check が `verdict.rs` の無変更を求めるので残した。3 か所とも説明の文だけで動作に影響しない。直す条件と責任者は進捗の「提案」に書いた。
- (c) R4 の表で「計画の範囲外（R6）」とした `unknown_tests.rs` の assert は、replan で input-owner の範囲に入り、同じ task の中で直した。
- (d) R5 の `g4_r1_review_request_` の試験は 17 件になった。atomic の 9 件と、WU の統合後 3 件・複数 repo 2 件（`dispatcher/tests/health_generic.rs`）、`Absent` の reviewer run のやり直し・reviewer の合格の再利用・候補の方針を信用しないことの 3 件（`dispatcher/tests/health_review_input.rs`）。`health_review_input_` の材料の試験は、旧い 2 件を消して単体 4 件と dispatcher の 1 件に置き換えた。R5 の「WU の check・WU の review では作らない」は、既存の `health_review_input_not_built_for_work_units` が材料 0 件でも合格する弱い形だったので、統合後の 3 件で WU の run が `ReviewRequest.health` を受けないことも確かめた。
- (e) 実装中に分かったこと: 案件の repo（`task.repos`）を持たない task の final review は、`HealthReviewInputBuilt`・`HealthReviewInputSkipped` のどちらも残さない（review の前の同期と材料が案件の repo ごとのため）。この ADR の決定（R1〜R3）は変えていない。意図した契約かは未確認で、進捗の「提案」に回した。

## 状態の記録
- 2026-11-11: 提案（unit contract）。実装の後、record が状態を「実装済み」にし、実装との食い違いを付記に書く。
- 2026-11-11: 実装済み（unit record）。R1〜R5 は unit input-owner・request-atomic・request-integrated の commit で実装した。実装との食い違いは上の「付記（2026-11-11、unit record）」。提出時の HEAD の検査は fix-review-gap の進捗の工程 5。
