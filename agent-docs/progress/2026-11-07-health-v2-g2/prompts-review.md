---
title: 健全性v2 G2 review-health — reviewer prompt に health の材料と health verdict の書き方を出す
tasks: [01M6VTFGWFVE57YXDKHR2E436G]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G2 review-health — reviewer prompt に health の材料と health verdict の書き方を出す

完了日: 2026-11-07

WorkUnit `review-health`。[ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md) の D4.1・D5.1・D5.4・D6.3 のうち reviewer prompt の部分を実装した。前の leaf `rules-brief` の `change_rules` を前提にする。基準は `$CELERIS_WU_BASE`。

## 変更
- commit は件名「G2 review-health: reviewer prompt に health の材料と health verdict の書き方を出す」と、この進捗ファイル・task のまとめ（[prompts.md](prompts.md)）の commit。
- `crates/task-worker/src/change_rules/review.rs`（新規。module `change_rules::review`）:
  - `health_section(review)`: `ReviewRequest.health` が `Some` のときだけ見出し `HEALTH_HEADING` の節を出す。repo ごとに `target_sha`・`reviewed_sha`、diff 統計（files・insertions・deletions と path ごとの行数）、owner 地図の行、repo health の report の要約（verdict・reviewable・counts と new・worsened の finding の rule_id・severity・delta・path）、policy（source・changed・files）、不変条件を出す。`paths` を上限で切った数（`diff.files` と `paths` の件数の差）と `findings.truncated` を「上限で省略」の行に出す。reviewable=false の report は材料にしないと書く。policy.changed の repo には `health.policy` を必ず書くと書く。`None` なら空文字列。
  - `result_section(review, artifacts)`: reviewer prompt の Result 節を組む唯一の関数。従来の文面（`verdicts` の形）を先頭にそのまま出し、health があるときだけ同じ節に `health`（`findings[{kind,severity,path,reason,repair}]`・`policy`）の形を足す。`kind` は `HealthFindingKind::ALL` を `as_str` で列挙し、意味は汎用規約で末尾の括弧に同じ名前を書いた項目を指す（説明文は写さない）。`repair.scope = "local"` を書ける種別は `allows_local_repair` で選ぶ（2 種）。policy の file を変えた repo があれば `health.policy` が必須、無ければ省くと書く。`health` が無い・未知の kind・severity の review.json は全 criterion が不合格になると書く。
  - 汎用規約の見出しの名前は `GENERIC_RULES` の 1 行目から取る（見出しの文字列を写さない）。
- `crates/task-worker/src/change_rules.rs`: `pub mod review;` と module の説明 1 行。
- `crates/task-worker/src/claude_code/prompt.rs`: `build_review_prompt` の Result 節の `format!`（8 行）を `change_rules::review::result_section` の呼び出しに置き換え、worker の evidence の後に `health_section` の呼び出しを 1 行足した。関数は 3 行短くなった。protocol の型は変えていない。

## 試験（prefix `change_rules_review_`、`crates/task-worker/src/change_rules/review/tests.rs`、5 件）
- `change_rules_review_without_health_keeps_the_legacy_prompt`: health が `None` の review と review context の無い run で、health の節・`"health"`・`health.policy` が prompt に無く、Result 節が従来の文字列（試験の `LEGACY_RESULT`）のまま result.json の指示に続く。7 種の名前は汎用規約の 1 回だけ出る。
- `change_rules_review_with_health_shows_every_field`: 2 repo の材料で、各欄（SHA・diff 統計・path・owner 行・report の verdict と counts・path のある finding と無い finding・policy・不変条件）が 1 節に出る。節は evidence の後・Result 節の前に 1 回だけ出る。`health.policy` を必ず書く行は policy を変えた repo の 1 回だけ。
- `change_rules_review_result_section_asks_for_the_health_verdict`: Result 節の追加文に health の形、`ALL` の 7 種、`allows_local_repair` の種別だけを挙げた局所修復の行、汎用規約の見出しへの参照、`health.policy` の必須（変えた repo の名前つき）、不合格の条件が出る。policy を変えた repo が無ければ「`policy` は省く」になる。prompt 全体で種別の名前は汎用規約 1 回 + 種別の一覧 1 回（局所修復の 2 種はもう 1 回）。
- `change_rules_review_kind_descriptions_are_not_copied`: 汎用規約の 7 種の項目の説明文は prompt に 1 回だけ出て、`review.rs` の source に無い。`review.rs` に種別の名前の文字列も無い。`HealthFindingKind` の doc comment の説明も `review.rs` と prompt に無い。汎用規約は 7 種のどれにも括弧内の名前の項目を持つ。
- `change_rules_review_truncated_material_is_shown`: `diff.files` 45・`paths` 2 件で「ほか 43 file は上限で省略」、`findings.truncated` 7 で「上限で省略した finding: 7 件」、reviewable=false の行が出る。切り詰めが無ければ省略の行は出ず、owner 行・不変条件が空なら「無し」と出る。

## 受け入れ条件ごとの証拠（提出時の HEAD で実行）
- 0. health が Some の review prompt に diff 統計・owner 行・new findings・policy・invariants の節と health verdict の書き方が出る → `cargo nextest run -p task-worker -E 'test(change_rules_)'` → 16 passed（このうち `change_rules_review_with_health_shows_every_field`・`change_rules_review_result_section_asks_for_the_health_verdict`・`change_rules_review_truncated_material_is_shown`）。
- 1. health が None の review prompt に health 節も Result 節の追加文も出ない → 同じ実行の `change_rules_review_without_health_keeps_the_legacy_prompt` が passed。
- 2. 7 種は `HealthFindingKind` から出し、source に説明文の写しが無い → 同じ実行の `change_rules_review_kind_descriptions_are_not_copied` が passed。
- 3. 検査 → 下の「検査の結果」。
- 4. task のまとめ → [prompts.md](prompts.md) に受け入れ条件ごとの条件・コマンド・出力の要点を書いた。

## 検査の結果（提出時の HEAD で実行）
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/check-health.sh` → exit 0。`repo-health: verdict=pass exit=0 fail_grade=0 new=0 worsened=0`、repo_health_gate は全件 OK。
- `bash scripts/dev/test-affected.sh` → 未実行（この後に実行して追記する）。
- 文書検査 3 本（`bash scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`・`sh scripts/dev/progress-index.sh --check`）→ 未実行（この後に実行して追記する）。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は crates/task-worker の 4 file（`change_rules.rs`・`change_rules/review.rs`・`change_rules/review/tests.rs`・`claude_code/prompt.rs`）と、この進捗ファイル・prompts.md。

## 判断
- 新しい描画は `change_rules::review` に置いた。種別は汎用規約の項目の名前で、Result 節はその項目を参照するので、規約の配布と同じ module が持つ。`prompt.rs`（1,884 行、閾値 2,000）の `build_review_prompt` は伸ばさなかった。
- Result 節は health の有無で別の関数に分けず、`result_section` 1 つにした。health が無いときの文面は従来のまま（D7.2 の置き換えは U6 の範囲）。
- health の節は worker の evidence の後・入力 artifact の前に置いた。reviewer が worker の申告を読んだ後に、決定的な材料を読む順にした。
- `severity` の値 `blocking`・`advisory` は文字列で書いた。`HealthSeverity` に `as_str` が無く、protocol の型はこの leaf で変えない。

## 未解決事項
- D5.4 の `design`（`ReviewOutput.design`）と `verdicts[].repair` の形は Result 節に出していない。`ReviewOutput.design`・`RunContext.design_review`・`HealthReviewInput.brief` は protocol に未作成（dispatch 側の unit の範囲）。型ができたら `result_section` に足す。
- D7.1（Commit references の節）・D7.2（Result 節の旧い形の置き換え。試験の `LEGACY_RESULT` もその時に消す）・D7.4（`PLANNER_CHECK_GUIDANCE` の CLAUDE.md と同じ文）はこの leaf では変えていない（U6 の範囲）。
- prompt は D5.2（health の無い review.json は全 criterion 不合格）と D5.3（blocking → 不合格、局所修復の 2 種）を事実として書く。dispatcher 側の読み込みと写像（`review.rs`・`review_verdict.rs`）はこの branch に無い（dispatch task 01M6VTFG9K5HEE817EW05JASMZ の範囲）。dispatcher が `ReviewRequest.health` を埋める `build_health_input` もこの branch に無いので、それまで本番の prompt に health の節は出ない。
- D4.1 は `paths`・`owner_rows` の上限超えの数も `truncated` に書くとするが、型の `truncated` は findings の 1 つだけ。`paths` は `diff.files` との差で省略数を出せる。`owner_rows` の省略数は出せない。
- D6.4（`approved = true` でも人の承認を待つ）は未実装なので、prompt には書いていない。

## 提案
- protocol の担当で `HealthRepoInput` に `owner_rows` の総数（または省略数）を足し、`health_section` で出す。
- U6 で D7.2 を実装するとき、`result_section` の従来の文面を `verdicts[].repair`・`health`・`design` の 1 節に置き換え、`change_rules_review_without_health_keeps_the_legacy_prompt` の `LEGACY_RESULT` の固定を新しい形の試験に置き換える。
