---
title: 網羅の runner（daemon）と readiness への接続
tasks: [01M6J81DZ7C8SMY7HNZTW8GT0Z]
status: done
updated: 2026-11-04
---
# 網羅の runner（daemon）と readiness への接続

完了日: 2026-11-04

## 変更
- `runner: lane_rules の全入力を流す runner と readiness への接続` — 次を足した。
  - `crates/celeris/src/daemon/script_exhaustive.rs`（新規）: `drive`・`finish`・`run`。`model_policy::all_features()` の 19,683 通りを正順・逆順で `call_lane_rules`（deadline は `SCRIPT_CALL_DEADLINE` 50 ms）に流し、`match_rule` と `classify_lane_rules`（`LaneCeiling::default()`）で比べる。fixture file は読まない。
    - 呼び出しの前に generation の in-flight を読み、live の要求があれば呼ばずに待つ。1 件ごとに 5 ms 空ける。timeout の後は 1 秒待つ。
    - 打ち切り: 不一致・安全違反は 1 件目（`mismatch`・`safety_violation`）、timeout か失敗の連続 3 件（`consecutive_errors`）、率 0.1% の超過が決まった時点（`rate_exceeded`）、isolate が無い・drain 中（`generation_gone`）、30 分（`wall_limit`）、停止の合図（`daemon_stopped`）。
    - 進捗は 1000 呼び出しごと、終了は store の `script_exhaustive_finish` に書く（失敗したら 1 秒ごとに書き直す）。live の `script_guard_*`・`script_compare_*` は呼ばない。
  - `script_layer.rs`: `ScriptGenerations` に instance id（`for_instance`。daemon は起動の `instance_id` を渡す）、`exhaustive_start` の実装（`ops` の lock の中で行を書き、runner を tick の外の tokio task で起こす。runner は lock を持たない）、`set_exhaustive_tuning`・`wait_exhaustive`（試験用）。復元は他の instance の `running` を `aborted`（`daemon_restarted`）にし、`shutdown` は runner を止めて `aborted`（`daemon_stopped`）を書いてから isolate を捨てる。
  - `script_guard.rs`（daemon）: readiness の log が `Missing` 固定でなく `script_exhaustive_standing` を読む（昇格の gate と同じ値）。
  - `docs/architecture-map.md` に網羅の行を足した。
- ADR の試験名との対応: `exhaustive_real_bundle_all_inputs_match_and_persist`・`exhaustive_new_generation_invalidates_evidence`・`exhaustive_always_different_bundle_fails`・`exhaustive_does_not_move_live_counters`・`exhaustive_tick_continues_while_running`・`exhaustive_restart_marks_running_aborted`・`exhaustive_waits_for_live_in_flight`（`crates/celeris/tests/script_exhaustive.rs`）。`exhaustive_live_call_not_busy_or_unavailable` は独立の試験にせず、`exhaustive_tick_continues_while_running`（tick の compare が `unavailable`・`busy`・`generation_gone` にならない）と `exhaustive_waits_for_live_in_flight`（runner が待つ間の live の呼び出しが `Reply`）で確かめる。`exhaustive_timeout_backs_off_and_stops_at_three` は単体試験（`src/daemon/script_exhaustive_tests.rs`、`tokio::time::pause`）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `cargo nextest run -p celeris --lib script_exhaustive` → 10 件合格（偽の isolate と `tokio::time::pause`。正順・逆順と 5 ms の間隔、live の待ち、timeout の 1 秒の待ちと連続 3 件、率、不一致、安全違反、generation の消失、30 分、停止）
- `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_exhaustive` → 7 件合格。本物の bundle（control-ts の source から作った bundle。`control-ts` で `pnpm install --offline --frozen-lockfile` の後）の isolate で 39,366 呼び出しが全件 `match`、再起動の後も `passed`（約 57 秒）。
- `bash scripts/dev/test-affected.sh` → exit 0（対象 celeris・celerisctl、949 件合格、script bundle の SKIP 0 件）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `python3 scripts/dev/check-architecture-map.py` → OK（893 件のパス）

## 未解決事項
- 本物の bundle の全件の試験は、負荷のかかった試験機で 50 ms を超えないよう deadline を 30 秒・間隔 0 にしている。50 ms の deadline と間隔は単体試験が時計の差し替えで確かめる。本番の所要時間・latency は未確認（証拠の行の `started_at`・`finished_at`・`latency_*` で分かる）。
- node_modules の無い環境では本物の bundle の試験は SKIP になる（既存の `script_compare_lane_rules.rs` と同じ扱い）。
- API・celerisctl（`exhaustive_start` の呼び出し側）は `api` unit の担当。
