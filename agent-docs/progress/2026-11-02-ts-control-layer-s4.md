---
title: S4 — CoS の digest pass（領域 cos_digest）の TS 同等実装と、記録済み trace の replay による検証
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S4 — CoS の digest pass の TS 同等移行

完了日: 2026-11-02（task 01M6EVP9GT07K2G01EMW0E6HNC。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- 受信箱一次対応の digest pass（`triage_digest`→`digest_one`）の判定を領域 `cos_digest` として TS に同等実装した。判定は文面・cards の組み立てと、stopped・interrupted の run を requeue するかの判断である。
- fixture（cases 2,066 行・traces 17 行）の全 2,083 行で、TS の command が Rust と byte 一致した。本物の deno_core host でも一致した。
- TS の command は S4 では apply しない。mode `compare` でも apply するのは Rust の command で、TS の結果は表 `script_compare_log` に記録するだけである。
- 本番の mode は `rust` のままで、本番 config は書き換えていない。本番で有効にするのは人の手順で、「本番への移行」に書いた。
- 方針の変更は入れていない。走査・20 件の上限・message の書き込み・requeue の SQL・`digest_due`・`dirty` は Rust のままである。

## 移した経路と理由
起点は triage run の終了で、次の順に流れる（付記 S4-1・S4-2）。

1. Rust が `cos_triage_runs_without_digest` で未 digest の run を読み、snapshot（`CosDigestInput`）を作る。
2. 判定が command（`post_digest`・`requeue_run`）を返す。TS は `control-ts/src/domains/cos_digest.ts`、Rust の参照実装は `crates/task-core/src/chat/digest.rs` の `cos_digest_decide`。
3. Rust が `validate_cos_digest_commands` で検証する。
4. Rust が apply する（`apply_digest_commands`）。

選んだ理由は 4 つある。

- 境界が既にあり、前段の Rust の変更は挙動を変えない分割で済む。
- run の終了・tick をまたぐ再配送・書き込みの retry・人の割り込み・再起動後の回収という実運用の非同期の流れを全部通る。
- 認可・予算・human_required・`policy_version` の検査の外にある。
- 変更頻度が文面に偏る。

S0 の計画は文面だけを移す案だった。「単なる rule 関数」にしないため、requeue の判断も command に含めた。
LLM は digest 経路に無い。記録済みの LLM 応答は、triage run の結果が `cos_inbox_items` に入った形で trace に含まれ、live で LLM を呼ばない（付記 S4-6）。

## 受け入れ条件ごとの証拠
下の結果は close unit が提出時の HEAD で実行したもの（基準は `$CELERIS_WU_BASE`）。各 unit が実行した結果は unit の記録にある。

### 0. 実運用相当の非同期 CoS 経路が TS で動く
- 本物の host の digest pass: `crates/celeris/tests/script_compare_cos_digest.rs` の `script_compare_cos_digest_dispatch_match_replay_and_rollback`。mode `compare` の tick で run ごとに `match` が 1 行記録され、message と item の行は mode `rust` の基準と byte 一致する。
- daemon の経路: `script_compare_cos_digest_daemon_restart_without_bundle_restores_rust`。
- 手順: `cd control-ts && corepack pnpm@12.6.0 install --offline --frozen-lockfile && corepack pnpm@12.6.0 run bundle && cd .. && CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_compare_cos_digest` → exit 0、5 passed・0 skipped。
- bundle の manifest の domains: `cos_digest`・`host_selftest`・`lane_rules`・`route_priority`。

### 1. fixture と trace の replay が Rust と一致し、安全回帰が無い
**fixture の全件一致**
- `node control-ts/scripts/compare.mjs cos_digest` → `cos_digest: 2083/2083 matched (fixtures/cos_digest/cases.jsonl, fixtures/cos_digest/traces.jsonl)`、exit 0。全行を正順・逆順の 2 回流す。
- cases 2,066 行（手書き 66・固定 seed の生成 2,000）は `script_fixtures_cos_digest_match_committed_fixture` が Rust の出力と byte 一致を確かめる。
- traces 17 行（16 scenario）は本物の dispatcher の経路で run を終えて書き出した。`cos_digest_trace_matches_committed_fixture` が再生成との byte 一致を、同じ試験の中の照合が commands と実際に書かれた message・requeue の一致を確かめる。照合が食い違いを検出することは `cos_digest_trace_verify_rejects_commands_that_differ_from_the_writes` で確かめた。

**本物の host の replay**
- `script_compare_cos_digest_real_host_matches_rust_on_all_fixtures`: 全行を同じ isolate に正順・逆順で通して Rust と byte 一致。isolate の作り直しは 0 回。
- `script_compare_cos_digest_dispatch_match_replay_and_rollback`: 記録の `thread_id`・`run_id` から入力を組み直し、`input_digest` が合う run を TS に通すと `commands_digest` が記録と同じになる。requeue した run は item が run から外れるので `drifted` に数える（付記 S4-10）。

**安全回帰 0: 検証落ちを `safety_violations` に数え、Rust の結果を変えない**
- 同じ場面の mode `rust` の基準と message・item の行が byte 一致することも確かめる試験（task-dispatch）:
  - `script_compare_cos_digest_rejects_card_of_another_item` → `invalid_output`、`["card_outside_items"]`
  - `script_compare_cos_digest_rejects_other_idempotency_key` → `invalid_output`、`["key_mismatch"]`
  - `script_compare_cos_digest_rejects_requeue_of_completed_run` → `invalid_output`、`["requeue_state_invalid"]`
- 検証関数が全規則の違反を拒む（14 通り、`scope_mismatch`・`duplicate_command` を含む）: `script_fixtures_cos_digest_validation_rejects_each_rule`。未知の欄の拒否: `script_fixtures_cos_digest_dtos_deny_unknown_fields`。
- TS が失敗しても Rust の結果が変わらない:
  - task-dispatch: `script_compare_cos_digest_mismatch_keeps_rust_text`・`script_compare_cos_digest_error_is_ts_error`・`script_compare_cos_digest_timeout_is_ts_error`・`script_compare_cos_digest_malformed_output_is_invalid_output`
  - 本物の host: `script_compare_cos_digest_different_answer_keeps_rust_digest`・`script_compare_cos_digest_throwing_bundle_keeps_rust_digest`
- mode `rust` では TS を呼ばない: `script_compare_cos_digest_mode_rust_never_calls_ts`。bundle が食い違えば呼ばず `unavailable` を記録する: `script_compare_cos_digest_without_runtime_or_with_other_bundle_is_unavailable`。
- 既存の安全検査は移さず、緩めていない（S4 の `crates/` の差分から試験を除いた行に `plan_conforms`・`job_wait_extend`・`human_required`・`policy_version` は 0 件）。`cos_triage_requeue_run` の `WHERE state='running'`、message の冪等 key、`digest_due` を空の走査で初めて下ろす規則は Rust のままである。plan_conforms・job_wait_extend の basis 検査、human_required、版整合の拒否は digest の外にあり、S4 の差分は触れていない。
- 実行: `cargo nextest run -p task-dispatch -E 'test(/cos_digest_script|script_compare_cos_digest|cos_digest_trace/) | test(/cos_chat_triage::/)'` → exit 0、78 passed。`cargo nextest run -p task-core -E 'test(/script_fixtures|script_compare_log/)'` → exit 0、13 passed。

### 2. event 順序・再配送・retry・cancel・再起動・policy_version/session freshness の試験
すべて `crates/task-dispatch/src/dispatcher/tests/cos_digest_script.rs`。全件で共通の不変条件を確かめる（digest message は key ごとに 1 件、requeue は item ごとに 1 回まで、記録は run ごとに 1 件まで）。上の実行に含まれ、合格した（11 件）。

| 観点 | 試験 |
|---|---|
| 順序 | `cos_digest_script_order_item_update_before_and_after_run_end` |
| 再配送 | `cos_digest_script_redelivery_same_run_twice` |
| retry | `cos_digest_script_retry_after_failed_message_write`・`cos_digest_script_retry_after_failed_read`・`cos_digest_script_retry_after_failed_requeue_write` |
| cancel | `cos_digest_script_ts_cancelled_at_deadline`・`cos_digest_script_dispatcher_dropped_mid_compare` |
| 再起動 | `cos_digest_script_restart_same_db`・`cos_digest_script_restart_after_failed_requeue`。本物の daemon は `script_compare_cos_digest_daemon_restart_without_bundle_restores_rust` |
| policy_version・session freshness | `cos_digest_script_policy_version_change_and_fresh_session` |
| mode と generation の切替 | `cos_digest_script_mode_back_to_rust_and_generation_change_mid_compare` |

既存の試験も無修正で通る。人の割り込みは `cos_chat_triage_human_interrupt_requeues_items_and_runs_the_human_message_first`、20 件超の drain は `cos_chat_triage_digest_restart_drains_more_than_twenty_runs_on_normal_ticks`。

### 3. 変更の影響範囲の test が通る
unit の結果（提出時の HEAD はそれぞれの unit の HEAD）:
- rust-split: `bash scripts/dev/test-affected.sh` exit 0（nextest 6,980 passed・21 skipped、web 697 passed、control-ts 48 passed）
- hook: exit 0（nextest 6,991 passed・21 skipped、web 697 passed）
- trace-capture: exit 0（nextest 2,993 passed・9 skipped、control-ts 48 passed）
- ts-digest: exit 0（control-ts の typecheck と test だけ）
- host-adapter: `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で exit 0（nextest 875 passed・0 skipped）
- robustness: exit 0（nextest 3,014 passed・9 skipped）
- 各 unit の `cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` は exit 0。

統合の段（integrate-design・core・impl・verify）は決定的な check を通った。

close unit の結果（提出時の HEAD）: `corepack pnpm@12.6.0 run typecheck` exit 0、`corepack pnpm@12.6.0 run test` は vitest 81 passed・node --test 34 passed。close unit の差分は文書だけ（この file・`docs/architecture-map.md`・境界 ADR）で、基準は `$CELERIS_WU_BASE`:
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。crate の試験は走らない）
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-doc-links.sh` → ok、`bash scripts/dev/check-adr-numbers.sh` → ok（356 files）、`sh scripts/dev/progress-index.sh --check` → ok、`python3 scripts/dev/check-architecture-map.py` → OK（846 件）

task 全体の変更に対する test-affected は、close unit では走らせていない。上の unit の結果と統合の段の check を根拠にする。

## unit と commit（件名）
| unit | 内容 | commit の件名 |
|---|---|---|
| [adr-s4](2026-11-02-ts-control-layer-s4/adr-s4.md) | 境界 ADR の付記 S4-1〜S4-9 | `phase s4: adr-s4: 境界 ADR に S4 の CoS digest 経路の付記、進捗を書く` |
| [rust-split](2026-11-02-ts-control-layer-s4/rust-split.md) | digest の 4 分割（挙動不変）と cases fixture | `phase s4: rust-split: CosTriageItemReport と入れ子の型に Serialize・Deserialize・JsonSchema・deny_unknown_fields を付ける`、`phase s4: rust-split: digest を snapshot→decide→validate→apply に分け、cos_digest の比較 fixture を足す` |
| [hook](2026-11-02-ts-control-layer-s4/hook.md) | 領域の登録、migration 0104、compare の結線 | `phase s4: hook: cos_digest 領域・script_compare_log・digest pass の compare を結線する` |
| [trace-capture](2026-11-02-ts-control-layer-s4/trace-capture.md) | traces fixture | `phase s4: trace-capture: digest pass の記録済み trace を dispatcher の経路から書き出す試験と traces.jsonl を足す` |
| [ts-digest](2026-11-02-ts-control-layer-s4/ts-digest.md) | TS 実装・単体試験・compare の dir 読み | `phase s4: ts-digest: cos_digest 領域の TS 実装・単体試験・compare の dir 読み` |
| [host-adapter](2026-11-02-ts-control-layer-s4/host-adapter.md) | 本物の host の試験 | `phase s4: host-adapter: 本物の deno_core host で cos_digest の replay と compare を確かめる試験を足す`、`phase s4: host-adapter: 進捗の記録を足す` |
| [robustness](2026-11-02-ts-control-layer-s4/robustness.md) | 順序・再配送・retry・cancel・再起動・版変化の試験 | `phase s4: robustness: digest pass の順序・再配送・retry・cancel・再起動・版変化の試験を足す` |
| close | この file・地図・境界 ADR の S4-10 | `phase s4: close: S4 の進捗まとめ・architecture-map・境界 ADR の実装との突き合わせ` |

統合の commit（`integrate wu/…`）は celeris が作った。

## 未解決
1. 本番の log の replay の道具は無い。replay は試験（`script_compare_cos_digest_dispatch_match_replay_and_rollback`）の中にだけある。付記 S4-5 の `drifted` の数え方も試験の中だけである。
2. 本番の log の replay で確かめられるのは requeue の無い run だけである。stopped・interrupted の run は requeue で item の `run_id` が消え、組み直した入力の `input_digest` が必ず合わない。requeue の判断の同等性は fixture で確かめた。
3. completed の run に `running` のまま残った item が退避された場合に replay が drifted になるかは未確認（host-adapter）。
4. 1 回の pass で in-flight の上限（全領域の合計 32）を超えた run の記録は落ち、warn の log だけが残る（付記 S4-4）。S5 の分母として足りるかは S5 で見る。この場面の試験は無い。
5. `release.sh`・`test-affected.sh` は `control-ts/dist` を作らない。dist が無いか古い環境では、本物の host の試験のうち bundle を使う 3 件が SKIP で通る（S2 の未解決 2 と同じ）。
6. 検証落ちの code のうち `scope_mismatch`・`duplicate_command` は検証関数の試験だけで確かめ、dispatcher の compare の経路では確かめていない。`generation_gone` も digest の場面の試験は無い（`ts_error` の分類は `script_compare_cos_digest_error_is_ts_error` で確かめた）。
7. 本物の daemon の試験（再起動）は、遅い host の 50 ms の `timeout` も許す。一致は別の試験で確かめている（host-adapter）。
8. `cargo clippy -p celeris --tests -- -D warnings` は、S4 が触っていない `campaign_lock_e2e` の compile error で止まる。必須の検査は `--tests` なしで通る（S2 の未解決 8 と同じ）。
9. TS の入力の型は手書きで、Rust の DTO から生成していない。`docs/api/script-host/host-v1.schema.json` にも `cos_digest` の DTO を足していない（S2 の領域と同じ扱い）。

## 提案（別 task の候補）
**次に移す候補**
1. B 定期見直し（`cos_review` cron）の入力文。先に `review_text` から冪等 key の確認と `project_brief_list` の読み出しを外へ出す Rust の変更が要る（付記 S4-1）。
2. C 受信箱の取り込み分類。変更頻度は最大だが、cursor・挿入 transaction・`policy_version` に近く、入力（`EventRow`・`InboxItem`・`Notice`）も大きい。先に入力の直列化を別の差分で入れる。

どちらも最初に op を使うなら、その前に S2-7 の要求ごとの V8 context の分離を決める（付記 S4-6）。

**見つけた bug（どれも直していない。言語移行と別の差分にする）**

3. requeue の書き込みが message の後で失敗すると、requeue は再試行されない。run が digest 済みになるためである。item は同じ pass の `triage_fallback` が人へ渡すので、二重の requeue も取り残しも出ないが、「次の CoS run で扱い直す」機会を失い、compare の記録も作られない。task-core の store に、message の書き込みと requeue を 1 つの transaction で行う API を足す（robustness）。
4. `digest_snapshot` は `chat_run_get` の失敗を `ok()` で捨て、reason 無しで digest を書く。message は冪等 key で固定されるので、一時的な読み出しの失敗で reason が恒久的に抜ける。NotFound 以外の失敗は Err にして次の tick に回す（robustness）。

**検証の補強**

5. release gate と統合の check で `control-ts` の bundle を作り、`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で本物の host の試験を走らせる（S2 の提案 1 と同じ。S4 の 3 件が増えた）。
6. `script_compare_log` を読む API か `celerisctl` の command を足す。今は sqlite3 で本番 DB を読み取り専用で開くしかない。あわせて本番の log の replay の道具を足すかを決める（未解決 1）。
7. S5 の `ts` mode で TS の command を apply するときの記録欄を決める（付記 S4-7）。

## 本番への移行
S4 は本番 config を書き換えていない。本番の `cos_digest` は `rust` のままで、migration の後も TS を呼ばない。
compare を本番で有効にするのは人の手順で、既存の認可手順（release → verify → 人の承認で promote）に従う。
**IEEE Access・K2・K4 の凍結環境と測定 release には入れない。** 測定の daemon では `[script]` を書かず、mode を変えない（境界 ADR (g)）。

1. migration 0104 を含む release を作る
   - `scripts/selfdeploy/release.sh <S4 を含む main の commit>`。migration 0104（`script_domain_modes` の `cos_digest` 行、表 `script_compare_log`）は promote 後の起動で当たる。S2 の 0103 が未適用なら一緒に当たる。
   - S1 の「本番への移行」の V8 archive の確認（`RUSTY_V8_ARCHIVE`）が済んでいること。
   - 既存の手順で verify し、人が承認して promote する。
2. bundle を作り、state_dir に配備する（release と同じ commit の checkout で）
   - `cd control-ts && corepack pnpm@12.6.0 install --offline --frozen-lockfile && corepack pnpm@12.6.0 run bundle`
   - `node scripts/compare.mjs cos_digest` が `2083/2083 matched` で exit 0 であることを確かめる。
   - `cd .. && cargo run --release -p celeris-script --bin script-deploy -- --dist control-ts/dist --state-dir <[script] state_dir>`
     → stdout の 1 行が `bundle_sha256`。`<state_dir>/script-bundles/<bundle_sha256>/` ができる。
3. config の `[script]` で active bundle を指す
   ```toml
   [script]
   state_dir = "<state_dir>"
   active_bundle_sha256 = "<2. の bundle_sha256>"
   ```
   `[script]` は起動時に読むので、daemon を既存の手順で再起動する。起動の log と通知に `script_bundle_rejected` が無いことを確かめる。
   - 注意: 既に `lane_rules`・`route_priority` を古い bundle で compare にしていると、bundle の hash が変わるので起動時に `rust` へ戻る（`system:script-startup`）。続けるなら 4. と同じ手順で新しい `bundle_sha256` を指して compare に戻す。
4. mode を compare にする
   - `celerisctl script domains` で `cos_digest` の `version` を読む（初めてなら 0）。
   - `celerisctl script mode cos_digest compare --expected-version <version> --generation <bundle_sha256> --mode-reason "S4 compare を本番で有効にする"`
5. 確かめ方
   - `celerisctl script domains` で `cos_digest` の mode が `compare`、`bundle_sha256` が 2. の値。
   - 記録は event `script_compare_recorded` ではなく表 `script_compare_log` にある。欄は同じである（付記 S4-5）。本番 DB を読み取り専用で開き、件数と outcome を数える:
     `sqlite3 -readonly <本番 DB> "SELECT json_extract(json,'$.outcome'), json_extract(json,'$.error.kind'), count(*) FROM script_compare_log WHERE domain='cos_digest' GROUP BY 1,2"`
   - 安全違反を数える:
     `sqlite3 -readonly <本番 DB> "SELECT count(*) FROM script_compare_log WHERE domain='cos_digest' AND json_extract(json,'$.safety_violations') != '[]'"`
   - 期待: 受信箱の triage run が終わって digest が書かれるたびに件数が増え（run ごとに 1 行）、outcome は `match` だけ、安全違反は 0 件。
   - `mismatch`・`invalid_output` が 1 件でもあれば、または安全違反が 1 件でもあれば、6. で戻して記録を別 task にする。
   - `unavailable` は bundle と generation の食い違いを示す。`ts_error` の `error.kind` が `timeout` なら 50 ms の deadline を超えた記録で、件数が多ければ戻して調べる。
   - digest の message 自体は compare の有無で変わらない。記録の欠けがあるかは、`chat_messages` の `client_message_id LIKE 'cos-triage-digest:%'` の件数の増えと比べる（in-flight の上限を超えた run と apply に失敗した run は記録されない）。
6. 戻し方
   - `celerisctl script mode cos_digest rust --expected-version <現在の version>`。戻した後は TS を呼ばない。digest は Rust の結果のまま書かれ続ける。
   - `[script]` を消して再起動しても、daemon が起動時に `rust` へ戻す（`system:script-startup`）。
   - mode の変更の履歴は `script_domain_events`、compare の記録は `script_compare_log` に残る（どちらも追記専用で、消さない）。
