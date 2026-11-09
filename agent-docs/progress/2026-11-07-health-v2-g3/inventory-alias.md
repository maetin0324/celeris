---
title: 健全性v2 G3 inventory-alias — reconcile_project_id の別名を消す
tasks: [01M6ZDF99MKHFN57PN0H7G6Z9Z]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 inventory-alias — reconcile_project_id の別名を消す

完了日: 2026-11-09

WorkUnit `alias`（task 01M6ZDF99MKHFN57PN0H7G6Z9Z、unit inventory の子）。unit `kind` の commit「inventory kind: cron の種類 health_inventory と task を作らない種類の共通の検証」が `crates/task-ops/src/cron_jobs.rs` に残した `pub fn reconcile_project_id`（`template_project_id` を呼ぶだけの 1 行の wrapper）を消した。CLAUDE.md の破壊的変更の方針と、設計 review の `removal_missing` に従う。基準は `$CELERIS_WU_BASE`。

## 実装前 brief
1. 根本原因: unit `kind` は案件 id の読み取りを `cron_jobs/input_kind.rs` の `template_project_id` に移したが、旧い名前 `reconcile_project_id` を wrapper として残した。同じ読み取りに名前が 2 つある。
2. 責務 owner: `crates/task-ops/src/cron_jobs/input_kind.rs` の `template_project_id`（`extra.project_id` の読み取りの正本）。
3. 既存の再利用先: `template_project_id`（`health_inventory.rs` と `input_kind::require_project_id` が既に使っている）。
4. 選択肢: (a) wrapper を消して呼び出し元を `template_project_id` に替える、(b) wrapper を残して doc comment に別名と書く。(a) を採った。(b) は別名を残すので規約 6 に反する。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: `cron_jobs.rs` の `pub fn reconcile_project_id` と、その doc comment。
7. 不変条件: `project_reconcile` の job の案件 id は `template_project_id` で読む（`health_inventory` と同じ関数）。試験 `health_inventory_shares_input_kind_validation` の末尾で確かめる。

## 消したもの
- `crates/task-ops/src/cron_jobs.rs`: `pub fn reconcile_project_id(job: &CronJob) -> Option<String>` と、その doc comment（5 行）。file は 1072 行から 1067 行に減った。

## 替えた呼び出し元
- `crates/task-ops/src/cron_jobs.rs` の `decide_post`: `CronJobKind::ProjectReconcile` の枝は `template_project_id(job)` で案件 id を読む。挙動は変わらない（消した wrapper は `template_project_id` をそのまま返していた）。
- `crates/task-ops/src/cron_jobs/health_inventory_tests.rs` の `health_inventory_shares_input_kind_validation`: 末尾の `assert_eq!(reconcile_project_id(..), template_project_id(..))` を `assert_eq!(template_project_id(&reconcile).as_deref(), Some(PROJECT))` に替えた。守る契約は「`project_reconcile` の job の案件 id を `health_inventory` と同じ関数で読む」で、変えていない。旧い assert は別名と正本が同じ値を返すことしか見ていなかった。新しい assert は、`decide_post` が使う関数が `project_reconcile` の雛形から案件 id を読めることを直接確かめる。

## 検査（提出時の HEAD で実行）
- `git grep -n reconcile_project_id -- crates/`: 一致無し（exit 1）。
- `grep -n 'ProjectReconcile => match template_project_id' crates/task-ops/src/cron_jobs.rs`: `decide_post` の 1 行が一致。
- `cargo nextest run -p task-ops --no-tests=fail -E 'test(/cron_jobs/)'`: exit 0、23 tests passed。
- `cargo fmt --all -- --check`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 6472 tests passed・23 skipped（task-ops とその逆依存の crate）、web の unit 104 files・711 tests passed、`repo_health_gate: all checks passed`。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更 path は `crates/task-ops/src/cron_jobs.rs` と `crates/task-ops/src/cron_jobs/health_inventory_tests.rs`（この進捗 file を書く前）。

## 未解決事項
- 無し。unit `kind` の進捗（`inventory-kind.md`）は `reconcile_project_id` に触れているが、その時点の記録なので書き換えていない。

## 提案
- 無し。

## 本番への移行
- 無し（config・表・API の変更は無い）。
