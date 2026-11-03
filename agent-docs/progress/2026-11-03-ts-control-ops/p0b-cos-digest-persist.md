---
title: P0-B — cos_digest の apply と compare_log・guard 計数の整合性
tasks: [01M6HGFR1TDAP02XY98ES1ZDA1]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# P0-B — cos_digest の apply と compare_log・guard 計数の整合性

完了日: 2026-11-03。親 task 01M6HG7T6ZKTX239KGH0QY4HFQ（段階移行 2）の子 p0b-cos-digest-persist。設計は
[ADR 2026-11-03-cos-digest-persist](../../adr/2026-11-03-cos-digest-persist.md)。S6 の既知の制約 11・未解決 5
（[S6 の進捗](../2026-11-02-ts-control-layer-s6.md)）を閉じる。

## 結論
- digest pass の apply を 1 つの SQLite transaction にした。run ごとの SAVEPOINT に、message（`chat_messages`）・requeue
  （`cos_inbox_items`）・記録（`script_compare_log`）・script-guard の計数（`script_guard_state`。戻し・無効化の CAS を含む）を入れる。
- mode `compare`（と canary の Rust の側）の影は、apply と同じ transaction で outbox（migration 0107 の表
  `cos_digest_shadow_outbox`）に 1 行を書き、影の task か、起動後・影の失敗後の tick の再処理が、行を消して記録 1 件を書く。
- TS の出力を apply する pass は、transaction の中で mode の行を読み直し、違えば Rust の command（fallback `mode_changed`）で apply する。
- 原子化できたので、mode CAS で canary/ts を拒否する案は採らなかった（ADR の「選ばなかった案」）。

## 変更（基準は git merge-base HEAD main）
commit（件名）:
- `p0b: cos_digest の apply・記録・計数を 1 つの transaction に入れる設計の ADR を足す`
- `p0b: cos_digest の apply・記録・script-guard の計数を 1 つの transaction に入れ、compare の影は outbox で記録 1 件にする`
- `p0b: schema 107 を ops-r1 の export の対応版と schema の試験に足す`
- `p0b: 進捗・性能の生記録・apply の計測 example を足す`

file と関数:
- `crates/task-core/src/chat/digest_apply.rs`（新）: `CosDigestApplyPass`・`CosDigestApplyRun`・`CosDigestRecordPlan`（`Append`・`Shadow`）・
  `CosDigestApplyStage`（試験の hook の段）・`CosDigestRunApply`・`CosDigestPassApplied`・`CosDigestApplyError`。
- `crates/task-core/src/store/cos_digest_apply.rs`（新）: `SqliteStore::cos_digest_apply_pass`・`cos_digest_apply_pass_with_hook`
  （`BEGIN IMMEDIATE`、TS の出力なら `read_mode_tx` で照合、run ごとの SAVEPOINT）・`cos_digest_shadow_finish`（outbox の行を消して記録 1 件と計数）・
  `cos_digest_shadow_pending`。
- `crates/task-core/migrations/0107_cos_digest_shadow_outbox.sql`（新）、`store/migrations.rs` の `SCHEMA_VERSION` 107。
- `crates/task-core/src/chat/store.rs`: `daemon_message_add_keyed_tx`（`chat_daemon_message_add_keyed` の本体を呼び手の transaction で書けるようにした）。
  `chat/triage.rs`: `requeue_run_tx`。
- `crates/task-core/src/store/script_guard.rs`: `observe_cos_digest_record_tx`（記録 1 行の計数）。`script_compare_log_append_observed` を消した。
- `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs`: `cos_digest_pass` → `ts_side_digest_pass`（`ModeChanged` なら Rust で apply し直す）・
  `compare_digest_pass`（apply と outbox の後に影を spawn、影は `cos_digest_shadow_finish` で閉じる）・`apply_plan`・`apply_digest_pass`・
  `CosDigestShadows`（この process の影の pass id と再処理の要否。`ShadowTicket` の Drop で再処理を頼む）。
  消したもの: `apply_digest_commands`・`apply_digest_commands_verified`・`append_pass_record`・`pass_records`・`VerifiedApplyError`。
- `crates/task-dispatch/src/dispatcher/cos_chat/triage.rs`: `triage_ingest` が digest pass の前に `cos_digest_shadow_reconcile` を呼ぶ
  （起動後と影の失敗後だけ store を読む）。`dispatcher/script_compare.rs`: `ScriptCompareState::digest_shadows`・`script_compare_invalidate_modes`。
- `crates/task-dispatch/src/script_policy.rs`: `ScriptDomainStore::script_compare_log_append_observed` を消した。
- `crates/campaign/src/ops_r1_trace.rs`: `SUPPORTED_SCHEMA_VERSIONS` に 107（表を 1 つ足しただけで、export が読む表・欄は同じ）。
- 文書: 境界 ADR に付記 P0-B、`docs/architecture-map.md` の TS 制御層の行（file・試験・不変条件・ADR）、`crates/celeris/src/daemon/script_guard.rs` の module 文書。
- 計測: `crates/task-core/examples/cos-digest-apply-bench.rs`（20 run の pass の store 時間。旧い command ごとの transaction と新しい 1 transaction）。

試験:
- 新 `crates/task-dispatch/src/dispatcher/tests/cos_digest_persist.rs`（8 件、名前は `cos_digest_persist_`）:
  - `cos_digest_persist_crash_at_each_stage_rust`・`_compare`・`_ts`: apply の 8 段（transaction の始め、run 2 件 × message・requeue・記録、commit の前）の
    それぞれで process を落とし（hook が panic して commit も tick の残りも走らない。store は開き直す）、何も残らないこと（message 0・requeue 0・記録 0・
    outbox 0・計数不変・item は running のまま）と、再起動後に message が key ごとに 1 件・requeue が item ごとに 1 回・記録が run ごとに 1 件・
    outbox が空・計数が数える記録の数と同じことを確かめる。commit の後に落ちる場面（compare の影は abort）も同じ。
  - `cos_digest_persist_process_abort_mid_transaction`: 子 process を同じ試験 binary で起動し、1 件目の記録の後で `std::process::abort()`（SIGABRT）。
    親が同じ DB file を開いて何も残っていないこと、再起動後に 1 回だけ digest されることを確かめる。
  - `cos_digest_persist_duplicate_pass_delivery`: 同じ pass を 3 回（同じ pass id 2 回、別の id 1 回）apply → message 1・requeue 1・記録 1・計数 +1。
    outbox の行を 2 回書き 2 回閉じる → 記録 1、2 回目の close は何も書かない。
  - `cos_digest_persist_mode_cas_race_applies_rust`: mode の確認と apply の transaction の間に人の CAS（ts→rust）を入れる → TS の command は apply されず、
    Rust の command を fallback `mode_changed` で apply、記録 2・計数 +2。
  - `cos_digest_persist_guard_rollback_with_records`: op の `denied` で全記録が安全違反 → script-guard が同じ transaction で generation を invalid にし
    `rust` へ戻す。commit の前に落とすと mode・generation・記録・計数のどれも変わらず、再起動後に全部が 1 回だけ揃う。
  - `cos_digest_persist_reconcile_skips_live_shadow`: 走っている影の outbox の行は再処理で閉じない。影が自分の記録（`ts_error`）で閉じる。
- 改めた試験（旧い挙動「requeue の失敗の後も message が残る」を確かめていたもの）: `cos_digest_script_retry_after_failed_requeue_write`・
  `cos_digest_script_restart_after_failed_requeue`・`cos_digest_script_ts_retry_requeue_write_failure`・`cos_digest_script_ts_restart_after_failed_requeue`
  （その run は何も残らず、次の pass で message・記録が 1 件）、`cos_digest_script_dispatcher_dropped_mid_compare`（落ちた影は再起動後に `unavailable` の記録 1 件）。
  `crates/celeris/src/daemon/script_guard_stop_tests.rs` の `append` は `cos_digest_apply_pass` で記録と計数を書く。
  `SCHEMA_VERSION` を 107 と書く試験 16 か所、`migration_0042_fills_gaps_in_main_schema_41_database`・`open_rejects_db_with_schema_version_newer_than_supported`（108）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `cargo nextest run -p task-core -p task-dispatch -p celeris -E 'test(/cos_digest_persist_/)'` → exit 0（8 passed、3063 skipped）
- `bash scripts/dev/test-affected.sh` → exit 0（7200 passed、21 skipped。web: tsc -b 通過、vitest 101 files・697 tests passed、node --test 113 pass）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `python3 -I scripts/dev/check-architecture-map.py` → OK（885 件）。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok。`bash scripts/dev/check-doc-links.sh` → exit 0
- 途中の test-affected の失敗（提出前に直した、または範囲外）:
  - 1 回目: 14 件。schema 107 を知らない試験（ops-r1 の export の対応版 `SUPPORTED_SCHEMA_VERSIONS`・schema の試験 2 件）で 13 件、
    `script_generation_e2e_switch_error_rollback_restart`（A の in_flight 2≠1）1 件。前者は直した。後者は単独で再実行して通り、以後の 2 回の全体実行でも通った。
  - 2 回目: `ops_r1_measure_cos_path_with_stub_harness` 1 件（health の status 0≠200）。単独で 3 回再実行して 3 回とも通った。既知の flake
    （fix-script-thread-flake の記録の、空き port の選択と daemon の bind の競合の推定）と同じ症状で、この変更の範囲外。
  - 3 回目: exit 0（上の行）。

## 性能の生記録（同じディレクトリ）
- [p0b-cos-ts.jsonl](p0b-cos-ts.jsonl): `bash scripts/dev/edit-activate-baseline.sh --scenario cos-ts --mode warm --reps 5 --series p0b-cos-digest-persist --out <file>`
  → exit 0。5 試行 7.766・7.335・8.214・11.59・8.161 秒、p50 8.161 秒・p95 11.59 秒（基準 p50≤30・p95≤60）、全試行 `cargo_invocations` 0・`link_count` 0。
  `--check-targets` → exit 0。S6 の 20 試行の p50 7.470 秒より 0.69 秒長いが、5 試行で数が足りない（summary は「5 trials < 20」）。
  この変更は Rust 側だけで、ts-verify の段（tsc・bundle・単体試験・golden・runner）は変えていない。
- [p0b-apply-bench.jsonl](p0b-apply-bench.jsonl): `cargo run -p task-core --example cos-digest-apply-bench -- $TMPDIR/bench 10` → exit 0。
  20 run の pass の store 時間（file DB）。旧い command ごとの transaction（`per_command`）は 10 回で p50 0.0208 秒・最大 0.1413 秒、
  新しい 1 transaction（`one_transaction`）は p50 0.0188 秒・最大 0.0907 秒。tick の中の digest の書き込みは遅くなっていない。

## 未解決
1. 本番に、0107 より前の窓で「digest の message はあるが item が `running` のまま」の run が残っているかは未確認。確かめ方（読み取り専用）は ADR の「本番への移行」。
   この変更は既存の行を直さない。
2. 旧 daemon（draining）と新 daemon が重なる間、旧 daemon の影がまだ走っていても、新 daemon は起動後の再処理でその outbox の行を `unavailable` で閉じる。
   記録は 1 件のまま（旧の影の close は何も書かない）だが、その run の TS の比較結果は残らない。
3. 記録を組み立てられない run（`serde_json` の失敗。今の型では起きない）は apply せずに warn を出し、次の pass で同じ run をもう一度試す。
   同じ失敗が続くとその run は digest されない（`digest_due` は下りない）。
4. migration の版 0107 は、並行する子 task（p0a・p1c）が migration を足すと衝突する。統合で番号を振り直す。
5. `ops_r1_measure_cos_path_with_stub_harness` の port の競合（既知の flake）はこの task では直していない。

## 次の gate
- 親の p1d（p0b の後）: この原子化を前提に、`cos_digest` の canary/ts の本番昇格の手順を検討する。昇格は人の API token だけで、この task は本番・release・mode に触れていない。
- release gate（`scripts/selfdeploy/release.sh`）の全体試験で、migration 0107 と `cos_digest_persist_` の試験を通す。

## 本番への移行
- migration 0107 は表を 1 つ足すだけ。本番は全領域 `rust` で、`rust` の pass は outbox も記録も書かない。config の変更は無い。

## 提案
- 無し。
