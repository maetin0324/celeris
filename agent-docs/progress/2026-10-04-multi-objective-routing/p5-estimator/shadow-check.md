---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: shadow-check
status: done
completed: 2026-10-05
---

# Phase 5 shadow-check: 偽 sidecar で adapter が Phase 4 shadow 評価に差し込めるかの結合検証と report 草稿

## 変更

- `crates/celeris/tests/routing_estimator_shadow.rs`（新規）: `routing_estimator_sidecar_plugs_into_shadow_eval`。
  一時 DB・in-process proxy（127.0.0.1:0）・偽上流 2 本・loopback の偽 sidecar・`FixedClock`。userns 不要。
  run 4 本で completed 2 / failed 1（`request_id` 違反 → `upstream_error`）/ dropped 1（`cap_exceeded`）を DB の
  `routing_shadow_recorded` に記録し、`routing_replay::export` → `evaluate_with_estimator`（celerisctl と同じ関数）で
  coverage 0.5・差・失敗理由を確認。primary 不変（応答と `primary_model`）、`constraint_violations = 0`、
  sidecar 送信 = 上限 3。daemon の `pub(crate)` 配線（`shadow_settings`・`SidecarDailyBudget`・
  `append_shadow_event`）は外から呼べないため試験内で鏡写し（daemon 内部試験は `routing_sidecar_tests.rs`）。
- `crates/celeris/tests/routing_estimator_sidecar_validate.rs`（新規、`#[ignore]`）: 実 process の応答を
  `EstimateResponseV1::validate` に通す。入力 dir が無ければ失敗（skip を合格にしない）。改ざん応答の拒否も確認。
- `scripts/model-routing/fake-shadow-check.sh`（新規）: 引数なし。偽 classifier の wrapper を ephemeral port で起動 →
  `/healthz` から descriptor → fixture request（prompt なし・あり）→ 上の validate 試験 → SIGTERM・exit 0・PID 消滅・port 閉鎖。
- `docs/reports/model-routing-routellm-shadow.md`（新規、草稿）: 偽 sidecar の結果、見つかった食い違い、
  実 sidecar の欄は全て「未実行: 人の手順待ち」。

## 証拠

- `cargo nextest run -p celeris --no-fail-fast -E 'test(routing_estimator_sidecar_plugs_into_shadow_eval)'` → exit 0、1 test run: 1 passed
- `sh scripts/model-routing/fake-shadow-check.sh` → exit 0（validated 2 sidecar response(s)、port closed）
- `f=docs/reports/model-routing-routellm-shadow.md; test -s $f && grep -q '未実行' $f && sh scripts/dev/check-doc-links.sh` → exit 0
- `sh scripts/model-routing/check-runbook.sh` → ok、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok
- `bash scripts/dev/test-parallel.sh` → exit 0、4034 tests run: 4034 passed、13 skipped
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo clippy -p celeris --all-targets -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- 範囲 check（scope）→ 範囲外 path なし

## 未解決事項（結合試験が回避している食い違い。実 sidecar の評価前に直す）

1. decision id の結合: estimator shadow の `primary_decision_id` は proxy の `pdec_…`。export は `decision_id` 一致の行にしか
   shadow を付けないので、dispatch の decision id の行には付かない。試験は shadow の id で `RoutingDecided` を書いて回避。
2. version 照合: daemon の `policy_version` は `estimator:<id>/<v>`、`task-ops routing_replay/estimator.rs::version_matches` は
   `estimator:` 接頭辞を受けない。正直な pin は `estimator_version_mismatch`。試験は照合が直るまで `estimator:route-test` で pin し直す
   （`KNOWN GAP` を stderr に出す。直れば正直な経路を通る）。
3. model 名: shadow の `model` は上流 wire model 名、dataset は model profile id。`same_as_heuristic`/`same_as_primary` は意味を持たない。
- 実 sidecar（実 weights）の評価は未実行（決定 `routellm-weights-use` 待ち）。

## 提案

- close 葉（または別葉）で 1・2 を task-ops / llm-proxy 側で直す: export は `parent_decision_id` か run で shadow を結ぶ、
  `version_matches` に `estimator:<id>/<v>` を足す。3 は shadow 記録に model profile id を持たせる。直した後、試験の回避を外す。
