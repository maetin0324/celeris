---
title: 健全性 v2 B0 capability-map cap-routing-budget — routing/budget・llm-proxy・script runtime の棚卸し
tasks: [01M6V1TGEG5QT5P6PM0Q0D0QMK]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性 v2 B0 capability-map cap-routing-budget

完了日: 2026-11-07

## 結論
- 断片 `capability-map/cap-routing-budget.json` に 9 capability・共有状態 30 件・architecture-map との差分 11 件を書いた。
  capability は routing（lane/route）、accounts/quota、budget/pricing、llm-proxy、routing-eval、routing の監査と結果、
  Project Controller、TS 制御層（script-runtime と control-ts）、cron job。
- 基準は着手時の `git merge-base HEAD main`（件名「repair: main 取り込みの衝突解消の import を整形する」）。
  40 桁の SHA は JSON の `base_sha` に記録した。
- 調べ方は読み取りだけ（grep で `INSERT`/`UPDATE`/`DELETE`・`Event::<Variant> {` の構築箇所・`fs::write`/`rename`）。
  build と試験は実行していない。

## 主な所見
- 書き手が複数の共有状態: `llm_proxy_requests`（llm-proxy::log と task-core::store::routing_log）、
  `project_controller_state`（task-core::store の project_wake・project_wake_effect・project_guard・project_briefs・authority の 5 module）、
  event `routing_features_recorded`（task-dispatch::dispatcher::routing_context と celeris::daemon::services）。
- architecture-map に予算・単価（task-core::pricing・task-ops::project_budget・task-dispatch::dispatcher::token_budget）の行が無い。
- 132 行は routing-eval が routing_evidence 表を書くと書くが、routing-eval の src には表の書き込みが無い。
  表を書くのは task-core::completion_outcome と task-core::store::routing_evidence。
- 51 行（Project Controller）の owner は task-dispatch だけで、状態表の書き手（task-core::store）と判定 TS が path 列に無い。

## 試験コスト
- `build_link_secs` は `test-targets.json` の `build_no_run[<crate>].secs`。段 1 で測った crate は task-core・task-dispatch・
  task-api・task-worker・celeris だけ。llm-proxy・routing-eval・script-runtime・task-ops・celerisctl・control-ts（vitest）は
  null（`not measured in stage 1`）。

## 証拠コマンドと結果（提出時の HEAD で実行）
- 断片を生成する script が全 path（owner・evidence_path・port・test・writer）の実在を確かめた: 欠落 0、9 capability。
- 範囲: 変更は `agent-docs/progress/2026-11-07-health-v2-b0/` の下だけ（`git status --porcelain`）。

## commit
- 「cap-routing-budget: routing/budget・llm-proxy・script runtime の capability 断片を追加」

## 未解決事項
- event `routing_shadow_recorded` の構築箇所は grep で見つからなかった（未確認）。共有状態には入れていない。
- 書き手の検出は SQL 文字列と `Event::` の構築の grep による。動的に組み立てた SQL は拾えていない可能性がある（未確認）。

## 提案
- architecture-map に予算・単価の行を足し、51・132・163 行の owner と書き手を直す（地図の修正は別 task で行う）。
