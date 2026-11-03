---
title: S4 — CoS の digest pass（領域 cos_digest）を TS の async 経路に移し、記録済み trace の replay で検証する
tasks: [01M6EVP9GT07K2G01EMW0E6HNC]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S4 — CoS の digest pass の TS 同等移行（async 経路）

完了日: 2026-11-03（task 01M6EVP9GT07K2G01EMW0E6HNC。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- 受信箱一次対応の digest pass 全体を、領域 `cos_digest` の TS の async 経路にした。TS の `cosDigestPass(ctx)`（`control-ts/src/domains/cos_digest.ts`）が Host API の op `cos_digest:list_runs` を await して未 digest の run を読み、run ごとに `cos_digest:read_run` を await し、run ごとの command（`post_digest`・`requeue_run`）を返す。
- mode `ts`（`cos_digest` だけ）では、TS の command を Rust が規則 1〜9 で検証し、通れば Rust の apply に渡す。TS の失敗・timeout・検証落ち・apply 直前の mode の変化では、pass 全体を Rust の command で apply する（fallback）。DB を書くのは Rust の apply だけである。
- mode `compare` は Rust の command を apply した後、凍結した読み出し（memo）から op に答えて TS を影で走らせ、記録だけを残す。
- `→ts` は人の actor だけが、同じ generation の `compare` からだけ行える。本番の mode は `rust` のままで、本番 config は書き換えていない。本番で compare・ts にするのは人の手順で、「本番への移行」に書いた。
- 前回の final review の不合格（criterion 0。TS に移ったのが同期の `decide()` だけで、TS の command がどの mode でも apply されない）は、境界 ADR の付記 S4-11〜S4-19 で仕様を決め、unit host-ops・mode-ts・pass-wire・ts-pass・traces-async・real-host・robust-ts・fixture-ref で実装した。実装との突き合わせは付記 S4-20 に書いた。
- 方針の変更は入れていない。`digest_due`・`dirty`・pass の起点・20 件の上限・message の冪等 key・requeue の `WHERE state='running'` の SQL は Rust のままである。

## 移した経路
起点は triage run の終了で、1 tick の digest pass は次の順に流れる（付記 S4-12・S4-14）。

1. Rust が pass の始めに `script_domain_modes` から mode を 1 回読み、pass ごとの memo（`CosDigestPassReads`）を作る。
2. mode `ts`: dispatcher が TS の `cosDigestPass` を呼び、200 ms（`COS_DIGEST_PASS_DEADLINE`）を上限に待つ。TS の op は host sink（容量 64 の bounded mpsc、応答は oneshot）を通って dispatcher の op server に届き、op server が memo を通して store を読む（list は 1 回、run ごとの read は 1 回まで）。store は V8 の thread に渡らない。
3. Rust の参照実装 `cos_digest_pass_decide` が同じ memo で出力を作る。TS が読んでいない run だけを Rust が読み足す。
4. Rust が TS の出力を検証する（run ごとの規則 1〜6 `validate_cos_digest_commands`、pass の規則 7〜9 `validate_cos_digest_pass`）。apply の直前に mode の行を読み直す。
5. Rust が apply する（run ごとに `post_digest`→`requeue_run`）。どちらの command を apply したかを `script_compare_log` の `applied_by` に残す。

scope: `read_run` は、その pass の `list_runs` の答えにある `(thread, run_id)` だけを読める。それ以外は `denied`（安全違反 `denied_scope`）。capability は manifest の宣言と許可表 `DOMAIN_CAPABILITIES` の共通部分だけが有効で、op は `cos_digest` の生きた要求の token でだけ通る（付記 S4-13）。

LLM は digest 経路に無い。CoS の判断（LLM の応答）は `cos_inbox_items` の `decision` として記録済みで、`read_run` の応答に入っている。replay は記録済みの応答を使い、live で LLM を呼ばない（付記 S4-16）。

## 受け入れ条件ごとの証拠
下の結果は close-async unit が提出時の HEAD で実行したもの（基準は `$CELERIS_WU_BASE`）。各 unit が実行した結果は unit の記録にある。

### 0. 実運用相当の非同期 CoS 経路が TS で動く（TS の async pass が op で読み、mode ts で TS の command が Rust の検証→apply に渡る）
- 本物の deno_core host と本物の bundle で、dispatcher の digest pass を mode `ts`・`compare` で走らせる（`crates/celeris/tests/script_compare_cos_digest.rs`）:
  - `cd control-ts && corepack pnpm@12.6.0 install --offline --frozen-lockfile && corepack pnpm@12.6.0 run bundle` → exit 0
  - `CELERIS_SCRIPT_REQUIRE_BUNDLE=1 cargo nextest run -p celeris --test script_compare_cos_digest` → exit 0、9 passed・0 skipped
- この 9 件のうち async 経路の 4 件:
  - `script_compare_cos_digest_ts_mode_applies_ts_commands`: mode `ts` の tick で、記録が run ごとに 1 件、`mode = ts`・`applied_by = ts`・`fallback = null`・`match`。op の log は `list_runs`・`read_run` が `ok`（store から読んだ）。message・item の行は mode `rust` の基準と byte 一致。TS の文面に印 `(ts-applied)` を足した bundle では、DB の message の本文が印で終わる（TS の command が apply された証拠）。
  - `script_compare_cos_digest_compare_answers_ops_from_rust_snapshot`: mode `compare` で、apply の後に表の名前を変えても TS の op は memo から答え、`match`・`applied_by = rust`。
  - `script_compare_cos_digest_ts_mode_falls_back_to_rust`: 違う答え・throw・op の `denied`・deadline 超えの 4 種で、message・item は基準と byte 一致、`applied_by = rust`、`fallback.code` は `run_missing`・`ts_error`・`ts_error`（安全違反 `denied_scope`）・`ts_error`（`timeout`）。
  - `script_compare_cos_digest_daemon_ts_restart_keeps_one_message`: 下の条件 2。
- dispatcher の経路（偽の TS が本物と同じ手順で op を await する）: `crates/task-dispatch/src/dispatcher/tests/cos_digest_pass.rs` の 8 件（`cos_digest_pass_ts_applies_validated_ts_commands`・`cos_digest_pass_ts_falls_back_on_error_timeout_invalid`・`cos_digest_pass_ts_rejects_run_outside_list`・`cos_digest_pass_ts_mode_changed_before_apply_uses_rust` ほか）。下の実行に含む。
- op の層（`crates/script-runtime/tests/cos_digest_ops.rs` 8 件、`tests/dto.rs`）: `cargo nextest run -p celeris-script --test cos_digest_ops --test dto` → exit 0、15 passed。manifest の宣言が無ければ `denied`、他の領域の要求からは `denied`、要求の終わりの後は `terminated`、deadline で `timeout`、作り直しで `generation_gone`、65 回目は `busy`、capability 付きの要求は isolate ごとに 1 件ずつ。
- mode の権限（`crates/task-api/tests/script_domains.rs`）: `cargo nextest run -p task-api --test script_domains -E 'test(/script_domain/)'` → exit 0、10 passed。`ts` は `cos_digest` だけ、`→ts` は人の actor だけ（operator・CoS・`system:*` は 403 `script_mode_requires_human`）、元は同じ generation の `compare` だけ（422 `script_mode_ts_requires_compare`）、戻しは誰でも、`canary` は今までどおり 422。CoS registry の試験: `cargo nextest run -p task-api -E 'test(/cos_ops_registry|cos_operator_skill/)'` → exit 0、6 passed。
- TS の単体: `corepack pnpm@12.6.0 -C control-ts run typecheck` → exit 0。`corepack pnpm@12.6.0 -C control-ts run test` → vitest 4 files・100 passed、node --test 41 passed・0 failed。`cos_digest.test.ts` の `cosDigestPass reads list then each listed run`・`never reads a run outside the list`・`skips a run whose read_run throws exception` を含む。
- bundle の manifest の `capabilities`: `cos_digest:list_runs`・`cos_digest:read_run`・`selftest:echo_task`（`control-ts/bundle.config.json` の `cos_digest` が 2 つを宣言）。

### 1. fixture と pass trace の replay が Rust と一致し、安全回帰が無い
**replay の全件一致**
- `node control-ts/scripts/compare.mjs cos_digest` → `cos_digest: 2086/2086 matched (fixtures/cos_digest/cases.jsonl, fixtures/cos_digest/pass_traces.jsonl)`、exit 0。fake host（`control-ts/src/testing/pass-replay.ts`）が trace の `ops` から op に答え、本物の入口 `cosDigestPass` を replay する。全行を正順・逆順の 2 回流す。
  - `pass_traces.jsonl` 20 行（1 行 1 pass）: 本物の dispatcher の経路で triage run を FakeAdapter で終え、mode `rust` と mode `ts` で同じ場面を走らせて書き出した。場面は completed 7・failed 3・stopped 2（2 件 requeue を含む）・interrupted 3・2 thread・`chat_run_get` の失敗（`reason: null`）・23 run の drain（2 pass）。`cos_digest_pass_trace_fixture_is_current` が再生成との byte 一致を、`verify_writes` が commands と実際に書かれた message・requeue の一致を確かめる。
  - `cases.jsonl` 2,066 行: 各行を run 1 つの pass に組み直して replay し、`decide()` の出力も比べる。Rust の出力との byte 一致は `script_fixtures_cos_digest_match_committed_fixture`。
- 比較が食い違いを検出すること: 写し（`$TMPDIR` の下）で TS の requeue の規則を 1 か所壊す（interrupted を requeue しない）と、`1682/2086 matched`・exit 1。照合の食い違いの検出は `cos_digest_pass_trace_verify_detects_mismatch`。
- 本物の host: `script_compare_cos_digest_real_host_matches_rust_on_all_fixtures`（cases の全行を同じ isolate に正順・逆順）。条件 0 の 9 passed に含む。

**安全回帰 0**
- 検証落ちでは TS の command を 1 件も apply せず、message・item が mode `rust` の基準と byte 一致する: `cos_digest_pass_ts_falls_back_on_error_timeout_invalid`（例外・timeout・形の誤り・規則 1・8・9）、`cos_digest_pass_ts_rejects_run_outside_list`（規則 7、安全違反 `run_outside_list`）、`cos_digest_pass_compare_denies_unlisted_read`（`denied_scope`）。規則 1〜9 を 1 件ずつ破る検証関数の試験は `chat::digest::pass_tests::cos_digest_pass_validation_rejects_each_rule`。
- mode `compare` の digest は従来どおり Rust の結果で、TS の誤答・例外・timeout・検証落ちで変わらない（`cos_chat_triage_digest_script_compare.rs` 10 件・`cos_digest_script.rs` の親 11 件）。
- 既存の安全検査は移さず、緩めていない。S4 の作業の始まり（commit `integrate wu/s2-pure-logic-parity (phase s2)`）から HEAD までの `crates/` の差分（試験の file を除く）の変更行に `plan_conforms`・`job_wait_extend`・`human_required`・`policy_version` は 0 件（`git diff … -- crates/ ':!*tests*' ':!*_tests.rs' | grep -cE …` → 0）。
- 実行: `cargo nextest run -p task-dispatch -p task-core -E 'test(/cos_digest|cos_chat_triage|script_compare|script_fixtures/)'` → exit 0、163 passed。内訳（`cargo nextest list` で数えた cos_digest の試験）: `cos_digest_script` 11、`cos_digest_script::cos_digest_pass` 8、`cos_digest_script::ts` 15、`digest_script_compare` 10、`pass_trace` 4、`chat::digest::pass_tests` 1、`script_fixtures_cos_digest_*` 4。

### 2. event 順序・再配送・retry・cancel・再起動・policy_version/session freshness の試験
mode `ts` の async pass の試験は `crates/task-dispatch/src/dispatcher/tests/cos_digest_script_ts.rs`（15 件）。mode `compare` の 11 件は `cos_digest_script.rs` に残る。全件で共通の不変条件を確かめる（message は冪等 key ごとに 1 件、requeue は item ごとに 1 回まで、記録は pass の run ごとに 1 件まで）。時計は `tokio::time::pause()` で止め、op の前後の遅延フックで await 中の出来事を起こす。実時間の待ちと CPU 負荷は使わない。上の 163 passed に含む。

| 観点 | mode `ts` の試験 | mode `compare` の試験 |
|---|---|---|
| 順序 | `cos_digest_script_ts_order_item_update_before_and_after_run_end`、`…_ts_item_updated_during_op_await` | `cos_digest_script_order_item_update_before_and_after_run_end` |
| 再配送 | `…_ts_redelivery_same_run_twice` | `cos_digest_script_redelivery_same_run_twice` |
| retry | `…_ts_retry_message_write_failure`・`…_ts_retry_scan_failure`・`…_ts_retry_list_runs_failure_during_op`・`…_ts_retry_read_run_failure_during_op`・`…_ts_retry_requeue_write_failure` | `cos_digest_script_retry_after_failed_message_write` ほか 2 件 |
| cancel（op の await 中） | `…_ts_cancel_deadline_during_op_await`（`timeout` で Rust を apply）・`…_ts_cancel_dispatcher_stop_during_op_await`（何も書かず、作り直した dispatcher が 1 回だけ apply） | `cos_digest_script_ts_cancelled_at_deadline`・`cos_digest_script_dispatcher_dropped_mid_compare` |
| 再起動 | `…_ts_restart_mid_pass_applies_once`・`…_ts_restart_after_failed_requeue`。本物の daemon は `script_compare_cos_digest_daemon_ts_restart_keeps_one_message`（同じ bundle では `ts` を保ち message 1 件、`[script]` の無い daemon は `system:script-startup` で `rust` に戻す） | `cos_digest_script_restart_same_db`・`cos_digest_script_restart_after_failed_requeue` |
| policy_version・session freshness | `…_ts_policy_version_bump_session_fresh`（版を 7 に上げた後の run は `policy_changed` で fresh に始まり、記録 2 件とも `applied_by = ts`） | `cos_digest_script_policy_version_change_and_fresh_session` |
| mode・generation の変化（await 中） | `…_ts_mode_and_generation_change_during_op_await`（`mode_changed` で Rust を apply）・`…_ts_generation_gone_during_await_falls_back` | `cos_digest_script_mode_back_to_rust_and_generation_change_mid_compare` |

起動時の復元（`restore_modes`）は real-host unit で `ts` の行も `rust` へ戻すように直した。直す前の code では `…_daemon_ts_restart_keeps_one_message` が落ちることを real-host unit が確かめた。

### 3. 変更の影響範囲の test が通る
close-async unit の差分は文書だけ（この file・`docs/architecture-map.md`・境界 ADR）。提出時の HEAD で実行した結果は「検査（close-async）」に書く。

各 unit の結果（それぞれの unit の提出時の HEAD）:
- host-ops: `bash scripts/dev/test-affected.sh` exit 0（Cargo.lock の変更で workspace 全体。nextest 7,105 passed・21 skipped、web 697 passed、control-ts 81 passed）
- mode-ts: exit 0（nextest 7,015 passed・21 skipped）
- pass-wire: exit 0（task-core の変更で workspace 全体。nextest 7,075 passed・21 skipped）
- ts-pass: exit 0（control-ts の typecheck と test だけ）
- traces-async: exit 0（nextest 3,028 passed・9 skipped、control-ts も）
- real-host: `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で exit 0（nextest 879 passed・0 skipped）
- robust-ts: exit 0（nextest 3,042 passed・9 skipped）
- fixture-ref: exit 0（nextest 879 passed・0 skipped）
- 各 unit の `cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` は exit 0。
- 前段（rust-split・hook・trace-capture・ts-digest・host-adapter・robustness）の結果は各 unit の記録にある。

統合の段（integrate-a-design・a-core・a-wire・a-verify）は決定的な check を通った。a-verify の統合は初回に落ち（real-host の試験が消えた `traces.jsonl` を読んだ）、fixture-ref が直した。

### 検査（close-async、提出時の HEAD）
- `bash scripts/dev/test-affected.sh` → 結果は下の「close-async の検査結果」に書く。
- `cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`bash scripts/dev/check-doc-links.sh`・`bash scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`・`python3 scripts/dev/check-architecture-map.py` → 同上。

## unit と commit（件名）
| unit | 内容 | commit の件名 |
|---|---|---|
| [adr-s4](2026-11-02-ts-control-layer-s4/adr-s4.md) | 境界 ADR の付記 S4-1〜S4-9 | `phase s4: adr-s4: 境界 ADR に S4 の CoS digest 経路の付記、進捗を書く` |
| [rust-split](2026-11-02-ts-control-layer-s4/rust-split.md) | digest の 4 分割（挙動不変）と cases fixture | `phase s4: rust-split: CosTriageItemReport と入れ子の型に Serialize・Deserialize・JsonSchema・deny_unknown_fields を付ける`、`phase s4: rust-split: digest を snapshot→decide→validate→apply に分け、cos_digest の比較 fixture を足す` |
| [hook](2026-11-02-ts-control-layer-s4/hook.md) | 領域の登録、migration 0104、compare の結線 | `phase s4: hook: cos_digest 領域・script_compare_log・digest pass の compare を結線する` |
| [trace-capture](2026-11-02-ts-control-layer-s4/trace-capture.md) | run ごとの traces fixture（後に traces-async が消した） | `phase s4: trace-capture: digest pass の記録済み trace を dispatcher の経路から書き出す試験と traces.jsonl を足す` |
| [ts-digest](2026-11-02-ts-control-layer-s4/ts-digest.md) | TS の `decide()`・単体試験・compare の dir 読み | `phase s4: ts-digest: cos_digest 領域の TS 実装・単体試験・compare の dir 読み` |
| [host-adapter](2026-11-02-ts-control-layer-s4/host-adapter.md) | 本物の host の試験 | `phase s4: host-adapter: 本物の deno_core host で cos_digest の replay と compare を確かめる試験を足す`、`phase s4: host-adapter: 進捗の記録を足す` |
| [robustness](2026-11-02-ts-control-layer-s4/robustness.md) | mode compare の順序・再配送・retry・cancel・再起動・版変化の試験 | `phase s4: robustness: digest pass の順序・再配送・retry・cancel・再起動・版変化の試験を足す` |
| close（v1） | 進捗・地図・付記 S4-10 | `phase s4: close: S4 の進捗まとめ・architecture-map・境界 ADR の実装との突き合わせ` |
| [adr-async](2026-11-02-ts-control-layer-s4/adr-async.md) | 付記 S4-11〜S4-19 | `phase s4: adr-async: 境界 ADR に digest pass 全体の async 経路の付記 S4-11〜S4-19` |
| [host-ops](2026-11-02-ts-control-layer-s4/host-ops.md) | Host API 1.1.0 の op 2 つ・capability・排他と失効 | `phase s4: host-ops: Host API 1.1.0 に cos_digest の async 読み出し op 2 つ・capability・要求ごとの排他と失効`、`phase s4: host-ops: 進捗に test-affected の結果を記録`、`phase s4: host-ops: host sink を ScriptCall の欄から call_with_host_sink の引数へ移し、celeris の呼び出し側を無変更にする`、`phase s4: host-ops: 範囲に host-stub.ts が入った後の検査結果を進捗に記録` |
| [mode-ts](2026-11-02-ts-control-layer-s4/mode-ts.md) | mode `ts` の受け付けと権限 | `phase s4: mode-ts: cos_digest だけ mode ts を受け、人の actor だけが切り替える`、`phase s4: mode-ts: celerisctl の inline 試験の行数上限に合わせて main.rs の追加を戻す` |
| [pass-wire](2026-11-02-ts-control-layer-s4/pass-wire.md) | dispatcher の pass・memo・規則 7〜9・mode ts の apply と fallback・記録の欄 | `phase s4: pass-wire: digest pass を TS の async pass へ結線し、mode ts で TS の command を検証→apply する`、`phase s4: pass-wire: test-affected の結果を進捗に記録` |
| [ts-pass](2026-11-02-ts-control-layer-s4/ts-pass.md) | TS の `cosDigestPass`・fake host の replay | `phase s4: ts-pass: cos_digest の入口を async の cosDigestPass(ctx) にし、fake host で pass trace を replay する` |
| [traces-async](2026-11-02-ts-control-layer-s4/traces-async.md) | `pass_traces.jsonl` と旧 traces の削除 | `phase s4: traces-async: 本物の dispatcher の digest pass から pass_traces.jsonl を書き出し、旧 traces を消す`、`phase s4: traces-async: 範囲 check に合わせ、試験を cos_digest_trace.rs に置き、compare.mjs と地図を戻す` |
| [real-host](2026-11-02-ts-control-layer-s4/real-host.md) | 本物の host の mode ts・compare・再起動、`restore_modes` の修正 | `phase s4: real-host: 本物の host で mode ts の apply・fallback・再起動を確かめ、起動時の復元を ts にも当てる`、`phase s4: real-host: 進捗の記録を足す`、`phase s4: real-host: 起動時の復元の直す前後の試験結果を記録に足す` |
| [robust-ts](2026-11-02-ts-control-layer-s4/robust-ts.md) | mode ts の頑健性の試験 15 件 | `phase s4: robust-ts: mode ts の async pass の順序・再配送・retry・cancel・再起動・policy_version の試験` |
| [fixture-ref](2026-11-02-ts-control-layer-s4/fixture-ref.md) | 本物の host の試験を `cases.jsonl` だけで回す | `phase s4: fixture-ref: 本物の host の試験を cases.jsonl だけで回し、消えた traces.jsonl を読まない` |
| close-async | この file・地図・境界 ADR の S4-20 | `phase s4: close-async: S4 の進捗・architecture-map・境界 ADR を async 経路の実装と突き合わせる` |

統合の commit（`integrate wu/…`）は celeris が作った。

## 未解決
1. pass の trace は FakeAdapter の試験から書き出したもので、本番の記録済み LLM 応答ではない。本番の log の replay の道具は作らない（付記 S4-16）。log は本文を持たないので、本番の log からは trace を作れない。
2. `read_run` が失敗する pass は fixture に入れていない。`cos_digest_pass_trace_read_failure_skips_the_run_in_both_modes` と `cos_digest.test.ts` の手書きの trace で確かめた（traces-async）。
3. `control-ts/scripts/compare.mjs` の冒頭の注釈（10 行目）は `traces.jsonl` の名前を残している。動作には関わらない。close-async はコードを変えない unit なので直していない（下の提案 6）。
4. 要求ごとに V8 context を分けていない。capability の無い要求と並行する `cos_digest` の要求の token を global から読めば、その要求の scope で op を呼べる（scope は同じ pass から広がらない）。要求ごとに isolate を作り直す方式が 200 ms に収まるかは未確認（付記 S4-13・S4-20 の (6)）。
5. 起動時の復元は「manifest の `cos_digest` に capability が足りない」を bundle の hash でしか見ない（未確認。付記 S4-20 の (8)）。
6. 本物の daemon の再起動の試験は、cold の isolate で 200 ms を超えたとき Rust の apply（`ts_error`・`timeout`）も許す。TS の apply は tick の試験で確かめている（real-host）。
7. mode `ts` の dispatcher は 1 tick で TS を最大 200 ms 待つ。tick の遅れの許容は S5 の停止基準で決める。
8. `release.sh`・`test-affected.sh` は `control-ts/dist` を作らない。dist が無いか古い環境では、本物の host の試験のうち bundle を使うものが SKIP で通る（S2 の未解決 2 と同じ）。close-async は bundle を作って `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で 0 skipped を確かめた。
9. 新しい binary は欄の無い旧い形の `script_compare_log` の行を読めない。本番は mode `rust` で行を書いていない想定（未確認）。非本番の DB に S4 v1 の compare の行があれば、読み出しが失敗しうる（pass-wire）。
10. `cargo clippy --workspace --all-targets` は S4 が触っていない `crates/task-core/src/chat/plan_rejudge_tests.rs` の既存の警告で落ちる。必須の検査は `--all-targets` なしで通る（pass-wire）。

## 提案（別 task の候補）
1. S5 で mode `ts` の canary と停止基準を決めるとき、`script_compare_log` の `applied_by`・`fallback` を分母に使う（adr-async）。
2. requeue の書き込みが message の後で失敗すると、requeue は再試行されない（run が digest 済みになる）。message の書き込みと requeue を 1 つの transaction で行う store の API を足す。言語移行と別の差分にする（robustness。mode `ts` でも同じ。`…_ts_retry_requeue_write_failure`）。
3. `digest_snapshot` が `chat_run_get` の失敗を `reason: null` にするので、一時的な読み出しの失敗で reason が恒久的に抜ける。NotFound 以外は Err にして次の tick に回す（robustness）。挙動の変更なので別 task にする。
4. release gate と統合の check で `control-ts` の bundle を作り、`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で本物の host の試験を走らせる（S2 の提案 1 と同じ）。
5. `script_compare_log` を読む API か `celerisctl` の command を足す。今は sqlite3 で本番 DB を読み取り専用で開くしかない。
6. `compare.mjs` の冒頭の注釈から `traces.jsonl` を消し、`pass_traces.jsonl` の `writes` 欄に触れる（未解決 3）。
7. 次に移す候補は B 定期見直し（`cos_review` cron）の入力文と C 受信箱の取り込み分類（付記 S4-1）。op を使う領域が 2 つ以上になる前に、要求ごとの isolate の分離を決める（未解決 4）。

## 本番への移行
S4 は本番 config を書き換えていない。本番の `cos_digest` は `rust` のままで、migration の後も TS を呼ばない。
本番で `compare`・`ts` にするのは人の手順で、既存の認可手順（release → verify → 人の承認で promote）に従う。付記 S4-14 は、本番で `compare`・`ts` にする手順と canary・停止基準を S5 で決めるとしている。下の手順は、人が S5 より前に試すと決めた場合のものである。
**IEEE Access・K2・K4 の凍結環境と測定 release には入れない。** 測定の daemon では `[script]` を書かず、mode を変えない（境界 ADR (g)）。

1. release を作る
   - `scripts/selfdeploy/release.sh <S4 を含む main の commit>`。migration 0104（`script_domain_modes` の `cos_digest` 行、表 `script_compare_log`）は promote 後の起動で当たる。S2 の 0103 が未適用なら一緒に当たる。
   - Host API が 1.1.0 になり schema の hash が変わったので、配備済みの古い bundle は新しい binary の load の照合で合わない。起動時に `system:script-startup` が各領域を `rust` へ戻す。2. で bundle を作り直す。
   - S1 の「本番への移行」の V8 archive の確認（`RUSTY_V8_ARCHIVE`）が済んでいること。既存の手順で verify し、人が承認して promote する。
2. bundle を作り、state_dir に配備する（release と同じ commit の checkout で）
   - `cd control-ts && corepack pnpm@12.6.0 install --offline --frozen-lockfile && corepack pnpm@12.6.0 run bundle`
   - `node scripts/compare.mjs cos_digest` が `2086/2086 matched` で exit 0 であることを確かめる。
   - `cd .. && cargo run --release -p celeris-script --bin script-deploy -- --dist control-ts/dist --state-dir <[script] state_dir>` → stdout の 1 行が `bundle_sha256`。
3. config の `[script]` で active bundle を指し、daemon を既存の手順で再起動する
   ```toml
   [script]
   state_dir = "<state_dir>"
   active_bundle_sha256 = "<2. の bundle_sha256>"
   ```
   起動の log と通知に `script_bundle_rejected` が無いことを確かめる。
4. mode を compare にする（人・operator のどちらでもよい）
   - `celerisctl script domains` で `cos_digest` の `version` を読む。
   - `celerisctl script mode cos_digest compare --expected-version <version> --generation <bundle_sha256> --mode-reason "S4 compare を本番で有効にする"`
5. compare の確かめ方（本番 DB を読み取り専用で開く）
   - `sqlite3 -readonly <本番 DB> "SELECT json_extract(json,'$.mode'), json_extract(json,'$.outcome'), json_extract(json,'$.applied_by'), count(*) FROM script_compare_log WHERE domain='cos_digest' GROUP BY 1,2,3"`
   - `sqlite3 -readonly <本番 DB> "SELECT count(*) FROM script_compare_log WHERE domain='cos_digest' AND json_extract(json,'$.safety_violations') != '[]'"`
   - 期待: triage run の digest ごとに 1 行、`mode = compare`・`outcome = match`・`applied_by = rust`、安全違反 0 件。`mismatch`・`invalid_output`・安全違反が 1 件でもあれば 7. で戻す。
6. mode を ts にする（**人の承認の後に、人が自分の API token で行う**）
   - `→ts` は人の actor（`human:*`）だけが行える。CoS・外部 operator（`operator:claude-code`）・`system:*` の要求は 403 `script_mode_requires_human` になる。元は同じ generation の `compare` に限る。
   - 人の API token を使う `celerisctl` で: `celerisctl script mode cos_digest ts --expected-version <現在の version> --generation <compare と同じ bundle_sha256> --mode-reason "<承認の記録>"`
   - 確かめ方: `celerisctl script domains` で `cos_digest` の mode が `ts`。5. の query で新しい行が `mode = ts`。`applied_by = ts`・`fallback IS NULL` の行が増え、`fallback` のある行（`json_extract(json,'$.fallback.code')`）が 0 件か少数であること。digest の message の件数（`chat_messages` の `client_message_id LIKE 'cos-triage-digest:%'`）が run ごとに 1 件であること。
   - `fallback` の code が `invalid_output`・`run_outside_list` の行、または安全違反の行が 1 件でもあれば 7. で戻す。fallback の pass では Rust の command が apply されているので、digest 自体は欠けない。
7. 戻し方
   - `celerisctl script mode cos_digest rust --expected-version <現在の version>`（`ts→rust`・`ts→compare`・`compare→rust` は誰でも、CoS も行える）。戻した後は TS を呼ばない。
   - `[script]` を消して再起動しても、daemon が起動時に `compare`・`ts` の行を `rust` へ戻す（`system:script-startup`）。
   - mode の変更の履歴は `script_domain_events`、記録は `script_compare_log` に残る（どちらも追記専用で、消さない）。
