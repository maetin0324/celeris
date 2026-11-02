---
title: S4 hook — cos_digest 領域の登録と digest pass への compare の結線
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 hook — cos_digest 領域の登録と digest pass への compare の結線

完了日: 2026-11-02（unit hook）。境界 ADR 付記 S4-2〜S4-5 に従う。apply するのは常に Rust の command で、挙動は変えていない。

## 結論
- 領域: `ScriptDomain::CosDigest`（`"cos_digest"`、`crates/task-core/src/script_compare.rs`）。migration 0104（`0104_script_cos_digest.sql`、`SCHEMA_VERSION` 104）が次の 2 つを足す。
  - `script_domain_modes` の `cos_digest` 行（mode `rust`、version 0）
  - 追記専用の表 `script_compare_log`（UPDATE・DELETE は trigger が拒む。付記 S4-5）
- 記録の型: `CosDigestCompareRecord`（`script_compare/cos_digest_record.rs`）。欄は S2-4 の `script_compare_recorded` と同じで、次が違う。
  - `input` は常に `null`
  - `rust`・`ts` は `commands_digest` と `command_kinds` だけを持つ
  - `mismatch` のときだけ `ts_commands_head`（先頭 512 byte）を足す
  - `applied` に `message_id`・`requeued` を持つ
- store: `script_compare_log_append` は、同じ thread・run・generation・input_digest・outcome の行があれば足さない。読み出しは `script_compare_log`。
- `PolicyScript::cos_digest(input, deadline)`（`crates/task-dispatch/src/script_policy.rs`）を足した。`ScriptPolicy`（`crates/celeris/src/daemon/script_policy.rs`）は `ScriptRuntimeHandle` で領域 `cos_digest` を呼ぶ。scope は空で、既定実装は無い。
- 分類: `classify_cos_digest` は次の順で決める。
  1. TS の出力を `CosDigestOutput { commands }`（`deny_unknown_fields`）に直す。直せなければ `invalid_output`。
  2. `validate_cos_digest_commands` に通す。落ちたら `invalid_output` にし、S4-3 の code を `safety_violations` に入れる。
  3. 通れば command 列の byte 一致で `match`／`mismatch` を決める。
  - error と deadline は S2 と同じ分類（`ts_error`）。
- 結線:
  - `digest_one` は apply 後に `CosDigestApplication`（入力・Rust の command・apply 結果）を返す。
  - `triage_ingest` はそれを集めて返す。
  - `tick_cos_chat_launch` は `Dispatcher::compare_cos_digest` に渡す。
  - `compare_cos_digest` が TS を呼ぶのは、mode が `compare` で、generation が `loaded` かつ `cos_digest` を宣言し、bundle が一致し、tokio runtime があるときだけ。別の tokio task で `SCRIPT_CALL_DEADLINE` 付きで呼ぶ。
  - mode `rust` では何もしない。PolicyScript が無いときと bundle が食い違うときは TS を呼ばず、`unavailable` を記録する。
  - S2 の `compare_domain` は、generation の確認を `compare_target` に、in-flight 上限の管理を `spawn_compare` に切り出した。両方を全領域で共用する（in-flight の上限は合計 32）。

## 受け入れ条件と証拠（提出時の HEAD で実行）
- 条件 0（mode compare で記録が残り、message と requeue は Rust の結果のまま）: `cargo nextest run -p task-dispatch -E 'test(/script_compare_cos_digest/)'` → 10 passed。
  - 各試験は completed と stopped の run を 1 つずつ置き、mode rust・PolicyScript 無しの基準と比べる。inbox thread の assistant message（key・text・cards・run_id）と `cos_inbox_items`（source_key・state・run_id・reason）の直列化が byte 一致することを確かめる。
  - 場面: 一致・不一致（`ts_commands_head`）・error・timeout（`start_paused`）・不正出力・検証落ち 3 種（他人の card id → `card_outside_items`、違う key → `key_mismatch`、completed での requeue → `requeue_state_invalid`）。
  - 記録が `script_compare_log` に run ごと 1 行あること、`applied.requeued`（stopped の run で 1）が入ることを確かめる。2 回目の digest pass（再配送）で message も記録も増えないことも確かめる。
- 条件 1（mode rust では TS を呼ばない）: `script_compare_cos_digest_mode_rust_never_calls_ts` で呼び出しは 0 回、記録も 0 件。`script_compare_cos_digest_without_runtime_or_with_other_bundle_is_unavailable` では bundle の食い違いで呼び出し 0 回、記録は `unavailable`。
- store の試験: `script_compare_log_appends_once_per_outcome_and_is_append_only`（task-core）。
- 条件 2:
  - `bash scripts/dev/test-affected.sh` → exit 0（nextest 6,991 passed・21 skipped、web 697 passed）
  - `cargo clippy --workspace -- -D warnings` → exit 0
  - `cargo fmt --all -- --check` → exit 0

## 追従した既存の試験・生成物
- `SCHEMA_VERSION` 103→104 を固定した試験 10 件。migration 0042 試験の版の列、`open_rejects_db_with_schema_version_newer_than_supported`（105 に）。
- `crates/campaign/src/ops_r1_trace.rs` の `SUPPORTED_SCHEMA_VERSIONS` に 104 を足した。0104 は export が読む表・欄を変えない。`crates/celeris/tests/ops_r1_trace.rs` は `task_core::SCHEMA_VERSION` を参照するので変更不要。
- 初期 mode の試験（task-core `script_domain_initial_modes_are_rust`、task-api `tests/script_domains.rs` の領域一覧）に `cos_digest` を足した。
- 生成物の再生成: `docs/api/v1/api-v1.schema.json`・`event.schema.json`（`UPDATE_SCHEMA=1`）、`web/api/generated/`（`node web/scripts/gen-types.mjs`）。
- `docs/api/v1/api.md` §3.134 と celerisctl の `script mode` の help に `cos_digest` を足した。
- CoS operations の試験は変更不要だった（test-affected で通過）。

## commit（件名）
- phase s4: hook: cos_digest 領域・script_compare_log・digest pass の compare を結線する

## 付記との食い違い
- 目的文の「script_compare_recorded を残す」は、付記 S4-5 のとおり `script_compare_recorded` と同じ欄の記録を表 `script_compare_log` に残す形で満たした。`events.task_id` は NOT NULL で、digest は task に属さない。event の variant は足していない（S4-7）。
- `docs/api/script-host/host-v1.schema.json` には DTO を足していない。S2 の `lane_rules`・`route_priority` の DTO もこの schema に無く、host の DTO（呼び出しの包み）だけを載せる方針と揃えた。
- 出力の包み `CosDigestOutput` は task-dispatch に置いた。ts-digest unit と同じ file を触らないためで、同じ形（`{commands: [...]}`）を TS 側が返す前提。
- 記録は apply が成功した run だけに作る。投入に失敗した run は次の tick で再試行され、その時に比べる。

## 未解決事項
- なし（この unit の範囲）。

## 提案
- close unit で、付記 S4-5 に `applied.message_id` が冪等 key で既にあった message の id にもなることと、記録が apply 成功時だけであることを書き足す。
