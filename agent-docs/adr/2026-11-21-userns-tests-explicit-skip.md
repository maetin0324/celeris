---
tasks: [01M7XSH46KVRV62ENAWWKWJKBQ]
---
# ADR 2026-11-21: userns の試験は env 無しの build で harness の ignored にし、env 有りの build では本体を走らせる（ADR-0126 B の付記）

- 日付: 2026-11-21
- 状態: Accepted（実装は WorkUnit `impl`。env 有りの実行の記録は WorkUnit `verify`。結果は 4 節の付記。試験 9 の子の行の読み取りの修正は 5 節の付記〈WorkUnit `peer-uid-probe`〉）
- task: 01M7XSH46KVRV62ENAWWKWJKBQ（戦略層 v1 子 1 の受入条件 3。unit `userns-skip`）
- 進捗: [userns-skip.md](../progress/2026-11-21-strategy-v1-1/userns-skip.md)（9 件の return の行と変更前の nextest の出力）
- 関係: [ADR-0126](0126-test-daemons-without-userns.md) B2・B3（既定 skip と env の優先順位）・却下した案の 3 つ目（`#[ignore]`）、
  [ADR-0095](0095-worker-runs-see-the-db-read-only.md)（db guard）、
  [ADR 2026-10-11 browser-a13-broker-peer-uid-real-process](2026-10-11-browser-a13-broker-peer-uid-real-process.md)（試験 9）、
  [ADR 2026-10-20 full-tests-at-release-only](2026-10-20-full-tests-at-release-only.md)。
- 範囲: `crates/task-worker/src/db_guard_tests.rs`（高 risk の class `sandbox`）と `crates/celeris-credentiald/`（class `credential`）の
  試験だけを変える。production のコード経路・`scripts/`（release.sh を含む）・config・API・DB は変えない。この ADR を設計 brief とする。

## 1. 実装前 brief

### 1.1 根本原因
ADR-0126 B2 は「env が無ければ試験の始めで抜け、stderr に 1 行出す」と決めた。抜けた試験は libtest と nextest が
**pass** と数える。アーキv4 H の safety.md が挙げた 9 件（`crates/task-worker/src/db_guard_tests.rs` の 7 件と
`crates/celeris-credentiald/tests/injection_peer_uid.rs` の 2 件）は、env 無しの nextest で `9 passed` になる
（進捗の「変更前の nextest の出力」）。本体が 1 行も走らないのに件数は pass で、gate の `passed` と人の読む要約で区別できない。
症状は件数（pass）に出て、原因は「skip を試験の中の return で表す」判定の置き場所（試験の本体）にある。

### 1.2 責務 owner
- env を読んで「走らせるか」を決める規則: 今は `crates/task-worker/src/test_support.rs` の `skip_unless_userns_tests` と
  `crates/celeris-credentiald/tests/injection_peer_uid.rs` の `skip_by_default` に別々の写しがある。新しい owner は
  `crates/task-worker/userns_tests_cfg.rs`（build script 専用の 1 file）。
- skip を件数に出すこと: test harness（libtest の `#[ignore]`）。試験の本体は判定を持たない。

### 1.3 既存の再利用先
- libtest の `#[ignore = "理由"]` と `cfg_attr`。nextest は ignored を `skipped` と数え、`nextest list --message-format json` の
  `ignored` 欄に出す。
- Cargo の build script の `cargo:rustc-cfg`・`cargo:rustc-check-cfg`・`cargo:rerun-if-env-changed`。
- ADR-0126 B3 の優先順位（そのまま写す。値も変えない）。

### 1.4 選択肢
1. **（採用）build script が env を読み、cfg `celeris_userns_tests` を出す。試験は `#[cfg_attr(not(celeris_userns_tests), ignore = "理由")]`**。
   env 無しの build では harness が ignored と数え（理由の文字列付き）、env 有りの build では普通の試験として本体が走る。
   `--ignored` を付けないので、他の ignored 試験を巻き込まない。
2. 試験の中で return する今の形のまま、stderr の `SKIPPED` を数える集計を gate に足す: 件数の意味（pass）は変わらず、
   集計の台本（release.sh）を変える必要がある。範囲外で、症状の側の修正になる。却下。
3. 常に `#[ignore]` にして gate で `--run-ignored all` を付ける: ADR-0126 の却下理由（他の ignored 試験を巻き込む）がそのまま当たる。却下。
4. `panic` で fail させる: env 無しの開発機・worker の sandbox で必ず落ちる。ADR-0126 A・B の目的（sandbox で回る）に反する。却下。

### 1.5 新しい状態・分岐・fallback・port
- 新しい cfg 1 つ（`celeris_userns_tests`）と build script 2 つ（`crates/task-worker/build.rs`・`crates/celeris-credentiald/build.rs`）。
  どちらも `#[path]` で同じ `crates/task-worker/userns_tests_cfg.rs` を読み、crate 固有の require 変数だけを渡す。
- production の分岐・fallback・port は足さない。cfg を読むのは試験の code だけ。

### 1.6 撤去対象
- `crates/celeris-credentiald/tests/injection_peer_uid.rs` の `skip_by_default()`（env の規則の写し）。
- `crates/task-worker/src/test_support.rs` の `skip_unless_userns_tests` の中の env の読み取り（cfg から導く形に置き換える）。
- 9 件の先頭の `return`（`if !userns_available() { return; }`・`if skip_unless_userns_tests() { return; }`・`if skip_by_default() { return; }`）と、
  試験 8 の「env が無ければ何もしない」return。

### 1.7 不変条件と確かめる試験
- I1: env 無し（4 変数を unset）の build では 9 件が nextest に ignored（skipped）と数えられ、1 件も passed にならない。
  `sh agent-docs/progress/2026-11-21-strategy-v1-1/userns-skip/check-skip.sh` が確かめる（`nextest list` の JSON と `nextest run` の行）。
- I2: env 有り（`CELERIS_USERNS_TESTS=1` など）の build では 9 件のうち試験 8 以外の 8 件が普通の試験として本体を走らせ、
  userns が作れなければ今の assert で fail する（assert の文言と条件は変えない）。WorkUnit `verify` が `--no-capture` で走らせ、
  task-worker の 7 件が stderr に出す `USERNS-BODY` の行と、試験 9 の `PEER-UID` の行を進捗に記録する。
- I3: ADR-0126 B3 の優先順位を保つ: `CELERIS_ISOLATION_TESTS=skip` は `CELERIS_USERNS_TESTS=1` より強い。verify が
  両方を付けた `nextest list` で 9 件が ignored のままであることを記録する。
- I4: env を読む規則は `crates/task-worker/userns_tests_cfg.rs` の 1 か所だけ（対象の 3 file に `"CELERIS_USERNS_TESTS"`・
  `"CELERIS_ISOLATION_TESTS"`・`"CELERIS_DB_GUARD_TESTS"` の読み取りが無い）。WorkUnit `impl` の check 4 が grep で確かめる。

## 2. 決定

### D1. 規則の file と build script
`crates/task-worker/userns_tests_cfg.rs` は build script 専用の module で、`emit(crate_require_var, rule_file)` を持つ。

- 優先順位（ADR-0126 B3 のまま。上が強い）:
  1. `CELERIS_ISOLATION_TESTS=skip` → 無効。
  2. `CELERIS_USERNS_TESTS=1`・`CELERIS_ISOLATION_TESTS=require`・crate 固有の `<var>=require` のどれか → 有効。
  3. どれも無い → 無効（既定）。
- 出すもの: 読む env の全部（`CELERIS_ISOLATION_TESTS`・`CELERIS_USERNS_TESTS`・crate 固有の変数）の
  `cargo:rerun-if-env-changed`、`cargo:rerun-if-changed=build.rs` と規則の file、`cargo:rustc-check-cfg=cfg(celeris_userns_tests)`
  （`unexpected_cfgs` を出さない。clippy `-D warnings` を通す）、有効なら `cargo:rustc-cfg=celeris_userns_tests`。
- crate 固有の変数: task-worker は `CELERIS_DB_GUARD_TESTS`（今の `test_support` と同じ）、celeris-credentiald は
  `CELERIS_LAUNCHER_TESTS`（今の `skip_by_default` と同じ）。2 つの crate の受け付ける値の違いは変えない。

### D2. 試験の形
- `db_guard_tests.rs` の 7 件と `real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch` は
  `#[cfg_attr(not(celeris_userns_tests), ignore = "…")]` を付け、先頭の return を消す。
- `userns_available()` は bool を返さず、`unshare -Ur true` の assert（文言は今のまま）を通ったら stderr に
  `USERNS-BODY <試験名>: …` の 1 行を出す helper（`assert_userns_available`）にする。試験 4 は今どおり `unshare(1)` の存在の
  assert だけを持ち、その後で同じ形の `USERNS-BODY` の行を出す。
- `inner_peer_uid_probe`（試験 8）は試験 9 が `sudo -n -u <user>` で自分の binary を起こす子の入口で、常に
  `#[ignore = "…"]` にする。`probe_as` は `--ignored --exact inner_peer_uid_probe` で起こす。socket の env が無い直接の実行
  （`--run-ignored all`・`--include-ignored`）は panic で fail する（黙って pass しない）。
- 試験 1（`user_systemd_bus_is_hidden_from_launched_process`）は socket を置く runtime dir を `test_support::short_tempdir()` の下に
  移す。env 有りで本体を走らせると、worker run の深い `TMPDIR`（93 文字）の下では socket の path が sun_path（108 byte）を超えて
  `bind` が fail した。今までは return して pass だったので見えていなかった。assert は変えない。
- `test_support::skip_unless_userns_tests` は `cfg!(celeris_userns_tests)` から導く。task-worker の lib の他の呼び出し元
  （`browser_tests.rs` の 8 件・`browser_launcher/userns.rs`）は今の return の形のまま（下の「残る穴」）。

### D3. env を変えると crate が再 build される
cfg は build の時点で決まる。env を変えると Cargo は build script を走らせ直し、cfg が変われば task-worker（または
celeris-credentiald）と、それに依存する crate を compile し直す。celeris-credentiald は task-worker の依存なので、
`CELERIS_LAUNCHER_TESTS` を変えると task-worker 以降も compile し直す。gate（release.sh）は 1 回の実行の中で env を変えないので、
増えるのは env の違う 2 つの実行を同じ target dir で続けたときの再 build だけ。cfg は lib の production のコードの意味を変えない
（cfg を読むのは `#[cfg(test)]` の code と `tests/` だけ）。
build した後に env を変えて test binary を直接走らせても、cfg は build の時の値のまま。cargo（test・nextest）を通せば一致する。

### D4. ADR-0126 との関係
- B2 の「未設定 → 試験の始めで抜け、stderr に 1 行」を、この 9 件では「未設定の build → harness の ignored（理由付き）」に置き換える。
  B2 の「`CELERIS_USERNS_TESTS=1` で環境が無ければ fail」と B3 の優先順位は変えない。
- ADR-0126 の却下した案「userns 試験を `#[ignore]` にする」の理由（`--ignored` が他の ignored 試験を巻き込む・理由が出ない）は、
  この形には当たらない: env 有りの build では ignore が付かず `--ignored` が要らない。理由は `ignore = "…"` の文字列で出る。
  この ADR はその却下を、cfg で切り替える `cfg_attr` の形に限って上書きする。
- B4（release.sh が `CELERIS_USERNS_TESTS=1` と `CELERIS_ISOLATION_TESTS=require` を export する）は変えない。gate では 9 件のうち
  試験 8 だけが ignored と数えられる（試験 9 が sudo 経由で走らせる）。

## 3. 残る穴（受け入れる。この unit では直さない）
- 試験の本体の中の 2 段目の skip（env の値によらず return して pass になる所）は残す: 試験 7 の「ssh が無い」、試験 9 の `missing()`
  （user が無い・sudo が使えない・fixture の祖先 dir を辿れない・probe の行が返らない）。試験 9 の `missing()` は今どおり
  `CELERIS_LAUNCHER_TESTS=require` のときだけ panic し、その判定は試験の中で env を読む（「走らせるか」ではなく「環境が無いとき
  fail にするか」の規則で、D1 の規則とは別）。release gate は `CELERIS_LAUNCHER_TESTS=require` を立てないので、sudo が使えない host
  では試験 9 は本体に入ってから return して pass になる。
  - 試験 7・9 の gate の扱いは [ADR 2026-11-21-release-gate-second-stage-skip](2026-11-21-release-gate-second-stage-skip.md) が置き換えた（本体の中の欠けは常に失敗、gate が走れない試験は明示して外し gate.json に残す）。D1 の本文は変えない。
- 同じ「env 無しで return して pass」の試験は他にもある（進捗の提案 1: ADR-0126 の helper 経由 28 件、自前の規則 7 件など）。
  task-worker の lib の 9 件（`browser_tests.rs` 8 件と `browser_launcher/userns.rs` 1 件）は同じ cfg を読む `skip_unless_userns_tests`
  を通るので、別の task で `cfg_attr` に移せる。`crates/task-worker/tests/userns_gate/mod.rs`・task-api・campaign は env を
  実行時に読む写しのままで、別の task で同じ規則の file に寄せる。

## 4. 付記（2026-11-21、WorkUnit `verify` の結果）
進捗の「verify の段」に出力と表がある。
- I1: `check-skip.sh` は exit 0（9 件が list で ignored、run で SKIP、PASS 0 件）。
- I2: `CELERIS_USERNS_TESTS=1` の build で task-worker の 7 件が本体に到達し（各試験の `USERNS-BODY` の行）、7 件とも PASS した。
  credentiald の 2 件は未確認: worker の sandbox は `NoNewPrivs: 1` で `sudo -n` が使えず、試験 9 は `missing()` で return して PASS と数えられ
  （3 節の「残る穴」のとおり）、`PEER-UID` の行は出なかった。試験 8 は直接の実行で panic して FAIL した（D2 のとおり）。
  人が release gate の host で `CELERIS_USERNS_TESTS=1 CELERIS_LAUNCHER_TESTS=require TMPDIR=/tmp` を付けて試験 9 を走らせ、
  `PEER-UID control`・`dac`・`broker`・`result` の 4 行と PASS を確かめる（手順は進捗の「未確認の 2 件」）。
- I3: `CELERIS_ISOLATION_TESTS=skip CELERIS_USERNS_TESTS=1` の `nextest list` で 9 件とも ignored のまま。`CELERIS_USERNS_TESTS=1` だけでは
  `inner_peer_uid_probe` の 1 件だけが ignored。

## 5. 付記（2026-11-21、WorkUnit `peer-uid-probe`。人の決定 first-gate-unmet の答え）
- 原因: operator が release gate の host（`NoNewPrivs: 0`、`sudo -n -u celeris-browser true` が 0）で試験 9 を
  `CELERIS_USERNS_TESTS=1 CELERIS_LAUNCHER_TESTS=require TMPDIR=/tmp` で走らせ、exit 100 になった（`PEER-UID control` の 1 行の後に
  `no PEER-UID-PROBE line` で panic）。`probe_as` は子を `--nocapture --test-threads=1` で起こす。libtest は 1 thread のとき
  `test inner_peer_uid_probe ... ` を試験の開始時に改行せずに出し、子の `println`（`inner_peer_uid_probe` の末尾）がその後ろに続く。
  親の `stdout.lines().find_map(Probe::parse)` と `Probe::parse` の `strip_prefix(MARK)` は、行の頭に `MARK` が無いこの行を読めなかった。
  子の本体は走っており、uid 995 の `connect()` は `EACCES` を返していた（DAC 層の拒否）。broker 層の判定には進んでいなかった。
- 直し方: 親が子の stdout を読む関数を `read_probe` の 1 つにした。各行の中の `MARK` の位置から行末までを `Probe::parse` に渡す。
  子を起こす引数と読み取りは `run_probe_child` にまとめ、`probe_as` は `sudo -n -u <user> /usr/bin/env` を前に付けてそれを呼ぶだけにした。
  `MARK` の後の欄の規則（未知の欄は `None`）、試験 9 の本体の判定（dac・broker・journal の件数）、`missing()` と
  `CELERIS_LAUNCHER_TESTS=require` の扱いは変えていない。
- 試験: `probe_found_after_libtest_prefix`（host の子の stdout と同じ形の文字列を読む）と `probe_read_from_real_child_run`
  （同じ UID の子を `run_probe_child` で本当に起こして読む。sudo は要らない）。どちらも cfg によらず走る。読み取りを旧い形に戻すと 2 件とも落ちる
  （出力は進捗 `agent-docs/progress/2026-11-21-strategy-v1-1/peer-uid-probe.md`）。
- host での確認: worker の sandbox は `NoNewPrivs: 1` で sudo が使えないので、試験 9 の本体（`PEER-UID` の dac・broker・result の行）は
  この unit では確かめていない。第一ゲートの決定（WorkUnit `gate`）で operator が release gate の host で同じ command を走らせ、
  `PEER-UID control`・`dac`・`broker`・`result` の 4 行と PASS を確かめる。
- 3 節の「残る穴」（release gate が `CELERIS_LAUNCHER_TESTS=require` を立てず、sudo が使えない host では試験 9 が return して PASS になる）は
  この unit では直さない（人の決定で別 task。U2）。
