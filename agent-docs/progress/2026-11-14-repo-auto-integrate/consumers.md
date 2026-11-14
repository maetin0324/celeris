---
title: 消費側（task-ops・task-dispatch・campaign）: 取り込み判定を repo の欄から読む
tasks: [01M7BT5CNT1PRK0JM3GK60BHSF]
status: done
updated: 2026-11-14
---
# 消費側（task-ops・task-dispatch・campaign）

完了日: 2026-11-14（ADR 2026-11-14-repo-auto-integrate D3/D4/D5/D8 のこの 3 crate 分）

## 変更
- `task_ops::delivery::auto_integrates(store, task)` を 1 つ置いた（project_id あり・`task.repos` 1 件以上・すべての repo の
  `project_repos` 行が存在して `auto_integrate = true`）。`prepare`（偽なら `DeliverySkipped{repo_not_enabled}`）、
  `stranded::reintegrate`、`review_spawn` の `delivery_requires_reviewer_run`、受信箱が同じ関数を使う。
- 受信箱: 理由 `no_commits`・`repo_not_enabled` は出さず、読んだ時点で判定が偽の task の項目も出さない。
- `SnapshotProject.auto_integrate` の書き込みと、`Project`・`ProjectRepo` の literal の `auto_integrate` を整理した。
- campaign の `check_project_rules` は「案件の `repos[]` のどれかで `auto_integrate` が有効（欠落・bool でない・`repos` が無いも有効扱い）なら止める」。
- 試験: Project の欄を使っていた fixture は repo の行の欄へ書き換えた。新規に足した試験:
  - `delivery::tests::auto_integrates_follows_every_repo_of_the_task`（D5 の表の 4 行、行無し・repos 空・project 無し）。
    案件の欄の試験では repo ごとの「すべて有効」を検出できない。
  - `inbox::tests::delivery_skip_no_commits_is_not_attention`（repo_not_enabled・無効 repo の過去の理由を出さない）。
  - `dispatcher::tests::target_sync::partially_enabled_multi_repo_task_does_not_require_a_reviewer_run`（一部無効の 2 repo は reviewer 必須でない）。
  - campaign の `preflight_contract`・`preflight` を repo の欄に書き換え（1 件でも有効なら拒否、欠落・不正形は拒否）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `cargo test -p task-ops` → lib 673 passed ほか全て成功。
- `cargo test -p task-dispatch` → lib 1760 passed（新規試験の追加前）、追加後は新規 1 件も成功。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy -p task-ops -p task-dispatch -- -D warnings` → exit 0。`--all-targets` は gate.rs・disk_forecast.rs・dispatch_ready_phases.rs（この WU が触れていないファイル）の既存の指摘で落ちる。
- `cargo check -p celeris-campaign --lib` → 成功。

## 未解決
- campaign の試験（`cargo test -p celeris-campaign`）は dev-dependency の task-api が api unit の置き換えまで compile しないため、この WU では実行できていない（未確認）。試験は api unit の後に通す。`preflight_contract.rs` の `real_project` は repo の欄を store で有効にしている（API の切り替えは task-api の試験の範囲）。
- `celeris` crate の delivery.rs（`tick_with`）は次の unit が直す。この WU では `workspace` 全体は compile しない。

## 提案
なし。
