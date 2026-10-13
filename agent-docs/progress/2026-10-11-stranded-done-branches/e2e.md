# e2e: 取り残し→一覧・詳細→route 操作→events の結合試験

tasks: [01M4MRE949FE9DWVT26NDF0T8V]
status: done（WU e2e）

設計は [ADR: done task の変更を取り残さない](../../adr/2026-10-11-stranded-done-branches.md)。

## 試験の内容

`crates/celeris/tests/integration_api.rs` の `integration_api_stranded_branch_round_trip_through_routes_and_events`。

- 一時 dir に `config.toml` を書き、`celeris::Config::load` → `celeris::api_settings` → `task_api::ApiState::new` で、daemon と同じ配線の `ApiState` を loopback に立てる（repo path・default branch・`releases_dir` は config から）。
- 一時 git repo（`main` と `celeris/<task>` の 1 commit）、案件・repo 行、完了した root task、worktree 目印 `worktree.json` を store に用意する。
- 定期 sweep と同じ経路 `celeris::sweep_stranded_branches`（`daemon::stranded_sweep::run` の公開口）で取り残しを検出する。
- 検査する順序:
  1. sweep → `detected`。`GET /api/v1/integrations?state=unintegrated` に 1 件。
  2. `GET /api/v1/tasks/{id}` の `integrations[0].state` = `unintegrated`、`promotion.status` = `promotion_pending`。一覧にも同じ状態。受信箱に `type=unintegrated` の item（`operation_path` が task×repo を指す）。
  3. `hold`（理由）→ `held`、`reopen` → `unintegrated`。
  4. `discard`（理由）→ `discarded`（理由を保存）、`reopen` → `unintegrated`。
  5. `merge` → 統合依頼の記録だけ（`integrated: false`、open な integration request が 1 件）。
  6. `main` へ `git merge --ff-only` → sweep → `integrated`（`merged` event）。詳細の promotion は release が無いので `promotion_pending` のまま。
  7. 同じ `merge` は冪等（`integrated: true`、event は増えない）。
  8. release（取り込んだ head を含む main の sha）を `releases_dir` に置く → `promotion.status` = `promoted`、`release_sha` = main の sha。受信箱の取り残しは消える。
- 各段で `events_for` の統合 event 列を順番ごと検査する: `[detected, held, reopened, discarded, reopened, merged]`。

時計は sweep に固定の unix 時刻を注入する。HTTP handler の時刻は実時計（handler の内部。注入は本体の変更を伴うので今回はしていない）。外部ネットワーク・userns・本番の `~/.local/celeris` には触れない。

## 本体の変更（最小）

- `crates/celeris/src/lib.rs`: `pub fn sweep_stranded_branches(store, now)` を足した。daemon の定期 job と同じ `stranded_sweep::run` を外から呼ぶ口（試験が daemon の配線を通るため）。
- `crates/task-ops/src/view.rs` `attach_integrations`: promotion を `integrated` の行に限って計算していたのを、全 state の行で計算するように直した。理由: ADR（2026-10-11-stranded-done-branches）は promotion pending を「統合状態と独立して表示」と定めている（同 ADR の検査項目 5）。旧い条件のままだと、取り残し（unintegrated）の task 詳細で promotion が `null` になり、受信箱・詳細で `promotion_pending` が出ない。

## 証拠

- `cargo test -p celeris --test integration_api integration_api_` — 1 passed（0.86 s）。
- `cargo test -p celeris --test integration_api --test stranded_integration` — 1 passed、1 passed。
- `cargo test -p task-ops` — 522 passed（view 変更後）。
- `cargo test -p task-api --test integration_api --test list_and_detail` — 4 passed、14 passed。
- `cargo test -p task-api schema` — 3 passed（schema 不変）。
- `cargo clippy --workspace --all-targets -- -D warnings` — exit 0（warning なし）。
- `cargo fmt --all -- --check` — exit 0（`cargo fmt -p celeris` で試験ファイルと lib.rs を整形後）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` — 範囲内（変更は `crates/celeris/src/lib.rs`、`crates/celeris/tests/integration_api.rs`、`crates/task-ops/src/view.rs`、本ファイル）。

## 未解決事項・提案

- 受信箱の `Unintegrated` item は `operation_path`（`/api/v1/tasks/{id}/integrations/{repo}/`）を持つが、merge/discard/hold/reopen のどれを呼ぶかは item に入っていない（`answer.native` ではなく path だけ）。web（views 後段）が操作名を足して route を呼ぶ必要がある。受信箱から 409 や説明文だけの URL で止まる経路が残っていないかは、この項目を埋めてから確かめる（最終 review の受け入れ条件 4）。
- `discarded` や `held` の行にも promotion の欄が付く（view 変更の帰結）。意味としては「本番に出ているか」の独立表示なので、人の表示方針として確認したい。今回は ADR の文言に合わせた。
- 試験の HTTP 側の時刻は実時計。決定性は sweep と git の順序で確保している。

## 全 workspace の検査（最終）

- `TMPDIR=/tmp/e2e-t bash scripts/dev/test-parallel.sh` — nextest passed 5145、failed 3、ignored 14。doctest は exit 0。失敗は 3 件すべて `task-api::browser_e2e`（`approved_credential_requires_conformance_and_does_not_consume_approval`、`denied_approval_fails_the_task_without_a_lease`、`registered_credential_does_not_open_approval_for_uncertified_backend`）で、応答が `503 browser_unavailable: credential broker is not configured`。browser の credential broker の配線が試験環境に無いことが原因と読める。stranded の promotion／view の経路は通らない。ただし base commit での同 3 件は未実行（比較していない）。
