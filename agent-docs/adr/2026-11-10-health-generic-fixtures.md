---
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
---
# ADR 2026-11-10: 非 Celeris の isolated Git fixture で repo health の汎用性とすり抜け耐性を確かめる
状態: 実装済み

- 日付: 2026-11-10
- 実装: 健全性 v2 G4（task 01M6TYVJSXBZMSBC0YGAT2TTX0）の後続 unit（py-detect・py-bypass・rs-paths・rs-stale・rs-limits・measure・record）と、policy だけの branch の子 task exc-expiry。この ADR の unit はコードを変えない。記録は [G4 の進捗](../progress/2026-11-10-health-v2-g4.md)。
- 土台: [ADR 2026-11-07 repo health の契約](2026-11-07-repo-health-contract.md)（report/1、exit 0/1/2、policy は base 側で評価、repo に profile が無ければ組み込み generic）、[ADR 2026-11-07 Celeris profile](2026-11-07-repo-health-celeris-profile.md)（Celeris 固有の所在・上限は `profiles/celeris.toml` にだけ置く）、[ADR 2026-11-07 健全性を設計と review に組み込む](2026-11-07-health-review-integration.md)（D6 の policy の branch）、[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](2026-11-08-health-enforcement-loop.md)（以下 G3 の ADR。J1〜J8、`applicability`、同梱の検査器、束縛、上限、棚卸し）、B0 の [slices.json](../progress/2026-11-07-health-v2-b0/slices.json) の G4。
- 関係: [必須検査を候補に束縛する](2026-10-23-required-checks-bound-to-candidate.md)、[ADR-0118](0118-review-target-sync-and-merge-candidate.md)（target・reviewed・merge candidate の SHA）、[reviewer の合格の再利用](2026-10-27-review-resync-halt-visible-and-reviewer-reuse.md)、[ADR-0131](0131-cron-jobs.md)（棚卸しの cron）、[費用の上限](2026-11-06-budget-usd-caps.md)、[全体試験は release だけ](2026-10-20-full-tests-at-release-only.md)、[破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)、[worker は subagent と LLM の CLI を起動しない](2026-10-07-worker-no-subagents-no-llm-cli.md)。

## 結論
- G4 は検査器・policy・dispatcher の経路を変えない。G1〜G3 の仕組みを、Celeris でない小さな Git repo に当てて、親 task の目的 (a)〜(h) を決定的な試験で確かめる。検査器・policy を変えるのは子 task exc-expiry の policy だけの branch に限る（人の回答 2026-11-10）。
- fixture は試験の実行時に `$TMPDIR` の下へ一時の Git repo として生成する。生成器（builder）は `scripts/tests/repo_health_fixtures.py` の 1 か所に置き、Python の試験は import で、Rust の試験は CLI `python3 -I scripts/tests/repo_health_fixtures.py build <kind> <variant> <dest>` で同じ builder を呼ぶ。fixture の内容を repo に commit しない。fixture の内容を Rust の試験に写さない。
- kind は ts（TypeScript/JavaScript）・rust（Cargo workspace）・multi（ts と rust の 2 repo）、variant は good・dup-contract・dup-state・dep-inversion・security-double・vendor-generated・docs-only と、policy を弱める 5 種（policy-exclude・policy-rule-off・policy-exception・policy-threshold・baseline-rewrite）と、判定の 3 種（non-git・unsupported-tool・history-smuggle）。
- 目的 (a)〜(h) は後続 unit ごとに試験 file と試験名の prefix を割り当てる（(3) の表）。期限付き例外 ((h) の一部) は今の検査器に無いので、決定 exception-expiry と子 task exc-expiry に回す。
- 精度・誤検出・追加遅延は fixture の行列（kind × variant）で、ラベル付きの期待 finding と実際の fail 級 finding の照合、検査器 1 回の実時間で測る。LLM の費用は有料の再測定をしないので未測定と書く（人の回答 2026-11-10）。
- 実ユーザーの repo・Celeris の本番 host・本番 DB には触れない。試験は外部ネットワークに出ない。

## 既存の正本と再利用先
| 責務 | 既存の正本 | G4 での扱い |
|---|---|---|
| 検査器と rule | `scripts/dev/repo-health/repo_health.py`・`health/`・`rules/`（`dup.contract`・`dup.state`・`dup.public-surface`・`deps.violation`・`deps.cycle`・`policy.changed`・既存検査の包み） | 変えない。fixture の profile が rule を有効にする |
| profile の解決 | `health/profile.py` の `_resolve`（repo の `scripts/dev/repo-health/profiles/default.toml`、無ければ組み込み generic） | 変えない。fixture の base commit が自分の `default.toml` を持つ |
| 例外 | `health/profile.py` の `Exception_`（reason 必須、fingerprint か path）と `engine._exception` | 期限の欄は無い。子 task exc-expiry が policy の branch で足す |
| file の種類（generated・fixture・test・product） | `health/adapters.py` の `Classifier`・`GENERATED_MARKERS`・`DEFAULT_CLASS_PATHS` | 変えない。vendor-generated の variant はこの分類と profile の `exclude_paths` で外れることを確かめる |
| 適用判定 | `task_core::required_checks::health::applicability`・`repo_facts`（J1） | 変えない。(g) の flag の非依存はこの関数の入力に欄が無いことと、dispatcher の経路の結果が変わらないことで確かめる |
| 実行の口と同梱の検査器 | `task_dispatch::health_review::gate`（`run_health_check`・`configure_bundled_checker`・`CHECKER_TREE_FILE`） | Rust の試験は stub でなく、作業ツリーの `scripts/dev/repo-health/` を写した同梱の配置を設定する |
| 束縛と再利用 | `task_core::required_checks`（`definition_digest` v3・`reusable_health`・`verify_required_checks`）、G3 の J2 | (e) はこの束縛が本物の検査器と fixture でも効くことを確かめる |
| 上限と往復・棚卸し | `ExecutionConfig.max_repairs`・`max_repairs_per_class`（class `health:<rule_id>`）、D4.3 の往復、cron `health_inventory`（`health_inventory_runs`） | (h) はこれらを非 Celeris の fixture で確かめる |
| Python の試験の土台 | `scripts/tests/test_repo_health_core.py`・`test_repo_health_dup.py` の `git()` と `tempfile.mkdtemp(dir=$TMPDIR)` | `git()` と一時 dir の作法を builder に移して 1 か所にし、既存の試験は変えない（既存の試験の inline の repo は rule 単体の試験で、fixture の行列とは目的が違う） |
| Rust の試験の土台 | `dispatcher/tests/health_enforce.rs` の `init_non_celeris_repo`・`configure_failing_bundled_checker`・`non_celeris_dispatcher`、`health_review/gate_tests.rs` の `init_repo` | `non_celeris_dispatcher` を再利用する。repo の生成は builder の CLI に置き換える（stub の検査器を使う既存の試験は変えない） |
| 試験の実行 | `python3 -m unittest discover -s scripts/tests -p 'test_repo_health*.py'`、`bash scripts/dev/test-affected.sh` | 新しい runner は作らない |

## 不変条件
- K1: fixture は `$TMPDIR` の下の一時 repo で、repo に commit しない。実ユーザーの repo・`/home/rmaeda/workspace`・本番の path を読まず書かない（builder は `dest` の外に書かない）。
- K2: fixture の内容は builder の 1 か所にだけある。Rust の試験は CLI で同じ builder を呼び、fixture の file を Rust の文字列に写さない。
- K3: G4 の試験は検査器・policy・baseline・Celeris profile を変えない。変えるのは子 task exc-expiry の policy だけの branch。
- K4: 試験は外部ネットワークに出ず、LLM を呼ばない。費用の数値は既存の記録から読むか「未測定」と書く。
- K5: Celeris 固有の flag（案件名・repo 名 celeris・selfdeploy の設定・task の mode）を変えても、同じ fixture に対する health の結果（Pass・Fail・NotApplicable・Error の判定と fail 級の finding の集合）は変わらない。
- K6: 試験は決定的で、時間の待ちや負荷の台本を置かない（CLAUDE.md の規則）。

## 決定

### (1) fixture の置き場所と builder
- 決定: fixture は試験の実行時に生成する。builder は `scripts/tests/repo_health_fixtures.py` の 1 か所。Python 3 標準ライブラリと git だけを使う。
- D1.1 CLI は `python3 -I scripts/tests/repo_health_fixtures.py build <kind> <variant> <dest>`。`dest` は無い dir か空の dir で、builder は `dest` の下にだけ書く。生成の後、`dest` には branch `main` の base commit と branch `candidate` の head commit がある（multi は `dest/ts` と `dest/rust` の 2 repo）。出力の末尾に 1 行の JSON `{"base": <sha>, "head": <sha>, "repos": [<dest 相対 path>], "expected": [<期待 finding のラベル>]}` を書く。Rust の試験はこの JSON を読む。
- D1.2 Python の試験は同じ module を import し、`build(kind, variant, dest) -> Fixture`（上の JSON と同じ欄）を呼ぶ。CLI と関数は同じ本体で、二重に書かない。
- D1.3 一時 dir は `tempfile.mkdtemp(dir=os.environ.get("TMPDIR"))`（Python）と `tempfile::tempdir()`（Rust。nextest は run ごとの `TMPDIR` を継ぐ）。試験の後に消す。
- D1.4 fixture の base commit は、自分の profile `scripts/dev/repo-health/profiles/default.toml` を持つ（評価する rule と層を宣言する。(2) の表）。検査器の本体 `repo_health.py` は fixture に置かない。Python の試験は作業ツリーの `scripts/dev/repo-health/repo_health.py` を `--repo <dest>` で呼び、Rust の試験は同じ dir を写した同梱の配置を `configure_bundled_checker` に設定する（base に検査器が無い repo は同梱の検査器を使う。G3 の D1.1）。
- D1.5 git の環境: builder は `GIT_CONFIG_GLOBAL=/dev/null`・`GIT_CONFIG_NOSYSTEM=1` と repo ごとの `user.name`・`user.email` で commit し、hook を置かない。試験から対象 repo（Celeris の作業ツリー）へ `GIT_*` を漏らさない（既存検査の包みと同じ作法）。
- 採らなかった: B0 の G4 の slice が書いた `scripts/dev/repo-health/fixtures/` に fixture を commit する案。この path は Celeris profile の `policy_paths`（`^scripts/dev/repo-health/`）に当たり、fixture を足す・変える branch のたびに `policy.changed` が fail になって check-health と test-affected が止まる。fixture は policy でないので、policy の path の外に置く。
- 採らなかった: Rust の試験ごとに inline で repo を書く案（今の `init_non_celeris_repo` の延長）。fixture の内容が Python と Rust の 2 か所になり、片方だけ直る（規約 2・K2）。
- 採らなかった: `crates/e2e/tests/repo_health*.rs`（B0 の write_set）に Rust の試験を置く案。WU の check・direct review・reviewer の再利用・段の統合は `Dispatcher` の内部の経路で、試験の土台（`new_test_dispatcher`・`non_celeris_dispatcher`・stub の adapter）は `crates/task-dispatch/src/dispatcher/tests/` にある。`crates/e2e/tests/` は無く、daemon を起こす e2e は release gate だけで走る。

### (2) fixture の種類（kind）と variant
- 決定: kind は 3 つ、variant は下の表。variant は「head が base に何を足すか」で、期待する finding のラベル（`<rule_id>:<鍵>`）を builder が JSON に書く。

| kind | base の中身 | profile（base の `default.toml`） |
|---|---|---|
| ts | `package.json`・`tsconfig.json`、`src/core/`（型と util）・`src/app/`（core を import）・`src/auth/verify.ts`（token の検証）・`docs/`・`README.md`。SQL は `src/core/store.ts` が `INSERT INTO sessions` を 1 か所で書く | `adapters.enabled = ["typescript", "markdown"]`、`deps.violation` の層 core → app、`deps.cycle`・`dup.contract`・`dup.state`・`dup.public-surface` を required、`exclude_paths = ['^vendor/']`、`policy_paths` に `^scripts/dev/repo-health/` と `^size-baseline\.json$`、`doc_only` は組み込みと同じ、`dup.contract` の例外 `path = '^src/broker/verify\.ts$'`（reason「security 境界の独立検証」） |
| rust | Cargo workspace `crates/core`・`crates/app`（app が core に依存）、`crates/core/src/store.rs` が `INSERT INTO sessions` を書く、`crates/auth/src/verify.rs`、`docs/`・`README.md` | ts と同じ構成を rust の path で書く（`adapters.enabled = ["rust", "markdown"]`、層 core → auth → app、例外 `path = '^crates/broker/src/verify\.rs$'`） |
| multi | `dest/ts` と `dest/rust` の 2 repo（上の 2 つ）。1 つの task が 2 つの repo を持つ場合に使う | それぞれの repo の profile |

| variant | head が足すもの | 期待（fail 級の finding と exit） | 目的 |
|---|---|---|---|
| good | 許された向きの機能追加（app が core の新しい関数を呼ぶ。新しい型は 1 つ、SQL の書き手は増えない） | fail 級なし、exit 0 | (a) |
| dup-contract | `src/app/session_view.ts`（rust は `crates/app/src/session_view.rs`）に、core の `Session` と同じ field の集合を持つ別名の型（product の class） | `dup.contract:shape:<言語>:<field の並び>` が candidate=true・severity warning・delta new・exception null で 1 件。evidence の path と line が head の変更 file を指す。exit 0・verdict pass。final review の材料（`ReviewRequest.health` の new findings）に入る | (b) |
| dup-state | app の module が `UPDATE sessions` を書く（書き手 2 module） | `dup.state:table:sessions` が dup-contract と同じ性質（candidate=true・warning・new・exception null）で 1 件。exit 0・verdict pass。final review の材料に入る | (b) |
| dep-inversion | core の file が app の module を import する（層の逆向き。rust は core の Cargo.toml が app に依存する） | fixture の base の profile が層を宣言するとき、`deps.violation` が fail 級（error・new・candidate=false）で exit 1・verdict fail。profile 無し（層の宣言なし）では `deps.violation` は `not_applicable`（reason `no_layers_declared`）で finding 0。ほかの rule は実測どおり: ts は exit 0・pass、rust は Cargo の循環で `deps.cycle` が fail 級 1 件・exit 1 | (b) |
| security-double | `src/broker/verify.ts`（rust は `crates/broker/src/verify.rs`）に auth と同形の検証の型と関数。base の profile に path の例外（reason あり） | finding は出るが `exception` が non-null で fail 級でない。exit 0。evidence の path を消す提案を含まない（検査器は削除しない） | (c) |
| vendor-generated | `vendor/lib/types.ts` に core と同名の型（`exclude_paths` で外れる）、`src/generated/api.ts` に `@generated` の marker と同形の型（class generated で `dup.contract` の `classes = ["product"]` に入らない） | fail 級なし、exit 0 | (c) |
| docs-only | `README.md`・`docs/` だけの変更 | `inputs.doc_only = true`、構造 rule は `not_applicable`、exit 0 | (d) |
| non-git | builder が `.git` の無い dir（kind の base の tree だけ）を作る | 検査器は `UsageError`（exit 2）。Celeris は `NotApplicable { not_a_git_repo }` で health の項目を足さない | (d) |
| unsupported-tool | base の profile が `doc.links` の command に無い script を書く | rule は `unsupported`、exit 2。Celeris は Error（合格にしない） | (d) |
| policy-exclude | dep-inversion の変更に加えて、head が base の profile の `exclude_paths` に `^src/app/` を足す | `policy.changed`（fail、`policy.files` に profile）、exit 1。dep-inversion の `deps.violation` は base の profile で評価するので残る | (f) |
| policy-rule-off | dep-inversion の変更に加えて、head が `deps.violation` の `required = false` にする | 同上 | (f) |
| policy-exception | dep-inversion の変更に加えて、head が `deps.violation` の例外（fingerprint）を足す | 同上 | (f) |
| policy-threshold | dep-inversion の変更に加えて、head が `deps.violation` の層の許可（`allow`）を広げる | 同上 | (f) |
| baseline-rewrite | dep-inversion の変更に加えて、head が `policy_paths` の `size-baseline.json` を書き換える | `policy.changed`、exit 1 | (f) |
| history-smuggle | candidate の branch が 2 commit で、先の commit で dep-inversion を入れ、後の commit で無関係の変更をする。base は `main` のまま | base との差分で `deps.violation` が `delta = new`・fail 級、exit 1（候補の branch の途中の commit を base にしない） | (f) |

- D2.1 ts の fixture は JavaScript の file（`.js`）も 1 つ含める（adapter は `.js` を typescript として読む）。
- D2.2 fixture の規模は kind ごとに 10 file・300 行の程度にとどめ、検査器 1 回が 1 秒の桁で終わるようにする（(4) の追加遅延の測定を試験の中で行うため）。
- D2.3 期待のラベルは `<rule_id>:<finding の鍵>` で、builder が variant ごとに書く。照合は report の `findings[]` の `rule_id` と `fingerprint` の元になる鍵で行い、fingerprint の値そのものは期待に書かない（検査器の版で変わりうる）。
- 採らなかった: 実在する OSS の repo を clone して fixture にする案。外部ネットワークに出る、規模が大きく時間が読めない、期待の finding にラベルを付けられない。
- 採らなかった: variant ごとに別の base を作る案。base は kind ごとに 1 つにし、variant は head だけを変える（stale の試験で「base が変わる」を独立に起こせる）。

### (3) 目的 (a)〜(h) と試験 file・試験名
- 決定: 後続 unit ごとに試験 file と試験名の prefix を下の表のとおり割り当てる。試験名は prefix の後に kind と variant を含める（例 `test_generic_ts_dup_contract_fails`、`health_generic_multi_each_repo_checked`）。

| 目的 | 試験 file | prefix | unit | 確かめること |
|---|---|---|---|---|
| (a) 良い変更は通る | `scripts/tests/test_repo_health_generic.py` | `test_generic_` | py-detect | ts・rust・multi の good が exit 0、fail 級 0、`summary.verdict = pass` |
| (b) 複製・不要な状態・依存逆転を根拠付きで検出 | `scripts/tests/test_repo_health_generic.py` | `test_generic_` | py-detect | dup-contract・dup-state は期待のラベルの finding が candidate=true・warning・new・exception null で 1 件ずつ、exit 0・verdict pass、`evidence[].path` と `line` が head の変更の file を指す。dep-inversion は層を宣言した base の profile で `deps.violation` が fail 級・exit 1。profile 無しは `not_applicable`（`no_layers_declared`）（試験 `test_generic_dep_inversion_without_layers_not_applicable`） |
| (c) 正当な二重検証・vendor/generated を誤って削らない | `scripts/tests/test_repo_health_generic.py` | `test_generic_` | py-detect | security-double は `exception` 付きで fail 級 0、vendor-generated は finding 0、どちらも exit 0 |
| (d) docs-only・non-Git・unsupported・tool 故障の判定 | `scripts/tests/test_repo_health_generic_bypass.py` | `test_generic_` | py-bypass | docs-only は `doc_only` と `not_applicable`、non-git と unsupported-tool は exit 2 と `summary.verdict`、tool 故障（`health/` の module を欠いた写し）は exit 2 で report を書かない |
| (f) policy・baseline・例外の弱化によるすり抜けの拒否（検査器の側） | `scripts/tests/test_repo_health_generic_bypass.py` | `test_generic_` | py-bypass | policy-exclude・policy-rule-off・policy-exception・policy-threshold・baseline-rewrite が `policy.changed` で exit 1 になり、dup-contract の finding も base の profile で残る。`--print-review-policy` が head の profile を読まない |
| 束縛（policy の hash） | `scripts/tests/test_repo_health_generic_bypass.py` | `test_generic_` | py-bypass | `policy.files` の digest が base の profile から出ること（head の profile を変えても変わらない） |
| (g) atomic/WU/review reuse の抜け道なし、複数 repo、flag の非依存 | `crates/task-dispatch/src/dispatcher/tests/health_generic.rs` | `health_generic_` | rs-paths | 本物の検査器（作業ツリーの `scripts/dev/repo-health/` を写した同梱の配置）と ts・rust の fixture で、WU の check・atomic の direct review・reviewer の合格の再利用の 3 経路が dep-inversion（class `health:deps.violation`）を Fail にして repair に回し、good を Pass にする。dup の候補（dup-contract・dup-state）は Fail にならず、final review の材料（`ReviewRequest.health` の new findings）に入る（試験名 prefix `review_input_`）。試験名の prefix は WU の check が `wu_check_`、atomic の direct review が `direct_review_`、reviewer の合格の再利用が `reviewer_reuse_`、複数 repo が `multi_repo_`。multi は 2 repo のそれぞれに health の項目があり、片方の Fail で統合が止まる。K5 の flag を変えた同じ task で結果が同じ |
| (e) candidate・base・policy の変更で古い合格を再利用しない | `crates/task-dispatch/src/dispatcher/tests/health_generic_stale.rs` | `health_generic_stale_` | rs-stale | good の Pass の後に head を変える（dep-inversion を足す）・base を進める・base の profile を変える、のそれぞれで前の Pass を再利用せず検査器が再び走る（`RequiredChecksEvaluated` が増える） |
| (f) baseline 改変で合格しない（dispatcher の側） | `crates/task-dispatch/src/dispatcher/tests/health_generic_stale.rs` | `health_generic_stale_` | rs-stale | `health_generic_stale.rs` の試験名の prefix は `stale_`・`baseline_weakening_not_pass`・`reviewer_reuse_`。baseline-rewrite と policy-exclude は PolicyOnly で統合を止め（G2 の D6）、history-smuggle は Fail。候補が `scripts/dev/repo-health/` を書き換えても検査器は同梱のものが走る（J3） |
| (h) 反復上限・同じ指摘の往復・変更なし repo の棚卸しの抑制 | `crates/task-dispatch/src/dispatcher/tests/health_generic_limits.rs` | `health_generic_limits_` | rs-limits | dep-inversion の repair が `max_repairs_per_class`（class `health:deps.violation`）で止まり Blocked と受信箱、同じ fingerprint の往復が root の受信箱に 1 件、`health_inventory` が同じ head_tree・policy・検査器の repo を 2 回目に省く |
| (h) 期限付き例外 | 子 task exc-expiry の policy の branch | （exc-expiry が決める） | exc-expiry | 決定 exception-expiry。G4 の branch では試験しない |

- D3.1 Rust の試験 file は `dispatcher/tests/mod.rs` に `mod health_generic;` ほか 3 行を足す。試験の土台は `health_enforce.rs` の `non_celeris_dispatcher` を `pub(super)` のまま使う。本物の検査器の同梱の配置は `health_generic.rs` に 1 つの helper `configure_real_bundled_checker()` を置き、`CARGO_MANIFEST_DIR` から `scripts/dev/repo-health/` と `scripts/dev/source-size-report.py`（`CHECKER_SIDE_FILES`）を一時 dir に写して `CHECKER_TREE_FILE` を書く。stale・limits の file はこの helper を `pub(super)` で使う（3 か所に写さない）。
- D3.2 Rust の試験が builder を呼ぶときは `python3 -I <CARGO_MANIFEST_DIR>/../../scripts/tests/repo_health_fixtures.py build <kind> <variant> <dest>` で、出力の JSON の `base`・`head` を task の branch と target に使う。python3 が無い環境では試験は失敗する（skip にしない。検査器も python3 を要する）。
- D3.3 (g) の flag の非依存は 1 つの試験 `health_generic_flags_do_not_change_outcome` で、同じ fixture に対して (i) 案件名を `celeris` に、(ii) repo id を `celeris` に、(iii) task の mode を prototype に、(iv) `DispatchConfig` の selfdeploy に関わる欄を有効に、(v) 案件の `self_improve` を立てる、したそれぞれの task の health の判定と fail 級の finding の集合が、既定の task と等しいことを確かめる。欄の実名は rs-paths が進捗に書く。
- D3.4 py-detect と py-bypass は `scripts/tests` の既存の discover（`-p 'test_repo_health*.py'`）に乗る。`test-affected.sh` は `scripts/tests` を対象にしないので、Phase 完了の検査では discover を別に走らせて進捗に書く（record）。
- D3.5 期限付き例外は今の `Exception_` に期限の欄が無い（`reason`・`fingerprint`・`path` だけ。未知の欄は profile の読み込みエラー）。検査器と profile を変える変更なので、G2 の D6 のとおり policy だけの branch に置く。決定 exception-expiry の問いは「例外に期限（`expires`、UTC の日付）を必須にするか任意にするか」で、子 task exc-expiry が人の答えのもとで実装する。G4 の試験は期限の無い例外だけを使う。
- 採らなかった: (e)(f) を Python だけで確かめる案。古い合格の再利用は dispatcher の束縛（`reusable_health`）の性質で、検査器だけでは確かめられない。
- 採らなかった: (g) の 3 経路を 1 つの試験にまとめる案。経路ごとに別の試験にし、どの経路が抜けたかが名前で分かるようにする。

### (4) 精度・誤検出・追加遅延・費用の測り方
- 決定: unit measure が kind × variant の行列を builder で作り、検査器を 1 回ずつ走らせて、次の 4 つを数える。報告は人向けの Markdown を成果物 dir に、機械向けの JSON を同じ dir に置く。repo には進捗 file の表だけを書く。
- D4.1 精度（検出）: 期待のラベルがある variant（dup-contract・dup-state は候補の finding、dep-inversion・policy の 5 種・history-smuggle は fail 級の finding）で、期待のラベルが該当の性質で出た数 / 期待の数。kind ごとと合計。
- D4.2 誤検出: 期待のラベルが無い variant（good・security-double・vendor-generated・docs-only）で出た fail 級の finding の数（0 が目標）と、期待のラベルがある variant で期待の外に出た fail 級の finding の数。1 件ずつ rule_id と evidence の path を表に書く。
- D4.3 追加遅延: 検査器 1 回の実時間（`time.monotonic()` で `repo_health.py` の起動から終了まで）を variant ごとに 3 回測った中央値。multi は 2 repo の和。基準は same host の `python3 -c pass` の起動時間。Celeris の review 1 回に対する追加分は「検査器 1 回の時間 × 経路の回数（WU の check 1・review 1・delivery の照合は再利用で 0）」で見積もり、実測は dispatcher の試験の `RequiredChecksEvaluated` の時刻の差を 1 件書く。
- D4.4 費用: 検査器は LLM を呼ばないので、health の決定的な検査の費用は 0 USD。reviewer の LLM の費用（health の材料を増やしたことによる増分）は有料の再測定をしないので「未測定」と書き、既存の記録（G2・G3 の進捗の review_cost）があれば出典付きで引く。推測の数値を書かない。
- D4.5 行列の実行は measure の run の中で 1 回。試験には入れない（試験は variant ごとの判定だけ）。CPU を焼く負荷をかけない（K6）。
- 採らなかった: 実 LLM の reviewer を fixture で走らせて費用を測る案。人の回答（有料の再測定なし）に反する。

### (5) write_set: B0 の G4 の write_set に足す path と理由
- 決定: slices.json は変えず、各 unit の計画の範囲 check で下の path を許す（G3 の (8) と同じ扱い）。

| 足す path（正規表現） | 理由 | 使う unit |
|---|---|---|
| `^scripts/tests/repo_health_fixtures\.py$` | builder（(1)）。`test_repo_health*.py` の既存の正規表現に当たらない | py-detect |
| `^crates/task-dispatch/src/dispatcher/tests/(mod\|health_generic\|health_generic_stale\|health_generic_limits)\.rs$` | (3) の Rust の試験と `mod` の宣言。B0 の `crates/e2e/tests/` は無い（(1) の採らなかった案） | rs-paths・rs-stale・rs-limits |
| `^crates/task-dispatch/src/dispatcher/tests/health_enforce\.rs$` | `non_celeris_dispatcher` の可視性を変える必要が出た場合だけ（stub の試験は変えない） | rs-paths |

- fixture の builder は `scripts/dev/repo-health/` の外に置くので、Celeris profile の `policy_paths` には当たらない。G4 の branch では `policy.changed` が出ない（py-bypass・rs-stale が check-health で確かめる）。
- 採らなかった: B0 の G4 の `consolidate_into_owner`（`scripts/dev/repo-health/fixtures/`）をそのまま使う案（(1) の採らなかった案）。

## 後続 unit の path・試験名の prefix
- py-detect: `scripts/tests/repo_health_fixtures.py`（builder、CLI と `build()`）、`scripts/tests/test_repo_health_generic.py`（`test_generic_`。(a)(b)(c)）。
- py-bypass: `scripts/tests/test_repo_health_generic_bypass.py`（`test_generic_`。(d)(f)・束縛）。builder に variant を足すときは py-detect の builder を変える（並行するときは variant の名前を (2) の表のとおりにし、統合で両側を保つ）。
- rs-paths: `crates/task-dispatch/src/dispatcher/tests/health_generic.rs`（`health_generic_`。(g)・flag・複数 repo）、`mod.rs` の宣言。
- rs-stale: `crates/task-dispatch/src/dispatcher/tests/health_generic_stale.rs`（`health_generic_stale_`。(e)(f)）。
- rs-limits: `crates/task-dispatch/src/dispatcher/tests/health_generic_limits.rs`（`health_generic_limits_`。(h)）。
- measure: 成果物 dir の `health-generic-matrix.md`・`health-generic-matrix.json`、進捗の表。
- record: 進捗 `agent-docs/progress/2026-11-10-health-v2-g4.md` の (a)〜(h) と試験の対応、この ADR の状態を「実装済み」に、付記。
- exc-expiry（子 task、policy の branch）: `scripts/dev/repo-health/health/profile.py`・`engine.py`・`profiles/celeris.toml`・schema・`scripts/tests/test_repo_health_review_policy.py` か新しい `test_repo_health_exception_expiry.py`。

## 本番への移行
- なし。G4 は検査器・policy・config・DB を変えない。exc-expiry の policy の branch は人の承認の後に operator が main に merge する（G2 の D6、G3 の ci-entry の前例）。

## 付記（2026-11-10、unit py-detect）: 契約との食い違いの訂正
- 理由: 初版の D2 の表・D3・D4 は、`dup.*` が fail 級（exit 1）になり、`deps.violation` が層の宣言なしでも動く前提で書いた。契約 [ADR 2026-11-07 repo health の契約](2026-11-07-repo-health-contract.md) は違う。`dup.*` は candidate=true・severity warning 以下で verdict に影響せず（検証器 S4 は候補の error を拒否する）、`deps.violation` は層の宣言が無ければ `not_applicable`（`no_layers_declared`）になる。v1 の run は 12 件の不一致で落ちた。検査器は契約どおりに動いているので、検査器は変えずこの ADR と試験を直した。
- 訂正: (1) dup-contract・dup-state の期待は「candidate の finding が 1 件、exit 0・verdict pass、final review の材料に入る」。(2) fail 級を前提にした行（policy 弱化 5 種・history-smuggle の土台、(e)(g)(h) の Fail と repair、repair の class）は dep-inversion と class `health:deps.violation` に移した。(3) dep-inversion は fixture の base の profile が層を宣言したときだけ fail 級。profile 無しは `not_applicable` で、ほかの rule（Rust の `deps.cycle` など）は実測どおり。(4) Rust の試験の割り当て: `health_generic.rs` に `wu_check_`・`direct_review_`・`review_input_`・`multi_repo_`・`flags_do_not_change_outcome`、`health_generic_stale.rs` に `stale_`・`baseline_weakening_not_pass`・`reviewer_reuse_`。flag に案件の `self_improve` を足した。
- 試験: `test_generic_dep_inversion_without_layers_not_applicable` を足した。

## 付記（2026-11-11、unit record）: 実装との食い違い
unit ごとの記録と提出時の検査は [G4 の進捗](../progress/2026-11-10-health-v2-g4.md)。上の py-detect の付記に無い食い違いを書く。
- (a) 結論の「dispatcher の経路を変えない」: G4 は dispatcher の review の修復の分類の入力を 1 か所変えた。rs-stale が欠け G4-S1（`Check::Reviewer` のある task で、決定的な不合格の修復を reviewer の「未評価」の項目が妨げ、health の Fail が `review_fail` になる）を見つけ、unit fix-s1 が `crates/task-dispatch/src/dispatcher/review_verdict.rs` の `Dispatcher::try_review_repair` で reason が `review::REVIEWER_NOT_EVALUATED_REASON`（`crates/task-dispatch/src/review.rs`）の項目を分類から外した（[ADR-0072](0072-task-execution-decomposition.md) の付記 G4-S1）。検査器・policy・baseline・`crates/campaign/` は G4 の branch で変えていない（`git diff --quiet $(git merge-base HEAD main) -- crates/campaign/ scripts/dev/repo-health/ scripts/dev/check-health.sh` が exit 0）。検査器の期限付き例外は子 task exc-expiry の policy だけの branch が main に入れた。
- (b) write_set（(5)）: 表に無い次の path を変えた（範囲は各 unit の計画の check で許した。slices.json は変えていない）。`crates/task-dispatch/src/health_review/scenario_tests.rs`（rs-paths。D3.1 の写す helper を新しく書かず、既存の `copy_checker` を `pub(super)` にして使った）、`crates/task-dispatch/src/dispatcher/review_verdict.rs`・`crates/task-dispatch/src/review.rs`・`crates/task-dispatch/src/dispatcher/tests/{review,health_enforce,ownerless_runs}.rs`（fix-s1）、`crates/task-dispatch/src/dispatcher/tests/git_workspace.rs`（deflake-ws。段 hardening の統合を止めた review の試験の待ちを tick の回数から出来事待ちにした）、`agent-docs/progress/2026-11-10-health-v2-g4/measure.py`（measure の台本）、`agent-docs/adr/0072-task-execution-decomposition.md`（G4-S1 の付記）。
- (c) (2) の variant: builder は 16 variant を持つ。表に無い `tool-failure`（包んだ検査の故障）を足した。`unsupported-tool` と `tool-failure` は base の profile に包んだ検査の command を足すので、「base は kind ごとに 1 つ」の例外になる。policy の弱化 5 種と tool の 2 種は profile 無しでは作れず、builder は `ValueError` を出す。docs-only は `inputs.doc_only = true` という欄でなく、report の契約の欄 `inputs.mode = "doc-only"` で確かめた。
- (d) (3) の (d) の行: 検査器の欠け G1・G2 を `@unittest.expectedFailure` の 2 試験に残した（人の決定 checker-gaps の a）。G1 `test_generic_checker_import_failure_is_error`: `health/` の module を欠いた検査器は exit 2 でなく exit 1（report は無い）。G2 `test_generic_missing_script_unsupported`: 無い script を包んだ `doc.links` は `unsupported` でなく `fail`（`/bin/sh` が dash で exit 2 を返し、126・127 にならない）。どちらも合格にはならない。直すのは G4 の後の policy だけの別 task。
- (e) (3) の (g) の行の `review_input_`: 候補の finding は report には入るが、`[review]` 節の無い repo（fixture の profile と組み込み generic）では final review の材料に入らない（未解決 G4-R1）。[ADR 2026-11-07 health-review-integration](2026-11-07-health-review-integration.md) の D4.4 は review 方針が `absent` の repo で `ReviewRequest.health = None` にし `HealthReviewInputSkipped` を残すと定めていて、この ADR の期待と食い違う。4 試験 `health_generic_review_input_{ts,rust}_{dup_contract,dup_state}_reaches_reviewer` は期待を弱めず `should_panic`（`no HealthReviewInputBuilt for the candidate`）で残した。`*_does_not_fail` の 4 試験（report に候補が 1 件、health は Pass）は合格する。どちらの ADR を直すかは G4 の外の別 task で決める。
  - 2026-11-11 訂正（G4-R1）: 直すのは health-review-integration の D4.4 に決めた。この ADR の期待（(2) の表の dup-contract・dup-state の「final review の材料に入る」と (3) の `review_input_`）は変えない。review 方針が `absent` の repo も `present` と同じ構築で材料を作り、`owner_rows`・`invariants` だけを空にする。決定は [ADR 2026-11-11 health-generic-review-input](2026-11-11-health-generic-review-input.md)（task 01M74X0XAN0MCFJ4KZ3VVFT2QA）。4 試験の `should_panic` は同じ task の unit input-owner が外し、adapter が実際に受けた `ReviewRequest.health` は unit request-atomic・request-integrated が `g4_r1_review_request_` の試験で確かめる。上の「`[review]` 節の無い repo では final review の材料に入らない」は、その実装が入るまでの状態の記録で、契約ではない。
- (f) rust の dep-inversion: 層を宣言した profile でも `deps.violation` に加えて `deps.cycle` が fail 級になる（core の `Cargo.toml` が app に依存し、Cargo の循環を実際に作る）。review の repair の class は `health:deps.cycle,health:deps.violation`。measure の fail 級の適合率 77.4% の差 14 件もこの `deps.cycle`。
- (g) D3.3 の flag の実名: (i) `Project.title`・`Project.slug`、(ii) `ProjectRepo.name`・`RepoRef.name`（`RepoId` は ULID で名前を持たない）、(iii) `Task.mode` = `TaskMode::Prototype`、(iv) `DispatchConfig.releases_dir`、(v) `self_improve` という欄はコードに無いので、案件ごとの自動化の欄 `Project.auto_integrate` を立てた。複数 repo の止まる試験は builder の multi でなく、別々に作った ts good と rust dep-inversion の 2 repo を使った（multi は 2 repo に同じ variant を入れる）。
- (h) (3) の (f) の dispatcher の側: 候補が自分の `scripts/dev/repo-health/repo_health.py` を持ち込んだ場合の verdict は Fail でなく Error（同梱の検査器が候補の `.py` を評価し、fixture の profile の adapter に python が無いので `unsupported:<rule>(adapter_missing:python)`）。合格にはならない。reviewer の合格の再利用で health が Fail になるのは、再利用の後に base 側が変わる場合だけ。
- (i) (3) の (h) の行: 修復の上限の試験は ts だけ（rust の逆向きの依存は `Cargo.toml` の行で、内容を書かずに file を移せない）。期限付き例外は exc-expiry が `scripts/tests/test_repo_health_exception_expiry.py` の 5 試験（`test_exception_expir` で始まる）で確かめた。
- (j) (4) の測り方: 検査器は 3 回でなく 5 回ずつ当てた。報告の file 名は `health-generic-matrix.*` でなく `g4-measurement.md`・`g4-measurement.json`（WorkUnit measure の成果物 dir）。`RequiredChecksEvaluated` の時刻の差（D4.3）は測っていない。

## 状態の記録
- 2026-11-10: 提案（unit adr）。実装の後、record が「実装済み」にし、実装との食い違いを付記に書く。
- 2026-11-11: 実装済み（unit record）。実装との食い違いは上の「付記（2026-11-11、unit record）」。未解決は G4-R1・G1・G2 で、直す別 task を進捗の「提案」に書いた。
- 2026-11-11: G4-R1 の直し方を決めた（task 01M74X0XAN0MCFJ4KZ3VVFT2QA の unit contract。[ADR 2026-11-11 health-generic-review-input](2026-11-11-health-generic-review-input.md)）。付記 (e) に訂正を足した。実装と試験の結果は同じ task の unit record が新しい ADR の状態の記録に書く。
