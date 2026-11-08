---
title: 健全性v2 G2 dispatch scenario — 通常変更と高 risk 変更を流れ全体で区別する試験
tasks: [01M6VTFG9K5HEE817EW05JASMZ]
status: done
updated: 2026-11-08
---
# 健全性v2 G2 dispatch scenario — 通常変更と高 risk 変更を流れ全体で区別する試験

完了日: 2026-11-08

WorkUnit `scenario`。基準は `$CELERIS_WU_BASE`（範囲 check）と `git merge-base HEAD main`（check-health）。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../../adr/2026-11-07-health-review-integration.md) の (6)。前の unit は [design-verdict](design-verdict.md)・[policy-review](policy-review.md)・[health-input](health-input.md)・[health-verdict](health-verdict.md)。

## 変更
件名「G2 scenario: 通常変更と高 risk 変更を流れ全体で区別する試験と本物の --print-review-policy の読み込みの試験」の commit と、この進捗の commit。実装（試験以外の code）は変えていない。
- `crates/task-dispatch/src/health_review/scenario_tests.rs`（新規。`dispatcher/tests/mod.rs` から `#[path = "../../health_review/scenario_tests.rs"] mod health_review_scenario;` で読み、`dispatcher::tests` の道具を使う）。試験 3 件:
  - `health_review_scenario_normal_change`: 案件の repo（一時 git repo）の main に `[review]` を出す stub の検査器と owner 地図を置き、reviewer の受け入れ条件を持つ compound task を planner → 採用 → WU → final review まで回す。高 risk の class に触れない計画（`crates/foo/src/lib.rs`）は `DesignReviewRequested` も設計 review の run も無く採用され、final review の `ReviewRequest.health` に diff 統計（変えた path）・owner 地図の行（`| foo |`）・findings の要約（new 1 件 `size.responsibility-growth`）が入り、task は `done`。同じ流れを検査器の無い repo（`[review]` 無し = 従来）でも回し、reviewer run の数（1）と run の総数が同じで、従来の側は health が `None` であることを比べる。
  - `health_review_scenario_high_risk_change`: store の class の path（`crates/task-store/src/rows.rs`）と `new_state` を宣言した brief の計画は、`DesignReviewRequested`（risk `high`）→ 設計 review の run（1 本）→ `DesignReviewed { pass: true }` → `ExecutionPlanned` の順で採用され、WU が 1 回走る。final review の health に変えた path と採用した計画の brief が入り、reviewer が blocking の `dual_source_of_truth` を返すと `ReviewVerdict { pass: false, reason: "reviewer(…): health:dual_source_of_truth: …" }` で不合格になる。repair の WU は作られず、planner が `replan = true` で起き、`replan_reason` に `dual_source_of_truth` が入る。
  - `health_review_scenario_real_cli`: repo の本物の `scripts/dev/repo-health/`（`repo_health.py` と `health/`・`profiles/`・`rules/`・`schema/` 等、`__pycache__` は除く）と side file の `scripts/dev/source-size-report.py` を一時 git repo に写して commit し、(a) `repo_health.py --print-review-policy --repo <repo> --base <commit>` を直接起こした出力の key の組が stub と同じ 6 つ（`schema`・`source`・`owner_map`・`high_risk`・`invariants`・`definition_sha256`）で `source = base` であること、(b) 作業ツリーの `profiles/default.toml` を `[review]` の無い内容に書き換えた後でも、policy-load の loader（`load_review_policy`）が base の commit から `Present` を返し、`definition_sha256`・`high_risk`・`invariants` が (a) の出力と一致し、`owner_map = docs/architecture-map.md`・class `store`（`^crates/task-core/src/store/` を含む）・`credential`・`sandbox` を持つことを確かめる。
- `crates/task-dispatch/src/dispatcher/tests/mod.rs`: 上の `#[path]` の mod 宣言と 2 行の doc comment（試験の file。size の規則の対象外）。

## 判断
- 試験の file は `health_review/` の下に置き、`dispatcher/tests/mod.rs` から `#[path]` で読む。fake harness の dispatcher 試験の道具（`worktree_dispatcher`・`project_with_repos`・`compound_task`・`run_until_idle`・`tree::leaf` 等）は `dispatcher::tests` の private な item なので、`health_review` の top-level から `crate::dispatcher::tests` を読むことはできない。file 名 `scenario_tests.rs` は repo health の adapters の分類で test になり、`size.responsibility-growth` に数えない。
- 「reviewer run の数は従来と同じ」は、検査器の無い repo（方針 `Absent`）で同じ計画・同じ adapter の流れを回した数との比較で確かめた（固定の数だけでなく、従来の流れと並べる）。
- 試験の adapter は、計画を持たない planner の試行で `artifacts/execution-plan.json` を消す。task の artifacts dir は run をまたいで同じで、消さないと replan の planner 試行が前の試行の file を残し、dispatcher がそれを読んで同じ計画を採用し直す（下の「未解決」）。
- 実装の不足は見つからなかった。stub と本物の `--print-review-policy` の JSON の形は一致し、loader の修正は要らなかった。

## 証拠（提出時の HEAD で実行）
この WU では main の取り込み・rebase をしていない。下の検査は試験の commit（件名「G2 scenario: 通常変更と高 risk 変更を流れ全体で区別する試験と本物の --print-review-policy の読み込みの試験」）の後に実行した（後の commit はこの進捗の file だけ）。
- `cargo nextest run -p task-dispatch -E 'test(health_review_scenario_)'` → exit 0、3 passed（受け入れ条件 0・1）
- `cargo nextest run -p task-dispatch -p task-core -p task-worker -E 'test(health_review_)'` → exit 0、69 passed
- `bash scripts/dev/test-affected.sh` → exit 0（456 秒）。check-health は verdict=pass・new=0・worsened=0（existing 718）、repo_health_gate の試験は全部通過。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch、nextest は 3555 passed・11 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy -p task-dispatch --tests` の出力に `scenario_tests.rs` の警告は無い
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 変更は `crates/task-dispatch/src/dispatcher/tests/mod.rs`・`crates/task-dispatch/src/health_review/scenario_tests.rs` とこの進捗だけ（範囲外なし）

## 未解決（後続の unit へ）
- replan の planner の試行が `execution-plan.json` を書かずに終わると、dispatcher は前の試行が残した同じ file を読み、同じ計画を版 2 として採用し直す（試験の adapter が file を消さなかったときに `ExecutionPlanned { version: 2, reason: "replan (planner run) (added=0, changed=0, removed=0)" }` と 2 本目の設計 review が起きた）。設計 review の不合格の計画は `execution-plan.rejected.*` に退けるが、採用した計画の file は残る。本番の harness で同じことが起きるかは未確認（この unit の範囲外。実装は変えていない）。

## 提案
- planner の replan の run を起こす前に、前の試行の `artifacts/execution-plan.json` を退ける（`execution-plan.rejected.*` と同じ扱い）か、run の開始より古い file を読まないようにする unit を、G2 の後に置く。
