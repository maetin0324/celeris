---
tasks: [01M6HGFR1TR6PJ8822DHW7NNNT]
status: done
completed: 2026-11-03
---
# P1-C: script 制御の管理 CLI と観測の不足を埋める

task 01M6HGFR1TR6PJ8822DHW7NNNT（段階移行 2、親 01M6HG7T6ZKTX239KGH0QY4HFQ の p1c-ops-observe）。設計は
[ADR 2026-11-03-ts-control-ops-cli-observe](../../adr/2026-11-03-ts-control-ops-cli-observe.md) と境界 ADR の付記 P1-C。
基準は `git merge-base HEAD main`。本番・release・mode の昇格・guard の解除・IEEE Access/K2/K4 の測定環境には触れていない。

## 変更（file・関数）
1. CLI（S5 未解決 1・提案 1）: `crates/celerisctl/src/commands/script.rs`
   - `ScriptCommand::{Mode, Generations, Guard}`、`ModeArg::Canary`、`ModeArgs::canary_bps`（`--canary-bps`、clap で 1..9999）。
   - `GenerationsCommand::{List, Register, Retire}`（`--retire-reason`）、`GuardCommand::{Status, Clear}`（`--clear-reason`、必須）。
     全体の `--reason`（CoS の監査の理由）と重ならない名前にした。
   - `request_of` が変更系 4 種（mode・register・retire・guard clear）の要求を組む。`render_table`（BPS 列と停止の行）、
     `render_generations`、`render_guard`。help の文面を S5-4（rust→compare→canary→ts、canary・ts・guard clear は人だけ）に直した。
   - `crates/celerisctl/src/main.rs` は `Command::Script` の doc だけ。CoS の run の包み（`cos_mapped`）は `request_of` をそのまま使う。
2. 未配備の登録（S5 未解決 2・提案 2）: `crates/celeris/src/daemon/script_layer.rs` の `ScriptGenerations::register_locked` は、
   配備先の dir が無ければ `ScriptGenerationError::NotDeployed` を返し `invalid` を書かない。`crates/task-api/src/script_generations.rs` に
   `NotDeployed` と 409 `SCRIPT_BUNDLE_NOT_DEPLOYED`（`script_bundle_not_deployed`）。dir が在って検証に落ちたら今までどおり `invalid`。
3. tick の待ちの log と metric（S5 未解決 4・提案 3）:
   - `crates/task-ops/src/daemon.rs` の `DaemonSnapshot.script_tick_wait: Option<ScriptTickWaitLive>`（`GET /daemon`）。
   - `crates/task-dispatch/src/dispatcher/script_canary.rs` の `Dispatcher::script_tick_wait_live`、`snapshot.rs` が載せる。
   - `crates/celeris/src/daemon/tick_loop.rs` の `SCRIPT_TICK_WAIT_LOG_INTERVAL`（600 秒）・`script_tick_wait_log_due`・`log_script_tick_wait`
     （予算切れが窓にあれば warn）。
   - `docs/api/v1/api-v1.schema.json`・`docs/api/v1/api.md`（改訂行・§3.20・§3.134.3）・`web/api/generated/{schema.json,types.ts}` を再生成。
4. script-guard の増分の見回り（S5 未解決 6・提案 4）: `crates/task-core/src/store/script_domains.rs` に
   `SqliteStore::script_compare_log_after(after, limit)`（`WHERE id > ?1 ORDER BY id LIMIT ?2`）。`crates/celeris/src/daemon/script_guard.rs` の
   `sweep` は `GuardState::log_cursor` から 500 行の page で差分だけ読む（`log_rows_read` で読んだ行を数える）。
5. 操作表の文面: `config/skills/cos-operator/operations.md` の register・retire の行に CLI と 409 を書いた（行・op 名・ALLOWED は変えない。
   registry も変えないので `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` は変えていない。`metadata.version` は 5 のまま）。

## 試験（足したもの）
- `celerisctl`: `script_canary_generations_and_guard_requests`・`script_generation_table_and_guard_line`・
  `script_cos_wrapped_requests_match_the_operations_table`（register・retire は ALLOWED の行、guard clear は行に無い）。
- `task-api`: `script_domain_guard_clear_refused_for_cos`（CoS の run credential の直接の POST と `/cos/operations` の包みが共に拒まれ、
  包みは 422 `cos_operation_not_allowed`・`human_only`、停止は残り `script_guard_cleared` は 0 件。人の token では解ける）、
  `script_domain_generation_register_not_deployed_409_is_retryable`。既存の operator の 403（`script_domain_mode_canary_requires_human`・
  `script_domain_guard_clear_human_only`）と CoS の canary・ts の 403（`script_domain_mode_through_cos_operations`）はそのまま通る。
- `celeris`: `script_daemon_register_before_deploy_is_retryable`（実 isolate。未配備 2 回は行無し → 配備 → `loaded`）、
  `script_guard_sweep_reads_compare_log_incrementally`（1100 行を page をまたいで読み、新しい行が無ければ 0 行、501 行目の違反で止める）、
  `script_tick_wait_log_is_due_every_ten_minutes`、`script_ops_restart_drain_retire_cycles`（下の計測）。
- `task-dispatch`: `route_priority_canary_override_reorders_only_its_harness`（`priority_override` は同じ harness の列だけを置き換え、
  違う harness・CoS には効かず、空きの無い source は次の候補へ進む。S5 未解決 5）。

## 計測（raw は同じ dir の `p1c-ops-cycles.jsonl`）
コマンド: `CELERIS_P1C_CYCLES=20 CELERIS_P1C_MEASURE_OUT=$PWD/agent-docs/progress/2026-11-03-ts-control-ops/p1c-ops-cycles.jsonl cargo nextest run -p celeris -E 'test(/script_ops_restart_drain_retire_cycles/)'` → exit 0（1 passed）。raw は cos-read-fix の変更の後に取り直した。
1 周 = 再起動（新しい登録簿で `restore`）→ 新しい bundle の配備と登録 → compare への切替（前の generation は draining）→ `reconcile`
（in-flight 0 で retired）→ lane_rules を 200 回（期限 `SCRIPT_CALL_DEADLINE` 50 ms）→ 受信箱の取り込みを 2 回 → `shutdown`。
3 周に 1 回は再起動の前に bundle を消し、起動時の戻しを起こす。bundle は手書きの JS、debug build の試験 process、CPU を焼く負荷は無い。

| 項目 | 20 周の値 |
|---|---|
| error / timeout | 0 / 0（4000 回） |
| 呼び出しの p50 / p95 | p50 134〜150 µs、p95 150〜193 µs、最大 561 µs |
| tick の待ち（5 回の和） | p50 681〜753 µs、p95 736〜965 µs、最大 1242 µs、予算 200 ms 超えは 0 |
| 生きた generation | 毎周 1（退役が追いつく） |
| 受信箱 | 起動時の戻しの周（3・6・9・12・15・18）だけ `inbox_opened`=1、他の周は 0。2 回目の取り込みで開いた項目（`inbox_reopened_on_second_ingest`）は 20 周とも 0 件。周をまたぐ累計 `inbox_opened_cumulative` の最終値は 6 で、戻しの周の数（`restore_rollbacks_cumulative`=6）と一致する。項目は周の中で閉じるので `inbox_open_items` は 20 周とも 0 |
| RSS（試験 process） | 開始 30.6 MB → 周 0 で 74.1 MB → 周 3 以後 79.2〜80.7 MB（最大 80.7 MB） |
| canary の比率（route_priority、2 万単位） | bps 100 → 1.01%、500 → 4.985%、5000 → 50.575% |

これは試験 process の計測で、本番 daemon の tick の遅延ではない。本番の値は `GET /daemon` の `script_tick_wait` と 10 分ごとの log 行で読む。
TS-only の高速経路（`scripts/dev/ts-verify.sh`、cargo 0 回）は変えていない。test-affected の `test-ts-verify` は 18 passed。

## 検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 0。nextest 7202 passed・21 skipped、web の tsc -b と vitest 697 passed、test-ts-verify 18 passed、
  script bundle の SKIP 0 件。1 回目は source-size gate（`celerisctl/src/main.rs` の inline 試験 335 行 > 300）で落ち、試験を
  `commands/script.rs` に移して直した。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `cargo run -q -p celerisctl -- script generations --help && cargo run -q -p celerisctl -- script guard --help` → exit 0。

## cos-read-fix（final review の差し戻し 3 点の修正）
1. CoS の run で読み取りの CLI が gate で落ちた欠陥: `crates/celerisctl/src/commands/script.rs` に `cos_read_only(&ScriptCommand)` を足し、
   `Domains`・`Generations(List)`・`Guard(Status)` だけを読み取りとする。`crates/celerisctl/src/main.rs` の `read_only` は Script の行を
   この関数に任せる。変更系（mode・register・retire・guard clear）は入らず、従来どおり `cos_mapped` で `/cos/operations` に包む（権限は広げない）。
   試験: `script_cos_read_only_is_domains_generations_list_and_guard_status`・`script_cos_read_only_and_request_of_partition_every_subcommand`
   （全 subcommand が読み取りか `request_of` のどちらか一方だけに入る）、task-api の `script_cos_read_only_gets_pass_with_run_credential`
   （CoS の run credential で `GET /script/domains`・`GET /script/generations` が 200、`cos_operations` の行は 0）。
   手で確かめた: `CELERIS_COS_RUN_CREDENTIAL=review-dummy celerisctl script --config /nonexistent/x.toml generations list`（`guard status` も）が
   gate を通り `config /nonexistent/x.toml: failed to read config` まで進む（exit 1、`domains` と同じ）。
2. `GuardCommand::Clear` の help と module doc: operator・MCP は 403、CoS の run は `/cos/operations` に包まれて 422
   `cos_operation_not_allowed`（human_only）と書き分けた。試験 `script_cos_read_only_guard_clear_help_names_403_and_422`。
3. `crates/celeris/src/daemon/script_ops_measure_tests.rs`: `inbox_duplicate_items` を消し、`inbox_opened_cumulative`・
   `restore_rollbacks_cumulative` を足した。周ごとに「戻しの周だけ `inbox_opened`=1」を、終わりに「累計 = (周 − 1) / 3」（20 周で 6）を assert する。
   raw を取り直し、上の表を新しい値にした。

## commit（件名）
- phase p1c: script 制御の管理 CLI（canary・generations・guard）、未配備の登録を 409 に、tick の待ちの snapshot と log、script-guard の増分の見回り、周回の計測
- phase p1c: CoS の包みの試験を commands/script.rs に移し、main.rs の inline 試験の大きさを戻す
- phase p1c: route_priority の上書きの試験の型を直す
- phase p1c: 進捗を書く
- cos-read-fix: CoS の run で script の読み取り CLI を通し、guard clear の help と受信箱の累計の根拠を直す

## 未解決
1. 本番 daemon での `script_tick_wait` の値と RSS の推移は未確認（本番は全領域 `rust`。人が compare 以上にした後に `GET /daemon` で読む）。
2. 見回りの cursor は process の中だけにあり、再起動後の最初の見回りは `script_compare_log` を 1 回全件読む。
3. 計測は debug build の試験 process で、dispatcher の実 tick の遅延（`slow tick`）は測っていない。dispatcher の中で 50 ms の判定を
   5 回待たせる試験は無い（S5 未解決 5 の後半）。
4. 二重の受信箱の知らせ（CoS 経由 1 回・受信箱 1 回。S5 未解決 8）の扱いは人の確認待ちのまま。この task で確かめたのは、
   戻し 1 回につき項目が 1 件だけ開き（累計 6 = 戻し 6 回）、2 回目の取り込みでは開かないことだけ。
5. `script guard clear` の CoS の拒否は registry の EXCLUDED に頼る（CLI は包むだけ）。

## 次の gate
- p1e-shadow-preflight（p0a・p1c の後）。人が release・verify・promote を判断し、その後に `celerisctl script generations register` と
  `celerisctl script mode … compare` を使う。canary・ts の昇格と guard の解除は人が自分の token で行う。

## 本番への移行
worker は本番を変えていない。config の欄は増減していない。release の後、人が次を行う。
- cos-operator skill の操作表の文面を本番 KB に取り込む: `celerisctl skills import config/skills --name cos-operator --root <kb_root>`（冪等。
  `metadata.version` は 5 のままで `[cos] policy_version` の書き換えは要らない）。
- 手順の変更: 未配備の hash の登録は 409 `script_bundle_not_deployed` になるだけで hash は使えなくならない。登録は
  `celerisctl script generations register <sha>`、canary は人の token の config で `celerisctl script mode <domain> canary --generation <sha> --canary-bps <n> --expected-version <v>`。

## 提案
1. `GuardState::log_cursor` を DB に置くかは、本番の `script_compare_log` の行数を見て決める（今は不要と判断）。
2. dispatcher の tick の中で TS の待ちを注入する試験フック（50 ms × 5 回）を足し、`tick_budget` の計数を実 tick で確かめる。
