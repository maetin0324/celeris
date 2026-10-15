---
task: 01M4Z2B2HA2FEK1ZRWKS8MXSRR
unit: cw-core
status: done
completed: 2026-10-15
---
# cw-core: credential 待ちの永続状態・event・冪等遷移

ADR 2026-10-15-mlsys-min-decision-assisted D4 に従う。名前: `waiting_for_auth`（待ち理由。task の Status は増やさない）、
`secret_stored`（event。secret id と fingerprint のみ）、集約キー `credential:<secret_id>`（ADR の「secret id 単位」）。

## 実装
- `crates/task-core/migrations/0081_credential_waits.sql`（全ブランチ走査で 0081 が空き。SCHEMA_VERSION 80→81、版数を固定する既存試験を更新）。
  - `credential_waits`: work_unit_id・task_id・secret_id・provider・purpose・state（waiting/rejected/verified/resumed/cancelled）・aggregation_key・notice_item_id・seen_fingerprint・last_verify_kind（valid/invalid_key/insufficient_permission）・resumed_at。秘密の値を持つ列は無い。
  - 開いている行は (work_unit_id, secret_id) で partial UNIQUE（enter_wait の冪等性）。
  - `credential_wait_events`: 追記専用（`secret_stored`、secret_id + fingerprint）。
- `crates/task-core/src/store/credential_wait.rs`（`SqliteStore` のメソッド）:
  `credential_wait_enter`（冪等・`first_for_key` で通知を 1 件に集約）、`credential_record_secret_saved`（重複 (secret, fingerprint) は何も足さない）、
  `credential_wait_mark_verified`（fingerprint 一致のみ）→`credential_wait_claim_resume`（`verified→resumed` の条件付き UPDATE。一度だけ true。task が cancelled なら `cancelled` にして false）、
  `credential_wait_mark_rejected`（再登録待ち。Valid は拒否）、`credential_wait_cancel`、`credential_waits_list_waiting`、`credential_wait_set_notice_item`、`credential_wait_get`。

## 証拠
- `cargo test -p task-core credential_wait`: 13 件 + 既存 1 件 pass（claim_resume の 8 並行で 1 件のみ成功、cancel 済みは再開しない、既存 v80 DB の migrate、重複 event、誤キー→更新→再開、reopen 復元、列に秘密値なし）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `bash scripts/dev/test-parallel.sh`: 初回 5160 pass / 1 fail（`delivery::migration_0042_fills_gaps…` が版数一覧に 81 を持っていなかった。修正して再実行で pass）。

## 未解決・提案
- event は task に属さない（secret の PUT は task 外）ので `Event` enum ではなく専用の追記専用表にした。`events` へ載せる必要があれば後続 unit（cw-resume）で task ごとに足す。
- 通知の発行・Blocked 遷移・検証・再queue は後続 unit（cw-enter / cw-resume）。`TaskWait` への credential 種の追加も cw-enter 側。
