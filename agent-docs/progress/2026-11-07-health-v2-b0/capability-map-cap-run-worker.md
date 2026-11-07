---
title: 健全性v2 B0 capability-map cap-run-worker — run/lease・worker・credential/sandbox の棚卸し
tasks: [01M6V1TGEG5QT5P6PM0Q0D0QMK]
status: done
updated: 2026-11-07
---

# 健全性v2 B0 capability-map cap-run-worker

完了日: 2026-11-07

## 結論
- 断片 `capability-map/cap-run-worker.json`（schema `celeris.health-v2-b0.capability-map-fragment/1`）に 8 capability、共有状態 14 件、architecture-map との差分 8 件を書いた。
- capability: run の claim と lease、孤児 run の回収、continuation・checkpoint・session resume、worker protocol と adapter、sandbox と db_guard、必須規則の配送、credential broker と credential 待ちの再開、scratch・build cache・disk guard。
- 複数の書き手がある共有状態: 表 `tasks`（lease 列、task-core の 5 module）、`work_units`（5 module）、`runs`（2 module）、`node_sessions`（3 module）、event `worker_finished`（task-dispatch の 5 module と task-ops::comment）、`worker_progress`（4 module）、file `lease.json`（task-worker・task-dispatch・celerisctl の 5 module）。
- architecture-map との主な食い違い: 行 80 の孤児回収の不変条件は「lease の期限切れを確かめてから」だが、`orphan.rs` は持ち主不在（holder_gone）を確かめれば失効を待たずに回収する。run の lease（`dispatcher/leases.rs`・store の `acquire_lease`）と credential 待ち（`credential_gate.rs`・`credential_resume.rs`）には地図の行が無い。

## 方法
- 読み取りだけ（grep、ファイルの先頭の doc comment、`model/events.rs` の Event の列挙）。build・試験は実行していない。
- SQLite 表の書き手は、`INSERT`・`UPDATE`・`DELETE FROM <表>` を含む非試験の `.rs` を grep で数えた。event の書き手は `Event::<Kind> {` を組み立てて追記する箇所を目で確かめた（match の読み取りは除いた）。
- 試験コストは `test-targets.json` の `build_no_run[<crate>].secs`。task-core・task-dispatch・task-worker・celeris だけに値がある。celeris-credentiald と e2e は計測外で `null`（cost_source は `not measured in stage 1`）。
- 基準は `git merge-base HEAD main`（件名「repair: main 取り込みの衝突解消の import を整形する」。SHA は JSON の `base_sha`）。WU の session は migration 0043 で `node_sessions` に足した列で、同じ表に含めた。

## 証拠（提出時の HEAD で実行）
- 生成スクリプトで、断片の全 path（owner・evidence・port・試験・書き手）が repo に実在することを検査した → 欠落 0。
- `git diff --name-only $CELERIS_WU_BASE` → 変更は `agent-docs/progress/2026-11-07-health-v2-b0/` の下の 2 file だけ。

## 未解決
- event の書き手は grep による目視で、マクロや helper（`Event::worker_progress` 等）経由の書き手には漏れがあり得る。全書き手の一覧は deps-shared-state unit が持つ。
