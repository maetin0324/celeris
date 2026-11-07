---
title: 健全性v2 G2 rules-brief — 汎用規約・repo 固有の規約・設計 brief の節を prompt に配る
tasks: [01M6VTFGWFVE57YXDKHR2E436G]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 G2 rules-brief — 汎用規約・repo 固有の規約・設計 brief の節を prompt に配る

完了日: 2026-11-07

WorkUnit `rules-brief`。[ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md) の D1.1〜D1.5 と D3.4 を実装した。基準は `$CELERIS_WU_BASE`。

## 変更
- commit は件名「G2 rules-brief: 汎用規約・repo 固有の規約・設計 brief の節を prompt に配る」と、この進捗ファイルの commit。
- `crates/task-worker/src/change_rules.rs`（新規）:
  - `GENERIC_RULES = include_str!("change_rules/generic.md")`。正本は generic.md の 1 file で、本文は変えていない。
  - `section()` は generic.md をそのまま出す。見出しは generic.md の 1 行目で、別の見出しは足さない。
  - `repo_section(context)` は `ExecutionPlannerContext.review_policy` から owner 地図・高 risk の class と path・不変条件を見出し `REPO_RULES_HEADING` の節に出す。方針が無い・中身が空なら空文字列。
  - `design_brief_section(planner)` は /3 の planner に `design_brief` の欄を generic.md の「実装前 brief」の 7 項目に対応させて示す。方針があれば、高 risk の class に触れる計画では brief が必須（無ければ `DesignBriefMissing`）と、High・Unknown なら採用の前に設計 review が起きることを書く。class の一覧は repo 固有の節を見出しで指し、再掲しない。方針が無ければ、brief は任意であることと、方針を読めなければ Unknown になり設計 review が起きることを書く。
- `crates/task-worker/src/claude_code/prompt.rs`: `prompt_header` の `crate::preamble::render` の直後に `change_rules::section()` と `change_rules::repo_section(context)` を 1 回ずつ足した（3 行）。`build_execution_plan_prompt` の /3 の分岐で `tree_plan_shape_section` の後に `design_brief_section` を足した（2 行）。adapter（codex・pi・acp・aider・claude_code）と CoS chat（`build_cos_chat_parts`）には足していない。`RunContext` と protocol の型は変えていない。
- `crates/task-worker/src/test_support.rs`: `assert_generic_rules_once`（本文と見出しが 1 回ずつ）と `assert_generic_rules_beside_required_rules`（必須規則の写しと汎用規約が 1 回ずつ、汎用規約は CLAUDE.md の写しの外で後ろ）。
- 既存試験 `available_genres_section_omits_lines_whose_list_is_empty` の needle を「できること」「渡すもの」から行の形「`  できること: `」「`  渡すもの: `」に絞った。理由: generic.md の規約 10 の「…検出できることを示す」が prompt に入るため、prompt 全体の部分一致では genre の行の有無を判定できない。守る不変条件（一覧が空なら genre の行を出さない）は変えていない。

## 試験（prefix `change_rules_generic_`・`change_rules_planner_`）
- `crates/task-worker/src/change_rules/tests.rs`（7 件）:
  - `change_rules_generic_once_in_every_run_kind`: execute（task・WU）・plan・案件計画・案件 replan・execution plan・その replan・/3 の plan・/3 の replan・review で、汎用規約が 1 回、Objective の前にある。
  - `change_rules_generic_absent_from_cos_chat`: CoS chat の prompt と Core・可変部のどちらにも汎用規約と repo 固有の節が無い。
  - `change_rules_generic_has_no_repo_specific_names`: generic.md に crates/ の crate 名と Celeris 固有の語が無い。
  - `change_rules_generic_and_required_rules_do_not_copy_each_other`: generic.md と CLAUDE.md の 12 文字以上の行が互いに含まれない。
  - `change_rules_generic_repo_section_follows_only_with_a_review_policy`: 方針があれば汎用規約の直後に repo 固有の節が 1 回出る。方針が無い・空なら出ない。
  - `change_rules_planner_brief_section_with_a_review_policy`: brief の節の 7 項目が generic.md の「実装前 brief」の項目数（7）と番号で対応し、必須・High・Unknown・設計 review を書き、class 名は prompt に 1 回だけ出る。
  - `change_rules_planner_brief_section_without_a_review_policy`: 方針が無いと節の文面が変わり、`DesignBriefMissing` と repo 固有の節への参照が無い。/1 の planner には brief の節が無い。
- adapter の試験 file（各 adapter の stub を再利用）: `change_rules_generic_reaches_codex_once_beside_required_rules`（codex/tests.rs）、`change_rules_generic_reaches_pi_once_beside_required_rules`（pi/tests.rs）、`change_rules_generic_reaches_acp_once_beside_required_rules`（acp/tests.rs）、`change_rules_generic_reaches_claude_code_once_without_a_rules_copy`（claude_code/tests.rs。claude は CLAUDE.md を直接読むので、prompt に必須規則の写しが無いことも確かめる）。
- 試験は `GENERIC_RULES` と `repo_claude_md()` を読み、規約の本文を写していない。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-worker --no-fail-fast` → 1253 passed、9 skipped。
- `cargo nextest run -p task-worker --lib change_rules_` → 7 passed（adapter の 4 件は上の全体の実行に含まれる）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0（下の「test-affected の結果」）。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の変更 path は crates/task-worker の 9 file とこの進捗ファイルだけ。

## test-affected の結果
- 対象（`--list`）: celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker。
- `bash scripts/dev/test-affected.sh` → exit 0、約 12 分。nextest 5626 passed（19 slow）、22 skipped。doc test は全 crate で失敗なし。
- 先頭の check-health: `repo-health: verdict=pass exit=0 fail_grade=0 new=2 worsened=0`。new の 2 件は `dup.contract`（warning）の `OwnerRow`・`DiffStat` で、どちらも先行の protocol の unit が足した `crates/task-worker/src/protocol/health.rs` の型と既存の型の同名。この unit の変更からは出ていない。
- 実行したのはコードの commit の後。その後の commit はこの進捗ファイルだけを変える。
- 進捗の commit の後に再実行 → exit 100。nextest 5626 件中 5625 passed、1 failed。落ちたのは `task-dispatch dispatcher::tests::work_units::an_interrupted_integration_is_redone_idempotently_after_restart`（work_units.rs:2994、`Blocked` と `Done` の不一致）。単独の再実行（`cargo nextest run -p task-dispatch -E 'test(an_interrupted_integration_is_redone_idempotently_after_restart)'`）では passed で、1 回目の全体の実行でも passed だった。この unit は task-dispatch を変えていない。

## 2 回目の run（check の不合格の確認）
- 1 回目の run の後に celeris が走らせた check（`bash scripts/dev/test-affected.sh && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check`）が exit 100 で落ちた。先頭の check-health は `verdict=pass exit=0 new=0 worsened=0`、repo_health_gate も全件 OK。
- 落ちたのは nextest の 1 件 `task-worker browser::launcher_run::tests::launcher_login::launcher_credential_login_forged_sink_frame_never_reaches_chrome`（browser_launcher_run_tests.rs:3338、「the sink closed without a CDP reply」、`left: None`）。
- 単独の再実行（`cargo nextest run -p task-worker -E 'test(launcher_credential_login_forged_sink_frame_never_reaches_chrome)'`）は 1 passed。実行時の load average は約 55。
- 原因の推定（未確認）: `run_login`（browser_launcher/backend.rs）は偽の broker を呼ぶ `inject` の前に、時間上限のある CDP の手順（login の navigation 15 秒、login の deadline）を踏む。高負荷でそのどれかが失敗すると、`broker.start` が呼ばれないまま `run_login` が Err を返す。試験の `result.is_err()` は通り、broker の thread が書く `reply` は `None` のまま残る。どの手順で落ちたかは確かめていない。
- この unit の変更は prompt の組み立てだけで、browser の経路を変えていない。task-worker を変えたので test-affected が task-worker の全試験を走らせ、この試験が対象に入った。成果は直さず、同じ check を再実行した。
- 再実行 1 回目（件名「G2 rules-brief: 進捗に check の不合格の確認を書く」の HEAD、load average 約 55）: `bash scripts/dev/test-affected.sh` → exit 100、約 9 分 38 秒。check-health は `verdict=pass new=0 worsened=0`。nextest 5626 件中 5625 passed、1 failed。落ちたのは別の試験 `celeris-campaign::launcher a_swapped_daemon_binary_is_rejected_on_every_path_and_stop_only_touches_the_launcher_pid`（crates/campaign/tests/launcher.rs:598。measure-launcher.sh の 198 行が `/proc/<pid>/cmdline` を開けず、「daemon exited before becoming healthy」、daemon.log は空）。単独の再実行（`cargo nextest run -p celeris-campaign --test launcher -E 'test(a_swapped_daemon_binary_is_rejected_on_every_path_and_stop_only_touches_the_launcher_pid)'`、load average 約 19）は 1 passed。この unit は celeris-campaign と scripts/campaign/ を変えていない。
- 再実行 2 回目（件名「G2 rules-brief: 進捗に再実行 1 回目の不合格を書く」の HEAD、load average 約 22）: `bash scripts/dev/test-affected.sh` → exit 0、約 9 分 10 秒。nextest 5626 passed（22 slow）、22 skipped。doc test は失敗なし。check-health は `verdict=pass exit=0 fail_grade=0 new=0 worsened=0`、repo_health_gate は全件 OK。続けて `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0、`cargo nextest run -p task-worker -E 'test(change_rules_)'` → 11 passed。この後の commit はこの進捗ファイルだけを変える。
- 全体の実行で落ちた試験は 3 回とも別の試験で、どれも単独では通った（task-dispatch の work_units、task-worker の browser、celeris-campaign の launcher）。`.config/nextest.toml` は `retries = 0`。

## 判断
- ADR D1.1 は `section()` の見出しを `## 変更の規約（汎用）` と書くが、task の Objective（generic.md の 1 行目を見出しにし、二重にしない）に従い、generic.md をそのまま出した。prompt の中の本文が generic.md と 1 バイトも違わないので、試験は `GENERIC_RULES` の部分一致で 1 回を確かめられる。
- repo 固有の節の見出しは generic.md の H1 と同じ階層の `# 変更の規約（この repo 固有）` にした。
- adapter ごとの配布の試験は、各 adapter の既存の試験 file に置いた。adapter の stub と request の helper を再利用でき、ほぼ同じ内容の小さい試験 file を 4 つ作らない（repo health の `dup.test-fixture` を避ける）。

## 未解決事項
- 上の task-dispatch の試験 1 件は全体の実行でだけ落ちることがある（原因は未確認）。
- task-worker の browser 試験 `launcher_credential_login_forged_sink_frame_never_reaches_chrome` は高負荷の全体実行でだけ落ちることがある（原因は上の推定で、未確認）。
- celeris-campaign の `a_swapped_daemon_binary_is_rejected_on_every_path_and_stop_only_touches_the_launcher_pid` も全体の実行でだけ落ちることがある（daemon が healthy になる前に終わる。原因は未確認）。
- review 方針（`review_policy`）は今は planner の context にだけある。worker・reviewer の run には repo 固有の節が出ない。dispatcher が base 側の方針を読んで `review_policy` を埋める配線（`planner_flow.rs` は今 `None`）は dispatch の unit の範囲。
- Aider には CLAUDE.md が届かない（ADR D1.6）。汎用規約は `build_prompt` を通るので Aider にも届く。

## 提案
- browser の担当で `launcher_credential_login_forged_sink_frame_never_reaches_chrome` を決定的にする。案: `run_login` の Err の code が偽 broker の経路のもの（`Unauthorized`）であることを確かめ、偽 broker の `start` が呼ばれたことを別の印で待つ。task-worker を変える WU は、この試験の揺れで必須検査が落ちうる。
