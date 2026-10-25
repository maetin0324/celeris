---
title: IEEE recovery scheduler — opt-in の実 scheduler 入口・wrapper 整理・offline e2e（wire-entry）
tasks: [01M5SSBCP75CTVF2SHD0QQ9ZGG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# opt-in の実 scheduler 入口・wrapper 整理・offline e2e（wire-entry）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。
[ADR](../../adr/2026-10-25-recovery-scheduler-backend.md) D2・D4・D5・I7・I8 に従い、実装で決めた形を付記 I12 に足した。
実 scheduler・ssh・qsub・外部 network には通信していない（偽 transport のみ。LLM 呼び出し 0）。

## やったこと

- `crates/campaign/src/dagbench/pbs/entry.rs` を追加。
  - `BackendChoice`（既定 `ExtSim`、`Pbs` は transport・時計の注入時だけ）。`dagbench run` は `BackendChoice::ExtSim` で開き、
    `--backend` の既定は `ext-sim`、`--backend pbs` は CLI で実 transport を組まずに拒否。
  - `refuse_wrapper_env`: `CELERIS_SUBMIT_CMD`/`CELERIS_SUBMIT_AUDIT` があれば recovery の起動を拒否（投入は intent 台帳 1 本）。
  - `Gate`/`GatedExt`: 保存済み Waiting（`state.json` / 測定専用 SQLite の cluster job wait を読み直す）を確かめたら、
    normal は完了解放 barrier を外し、A2 は所有 controller を止める。
  - `run_trial`: normal と A2（保存確認→停止→停止中に observer が同じ job の終了原票を取得→再開→同じ job を消費→独立採点）、
    raw（`celeris.dagbench-scheduler-raw/1`）への原票と結合行、`barriers.jsonl`・`a2.json`・`grade.json`。`BaselineController` を同梱。
  - `create_measurement`: spec を pin して新規 raw を作る。
- `crates/campaign/fixtures/dagbench/recovery-scenarios.json`（`rcv-1`: 外部 job 1 本、両 arm 共通）。
- `scripts/campaign/celeris-qsub`: recovery の投入には使わない旨を注記し、`CELERIS_SUBMIT_INTENT` のある環境では投入せず exit 4（A arm の挙動は不変）。
- 試験 `crates/campaign/tests/dagbench_pbs_e2e.rs`（5 件）。
- `docs/architecture-map.md` に `campaign::dagbench::pbs` の行（6 列、関連試験・不変条件付き）。

## 証拠

commit「phase 1: wire opt-in PBS recovery entry with offline e2e」の HEAD で実行。

- `cargo test -p celeris-campaign --test dagbench_pbs_e2e` → 5 passed。
  - 両 arm × normal/A2 を 1 測定 dir で: 物理 qsub 4・消費 4、全 trial `validated_completed`、A2 の barrier 順序
    `wait_saved→controller_stopped→released→observed_finished_while_stopped→controller_resumed`、再開前後で intent 行不変、
    消費した job_ref が保存済みの job と一致。validator 合格、再収集で qsub・消費は増えず raw からの再集計が一致。
  - grade 失敗（baseline A2・celeris normal）は `executed_unvalidated`。
  - unknown submit（qsub timeout・marker 検索未確認）は両 arm で停止、qsub 1 のまま、`unknown_submit` を記録し `rejected_before_send` にしない、消費 0。
  - 既定 ext-sim・`--backend pbs` の CLI 拒否・wrapper 環境の拒否・`celeris-qsub` の recovery job 内拒否（印なしは従来どおり記録）。
- `python3 scripts/dev/check-architecture-map.py` → OK（exit 0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0、2684 passed / 7 skipped（commit「phase 1: wire opt-in PBS recovery entry with offline e2e」の HEAD）。

## 未解決事項

- A2 の controller 停止は in-process（incarnation の終了）。別 process の PID・starttime 確認と SIGKILL は実接続の段で `dagbench::process` に載せる（I12）。
- CLI からの実 PBS 実行（`SshPbs`・実 Site の barrier/成果物同期）は未実装。実接続は認証・残枠確認と人の承認後。
- `celeris.dagbench-scheduler-metrics/1` は未実装。

## 提案

なし。
