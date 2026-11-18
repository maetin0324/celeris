---
title: アーキ v4 C profile — [review] に境界の class と class ごとの不変条件を足す（policy の唯一の leaf）
tasks: [01M7MZTKB29VF5TX7FCQ15XEMW]
status: done
updated: 2026-11-18
completed: 2026-11-18
---
# profile（WorkUnit）

ADR: [2026-11-17-boundary-change-risk-review.md](../../adr/2026-11-17-boundary-change-risk-review.md) の D2・D6。基準は `$CELERIS_WU_BASE`。
policy の file を変える唯一の leaf。人が差分を承認し（2026-11-18）、operator が main に hand merge した（下の「人の承認と merge」）。

## 人が決めたこと（merge の前に聞いた。回答は下の「人の承認と merge」）
1. 下の「policy の差分の要約」を承認するか。
2. **merge の順**: この差分を main に入れると、main の Rust の試験 `health_review_scenario_real_cli`（task-dispatch、
   `crates/task-dispatch/src/health_review/scenario_tests.rs:581`）が 1 件落ちる。daemon は新しい JSON を読める（下の「Rust の読み込み」）。
   並行の leaf `risk-rule` の branch は `PathClass.invariants`（`#[serde(default, skip_serializing_if = "Vec::is_empty")]`）を足してあり、
   その branch とこの branch を合わせた木では同じ試験が通る（下の「Rust の読み込み」の 2 回目の run の実測）。task の質問で次のどれかを選ぶ:
   - `rust-first`（推奨）: `risk-rule` の `PathClass.invariants` の欄の追加（欄・試験の literal・生成 schema。risk の規則の変更は含めない）だけを
     小さな task で先に main に入れ、その後に operator がこの branch を merge する。main の試験は落ちない。C は小さな task が main に入るまで待つ。
     C の branch の main 同期で `risk-rule` と同じ変更が衝突したら、`risk-rule` の側を残す。
   - `merge-now`: この差分のまま今 merge する。C が main に入るまで、main で上の試験が落ちる（task-dispatch とその依存を変える他の task の
     test-affected と release gate が落ちる）。
   - `two-step`: この branch から class ごとの `invariants` の 13 行を外して今 merge する（class・path・`[review.invariants]` の 4 名・検査器・schema は残す）。
     class ごとの `invariants` は C が main に入った後に別の policy の task で足す。main の試験は落ちないが、C の実 profile では risk の理由
     （`invariant:<class>:<名前>`）と prompt に不変条件が出ない。
   1 回目の run は `rust-first` を推奨せず `two-step` を推奨した。2 回目の run で `risk-rule` の branch と合わせた木で試験が通ることを確かめ、
   `two-step` では C の受け入れ条件 0（判定の記録に不変条件を出す）を実 profile で示せないため、推奨を `rust-first` に変えた。

## 実装前 brief
1. 根本原因: profile の 7 class に、監査で見つかった境界の file（`task-api/src/knowledge.rs`・`cos/operations.rs`・`dispatcher/worker_finish.rs`・
   `task-worker/src/browser_policy.rs` ほか ADR D2 の表）が合わない。class ごとに守る不変条件を書く欄が無い。
2. 責務 owner: `scripts/dev/repo-health/profiles/celeris.toml` の `[review]`、`health/profile.py`（`_validate_review`・`review_policy`）、
   `schema/review-policy-v1.schema.json`。
3. 既存の再利用先: `_validate_review`・`_check_regexes`・`Profile.review_policy`・extends の `_merge`、既存の試験 `test_repo_health_review_policy.py`。
4. 選択肢: (a) high_risk の項目に任意の `invariants` を足す（採用。ADR D2・D6。現行の daemon は項目の未知の欄を捨てるので読める）。
   (b) top level に class → 不変条件の表を足す（不採用。`ReviewPolicyOut` の `deny_unknown_fields` で現行の daemon が `Unavailable` になる）。
5. 新しい状態・分岐・fallback・port: port は high_risk の項目の任意の欄 `invariants` 1 つ（profile・JSON・schema）。状態・fallback は無し。
   検査器の分岐は「`invariants` を書いた class だけ JSON に出す」1 つ（書かない repo の出力の形を変えない）。
6. 撤去対象: 無し（欄の追加。旧い経路は無い）。
7. 不変条件と試験: 境界の file が期待の class に合い、web/・docs/ の追跡 file はどの class にも合わない。未知の不変条件の名前・形の違う値は読み込みエラー。
   各 path の式が追跡 file に当たる（既存の試験）。試験は下の「検査」。

## 変更（commit の件名）
- `arch-v4 C profile: [review] に境界の 6 class と selfdeploy の instance.rs、class ごとの invariants を足し、検査器と schema が invariants の形と名前を検査する`

## 人が承認する policy の差分の要約
policy の file は 3 つ（`git diff $CELERIS_WU_BASE HEAD -- scripts/dev/repo-health` で全文を見られる）。

| file | 変更 |
|---|---|
| `scripts/dev/repo-health/profiles/celeris.toml` | 新しい class 6 つ: `authz`（`task-api/src/principal.rs`・`middleware.rs`・`handlers/authority.rs`・`task-core/src/authority.rs`・`task-api/src/cos/operations.rs`）、`transaction`（`task-api/src/knowledge.rs`・`task-ops/src/lifecycle.rs`）、`event-apply`（`task-core/src/transition.rs`・`task-ops/src/replay.rs`）、`browser`（`task-worker/src/browser_policy.rs`・`browser_credential.rs`・`browser_cdp_credential_ipc.rs`・`browser_launcher_run.rs`・`task-api/src/browser_credentials.rs`）、`worker-finish`（`task-dispatch/src/dispatcher/worker_finish.rs`）、`review-gate`（`task-core/src/design_review.rs`・`task-dispatch/src/health_review/{design,design_gate,design_verdict,policy,policy_review,gate}.rs`）。既存の `selfdeploy` に `crates/celeris/src/instance.rs` を足した。13 class すべてに `invariants`（ADR D2 の表のとおり）。`[review.invariants]` に 4 名（`api-principal-from-token` → `agent-docs/adr/2026-10-27-api-principals.md`、`cos-audit-same-tx` → `2026-10-09-cos-operations-all-mutations.md`、`lifecycle-human-epoch-same-tx` → `2026-11-04-project-controller.md`、`policy-change-needs-human` → `2026-11-07-health-review-integration.md`）。既存の 7 class の path は変えていない |
| `scripts/dev/repo-health/health/profile.py` | high_risk の項目に任意の `invariants` を許す（空でない文字列の重複の無い配列、1 件以上）。名前は extends を解決した後の `[review.invariants]` の key に限り、外れたら `profile_invalid`（report と `--print-review-policy` の両方）。`--print-review-policy` は `invariants` を書いた class だけに出す |
| `scripts/dev/repo-health/schema/review-policy-v1.schema.json` | `high_risk[]` の項目に任意の `invariants`（文字列の配列、1 件以上）。schema の id（`/1`）と top level の欄は変えていない |

根拠の file の節を読んで名前の意味と合うことを確かめた: api-principals は D2（guard が Bearer を照合し `Principal` を載せる）、cos-operations は
`OperationAudit::apply` の 1 transaction（:375）、project-controller は D12 の human_epoch（:186、同じ transaction で増やす）と I6（:123）、
health-review-integration は I8（:106）。

policy の file でない変更: 試験 `scripts/tests/test_repo_health_review_boundary.py`（新規、7 件）とこの進捗 file。

## Rust の読み込み（main の code、Rust は変えていない）
- `PathClass`（`crates/task-core/src/design_review.rs:41-45`）は `deny_unknown_fields` が無く、項目の `invariants` を捨てる。`ReviewPolicyOut`
  （`crates/task-dispatch/src/health_review/policy.rs:47-55`）の `deny_unknown_fields` は top level だけに効く。
- 実測: `cargo nextest run -p task-dispatch -E 'test(/health_review_scenario_real_cli|health_review_policy|health_enforce_g2_review_policy/)'`
  → 32 件中 31 件 pass、1 件 fail。落ちたのは `health_review_scenario_real_cli` の `serde_json::to_value(&policy.high_risk) == raw["high_risk"]`
  （:581。読んだ値を書き直すと `invariants` が消える）。その前の `load_review_policy` は `Present` を返した（:577 の panic は起きない）。
  `definition_sha256` の比較も通った。
- 結論: 現行の daemon は新しい JSON を読める（方針は `Present`）。ただしこの差分だけを main に入れると上の試験が落ちる（「人が決めること」2）。
- 2 回目の run の実測（同じコマンド）: この branch の HEAD では 32 件中 31 件 pass（落ちたのは同じ :581 の比較）。
  `risk-rule` の branch の head にこの branch を merge した使い捨ての worktree（`$TMPDIR`、commit しない）では 32 件すべて pass。
  注意: 同じ `CARGO_TARGET_DIR` で別の worktree の木を build すると、元の worktree の試験の binary が古い木の path（`CARGO_MANIFEST_DIR`）のまま
  新しいと判定される（使い捨ての worktree を消した後に `copy_checker` が `NotFound` で落ちた）。`git ls-files -z crates | xargs -0 touch` で作り直した。

## 検査（merge の前の run で実行）
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_review_*.py'` → 26 件 OK（新しい 7 件と既存の review policy の 19 件。
  既存の `test_health_review_celeris_profile_points_at_existing_paths` が各 path の式が追跡 file に当たることを確かめる）。
- 失敗注入（commit しない）: 名前の検査（`_check_review_invariant_names` の呼び出し）を外す → `test_boundary_unknown_invariant_name_is_load_error`・
  `test_boundary_invariant_names_resolve_after_extends` が fail。`authz` から `cos/operations` の行を消す → `test_boundary_audit_gaps_and_adr_files_match_their_class` が fail。
- `bash scripts/dev/check-health.sh --out <artifacts>/health-report.json` → exit 1。必須 rule のうち fail は `policy.changed` だけで、他の 14 件は pass
  （想定どおり。修復しない）。
- `bash scripts/dev/test-affected.sh` → exit 1（対象 crate は 0 件。先に走る check-health の `policy.changed` で落ちる）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 人の承認と merge
- 人の決定（2026-11-18。認可 01M7NX38P09PR9JTTAPDA96VW2 への回答を外部の運用者が task のコメントで伝えた）:
  - この leaf の policy の差分（`profiles/celeris.toml` の境界の 6 class と selfdeploy の `instance.rs`、全 class の `invariants`、
    `[review.invariants]` の 4 名、`health/profile.py` の検査、`review-policy-v1.schema.json`）を承認する。
  - main に入れる順は `rust-first`（推奨案）。
- 運用者がしたこと（コメントのとおり。この run は件名で main の履歴にあることを確かめた）:
  - 前提の小さな task 01M7NZW0BSD3FR38XMJG190BB4 を起票し、main に入れた。件名「ADR: PathClass に任意の欄 invariants を先に足す決定（アーキv4 C の D2・D6 の一部）を記録する」
    「PathClass に任意の欄 invariants を足し、往復試験と schema を更新する」。
  - この WU の branch を main に no-ff で merge した。件名「integrate 01M7MZTKB2 profile: [review] に境界の 6 class と class ごとの不変条件を足す（policy の branch、人の承認と operator の hand merge）」。
    task の branch もその merge へ fast-forward した。WU の branch は fast-forward していない（merge の第 2 親がこの branch の 1 つ前の run の head）。
- この run は差分を作り直していない（自分の変更を自分で承認していない）。この run の変更はこの進捗 file だけ。

## 再実行の run の検査（提出時の HEAD で実行。2026-11-18）
- merge の commit: `m=$(git log --format=%H --grep='^integrate 01M7MZTKB2 profile' -n 1 main HEAD)` → 1 件見つかった（親は 2 つ）。
  `git diff --name-only $m^1 $m` → 8 file: `scripts/dev/repo-health/{health/profile.py,profiles/celeris.toml,schema/review-policy-v1.schema.json}`、
  `scripts/tests/test_repo_health_review_boundary.py`、`agent-docs/adr/2026-11-17-boundary-change-risk-review.md`、`agent-docs/progress/2026-11-17-arch-v4-c.md`
  と同名 dir の `adr.md`・`profile.md`。許可した path（policy の file・新しい試験・C の ADR と進捗）の外は無い。
  `git diff --stat HEAD $m^2` → 差分なし（merge した木はこの branch の HEAD と同じ）。`git diff --stat HEAD main -- scripts/dev/repo-health scripts/tests/test_repo_health_review_boundary.py` → 差分なし。
- `python3 -I -m unittest discover -s scripts/tests -p 'test_repo_health_review_*.py'` → 26 件 OK（新しい 7 件と既存の review policy の 19 件）。
- `bash scripts/dev/check-health.sh --out <artifacts>/health-report.json` → exit 0。必須 rule 15 件と任意 rule 6 件がすべて pass（`policy.changed` も pass。
  `--base auto` は `git merge-base HEAD main` で、merge の後は HEAD になり、policy の差分が比較に入らないため）。
- Rust の読み込み（main の木。operator は回していない）: `git worktree add --detach $TMPDIR/main-tree main` の木で
  `cargo nextest run -p task-dispatch -E 'test(/health_review_scenario_real_cli|health_review_policy|health_enforce_g2_review_policy/)'`
  → exit 0、32 tests run: 32 passed, 1865 skipped。merge 前にこの branch だけで落ちた `health_review_scenario_real_cli` の往復の比較
  （class ごとの `invariants`）が main で通る。使い捨ての worktree は消し、この worktree の `crates` を touch した（上の注意と同じ理由）。
- clippy・fmt・test-affected・progress-index（提出時の HEAD）: 下の「提出時の検査」。

## 提出時の検査（提出時の HEAD で実行。2026-11-18。この run に main との同期は無い）
- `bash scripts/dev/test-affected.sh` → exit 0（repo-health: verdict=pass、`policy.changed` pass、「試験対象の変更なし（docs のみ等）」）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0、出力なし。
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 5 path（`profile.md`・policy の file 3 つ・新しい試験）。objective の範囲の外は無い。

## 未解決
- 無し（merge の順は人が `rust-first` に決め、main に入った）。

## 提案
- WU の run が `decisions` を書き、check が落ちたときも決定の要求を記録する（今は `reads_worker_decisions` が reason `completed` だけを読み、
  決定が黙って捨てられる）。policy の leaf のように check が人の操作の前に必ず落ちる leaf では、決定が人に届かない。
- Rust の `PathClass.invariants` を足す leaf（ADR D6 の最後の項。`prompts` か `risk-rule`）は、`health_review_scenario_real_cli` の往復の比較が
  `invariants` を含めて通ることを受け入れ条件に入れる。
