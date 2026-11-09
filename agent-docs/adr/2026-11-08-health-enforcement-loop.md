---
tasks: [01M6TYVJSWEY3ZE50S02ETYDNM]
---
# ADR 2026-11-08: repo health を通常の開発・統合ループに強制し、証跡を候補に結び付け、修復と費用に上限を置く
状態: 提案

- 日付: 2026-11-08
- 実装: 健全性 v2 G3（task 01M6TYVJSWEY3ZE50S02ETYDNM）の後続 unit（evidence・p9・ci-guide・inventory・loop-points・limits・closeout）。この ADR の unit はコードを変えない。記録は [G3 の進捗](../progress/2026-11-07-health-v2-g3.md)。
- 土台: [ADR 2026-10-23 必須検査を候補に束縛する](2026-10-23-required-checks-bound-to-candidate.md)（`task_core::required_checks`、`Event::RequiredChecksEvaluated`、delivery の `verify_required_checks`）、[ADR-0118](0118-review-target-sync-and-merge-candidate.md)（target・reviewed・merge candidate の SHA）、[ADR 2026-11-07 repo health の契約](2026-11-07-repo-health-contract.md)（report/1、exit 0/1/2、policy は base 側で評価）、[ADR 2026-11-07 健全性を設計と review に組み込む](2026-11-07-health-review-integration.md)（D5.3 の health verdict の写像、D6 の policy の branch）、B0 の [slices.json](../progress/2026-11-07-health-v2-b0/slices.json) の G3。
- 関係: [ADR-0120](0120-pre-review-sync-integration-repair.md)（review 前の同期の衝突の修復）、[段の統合の自動修復](2026-10-29-phase-integration-auto-repair.md)、[ADR-0072](0072-task-execution-decomposition.md)（repair・replan の上限）、[費用の上限](2026-11-06-budget-usd-caps.md)、[ADR-0131](0131-cron-jobs.md)（cron）、[task の冪等キー](2026-10-27-task-idempotency-keys.md)、[ADR-0009 P-9](0009-proposal-adoption-and-phase-closure.md)、[reviewer の合格の再利用](2026-10-27-review-resync-halt-visible-and-reviewer-reuse.md)、[破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)、[全体試験は release だけ](2026-10-20-full-tests-at-release-only.md)。

## 結論
- repo health を当てるかどうかは、1 つの純粋関数 `task_core::required_checks::health::applicability` が repo の事実（Git 管理か・Celeris が統合する作業ツリーか・base に検査器があるか・同梱の検査器があるか）だけで決める。案件名・`self_improve`・selfdeploy・repo 名・task の mode は入力にしない。WU の check・段の統合・task の review（atomic/direct を含む）・review の再利用・review 前の同期・統合の修復の後・delivery の merge candidate の全経路がこの関数を通る。
- base に検査器が無い repo には、Celeris の release に同梱した検査器（`<release>/repo-health/`）を使う。repo に profile が無ければ組み込み generic で評価する（契約 D2）。
- health の結果は既存の必須検査の証跡 `RequiredChecksEvaluated` の 1 項目として残し、repo・base SHA・head SHA・候補の tree・policy hash・検査器の版・report digest に結び付ける。新しい証跡の仕組みや review engine は作らない。report/1 の schema と検査器は変えない。
- 統合を止めて repair に回すのは、決定的な検査の fail 級の finding（severity error、delta new か worsened、例外なし、候補でない）だけ。LLM の finding は根拠（差分の path と決定的な anchor）が無ければ止めない。
- 回数・費用・時間は既存の repair・replan・木・案件の上限の中で数え、新しい上限の欄を作らない。同じ fingerprint の往復は 1 件にまとめて木の root の受信箱へ上げる。error・unknown は合格にせず、使い切りは Blocked と受信箱で止める。
- repo の CI からは同じ検査器を `scripts/dev/repo-health/ci.sh` で呼べる。Celeris は外部からの手動 push を強制できない。
- 全体の棚卸しは既存の cron に新しい種類 `health_inventory` を足し、既定では job を作らない。未変更の repo は省き、同じ debt を重複して起票しない。
- P-9 は案 A（依存先の reopen で `dependency_failed` の後続を戻し、戻せない後続は受信箱で知らせる）を採る。

## 既存の正本と再利用先
| 責務 | 既存の正本 | G3 での扱い |
|---|---|---|
| 必須検査の結果と候補の束縛・再利用・照合 | `crates/task-core/src/required_checks.rs`（`definition_digest`・`reusable_passes`・`verify_required_checks`・`candidate_required_checks`） | 下位 module `required_checks/health.rs` を足し、health を同じ束縛・同じ digest・同じ照合に入れる |
| 必須検査の実行 | `crates/task-dispatch/src/review/required.rs` の `evaluate` | health の項目を同じ列で評価する |
| WU と段の統合の検査の実行 | `dispatcher/phase_integration.rs` の `run_integration_checks`・`integration_checks_for_phase` | health の項目を同じ列に足す |
| base の検査器の取り出しと起動 | `crates/task-dispatch/src/health_review/policy.rs` の `with_base_checker`・`HEALTH_INPUT_TIMEOUT` | 同梱の検査器の場合を足す。起動の口は 1 つのまま |
| health の材料と policy の関門 | `health_review/input.rs`（`health_job`）・`health_review/policy_review.rs`（`policy_review_gate`） | 変えない。reviewer への材料は G2 のまま |
| reviewer の health verdict の写像 | `health_review/verdict.rs` | 根拠の無い blocking を advisory に下げる（D3.3） |
| 候補の SHA | ADR-0118 の `ReviewTargetSynced`・delivery の `target_sha`・`reviewed_sha`・`merge_candidate_sha` | 新しい SHA の仕組みは作らない |
| repair・replan の上限 | `ExecutionConfig.max_repairs`・`max_repairs_per_class`・`max_replans`、段の統合の自動修復、`IntegrationRepair` の上限、`MAX_TARGET_RESYNCS` | health 由来の修復もここで数える |
| 費用・木の上限 | `TreeLimits.max_tree_runs`・`max_tree_usd_micros`、案件の `max_usd_per_day`、task の `max_tokens` | 新しい欄は作らない |
| cron | `crates/task-ops/src/cron_jobs.rs`（`CronJobKind`・`CronInputSink`） | 種類 `health_inventory` を足す |
| 依存の cascade | `crates/task-core/src/store/transition.rs` の `cascade_after_transition_tx` | P-9 の復帰もここに置く |

## 不変条件
- J1: 適用判定は 1 つの関数で、入力に案件名・repo 名・`self_improve`・selfdeploy・task の mode を持たない。全経路がこれを呼ぶ（試験で全経路を確かめる）。
- J2: health の結果は `(repo_id, base_sha, head_sha, head_tree, policy_sha256, checker_sha256)` に束縛され、どれかが変われば合格は再利用されない。
- J3: 検査器と policy は base 側（base に無ければ Celeris の release の同梱）から取る。候補は自分を評価する検査器・policy を変えられない（契約 D2、G2 の I3）。
- J4: exit 2（error・unknown・unsupported）と report を作れない場合は合格にしない。
- J5: 統合を止めるのは決定的な検査の fail 級の finding だけ。LLM の finding は根拠が無ければ止めない。
- J6: health 由来の repair・review・replan は既存の上限の中で数える。上限に達したら Blocked と受信箱で止め、repair を増やさない。
- J7: 棚卸しは同じ debt を 2 回起票しない。起票は draft で、横断の書き換えをしない。
- J8: events は追記だけ（P-9 の復帰も新しい event を足す）。

## 決定

### (1) 適用判定: 1 つの関数と全経路の入口
- 決定: 適用は `applicability` の 1 関数が repo の事実だけで決め、下の表の全経路がそれを呼ぶ。base に検査器が無い repo は release に同梱した検査器を使う。
- D1.1 関数は `task_core::required_checks::health::applicability(facts: &HealthRepoFacts) -> HealthApplicability`（純粋関数。I/O なし）。
  - `HealthRepoFacts { git: bool, integrated_by_celeris: bool, base_checker: Option<CheckerId>, bundled_checker: Option<CheckerId> }`。`CheckerId { source: Base | Bundled, sha256 }`。
  - 事実を集めるのは `task_core::required_checks::health::repo_facts(base_sha, git, bundled)`。`git` は repo で git を実行する closure（`candidate_required_checks` と同じ形）。`git rev-parse --show-toplevel` が通れば Git 管理、`git cat-file -e <base>:scripts/dev/repo-health/repo_health.py` で base の検査器の有無を見る。読み取りの失敗（存在しないことを確かめられない）は `Unavailable`（fail closed）。
  - 結果: `Applies { checker: CheckerId }`（base の検査器があれば Base、無ければ Bundled）、`NotApplicable { reason }`（`not_a_git_repo`・`remote_workspace`）、`Unavailable { reason }`（`checker_missing`: base にも同梱にも検査器が無い、`facts_unreadable:<理由>`）。
  - `remote_workspace` を `NotApplicable` にするのは、remote workspace は Celeris が branch を統合せず（ADR-0079 D5）、delivery の対象にもならず、候補の SHA が無い（G2 D4.4）ため。止める統合が Celeris の中に無い。その repo には (5) の CI の入口を使う。
- D1.2 全経路と入口の関数。どの経路も D1.1 の関数を呼び、`Applies` なら health の項目を必須検査の列に足す。`NotApplicable` は理由を証跡に残して health の項目を足さない。`Unavailable` は health の項目を `error` の結果として足す（合格にしない）。

  | 経路 | 入口の関数（file） | health を当てる所 |
  |---|---|---|
  | atomic/direct の task の review（計画なしの直行経路を含む） | `Dispatcher::spawn_review`（`dispatcher/review_spawn.rs`）→ `review::review_task`（`review.rs`） | `review/required.rs` の `evaluate` が必須検査の列の 1 項目として評価する |
  | WU の check | `Dispatcher::spawn_work_unit_checks`（`dispatcher/work_units.rs`）→ `phase_integration::run_integration_checks` | WU の作業ツリーで、base は `wu.base_commit`、head は WU の HEAD |
  | 段の統合 | `Dispatcher::start_integration`（`dispatcher/phase_integration.rs`）→ `integration_checks_for_phase` → `run_integration_checks` | 統合後の作業ツリーで、base は段の基点、head は統合の HEAD |
  | review の再利用 | `Dispatcher::reviewer_reuse`（`review_spawn.rs`）・`required_checks::reusable_passes` | reviewer を省いても health の項目は (2) の束縛でだけ再利用し、束縛が違えば評価し直す |
  | review 前の main 同期・rebase | `spawn_review` の `ReviewSync`（ADR-0118）・`defer_stale_review` | 同期の後の `(target_sha, reviewed_sha)` で評価する。同期で head が変われば前の証跡は失効する |
  | 統合の修復の後 | `IntegrationRepairScheduled`（`dispatcher/review_verdict.rs`）・`schedule_integration_check_repair`・`schedule_merge_repair`（`phase_integration.rs`）・配送の局所修復 | 修復の後は上の review か段の統合の経路に戻り、新しい head で評価する。修復の前の証跡は使わない |
  | delivery の merge candidate | `celeris::delivery::check_required_evidence`（`crates/celeris/src/delivery.rs`）→ `required_checks::verify_required_checks` | D1.1 を同じ事実で呼び、`Applies` なら health の項目の証跡を必須にする |

- D1.3 実行の口は 1 つ: dispatcher 側は `health_review::gate::run_health_check(repo, base, head, checker, timeout)`（新規。`health_review/policy.rs` の `with_base_checker` を同梱の場合に広げて使う）。WU・段の統合・review は同じ関数を呼ぶ。経路ごとに検査の起動を書かない（slices.json の G3 `remove`）。
- D1.4 検査器の所在: base に `scripts/dev/repo-health/repo_health.py` があれば base の版（G2 の付記「検査器も base の版を使う」）。無ければ Celeris の release に同梱した `<release>/repo-health/`。release.sh は、build した commit の `scripts/dev/repo-health/` から `profiles/` を除いた file（`repo_health.py`・`health/`・`rules/`・`schema/`・`validate_report.py`）と `scripts/dev/source-size-report.py` を `<release>/repo-health/` に写し、その commit の tree id を `<release>/repo-health/CHECKER_TREE` に書く。daemon は実行中の binary の release dir（`browser_ledger_path` と同じ求め方）から探す。`profiles/` を写さないのは、Celeris の profile（`celeris.toml`）を他の repo に当てないため。profile は対象 repo の base にあればそれ、無ければ組み込み generic（契約 D2）。
- D1.5 Celeris の repo では `workspace.toml` の `test-affected.sh` が `check-health.sh`（作業ツリーの評価）を先に呼ぶ。D1.2 の health の項目（commit の評価）と入力が違うので、両方を残す。所要時間の増分は limits unit が計測し（slices.json の `baseline_metrics` の `edit_verify_secs`）、進捗に書く。
- 採らなかった: 経路ごとに「health を走らせるか」を判定する案。WU・統合・review・delivery で条件が食い違い、抜け道ができる（G2 の final review が見つけた穴と同じ形）。
- 採らなかった: base に検査器が無い repo を対象外にする案（G2 の今の扱い）。Celeris 以外の repo には health が一度も当たらず、親 task の「Git 管理下のソフトウェア repo 一般に適用する」に反する。
- 採らなかった: 検査器を binary に埋め込む案。Python の file 群を build.rs で埋めて展開する仕組みが新しく要る。release dir への同梱は `browser/conformance.json` と同じ置き方で足りる。
- 採らなかった: task の mode（prototype）で外す案。ADR-0046 D4 は `workspace.toml` の必須検査を外すだけで、health は repo の構造の新しい悪化を見る別の検査。mode で外すと prototype の task で delivery の抜け道になる。

### (2) 証跡: 候補への結び付け・再利用・失効
- 決定: health の結果は既存の必須検査の証跡の 1 項目にし、base SHA・head SHA・候補の tree・policy hash・検査器の版・report digest に結び付ける。値は Celeris 側で計算し、report/1 と検査器は変えない。
- D2.1 型は `task_core::required_checks::health::HealthEvidence { repo_id, base_sha, head_sha, head_tree, policy_sha256, checker: CheckerId, report_sha256, tool_version, verdict: Pass | Fail | Error | PolicyOnly | NotApplicable, blocking: Vec<String>, reason: Option<String>, reused_from: Option<String> }`。`blocking` は fail 級の finding の fingerprint（sort 済み、上限 50 件、超えた数は `reason` に書く）。
- D2.2 置き場所は既存の `PhaseCheckResult` に足す任意欄 `health: Option<HealthEvidence>`（serde default）。health の項目の `cmd` は固定の識別子 `celeris:repo-health/1`（定数 `HEALTH_CHECK_ID`）。review の項目は `RequiredChecksEvaluated.results[]` に、WU・段の統合の項目は `WorkUnitCheckFinished`・`IntegrationCheckFinished` に同じ任意欄 `health` を足して残す。新しい Event の種類は足さない。
- D2.3 値の出どころ（report/1 の schema と検査器は変えない。Celeris 側で計算する）:
  - `head_tree`: `git rev-parse <head_sha>^{tree}`。
  - `checker.sha256`: base の版は `git rev-parse <base>:scripts/dev/repo-health` の tree id と `scripts/dev/source-size-report.py` の blob id を domain `celeris.repo-health.checker/1` で sha256。同梱の版は `CHECKER_TREE` の値から同じ式で計算する（同じ commit の検査器なら base でも同梱でも同じ値）。
  - `policy_sha256`: domain `celeris.repo-health.policy/1` で、report の `policy.version`・`policy.source`・`inputs.profile.resolved`・`inputs.profile.source` と、base の `scripts/dev/repo-health/profiles` の tree id（無ければ `builtin`）と report の `policy.files[].base_blob` を順に入れた sha256。policy は base 側で評価されるので、同じ base_sha なら同じ値になる。base_sha が違っても policy が同じことを (6) の棚卸しの省略に使う。
  - `report_sha256`: report の file の byte 列の sha256（契約 D8 の決定性）。
- D2.4 `definition_digest` に health の項目を入れる: `Applies` なら必須検査の cmd 列の後に `HEALTH_CHECK_ID` と `checker.sha256` を長さ付きで足す（domain は `celeris.required-checks/3` に上げる。旧い証跡はすべて失効し、昇格の後の配送は 1 回だけ再 review になる）。これで検査器が変われば ADR 2026-10-23 D3 の「定義指紋が違えば失効」に乗る。
- D2.5 再利用: 同じ `(repo_id, base_sha, head_tree, policy_sha256, checker.sha256)` で verdict が Pass の証跡があり、`summary.reviewable == true` の report から作られていれば、検査器を走らせず結果を写す。写した証跡は今の `(base_sha, head_sha)` に束縛して新しく記録し、`reused_from` に元の head_sha を書く（同じ tree の別 commit。例: 単一の WU の統合の HEAD）。Fail・Error・PolicyOnly は再利用しない（不合格は再利用しない。ADR 2026-10-23 D3）。
- D2.6 失効: base_sha・head_tree・policy_sha256・checker.sha256 のどれかが違えば再利用しない。delivery の照合（`verify_required_checks`）は今の `target_sha`・`reviewed_sha` に束縛された証跡だけを見る（今のまま）。加えて、health の項目の `head_tree` が `reviewed_sha` の tree と一致し、`checker.sha256` が今の適用判定の検査器と一致し、verdict が Pass（または PolicyOnly で G2 の `policy_review_gate` の承認がある）ことを確かめる。
- D2.7 作業ツリーが dirty なまま走った WU の check の report（`reviewable == false`）は、その WU の合否には使うが、再利用と delivery の証跡には使わない。
- 採らなかった: 新しい Event の種類 `HealthEvaluated` を全経路に足す案。必須検査の束縛・照合と並ぶ 2 つ目の証跡になり、delivery が 2 つを突き合わせることになる。
- 採らなかった: report/1 に policy hash・検査器の版の欄を足す案。契約と検証器・examples をすべて直すことになり、検査器を変える branch は policy の branch（G2 D6）になる。値は base の blob と report の既存欄から決まる。
- 採らなかった: G2 の `HealthReviewInputBuilt` を証跡にする案。reviewer に渡す材料の記録で、reviewer の無い review・WU・段の統合では作られない。

### (3) 修復: 決定的な違反だけが統合を止める
- 決定: 決定的な検査の fail 級の finding（new か worsened の error）だけが統合を止めて既存の repair に回る。error は repair に回さず Blocked、根拠の無い LLM の finding は止めない。
- D3.1 health の項目の判定: report の exit 0 は Pass。exit 1 で fail 級の finding が `policy.changed` だけで変更 path が全部 `policy.files` にあれば PolicyOnly（G2 D6.2 と同じ判定。WU・段の統合では合格扱いで先へ進め、task の review で G2 の `policy_review_gate` が人の承認を求める）。それ以外の exit 1 は Fail。exit 2・report を書かない・時間切れ・壊れた report は Error。
- D3.2 Fail は統合を止めて既存の repair に回す。WU の check なら WU の repair（ADR-0072 D16）、段の統合なら `schedule_integration_check_repair`（段の統合の自動修復）、task の review なら `FailedCheck { check: Command { cmd: HEALTH_CHECK_ID }, reason, repair_hint }` として `classify_review_failure` に渡す。reason は `health:<rule_id>:<fingerprint>[,…]` と evidence の path で、`repair_hint` は `RepairClass` の bucket `health:<rule_id>` を持つ（rule ごとに `max_repairs_per_class` を数えるため）。修復の範囲は既存の `RepairScope`（task の許可範囲）のまま。範囲外の path が要るなら `plan_issue` で replan（slices.json の stop_conditions と同じ）。
- D3.3 LLM の finding: reviewer の `ReviewOutput.health` の blocking の finding は、`path` が候補の差分（`HealthRepoInput.diff.paths`）にあり、かつ決定的な anchor（report の fingerprint、`owner_rows` の行、`brief.removals` の項目のどれか）を `reason` で挙げているときだけ G2 D5.3 のとおり写す。根拠の無い blocking は `health_review/verdict.rs` が advisory に下げ、reason に `health:ungrounded` を付けて残す。LLM の finding で段の統合・WU の check を止めることはない（reviewer は task の review でだけ走る）。
- D3.4 Error は repair に回さない。repair で直せない環境の原因（`tool_missing`・`base_unreachable`・検査器の例外）が多いため。経路は止め、task を Blocked にし、受信箱に `health_check_error: <reason>` を 1 件出す。人が環境を直して comment か retry で再開する。
- 採らなかった: warning・候補（dup.*・link.unlinked・migration.remnant）でも止める案。契約 D3・D6 で合否に使わないと決めた静的推定で、LLM の好みと同じく誤検出で統合を止める。
- 採らなかった: Error を 1 回の局所修復に回す案。環境の失敗を code の修復 run で直そうとして費用だけを使う。

### (4) 上限: round・再試行・費用・時間と同じ指摘の集約
- 決定: 回数・費用・時間は既存の上限の中で数え、新しい欄を作らない。同じ fingerprint の往復は木の root の受信箱に 1 件にまとめ、使い切りは Blocked で止める。
- D4.1 新しい上限の欄・config は作らない。health 由来の修復と再 review は次の既存の上限で数える: task の `max_repairs`（既定 3）・`max_repairs_per_class`（既定 2、class は `health:<rule_id>`）・`max_replans`（既定 3）、段の統合の自動修復の上限、`IntegrationRepair` の上限、`MAX_TARGET_RESYNCS`、CoS の `review_rework`（上限 2 回）、木の `max_tree_runs`・`max_tree_replans`・`max_tree_usd`、案件の `max_usd_per_day`、task の `max_tokens`。
- D4.2 時間: 検査器の 1 回の実行は全経路で `HEALTH_INPUT_TIMEOUT`（600 秒）。時間切れは Error（D3.4）。同じ束縛の評価は (2) の再利用で 1 回だけなので、所要時間の合計は「評価の回数 × 600 秒」以下で、評価の回数は D4.1 の round の上限で決まる。run の時間は既存の `max_wall_secs` のまま。
- D4.3 同じ指摘の往復: 修復の後の評価で、修復の前と同じ fingerprint の集合が残る、または前の修復で消えた fingerprint が再び現れたら、それ以上 repair を起こさない。task を Blocked にし、木の root（root でなければ自分）の受信箱に 1 件だけ出す。項目の鍵は `(root task, fingerprint の集合の sha256)` で、別の leaf が同じ集合に当たっても 1 件にまとめる。既存の自動 replan の停滞検出（同じ WU・同じ失敗）と同じ判定に fingerprint の集合を使う。
- D4.4 使い切り: D4.1 のどれかに達したら、既存の上限の扱い（Blocked・受信箱・木の上限の決定の要求）のまま止める。health のために repair・replan・子 task を新しく足さない。error・unknown は合格に変えない（J4）。
- 採らなかった: health 専用の `max_health_repairs` などの欄。既存の上限と二重になり、どちらで止まったかが分からなくなる。
- 採らなかった: 往復を検出したら自動で replan する案。同じ fingerprint の往復は計画の誤りか検査器の誤りで、replan を重ねても費用が増えるだけ。人か CoS が決める。

### (5) CI の入口と、強制できる範囲
- 決定: `ci.sh` は repo の CI から同じ検査器を呼ぶ入口（policy の file）で、Celeris の証跡にはしない。Celeris は外部の手動 push を強制できず、それは forge の branch 保護が受け持つ。
- D5.1 `scripts/dev/repo-health/ci.sh`（unit ci-entry）は repo の CI から同じ検査器を呼ぶ薄い入口。`repo_health.py --base <merge-base> --head <commit> --policy-version 1` を走らせ、report の path を出し、exit code（0・1・2）を返す。policy の file だけの変更は G2 D6.2 と同じ扱いにする。Celeris の内部の状態・API は読まない。
- D5.2 ci.sh は検査器と同じ dir にあるので policy の file。unit ci-entry は G2 の前例どおり policy だけの branch にし、人の承認の後に operator が main に merge する（計画の承認の回答）。
- D5.3 Celeris は CI の結果を証跡にしない。CI は Celeris が束縛した候補を見ていないので、Celeris は (2) の証跡を自分で作る。
- D5.4 Celeris が強制できるのは、Celeris が統合する経路（D1.2）だけ。人や外部の道具が main に直接 push した変更は止められない。それを止めるのは repo の forge の branch 保護（ci.sh を必須の status check にする）で、repo の管理者が設定する。導入手順は `docs/guides/repo-health-ci.md`（unit ci-guide）に書く。直接 push で入った debt は (6) の棚卸しが後から見つける。
- 採らなかった: Celeris が forge の CI の結果を読んで delivery の証跡にする案。forge ごとの API と認証が要り、CI が見た commit と候補の一致を Celeris が確かめられない。
- 採らなかった: git の pre-push hook を配る案。client 側で外せるので強制にならない。

### (6) 棚卸し: 既存 cron の任意・低頻度の job
- 決定: 既存の cron に種類 `health_inventory` を足す。既定では job を作らない。差分検査・未変更 repo の省略・冪等キーによる重複起票の防止・draft だけの起票にする。
- D6.1 `crates/task-ops/src/cron_jobs.rs` に種類 `health_inventory`（`extra.kind = "health_inventory"`、`extra.project_id` 必須、重ね掛けは `skip` だけ、task の欄は書けない）を足す。`project_reconcile` と同じく task も chat も作らず、`CronInputSink` の新しい method で dispatcher に仕事を積む。dispatcher は LLM を呼ばず、検査器の実行は tick の外（`spawn_blocking`）で行う。
- D6.2 既定では job を作らない（人の計画の承認の回答）。有効にするのは人で、手順は本番への移行の節に書く。推奨の周期は週 1 回。
- D6.3 差分検査: 案件の Git の repo ごとに既定の branch の HEAD を読み、前回の棚卸しの HEAD を base にして (1)・(2) と同じ関数で評価する。初回は base = head で、全部 `existing` になるので数だけを記録して起票しない。
- D6.4 未変更 repo の省略: 前回の記録と `(head_tree, policy_sha256, checker.sha256)` が同じ repo は検査器を走らせず、省いた数だけを記録する。
- D6.5 記録: 新しい表 `health_inventory_runs`（migration。追記だけ。列は job id・予定時刻・repo id・`HealthEvidence` の JSON・起票した task id）。cron の履歴 `CronJobRun` には repo の数・評価・省略・起票の数を `detail` に書く。結果の型は (2) の `HealthEvidence` と同じで、別の result の形を作らない。
- D6.6 重複起票の防止: 起票するのは fail 級の finding（severity error、delta new か worsened、候補でない）だけ。1 件の finding に 1 件の draft task を、冪等キー `health-debt:<repo_id>:<fingerprint>:<head_sha>` で作る（[task の冪等キー](2026-10-27-task-idempotency-keys.md)）。同じ repo と fingerprint のキーを持つ終端でない task があれば起票しない。base が前回の HEAD なので、残り続ける debt は `existing` で、再び起票されない。1 回の発火の起票は 3 件まで、残りは数を `detail` に書く。
- D6.7 横断の書き換えをしない: 起票は draft（人か CoS が承認して始まる）。objective は 1 件の finding の evidence の path に限り、計画・子 task・複数の debt をまとめた書き換えを作らない。候補の finding（dup.* など）は数えるだけで起票しない。
- 採らなかった: 雛形から LLM の task を定期に作る案（`CronJobKind::Task`）。repo が変わっていなくても毎回 run の費用がかかり、同じ debt を毎回起票しうる。
- 採らなかった: 結果を `CronJobRun.detail` の自由文だけに残す案。履歴の 1 行は発火の時点で書かれ、検査の結果より先にできる。repo ごとの省略の判定に型の無い文を読むことになる。

### (7) P-9: 依存先の reopen と `dependency_failed` の後続（決定 p9-scope）
- 決定 p9-scope: 案 A（後続を戻し、戻せない後続は受信箱で知らせる）。詳細は下の D7.1〜D7.4。
- 今の動き: `cascade_after_transition_tx` は failed・cancelled への遷移で終端でない後続を `Trigger::DependencyFailed` で cancel する。`from.is_terminal()` なら何もしないので、依存先を reopen しても後続は戻らない。戻すのは `retry_task_tx` の依存の付け替え（cancelled で最後の遷移の理由が `dependency_failed` の後続を draft へ）だけ。
- 選択肢:
  - A: reopen（終端から終端でない状態への遷移）の同じ transaction で、`dependency_failed` で cancel された後続を戻す。戻せない後続は受信箱で知らせる。
  - B: 後続は戻さず、reopen のときに受信箱へ「後続 X が `dependency_failed` で cancel のまま」と 1 件知らせる。人か CoS が retry する。
  - C: 今のまま。親の closeout で cancelled の子を点検する手順だけを置く。
- 決定 p9-scope: A。B0 の ADR（[健全性 v2 の基準](2026-11-07-health-v2-baseline.md)）が推奨案 A として G3 に入れた範囲。
  - D7.1 `cascade_after_transition_tx` に `from.is_terminal() && !to.is_terminal()` の枝を足す（依存の cascade は 1 か所。slices.json の `consolidate_into_owner`）。戻す対象は、cancelled で最後の遷移の理由が `dependency_failed`、cancel の後に人・CoS の遷移が無く、`depends_on` に reopen した task を含む後続。`Cancelled → Ready` の遷移を新しい `Trigger::DependencyReopened`（理由 `dependency_reopened`）で追記する。依存の待ちは `depends_on` が今までどおり扱う。推移的に戻す（cascade で cancel された後続の後続も同じ条件で戻す）。
  - D7.2 対象の判定は `retry_task_tx` の付け替えと同じ述語（`transition.rs` の 1 つの関数）を使う。retry は新しい task が draft で始まるので、付け替えた後続も今までどおり draft に戻す。
  - D7.3 条件に合わない後続（cancel の後に人が触った、別の理由の cancel）は戻さず、reopen した task に受信箱の項目を 1 件出す（後続の id と cancel の理由）。
  - D7.4 events は書き換えない。戻す遷移も知らせも新しい event の追記。
- 採らなかった B: 後続が止まったまま人の操作を待つ。v2 の R3→R4 のように依存でつないだ子が、reopen のたびに人手の retry を要する。
- 採らなかった C: 点検を忘れると後続が黙って止まる（B0 の task-history の事例）。
- 付記（2026-11-08、unit p9）: 採った案 A の実装と、D7.1〜D7.4 との突き合わせ。人の回答（決定 p9-scope）は「後続を pending に戻し、受信箱にも記録する。戻した後続は受信箱の項目に task id と依存先を書き、後継を手で起票済みのときに人が重複を cancel できるようにする」。
  - D7.1 の実装: `cascade_after_transition_tx` の先頭に `from.is_terminal() && !to.is_terminal()` の枝を置いた。終端から戻る遷移は `Trigger::Reopen`（done・failed → ready）、`Trigger::Rereview`（done・failed → reviewing）、`Trigger::DependencyReopened` の 3 つで、どれもこの枝を通る。枝は `depends_on` にこの task を含む後続ごとに、状態・最後の遷移・ほかの依存先の状態を読み、純粋関数 `transition::dependency_reopen_trigger` が返す遷移を適用する。関数は、後続が下の述語に合い、ほかの依存先に failed・cancelled（または見つからない task）が無いときだけ `Trigger::DependencyReopened` を返す。戻した後続の遷移が同じ枝を通るので、推移的に戻る。ほかの依存先がまだ failed の後続は、その依存先が戻ったときに戻る。
  - D7.1 との違い: 戻す先は「cancel の前が draft なら draft、それ以外は ready」にした（`DependencyReopened { draft }`）。人がまだ受け入れていない draft を、依存先の reopen だけで ready にしないため。人の回答の「pending」は、依存先の完了を待つ ready（`ready_tasks` が依存先の done を待つ）として実装した。
  - D7.2 の実装: 述語は `crates/task-core/src/transition.rs` の `cancelled_by_dependency` の 1 つで、cancelled かつ最後の `Transitioned`（`SqliteStore::last_transitioned_tx` が読む `(from, reason)`）の理由が `dependency_failed` なら cancel の前の状態を返す。`retry_task_tx` の付け替えも同じ関数を使う。cancelled から出る遷移は retry の付け替えと `DependencyReopened` だけで、どちらも最後の遷移の理由を変えるので、「cancel の後に人・CoS の遷移が無い」はこの述語に含まれる。人・CoS の cancel（理由 `cancel`）と連鎖の中止（`parent_cancelled`・`project_cancelled`・`milestone_cancelled`）は戻さない。
  - D7.4 の実装: 戻す遷移は `Transitioned { from: cancelled, to: ready | draft, reason: "dependency_reopened" }` の追記だけで、cancel の event は変えない。attempts は据え置き（replay の畳み込みとも合う）。試験は `crates/task-core/src/store/dependency_reopen_tests.rs` の `dependency_reopen_` 5 件。
  - D7.3 と人の回答の受信箱の項目: unit p9 では未実装。新しい受信箱の種類は `crates/task-api/src/inbox_notifications.rs` の網羅の match と `web/features/inbox/inbox-model.ts` の `KIND_LABELS`（`Record<InboxKind, string>`）に、項目の正本に新しい event を足すなら `crates/task-api/src/query.rs` の網羅の `event_type_name` に行が要る。どれも unit p9 の範囲 check の外なので、計画の問題（`yield.plan_issue`）として範囲の拡張を申告した（記録は [p9 の進捗](../progress/2026-11-07-health-v2-g3/p9.md)）。
  - 置き場所: 判定は `crate::transition` の純粋関数 2 つ、読み出しと遷移の適用は `cascade_after_transition_tx` の枝の中に置いた。新しい module を作らないので、store との循環の節点は 61 のまま（`deps.cycle` は existing）。store の行数と `SqliteStore` の method 数の既存の超過は増やしていない。失敗の伝播と retry の付け替えが別々に持っていた後続の走査（`non_terminal_dependents_tx`・`dependents_of_tx`、同じ `LIKE` 走査）を `dependents_of_tx(…, non_terminal)` の 1 つにまとめ、最後の遷移の読み出しは `last_transitioned_reason_tx` を `last_transitioned_tx`（`(from, reason)`）に置き換えた（browser の前提 gate の呼び出しも同じ関数を使う）。
  - 残る制約: 人が後続を手で retry して後継を作った後でも、元の後続は `dependency_failed` の cancel のままなので、依存先の reopen で戻る。retry の記録（`Retried`）は新しい task の側にあり、見分けるには events の全走査が要る。人の回答どおり、受信箱の項目で重複を cancel してもらう。

### (8) write_set: slices.json の G3 に足す path と理由
- 決定: slices.json の G3 の write_set に下の表の path を足す（slices.json 自体は変えず、各 unit の範囲 check で許す）。
B0 の G3 の `write_set` に次を足す。足す作業は unit closeout が slices.json を変えずに進捗に書き、範囲 check は各 unit の計画の check で許す（slices.json は B0 の成果で、G3 の branch では変えない）。

| 足す path（正規表現） | 理由 | 使う unit |
|---|---|---|
| `^crates/task-core/src/required_checks(\.rs\|/[a-z_]+\.rs)$` | 束縛の正本の拡張（`health` の下位 module、`definition_digest` の v3、`verify_required_checks`） | evidence |
| `^crates/task-core/src/model/events\.rs$` | `PhaseCheckResult`・`WorkUnitCheckFinished`・`IntegrationCheckFinished` の任意欄 `health` | evidence |
| `^crates/task-core/src/cron\.rs$`・`^crates/task-ops/src/cron_jobs(\.rs\|/[a-z_]+\.rs)$` | 種類 `health_inventory` と sink の method | inventory |
| `^crates/task-core/src/store/[a-z_]+\.rs$` | `health_inventory_runs` の読み書き（既存の `store/(transition\|tasks\|…)` の外） | inventory |
| `^crates/task-dispatch/src/health_review/[a-z_]+(\.rs\|/[a-z_]+\.rs)$` | 実行の口 `gate::run_health_check`、同梱の検査器、verdict の根拠の判定（D3.3） | evidence・loop-points・limits |
| `^crates/task-dispatch/src/review(\.rs\|/[a-z_]+\.rs)$` | `review_task` と `review/required.rs` の health の項目（今の write_set は `review/required` だけ） | loop-points |
| `^crates/task-dispatch/src/dispatcher/(review_spawn\|worker_finish\|tree_units\|cos_chat/review)\.rs$` | 再利用・同期・修復の後の経路、上限と往復の集約、cron の sink | loop-points・limits・inventory |
| `^crates/task-ops/src/(inbox\|human_inbox)(\.rs\|/[a-z_]+\.rs)$` | 往復の集約（D4.3）と P-9 の知らせ（D7.3）の受信箱の項目 | limits・p9 |
| `^crates/celeris/src/daemon/[a-z_]+\.rs$` | 同梱の検査器の path を release dir から求める（`browser_ledger_path` と同じ形） | evidence |
| `^scripts/selfdeploy/release\.sh$`・`^scripts/selfdeploy/tests/[a-z0-9_.-]+$` | 検査器を `<release>/repo-health/` に同梱する | evidence |
| `^docs/guides/(repo-health-ci\|source-health)\.md$` | CI の導入手順と、健全性の guide からの参照 | ci-guide |
| `^docs/api/v1/[a-z0-9_.-]+$`・`^web/api/generated/[a-z0-9_.-]+$` | event と cron の欄の追加に伴う API schema と web の生成型の再生成 | evidence・inventory |
| `^docs/architecture-map\.md$` | close-out で地図に 1 行（地図は 214 行で 200 行の目安を超えているので 1 行まで） | closeout |

- `scripts/dev/repo-health/ci.sh` とその試験は G3 の branch でなく policy の branch（unit ci-entry）に置く（D5.2）。
- 採らなかった: slices.json の G3 の `write_set` を G3 の branch で書き換える案。B0 の成果は計測の基準で、slices-check.py の入力でもある。基準を変える branch は G1 の契約で policy の変更と同じ扱いになる（`capability-map.json` は Celeris の profile の `policy_paths` にある）。

## 後続 unit の path・試験名の prefix
- evidence: `required_checks/health.rs`（`applicability`・`repo_facts`・`HealthEvidence`・`HEALTH_CHECK_ID`）、`definition_digest` の v3、`verify_required_checks` の health の照合、`health_review/gate.rs`、同梱の検査器（release.sh と daemon の path）。試験 prefix `health_evidence_`（例 `health_evidence_head_change_invalidates_pass`・`health_evidence_base_change_invalidates_pass`・`health_evidence_policy_change_invalidates_pass`・`health_evidence_checker_change_invalidates_pass`・`health_evidence_same_tree_reuses_pass`・`health_evidence_applicability_ignores_project_and_repo_name`）。
- p9: `store/transition.rs`・`store/tasks.rs`・`transition.rs`。試験 prefix `dependency_reopen_`。
- ci-guide: `docs/guides/repo-health-ci.md`。
- inventory: `cron_jobs.rs`・`cron.rs`・migration・store・dispatcher の sink。試験 prefix `health_inventory_`（例 `health_inventory_skips_unchanged_repo`・`health_inventory_does_not_file_same_debt_twice`・`health_inventory_files_drafts_only`）。
- loop-points: D1.2 の全経路の配線。試験 prefix `health_gate_path_`（`direct_review`・`work_unit_check`・`phase_integration`・`reviewer_reuse`・`pre_review_sync`・`integration_repair`・`delivery` の 7 件以上）。
- limits: D3・D4。試験 prefix `health_limits_`（例 `health_limits_fail_routes_to_repair`・`health_limits_error_blocks_without_repair`・`health_limits_ungrounded_llm_finding_does_not_block`・`health_limits_same_fingerprint_escalates_once`・`health_limits_repair_cap_blocks`）。
- closeout: この ADR の状態と付記、地図の行、task の進捗、提出時の検査。

## 本番への移行
- release: 同梱の検査器は、この変更を含む最初の release から `<release>/repo-health/` に入る。それより前の release の daemon は base に検査器の無い repo を `Unavailable(checker_missing)` にするので、evidence と loop-points は同じ release に入れる（release は main の HEAD から作るので、統合の後なら同じ release に入る）。
- 必須検査の証跡: `definition_digest` の domain を v3 に上げるので、昇格の時点で `MergeQueued` の配送は証跡が無くなり、1 回だけ再 review になる（ADR 2026-10-23 の移行と同じ）。config・既存の表の変更は無い。新しい表 `health_inventory_runs` は migration で足す。
- 棚卸しの job: 既定では作らない。人が有効にするときは、Web の cron の画面で job を追加し、種類 `health_inventory`・対象の案件・周期（週 1 回）・重ね掛け `skip` を選んで有効にする（正確な画面の操作は unit inventory が docs に書く）。止めるときは同じ画面で無効にする。
- CI: repo の管理者が `docs/guides/repo-health-ci.md` の手順で CI に ci.sh を足し、forge の branch 保護で必須の check にする。Celeris の config は変えない。

## 状態の記録
- 2026-11-08: 提案。G3 の unit adr で書いた。実装と食い違いが出たら close-out で付記に書く。
- 2026-11-09: 付記（loop-points、task 01M6ZDF9Q4BWD9DX6S9KX3KKS1）。D1.2 の 7 経路を配線した（[記録](../progress/2026-11-07-health-v2-g3/loop-points.md)）。(a) 試験名の prefix は task の目的文の `health_enforce_` で、「後続 unit の path・試験名の prefix」節の `health_gate_path_` は使っていない。(b) D3.3 の根拠の判定は limits でなく loop-points（unit grounding、`health_review/verdict.rs`、試験 `health_enforce_llm_finding_`）が実装した。task の目的文に合わせ、候補の差分の path と行番号の引用も根拠にする。(c) delivery は PolicyOnly の証跡を通さない（core-binding の提案 1 の決着）。D2.6 の「G2 の承認があれば通す」は、承認を `(repo, target_sha, reviewed_sha)` に結び付ける event が無いので実装していない。状態は closeout が改める。
- 2026-11-09: 付記（limits、task 01M70C8NHEANARRKZD9RGKZ87D）。D3.4・D4 を WU の check・段の統合・task の review に配線した（[記録](../progress/2026-11-07-health-v2-g3/limits.md)）。(a) 同じ指摘の往復（D4.3）は、源の task を Blocked（質問 `health_findings_repeated: <位置>: …`）にし、木の root に event `HealthFindingsRepeated`（種類名 `health_findings_repeated`）を追記する。受信箱の種類 `health_repeat` が `(root, set_sha256)` ごとに 1 件を出す。`set_sha256` は task-core の `health_repeat::fingerprint_set_sha256`（`Event::health_findings_repeated` が呼ぶ）の 1 か所が求め、往復の判定（`health_review::limits`）は fingerprint の集合だけを返す。往復の判定・root への追記・質問の文は 3 経路とも `check_item::health_repeat_stop` を通る。(b) 段の統合で health の修復が `max_repairs`・`max_repairs_per_class`（bucket `health:<rule_id>`）を使い切ったら、replan せず Blocked（質問 `health_repair_limit: <位置>: …`）で止める。(c) task の review の health の Error・Unavailable と、health だけの不合格の上限の使い切りは、新しい `Trigger::ReviewBlocked`（reason `review_blocked`、attempts 不変）で Blocked にし、受信箱に質問を 1 件出す（質問の先頭は `health_check_error:`・`health_repair_limit:`。上限の使い切りの質問の文は段の統合と同じ `check_item::repair_limit_question`。loop-points の記録の「違い」4 の Failed を直した）。(d) 往復の連なりを切る人の介入の判定は `task_ops::plan_gate::is_human_resume`（replan の依頼・answer・comment・reopen）の 1 か所で、`plan_gate::stall` の `stalled_replans` と `health_review::limits` が共有する。状態は closeout が改める。
