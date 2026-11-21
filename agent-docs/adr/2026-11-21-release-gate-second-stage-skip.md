---
tasks: [01M7YWAC1BVNG09WKDQJ6CA86Z, 01M7ZDW4QK6N27N7MFJPCZ13JN]
---
# ADR 2026-11-21: release gate は userns の試験の本体の中の 2 段目の skip を PASS と数えない

- 日付: 2026-11-21
- 状態: Accepted（G1〜G6 は確定。G5 は人の決定 `daemon-gate-no-new-privs` の CoS の回答〈推奨どおり〉で確定。
  G7〈移行の 1 回目の release〉は task 01M7ZDW4QK6N27N7MFJPCZ13JN が人の決定 `first-release` の CoS の回答〈opt-in-env〉で足した。
  G7 は一時の例外で、task 01M7ZYFB4YSJHWR7SF97G501S8 が 2026-11-21 に撤去した〈G7 の節に記録〉）
- task: 01M7YWAC1BVNG09WKDQJ6CA86Z（戦略層 v1 子 1 の第一ゲートの U2。operator が起票）
- 進捗: [2026-11-21-userns-skip-gate.md](../progress/2026-11-21-userns-skip-gate.md)（gate の実行文脈の確認の command と出力）
- 関係: [ADR-0126](0126-test-daemons-without-userns.md) B2・B3・B4、
  [ADR 2026-11-21-userns-tests-explicit-skip](2026-11-21-userns-tests-explicit-skip.md)（子 1。D1・§3）、
  [ADR-0051](0051-supervised-delivery.md)（delivery の prepare）、[ADR 2026-10-20-full-tests-at-release-only](2026-10-20-full-tests-at-release-only.md)、
  `docs/ops/selfdeploy.md`
- 範囲: `scripts/selfdeploy/release.sh`（高 risk の class `selfdeploy`）、`crates/celeris-credentiald/tests/injection_peer_uid.rs`（class `credential`）、
  `crates/task-worker/src/db_guard_tests.rs`（class `sandbox`）の試験の file、`scripts/selfdeploy/tests/` の試験。production の Rust のコード経路・
  config の欄・API・DB は変えない。この ADR を設計 brief とする。

## 1. 事象

- `scripts/selfdeploy/release.sh` は `CELERIS_USERNS_TESTS=1` を既定で立てる（ADR-0126 B4）。子 1 の ADR D1 により、この build では userns の
  試験 9 件のうち 8 件が普通の試験として本体に入る。
- 本体に入った後の 2 段目の skip は return して PASS と数えられる。
  - 試験 7 `db_guard::tests::ssh_config_includes_still_parse_inside_the_namespace`: `ssh -V` が起きない・`/etc/ssh/ssh_config.d` が無いと
    `skip: no ssh …` を出して return する。
  - 試験 9 `real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch`（celeris-credentiald の `tests/injection_peer_uid.rs`）:
    user が無い・`sudo -n -u <user> true` が通らない・fixture の祖先 dir を他 UID が辿れない・子の `PEER-UID-PROBE` の行が返らないと、
    `missing()` が `SKIP: (not passed) …` を出して return する。`missing()` が panic するのは `CELERIS_LAUNCHER_TESTS=require` のときだけで、
    gate はこれを立てない。
- gate の log（`gate-logs/cargo-test.log`）は落ちた試験の出力しか残さない（`.config/nextest.toml` の `status-level = "fail"`）。gate.json の
  `cargo_test` も件数の写しなので、2 段目の skip は記録から区別できない。
- 実例: 2026-11-21 に operator が login shell（`NoNewPrivs: 0`）で試験 9 を `CELERIS_LAUNCHER_TESTS=require` 付きで流すと、子の行の読み取りの
  不具合で FAIL した。gate の設定では同じ失敗が `missing()` を通って PASS と数えられる（子 1 の進捗 U1・U2。読み取りは子 1 の replan で直した）。

## 2. gate が走る文脈（進捗の確認の要約）

| 項目 | 結果 | 根拠 |
|---|---|---|
| gate を起こすもの | 本番の通常の経路は daemon の delivery。`delivery.rs` の `start_prepare` → `current/scripts/prepare.sh` → `current/scripts/release.sh` | 記録のある gate 2 件（7f69057907ad・147c84783273）は両方 delivery の `prepare.log` を持つ。この run の最中にも daemon を親とする prepare.sh が子 1 を含む main の gate を走らせていた |
| unit | `celeris@<sha12>.service`（daemon の cgroup） | 走っている prepare.sh・test-parallel.sh の `/proc/<pid>/cgroup` |
| NoNewPrivs | 1（daemon・prepare.sh・test-parallel.sh） | `/proc/<pid>/status`。雛形 `deploy/systemd/celeris@.service` の `NoNewPrivileges=true` |
| `sudo -n -u celeris-browser true` | 使えない（推定。no_new_privs の下では kernel が setuid を無視する） | sandbox では exit 1・`The "no new privileges" flag is set`。gate の文脈の実行は未確認（人の command は進捗） |
| ssh | ある見込み（`/usr/bin/ssh`・`/etc/ssh/ssh_config.d`、unit の PATH は `/usr/bin` を含む） | sandbox で確認。gate の文脈は未確認 |
| userns | 使える | gate.json の `cargo_test.userns: true` |
| 人・operator が login shell で release.sh を起こす | NoNewPrivs 0、`sudo -n` が通る（operator が 2026-11-21 に確認） | 子 1 の進捗 U1 |

daemon は unit の設計として no_new_privs を立てている。そのため、本番の通常の gate では試験 9 の本体は**どの方式でも走れない**。

## 3. 実装前 brief

### 3.1 根本原因
2 段目の前提（ssh・sudo・probe の行）の欠けを「試験の中の return」で表している。return した試験は harness が pass と数える。
ADR-0126 B2 は「`CELERIS_USERNS_TESTS=1` で環境が無ければ（道具が無い等）skip ではなく fail」と決めたが、試験 7 と試験 9 はこの規則に
従っていない（試験 9 は別の env `CELERIS_LAUNCHER_TESTS=require` のときだけ fail）。症状は gate の件数（passed）に出る。

### 3.2 責務 owner
- 環境が欠けたときに fail にするか: 各試験の本体（試験 7 の ssh の確認、試験 9 の `missing()`）。
- その host で走らせられない試験を gate から外すこと: gate（`scripts/selfdeploy/release.sh`）。外した事実を gate.json に書くのも release.sh。

### 3.3 既存の再利用先
- 子 1 の D1 の cfg `celeris_userns_tests`（gate では `CELERIS_USERNS_TESTS=1` で既に有効）。cfg を読む条件は変えない。
- test-parallel.sh は追加の引数を `cargo nextest run` だけに渡す（`scripts/dev/test-parallel.sh` の準備の節）。nextest は filter
  （`-E`）で外した試験を `skipped` と数え、`CELERIS_TEST_SUMMARY` の `nextest_skipped` に入る。
- release.sh の `skip_step`・gate.json の `steps[].skipped`・`reason`（段を飛ばしたことを残す既存の形）。
- 偽の runner（`SD_TEST_PARALLEL`）で release.sh を試す既存の試験（`scripts/selfdeploy/tests/release_parallel_test_gate.sh` など）。

### 3.4 選択肢
- **(a) gate が `CELERIS_LAUNCHER_TESTS=require` も立てる**: 試験 9 の `missing()` は panic になる。しかし同じ env を読む試験が他に 2 件ある:
  `crates/task-worker/tests/browser_launcher_ptrace.rs` の `launcher_chrome_denies_daemon_uid_ptrace`（host の launcher に実際の Chrome の session を
  起こす。launcher の protocol の版ずれ・socket が無い・subuid が無いと、require では panic）と、`crates/task-worker/src/browser_launcher/userns.rs` の
  `real_namespace_when_host_is_ready`。gate でこの env を立てると、この 2 件も厳しくなり、host の launcher が tree より古いだけで gate が落ちる。
  影響が task の範囲（試験 7・9）の外に出る。また試験 7 には効かない（task-worker の試験 7 はこの env を読まない）。**env を立てる形は採らない。**
- **(b) 2 段目の前提を試験の外に出す**:
  - build script で sudo・ssh を probe して cfg にする: cargo は host の状態の変化を `rerun-if-*` で追えず、共有の release の target に古い cfg が残る。
    build の中で sudo を起こすことになり、子 1 の D1 の規則の file も変わる。**採らない。**
  - nextest の filter: gate が前提の欠けを probe して自動で外すと、NoNewPrivs 0 の host で sudoers の設定が消えたときも黙って外れる（今の穴と同じ）。
    **自動で外す手段としては採らない。人が明示して外すときの手段としてだけ使う（G4）。**
- **(c) 試験 9 の PEER-UID の行と試験 7 の判定を gate の log・gate.json に残し、gate が数える**: 特定の試験の成功時の出力を捕まえる設定
  （`success-output`）と、試験と gate の間の文字列の契約が要る。件数（passed）の意味は変わらず、症状の側の集計になる（子 1 の ADR §1.4 の案 2 と
  同じ理由で却下）。試験 7 には出す行が無い。**採らない。**
- **（採用）(a) の目的を ADR-0126 B2 の規則で実現する**: cfg 有りの build（＝gate）では、試験 7・試験 9 の 2 段目の前提の欠けを fail にする
  （env を読まない）。その host で走れない試験は、gate が nextest の filter で明示的に外し、skipped と数えて gate.json に残す
  （(a) の「人が明示して外し gate の記録に残す」と同じ形。手段は (b) の filter）。

### 3.5 新しい状態・分岐・fallback・port
- release.sh の新しい入力 `SD_GATE_SKIP_HOST_TESTS`（人が明示する除外。G4）と、gate.json の新しい欄 `host_tests`・`gate_env`（G6）。
- G5 の推奨を採るなら、release.sh が自分の `NoNewPrivs` を読む分岐 1 つ（試験用に読む file を差し替える口を 1 つ）。
- production の Rust のコード・config・API・DB には足さない。

### 3.6 撤去対象
- 試験 9 の `require()`（`CELERIS_LAUNCHER_TESTS` を実行時に読む関数）と、`missing()` の「require でなければ `SKIP: (not passed)` を出して return」の分岐。
  `missing()` は常に panic する（関数名と呼び出し元は impl が整える）。冒頭の doc の「sudo が使えない…環境では理由を出して skip する」の説明も直す。
- 試験 7 の `eprintln!("skip: no ssh …"); return;`。
- 子 1 の ADR §3 の「残る穴」のうち、gate の試験 7・9 の部分（impl が §3 の末尾にこの ADR への参照を 1 行足す。D1 の本文は変えない）。

### 3.7 不変条件と確かめる試験
5 節に書く。

## 4. 決定

### G1. 方式
3.4 の「採用」のとおり。gate は `CELERIS_LAUNCHER_TESTS` を立てない（release.sh は今と同じく、この env を触らない）。

### G2. 試験 9（celeris-credentiald）
- cfg `celeris_userns_tests` 有りの build で本体に入った後、user が無い・sudo が使えない・fixture の祖先を辿れない・`PEER-UID-PROBE` の行が
  返らない、のどれでも panic して FAIL する。panic の文言は「peer-uid real-process test environment required: <理由>」を保つ。
- 走らせるか（ignored か）は子 1 の D1 のまま。env 無しの build では今どおり ignored。
- `CELERIS_LAUNCHER_TESTS=require` は D1 の cfg を有効にする変数として残る（build script が読む）。試験 9 の中では読まない。

### G3. 試験 7（task-worker）
- cfg 有りの build で `ssh -V` が起きない・`/etc/ssh/ssh_config.d` が dir でないとき、panic して FAIL する（ADR-0126 B2「道具が無ければ fail」）。
  env は読まない（子 1 の ADR の I4〈db_guard_tests.rs に `CELERIS_USERNS_TESTS`・`CELERIS_ISOLATION_TESTS`・`CELERIS_DB_GUARD_TESTS` の
  読み取りが無い〉を保つ）。

### G4. 人が明示して外す（sudo・ssh の無い host）
- release.sh は `SD_GATE_SKIP_HOST_TESTS` を読む。値は `sudo`・`ssh` の comma 区切り（空・未設定は何も外さない）。
  - `sudo` → 試験 9 を外す。`ssh` → 試験 7 を外す。
  - key ごとの nextest の filter 式は release.sh の 1 か所に置く（例: `package(celeris-credentiald) & test(=real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch)`、
    `package(task-worker) & test(=db_guard::tests::ssh_config_includes_still_parse_inside_the_namespace)`）。外す key があれば
    `-E 'not (<式> | …)'` を test-parallel.sh に渡す。nextest はこれを skipped と数える（PASS にならない）。
  - 未知の key は gate の前に落とす（黙って無視しない）。
  - 非常用の runner（`SD_GATE_TEST_RUNNER=cargo-test`）でも同じ試験を外す（`--skip <name> --exact` に当たる形）か、組み合わせを拒んで落とす。
    どちらにするかは impl が決め、試験で固定する。
- ADR-0126 B4 の `CELERIS_USERNS_TESTS=0` と同じく、人が release.sh を起こすときに env で明示し、gate.json に残す（G6）。

### G5. daemon が起こす gate（NoNewPrivs 1）— 人の決定 `daemon-gate-no-new-privs`
G2 の後、daemon の gate では試験 9 が毎回 `sudo` の失敗で FAIL する（2 節）。何もしなければ、この変更を含む release が昇格した後の最初の
daemon の gate から cargo-test の段で落ちる（release.sh は current の release の写しが走るので、この変更を含む release 自身の gate は旧い台本で通る）。

推奨（`nnp-skip-recorded`）:
- release.sh は gate の前に自分の `/proc/self/status` の `NoNewPrivs` を読む。1 なら、`SD_GATE_SKIP_HOST_TESTS` に `sudo` があるのと同じく試験 9 を
  外し、gate.json の `host_tests.sudo` に `"off_by": "no_new_privs"` と理由（`NoNewPrivs: 1: sudo cannot gain privileges`）を残す。
- 自動で外すのは `sudo` だけで、条件は no_new_privs（kernel が setuid を無視する事実）に限る。`sudo -n` の probe の失敗では外さない
  （NoNewPrivs 0 の host で sudoers の設定が消えたら、試験 9 は FAIL する）。ssh は自動で外さない。
- 試験のために読む file を差し替える口（例: `SD_PROC_SELF_STATUS`）を 1 つ置く。既存の `SD_TEST_PARALLEL` と同じく試験だけが使う。
- 理由: daemon の no_new_privs は unit の設計（人が入れた `NoNewPrivileges=true`）で、この文脈では試験 9 は走れない。本番の設定を変えずに、
  毎回の gate.json に「走っていない」と理由が残る。

他の選択肢:
- `human-env-optout`: 自動の分岐を置かない。人が daemon の環境に `SD_GATE_SKIP_HOST_TESTS=sudo` を足す（unit の drop-in か `paths.env`）。
  daemon の再起動が要り、この変更を含む release の昇格より前に済ませる必要がある。済んでいなければ、昇格後の daemon の gate が落ちる。
- `gate-fails-on-daemon`: daemon の gate では試験 9 を FAIL のままにし、release は operator が login shell で release.sh を起こして作る。
  delivery の自動の release 準備が止まる。

### G6. gate の記録
- gate.json に `host_tests` を足す:
  `{"sudo": {"run": true|false, "off_by": null|"SD_GATE_SKIP_HOST_TESTS"|"no_new_privs", "reason": "…", "tests": ["<試験名>"]}, "ssh": {…}}`。
- gate.json に `gate_env` を足す: `CELERIS_USERNS_TESTS`・`CELERIS_ISOLATION_TESTS`・`CELERIS_LAUNCHER_TESTS`・`SD_GATE_SKIP_HOST_TESTS` の実際の値
  （未設定は null。4 つ）。ADR-0126 B4 の「人が `CELERIS_USERNS_TESTS=0` を明示した」ことも、ここで読めるようになる。
- `release-records.jsonl` の gate の要約（[ADR 2026-11-20-release-record-summary-log](2026-11-20-release-record-summary-log.md)）にも `host_tests` を写す（prune で gate.json が消えても残す）。

### G7. 移行の 1 回目の release の例外（撤去済み）
- 経緯: 移行の 1 回目の release の gate は、G4〜G6 を持たない旧い release.sh で走り、daemon の文脈（NoNewPrivs 1）で試験 9 が panic して
  落ちる。これを避けるため、試験 7・9 は env `CELERIS_SECOND_STAGE_TESTS=require` があるときだけ 2 段目の欠けを FAIL にし、新しい release.sh が
  その env を立てた（人の決定 `first-release`、CoS の回答 `opt-in-env`）。一時の例外で、撤去の条件と責任者（operator）を決めていた。
- 撤去の条件の確認: G7 を含む release の昇格の後、新しい release.sh が走った最初の release（9e33fe4f46f4、gate 2026-11-21T20:55:46Z、daemon の gate）の
  gate.json を operator（claude-code-ops-session）が読み、`gate_env.CELERIS_SECOND_STAGE_TESTS` が `"require"`、`gate_env.CELERIS_LAUNCHER_TESTS` が null、
  `host_tests.sudo` が `run: false`・`off_by: "no_new_privs"`、`host_tests.ssh.run` が true、`cargo_test.nextest_skipped` が前の gate の 23 から 24 に
  なったことを確かめた。release-records.jsonl の gate の行にも同じ `host_tests` がある。
- 撤去（task 01M7ZYFB4YSJHWR7SF97G501S8、2026-11-21）: 次を消した。試験 7・9 は G2・G3 の形（cfg 有りの build で 2 段目の欠けは常に FAIL。env を読まない）に戻った。
  - `injection_peer_uid.rs` と `db_guard_tests.rs` の `second_stage_strict()`、env が無いときの return（PASS と数える分岐）。
  - 試験 9 の `CELERIS_LAUNCHER_TESTS` の読み取り。
  - 旧挙動を固定する試験 2 件（`test9_keeps_old_behaviour_without_the_gate_env_under_no_new_privs`・`ssh_test_keeps_old_behaviour_without_the_gate_env`）。
  - release.sh の strict の env の export と、shell の試験の fixture の `CARGO_ENV_LOG`（その env を記録する口）。
  - gate.json の `gate_env` の `CELERIS_SECOND_STAGE_TESTS` の欄は**消した**。理由: 値は常に `require` で、env 自体が無くなるので情報が無い。
    残すと、読む側（operator・文書・試験）が存在しない env の意味を覚え続けることになり、「旧い仕組みを残さない」方針に反する。
    撤去前の gate.json にはこの欄が残るが、読む処理は無い。
- 残すもの: G4・G5 の外し方（NoNewPrivs 1 で試験 9 を filter で外す、`SD_GATE_SKIP_HOST_TESTS`）と `host_tests` の記録。
- 撤去を含む最初の release の gate が落ちない見込み: その gate は current（G7 を含む）の release.sh で走る。この台本は strict の env を立て、NoNewPrivs 1 では
  試験 9 を nextest の filter で外す（skipped）。撤去後の試験 7 は env を読まず、ssh があるので本体が走って PASS する（9e33fe4f46f4 の gate で `host_tests.ssh.run` が
  true だった）。env は撤去後の試験が読まない値になるだけなので、試験の結果は変わらない。shell の試験 `release_host_tests_gate.sh` は撤去後の release.sh で通る
  （NoNewPrivs 0 で filter なし、1 で試験 9 だけ外れ、`gate_env` が 4 つの欄）。試験 9 の本体は sandbox で走らないので、host での実行は operator が
  login shell で行う（進捗 `2026-11-21-userns-gate-g7-removal.md` の本番への移行）。この見込みは最初の gate.json で確かめる。

## 5. 試験（不変条件と、それを確かめる試験）

どれも sleep の待ちや CPU の負荷に頼らない。子 process・偽の command・偽の file で決定的に作る。修正を外すと落ちることを impl が失敗注入で確かめ、
進捗に書く。既存の試験が検出できない理由: 今の試験は「env 無しで ignored」（子 1 の check-skip.sh）と「本体が走ったときの判定」しか見ず、
本体の中で前提が欠けたときの数え方を見ていない。release.sh の試験は gate の env も除外も見ていない。

- **I1（試験 9 の 2 段目は FAIL）**: credentiald の試験から、試験 9 を子 process（同じ試験 binary を `--ignored --exact <試験 9>`）で起こす。
  PATH の先頭に偽の `sudo` を置き、`CELERIS_PEER_UID_USER` には試験の UID と違う既存の user（例 `nobody`）を渡す。
  (i) 偽の sudo が exit 1 → 子は FAIL し、stderr に `environment required` が出る。(ii) 偽の sudo が `true` には exit 0、probe には何も出さない
  → 子は FAIL し、`no PEER-UID-PROBE line` が出る（「probe の行が返らない」）。この親の試験は cfg によらず走る（本物の sudo は要らない）。
  失敗注入: `missing()` を return の形に戻すと子が PASS し、親が落ちる。
- **I2（試験 7 の 2 段目は FAIL）**: task-worker の試験から、試験 7 を子 process で起こし、ssh の無い PATH（または存在しない ssh_config.d）を渡す →
  子は FAIL する。試験 7 が userns の確認より先に ssh の前提を見るか、子の PATH に `unshare` だけを置くかは impl が決める。
  失敗注入: return の形に戻すと子が PASS し、親が落ちる。
- **I3（gate は明示の除外だけを外し、記録する）**: `scripts/selfdeploy/tests/` の shell の試験。偽の runner（`SD_TEST_PARALLEL`）が受け取った
  引数と env を書き出す。(i) 除外なし・NoNewPrivs 0 → filter の引数なし、`host_tests.sudo.run`・`ssh.run` が true。
  (ii) `SD_GATE_SKIP_HOST_TESTS=sudo,ssh` → 2 つの試験を外す `-E` が渡り、`off_by` が `SD_GATE_SKIP_HOST_TESTS`。
  (iii) 未知の key → cargo-test の段の前に gate が落ち、gate.json に残る。(iv) `gate_env` に 4 つの値が出る。
  失敗注入: filter を渡す行を消すと (ii) が落ちる。
- **I4（G5 の推奨を採るとき: NoNewPrivs 1 は試験 9 だけを外す）**: 差し替えた status の file に `NoNewPrivs:\t1` → 試験 9 だけが外れ、
  `off_by` が `no_new_privs`。試験 7 は外れない。`NoNewPrivs:\t0` → 何も外れない。失敗注入: 分岐を消すと落ちる。
- **I5（この host の gate が落ちない根拠）**: 2 節の文脈（daemon の gate は NoNewPrivs 1・ssh あり）と I3・I4 の結果から、daemon の gate では
  試験 9 が外れて skipped、試験 7 は本体が走って今と同じく PASS、と impl が進捗に書く。昇格後の最初の gate.json で確かめる手順を
  record の WorkUnit が「本番への移行」に書く。
- 子 1 の check-skip.sh（env 無しの build で 9 件が ignored、PASS 0 件）は変えずに通る。

## 6. 他の ADR との関係

- **ADR-0126 B2**: 試験 7・9 を B2 の「`CELERIS_USERNS_TESTS=1` で環境が無ければ fail」に揃える。B3 の優先順位は変えない。
- **ADR-0126 B4**: 変えない。release.sh は今どおり `CELERIS_USERNS_TESTS=1`（既定）と `CELERIS_ISOLATION_TESTS=require` を立てる。
  userns の無い host の明示の外し方（`CELERIS_USERNS_TESTS=0`）もそのまま。この ADR は同じ形（人が env で明示し、gate の記録に残す）を、
  sudo・ssh の 2 段目の前提について `SD_GATE_SKIP_HOST_TESTS` で足す。B4 の明示は G6 の `gate_env` で gate.json から読めるようになる。
- **子 1 の ADR（2026-11-21-userns-tests-explicit-skip）**:
  - D1（env 無しの build で harness の ignored にする規則、`userns_tests_cfg.rs` の優先順位）は変えない。gate の env も変えないので、gate の build で
    ignored になる試験は今と同じ（試験 8 だけ）。共有の release の target の再 build も起きない。
  - D2 のうち試験 9 の `missing()` と試験 7 の return、§3 の「残る穴」の gate の部分を、この ADR が置き換える。§3 のうち試験 7・9 の外
    （task-worker の lib・tests/・task-api・campaign の env を実行時に読む試験）は残る。
  - I4（3 file に env の読み取りが無い）は保つ。試験 9 からは `CELERIS_LAUNCHER_TESTS` の読み取りも消える。
- **ADR-0051・delivery**: daemon の delivery が gate を起こす経路は変えない。G5 はその文脈（no_new_privs）の扱いである。

## 7. 残る穴（受け入れる）

- daemon の gate（NoNewPrivs 1）では、G5 のどの選択肢でも試験 9 の本体は走らない。子 1 の U1 のような試験 9 の不具合は、NoNewPrivs 0 の文脈
  （人・operator の login shell で release.sh か試験 9 を流す）でしか見つからない。credentiald を変えた release で operator が試験 9 を流す運用に
  するかは別に決める（進捗の提案 2）。
- 試験 7・9 の外にも、本体の中の `missing()`・return で PASS と数える試験が残る。`browser_launcher_ptrace.rs` の
  `launcher_chrome_denies_daemon_uid_ptrace`（launcher の socket・subuid・protocol の版ずれ）、`browser_launcher/userns.rs` の
  `real_namespace_when_host_is_ready`、task-api の `userns_available()` を使う 2 件（子 1 の userns-skip.md の提案 1 の A）。この task の範囲外。
- env 有りの build を gate の外で使う人（`CELERIS_USERNS_TESTS=1` を付けた手元の実行・reviewer の run）では、sudo が使えなければ試験 9 が、ssh が
  無ければ試験 7 が FAIL する。B2 の意図どおり（明示的に求めたのに黙って飛ばさない）で、外すなら nextest の `-E` で明示する。
  G7 の撤去後は、strict の env の有無によらない。
