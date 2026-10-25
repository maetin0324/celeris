---
title: IEEE recovery scheduler — ExtBackend の PBS 実装と両 arm 共通 spec（ext-pbs）
tasks: [01M5SSBCP75CTVF2SHD0QQ9ZGG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# ExtBackend の PBS 実装と両 arm 共通 spec（ext-pbs）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。
[ADR](../../adr/2026-10-25-recovery-scheduler-backend.md) D2・D4・実装付記 I4・I5 に従い、実装で決めた形を付記 I10 に足した。
実 scheduler・ssh・qsub・外部 network には通信していない（偽 transport のみ）。

## やったこと

- `crates/campaign/src/dagbench/pbs/backend.rs` を追加。
  - `RecoverySpec`（`deny_unknown_fields`、既定値なし）: 資源（select/place/walltime/queue/account）・終了条件（poll/deadline）・attempt 上限 1 を arm を取らない関数（`termination`・`workload_attempts`・`intent_spec`/`qsub_request` の資源部）で作る。`workload_attempts≠1`・`rerunnable=true`・sha256 でない pin・0 の上限は読み込みエラー。
  - spec の sha256 を測定 dir の `recovery-spec.json` pin と照合し、両 arm が別 spec なら `PbsExt::open` が失敗する。
  - `PbsExt`（`ExtBackend` 実装）: submit は `IntentLedger::submit`（unknown_submit は `resolve` で poll 間隔ごとに deadline まで照合）、lookup は `qstat -x -f` 原票を `check_record` で照合して Running/Completed。照合不能は `Lookup::Unresolved`/`SubmitError::Unresolved`、履歴消失・照会失敗は `io::Error`。`Lookup::Unknown` は台帳に intent が無いときだけ。時計は注入可能な wall clock（`IntentClock`）で、`advance(t)` は開始 + t まで待ってから poll。`due_secs` は None。`record_consumed` は `consumed.jsonl` で 2 回目を `false`。枠と max_jobs は測定 dir 全体で数え、未解決 intent は枠を戻さない。
- `ext.rs` に `Lookup::Unresolved`・`SubmitError::Unresolved` を足した（ExtSim は返さない）。baseline の再開照会・reconcile・submit と celeris_side の応答喪失分岐は、これを受けると再投入へ戻さずにエラーで止まる。ipc の `PROTOCOL_VERSION` を 2 にした。
- 試験 `crates/campaign/tests/dagbench_pbs_backend.rs`（7 件）: 両 arm の qsub argv が identity 欄以外一致し実送信も一致・spec hash 一致、別 spec の pin 拒否と不正 spec、再起動後の同じ job 再追跡（qsub 1）と二重消費拒否、既存 `Baseline` を PbsExt に差し替えて全保存点で停止→再開しても qsub 2・消費 2、unknown submit の deadline 停止（lookup も Unresolved、qsub 1、枠を戻さない）、baseline が Unresolved で停止し再起動でも再投入しない、履歴消失は Err・原票不一致は Unresolved。

## 証拠

commit「phase 1: add PBS ExtBackend with shared recovery spec」の HEAD で実行。

- `cargo test -p celeris-campaign --test dagbench_pbs_backend` → 7 passed。
- `cargo nextest run -p celeris-campaign --test dagbench_*`（全 15 file）→ 106 passed / 3 skipped。既存 dagbench 試験は無変更（`git diff $CELERIS_WU_BASE -- crates/campaign/tests` は新 file のみ）。
- `bash scripts/dev/test-affected.sh` → exit 0、2673 passed / 7 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → crates/campaign と ADR・本進捗のみ。

## 未解決事項

- ipc server を `Box<dyn ExtBackend + Send>` に広げる変更（I1）と、CelerisSide を PbsExt で走らせる入口は wire-entry に残した。
- raw（`celeris.dagbench-scheduler-raw/1`）への qstat 原票の書き出しは raw-collect。`PbsExt::drain_replies` で原票を引き取れる。

## 提案

なし。
