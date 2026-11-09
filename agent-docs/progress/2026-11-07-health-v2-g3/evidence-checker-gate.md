---
title: 健全性v2 G3 evidence checker-gate — 実行の口 health_review::gate（report の起動を 1 つにし、D3.1 の判定と同梱の検査器）
tasks: [01M6YJF7HJ9J0EQHRM5SN5TWV4]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 evidence checker-gate — 実行の口 health_review::gate（report の起動を 1 つにし、D3.1 の判定と同梱の検査器）

完了日: 2026-11-09

WorkUnit `checker-gate`（task 01M6YJF7HJ9J0EQHRM5SN5TWV4）。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D1.3・D2.3・D3.1 を task-dispatch に実装した。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。経路への配線（WU・段の統合・review が `run_health_check` を呼ぶ、G2 の材料と policy の関門の付け替え）は後続の `loop-points`、daemon が同梱の所在を設定するのは `bundle-path` が行う。

## 変更
- 件名「G3 evidence checker-gate: 実行の口 health_review::gate（report の起動を 1 つにし、D3.1 の判定と同梱の検査器）」の commit
  - `crates/task-dispatch/src/health_review/gate.rs`（新規。`health_review.rs` には `pub mod gate;` の 1 行だけを足した）
    - `run_report(checker, repo, base, head, out)`: `repo_health.py --repo --base --head --policy-version 1 --out` を 1 回起こし、report の byte 列と exit code を持ち、schema を確かめて `RepoHealthReport` に読む。失敗は `ReportError`（`NotRun`・`Missing`・`Invalid`。表示は G2 と同じ理由の文）。`input.rs` の `run_report` は report の path を決めてこれを呼ぶだけにした。
    - report の型（`RepoHealthReport` と下位の型）と `REPORT_SCHEMA`・`POLICY_VERSION` を `input.rs` から移した。D2.3 に要る欄（`tool.version`・`inputs.profile.resolved`/`source`・`policy.version`・`rules[]`）を `serde(default)` で足した。
    - `run_health_check(repo_id, repo, base, head, checker, timeout) -> HealthEvidence`: `checker` は適用判定が返した `CheckerId` をそのまま使う（`source` が base なら base から出し、同梱なら `bundled_checker()` の dir。版が設定と違えば `checker_mismatch`、未設定なら `checker_missing` の Error）。検査器の有無は調べ直さない。
      - D2.3 の値: `head_sha` は `git rev-parse <head>^{commit}`、`head_tree` は `<head>^{tree}`、`base_sha` は `with_base_checker` が解決した sha、`policy_sha256` は task_core の `policy_sha256`（base の `scripts/dev/repo-health/profiles` の tree id。無ければ builtin）、`report_sha256` は report の byte 列の sha256、`tool_version` は `tool.version`、`reviewable` は `summary.reviewable`、`checker` は渡した `CheckerId`。
      - D3.1 の判定: exit 0 は Pass。exit 1 は、同じ検査器の `health.engine.policy_review_files(report, changed)` を `python3 -I`（検査器の dir を `sys.path` に入れる。`check-health.sh` と同じ呼び方）で呼び、file を返せば PolicyOnly（reason `policy_only: <files>`）、返さなければ Fail。blocking は同じ呼び出しの `engine.is_fail_grade` の finding の fingerprint を sort して 50 件まで（超えた数は Fail の reason）。exit 2 は Error（reason は必須 rule の `<status>:<id>(<reason>)`。`unsupported`・`unknown`・`error`）。時間切れ・起動の失敗は `checker_not_run`、report 無しは `report_missing`、parse 失敗・schema 違いは `report_invalid`、その他の exit は `unexpected_exit` の Error。
    - `configure_bundled_checker(dir)`・`bundled_checker()`・`read_bundled_checker(dir)`: `<release>/repo-health` の `CHECKER_TREE`（`repo-health <tree id>`・`source-size-report <blob id>`。release.sh の `sd_bundle_repo_health` が書く）から task_core の `checker_sha256` で版を計算し、process の `OnceLock` に 1 回だけ置く。別の値での 2 回目は拒否する。環境変数での上書きは置かない。
  - `crates/task-dispatch/src/health_review/policy.rs`
    - `CHECKER_DIR`・`CHECKER_SCRIPT`・`CHECKER_SIDE_FILES` の定義を消し、`task_core::required_checks::health` の定数を使う。`with_base_checker` の `on_base` は task_core の `base_has_file` を呼ぶ（git は新しい `git_stdout` で締め切りの中で走らせる）。
    - `with_base_checker` に引数 `site: CheckerSite`（`Base`・`Bundled(&Path)`）を足した。`Base` は今までどおり base から一時 dir に出し、base に検査器が無ければ `None`。`Bundled` は target を解決した後、同梱の dir をそのまま検査器にする（同梱は base から出したときと同じ repo 相対の配置）。起動の口はこの関数 1 つのまま。`BaseChecker` に `python`（同じ検査器の dir を cwd に `python3 -I -B`）と `checker_dir` を足した。`Scratch` は `gate` も使うので接頭辞を引数にした。
    - `input.rs` の 2 か所（G2 の材料と `policy_review.rs` の関門が読む policy の状態）と `load_review_policy` は `CheckerSite::Base` を渡す（base に検査器が無ければ当てない経路は変えない。付け替えは `loop-points`）。
  - 試験の import: 消した定数を参照していた試験 7 本（`dispatcher/tests/health_review_{design_gate,design_verdict,input,policy}.rs`・`health_review/{input/tests,policy_review/unknown_tests,scenario_tests}.rs`）の `use` を task_core の定数と `gate::REPORT_SCHEMA` に替えた。試験の中身は変えていない。

## 実装前 brief（規約）
1. 根本原因: repo health の起動が `input.rs` の `run_report` に経路ごとに書かれ、検査器の path の定数が `policy.rs` と task_core の 2 か所にあり、D3.1 の判定と同梱の検査器を走らせる口が無い。
2. 責務 owner: 起動と report の読み込みは `health_review::gate`、検査器の取り出しは `health_review::policy::with_base_checker`、検査器の path と有無の判定・指紋は `task_core::required_checks::health`、policy だけの変更の判定は検査器の `health.engine`。
3. 既存の再利用先: `with_base_checker`・`BaseChecker::run`・`finished`/`left`（policy.rs）、task_core の `base_has_file`・`checker_sha256`・`policy_sha256`・`HealthEvidence`、検査器の `engine.policy_review_files`・`is_fail_grade`（`check-health.sh` と同じ呼び方）。
4. 選択肢: (a) 同梱用に別の起動関数を足す → 起動の口が 2 つになるので不採用。(b) `with_base_checker` に出どころの引数を足す → 採用。policy だけの判定は (c) Rust に書き直す案を捨て、同じ検査器の python を呼ぶ。
5. 新しい状態・分岐: `CheckerSite`（base・同梱の 2 値。D1.4 の要求）、process に 1 回の `OnceLock<BundledChecker>`（D1.4。daemon の起動で設定）。fallback・切替 flag は無し。
6. 撤去対象: `policy.rs` の検査器の path の定数 3 つ、`input.rs` の report の起動の本体と report の型・定数（gate へ移した）。
7. 不変条件: 起動の口は 1 つ（`with_base_checker`）。exit 2・report 無し・時間切れは合格にしない（J4）。候補の検査器は使わない（J3）。PolicyOnly は検査器の判定だけから出る。試験は下の表。

## ADR とのずれ
- `run_health_check` の引数の先頭に `repo_id: RepoId` を足した（ADR D1.3 は `(repo, base, head, checker, timeout)`）。`HealthEvidence.repo_id` を埋めるため。
- 変更 path（`policy_review_files` の `changed`）は Rust 側で `git diff --no-renames --name-only -z --no-ext-diff <base> <head>`（検査器の snapshot と同じ引数）で取り、JSON の file で渡す。snapshot は submodule の path を除くが、gate は除かない。submodule を変えた候補は policy だけの変更にならず Fail になる（fail closed）。
- blocking（fail 級の fingerprint）は Rust で `is_fail_grade` を書き直さず、同じ呼び出しで検査器の `engine.is_fail_grade` を使う。この呼び出し（`policy_review_files`）自体が失敗したときは、PolicyOnly か Fail かを決められないので Error（`policy_review_failed`）にした。
- `with_base_checker` の `on_base` を task_core の `base_has_file` に替えたので、`cat-file -e` が失敗し `ls-tree` でも無いと確かめられない場合は「無い」ではなく失敗（`input.rs` では policy 不明、`load_review_policy` では `Unavailable`）になる。無いと確かめられた base は今までどおり `None`。
- 同梱の検査器は一時 dir に写さず、release の dir をそのまま cwd にして `python3 -I -B` で起こす（bytecode を書かない）。

## 試験（`crates/task-dispatch/src/health_review/gate_tests.rs`、9 件）
一時 git repo の base に stub の `repo_health.py`（隣の `stub.json` の report の文字列を `--out` に書き、`exit` で終わる）と `health/engine.py`（本物と同じ `is_fail_grade`・`policy_review_files`）を commit する。検査器は `repo_facts` → `applicability` が返したもの。既存の試験（`input/tests.rs`・`policy/tests.rs`・`policy_review/*`・`scenario_tests.rs`・`flow_tests.rs`）は G2 の材料と policy の関門を見るだけで、`HealthEvidence` を作る口・D3.1 の判定・同梱の検査器を通らないので、下の不変条件を検出できない。

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `health_evidence_gate_pass_binds_each_field` | exit 0 の証跡が repo_id・解決した base/head の SHA（head は ref で渡しても SHA）・head の tree・D2.3 の policy の指紋（base の profile の tree id）・適用判定の検査器・report の sha256・tool.version・reviewable に結び付く（J2） | `HealthEvidence` を作る関数が今まで無かった |
| `health_evidence_gate_exit1_is_fail_with_sorted_blocking` | exit 1 で policy だけでなければ Fail、blocking は fail 級だけを sort して 50 件、超えた数は reason | G2 の要約（`summarize_report`）は severity を見ず、sort も verdict も無い |
| `health_evidence_gate_policy_only` | 変更 path が全部 policy の file なら、同じ検査器の `policy_review_files` が返す file で PolicyOnly | G2 の関門は `policy.changed` を見るだけで、`policy_review_files` を呼ぶのは `check-health.sh`（作業ツリー）だけ |
| `health_evidence_gate_policy_mixed_is_fail` | policy と他の file の混在は report が同じでも Fail（base..head の変更 path を渡している） | 同上。変更 path を検査器に渡す経路が無かった |
| `health_evidence_gate_exit2_is_error` | exit 2 は report があっても Error、reason に unsupported・unknown の必須 rule（J4） | G2 は exit code を見ず report を要約するだけ |
| `health_evidence_gate_missing_or_broken_report_is_error` | report 無し・壊れた report・schema 違い・時間切れは exit 0 でも Error（J4）、reason の先頭の語で区別 | G2 の不明は policy の状態の `unavailable` で、exit 0 で report の無い場合を合否に写す口が無かった |
| `health_evidence_gate_bundled_checker` | base に検査器の無い repo は適用判定が同梱を返し、gate は同梱の dir で評価する。CHECKER_TREE からの版は同じ commit の base の版と同じ値。設定と違う版は Error | 同梱の検査器を走らせる口が無かった（`with_base_checker` は base だけ） |
| `health_evidence_gate_no_profile_policy_is_builtin` | base に profile の dir の無い repo の policy の指紋は builtin で計算する（契約 D2、D2.3） | policy の指紋を計算する呼び出し元が無かった |
| `health_evidence_gate_ignores_candidate_checker` | 候補が検査器・stub・engine を書き換えて合格を出しても base の検査器で評価する（J3） | G2 の I3 の試験は review 方針（`--print-review-policy`）だけで、report と判定の経路を見ない |

## 範囲の外の path
無し。`sh "$CELERIS_WU_SCOPE_PATHS"` の出力は上の「変更」の 12 path（`gate.rs`・`gate_tests.rs`・`health_review.rs`・`policy.rs`・`input.rs` と試験の import の 7 本）とこの進捗ファイル。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch -E 'test(/health_evidence_gate_/)'` → 9 tests run: 9 passed。
- `cargo nextest run -p task-dispatch -E 'test(/health_review|health_evidence/)'` → 89 tests run: 89 passed（既存の health_review の試験 80 件を含む）。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。nextest 3596 tests run: 3596 passed、11 skipped。check-health は verdict pass（exit 0、fail_grade 0、new 1 は `dup.contract` の warning の候補で、core-types が足した `UnavailableReason` と `route_select/availability.rs` の同名の型。この unit の変更ではない）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `grep -n 'pub const CHECKER' crates/task-dispatch/src/health_review/policy.rs` → 0 件。

## 未解決
- 同梱の検査器の試験は process に 1 回の `OnceLock` を設定する。nextest は試験ごとに process を分けるので他の試験に影響しない。`cargo test` で同じ process に走らせても、設定するのはこの試験だけ。
- `dup.contract` の候補（`UnavailableReason` の同名）は warning で合否に使わない。名前を分けるかは core-types の範囲。

## 提案
- `loop-points` は WU・段の統合・review から `run_health_check` を呼び、G2 の材料（`input.rs`）と policy の関門の「base に検査器が無ければ当てない」経路を適用判定と同梱に付け替える。そのとき `input.rs` の report も `run_health_check` の report を共有すれば、同じ candidate で検査器を 2 回起こさずに済む。
- `bundle-path` は daemon の起動で `configure_bundled_checker(<release dir>/repo-health)` を 1 回呼び、`repo_facts` の `bundled` に `bundled_checker().map(|b| &b.id)` を渡す。

## 本番への移行
無し（関数を足しただけで、経路には未接続。config・DB の変更は無い）。
