---
title: 健全性v2 G3 inventory-kind — cron の種類 health_inventory と task を作らない種類の共通の検証
tasks: [01M6ZDF99MKHFN57PN0H7G6Z9Z]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 inventory-kind — cron の種類 health_inventory と task を作らない種類の共通の検証

完了日: 2026-11-09

WorkUnit `kind`（task 01M6ZDF99MKHFN57PN0H7G6Z9Z、unit inventory の子）。[G3 の ADR](../../adr/2026-11-08-health-enforcement-loop.md) の (6) D6.1・D6.2 と (8) の write_set `^crates/task-ops/src/cron_jobs(\.rs|/[a-z_]+\.rs)$` に従う。基準は `$CELERIS_WU_BASE`。

## 実装前 brief
1. 根本原因: cron に棚卸しの種類が無い。task を作らない種類の検証（task の欄の一覧・overlap）は `validate_cos_review` と `validate_project_reconcile` に同じ形で 2 回書かれていて、3 種類目を足すと 3 回目の写しになる。
2. 責務 owner: `crates/task-ops/src/cron_jobs.rs`（種類の読み取り・検証・発火の分岐・`CronInputSink`）。
3. 既存の再利用先: `project_reconcile` の経路（`decide_post`・`check_manual_run`・`NoCronInputSink` の既定 Err）。
4. 選択肢: (a) `validate_health_inventory` を 3 つ目の写しとして書く、(b) 共通の関数 1 つに寄せて 3 種類が呼ぶ。(b) を採った（規約 2）。
5. 新しい状態・分岐・fallback・port: `CronJobKind::HealthInventory` と `CronInputSink::post_health_inventory`（既定 Err。daemon の tick の sink だけが積む）。fallback・flag は無し。
6. 撤去対象: 2 つの validator の中の task の欄の一覧と overlap の検査の写し、`reconcile_project_id` の中の読み取りの本体。
7. 不変条件: 3 種類が同じ欄を同じ文言で拒む、既定では job が無く発火しない、sink の無い発火は `error` を履歴に残す、手動実行は拒否。下の試験で確かめる。

## 変更
- `crates/task-ops/src/cron_jobs/input_kind.rs`（新規）: task を作らない種類の共通の検証 `validate_input_kind(job, kind)`（task の欄の一覧 `INPUT_KIND_TASK_FIELDS` と overlap `skip` の検査）、`extra.project_id` の読み取り `template_project_id`、必須の検査 `require_project_id`。
- `crates/task-ops/src/cron_jobs/health_inventory.rs`（新規）: `CRON_KIND_HEALTH_INVENTORY = "health_inventory"`、雛形の検証（共通の検証 + `extra.project_id` 必須）、発火で sink に積む `post`、sink の無い呼び出し元の答え、手動実行の拒否。
- `crates/task-ops/src/cron_jobs.rs`: mod 宣言と `pub use`、`CronJobKind::HealthInventory`、`template_kind` の読み取り、`validate_job`・`fire_job`・`decide_post`・`check_manual_run` の分岐、`CronInputSink::post_health_inventory(job, scheduled_for, project_id)`（既定 Err）。`validate_cos_review`・`validate_project_reconcile` は共通の関数を呼ぶ形にし、`reconcile_project_id` は `template_project_id` を呼ぶ。file は 1096 行 → 1072 行（+41 −65）。
- `crates/task-ops/src/cron_jobs/health_inventory_tests.rs`（新規）: 下の試験 6 件。
- 変えていないもの: `crates/task-api/src/cron_jobs.rs` は `CronJobKind` を `CosReview` とそれ以外（`_`）で分けるだけなので変更不要。`docs/api/v1` には `extra.kind` の値の一覧が無い（`CronTaskTemplate.extra` の説明は「job 固有の値」だけ）ので、文書と生成物（`docs/api/v1/api-v1.schema.json`・`web/api/generated/`）は変えていない。config の例と seed には足していない（D6.2）。dispatcher の sink（`dispatcher/cos_chat/review.rs` の `CosReviewSink`）は unit runner が実装する。

## 試験
守る不変条件と、既存の試験で検出できない理由（既存の cron の試験は `cos_review` と `project_reconcile` しか作らない）:
- `health_inventory_requires_project_id`: `extra.project_id` が無い・空白・文字列でない job は作れない。正しい job は `CronJobKind::HealthInventory` と読まれる。
- `health_inventory_rejects_task_fields_and_overlap`: task の欄 9 つのどれを書いても、`overlap = queue` でも作れない。
- `health_inventory_disabled_by_default`: job が無い store は発火しない。`enabled` を省いた seed と同じ一時停止の job も発火しない。`config/*.toml` の例のどれにも `health_inventory` が無い（D6.2）。
- `health_inventory_without_sink_fails_fire`: `NoCronInputSink` の発火は `error`（`health_inventory runs are posted only by the daemon tick`）を履歴に残し、task を作らない。手動実行は拒否する。
- `health_inventory_fires_through_the_sink_once_per_time_and_never_creates_a_task`: sink は予定時刻ごとに 1 回、`extra.project_id` を受け取る。`Busy` は `skipped_overlap`。task は作らない。
- `health_inventory_shares_input_kind_validation`: `cos_review`・`project_reconcile`・`health_inventory` の 3 種類で、task の欄 9 つと `overlap = queue` の各場合について `validate_job` の誤りが共通の関数 `validate_input_kind` の誤りと同じ文になる。`reconcile_project_id` と `template_project_id` が同じ値を返す。

既存の `crates/task-ops/src/cron_jobs/tests.rs` と `crates/task-dispatch/src/dispatcher/tests/project_wake.rs` は変えていない。

## 検査（提出時の HEAD で実行）
- `cargo nextest run -p task-ops cron_jobs`: exit 0、23 tests passed（新しい 6 件と既存の 17 件）。
- `cargo fmt --all -- --check`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `bash scripts/dev/test-affected.sh`: 結果は下の「test-affected の結果」に書く。

## ADR との違い
- D6.1 は「`crates/task-ops/src/cron_jobs.rs` に種類を足す」と書くが、本体は `cron_jobs/health_inventory.rs`、共通の検証は `cron_jobs/input_kind.rs` に置き、`cron_jobs.rs` には mod 宣言・分岐・1 行の委譲だけを足した。理由: task の objective の指示と、check-health の `size.responsibility-growth` を悪化させないため。write_set の `^crates/task-ops/src/cron_jobs(\.rs|/[a-z_]+\.rs)$` の範囲に収まる。
- 手動実行の拒否の文は `project_reconcile` の「the daemon tick posts the wake」に対して「the daemon tick posts the run」にした（積むのは wake でなく棚卸しの 1 回分なので）。扱い（拒否）は同じ。
- D6.2 の「既定の seed に無い」は、daemon の既定の `[cron]` が空（`CronConfig::default`）なので、試験では配布する config の例（`config/*.toml`）に `health_inventory` が無いことを確かめた。task-ops は celeris crate に依存しないので `CronConfig` を直接は読まない。

## 未解決事項
- dispatcher の sink が `post_health_inventory` を実装するまで（unit runner）、人が job を作ると daemon の tick の発火は毎回 `error`（`health_inventory runs are posted only by the daemon tick`）になる。既定では job が無いので本番の挙動は変わらない。

## 提案
- 無し。

## 本番への移行
- この unit では無し（config・表・API の変更は無い）。有効化の手順は unit record が書く。
