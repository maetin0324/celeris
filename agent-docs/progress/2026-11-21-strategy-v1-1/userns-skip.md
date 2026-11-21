---
title: userns の 9 件を env 無しでは skip と数え、env 有りでは本体を実行する形に直す
tasks: [01M7XSH46KVRV62ENAWWKWJKBQ]
status: running
updated: 2026-11-21
---
# userns の 9 件を env 無しでは skip と数え、env 有りでは本体を実行する形に直す

完了日: （未完了。before・impl の段まで。verify の段が完了にする）

結論: 対象の 9 件は、userns の env が無い今の build で nextest に `9 passed` と数えられる（exit 0）。
試験の本体は 1 行も走らず、`return` で抜けて pass になっている。この段（before）はコードを変えていない。

impl の段の結論: build script が env を読んで cfg `celeris_userns_tests` を出し、9 件は env 無しの build で harness の ignored に
なった。`check-skip.sh` は exit 0（9 件が list で ignored、run で SKIP、PASS 0 件）。env 有り（`CELERIS_USERNS_TESTS=1`）の試し実行では
task-worker の 7 件が本体に入り（`USERNS-BODY` の行）、試験 1 だけが深い `TMPDIR` で socket の path が長すぎて fail したので、
runtime dir を短い一時 dir に移して直した（assert は変えていない）。設計は
[ADR 2026-11-21-userns-tests-explicit-skip](../../adr/2026-11-21-userns-tests-explicit-skip.md)。

## 対象の 9 件（return の行は提出時の HEAD で確認）

| # | file:line（return） | 試験名 | return させる判定 | 判定の実体 |
|---|---|---|---|---|
| 1 | `crates/task-worker/src/db_guard_tests.rs:100` | `db_guard::tests::user_systemd_bus_is_hidden_from_launched_process` | `if !userns_available()`（:99） | `db_guard_tests.rs:69` の `userns_available()` → `test_support.rs:24` の `skip_unless_userns_tests()` |
| 2 | `crates/task-worker/src/db_guard_tests.rs:158` | `db_guard::tests::host_config_read_only_inside_namespace` | `if !userns_available()`（:157） | 同上 |
| 3 | `crates/task-worker/src/db_guard_tests.rs:300` | `db_guard::tests::the_db_and_its_wal_files_are_read_only_but_siblings_stay_writable` | `if !userns_available()`（:299） | 同上 |
| 4 | `crates/task-worker/src/db_guard_tests.rs:353` | `db_guard::tests::a_nested_user_namespace_cannot_undo_the_read_only_mount` | `if crate::test_support::skip_unless_userns_tests()`（:352） | `test_support.rs:24` を直に呼ぶ（`unshare -Ur true` の確認は無く、`unshare --help` の存在を assert する） |
| 5 | `crates/task-worker/src/db_guard_tests.rs:379` | `db_guard::tests::the_guarded_process_has_no_capabilities_and_keeps_its_uid` | `if !userns_available()`（:378） | `db_guard_tests.rs:69` |
| 6 | `crates/task-worker/src/db_guard_tests.rs:397` | `db_guard::tests::probe_confirms_the_db_is_not_writable_inside` | `if !userns_available()`（:396） | `db_guard_tests.rs:69` |
| 7 | `crates/task-worker/src/db_guard_tests.rs:450` | `db_guard::tests::ssh_config_includes_still_parse_inside_the_namespace` | `if !userns_available()`（:449） | `db_guard_tests.rs:69` |
| 8 | `crates/celeris-credentiald/tests/injection_peer_uid.rs:194` | `inner_peer_uid_probe` | `let Some(socket) = std::env::var_os(PROBE_SOCKET_ENV) else`（:193） | `CELERIS_PEER_UID_PROBE_SOCKET` が無ければ何もしない。試験 9 が `sudo -n -u <user>` で自分の test binary を `--exact inner_peer_uid_probe` で起こす子 process の入口 |
| 9 | `crates/celeris-credentiald/tests/injection_peer_uid.rs:350` | `real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch` | `if skip_by_default()`（:349） | `injection_peer_uid.rs:53` の `skip_by_default()` |

planner が挙げた行（:100・:158・:300・:353・:379・:397・:450・:194・:350）は、どれも提出時の HEAD の return の行と一致した。

### env を読む規則（今は 3 か所に別々の写しがある）
- `crates/task-worker/src/test_support.rs:24` `skip_unless_userns_tests()`（lib 内の試験用）:
  `CELERIS_ISOLATION_TESTS=skip` → skip。`CELERIS_USERNS_TESTS=1`・`CELERIS_ISOLATION_TESTS=require`・`CELERIS_DB_GUARD_TESTS=require` のどれか → 走らせる。どれも無い → skip（既定）。
- `crates/task-worker/src/db_guard_tests.rs:69` `userns_available()`: 上の helper が skip を返せば false。走らせる側なら `unshare -Ur true` を試し、失敗すれば assert で fail（「env 有りで userns が作れなければ fail」はここで保たれている）。
- `crates/celeris-credentiald/tests/injection_peer_uid.rs:53` `skip_by_default()`: `CELERIS_ISOLATION_TESTS=skip` → skip。`CELERIS_USERNS_TESTS=1`・`CELERIS_ISOLATION_TESTS=require`・`CELERIS_LAUNCHER_TESTS=require` のどれか → 走らせる。どれも無い → skip。
  `CELERIS_DB_GUARD_TESTS=require` は見ない（task-worker の lib の helper と違う）。

### 2 つ目の skip の分岐（env を立てても return して pass になりうる所）
- 試験 7（ssh）: `db_guard_tests.rs:454`。`/etc/ssh/ssh_config.d` が dir でない、または `ssh -V` が起動できなければ `skip: no ssh or no ...` を出して return。env の値によらない。
- 試験 9（peer uid）: `missing()`（`injection_peer_uid.rs:70`）を通る 7 か所の return。
  `:357`（user `celeris-browser` が無い）、`:373`（`sudo -n -u <user> true` が非 0）、`:377`（sudo が無い）、`:381`（sudo の起動の他の失敗）、
  `:409`（DAC 層の probe の行が返らない）、`:434`（fixture の祖先 dir を別 UID が辿れない）、`:441`（broker 層の probe の行が返らない）。
  `CELERIS_LAUNCHER_TESTS=require` のときだけ `missing()` が panic して fail になる。release gate は `CELERIS_USERNS_TESTS=1` を立てるが
  `CELERIS_LAUNCHER_TESTS=require` は立てない（`scripts/selfdeploy/release.sh:182`）ので、gate でも sudo が無い host では return して pass になる。
- 試験 8（`inner_peer_uid_probe`）: これは userns の opt-in ではなく、子 process の入口。impl の段でこの試験を `#[ignore]` にすると、
  試験 9 の `probe_as()`（`injection_peer_uid.rs:318`）が `--exact inner_peer_uid_probe` で起こす子は ignored のまま走らず、
  `PEER-UID-PROBE` の行が返らない → `missing()` → return（require なら fail）になる。impl では子の起動に `--ignored`（または `--include-ignored`）を足すか、
  試験 8 を ignored にしない扱いを決める必要がある（提案 2）。
- 試験 1〜3・5〜7: `userns_available()` は env 有りで `unshare -Ur true` が失敗すると assert で fail する（skip しない）。
- 試験 4: `unshare -Ur true` の確認をせず、`unshare(1)` の存在だけを assert する。userns が作れない host では本体の
  `nested-userns-ok` の assert（`db_guard_tests.rs:372`）で fail する。

## 変更前の nextest の出力（負の証拠。提出時の HEAD で実行）

command（filterset の全文は `userns-skip/before-nextest.txt` の 1 行目）:

```
env -u CELERIS_USERNS_TESTS -u CELERIS_ISOLATION_TESTS -u CELERIS_DB_GUARD_TESTS -u CELERIS_LAUNCHER_TESTS \
  cargo nextest run -p task-worker --lib -p celeris-credentiald --test injection_peer_uid \
  --status-level pass --success-output immediate --no-tests=fail -E '<9 件の test(=...) と binary_id(...)>'
```

- exit code: 0
- 件数: `Starting 9 tests across 2 binaries (1225 tests and 1 binary skipped)`、`Summary [...] 9 tests run: 9 passed, 1225 skipped`
- PASS の行: 9 件（`grep -cE '^\s+PASS \[' before-nextest.txt` → 9）。ignored・SKIP と数えられた対象は 0 件。
- 各試験の stderr: 試験 1〜7 は `SKIPPED (userns test, not passed): set CELERIS_USERNS_TESTS=1 to run (ADR-0126)`、
  試験 9 は `SKIPPED (peer-uid real-process test, not passed): ...`。試験 8 は何も出さない（env が無いので即 return）。
  libtest の行はどれも `test result: ok. 1 passed; 0 failed; 0 ignored`。
- `--success-output immediate` は stderr の SKIPPED の文言（return の分岐を通った証拠）を残すために足した。`.config/nextest.toml` の既定
  `status-level = "fail"` では PASS の行が出ないので `--status-level pass` を付けた。
- 出力の file: `agent-docs/progress/2026-11-21-strategy-v1-1/userns-skip/before-nextest.txt`（repo の path は `<repo>`、target は `<target>`、
  nextest の run ID は `<run-id>` に置き換えた）。

## impl の段（build script の cfg と ignored）

### 変えたもの
- `crates/task-worker/userns_tests_cfg.rs`（新規。build script 専用）: env を読む規則の唯一の置き場所。ADR-0126 B3 の優先順位
  （`CELERIS_ISOLATION_TESTS=skip` → 無効、`CELERIS_USERNS_TESTS=1`・`CELERIS_ISOLATION_TESTS=require`・crate 固有の `<var>=require`
  → 有効、無ければ無効）で、読む 3 変数の `cargo:rerun-if-env-changed`、`cargo:rerun-if-changed`（`build.rs` と規則の file）、
  `cargo:rustc-check-cfg=cfg(celeris_userns_tests)`、有効なら `cargo:rustc-cfg=celeris_userns_tests` を出す。
- `crates/task-worker/build.rs`（新規。固有の変数 `CELERIS_DB_GUARD_TESTS`）と `crates/celeris-credentiald/build.rs`（新規。固有の変数
  `CELERIS_LAUNCHER_TESTS`）: `#[path]` で規則の file を読み `emit` を呼ぶだけ。
- `crates/task-worker/src/db_guard_tests.rs`: 7 件の先頭の return を消し、`#[cfg_attr(not(celeris_userns_tests), ignore = "…")]` を付けた。
  `userns_available()`（bool を返す）を `assert_userns_available()`（`unshare -Ur true` の assert は今の文言のまま。通ったら stderr に
  `USERNS-BODY <試験名>: unshare -Ur true ok`）に置き換えた。試験 4 は今どおり `unshare(1)` の存在の assert だけで、その後に
  `USERNS-BODY <試験名>: unshare(1) present` を出す。試験 1 は socket を置く runtime dir を `test_support::short_tempdir()` の下に移した
  （下の「env 有りの試し実行」）。
- `crates/task-worker/src/test_support.rs`: `skip_unless_userns_tests` は env を読まず `cfg!(celeris_userns_tests)` から導く
  （lib の他の呼び出し元 `browser_tests.rs` 8 件・`browser_launcher/userns.rs` 1 件は return の形のまま。提案 1）。
- `crates/celeris-credentiald/tests/injection_peer_uid.rs`: `skip_by_default()` を消した。試験 9 は `cfg_attr` の ignored にした。
  試験 8 `inner_peer_uid_probe` は常に `#[ignore = "helper: …"]` にし、socket の env が無い直接の実行は panic で fail する。
  `probe_as` は子を `--ignored --exact inner_peer_uid_probe` で起こす。`missing()` の strict 判定 `require()`（`CELERIS_LAUNCHER_TESTS=require`）
  は「本体に入った後に環境が無ければ fail にするか」の規則なので試験の中に残した（ADR の「残る穴」）。

### env 無しの検査（check-skip.sh）
`sh agent-docs/progress/2026-11-21-strategy-v1-1/userns-skip/check-skip.sh`（repo の root で引数なし。dash で動く。JSON は `python3 -I` で読む）:
1. 4 変数を `env -u` した上で `cargo nextest list -p task-worker --lib -p celeris-credentiald --test injection_peer_uid --message-format json`
   を取り、9 件が在って `ignored: true` か。
2. 同じ env で 9 件の filterset を `cargo nextest run --status-level all --final-status-level all --no-tests=warn` し、9 件がどれも
   `SKIP` の行だけに出て `PASS`・`FAIL` の行に出ないか。
結果（提出時の HEAD で実行）: exit 0。`list: ignored` 9 行、`run: SKIP` 9 行、`Summary [...] 0 tests run: 0 passed, 1234 skipped`。

負の確認: 2 の判定の python を変更前の出力 `userns-skip/before-nextest.txt` に当てると、9 件すべてが
`-> ['PASS'] (want only SKIP)` になり exit 1（変更前の形なら check-skip.sh は落ちる）。

### env 有りの試し実行（impl の段の確認。正式の記録は verify の段）
command: `env -u CELERIS_ISOLATION_TESTS -u CELERIS_DB_GUARD_TESTS -u CELERIS_LAUNCHER_TESTS CELERIS_USERNS_TESTS=1 cargo nextest run
-p task-worker --lib -p celeris-credentiald --test injection_peer_uid --no-capture --status-level all --run-ignored all -E '<9 件の filterset>'`
（filterset は `before-nextest.txt` の 1 行目と同じ）。試験 1 を直す前の結果: `9 tests run: 7 passed, 2 failed`。
- 試験 2〜7: `USERNS-BODY db_guard::tests::<名前>: unshare -Ur true ok`（試験 4 は `unshare(1) present`）の後に PASS。試験 7 は
  `skip: no ssh` を出さずに本体の `ssh -G` まで通った。
- 試験 1: `USERNS-BODY` の後、`UnixListener::bind` が `path must be shorter than SUN_LEN` で FAIL。この run の `TMPDIR` は 93 文字で、
  `tempfile::tempdir()` の下の `runtime/systemd/private` が sun_path（108 byte）を超える。今までは return して pass だったので
  見えていなかった。runtime dir を `test_support::short_tempdir()`（`TMPDIR` が 40 文字を超えれば `/tmp`）に移した後、同じ env で
  この 1 件を走らせて `USERNS-BODY …: unshare -Ur true ok` と `1 test run: 1 passed`。
- 試験 9: 本体に入り、`SKIP: (not passed) cannot run a process as celeris-browser: \`sudo -n -u celeris-browser true\` exit=Some(1):
  sudo: /etc/sudo.conf is owned by uid 65534, should be 0` を出して `missing()` で return し PASS（2 段目の skip。この run の container では
  sudo が使えない）。DAC 層・broker 層の本体は未確認。
- 試験 8: `--run-ignored all` で直接走り、socket の env が無いので panic して FAIL（設計どおり。ADR の D2）。verify の段で
  `--run-ignored all` を付けるなら、この FAIL は「直接の実行を拒む」挙動の記録になる。

## LoC 差分
- before の段: コードの差分は 0 行。足したのは `agent-docs/progress/2026-11-21-strategy-v1-1/` の下の文書・出力・走査の台本だけ。
- impl の段（`git diff --numstat "$CELERIS_WU_BASE" -- crates/task-worker crates/celeris-credentiald`、6 file、+141 −70）:
  `crates/celeris-credentiald/build.rs` +12 −0、`crates/celeris-credentiald/tests/injection_peer_uid.rs` +21 −28、
  `crates/task-worker/build.rs` +8 −0、`crates/task-worker/src/db_guard_tests.rs` +53 −27、`crates/task-worker/src/test_support.rs` +8 −15、
  `crates/task-worker/userns_tests_cfg.rs` +39 −0。production のコード（lib の `#[cfg(test)]` の外）は変えていない（build script 2 つと規則の file を足しただけ）。
- task 全体の LoC 差分（`git merge-base HEAD main` 基準）は verify の段で書く。

## 変更
- `strategy-v1 1 userns-skip impl: build script の cfg で 9 件を env 無しでは ignored にする` — 上の「impl の段」の 6 file、ADR、
  `userns-skip/check-skip.sh`、この md。
- `strategy-v1 1 userns-skip before: 9 件の return の行と env 無しの nextest の出力を記録する` — この md、`userns-skip/before-nextest.txt`、
  `userns-skip/scan-env-return.py`・`userns-skip/scan-env-return.tsv` を足した。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 上の nextest → exit 0（9 passed、PASS 行 9）
- `python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/userns-skip/scan-env-return.py "$PWD"` → 76 行（下の提案 1 の元）
- `sh scripts/dev/progress-index.sh --check` → 下の「検査」に結果を書く
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → この進捗 dir の下の path だけ

## 未解決
- verify の段は未着手（`CELERIS_USERNS_TESTS=1` での 9 件の正式の記録、`CELERIS_ISOLATION_TESTS=skip` との優先順位の記録、status: done）。
- 試験 9 は sudo と user `celeris-browser`（uid 995、この host に在る）が要る。impl の段の run の container では `sudo -n` が
  `/etc/sudo.conf is owned by uid 65534` で使えず、DAC 層・broker 層の本体は未確認。release gate の host で人が確かめる手順は verify の段で書く。

## 提案

### 1. 他の crate にも「env が無いと return して pass」の試験がある（この unit では直さない）

走査: `userns-skip/scan-env-return.py`（`#[test]`・`#[tokio::test]` の fn の中で、`return` の行とその前 3 行の `if`・`let`・`match` に
`std::env::var`・`var_os`、または同じ file の env を読む helper・`skip_unless_userns_tests`・`skip_by_default` の呼び出しがあるものを出す）。
出力は `userns-skip/scan-env-return.tsv`（76 行。うち 7 行が対象の 9 件の task-worker 分、2 行が credentiald 分）。
helper を経由して別の fn の中で return する形と、vendor/ は数えていない（下限の数）。手で分類した結果:

**A. ADR-0126 の helper（`skip_unless_userns_tests`・その別名 `skip`・`skip_by_default`）で既定 skip するもの: 28 件**
- task-worker の lib（対象の 9 件と同じ crate・同じ helper）9 件: `crates/task-worker/src/browser_tests.rs:712`（`execution_fallback_uses_fresh_session_and_refuses_without_conformance`）、
  `:1127`（`specialist_wraps_existing_harness_and_runs_same_browser_task`）、`:1418`（`opencode_and_claude_share_supervised_browser_lifecycle_artifacts_and_cleanup`）、
  `:1537`（`cleanup_failure_marks_browser_failed_instead_of_reporting_completion`）、`:1609`（`launch_uses_generated_policy_and_binds_its_hash_to_the_run`）、
  `:1714`（`actions_and_domains_outside_task_policy_are_stopped_before_the_substrate`）、`:1863`（`production_action_path_reaches_fixture_through_real_browser_and_egress`）、
  `:2697`（`click_approval_opens_a_wait_and_resumes_the_same_session_once`）、
  `crates/task-worker/src/browser_launcher/userns.rs:377`（`real_namespace_when_host_is_ready`。`CELERIS_LAUNCHER_TESTS=require` でなく helper が skip なら return）
- task-worker の tests/（`tests/userns_gate/mod.rs:11` の helper）16 件: `browser_cdp_sink.rs:420`、`browser_egress_relay.rs:69`、`browser_h3_wire.rs:550`、
  `browser_injection_attacks.rs:1626`、`browser_injection_wire.rs:565`、`browser_launcher_ptrace.rs:406`（`CELERIS_LAUNCHER_TESTS=require` でなく helper が skip なら return）、
  `browser_restore_deliver.rs:197`・`:328`・`:385`・`:476`、`browser_runtime_isolated.rs:135`・`:207`・`:437`・`:641`、
  `browser_runtime_supervisor.rs:513`、`browser_shared_cdp.rs:703`（すべて `crates/task-worker/tests/` の下）
- task-api の tests/（task-worker の `userns_gate` を `#[path]` で読む）3 件: `crates/task-api/tests/browser_h3_injection.rs:999`、
  `crates/task-api/tests/browser_eval_surface_suite/browser_restore_deliver.rs:115`、`crates/task-api/tests/browser_eval_surface_suite/browser_restore_live_session.rs:145`。
  後の 2 件は `skip() || !userns_available()` で、`crates/task-api/tests/common/mod.rs:827` の `userns_available()` は `CELERIS_USERNS_TESTS=require` の
  ときだけ fail する。release gate の `=1` では userns が作れない host で return して pass になる（2 つ目の skip の分岐）。

**B. 同じ opt-in の規則を自前で書いているもの: 7 件**
- campaign（`CELERIS_USERNS_TESTS` が `1` か `require` でなければ return）7 件: `crates/campaign/tests/ops_r2_b_start.rs:502`、
  `crates/campaign/tests/ops_r2_probe.rs:235`・`:288`（`userns_enabled()`、:56）、`crates/campaign/tests/ops_r2_socket.rs:128`、
  `crates/campaign/tests/preflight_deny_file.rs:62`、`crates/campaign/tests/runtime_probe.rs:136`・`:199`。
  `CELERIS_ISOLATION_TESTS` を見ない点が ADR-0126 の helper と違う。

**C. env が入力（実物の資産・本番 DB の写し・計測の出力先）を渡し、無ければ return するもの: 9 件**
- `crates/campaign/tests/k4_cells_validity.rs:479`（`CAMPAIGN_K4_TGZ`）、`crates/campaign/tests/real_asset_grade.rs:694`（`CAMPAIGN_ASSET_DIR`）・`:820`（`CAMPAIGN_K4_TGZ`）、
  `crates/campaign/tests/dagbench_overhead.rs:720`（`RAW_DIR_ENV`）、`crates/campaign/tests/k_cases_grade.rs:84`（`CAMPAIGN_UPDATE_CASES=1`、期待値の更新の道具）、
  `crates/celeris/tests/tick_prod_copy_steps.rs:53`（`TICK_PROD_DB`）、`crates/task-ops/tests/feed_measure.rs:13`（`CELERIS_FEED_MEASURE_DB`）、
  `crates/task-worker/src/browser_credential.rs:570`（`BROWSER_WIRING_EXPORT`）、`crates/task-worker/src/scratch/tests.rs:1194`（`CELERIS_REFLINK_TEST_DIR`）

**D. 子 process の入口（親の試験が env を付けて自分の binary を起こす。検査の skip ではない）: 20 件（対象の試験 8 と同じ形）**
- campaign 9 件: `tests/common/dagbench_durable.rs:49`、`tests/dagbench_accept.rs:129`、`tests/dagbench_fault.rs:128`、`tests/dagbench_harness.rs:234`・`:567`・`:921`、
  `tests/dagbench_overhead.rs:155`、`tests/dagbench_process.rs:48`、`tests/global_ledger_atomic.rs:107`
- celeris 2 件: `tests/long_run_soak.rs:696`、`tests/script_runtime.rs:292`。script-runtime 1 件: `tests/runtime.rs:445`
- task-worker 8 件: `src/browser_tests.rs:1820`・`:1961`、`tests/browser_egress_relay.rs:391`、`tests/browser_runtime_isolated.rs:365`・`:582`・`:615`、
  `tests/browser_runtime_supervisor.rs:301`・`:551`

**E. env ではなく道具・環境の probe の失敗で return するもの（env は fail にするかだけを決める）: 2 件**
- `crates/campaign/tests/a_arm_hardening.rs:400`（`bwrap_usable()`）、`crates/task-worker/src/browser_post_login_tests.rs:1076`（`agent_browser_binary()`）

**F. impl の段で見つけた追加（走査は `crates/` だけだった）: 1 件**
- `tests/e2e/tests/worker_db_read_only.rs:148`（`a_codex_worker_run_cannot_write_the_daemon_db_but_can_read_it_with_celerisctl`）:
  `CELERIS_USERNS_TESTS` が `1` でなければ return（B と同じ自前の規則）。

提案: A（28 件）と B（7 件）は対象の 9 件と同じ「env 無しで pass と数える」問題を持つ。impl の段で作る build script の cfg と ignored の仕組みを、
別の task で crate ごとに広げる（task-worker の tests/・task-api・campaign）。C は opt-in の入力なので、同じ仕組みで ignored にするかを別に決める。
D は子 process の入口なので、ignored にするなら親の起動に `--ignored` を足す（下の 2）。

### 2. 子 process の入口と ignored の組み合わせ
試験 8 のような子の入口を `#[ignore]` にすると、`--exact <name>` で起こした子は走らない。impl の段では、子の入口を常に ignored にし、
子を起こす側（`probe_as`）に `--ignored` を足した。env の無い直接の実行は panic で fail する（ADR の D2）。D の 20 件に同じ形を広げるかは
提案 1 の task で決める。

### 3. env を読む規則の写しが 5 つある
`task-worker/src/test_support.rs:24`、`task-worker/tests/userns_gate/mod.rs:11`、`celeris-credentiald/tests/injection_peer_uid.rs:53`、
campaign の各 file の直書き、`task-api/tests/common/mod.rs:827`。優先順位と受け付ける値（`CELERIS_DB_GUARD_TESTS`・`CELERIS_LAUNCHER_TESTS`・`require`）が少しずつ違う。
この task の対象の 2 crate では build script の 1 か所（`crates/task-worker/userns_tests_cfg.rs`）に寄せた。残り
（`task-worker/tests/userns_gate/mod.rs`・task-api・campaign・e2e）は提案 1 の task で同じ file に寄せる（各 crate に `build.rs` を足し、
`#[path]` で読む）。

## 検査
### impl の段（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sh agent-docs/progress/2026-11-21-strategy-v1-1/userns-skip/check-skip.sh` → exit 0（`list: ignored` 9 行・`run: SKIP` 9 行・
  `0 tests run: 0 passed, 1234 skipped`）
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-campaign・celeris-credentiald・celerisctl・llm-proxy・task-api・
  task-dispatch・task-worker と web（typecheck+unit）。repo-health `verdict=pass`（new 0・worsened 0）、nextest
  `6238 tests run: 6238 passed (11 slow), 30 skipped`、web `Test Files 101 passed`・`Tests 705 passed`、doc test は失敗 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（出た warning は vendor の deno_core の既存の 1 件だけ）
- `cargo fmt --all -- --check` → exit 0
- `test -f agent-docs/adr/2026-11-21-userns-tests-explicit-skip.md && sh scripts/dev/check-adr-numbers.sh` → exit 0
- 計画の check 4（9 件の早期 return と対象 3 file の env の読み取りが無いこと、build script が規則の file を読むこと）→ exit 0
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → 範囲の正規表現の外の path は 0 件

### before の段（提出時の HEAD で実行）
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`（exit 0）
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 4 path（この md、`userns-skip/before-nextest.txt`・`scan-env-return.py`・`scan-env-return.tsv`）
- コードを変えていないので `test-affected.sh`・clippy・fmt はこの段では走らせていない（impl の段で走らせる）。
