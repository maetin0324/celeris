---
title: PathClass.invariants の追加（アーキ v4 C 前提、unit invariants）
tasks: [01M7NZW0BSD3FR38XMJG190BB4, 01M7NZXF1WCP8ADXKK1ENGS1PW]
status: done
updated: 2026-11-18
---
# PathClass.invariants（unit invariants）

完了日: 2026-11-18

## したこと
- ADR `agent-docs/adr/2026-11-18-pathclass-invariants-field.md` を追加した（C の D2・D6 のうち欄の部分を先に入れる。risk の規則・reason・prompt の変更は C が行う）。
- `PathClass.invariants`（`#[serde(default, skip_serializing_if = "Vec::is_empty")]`）を追加した。`assess_risk`・`high_risk_classes`・reason・prompt は変えていない。
- `PathClass {` の全 literal（7 file の試験）に `invariants: vec![]` を足した。
- `docs/protocol/worker-protocol.schema.json` を生成の手順で作り直した（下の条件 2）。
- 往復の 2 試験（`path_class_invariants_survive_round_trip`、`path_class_without_invariants_round_trips_without_field`）を追加した。
- `scripts/dev/repo-health/` と profile の `policy_paths` は変えていない。

## commit の順（基準 `git merge-base HEAD main`）
1. `ADR: PathClass に任意の欄 invariants を先に足す決定（アーキv4 C の D2・D6 の一部）を記録する` — ADR 1 file だけ。
2. `PathClass に任意の欄 invariants を足し、往復試験と schema を更新する` — 欄・literal・生成 schema・往復の試験。
3. `repair: PathClass.invariants の進捗に失敗注入・schema の生成手順・条件ごとの証拠を書く` — この進捗ファイル。

経緯: 前の run（WU invariants）は ADR と実装を 1 つの commit にまとめた。reviewer が「実装の前に ADR を commit した事実を確認できない」と指摘したので、repair run（2026-11-18）で task branch を `git merge-base HEAD main` から作り直し、ADR の commit を実装の commit の前に置いた。ADR の本文と実装の内容は前の run のものから変えていない（作り直しの前後で ADR・crates・schema の tree の差分はゼロ。違いは進捗ファイルだけ）。

## 条件ごとの証拠（提出時の HEAD で実行）

### 条件 0: 欄の形と差分の範囲
- `git diff --stat $(git merge-base HEAD main) HEAD` の対象は ADR・進捗・`crates/task-core/src/design_review.rs`・試験 7 file・`docs/protocol/worker-protocol.schema.json` だけ。`scripts/dev/repo-health/` の差分は無い。

### 条件 1: 往復の試験と失敗注入
- `cargo nextest run -p task-core -E 'test(/path_class_/)'`: exit 0、2 tests run: 2 passed。
- 失敗注入（`crates/task-core/src/design_review.rs` の欄を一時的に書き換え、各回の後に `git checkout -- crates/task-core/src/design_review.rs` で戻した）。どの回も同じコマンドを実行した:
  - A. 欄を JSON から消す（`#[serde(skip)]` に置き換え。`deny_unknown_fields` の無い型が未知の欄を読み捨てるのと同じ挙動）: exit 100、`path_class_invariants_survive_round_trip` が FAIL（`left: []`、`right: ["events-append-only", "no-data-loss"]`）。1 passed, 1 failed。
  - B. `skip_serializing_if` を外す（`#[serde(default)]` だけ）: exit 100、`path_class_without_invariants_round_trips_without_field` が FAIL（出力に `"invariants": Array []` が出る）。1 passed, 1 failed。
  - C. 欄（doc comment・attribute・`pub invariants`）を消す: exit 101、`task-core`（lib test）の compile が失敗（`error[E0560]: struct PathClass has no field named invariants` 4 件、`error[E0609]: no field invariants on type PathClass` 2 件）。
- 戻した後に同じコマンドを再び実行し、exit 0、2 passed を確かめた。

### 条件 2: 生成 schema の作り直しの手順
- 手順（`docs/protocol/worker-protocol.md` の冒頭と `committed_schema_matches_generated` の doc に書かれた生成の手順）:
  1. `git show $(git merge-base HEAD main):docs/protocol/worker-protocol.schema.json > docs/protocol/worker-protocol.schema.json`（base の schema に戻す）
  2. `UPDATE_SCHEMA=1 cargo test -p task-worker --lib committed_schema_matches_generated`: exit 0（1 passed。`schema_value()` の出力を file に書く）
  3. `git diff --exit-code HEAD -- docs/protocol/worker-protocol.schema.json`: exit 0（生成の結果は commit 済みの file と一致する）
- `UPDATE_SCHEMA` を付けずに `cargo test -p task-worker --lib committed_schema_matches_generated`: exit 0、1 passed。
- base との差分は `docs/protocol/worker-protocol.schema.json | 7 +++++++`（`PathClass` の `invariants` の定義の追加だけ）。
- `PathClass` を含む生成 schema は、`grep -rln PathClass docs/` で探してこの 1 file だけだった。

### 条件 3: ADR を実装の前に commit
- 上の「commit の順」のとおり、ADR だけの commit が実装の commit の前にある。`git log --format=%s --reverse $(git merge-base HEAD main)..HEAD` の 1 行目が ADR の commit。
- ADR に、C の ADR（2026-11-17-boundary-change-risk-review）の D2・D6 の欄の部分を先に入れること（D2）、規則・reason・prompt の変更は C が行うこと（D3）を書いた。

### 条件 4: 検査
- `cargo nextest run -p task-dispatch -E 'test(/health_review_scenario_real_cli|health_review_policy|health_enforce_g2_review_policy/)'`: exit 0、32 tests run: 32 passed, 1865 skipped。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0、出力なし。
- `bash scripts/dev/test-affected.sh`（`CELERIS_WU_BASE` はこの repair run の開始時点の snapshot の commit。差分はこの進捗ファイルだけ）: 結果は報告に書く。
- `CELERIS_WU_BASE=$(git merge-base HEAD main) bash scripts/dev/test-affected.sh`（task 全体の差分。対象は `--list` で celeris・celeris-campaign・celeris-credentiald・celeris-mcp・celeris-script・celerisctl・e2e・llm-proxy・routing-eval・task-api・task-core・task-dispatch・task-ops・task-worker と web の typecheck+unit、ts-verify）: 結果は報告に書く。
- `sh scripts/dev/progress-index.sh --check`: 結果は報告に書く。

## 未解決事項
無し。
