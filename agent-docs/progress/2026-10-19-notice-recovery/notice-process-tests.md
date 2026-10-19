---
unit: notice-process-tests
status: done
completed: 2026-10-19
---
# notice-process-tests: dagbench_process に通知主導の実 process 受入を追加

- `crates/campaign/tests/dagbench_process.rs`: `recovery_notice_continuation`（受理後 / 待機後の停止 → 外部完了 → 再開 barrier、
  重複通知、同一 External 生存、通知なしでは再開しない probe）と `recovery_lost_response`（受理・物理起動を永続確定後に応答のみ喪失 →
  制御 SIGKILL → 再開照会で同一 logical/physical ID・起動数・消費を照合）。baseline / Celeris の両方式で同一 DAG・枠・seed・event・nth。
  既存の `role_entry` / `Supervisor` / barrier / process raw /2 / `observe_until_complete` を再利用（external-ipc は再実装なし）。
- 通知主導: control は `NoticeChannel`（有界 blocking receive・永続適用後 ack）で完了を知る。Celeris 側は poller を失敗させる。
- 「通知なしでは再開しない」: 外部時計の前進を拒否する再開 control（`frozen`）は完了も通知も得られず失敗し、消費・継続は 0。各 run は
  Limits の wall と待ち上限（`INSURANCE` / `PROBE_BOUND`）で打ち切り、所有子を kill・回収する。
- 新版 raw: process raw /2 に `notice-run.json`（`celeris.dagbench-notice-raw/1`）と独立 ledger `notice.jsonl`（control_consume）を置き、
  `compute_notice` が ext.jsonl / notice.jsonl から数え直す。marker の無い旧版 process raw は `RawKind::PollEraProcess` として区別し、
  新版の再集計は明示的に拒否（`notice_raw_is_distinguished_from_poll_era_process_raw`）。
- `baseline.rs`（修正）: 通知主導では再開時の起動照会が完了済みを見つけても wait を完了させない（通知の実受信で継続する）。照会は物理 ID の確認のみ。
- 保存先の点検: raw を残すのは `run_dir`（`DAGBENCH_PROCESS_RAW_DIR`、絶対 path 必須）だけ。未指定は TempDir（試験後に消える）。
  `ipc_bounded_shutdown` / `dagbench_accept` は元から TempDir。unix socket のみ短命の `/tmp` dir。

## 証拠
- `DAGBENCH_PROCESS_RAW_DIR=/local/celeris/data/workspaces/01M5AYRWWBPVBWDCAMZ476MSMP/artifacts/raw cargo test -p celeris-campaign --test dagbench_process -- recovery_notice_continuation recovery_lost_response --exact` → 2 passed（24s）。
- 環境変数未指定で `cargo test -p celeris-campaign --test dagbench_process --test dagbench_accept --test dagbench_ipc --test dagbench_baseline` → 9 / 6 / 9 / 6 passed。
- `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 未解決
- `cargo clippy -p celeris-campaign --tests` は既存の `tests/launcher.rs`（redundant_closure）で落ちる。本 unit の範囲外で gate（`--workspace`）には含まれない。
