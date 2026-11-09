---
title: 健全性v2 G3 limits integration-wire — WU の check と段の統合で、health の往復と修復の上限の使い切りを Blocked と root の受信箱で止める
tasks: [01M70C8NHEANARRKZD9RGKZ87D]
status: done
updated: 2026-11-09
completed: 2026-11-09
---
# 健全性v2 G3 limits integration-wire — WU の check と段の統合で、health の往復と修復の上限の使い切りを Blocked と root の受信箱で止める

完了日: 2026-11-09

WorkUnit `integration-wire`（task 01M70C8NHEANARRKZD9RGKZ87D）。[ADR 2026-11-08](../../adr/2026-11-08-health-enforcement-loop.md) の D4.3・D4.4 を WU の check と段の統合に配線した。判定は `repeat` の `health_review::limits::judge`、root への追記は `escalation` の `task_ops::inbox::health_repeat::record_findings_repeated` を使い、どちらも変えていない。基準は `$CELERIS_WU_BASE`。

結論: 完了。WU の check と段の統合で、health だけの不合格が修復の後も同じ fingerprint の集合で残ると、WU の retry・統合の repair・replan を起こさず、WU（統合 WU）と task を Blocked（質問 `health_findings_repeated: <位置>: …`）にし、木の root に `HealthFindingsRepeated` を 1 件追記する（受信箱の `health_repeat` は `(root, 集合)` ごとに 1 件）。段の統合の health の修復が `max_repairs`・`max_repairs_per_class`（bucket `health:<rule_id>`）を使い切ると、replan せず Blocked（質問 `health_repair_limit: …`）で止める。health の Error は今どおり Blocked。health でない失敗の扱いは変えていない。

## 実装前 brief
1. 根本原因: WU の check の health の Fail は毎回 WU の retry に、段の統合の health の Fail は毎回 repair に回り、同じ指摘が残っても止まらない。段の統合の修復の上限を使い切ると `integration_gives_up_with` が `max_replans` の余りで replan する（D4.4 に反する）。
2. 責務 owner: 検査の列の health の行の扱いは `health_review/check_item.rs`（`dispatcher::phase_integration` の子 module。「写しの規則はここだけに置く」）。段の統合の repair の判定は `phase_integration.rs` の `schedule_integration_check_repair`、replan の可否は `integration_gives_up_with`。
3. 既存の再利用先: 往復の判定 `health_review::limits::judge`、root への追記 `record_findings_repeated`、位置の表示 `HealthRepeatSite::label`、Blocked と受信箱の質問は既存の `Terminal::Question`（WU の check）と `integration_gives_up_with` の Blocked の枝（段の統合）、bucket の数え方 `check_item::repair_count`。
4. 選択肢: (a) `integration_gives_up_with` に replan の可否の引数を足す案は、呼び出し 9 か所を変える。(b) 既存の D3.4 と同じく、止める理由の先頭の語で replan を止める案。(b) を採り、語の一覧を `check_item::stops_without_replan` の 1 か所に置いた（`health_check_error`・`health_findings_repeated`・`health_repair_limit`、語の後に `:`）。WU の check は `settle_health_row` を `&self` の method にし、Error と同じ質問へのすり替えに往復を足した（`work_units.rs` は行を増やさない）。
5. 新しい状態・分岐・fallback・port: 無し。config の欄・表・migration・event の種類は増やしていない。質問の先頭の語 2 つ（`health_findings_repeated`・`health_repair_limit`）を足した。
6. 撤去対象: `integration_gives_up_with` の `why.starts_with(HEALTH_CHECK_ERROR)` の判定（`stops_without_replan` に置き換えた）。`settle_health_row` の関連関数の形（method に置き換えた）。
7. 不変条件: 往復は retry・repair・replan を増やさず Blocked と root の受信箱 1 件、使い切りは replan しない、Error は合格にならず repair・replan しない、health でない失敗と health の混ざる不合格・往復でない新しい指摘は今どおり。下の試験で確かめる。

## 変更（commit の件名）
- 「G3 limits integration-wire: WU の check と段の統合で health の往復と修復の上限の使い切りを Blocked と root の受信箱で止める」
  - `crates/task-dispatch/src/health_review/check_item.rs`:
    - `HEALTH_FINDINGS_REPEATED`・`HEALTH_REPAIR_LIMIT` と `stops_without_replan(why)`（Error・往復・使い切りの質問なら `true`）。
    - `health_repeat_stop(task, site)`: task の events を `limits::judge`（範囲は `site` から）で判定し、Repeat なら `record_findings_repeated` で root に追記して質問の文を返す。events の読み出し・追記の失敗は `DispatchError` で呼び出し側に返す（他の store の失敗と同じ扱い）。
    - `phase_health_stop(task, phase, class, used, summary)`: class が Health のときだけ、往復なら往復の質問、使い切り（`used` が `Some`）なら上限の質問（統合の修復の回数・bucket の回数と上限、失敗の要約）を返す。
    - `settle_health_row(&self, task, wu, result, check_run)`: Error の質問に加え、不合格がすべて health の項目（`only_health_fails`）で WU の key の範囲が往復なら、同じく質問にすり替えて検査の記録を空にする（WU の retry にしない）。戻り値を `Result` にした。
    - `repair_count` の doc comment を「段の統合と task の review の修復の bucket」に直した（loop-points の提案 1。`review_verdict.rs` の `try_review_repair` も使う）。
  - `crates/task-dispatch/src/dispatcher/phase_integration.rs`: `schedule_integration_check_repair` が `repair_count` の後で `phase_health_stop` を呼び、`Some` なら `integration_gives_up_with` に質問を渡して止める。`integration_gives_up_with` の replan の可否を `stops_without_replan` で決める。
  - `crates/task-dispatch/src/dispatcher/work_units.rs`: `on_work_unit_checks_finished` の呼び出し 1 行と doc comment 2 行（行数は同じ。source-size の負債の file なので増やさない）。
  - `crates/task-dispatch/src/dispatcher/tests/health_enforce.rs`: stub の検査器の fingerprint の番号を作業ツリーの `FP` の数（無ければ 1）にした（既存の試験の fingerprint は変わらない）。`commit_base_checker`・`health_items`・`unit`・`with_check` を `pub(super)` にした（新しい試験 file が使う）。
  - `crates/task-dispatch/src/dispatcher/tests/health_limits_integration.rs`（新規）と `tests/mod.rs` の `mod health_enforce;` の直後の宣言 1 行。
  - `crates/task-dispatch/src/health_review/check_item_tests.rs`: 単体の試験 2 件。
- `crates/task-dispatch/src/dispatcher.rs` は変えていない。`health_review/limits.rs` も変えていない。

## 試験（守る不変条件と、既存の試験で検出できない理由）
dispatcher の試験は `run_until_idle`（dispatcher が静まるまで tick を回す出来事待ち）で決まり、時刻に依らない。検査器は試験の repo の base に commit した stub。外部ネットワークと LLM を使わない。
- `health_limits_work_unit_same_fingerprint_blocks`: WU の check の health だけの Fail が retry の後も同じ集合なら、retry は 1 回だけで、WU は Blocked（Question）、task は Blocked、質問は `health_findings_repeated: WU a の check: `、root（task 自身）に `HealthFindingsRepeated{site: work_unit a}` が 1 件、replan なし。既存の `health_enforce_work_unit_check_fail_routes_to_work_unit_repair` は最初の Fail が retry になることしか見ない。
- `health_limits_work_unit_different_findings_still_retry`: 修復ごとに fingerprint が変わる（作業ツリーの `FP` を run ごとに 1, 2, … にする）WU は往復でなく、retry が 2 回以上続き、追記も質問も無い。
- `health_limits_work_unit_mixed_failure_keeps_retry`（7 件目）: health の Fail に health でない検査の失敗（`false`）が混ざると、health の集合が同じでも往復で止めず retry を続ける。
- `health_limits_same_fingerprint_escalates_once`: 同じ root の 2 つの子 task の WU が同じ集合で往復すると、両方 Blocked、`HealthFindingsRepeated` は 2 件とも root に載り（子の task 自身には載らない）、受信箱の `health_repeat` は root の 1 件で源に 2 つの task が並ぶ。task-ops の受信箱の試験は event を直接書くので、経路が root を求めて書くことを検出できない。
- `health_limits_integration_same_fingerprint_blocks`: 段の統合の health の Fail が修復の後も同じ集合なら、repair は 1 件だけ、replan なし、`PhaseIntegrated` なし、統合 WU と task は Blocked、質問は `health_findings_repeated: 段 build の統合: `、root に `site: phase build` が 1 件。既存の `health_enforce_phase_integration_fail_schedules_repair` は修復が直す流れしか見ない。
- `health_limits_integration_repair_cap_blocks`: 修復ごとに fingerprint が変わり（1 → 2 → 3、往復でない）、`max_repairs_per_class`（既定 2）を使い切ると、repair は 2 件、replan なし、統合 WU と task は Blocked、質問は `health_repair_limit: 段 build の統合: ` で `health:deps.violation の修復 2/2 回` を含む。変更の前は `integration_gives_up_with` が replan していた。
- `health_limits_integration_error_blocks_without_repair`: 段の統合の health の Error（検査器の exit 2）は repair・replan・`PhaseIntegrated`・追記が無く、Blocked と `health_check_error: ` の質問。replan を止める判定を `stops_without_replan` に移したので、Error が replan に戻らないことを固定する（Unavailable は既存の `health_enforce_phase_integration_unavailable_blocks_task`）。
- `health_limits_stops_without_replan_only_health_stops`（単体）: 3 種の質問だけが replan を止め、似た語（`health_check_errors`・`health_repair_limited`）や統合の検査の失敗の文は止めない。
- `health_limits_only_health_fails_ignores_mixed_rows`（単体）: 往復の判定に入るのは不合格がすべて health の項目の列だけ。
- 変更を外す確かめ: `health_repeat_stop` を常に `None`、`phase_health_stop` の使い切りを常に `None` にした一時の変更で、上の 4 件（work_unit_same_fingerprint・integration_same_fingerprint・escalates_once・integration_repair_cap）が落ちることを確かめ、元に戻した。

## 契約の変更（既存の試験への影響）
- WU の check の health の Fail が同じ集合で 2 回続くと、WU の retry を使い切る前に Blocked になる。`health_enforce_work_unit_check_fail_routes_to_work_unit_repair` は最初の Fail が retry になり合格しないことだけを確かめているので、直さずに合格した。
- 段の統合の health の修復の上限の使い切りは、replan から Blocked（質問）に変わった。これを確かめていた既存の試験は無い。
- task-dispatch の `health_enforce_` の試験はすべて変更なしで合格した（stub の `FP` は既存の試験では書かれないので、fingerprint は前と同じ `deps.violation:0000000000000001`）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-dispatch --no-tests=fail -E 'test(/health_limits_/) | test(/health_enforce_/)'` → exit 0: 60 tests run: 60 passed。WU の check の名前の確かめ（plan の check の `grep -rqE "fn <名前>\b"` の loop）で 6 件とも見つかる。
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 3669 tests run: 3669 passed（11 skipped）。repo-health verdict=pass（fail_grade 0、new 2、worsened 0、`source-size` pass）。new の 2 件は optional の warning（`dup.contract`: `required_checks::health::UnavailableReason` と `route_select::availability::UnavailableReason`、`dup.public-surface`: `campaign/src/jev_smoke.rs` と `health_review/limits.rs`）で、この unit が変えた file ではない。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
- 範囲 check（plan の `scope: true` の cmd を `sh "$CELERIS_WU_SCOPE_PATHS"` で）→ 範囲外の path なし。

## 本番への移行
- config・表・migration・API の変更は無い。release に入ると、WU の check と段の統合の health の往復で task が Blocked になり、受信箱に源の task の質問と root の `health_repeat` が出る。人は源の task への comment・回答で再開する（`limits` は人の再開の後の最初の評価を往復と数えない）。

## 未解決
- WU の check の health だけの不合格で、指摘が毎回変わり WU の retry（`execution_scheduler::failed` の `max_retries`）を使い切った場合は、今どおり WU が Failed になる。その後に自動 replan が起きるか（D4.4「health のために replan を足さない」に当たるか）は、この unit の範囲（D4.4 の (c) は段の統合）の外で、未確認。
- 段の統合で health の Fail と health でない失敗が混ざる不合格は、今どおり `classify_review_failure` が Substantive にして replan する（health でない失敗の扱いを変えない）。

## 提案
- 兄弟の `review-wire` は往復の判定と root への追記に `check_item` の `health_repeat_stop`（`HealthRepeatSite::Review` も受ける）を使えるよう、統合の段（`integrate-wire`）で可視性を `pub(in crate::dispatcher)` に広げて 1 か所にまとめるとよい（同じ判定と追記を 2 か所に置かない）。
- 上の未解決 1 の WU の retry の使い切りは、`budgets` の unit で、health だけの不合格なら replan でなく Blocked にするかを決めるとよい。
