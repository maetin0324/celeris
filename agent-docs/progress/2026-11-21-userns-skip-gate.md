---
title: release gate で userns の試験の本体の中の skip（試験 7 の ssh 無し・試験 9 の missing()）を PASS と数えない
tasks: [01M7YWAC1BVNG09WKDQJ6CA86Z, 01M7ZDW4QK6N27N7MFJPCZ13JN]
status: done
updated: 2026-11-21
---
# release gate で userns の試験の本体の中の skip（試験 7 の ssh 無し・試験 9 の missing()）を PASS と数えない

完了日: 2026-11-21（WorkUnit record まで。統合は celeris）。子 task 01M7ZDW4QK6N27N7MFJPCZ13JN（移行の 1 回目の release、下の「移行の 1 回目の release」）も 2026-11-21。

## 結論（WorkUnit adr）

- この host の本番の release gate は、人の login shell ではなく **daemon（`celeris@<sha12>.service`）が起こす**。daemon の
  delivery（`crates/celeris/src/delivery.rs` の `start_prepare`）が `current/scripts/prepare.sh` を起こし、prepare.sh が隣の
  `release.sh` を起こす。daemon とその子は `NoNewPrivs: 1` で走る（unit の雛形 `deploy/systemd/celeris@.service` の
  `NoNewPrivileges=true`）。この文脈では `sudo` は権限を上げられないので、試験 9 は毎回 `missing()` を通って return し、PASS と数えられる。
- そのため、案 (a) をそのまま入れる（release.sh が `CELERIS_LAUNCHER_TESTS=require` を立てるだけ）と、この変更を含む release が昇格した後の
  daemon の gate は、試験 9 が `sudo -n` の失敗で panic して毎回 cargo-test の段で落ちる。task の条件「この host の release gate が、この変更だけで
  落ちる状態にしない」と、人が書いた (a) の扱い「sudo の無い host では人が明示して外す」が、この host の daemon の gate でぶつかる。
- `CELERIS_LAUNCHER_TESTS=require` は試験 9 の外の 2 件（`browser_launcher_ptrace.rs` の実 launcher の session の試験と
  `browser_launcher/userns.rs` の試験）も厳しくする（下の 7）。gate で一律に立てると、host の launcher が tree より古いだけで gate が落ちうる。
- ADR [2026-11-21-release-gate-second-stage-skip](../adr/2026-11-21-release-gate-second-stage-skip.md) を書いた。選択は「(a) の目的（2 段目の前提の
  欠けを FAIL にする）を ADR-0126 B2 の規則で実現する」: cfg 有りの build では試験 7・9 の 2 段目の前提の欠けを panic にし（env を読まない）、
  その host で走れない試験は人が `SD_GATE_SKIP_HOST_TESTS` で明示して外す。外した試験は nextest の filter で skipped と数え、gate.json に残す。
  gate は `CELERIS_LAUNCHER_TESTS` を立てない。(b) の build script と (c) は却下した。
- daemon の gate（NoNewPrivs 1）の扱いは人の決定 `daemon-gate-no-new-privs` に回した（推奨は「release.sh が NoNewPrivs を読んで試験 9 だけを外し、
  理由を gate.json に残す」）。答えが出るまで WorkUnit impl は始めない（決定の `needed_before` は `impl`）。
- コードは変えていない。

## release gate が走る文脈の確認（この worker の環境で実行）

この worker の run は daemon の cgroup の中の bwrap の sandbox で走る（下の 1）。daemon 自身・走っている gate の process は `/proc` から読めた。
sandbox と gate の文脈が違う項目は「未確認」とし、人が host で実行する command を書いた。

### 1. release の build を誰が・どの unit から起こすか（確認済み）

コードの経路:
- `crates/celeris/src/delivery.rs:1000` `start_prepare` が `bash <releases_dir の親>/current/scripts/prepare.sh <head> <dir>` を spawn する
  （`SD_REPO`・`CELERIS_STATE_DIR`・`CELERIS_CONFIG` だけを足し、他の env は daemon のものを継ぐ）。
- `scripts/selfdeploy/prepare.sh` は `timeout 7200 bash "$script_dir/release.sh" "$sha"` の後に verify.sh を起こす。`$script_dir` は
  `current/scripts` なので、gate の台本は**いま current の release に梱包された release.sh**である。新しい release.sh が効くのは、その release が昇格した後の gate から。
- `docs/ops/selfdeploy.md` §2・§9: 人とワーカーも `release.sh` を直接起こしてよい（D5）。

実際の gate:
```
$ pgrep -f 'selfdeploy/release.sh|scripts/prepare.sh|dev/test-parallel.sh' … （各 pid の cmdline・PPid・NoNewPrivs・cgroup を表示）
pid=3771083 ppid=2887885 cmd=bash /local/celeris/state/current/scripts/prepare.sh 53a88490a5d96dea3b8db7fc46cd6cd7e4a03cbf /local/celeris/state/releases/.deliveries/01M7XQ9Q4P4ZQ38Y509J9XYAQ0/53a88490a5d96dea3b8db7fc46cd6cd7e4a03cbf
NoNewPrivs:	1
Seccomp:	2
0::/user.slice/user-1001.slice/user@1001.service/app.slice/app-celeris.slice/celeris@147c84783273.service
pid=3771689 ppid=3771089 cmd=bash /local/celeris/state/releases/.build/tree/scripts/dev/test-parallel.sh
NoNewPrivs:	1
Seccomp:	2
0::/user.slice/user-1001.slice/user@1001.service/app.slice/app-celeris.slice/celeris@147c84783273.service
```
- PPid 2887885 は daemon（下の 2）。子 1（01M7XQ9Q4P4ZQ38Y509J9XYAQ0）を含む main の HEAD の gate を、daemon の delivery がこの run の最中に走らせていた
  （`prepare.log` の最後の行は `13:34:43Z [release] step cargo-test: bash …/scripts/dev/test-parallel.sh`）。

過去の gate:
```
$ ls /local/celeris/state/releases/.deliveries/01M7XQ9PZQ3DQZMYKNFNPTPBQJ/*/   → prepare.log  result.json
$ head -1 …/01M7XQ9PZQ3DQZMYKNFNPTPBQJ/147c84783273a177114b6681e950b60cf58b28fd/prepare.log
2026-11-21T00:43:52Z [release] ref=147c84783273a177114b6681e950b60cf58b28fd sha=147c84783273a177114b6681e950b60cf58b28fd sha12=147c84783273
$ cat …/result.json   → {"ok":true}
```
`logs/release-records.jsonl` の gate の行と delivery の `prepare.log` を sha12 で突き合わせた（python3 -I の 1 回の読み取り）:
```
2026-11-20T17:06:45Z 7f69057907ad ok= True daemon_delivery= 01M7MZT8KX547464YYS76QNP4G userns= True
2026-11-21T01:11:35Z 147c84783273 ok= True daemon_delivery= 01M7XQ9PZQ3DQZMYKNFNPTPBQJ userns= True
gates since 11-17: 2 with delivery prepare.log: 2
```
- 記録のある gate 2 件（`release-records.jsonl` は 2026-11-20 の ADR からの記録）は、どちらも daemon の delivery が起こした。昇格（`promoted`）の
  actor は `external_operator:claude-code-ops-session` だが、gate を起こしたのは daemon である。
- 子 1 の進捗の S9・U1 の host の実行は、operator の login shell（`NoNewPrivs: 0`）で試験 9 だけを流したもので、release gate の文脈ではない。

### 2. その process の NoNewPrivs（確認済み）

```
$ for p in $(pgrep -f 'bin/celeris --config'); do tr '\0' ' ' </proc/$p/cmdline; grep -E 'NoNewPrivs|Seccomp:' /proc/$p/status; cat /proc/$p/cgroup; done
/local/celeris/state/releases/147c84783273/bin/celeris --config /home/rmaeda/.config/celeris/config.toml --release 147c84783273
NoNewPrivs:	1
Seccomp:	2
0::/user.slice/user-1001.slice/user@1001.service/app.slice/app-celeris.slice/celeris@147c84783273.service
```
- daemon・prepare.sh・test-parallel.sh のどれも `NoNewPrivs: 1`（上の 1 も同じ）。no_new_privs は子に継がれ、外せない（kernel の規則）。
- 雛形 `deploy/systemd/celeris@.service` は `NoNewPrivileges=true`。host に入っている unit の値は sandbox から読めなかった
  （`systemctl --user show …` → `Failed to connect to user scope bus via local transport: No such file or directory`）。
  process の `NoNewPrivs: 1` が実際の値なので、判断には足りる。
- 未確認（人が host で確かめる）: `systemctl --user show celeris@147c84783273.service -p NoNewPrivileges` → `NoNewPrivileges=yes` を期待。

### 3. `sudo -n -u celeris-browser true` が通るか（gate の文脈では未確認。根拠は 2）

この worker の sandbox:
```
$ id
uid=1001(rmaeda) gid=1001(rmaeda) groups=1001(rmaeda),65534(nogroup)
$ grep -E 'NoNewPrivs|Seccomp|CapEff' /proc/self/status
CapEff:	0000000000000000
NoNewPrivs:	1
Seccomp:	2
$ sudo -n -u celeris-browser true; echo "sudo rc=$?"
sudo: /etc/sudo.conf is owned by uid 65534, should be 0
sudo: The "no new privileges" flag is set, which prevents sudo from running as root.
sudo: If sudo is running in a container, you may need to adjust the container configuration to disable the flag.
sudo rc=1
$ getent passwd celeris-browser
celeris-browser:x:995:989::/var/lib/celeris-browser:/usr/sbin/nologin
```
- sandbox は user namespace の中（`/etc/sudo.conf` の owner が 65534 に見える）なので、gate の文脈と同じではない。
- gate の文脈（daemon の子、`NoNewPrivs: 1`）でも、kernel は setuid の bit を無視するので、`sudo` は root になれず exit 1 になると推定する。
  試験 9 は `missing()` を「cannot run a process as celeris-browser: sudo -n -u celeris-browser true exit=Some(1): …」の理由で通り、
  今の gate（`CELERIS_LAUNCHER_TESTS` 無し）では PASS と数えられる。
- 未確認（人が host の login shell で確かめる。daemon と同じ no_new_privs を立てて sudo を試す）:
  ```
  setpriv --no-new-privs sudo -n -u celeris-browser true; echo "rc=$?"
  ```
  期待: `rc=1` と `The "no new privileges" flag is set` の行。比較として no_new_privs を立てない
  `sudo -n -u celeris-browser true; echo "rc=$?"` は `rc=0`（operator が 2026-11-21 に login shell で確かめた。子 1 の進捗の U1）。
  daemon の unit と同じ形で確かめるなら
  `systemd-run --user --wait --pipe -p NoNewPrivileges=yes sudo -n -u celeris-browser true; echo "rc=$?"`（期待 `rc=1`）。

### 4. ssh があるか（sandbox では確認済み、gate の文脈では未確認）

```
$ command -v ssh; ls -l /usr/bin/ssh
/usr/bin/ssh
-rwxr-xr-x 1 nobody nogroup 1125464  8月 26  2025 /usr/bin/ssh
$ ls -ld /etc/ssh/ssh_config.d
drwxrwxr-x 2 rmaeda rmaeda 60 10月 17 14:10 /etc/ssh/ssh_config.d
```
- daemon の unit の `PATH` は `%h/.local/bin:%h/.cargo/bin:/usr/lib/node_modules/corepack/shims:/usr/local/bin:/usr/bin:/bin`（雛形）で、
  `/usr/bin` を含む。試験 7 の前提（`ssh -V` が起きる・`/etc/ssh/ssh_config.d` が dir）は、gate の文脈でも満たされると推定する。
- 未確認（人が host で確かめる）: `ls -l /usr/bin/ssh; ls -ld /etc/ssh/ssh_config.d`（どちらも在ることを期待）。

### 5. gate の記録から 2 段目の skip を区別できるか（確認済み: できない）

```
$ grep -c "PASS\|SKIP" /local/celeris/state/releases/147c84783273/gate-logs/cargo-test.log
0
$ tail -3 …/gate-logs/cargo-test.log   （要点）
CELERIS_TEST_SUMMARY {"runner": "nextest", … "passed": 8567, "failed": 0, "ignored": 24, "nextest_passed": 8564, "nextest_skipped": 22, … "userns": true}
test-parallel: ok
$ grep -n "status-level\|success-output\|failure-output" .config/nextest.toml
14:status-level = "fail"
15:final-status-level = "slow"
16:failure-output = "immediate-final"
```
- gate の log には落ちた試験の出力しか出ない（`status-level = "fail"`、`success-output` は既定の never）。試験 9 の `SKIP: (not passed)` の行も、
  試験 7 の `skip: no ssh` の行も残らず、件数は `passed` に入る。gate.json の `cargo_test` も同じ集計の写しである。
- userns は gate の文脈で使える（147c84783273 と 7f69057907ad の gate.json の `cargo_test.userns` は `true`）。

### 6. gate の TMPDIR と試験 9 の fixture の場所（確認済み）

- gate の cargo-test の log の行（`rm: cannot remove '/local/celeris/data/scratch/tr-1001/.trash-…/t/…'`）から、test-parallel.sh の一時 dir の root は
  `/local/celeris/data/scratch/tr-1001`（35 byte）で、TMPDIR は `<root>/XXXXXX/t` の 44 byte になる（`scripts/dev/lib/test-tmp-lifecycle.sh`）。
- 試験 9 の `short_tempdir()` は TMPDIR が 40 byte を超えると `/tmp` を使う（`crates/celeris-credentiald/tests/injection_peer_uid.rs:568`）。
  gate では fixture は `/tmp` の下にでき、`untraversable_ancestor()` の missing() には当たらない見込み（sudo が使える文脈で確かめるまでは未確認）。
  `tr-1001` は `drwx------` なので、fixture をそこに置く変更をすると、sudo が使える文脈でも broker 層が missing() になる。

### 7. `CELERIS_LAUNCHER_TESTS` を読む試験（確認済み）

```
$ grep -rn "CELERIS_LAUNCHER_TESTS" --include=*.rs crates tests | grep -v injection_peer_uid.rs
crates/celeris-credentiald/build.rs:9:        "CELERIS_LAUNCHER_TESTS",
crates/task-worker/src/browser_launcher/userns.rs:374:        if std::env::var("CELERIS_LAUNCHER_TESTS").as_deref() != Ok("require")
crates/task-worker/src/browser_launcher/userns.rs:383:            Err(e) if std::env::var("CELERIS_LAUNCHER_TESTS").as_deref() != Ok("require") => {
crates/task-worker/tests/browser_launcher_ptrace.rs:2://! host 未準備の CI では理由を表示して skip する。実証時は CELERIS_LAUNCHER_TESTS=require。
crates/task-worker/tests/browser_launcher_ptrace.rs:4://! または `CELERIS_LAUNCHER_TESTS=require` を与えた時だけ走る（ADR-0126 B）。合成 table の試験は既定で走る。
crates/task-worker/tests/browser_launcher_ptrace.rs:44:    if std::env::var("CELERIS_LAUNCHER_TESTS").as_deref() == Ok("require") {
crates/task-worker/tests/browser_launcher_ptrace.rs:403:    if std::env::var("CELERIS_LAUNCHER_TESTS").as_deref() != Ok("require")
```
- `browser_launcher_ptrace.rs` の `launcher_chrome_denies_daemon_uid_ptrace` は、require では launcher の socket が無い・subuid が無い・
  launcher の protocol の版ずれ（`launcher_unusable`）で panic する。host の launcher（root が入れる system unit）に実際の session を起こす試験である。
- `browser_launcher/userns.rs` の `real_namespace_when_host_is_ready` は、require では `create()` の失敗で panic する。
- この 2 件が今の gate で本体まで走っているか、host の launcher の状態で落ちるかは、gate の log に出ないので未確認。gate で
  `CELERIS_LAUNCHER_TESTS=require` を立てると、この 2 件の扱いも変わる。ADR はこれを理由に env を立てる形を採らなかった。

## 案 (a) をそのまま入れたときの影響（上の確認からの推論）

試験 9 の `missing()` を FAIL にする形（`CELERIS_LAUNCHER_TESTS=require` を立てる形でも、ADR の G2 の形でも同じ）で、外す仕組みが無いとき:

| gate を起こす文脈 | NoNewPrivs | sudo -n | 変更の後の試験 9 |
|---|---|---|---|
| daemon の delivery（本番の通常の経路。上の 1） | 1 | 使えない | `missing()` が panic → cargo-test の段で FAIL（毎回） |
| 人・operator の login shell で release.sh | 0 | 使える（operator が確認） | 本体が走る。probe の行が返らない等は FAIL |
| worker の run（sandbox） | 1 | 使えない | FAIL |

- release.sh は current の release の写しが走るので、この変更を含む release 自身の gate は旧い release.sh で通る。落ち始めるのは、その release が
  昇格した後の最初の daemon の gate からである。
- この表が人の決定 `daemon-gate-no-new-privs` の理由である。

## 実装（WorkUnit impl）

人の決定 `daemon-gate-no-new-privs`（CoS の回答。推奨どおり）: release.sh が NoNewPrivs 1 を読んだら試験 9 だけを外し、skipped と数え、理由を
gate.json に残す。本番の設定は変えない。ADR の G5 の推奨と同じなので ADR の本文は書き換えていない。

### ADR と WorkUnit の受け入れ条件 2 の差（要確認）
WorkUnit の受け入れ条件 2 は「release.sh が `CELERIS_LAUNCHER_TESTS` を既定 require にし」と書くが、ADR G1（Accepted）は
`CELERIS_LAUNCHER_TESTS` を立てない形を選んだ（試験 9 の外の 2 件〈`browser_launcher_ptrace.rs`・`browser_launcher/userns.rs`〉も厳しくなり、
host の launcher が古いだけで gate が落ちるため。試験 7 にはこの env が効かない）。objective の「ADR の方式で実装する」に従い ADR を採った。
同じ目的（2 段目の欠けを PASS にしない・外した値を記録に残す）は次で満たす: 試験 7・9 は env に関係なく本体の欠けで FAIL、外した値は
gate.json の `gate_env`（`CELERIS_LAUNCHER_TESTS` を含む 4 つの env の実際の値）と `host_tests` に残る。env を既定 require にしたいなら人が決める。

### 変更
- `crates/celeris-credentiald/tests/injection_peer_uid.rs`: `require()` を消し、`missing()` は常に panic（戻り値 `!`、env を読まない）。
  `return` の分岐も消した。子 process の試験 2 件（`test9_fails_instead_of_skipping_when_sudo_is_unusable`・
  `test9_fails_instead_of_skipping_when_no_probe_line_returns`）。偽の `sudo` を PATH の先頭に置き、`--include-ignored --exact` で試験 9 を子で起こす。
- `crates/task-worker/src/db_guard_tests.rs`: 判定を純関数 `ssh_prerequisite_gap(config_d_is_dir, ssh_runs) -> Option<String>` に出した。
  試験 7 は ssh の前提を userns の確認より先に見て、欠けなら panic。単体試験（純関数）と子 process の試験
  `ssh_test_fails_instead_of_skipping_when_ssh_is_missing`（PATH を空 dir にして試験 7 を子で起こす）。
- `scripts/selfdeploy/release.sh`: `SD_GATE_SKIP_HOST_TESTS`（sudo・ssh。未知の key は gate の前に sd_die）、NoNewPrivs 1 なら sudo を自動で外す
  （`SD_PROC_SELF_STATUS` は試験だけが差し替える）、nextest は `-E 'not (...)'`、cargo-test runner は `--skip <name> --exact`（組み合わせを拒まず
  同じ試験を外す）、gate.json に `host_tests`・`gate_env`、release-records.jsonl（`lib.sh`）に `host_tests`。`CELERIS_LAUNCHER_TESTS` は触らない。
- `scripts/selfdeploy/tests/release_host_tests_gate.sh`（新規）、共有 fixture `release-gate-tree.sh` は `SD_PROC_SELF_STATUS` を NoNewPrivs の行の無い
  file に固定（この worker は NoNewPrivs 1 で、固定しないと既存の `release_parallel_test_gate.sh` の cargo-test の節が落ちた）。
- 文書: `docs/ops/selfdeploy.md`、子 1 の ADR §3 に ADR への参照 1 行。
- 未知の key は gate の前（作業ツリーを作る前）に落ちるので gate.json は残らない（ADR I3 (iii) の「gate.json に残る」からの差。log と exit 1 に理由が出る）。
- 行わなかったこと: `CELERIS_USERNS_TESTS=0` のとき `host_tests.run` を false にする処理（`gate_env` で 0 が読める）。

### 失敗注入（修正を外すと試験が落ちる）
cargo は `$CARGO_TARGET_DIR`、修正を外して試験を走らせ、直したあと元に戻して再度 PASS を確認した。
| 外した修正 | 落ちた試験 | 結果 |
|---|---|---|
| 試験 9 の `missing()` を「SKIP を出して exit 0」に戻す | `test9_fails_instead_of_skipping_when_sudo_is_unusable`・`test9_fails_instead_of_skipping_when_no_probe_line_returns`（`child must FAIL, not pass`） | 6 件中 2 件 FAIL |
| 試験 7 の panic を `eprintln!("skip"); return;` に戻す | `db_guard::tests::ssh_test_fails_instead_of_skipping_when_ssh_is_missing` | 9 件中 1 件 FAIL |
| release.sh の nextest への ` -E "not (…)"` を消す | `release_host_tests_gate.sh`: `FAIL: filter missing/wrong: nextest run --workspace … --test-threads 12` | rc=1 |
| release.sh の NoNewPrivs の分岐を `if false &&` にする | 同: `FAIL: NoNewPrivs 1 filter wrong` | rc=1 |
| release.sh の `gate_env` の行を消す | 同: `FAIL: gate_env wrong` | rc=1 |
元に戻した後: `cargo nextest run -p celeris-credentiald --test injection_peer_uid` 6 passed・2 skipped（ignored 2 件は env 無しの試験 9 と helper）、
`cargo nextest run -p task-worker --lib -E 'test(ssh_)'` 9 passed、`release_host_tests_gate.sh` ok。

### この host の gate が落ちない根拠（ADR I5）
- daemon の gate: NoNewPrivs 1・ssh あり（上の 2・4。ssh の gate 文脈は未確認）→ release.sh が試験 9 だけを外す（`host_tests.sudo.off_by = "no_new_privs"`）。
  試験 7 は本体が走り、ssh が在れば今と同じく PASS。`release_host_tests_gate.sh` の NoNewPrivs 1 の節がこの分岐を決定的に示す。
- この変更を含む release 自身の gate は current の旧い release.sh で走り、旧い release.sh は filter を渡さない。impl の段の形のままだと、その gate で
  試験 9 が daemon の文脈（NoNewPrivs 1）で panic して落ちる。子 task 01M7ZDW4QK6N27N7MFJPCZ13JN が ADR G7 で直した（下の「移行の 1 回目の release」）。

## 移行の 1 回目の release（子 task 01M7ZDW4QK6N27N7MFJPCZ13JN）

人の決定 `first-release`（CoS の回答 `opt-in-env`。推奨どおり）に従い案 a を入れた。ADR に G7・I6・I7 を足した。

### 実装前 brief
1. 根本原因: delivery の prepare.sh（`scripts/selfdeploy/prepare.sh:19`）は current の release に梱包された release.sh を起こす。impl の段は
   試験 7・9 の厳格化を試験の中だけで常に有効にし、外す手段（G4・G5）を新しい release.sh にだけ置いた。旧い release.sh で走る最初の gate には
   外す手段が無く、試験 9 が NoNewPrivs 1 で panic する。症状は最初の release の gate の cargo-test の段に出る。
2. 責務 owner: FAIL にするかの判定は試験 7・9 の本体（`db_guard_tests.rs` と `injection_peer_uid.rs` の `second_stage_strict()`）。gate がそれを
   有効にするのは `scripts/selfdeploy/release.sh`。
3. 既存の再利用先: impl の段の子 process の試験（偽の sudo・空の PATH）、`release_host_tests_gate.sh` と共有の fixture の偽の cargo、
   gate.json の `gate_env`。
4. 選択肢: 案 a（新しい release.sh だけが立てる env で厳格化を有効にする）と案 b（コードを変えず 1 回目だけ operator が手で gate を通す）。
   人の決定で a。env の名前は `CELERIS_LAUNCHER_TESTS` と分けた（G1 を変えない。試験 9 の外の 2 件を厳しくしない）。
5. 新しい状態・分岐: 実行時に読む env `CELERIS_SECOND_STAGE_TESTS` と、env が無いときの旧い挙動の分岐。一時の例外で、撤去の条件と責任者は ADR G7。
   fixture の偽の cargo に `CARGO_ENV_LOG`（試験だけが使う）。
6. 撤去対象: 無し（impl の段の「常に FAIL」を env 付きにした）。旧い挙動の分岐は G7 の撤去の follow-up で消す（下の提案 4）。
7. 不変条件: env 無し・NoNewPrivs 1 で試験 7・9 は FAIL しない／env 有りで欠けは FAIL／新しい release.sh は env を必ず立て、NoNewPrivs 1 なら
   試験 9 を filter で外して gate.json に残す。試験は下。

### 変更
- `crates/celeris-credentiald/tests/injection_peer_uid.rs`: `second_stage_strict()`（`CELERIS_SECOND_STAGE_TESTS=require` か
  `CELERIS_LAUNCHER_TESTS=require`）。`missing()` は strict なら panic、そうでなければ `SKIP: (not passed) …` を出し、呼び出し側が return する
  （前の変更より前の挙動）。子 process の試験は env を明示して起こす（`GateEnv`）。新しい試験
  `test9_keeps_old_behaviour_without_the_gate_env_under_no_new_privs`: 子に `PR_SET_NO_NEW_PRIVS` を立て、自分の NoNewPrivs が 1 なら本物の
  sudo と同じ文言で exit 1 する偽の `sudo` で試験 9 を起こす。env 無しなら PASS・`SKIP: (not passed)`、`CELERIS_SECOND_STAGE_TESTS=require` なら FAIL。
- `crates/task-worker/src/db_guard_tests.rs`: `second_stage_strict()`（`CELERIS_SECOND_STAGE_TESTS=require` だけ。I4 の 3 つの env は読まない）。
  試験 7 は strict なら panic、そうでなければ `skip: <理由>` を出して return。新しい試験 `ssh_test_keeps_old_behaviour_without_the_gate_env`。
- `scripts/selfdeploy/release.sh`: `export CELERIS_SECOND_STAGE_TESTS=require`（呼び出し元の値を使わない）、`gate_env` に 5 つ目の欄。
- `scripts/selfdeploy/tests/release_host_tests_gate.sh`・`fixtures/release-gate-tree.sh`: 偽の cargo が `CARGO_ENV_LOG` に env の値を残し、
  NoNewPrivs 0・1 と呼び出し元が `0` を立てた場合のそれぞれで、nextest run を起こした cargo が `require` を見たかを確かめる。
- 文書: ADR G7・I6・I7・7 節、`docs/ops/selfdeploy.md`。

### 決定的な試験の結果（この worker の sandbox。NoNewPrivs 1）
```
$ cargo nextest run -p celeris-credentiald --test injection_peer_uid
     Summary [   0.023s] 7 tests run: 7 passed, 2 skipped
$ cargo nextest run -p task-worker --lib -E 'test(ssh_)'
     Summary [   2.030s] 10 tests run: 10 passed, 1225 skipped
$ CELERIS_SECOND_STAGE_TESTS=require cargo nextest run -p task-worker --lib -E 'test(ssh_)'     # 新しい gate の親の env
     Summary [   2.030s] 10 tests run: 10 passed, 1225 skipped
$ bash scripts/selfdeploy/tests/release_host_tests_gate.sh
release_host_tests_gate: ok
```
gate と同じ cfg（`CELERIS_USERNS_TESTS=1`）の build で、試験 9 そのものを gate の 3 つの形で流した（`T9='test(=real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch)'`）:
```
$ grep NoNewPrivs /proc/self/status
NoNewPrivs:	1
# 旧い台本相当（strict の env 無し、NoNewPrivs 1）
$ env -u CELERIS_SECOND_STAGE_TESTS -u CELERIS_LAUNCHER_TESTS CELERIS_USERNS_TESTS=1 cargo nextest run -p celeris-credentiald --test injection_peer_uid --no-capture --status-level all -E "$T9"
        PASS [   0.008s] (1/1) celeris-credentiald::injection_peer_uid real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch
SKIP: (not passed) cannot run a process as celeris-browser: `sudo -n -u celeris-browser true` exit=Some(1): sudo: /etc/sudo.conf is owned by uid 65534, should be 0
     Summary [   0.008s] 1 test run: 1 passed, 8 skipped        → rc=0
# 新しい台本相当で filter が無いとき（+ CELERIS_SECOND_STAGE_TESTS=require）
        FAIL [   0.011s] (1/1) … real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch
peer-uid real-process test environment required: cannot run a process as celeris-browser: `sudo -n -u celeris-browser true` exit=Some(1): …
     Summary [   0.012s] 1 test run: 0 passed, 1 failed, 8 skipped   → rc=100
# 新しい台本相当（NoNewPrivs 1 の filter `-E "not (package(celeris-credentiald) & $T9)"`、全件）
     Summary [   0.022s] 7 tests run: 7 passed, 2 skipped        → rc=0
```
- 試験 7 の本体を cfg 有りで流す確認は、この sandbox では userns が使えない（`assert_userns_available()` が落ちる）ので行っていない。
  子 process の試験（ssh の無い PATH）で、env 無しは PASS・env 有りは FAIL を確かめた。

### 失敗注入（修正を外すと試験が落ちる。各注入の後に元へ戻し、上の PASS を再確認した）
| 外した修正 | 落ちた試験 | 結果 |
|---|---|---|
| 試験 9 の `second_stage_strict()` を常に true（env に依らず厳格） | `test9_keeps_old_behaviour_without_the_gate_env_under_no_new_privs`（`old release.sh context must not FAIL`） | 7 件中 1 件 FAIL |
| 試験 7 の `second_stage_strict()` を常に true | `db_guard::tests::ssh_test_keeps_old_behaviour_without_the_gate_env` | 10 件中 1 件 FAIL |
| 試験 9 の `second_stage_strict()` を常に false（旧い挙動だけ） | `test9_fails_instead_of_skipping_when_sudo_is_unusable`・`…_when_no_probe_line_returns`・`test9_keeps_old_behaviour_…`（strict の子） | 7 件中 3 件 FAIL |
| 試験 7 の `second_stage_strict()` を常に false（`false && …`） | `db_guard::tests::ssh_test_fails_instead_of_skipping_when_ssh_is_missing` | 10 件中 1 件 FAIL |
| release.sh の `export CELERIS_SECOND_STAGE_TESTS=require` を消す | `release_host_tests_gate.sh`: `FAIL: gate_env wrong: {… 'CELERIS_SECOND_STAGE_TESTS': None …}` | rc=1 |
| release.sh を `${CELERIS_SECOND_STAGE_TESTS:-require}`（呼び出し元の値を使う）にする | 同: `FAIL: caller's CELERIS_SECOND_STAGE_TESTS overrode the gate` | rc=1 |
- 試験 7 の「常に false」の最初の注入は `false || …` と書き、意味が変わらず 10 件とも PASS した（注入の誤り）。`false && …` で取り直した結果が上の行。

### gate の文脈の確認（この worker の sandbox で実行。読み取りだけ）
```
$ setpriv --version
setpriv from util-linux 2.40.2
$ grep -E 'NoNewPrivs|Seccomp:' /proc/self/status
NoNewPrivs:	1
Seccomp:	2
$ setpriv --no-new-privs sudo -n -u celeris-browser true; echo "rc=$?"
sudo: /etc/sudo.conf is owned by uid 65534, should be 0
sudo: The "no new privileges" flag is set, which prevents sudo from running as root.
sudo: If sudo is running in a container, you may need to adjust the container configuration to disable the flag.
rc=1
$ setpriv --no-new-privs grep NoNewPrivs /proc/self/status
NoNewPrivs:	1
$ grep -n 'NoNewPrivileges\|^Environment\|^ExecStart' ~/.config/systemd/user/celeris@.service
13:Environment=CELERIS_STATE_DIR=/local/celeris/state
14:EnvironmentFile=-%h/.config/celeris/paths.env
19:Environment=PATH=%h/.local/bin:%h/.cargo/bin:/usr/lib/node_modules/corepack/shims:/usr/local/bin:/usr/bin:/bin
20:ExecStart=/local/celeris/state/releases/%i/bin/celeris --config %h/.config/celeris/config.toml --release %i
30:NoNewPrivileges=true
$ ls ~/.config/systemd/user/celeris@.service.d/      → 50-credentiald.conf（中身は [Unit] Wants=celeris-credentiald@%i.service だけ。NoNewPrivileges の行は無い）
$ for p in $(pgrep -f 'bin/celeris --config'); do …; done   （daemon の行）
/local/celeris/state/releases/56f1f8d3ee4c/bin/celeris --config /home/rmaeda/.config/celeris/config.toml --release 56f1f8d3ee4c
NoNewPrivs:	1
0::/user.slice/user-1001.slice/user@1001.service/app.slice/app-celeris.slice/celeris@56f1f8d3ee4c.service
$ systemctl --user show 'celeris@*.service' -p Id -p NoNewPrivileges
Failed to connect to user scope bus via local transport: No such file or directory
$ readlink -f /local/celeris/state/current
/local/celeris/state/releases/56f1f8d3ee4c
$ grep -c 'SECOND_STAGE\|SD_GATE_SKIP_HOST_TESTS\|CELERIS_LAUNCHER_TESTS' /local/celeris/state/current/scripts/release.sh
0
$ grep -n 'CELERIS_USERNS_TESTS=\|CELERIS_ISOLATION_TESTS=' /local/celeris/state/current/scripts/release.sh
177:export CELERIS_ISOLATION_TESTS="${CELERIS_ISOLATION_TESTS:-require}"
182:export CELERIS_USERNS_TESTS="${CELERIS_USERNS_TESTS:-1}"
$ grep -n 'release.sh' /local/celeris/state/current/scripts/prepare.sh
19:timeout --signal=TERM --kill-after=30s 7200 bash "$script_dir/release.sh" "$sha"
$ grep -cE '^(export )?(CELERIS_LAUNCHER_TESTS|CELERIS_SECOND_STAGE_TESTS|CELERIS_USERNS_TESTS|SD_GATE_SKIP_HOST_TESTS)=' ~/.config/celeris/paths.env
0
$ grep -cE 'CELERIS_LAUNCHER_TESTS|CELERIS_SECOND_STAGE_TESTS|SD_GATE_SKIP_HOST_TESTS' ~/.config/systemd/user/celeris@.service ~/.config/systemd/user/celeris@.service.d/50-credentiald.conf
…celeris@.service:0
…50-credentiald.conf:0
$ tr '\0' '\n' </proc/812926/environ
/proc/812926/environ: Permission denied
```
- 確認できたこと: (1) 入っている unit の file は `NoNewPrivileges=true`（30 行目）で、drop-in は上書きしていない。走っている daemon（release 56f1f8d3ee4c）は
  `NoNewPrivs: 1`。(2) `setpriv --no-new-privs` の下で `sudo -n -u celeris-browser true` は exit 1 と `The "no new privileges" flag is set`。
  (3) current の release.sh（56f1f8d3ee4c）は `CELERIS_SECOND_STAGE_TESTS`・`SD_GATE_SKIP_HOST_TESTS`・`CELERIS_LAUNCHER_TESTS` を立てない。
  unit・drop-in・`paths.env` にもこれらの env は無い。したがって、この変更を含む最初の gate では試験 7・9 は旧い挙動で走る。
- 未確認: (a) この sandbox は user namespace の中で、親がすでに NoNewPrivs 1 なので、`setpriv --no-new-privs` は状態を変えていない
  （`/etc/sudo.conf` の owner が 65534 に見える）。NoNewPrivs 0 の login shell から `setpriv --no-new-privs` を立てたときの sudo の挙動は、
  人が host で `setpriv --no-new-privs sudo -n -u celeris-browser true; echo rc=$?`（期待 `rc=1`）を実行して確かめる。
  (b) systemd が解釈した値（`systemctl --user show celeris@56f1f8d3ee4c.service -p NoNewPrivileges`、期待 `NoNewPrivileges=yes`）は user bus に
  届かず未確認。unit の file と daemon の `NoNewPrivs: 1` が根拠。(c) daemon の process の env は読めなかった（Permission denied）。env を足す元
  （unit・drop-in・`paths.env`）に無いことだけを確かめた。

## 本番への移行（人が実行する手順）

設定・unit・本番 DB は変えない。この変更の release が昇格した後の、最初の daemon の gate で次を確かめる。
`<sha12>` は昇格した release の sha12、`$CELERIS_STATE_DIR` は本番の state dir（`/local/celeris/state`）。

1. 1 回目の gate（この変更を含む release 自身の gate）: current の旧い release.sh が走り、`CELERIS_SECOND_STAGE_TESTS` を立てないので、
   試験 7・9 は旧い挙動（2 段目の欠けは理由を出して PASS）で通る（ADR G7）。operator の手の操作は要らない。その gate.json には `host_tests`・
   `gate_env` が無い（旧い台本は書かない）。cargo-test の段で試験 9 が `environment required` で落ちたら、G7 が効いていない（daemon の env に
   `CELERIS_SECOND_STAGE_TESTS=require` か `CELERIS_LAUNCHER_TESTS=require` がある）ので、`prepare.log` と daemon の env の元を確かめる。
2. 昇格後の最初の daemon の gate（新しい release.sh が current になった後）の gate.json を読む:
   ```
   python3 -I -c 'import json,sys; g=json.load(open(sys.argv[1])); print(json.dumps({k:g[k] for k in ("host_tests","gate_env")}, indent=1)); print(g["cargo_test"]["nextest_skipped"])' $CELERIS_STATE_DIR/releases/<sha12>/gate.json
   ```
   期待: `host_tests.sudo.run=false`・`off_by="no_new_privs"`、`host_tests.ssh.run=true`（試験 7 が本体まで走った）、
   `gate_env.CELERIS_SECOND_STAGE_TESTS="require"`（試験 7・9 が strict で走った）、`gate_env.CELERIS_LAUNCHER_TESTS=null`、
   `nextest_skipped` が前の gate より 1 増える（試験の数が変わらないとき）。これを確かめたら operator は G7 の撤去の follow-up を起こす（提案 4）。`host_tests.sudo.run` が true なら、
   gate が NoNewPrivs を読めていない。`ssh.run=false` なら外した理由（`off_by`・`reason`）を読む。
3. release-records.jsonl の gate の行にも同じ `host_tests` がある:
   `tail -1 $CELERIS_STATE_DIR/logs/release-records.jsonl | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("host_tests"))'`
4. sudo・ssh が無い host で release するとき: 人が外す試験を明示する。`SD_GATE_SKIP_HOST_TESTS=sudo,ssh bash scripts/selfdeploy/release.sh <sha>`
   （`sudo` = 試験 9、`ssh` = 試験 7。comma 区切り。未知の key は gate の前に落ちる）。ssh が無いのに外さないと、試験 7 が panic して gate が落ちる
   （PASS と数えない）。外した試験は nextest の filter で skipped と数え、gate.json の `host_tests.<key>.off_by="SD_GATE_SKIP_HOST_TESTS"` と
   `reason` に残る。2 の command で見る。`CELERIS_USERNS_TESTS=0`（ADR-0126 B4）で userns の試験全体を外す場合は `gate_env.CELERIS_USERNS_TESTS="0"` で読める。
5. 試験 9 の本体は daemon の gate では走らない。credentiald を変えた release では operator が login shell で流す（G7 の撤去までは
   `CELERIS_SECOND_STAGE_TESTS=require` を付ける。付けないと欠けが PASS と数えられる）:
   `CELERIS_USERNS_TESTS=1 CELERIS_SECOND_STAGE_TESTS=require TMPDIR=/tmp cargo nextest run -p celeris-credentiald --test injection_peer_uid --no-capture --status-level all -E 'test(=real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch)'`
6. 「未確認」の項目は host で次を実行して確かめる（unit の file の `NoNewPrivileges=true` と、NoNewPrivs 1 の sandbox での `setpriv --no-new-privs` の下の
   sudo の exit 1 は、子 task 01M7ZDW4QK6N27N7MFJPCZ13JN が確かめた。systemd が解釈した値と、NoNewPrivs 0 から立てたときの sudo は未確認）:
   `systemctl --user show celeris@<sha12>.service -p NoNewPrivileges`（期待 `yes`）、
   `setpriv --no-new-privs sudo -n -u celeris-browser true; echo rc=$?`（期待 `rc=1`）、`ls -l /usr/bin/ssh; ls -ld /etc/ssh/ssh_config.d`。

## 未解決（impl の段で追加）
- 人に聞く問い: (a) gate 自身で試験 9 を走らせたい場合の daemon の環境の変更（`SD_GATE_SKIP_HOST_TESTS` や unit の変更）、(b) release を operator が手で起こす運用への切り替え。
- 上の「ADR と受け入れ条件 2 の差」: `CELERIS_LAUNCHER_TESTS` を gate で既定 require にするかは人が決める。

## 検査（impl の段。提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0（6266 tests run: 6266 passed, 30 skipped。repo-health verdict=pass new=0 worsened=0、script bundle SKIP 0 件）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- shell の試験: `release_host_tests_gate.sh`・`release_parallel_test_gate.sh`・`release_record_summary_survives_prune.sh`・`release_web_required.sh`・
  `release_target_settled_after_build.sh`・`browser_ledger_release_stages.sh` → すべて exit 0。release.sh・verify.sh・promote.sh の本番での実行はしていない。

## 変更
- `selfdeploy userns-skip-gate adr: release gate の実行文脈の確認と 2 段目の skip の扱いを決める ADR を書く` — この md と
  ADR `agent-docs/adr/2026-11-21-release-gate-second-stage-skip.md` を足した。コードは変えていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 下の「検査」に書く。

## 未解決
- 人の決定 `daemon-gate-no-new-privs` は CoS が推奨どおり回答済み（impl で反映）。
- 人の決定 `first-release` は CoS が `opt-in-env` で回答済み（子 task 01M7ZDW4QK6N27N7MFJPCZ13JN で反映。ADR G7）。
- 「未確認」の項目（systemd が解釈した `NoNewPrivileges` の値、NoNewPrivs 0 から `setpriv --no-new-privs` を立てたときの sudo、gate の文脈の
  `/usr/bin/ssh`、daemon の process の env）は、人が host で command を実行して確かめる（本番への移行 6）。
- G7 の移行の間は、strict の env を立てない手元の実行（`CELERIS_USERNS_TESTS=1` だけ）で試験 7・9 の 2 段目の欠けが PASS と数えられる（ADR 7 節）。
- daemon の gate（NoNewPrivs 1）では、どの選択肢でも試験 9 の本体は走らない。U1 のような試験 9 の読み取りの不具合は、NoNewPrivs 0 の文脈
  （人・operator の login shell）で試験 9 を流したときにしか見つからない（ADR の「残る穴」）。

## 提案
1. **本番への移行（WorkUnit record に渡す）**: 昇格後の最初の daemon の gate の gate.json で、`host_tests` の `sudo` と `ssh` の欄
   （ADR D4）と `cargo_test` の skipped の件数を確かめる。試験 9 が外れたなら `off_by` と理由が記録され、`nextest_skipped` が 1 増えていること、
   試験 7 が走ったこと（`ssh.run: true`）を見る。command は ADR の「試験」と impl の結果に合わせて record が書く。
2. **試験 9 の本体を release ごとに確かめる場所**: daemon の gate では走らないので、credentiald を変えた release では operator が login shell で
   `CELERIS_USERNS_TESTS=1 CELERIS_LAUNCHER_TESTS=require TMPDIR=/tmp cargo nextest run -p celeris-credentiald --test injection_peer_uid --no-capture --status-level all -E 'test(=real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch)'`
   を流す、という運用にするかを別に決める（この task の範囲外）。
3. **子 1 の ADR §3 への参照**: impl の WorkUnit が、[ADR 2026-11-21-userns-tests-explicit-skip](../adr/2026-11-21-userns-tests-explicit-skip.md) §3 の
   「残る穴」の末尾に、この ADR で gate の扱いを決めたことへの 1 行の参照を足す（D1 の本文は変えない）。
4. **G7 の旧挙動の分岐の撤去（follow-up。子 task 01M7ZDW4QK6N27N7MFJPCZ13JN が提案。起票は CoS が取り込み後に判断する）**:
   条件は「G7 を含む release が昇格した後、その release.sh で走った最初の release の gate.json で `gate_env.CELERIS_SECOND_STAGE_TESTS="require"`・
   `host_tests.sudo`（daemon の gate なら `run: false`・`off_by: "no_new_privs"`）・`host_tests.ssh.run: true` を operator が確かめた後」。責任者は operator。
   消すもの: `injection_peer_uid.rs`・`db_guard_tests.rs` の `second_stage_strict()` と env が無いときの return、試験 9 の `CELERIS_LAUNCHER_TESTS` の読み取り、
   試験 `test9_keeps_old_behaviour_without_the_gate_env_under_no_new_privs`・`ssh_test_keeps_old_behaviour_without_the_gate_env`、
   release.sh の `export CELERIS_SECOND_STAGE_TESTS=require`（`gate_env` の欄を残すかは撤去の task が決める）、ADR の G7 への注記、本番への移行 5 の env。
   受け入れ条件の案: 撤去後、strict の env を立てない子 process でも試験 7・9 の 2 段目の欠けが FAIL する（I1・I2 の子から env を消して通る）。

## 検査
### adr の段（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0（19.5 秒）。`test-affected: 試験対象の変更なし（docs のみ等）`、
  `repo-health: verdict=pass exit=0 … new=0 worsened=0`。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。`sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）。
  `sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (474 files)`）。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → この md と ADR の 2 path だけ。
- test-affected・clippy・fmt は commit「selfdeploy userns-skip-gate adr: release gate の実行文脈の確認と 2 段目の skip の扱いを決める ADR を書く」の HEAD で実行した。
  その後の commit はこの節を足しただけで、文書の検査（progress-index・doc-links・adr-numbers・範囲）はその後に再実行した。

### record の段（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`。この段はコードを変えていない）
- `bash scripts/dev/test-affected.sh` → exit 0（`test-affected: 試験対象の変更なし（docs のみ等）`、repo_health_gate: all checks passed）。
  コードの試験は impl の段で 6266 passed を確認済み。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- commit: 「selfdeploy userns-skip-gate record: 進捗の本番への移行と提出時の検査を書く」（この段）。最後の同期の後の再実行は integrate-record が行う。
