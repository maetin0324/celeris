---
title: 健全性v2 G2 dispatch health-verdict — 構造化 health verdict の blocking を不合格にし repair/replan に写す
tasks: [01M6VTFG9K5HEE817EW05JASMZ]
status: done
updated: 2026-11-08
---
# 健全性v2 G2 dispatch health-verdict — 構造化 health verdict の blocking を不合格にし repair/replan に写す

完了日: 2026-11-08

WorkUnit `health-verdict`。基準は `$CELERIS_WU_BASE`（範囲 check）と `git merge-base HEAD main`（check-health）。設計は [ADR 2026-11-07 健全性を設計と review に組み込む](../../../adr/2026-11-07-health-review-integration.md) の D5.2・D5.3・I6・I7。

## 変更

件名「G2 health-verdict: review.json の health verdict を読み、blocking を不合格にして repair/replan に写す」の commit。

- `crates/task-dispatch/src/health_review/verdict.rs`: `Verdict::from_review_json(text, health_requested, criteria, tag, review_rel)`。reviewer run の `review.json` を受け入れ条件ごとの `Verdict` にする。`Err` は全 criterion を不合格にする reason で、呼び出し側は既存の `fail_all` に渡す。
  - D5.2・I6・I7: `health_requested`（`ReviewRequest.health` を渡した）のとき、`health` を JSON から外して別に読む。欄が無い・`null`・未知の kind・未知の severity・object でない、のどれでも reason は `{tag}: health verdict missing or invalid in review.json`。`verdicts` 自体が読めなければ従来の reason（`is not a valid ReviewOutput`）のまま。
  - D5.3: blocking の finding 1 件ごとに `Verdict { criterion_idx: reviewer の受け入れ条件の最小の index, pass: false, reason: "{tag}: health:<kind>: <reason>", repair_hint }` を末尾に足す。`removal_missing`・`test_change_unjustified`（`HealthFindingKind::allows_local_repair`）で `repair.scope = "local"` のものだけ reviewer のヒントを写す。ほかは `repair_hint = Some({scope: "substantive", class: <kind>})`（下の「ADR との違い」）。`classify_review_failure` は変えていない。
  - advisory: `Verdict { pass: true, reason: "{tag}: health:<kind> (advisory): <reason>" }` を先頭に足す。`Event::ReviewVerdict` に写り、次の run の `prior_review`（最後の review run の verdict 全部）に載る。合否は変えない。
  - `health_requested = false`（`[review]` の無い repo、WU の review、health の材料を作らなかった review）は `ReviewOutput` 全体を 1 回で読み、`health` を見ない（従来と同じ）。
- `crates/task-dispatch/src/review.rs`: `run_reviewer_inner` の `review.json` の読み込みと条件ごとの写像（34 行）を `Verdict::from_review_json(..).or_else(fail_all)` の 1 行（と注記 1 行）に置き換えた。`req` を adapter に渡す前に `health = matches!(&req.context.review, Some(r) if r.health.is_some())` を 1 行で取る。`ReviewOutput` の import を消した。`ReviewRequest.health` を埋める配線は health-input の unit（`into_request` の `health: None` はそのまま）。
- 試験 `crates/task-dispatch/src/health_review/verdict/tests.rs`（prefix `health_review_verdict_`、8 件）。分類はディスパッチャ（`review_verdict.rs`）と同じく、不合格の verdict を `Check::Reviewer` の `FailedCheck` にして `classify_review_failure` に通す。
  - `missing_health_fails_every_criterion`: 欄なし・`null` で全条件が同じ reason の不合格。
  - `unknown_kind_or_severity_fails_every_criterion`: 未知の kind・severity・object でない値。`verdicts` が壊れていれば従来の reason。
  - `removal_missing_with_local_scope_repairs_locally`: reviewer の最小の条件（1）・ADR の reason・ヒントのまま → `ReviewerLocal`。
  - `dual_source_of_truth_replans`: reviewer が local と書いても `Substantive`。局所の finding と混ざっても `Substantive`。
  - `seven_kinds_follow_the_d5_1_table`（受け入れ条件 0）: 表を写した期待値（`allows_local_repair` を使わない）で、7 種それぞれの local あり・repair なし・local 以外の scope の分類。
  - `substantive_reason_mentioning_format_still_replans`: reason に `format` を含む substantive の種別も `Substantive`。同じ reason でヒントが無ければ既存の規則で format の局所修復と推測されることも確かめた。
  - `advisory_only_passes_and_is_recorded`（受け入れ条件 2）: 全部合格・分類なし。安定 sort の後も条件の最後の verdict は reviewer 自身の判定。`Event::ReviewVerdict` に写すと `prior_review_from_events` に advisory が載る。blocking と混ざれば blocking だけが合否を決める。
  - `without_health_request_keeps_previous_behavior`: health を渡さない review では、health の欄が無い・空・blocking を含む・未知の kind のどれでも従来どおり。

## ADR との違い（判断）

- 局所修復に写らない blocking の finding（5 種の全部と、2 種で local でないもの）は、ADR D5.3 と task の目的の「`repair_hint` は `None`」でなく `Some({scope: "substantive", class: <kind>})` にした。`None` では `classify_one` が `classify_reviewer_reason` に回し、reason に `fmt`・`format`・`lint`・`clippy` を含むと `ReviewerLocal(Format|Lint)` と推測する（例: 「the config format is parsed twice」の `duplicate_implementation` が局所修復になる）。D5.1 の表の「substantive → replan」を reviewer の文面に依らず守るため、`"local"` 以外の scope を substantive と扱う既存の規則を使った。`classify_review_failure` は変えていない。
- 写像は ADR の `review_verdict.rs`（dispatcher/）でなく task の計画どおり `health_review/verdict.rs` に置いた。`review.rs` からは `crate::health_review::…` の path でなく `Verdict` の関数として呼ぶ。path で呼んだ最初の版は、`review` → `health_review` → `dispatcher`（policy-load の `impl Dispatcher`）→ `review` の循環になり、check-health の `deps.cycle` が worsened（循環の単位 3 → 4）で落ちた。policy-load の `impl Dispatcher` と同じく、`health_review` に `impl Verdict` を置いた。
- advisory の記録は新しい event の種類を作らず、合格の `Event::ReviewVerdict` にした（ADR D5.1 の「`Event::ReviewVerdict` の記録と次の run の `prior_review`」）。同じ criterion の最後の verdict を採る読み手（`task-ops` の `root_done.rs`・`view.rs`）が advisory を判定と読まないように、advisory は reviewer 自身の判定より前に置く。

## size の制約

`review.rs` は 5 行増え 36 行減った（差し引き 31 行減。増えたのは import の折り返し 2 行、`health` の 1 行、注記と呼び出しの 2 行）。新しい処理は `health_review/verdict.rs`（166 行）と試験（326 行）。check-health は verdict=pass・new=0・worsened=0。

## 証拠（提出時の HEAD で実行）

実装の commit の HEAD で実行した。その後の commit は、この進捗 file だけを変える。基準は `$CELERIS_WU_BASE`（範囲 check）と `git merge-base HEAD main`（check-health・test-affected）。

- `cargo nextest run -p task-dispatch -E 'test(health_review_verdict_) | test(/^review::tests::/)'` → exit 0、42 passed（`health_review_verdict_` 8 件、既存の review の試験 34 件）
- `bash scripts/dev/test-affected.sh` → exit 0（対象 crate: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）。check-health は verdict=pass・new=0・worsened=0。nextest 3,519 件 passed（11 skipped）。doc test も通った。script bundle SKIP 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- 範囲 check（`git diff --name-only $CELERIS_WU_BASE HEAD` を計画の許可 path で絞る）→ 範囲外の path なし。`sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は `health_review/verdict.rs`・`health_review/verdict/tests.rs`・`review.rs`（とこの進捗 file）。

## 未解決

- `ReviewRequest.health` を埋める配線（health-input の unit）がまだ入っていないので、この WU の branch だけでは本番の review で `health_requested` は常に false。流れ全体での確認は scenario の unit。
- D6.1 の `policy_mixed`、D6.3 の `HealthVerdictOut.policy` の必須化、D6.4 の人の承認は policy-review の unit。この unit は `policy` の欄を読むが使わない。
- advisory の finding の `path` と blocking の `path` は reason に入れていない（ADR の reason の形のまま）。repair WU の objective には reviewer の `reason` の文だけが渡る。

## 提案

- close の unit で ADR に付記する: D5.3 の「ほかの 5 種は `None`」を「`scope = "substantive"` のヒント（reason の文面からの推測を止める）」に改める。写像の置き場所は `health_review/verdict.rs`（`impl Verdict`）。
- 必要なら reason に `path` を足す（`{tag}: health:<kind>: <path>: <reason>`）かを ADR で決める。repair WU が直す場所を探す手間が減る。
