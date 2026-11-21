---
title: release gate で userns の試験の本体の中の skip（試験 7 の ssh 無し・試験 9 の missing()）を PASS と数えない
tasks: [01M7YWAC1BVNG09WKDQJ6CA86Z]
status: running
updated: 2026-11-21
---
# release gate で userns の試験の本体の中の skip（試験 7 の ssh 無し・試験 9 の missing()）を PASS と数えない

完了日: 未完了（WorkUnit impl まで。record は後の WorkUnit）

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
- この変更を含む release 自身の gate は current の旧い release.sh で走り、旧い release.sh は filter を渡さない。その gate では新しい試験 9 が
  daemon の文脈（NoNewPrivs 1）で panic しうる。**この release の gate が落ちたら、原因はこの点**（旧い台本は外さない）。落ちるなら
  `SD_GATE_SKIP_HOST_TESTS` は旧い台本は読まないので、operator が login shell で release.sh を起こす（NoNewPrivs 0 なら sudo が通る）。
  未確認: 実際に落ちるか（試験 9 は env 有りの build でだけ走る。gate の build は `CELERIS_USERNS_TESTS=1` なので走る）。

## 本番への移行（人が実行する手順。record の WorkUnit が整える）
1. この変更の release が昇格した後、最初の daemon の gate の gate.json を読む:
   `python3 -I -c 'import json,sys; g=json.load(open(sys.argv[1])); print(json.dumps({k:g[k] for k in ("host_tests","gate_env")}, indent=1)); print(g["cargo_test"]["nextest_skipped"])' $CELERIS_STATE_DIR/releases/<sha12>/gate.json`
   期待: `host_tests.sudo.run=false`・`off_by="no_new_privs"`、`host_tests.ssh.run=true`、`gate_env.CELERIS_LAUNCHER_TESTS=null`、
   `nextest_skipped` が前の gate より 1 増える。release-records.jsonl の gate の行にも `host_tests` がある。
2. 試験 9 の本体は daemon の gate では走らない。credentiald を変えた release では operator が login shell で流す:
   `CELERIS_USERNS_TESTS=1 TMPDIR=/tmp cargo nextest run -p celeris-credentiald --test injection_peer_uid --no-capture --status-level all -E 'test(=real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch)'`
3. config・unit は変えない。

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
- 人の決定 `daemon-gate-no-new-privs`（daemon の gate で試験 9 をどう扱うか）。WorkUnit impl はこの答えを待つ。
- 上の 2〜4 の「未確認」の 3 項目は、人が host で command を実行して確かめる。
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
