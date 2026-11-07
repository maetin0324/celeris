---
title: 健全性v2 G2 dispatch risk-events — assess_risk・設計 review と health の event・設計 review の wire 型
tasks: [01M6VTFG9K5HEE817EW05JASMZ]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G2 dispatch risk-events — assess_risk・設計 review と health の event・設計 review の wire 型

完了日: 2026-11-07

WorkUnit `risk-events`。基準は `$CELERIS_WU_BASE`。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../../adr/2026-11-07-health-review-integration.md) の D3.3・D3.5・D4.1・D4.3・D4.4。dispatcher の配線はしていない（後続の policy-load・design-gate・health-input・health-verdict・design-verdict）。

## 変更
件名「G2 risk-events: assess_risk・設計 review と health の event・設計 review の wire 型」「G2 risk-events: design_review を task-core の module の循環から外す」の commit。
- `crates/task-core/src/design_review.rs`: `ReviewPolicyState`（`Present(ReviewPolicy)`・`Absent`・`Unavailable(理由)`）、`RiskLevel`（`none`・`low`・`high`・`unknown`、`needs_design_review` は high・unknown で true）、`RiskAssessment { level, reasons }`、`high_risk_classes(policy, write_paths)`（合う class の名前を方針の順で返す）、`assess_risk(&ReviewPolicyState, write_paths, brief)`。`DesignBrief`・`DesignOption`（plan-brief unit が `execution_plan.rs` に置いた型）をここへ移し、`execution_plan` は `pub use` で同じ path（`task_core::execution_plan::DesignBrief`）のまま出す。型の中身・serde・schema は変えていない。
  - 規則は D3.3 のとおり: `Absent` → `none`（`review_policy_absent`）、`Unavailable` → `unknown`（`review_policy_unavailable:<最初の行、200 文字まで>`）、高 risk の class に合う path かつ brief が `new_state`・`new_branch`・`new_fallback`・`new_port` のどれかを宣言 → `high`（class と欄の組ごとに `path_class:<class>+<欄>`）、`new_fallback` は path に依らず `high`（`new_fallback`）、class に合い brief が無い → `low`（`brief_absent_human_plan`）、ほか → 理由なしの `low`。
  - path の当て方は `profile.py` と同じ `re.search`（`regex::Regex::is_match` は部分一致）。task-core に `regex = "1"` を足した（Cargo.lock に既にある 1.13.1。crate の依存の向きは変えていない）。
- `crates/task-core/src/execution_plan/design_brief.rs`: 計画の側の関数 `plan_write_paths`（/3 の全 unit の `expected_write_paths` と leaf の `context.paths`、/1・/2 の `work_units` の `context.paths` の和。重複なし・昇順）、`assess_plan_risk(policy, spec)`（`plan_write_paths` と計画の `design_brief` で `assess_risk` を呼ぶ）、event の鍵 `plan_sha256`（計画の JSON の SHA-256。既存の `project_proposal::sha256_hex` を使う）。`execution_plan` から再公開。
- `crates/task-core/src/model/events.rs`: `Event::DesignReviewRequested { plan_sha256, plan, risk, policy_sha256? }`・`DesignReviewed { plan_sha256, review_run, pass, findings? }`・`HealthReviewInputBuilt { review_run, repo_id, target_sha, reviewed_sha, definition_sha256, input }`・`HealthReviewInputSkipped { review_run, repo_id, reason }`。どれも状態を変えない（`replay` は無視）。
- `crates/task-api/src/query.rs`: `event_type_name` と `EVENT_TYPES`（86 → 90 語）に 4 つの type 名を足した。
- `crates/task-worker/src/protocol/design_review.rs`（新規）: `DesignReviewRequest { brief?, plan_summary, risk, review_policy? }`・`DesignVerdictOut { pass, findings? }`（finding は health verdict と同じ `HealthFindingOut`）。`protocol.rs` に `ReviewRequest.design`・`ReviewOutput.design`、`protocol/health.rs` に `HealthReviewInput.brief`。新しい欄はすべて `serde(default, skip_serializing_if)`、`PROTOCOL_VERSION` は 4 のまま。`lib.rs` の再公開に 2 型を足した。
- `crates/task-dispatch/src/review.rs`: `ReviewHumanContext::into_request` の literal に `design: None`（final review の要求）。
- 再生成: `docs/protocol/worker-protocol.schema.json`・`docs/api/v1/api-v1.schema.json`・`docs/api/v1/event.schema.json`（`UPDATE_SCHEMA=1`）、`web/api/generated/{schema.json,types.ts}`（`node web/scripts/gen-types.mjs`）。`docs/protocol/worker-protocol.md`（`context.review` の欄、§10.2 の `HealthReviewInput.brief` と「採用前の設計 review」）と `docs/api/v1/api.md`（`types` の語彙と 4 つの event の形）を書き足した。
- 試験: task-core `crates/task-core/src/design_review/tests.rs`（prefix `health_review_risk_`、9 件: D3.3 の 6 つの場合を 1 件ずつ、write path と class の集め方、risk の JSON と `plan_sha256`、4 つの event の往復と任意欄の無い JSON の読み込み）。task-worker `crates/task-worker/src/protocol/design_review/tests.rs`（prefix `health_review_design_protocol_`、4 件: 依頼の往復、verdict の往復と未知の kind の拒否、旧い JSON の読み込みと新しい欄の非出力、schema に型が載ること）。task-api `health_review_event_types_match_their_serde_names`（type 名の一致）。

## ADR との違い（判断）
- 計画から write path を集める関数（`plan_write_paths`）・計画 1 つの判定（`assess_plan_risk`）・`plan_sha256` は `design_review.rs` でなく `execution_plan/design_brief.rs` に置き、逆に `DesignBrief`・`DesignOption` を `design_review.rs` へ移した。最初の版（全部を `design_review.rs` に置いた）では、`model`（`Event` が `RiskAssessment` を使う）→ `design_review` → `execution_plan` → `model` の辺で `design_review` が task-core の module の循環に入り、repo health の `deps.cycle` が worsened（循環の単位 61 → 62）で fail になった。今の版では `design_review` は計画の型を使わず、循環に入らない（`check-health.sh` の worsened=0）。`assess_risk` は ADR のとおり `design_review` にある。
- 設計 review の依頼は `RunContext.design_review` でなく `ReviewRequest.design`（`context.review.design`）に置いた。`RunContext` の直下は既に 40 欄で、repo health の `size.responsibility-growth`（`type_fields = 40`）の閾値に当たる（[protocol の進捗](../protocol.md)）。設計 review の run は reviewer run なので、`context.review` が `Some` の run への既存の扱い（browser を渡さない `browser_policy.rs`・`browser.rs` の判定）がそのまま効く。設計 review の run の `summary`・`evidence`・`criteria` は空にする。prompt（`build_review_prompt`）が `design` を読む配線は後続の unit。
- `assess_risk` の第 1 引数は `Option<&ReviewPolicy>` でなく `&ReviewPolicyState` にした（`absent` と `unavailable` を 1 つの `None` に潰さないため。task の目的のとおり）。path class の正規表現を Rust の `regex` で読めないとき（Python の `re` だけが読む後読みなど）は `unavailable` と同じ `unknown` にする（fail closed）。
- `DesignReviewed.findings` と `HealthReviewInputBuilt.input` は `serde_json::Value`（wire 型 `HealthFindingOut`・`HealthRepoInput` の JSON の写し）にした。型は task-worker の protocol にあり、task-core は task-worker に依存しないため。task-core に同じ形の型をもう 1 つ置くと、同じ契約の二重定義になる。読む側（後続の dispatcher の unit）は `serde_json::from_value` で wire 型に戻す。
- `DesignReviewRequested.policy_sha256` は `Option`（方針を読めない `unknown` では値が無い）。`DesignReviewRequest.brief`・`review_policy` も同じ理由で `Option`。
- ADR の「brief の無い人の計画」は origin を引数に取らずに判定する。planner の計画は採用前の `require_design_brief` で差し戻されるので、class に合い brief の無い計画が `assess_risk` に来るのは planner 以外の計画だけ。daemon の repair の計画（/1）に `assess_risk` を使うかは design-gate の unit が決める。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-core -E 'test(health_review_risk_)'` → 9 passed
- `cargo nextest run -p task-worker -E 'test(/health_review_design_protocol_|health_protocol_/)'` → 10 passed（新規 4 件と既存の health_protocol_ 6 件）
- `cargo nextest run -p task-api -p task-worker -p task-core -E 'test(/schema/)'`（更新モード無し）→ 37 passed（`committed_schema_matches_generated` を含む）
- `node web/scripts/gen-types.mjs` の後の `git status` で生成物の差分が commit に入っていることを確認
- `bash scripts/dev/test-affected.sh` → 未実行（下で更新する）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → 未実行（下で更新する）

## 未解決
- `re.search` で当てるので、計画の write path が class より広い接頭辞（例 `crates/task-core/`）だと `^crates/task-core/src/store/` などの class に合わない。広い path を書いた計画は高 risk にならない。
- 配線（方針の読み込み、planner_flow での `require_design_brief`・`assess_plan_risk` の呼び出し、event の追記、reviewer prompt の設計 review の節、`review.json` の `design` の読み込み）は後続の unit。

## 提案
- write path が class の path の接頭辞になる場合（計画の path が class の path より広い場合）も合ったとみなすかを、design-gate の unit か ADR の付記で決める。合ったとみなせば上の未解決は消えるが、`re.search` の意味から外れる。
