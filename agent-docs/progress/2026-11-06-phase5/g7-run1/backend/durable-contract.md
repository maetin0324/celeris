---
title: 起動境界の永続契約（chat run の launched_at・prelaunch_reason・attempt 欠測・cost_state）
tasks: [01M6RJTQQ629D0NP953708JMS4]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# 起動境界の永続契約

完了日: 2026-11-06

正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md)（D1・D4・D5）。
この WU の細目 ADR: [2026-11-06-chat-run-launch-boundary-store](../../../../adr/2026-11-06-chat-run-launch-boundary-store.md)
（旧い走行中の行の判別 `launch_tracked`、attempt 欠測の保存先）。

## 変更（commit は件名で列挙）
- `phase 1: 起動境界の回帰試験（修正前に失敗）と store 側 ADR`
- `phase 2: chat run の起動境界と attempt 欠測を永続化する`

## 修正前の回帰（phase 1 の commit 時点の現行実装）
- コマンド: `cargo test -p task-core --lib launch_boundary_regression`
- exit: 101（2 件とも FAILED）。
- 失敗の assertion:
  - `chat_runs_has_launch_boundary_columns`: `missing column launched_at: ["run_id", …, "first_output_at"]`
  - `claim_marks_new_rows_as_launch_tracked_with_no_launch_time`: `no such column: launch_tracked`

## 修正後
- 同じコマンド（試験は 14 件に増やした）: 14 passed, 0 failed。
- 提出時の HEAD で実行:
  - `bash scripts/dev/test-affected.sh`: exit 0。nextest 7775 件 pass（skip 22）、doctest ok、web 型検査（tsc -b）と web unit（vitest・node --test）も通過。
  - `cargo clippy --workspace -- -D warnings`: exit 0。
  - `cargo fmt --all -- --check`: exit 0。
- 生成物: `UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema_matches_generated && node web/scripts/gen-types.mjs`
  で `docs/api/v1/api-v1.schema.json`・`web/api/generated/schema.json`・`web/api/generated/types.ts` を更新した。
- 基準は `git merge-base HEAD main`（WU では `$CELERIS_WU_BASE`）。

## 永続 API（`task-core`、`SqliteStore` のメソッド）
後続（worker/dispatcher の起動接続）が呼ぶ。この WU では接続していない。

起動側の順序: route と account の決定 → skill の配置 → run credential の発行 → **`chat_run_mark_launched`** → `Ok` のときだけ spawn。

| API | 引数 | 振る舞い |
|---|---|---|
| `chat_run_mark_launched(run_id, now)` | run id、注入時刻 | `launched_at` を**初回だけ**書く。再送は保存済みの run を返し、event を足さない。`Err` なら spawn しない。spawn が失敗しても `launched_at` は残す。route の failover（2 本目以降）は書き換えない。terminal・`stopping`・旧行（`launch_tracked=0`）は `Conflict`（書かない）。 |
| `chat_run_finish_prelaunch(run_id, state, reason, reason_text, now)` | `state` は failed/stopped/interrupted、`reason` は閉じた 6 値 `ChatPrelaunchReason` | 起動前終端。`launched_at` ありは `Conflict`（何も書かない）。旧行も `Conflict`。terminal 済みは同じ state・reason なら冪等、違えば `Conflict`。completed は `Invalid`。文字列からは `ChatPrelaunchReason::parse`（列挙外は `Invalid`）。 |
| `chat_run_recover(run_id, cause, reason_text, now)` | `ChatRecoveryCause::{DaemonRestart, HumanStop}` | 新規行で `launched_at` NULL → `daemon_restart_before_launch`（interrupted）/ `stopped_before_launch`（stopped）。起動済み行と旧行（`launch_tracked=0`、境界不明）は reason を付けずに interrupted / stopped で閉じる。terminal 済みは何もしない。 |
| `chat_run_record_attempt_usage(run_id, Option<&Usage>)` | attempt ごとに 1 回 | `Some` は `usage_json` へ加算（欠けた欄は None のまま）。`None` は `attempts_usage_missing` を 1 増やす。1 回でも呼んだ run では `chat_run_finish_with_telemetry` が `usage_json` を上書きしない。`launched_at` の無い run と terminal は `Conflict`。 |

- 通知 payload: 既存の `run` event（`ChatEventData::Run`、中身は更新後の `ChatRun`）。`mark_launched` の初回と各終端で 1 件ずつ。再送では追記しない。いずれも状態更新と同じ transaction。
- 失敗時: 記録に失敗（`Err`）したら spawn しない。`ChatError::Conflict` は「すでに終端・stop 済み・旧行」で、起動してはいけない合図。
- `cost_state`（view と将来の予算の和が同じ判定を使う）:
  - `not_launched`: finished・`launch_tracked=1`・`launched_at` NULL・`prelaunch_reason` が列挙内。
  - `known`: finished・usage が読める・`attempts_usage_missing=0`。
  - それ以外（走行中、旧行、境界不明、列挙外の reason、欠測あり、旧 event）は `unknown`。
- migration 0120（main 取り込みで 0119 から付け替え。0119 は main の `0119_chat_queued_index`）: 5 列を足すだけ。既存行は推測で埋めない（`launched_at`/`prelaunch_reason` NULL、`launch_tracked=0`）。usage・credential・events は変えない。

## 試験（`crates/task-core/src/chat/launch_boundary_regression_tests.rs`、一時 SQLite・注入時刻）
初回時刻の保持と再送、6 reason の roundtrip、列挙外の fail closed、起動後の prelaunch 拒否、再送と矛盾、stopping の起動拒否、
回収 2 原因 × 3 種の行（新規未起動・起動済み・旧行）、attempt の既知→欠測・欠測→既知・複数既知の合計、最終 telemetry が欠測と既知 usage を消さないこと、
再 open、旧 event の読取、migration 前後で usage・credential・events が残ること。

## 範囲外に触った箇所（コンパイル・試験の都合）
`SCHEMA_VERSION` 118→119（main 取り込み後は 120）に伴い、版数を固定した試験（task-core 内の複数、`task-dispatch` の `project_evidence_integrity.rs` 1 行）と
`crates/campaign/src/ops_r1_trace.rs` の対応版数の一覧に 119（同 120）を足した。0119（同 0120）の ALTER を再適用する旧版 fixture（`roadmap_tests.rs`・`script_domains_tests.rs`）には追加列の DROP を足した。

## 未解決・後続へ
- 予算の和 `project_run_usage_between`（`store/project_guard.rs`）はまだ旧条件のまま。`cost_state` と同じ判定にする作業は usage-query / budget WU。
- `budget_today.not_launched_runs`、API 文書（`docs/api/v1/api.md`）は api-contract WU。
- worker/dispatcher の接続（`launch.rs` の spawn 直前の `mark_launched`、`record_attempt` からの `chat_run_record_attempt_usage`、`routes.rs` の `fail_routes` の `finish_prelaunch`、起動時回収の `chat_run_recover`）は後続。
- 人の stop は現行どおり `running → stopping` で、起動前の `stopped_before_launch` へ閉じる呼び出し（`chat_run_recover(HumanStop)`）は起動側が行う。

## 提案
- 後続は `cost_state` の判定（`run_store.rs` の `cost_state_of`）を SQL の条件に写すとき、同じ試験の組（旧行・列挙外・欠測あり）を使う。
