---
title: 健全性v2 B0 capability-map — capability→owner→不変条件→public port→試験の地図と共有状態の書き手
tasks: [01M6V1TGEG5QT5P6PM0Q0D0QMK]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性v2 B0 capability-map

完了日: 2026-11-07

## 結論
- 6 つの調査 unit の断片を `capability-map.json`（schema `celeris.health-v2-b0.capability-map/1`）に統合した。
  capability 41 個（断片の id は全部入り、重複 id は無し）、関連試験 228 件（全 path 実在）、共有状態 263 件、
  書き手が複数ある共有状態 52 件（SQLite 表 16・event kind 29・file 7、うち複数 crate が書くもの 40 件）、
  docs/architecture-map.md との食い違い 51 件。
- 人が読む判断材料は親 task の成果物 `artifacts/b0/capability-map.md` に書いた（capability 表、crate 依存と
  module の use の向き、複数書き手の共有状態、architecture-map との食い違いの 4 節）。
- 基準は着手時の `git merge-base HEAD main`（SHA は JSON の `base_sha`）。crates/・web/・control-ts/・
  docs/architecture-map.md は変えていない。

## 変更
- `phase B0: capability-map assemble の統合 JSON と進捗` — 統合 script `capability-map-assemble.py`、
  描画 script `capability-map-render.py`、`capability-map.json`、この進捗ファイルを追加した。

## 統合の規則
- capability は id で重複を除く。同じ id があれば owner・不変条件・port・試験を両方残す（今回は重複なし）。
- `tests` は試験 path の文字列の配列。試験ごとの target と build/link 秒は `test_costs` に置く。
- 共有状態は module-deps.json と断片を (kind, name) で合わせた。断片の event 名（Rust の variant 名）は
  snake_case に直した。「a / b / c」とまとめた表名は分け、その表名を含む file の書き手だけを付けた。
  「tasks（lease 列）」のような列の注記は表 tasks に寄せ、注記を notes に残した。
- multi_writer は書き手が 2 crate 以上、または 2 つ以上の top-level module（path の src/ 直下）にまたがるもの。
  module-deps の数え（表 14・event 20・file 5）より増えた分は、各調査が足した書き手による。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/capability-map-assemble.py` → exit 0、
  missing_paths 0 件。
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/capability-map-render.py <親 task の artifacts/b0/capability-map.md>` → exit 0。
- task の受け入れ 0 の python 一行（capability 8 個以上・tests の path 実在）→ exit 0、欠け `[]`。
- 再計画 v2 の WU check 相当（fragment の id 41 件がすべて統合 JSON にある、各 capability に owners・invariants・public_ports・tests・test_costs がある、tests は実在 path の文字列、test_costs は実在 path と build_link_secs を持つ）→ exit 0、`ok 41 52 51`（capability 41、multi_writer_state 52、arch_map_diffs 51）。
- `sh scripts/dev/check-doc-links.sh` → exit 0
- `sh scripts/dev/check-adr-numbers.sh` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0
- `git diff --quiet $(git merge-base HEAD main) -- crates/ web/ control-ts/` → exit 0
- 文書と JSON だけの変更なので、test-affected・clippy・fmt は対象の crate が無い（未実行）。

## 解決済み
- WU check と task の受け入れ条件 0 は `tests` の形で食い違っていた（WU check は object、task 条件は path 文字列を読む）。再計画 v2 で WU check を直し、`tests` は path 文字列、試験コストは `test_costs`（path・build_link_secs）に置く形に揃えた。データは変えていない。

## 未解決
- 試験コストが未計測の試験が 56 件ある。段 1 で build を測ったのは task-core・task-dispatch・task-api・
  task-worker・celeris の 5 crate だけで、task-ops・llm-proxy・campaign・script-runtime・celerisctl・web・
  control-ts は測っていない。
- 共有状態の書き手は grep による。動的 SQL と store 関数の呼び手を辿った書き込みは数えていない（漏れは未確認）。
- 現行の書き手が無い event kind 4 種（model_role_assignment_changed・project_plan_proposed・
  project_plan_decided・routing_shadow_recorded）を残す意図は未確認。

## 提案
- 統合設計（g1-v0-design）で、task-api が直接書く表（cos_inbox_items・chat_client_requests・
  browser_task_policies）を task-core の store 関数へ寄せるかを決める。
- 複数 crate が組み立てる event kind（transitioned は 4 crate・25 module）の書き手を store 関数に絞るかを決める。
- 食い違い 51 件を docs/architecture-map.md に反映する task を別に立てる。
