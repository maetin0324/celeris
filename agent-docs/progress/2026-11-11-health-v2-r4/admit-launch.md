---
title: "健全性v2 R4 admit-launch: lane の保留・route・credential・variant の pin の起動可否を dispatch_admission::admit へ移す"
tasks: [01M7BYQNFCE4Z8PFD95T6VQ37J]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 admit-launch

完了日: 2026-11-14

## 概要

- `dispatch_admission::admit` を、admit-place が移した scope env の後ろへ広げた。langmem の確認と lane の決定・分類の待ち・
  lane の引き上げの保留・route の拒否・credential の待ち・variant の pin の順序と組み合わせと止め方は admit の `route_gates` にある
  （ADR 2026-11-14 D5 の 25〜30）。
- `dispatch_one` は admit が求めた fact を既存の関数で集め（`knowledge_fallback_reason`・`decide_lane_for_work_unit`・`decide_lane`・
  `defer_for_classification`・`apply_lane_assist`・`select_route_with`・`compare_route_priority`・`route_adapter`・
  `route_launch_target`・`credential_gate_hold`・`variant_run_pin`）、止め方（`note_route_refusal`・`block_variant_task`）を適用し、
  起動を返したときだけ lease（`acquire_lease`・`acquire_work_unit_lease`）・`WorkerStarted` 等の event・`spawn_worker` を今のとおり行う。
  起動を止める独自の判断は持たない。credential_gate.rs・variant_pin.rs・lane_assist.rs ほか dispatcher の他の file は変えていない。
- `dispatch_run.rs` に `if self.defer_for_classification(`・`if held`・`if self.credential_gate_hold(` の字面は無い（コメントにも無い）。
- planner run の `LaneDecision` の組み立て（admit が決めた planner run の tier を写すだけの値）を `dispatch_admission::planner_lane` に
  移した（ADR 2026-11-14 D2 に理由を書いた）。
- `dispatch_one` の行数: 移行前 1018 行（34〜1051 行）→ 移行後 1017 行（35〜1051 行）。`dispatch_run.rs` は 1652 行 → 1652 行。
  `dispatcher` module の本体の行（repo health の size.responsibility-growth が数える行）は `dispatch_run.rs` で 1428 → 1428、
  module の合計 31662 → 31662。
- `dispatch_admission/tests.rs` の試験は 13 本 → 17 本。ADR 2026-11-11 と `cex_` の試験と dispatcher の既存の試験は変えていない。

## 実装前 brief

1. 根本原因: `dispatch_one` の lane から variant の pin までの部分は、起動を止める判断（分類を待つか・引き上げで保留するか・route を
   受けたか・credential を待つか・pin を確かめたか）の順序を、lane の決定・route の選択・記録の副作用と交互に `if … { return Ok(false) }`
   で書いていた。順序の入れ替えを検出する試験が無い（既存の試験は 1 つの gate だけを止める）。
2. 責務 owner: 順序と組み合わせは `dispatch_admission`（ADR 2026-11-11 D3・ADR 2026-11-14 D1）。個々の判定と副作用は既存の owner
   （lane_assist.rs・route_select.rs・route_launch.rs・credential_gate.rs・variant_pin.rs）と、それを呼ぶ `dispatch_one`。
3. 既存の再利用先: 上の概要に挙げた関数をそのまま呼ぶ。試験は admit-gate の `drive`（admit が求めた fact だけを写す）を広げて使う。
4. 選択肢: (a) admit-gate・admit-place と同じく fact を 1 つずつ求める（採用。今は呼ばれない credential の待ち・variant の blocked を
   起こさない）。(b) route の拒否の記録を route の fact の中で行う（dispatch_one が結果で分岐することになるので不採用。`RouteRefused`
   の止め方で当てる）。(c) planner run の `LaneDecision` を dispatch_run.rs の関数のまま残す（dispatcher module の本体の行が
   WU base の 31662 を超えて worsened になるので不採用。値を写すだけの関数として dispatch_admission に移す。admit-gate の
   `AdmissionTaskValues::read` と同じ扱い）。
5. 新しい状態・分岐・fallback・port: 状態と fallback は無し。新しい型は入出力の値（`AdmissionRoute`、`AdmissionFact` の `Lane`・
   `ClassificationDefer`・`LaneAssist`・`Route`・`CredentialGate`・`VariantPin`、`AdmissionStop` の `RouteRefused`・`VariantPinFailed`）
   だけ。dispatch_one の loop には「admit が先に求めた fact が揃っていなければ起こさない」`return Ok(false)` が 4 か所ある
   （lane の引き上げで lane の決定が無い、credential の待ちと variant の pin で launch の形が無い、起動の前に route か pin が無い）。
   admit の順（試験で固定）では起きない。
6. 撤去対象: dispatch_one の中の `!is_planner_dispatch && let Some(decision)` の分岐・分類の待ちと `held` の `return`・launch の
   `Err` の分岐と `return`・credential の待ちの `return`・variant の pin の `Err` の分岐と `return`、`planner_human_frontier`・`cos`・
   `replan_dispatch` の別名の変数、dispatch_one の中の planner run の `LaneDecision` の組み立て。
7. 不変条件: 権限と予算の判定を減らさず順序を変えない。credential の待ちと variant の pin は lease より先。並列 WU の 2 本目以降も
   route の gate をすべて見る。確かめる試験は `dispatch_admission/tests.rs` の 17 本、`cex_` の試験、既存の dispatcher の試験
   （test-affected）。

## admit の止め方と旧い行の対応表（旧い行は WU base の `dispatch_run.rs`、新しい行は提出時の HEAD）

| 判断 | 旧い行（WU base） | admit（`dispatch_admission.rs`） | dispatch_one の適用（新しい行） |
|---|---|---|---|
| langmem の確認と汎用の harness へ倒す上書き | 379〜396 | `route_gates` の `Lane` の要求（25） | `AdmissionFact::Lane` の arm（374〜） |
| lane の決定（planner run・WU・task） | 397〜444 | 同上 | 同上。planner run は `planner_lane` |
| 分類の待ち（`defer_for_classification`） | 448〜450 | `ClassificationDefer`、待つなら `Stop(Quiet)`（26） | `AdmissionFact::ClassificationDefer` の arm（403〜） |
| lane の引き上げの保留（`apply_lane_assist` の held） | 451〜455 | `LaneAssist`、held なら `Stop(Quiet)`（27） | `AdmissionFact::LaneAssist` の arm（408〜） |
| 26・27 を planner run と lane の無い run で飛ばす | 447 の `!is_planner_dispatch && let Some(decision)` | `decided && !launch.planner_run` | 無し |
| lane を task に当て、route を選び launch の形を作る | 457〜562 | `Route`（28） | `AdmissionFact::Route` の arm（418〜）。受けたら `clear_warned_unroutable` |
| route の拒否（`note_route_refusal`） | 563〜572 | `Stop(RouteRefused)`（28） | `AdmissionStop::RouteRefused`（124〜） |
| credential の待ち（`credential_gate_hold`） | 574〜577 | `CredentialGate`、待つなら `Stop(Quiet)`（29） | `AdmissionFact::CredentialGate` の arm（539〜） |
| variant の pin（`variant_run_pin`・`block_variant_task`） | 585〜592 | `VariantPin`、Err なら `Stop(VariantPinFailed)`（30） | `AdmissionFact::VariantPin` の arm（547〜）と `AdmissionStop::VariantPinFailed`（129〜） |
| 起動（lease・`WorkerStarted`・spawn） | 598〜 | `Launch`（30 を通った後だけ） | 580〜（今のとおり） |

## 順序の等価性で確かめたこと

- 移行前と同じ順で fact を求め、先の gate が止めたら後ろの関数を呼ばない: langmem の確認 → lane の決定 → 分類の待ち → lane の
  引き上げ → lane を task に当てる → route の選択（features の記録・compare での選び直し・lane assist と routing の解決の記録を含む）→
  launch の形 → 受けたら `clear_warned_unroutable` → credential の待ち → variant の pin → lease。
- 26・27 は移行前と同じく planner run でなく lane を決めた run だけ。並列 WU の 2 本目以降も移行前と同じく全部見る（`second_pass` の
  条件は移行前もこの部分に無かった）。
- 移行前と比べて、adapter の id・source・model・account を読む位置が variant の pin の後になった。どれも pick と登録表を読むだけで
  副作用が無い（`source_account_adapter` は登録表を引くだけ）ので結果は同じ。
- `planner_lane` の規則 id と理由の文は移す前と同じ。tier の語は dispatcher の `tier_label` と同じ写像の
  `task_core::model_catalog::assignments::tier_str` で作る（どちらも frontier・standard・cheap を返す）。人が frontier を明示した
  planner run の記録は既存の `planning_and_gate.rs` の試験（`planner/human-frontier`）が確かめる。system の側の規則 id
  （`planner/system-<tier>`）を直接確かめる試験は無い（未確認の範囲。値は移す前と同じ式）。
- 既存の順の試験の直し方: 求める順を固定する 3 本（`admit_first_pass_asks_facts_in_the_dispatch_order`・
  `admit_second_parallel_unit_skips_only_the_first_pass_gates`・`admit_place_facts_follow_the_dispatch_order`）は、期待する列の
  後ろに route の fact（`route_facts`）を足した。`admit_place_facts_follow_the_dispatch_order` の「最後が scope env」は「route の
  fact の前の最後が scope env」（`before_route`）に直した。どれも弱めていない。

## 試験が順の入れ替えを検出すること（一時的に `route_gates` を書き換えて確かめ、元に戻した）

- credential の待ちと variant の pin の順を入れ替える: 6 本が落ちた（`admit_credential_wait_precedes_the_variant_pin_and_the_launch`・
  `admit_second_parallel_unit_keeps_the_route_gates` ほか）。
- route の拒否と credential の待ちの順を入れ替える: 6 本が落ちた（`admit_route_refusal_stops_before_the_credential_wait_and_the_pin`
  ほか）。
- 分類の待ちと lane の引き上げを飛ばす: 6 本が落ちた（`admit_lane_holds_come_after_the_place_and_before_the_route` ほか）。

## 変更（件名）

- `phase admit: lane の保留・route・credential・variant の pin の起動可否を dispatch_admission::admit へ移す`
- `phase admit: admit-launch の進捗を書く`（文書だけ）

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

- WU の check 4（純粋さと同名の関数の定義なしの grep、`admit`/`AdmissionInput`/`AdmissionProposal` が根の file にあること、
  `dispatch_admission::admit(` の呼び出し、lease・`spawn_worker(`・`WorkerStarted` が `dispatch_run.rs` に残ること、
  `if self.defer_for_classification(`・`if held`・`if self.credential_gate_hold(` などが無いこと、`#[test]` が 12 本以上、
  `cargo nextest run -p task-dispatch -E 'test(cex_) | test(/^dispatch_admission::/) | test(structure_guard_)'`）:
  exit 0、53 passed、1809 skipped（`#[test]` は 17 本）。
- `bash scripts/dev/test-affected.sh`: exit 0。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch。
  3917 passed、11 skipped。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- repo health を WU base と比べる（celeris の `celeris:repo-health/1` と同じ基準）:
  `python3 scripts/dev/repo-health/repo_health.py --repo . --base "$CELERIS_WU_BASE" --head HEAD --policy-version 1`: exit 0、
  verdict=pass、new 0、worsened 0。`dispatch_one` の関数の行 1018 → 1017、`dispatcher` module の本体の行 31662 → 31662（どちらも existing）。
- WU の check 2（`cargo clippy … && cargo fmt … && bash scripts/dev/check-health.sh && test -f <進捗> && sh scripts/dev/progress-index.sh --check`）:
  exit 0。check-health は verdict=pass、fail_grade 0、worsened 0。new 1 件は warning の `migration.remnant`
  （`host_maintenance.rs` の `legacy` の語。この unit は触っていない）。progress-index --check: ok。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を計画の許す path と比べる）: 範囲外 0。変えた path は ADR 2026-11-14・
  `dispatch_admission.rs`・`dispatch_admission/tests.rs`・`dispatch_run.rs`・`docs/architecture-map.md`・この進捗。
- `git diff --name-only "$CELERIS_WU_BASE" -- agent-docs/adr/2026-11-11-dispatcher-state-ownership.md crates/task-dispatch/src/dispatcher/tests crates/task-dispatch/tests`:
  0 件（ADR 2026-11-11 と `cex_` の試験と既存の dispatcher の試験は変えていない）。

## 未解決事項

- planner run の system の側の lane の記録（`planner/system-<tier>`）を直接確かめる試験は無い（移す前も無かった）。

## 提案

- 無し。
