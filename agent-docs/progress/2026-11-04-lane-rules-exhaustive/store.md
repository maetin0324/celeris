---
title: 網羅の証拠の永続（migration 0110）と store API
tasks: [01M6J81DZ7C8SMY7HNZTW8GT0Z]
status: done
updated: 2026-11-04
---
# 網羅の証拠の永続（migration 0110）と store API

完了日: 2026-11-04

## 変更
- `store: 網羅の証拠の永続(migration 0109)と store API` — migration `0110_script_exhaustive_runs.sql`（`SCHEMA_VERSION` 110。main の取り込みで 0109 が `execution_gate` に使われたため 0109 から振り直した）。終わった行の UPDATE・DELETE を trigger が拒み、領域ごとの `running` は部分 UNIQUE index で 1 行に限る。
- `store/script_exhaustive.rs`: `script_exhaustive_begin`・`_progress`・`_finish`・`_abort_stale`・`_runs`・`_standing`、`current_reference_sha256`、`ScriptExhaustiveFinish`。
  - `begin` は mode が `compare`・`canary` で bundle が mode の行の generation と一致するときだけ書く。全体の停止の間・`lane_rules` 以外・実行中の行があるときは拒む。
  - `finish` は `WHERE state='running'` の CAS（行が既に終わっていれば `None`）。不一致は canary・ts を `rust` へ戻し、`compare` は変えない。安全違反は generation を `invalid` にする。戻しは `system:script-guard` で同じ transaction。timeout・失敗の率だけの失敗は mode を変えない。
  - live の `script_guard_state`・`script_compare_log`・`script_domain_events`（戻しの記録を除く）には書かない。
- `script_domain_set_mode` の昇格 gate が同じ transaction で `standing_tx` を読み、`readiness` に渡す（旧 `Missing` 固定を置き換えた）。
- `ScriptDomainError` に `ExhaustiveDomainUnsupported`（422）・`ExhaustiveModeConflict`（409）・`ExhaustiveRunning`（409）。`task-api` の `domain_problem` が写す。`ScriptGenerationControl::exhaustive_start` を既定の実装（`Unavailable`）付きで足した。
- migration 版数を持つ試験（版数 108 の表明、`SchemaTooNew` の 110→111）と `campaign::ops_r1_trace::SUPPORTED_SCHEMA_VERSIONS` に 110 を足した（main の取り込みで 0109 から振り直し）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `cargo nextest run -p task-core script_exhaustive script_domain_promote` → 21 件合格（再起動後の読み出し `script_exhaustive_survives_reopen`、generation・参照の変更で `Missing` になる `script_exhaustive_generation_change_is_missing`、戻し 3 種、live 計数に触れない試験、昇格 gate の試験を含む）
- `bash scripts/dev/test-affected.sh` → exit 0（7270 件合格、21 skipped）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決事項
- `crates/celeris/src/daemon/script_guard.rs` の readiness の log はまだ `Missing` を渡す。この unit の範囲外なので、`runner` 工程で `script_exhaustive_standing` に替える。
- `exhaustive_start` の本物の実装（runner）と API・celerisctl は後続の unit。
