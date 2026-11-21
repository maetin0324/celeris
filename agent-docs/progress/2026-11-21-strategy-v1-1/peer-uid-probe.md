---
title: 戦略層 v1 子 1 peer-uid-probe — 試験 9 の親が libtest の前置きの付いた PEER-UID-PROBE 行を読む
tasks: [01M7XQ9Q4P4ZQ38Y509J9XYAQ0]
status: done
updated: 2026-11-21
completed: 2026-11-21
---
# peer-uid-probe（WorkUnit。人の決定 first-gate-unmet の答え〈replan、範囲は試験 9 の修正だけ〉）

完了日: 2026-11-21。変えたのは `crates/celeris-credentiald/tests/injection_peer_uid.rs`（試験の file）1 つと、ADR の付記・この進捗だけ。
production のコードは変えていない。

## 結論

- 親が子の stdout を読む関数を `read_probe` の 1 つにした。各行の中の `PEER-UID-PROBE`（`MARK`）の位置から行末までを `Probe::parse` に渡す。
  `probe_as` は `run_probe_child` を通してこの関数だけを使う。
- sudo の要らない試験を 2 件足した（`probe_found_after_libtest_prefix`・`probe_read_from_real_child_run`）。読み取りを旧い
  `stdout.lines().find_map(Probe::parse)` に戻した一時の変更では 2 件とも落ち、直した後は 2 件とも通る（下の「前後の試験」）。
- 試験 9 の本体の判定（dac・broker・journal の件数）、`missing()`、`CELERIS_LAUNCHER_TESTS=require` の扱い、`Probe::parse` は base と同じ。
- 試験 9 の本体（`PEER-UID` の dac・broker・result の行）は未確認。worker の sandbox は `NoNewPrivs: 1` で sudo が使えない。
  第一ゲートの決定で operator が release gate の host で下の command を走らせて確かめる。

## 実装前 brief

1. 根本原因: `probe_as`（base の `injection_peer_uid.rs:309`）は子を `--nocapture --test-threads=1`（:319〜320）で起こす。libtest は
   1 thread のとき試験の開始時に `test inner_peer_uid_probe ... ` を改行せずに出し、子の `println`（:188）がその後ろに続く。
   親の `stdout.lines().find_map(Probe::parse)`（:326）と `Probe::parse` の `line.strip_prefix(MARK)`（:125）は、行の頭に `MARK` が
   無いこの行を読めない。症状は試験 9 の `missing(reason)`（:401）に出た（`require` で panic、既定では SKIP の行を出して PASS）。
2. 責務 owner: 親が子の出力を読む部分（`probe_as` の読み取り）。子の出力の形（`Probe::line`）と欄の規則（`Probe::parse`）は正しい。
3. 既存の再利用先: `Probe::parse`（欄の規則）、`Fixture::serve`（同 UID の broker）、`journal_count`、子の入口 `inner_peer_uid_probe`。
4. 選択肢: (a) 親が各行の中の `MARK` の位置から読む。(b) 子が `MARK` の行を改行から始める（`println!("\n{}", …)`）。
   (a) を採った。子の出力の形は libtest の版や引数（`--test-threads`・`--format`）で変わりうるので、親の読み取りで吸収する方が
   壊れにくい。(b) は子の側で libtest の出力の形を前提にする。
5. 新しい状態・分岐・fallback・port: 無し。`run_probe_child` は `probe_as` の本体を、子を起こす command を引数に取る形にしただけ。
6. 撤去対象: `probe_as` の中の旧い読み取り `stdout.lines().find_map(Probe::parse)`（`read_probe` に置き換えた）。
7. 不変条件: 親は libtest の前置きの有無によらず子の `PEER-UID-PROBE` 行を読む（`probe_found_after_libtest_prefix`・
   `probe_read_from_real_child_run`）。欄の規則（未知の欄は `None`）は変えない（`probe_line_round_trips`）。

既存の試験で検出できなかった理由: `probe_line_round_trips` は `Probe::parse` に行の頭から渡すだけで、親が子の stdout を読む経路を通らない。
子を起こす経路は試験 9 だけが通り、試験 9 は sudo が使える host でしか本体に入らないので、worker の sandbox と release gate の既定では
`missing()` で return して PASS になっていた。

## 直した行（commit「strategy-v1 1 peer-uid-probe: 試験 9 の親が libtest の前置きの付いた PEER-UID-PROBE 行を読む」）

file はすべて `crates/celeris-credentiald/tests/injection_peer_uid.rs`。行番号は提出時の HEAD。

| 行 | 変更 |
|---|---|
| 306〜313 | `read_probe(stdout: &str) -> Option<(u32, Probe)>` を足した。`lines().find_map(\|line\| Probe::parse(&line[line.find(MARK)?..]))` |
| 315〜344 | `run_probe_child(launcher: Command, socket: &Path)` を足した。base の `probe_as` の本体（socket の env・自分の binary・`--ignored --exact inner_peer_uid_probe --nocapture --test-threads=1`・読み取り・`Err` の文）を移し、読み取りを `read_probe` に替えた（:335）。`Err` の文は子を起こした command の全体（`{launcher:?}`）を出す |
| 346〜351 | `probe_as` は `sudo -n -u <user> /usr/bin/env` を組み立てて `run_probe_child` を呼ぶだけにした |
| 495〜529 | 試験 `probe_found_after_libtest_prefix` を足した |
| 531〜550 | 試験 `probe_read_from_real_child_run` を足した |

`git grep -n 'find_map(Probe::parse)' -- crates/` → 1 件（:496 の doc comment の中の説明だけ。code には無い）。

### 足した試験と、守る不変条件

- `probe_found_after_libtest_prefix`: operator が host で得た子の stdout と同じ形の文字列
  （`running 1 test\ntest inner_peer_uid_probe ... PEER-UID-PROBE uid=995 pid=752262 connect_errno=13\nok\n\ntest result: ok. …`）を
  `read_probe` で読み、`(995, ConnectFailed { errno: Some(EACCES) })` を得る。行の頭に `MARK` がある形も読めること、`MARK` の無い
  stdout は `None` になることも確かめる。
- `probe_read_from_real_child_run`: 同 UID の `Fixture::serve(995)` の socket に対し、`run_probe_child(Command::new("/usr/bin/env"), …)` で
  自分の test binary を試験 9 と同じ引数で本当に起こし、`(自分の uid, Reply { code: Some("injection_worker_not_allowed") })` を得る。
  journal に自分の uid の `injection_worker_not_allowed` が 1 行あることも確かめる（子の接続が broker に届いた証拠）。試験 9 との違いは
  子を起こす command の前置き（`sudo -n -u <user>`）だけ。libtest が実際に出す stdout を読むので、libtest の出力の形が変わっても検出する。
- どちらも `#[ignore]` も cfg も付かず、env 無しの build で走る。

## 前後の試験

### 修正を外した一時の変更（落ちる）

`read_probe` の本体を `stdout.lines().find_map(Probe::parse)` に戻した一時の変更（commit していない）で:

`env -u CELERIS_USERNS_TESTS -u CELERIS_ISOLATION_TESTS -u CELERIS_DB_GUARD_TESTS -u CELERIS_LAUNCHER_TESTS cargo nextest run -p celeris-credentiald --test injection_peer_uid --no-fail-fast --status-level all --final-status-level fail -E 'test(=probe_found_after_libtest_prefix) | test(=probe_read_from_real_child_run)'`
→ exit 100、`2 tests run: 0 passed, 2 failed, 4 skipped`。

```
FAIL [   0.005s] (1/2) celeris-credentiald::injection_peer_uid probe_found_after_libtest_prefix
thread 'probe_found_after_libtest_prefix' panicked at crates/celeris-credentiald/tests/injection_peer_uid.rs:505:5:
assertion `left == right` failed
  left: None
 right: Some((995, ConnectFailed { errno: Some(13) }))

FAIL [   0.018s] (2/2) celeris-credentiald::injection_peer_uid probe_read_from_real_child_run
thread 'probe_read_from_real_child_run' panicked at crates/celeris-credentiald/tests/injection_peer_uid.rs:538:5:
assertion `left == right` failed
  left: Err("no PEER-UID-PROBE line from \"/usr/bin/env\" \"CELERIS_PEER_UID_PROBE_SOCKET=/tmp/czBAr0WU/run/celeris-credentiald/injection.sock\" \".../target/debug/deps/injection_peer_uid-<hash>\" \"--ignored\" \"--exact\" \"inner_peer_uid_probe\" \"--nocapture\" \"--test-threads=1\" (exit=Some(0)); stdout=\"running 1 test\\ntest inner_peer_uid_probe ... PEER-UID-PROBE uid=1001 pid=839643 reply=injection_worker_not_allowed\\nok\\n\\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.00s\" stderr=\"\"")
 right: Ok((1001, Reply { code: Some("injection_worker_not_allowed") }))
```

2 件目の子の stdout は、operator が host で得た stdout と同じ形（`test inner_peer_uid_probe ... ` の後に `PEER-UID-PROBE` が続く）だった。
sandbox の中でも、同 UID の子で host の失敗を再現できた。

### 直した後（通る）

`env -u CELERIS_USERNS_TESTS -u CELERIS_ISOLATION_TESTS -u CELERIS_DB_GUARD_TESTS -u CELERIS_LAUNCHER_TESTS cargo nextest run -p celeris-credentiald --test injection_peer_uid --no-fail-fast --status-level all --final-status-level all`
→ exit 0、`4 tests run: 4 passed, 2 skipped`。

```
SKIP celeris-credentiald::injection_peer_uid inner_peer_uid_probe
SKIP celeris-credentiald::injection_peer_uid real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch
PASS celeris-credentiald::injection_peer_uid probe_found_after_libtest_prefix
PASS celeris-credentiald::injection_peer_uid probe_line_round_trips
PASS celeris-credentiald::injection_peer_uid same_uid_control_is_worker_not_allowed
PASS celeris-credentiald::injection_peer_uid probe_read_from_real_child_run
```

## 試験 9 の判定が base と同じこと

base（`$CELERIS_WU_BASE`）と提出時の HEAD の file から関数の本体（`fn <名>(` から最初の `\n}\n` まで）を取り出して比べた
（`git show $CELERIS_WU_BASE:<path>` と `git show HEAD:<path>` を python で比較。`Probe::parse` は impl の中の `fn parse(` から `\n    }\n` まで）。次の 7 つは完全に同じ:
`real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch`（dac・broker・journal の assert と `missing()` の呼び出し）、
`missing`、`require`、`inner_peer_uid_probe`、`Probe::parse`、`probe_line_round_trips`、`same_uid_control_is_worker_not_allowed`。
`git diff $CELERIS_WU_BASE HEAD -- crates/` の hunk は `probe_as` の周り（`read_probe`・`run_probe_child`・`probe_as`）と、足した 2 件の試験だけ。

## env 無しの 9 件（check-skip）

`sh agent-docs/progress/2026-11-21-strategy-v1-1/userns-skip/check-skip.sh` → exit 0。
`check-skip: ok (9 tests ignored in the list and skipped in the run; none passed)`、run の `Summary … 0 tests run: 0 passed, 1238 skipped`。

## LoC（`$CELERIS_WU_BASE` と HEAD。分類規則は base の `scripts/dev/source-size-report.py` と `.toml` を `git show` で取り出して固定）

`python3 -I <base の source-size-report.py> --root . --config <base の toml> --exclusive-classes --rev <rev> --format json`:

| 区分 | base | HEAD | 差 |
|---|---|---|---|
| production（全言語） | 376359 | 376359 | 0 |
| production（Rust） | 331668 | 331668 | 0 |
| external_test | 465795 | 465867 | +72 |
| own_total_with_tests（全言語） | 1037717 | 1037789 | +72 |
| own_total_with_tests（Rust） | 890456 | 890528 | +72 |

`git diff --numstat $CELERIS_WU_BASE HEAD -- crates/` → `83 11 crates/celeris-credentiald/tests/injection_peer_uid.rs`。
test の純増 +72 行の内訳: 足した 2 件の試験（doc comment と空行を含む）が 57 行（:495〜529 と :531〜550）、`read_probe`・`run_probe_child`・`probe_as` への分割が +15 行（26 行足して 11 行消した）。

## operator が release gate の host で流す command（第一ゲートの決定で確かめる）

release gate と同じ host の、celeris の worker run の外の login shell で:

1. 前提: `grep NoNewPrivs /proc/self/status` が `NoNewPrivs: 0`。`sudo -n -u celeris-browser true; echo $?` が `0`。
2. この修正を含む commit の tree（子 1 の統合後の branch、または main に入った後の main）を local disk に置き、`CARGO_TARGET_DIR` も local disk にして:
   ```
   CELERIS_USERNS_TESTS=1 CELERIS_LAUNCHER_TESTS=require TMPDIR=/tmp \
     cargo nextest run -p celeris-credentiald --test injection_peer_uid --no-capture --status-level all \
     -E 'test(=real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch)'
   ```
3. 確かめること: exit 0、`1 test run: 1 passed`。stderr に次の 4 行がある:
   - `PEER-UID control uid=<自分の uid> Reply { code: Some("injection_worker_not_allowed") }`
   - `PEER-UID dac uid=995 ConnectFailed { errno: Some(13) }`
   - `PEER-UID broker uid=995 Reply { code: Some("peer_uid_mismatch") }`
   - `PEER-UID result dac=connect_errno=13 broker=peer_uid_mismatch (peer uid 995, broker uid <自分の uid>)`
   `SKIP: (not passed)` の行が出たら（`require` では panic になる）、その理由を決定の答えに書く。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

検査は commit「strategy-v1 1 peer-uid-probe: ADR に試験 9 の読み取りの修正の付記を足し、進捗を書く」の HEAD で実行した。
その後の commit はこの節の文だけを書き足した（code・試験・Cargo の入力は変えていない）。

- `bash scripts/dev/test-affected.sh` → exit 0（12 分 48 秒）。対象 crate は celeris・celeris-campaign・celeris-credentiald・celerisctl・
  llm-proxy・task-api・task-dispatch・task-worker と web（理由: web の型の元になる crate の変更: celeris-credentiald）。
  nextest `Summary … 6262 tests run: 6262 passed (9 slow), 30 skipped`。web は tsc -b が通り、vitest `Test Files 101 passed`・
  `Tests 705 passed`、`node --test` `tests 110`・`pass 110`・`fail 0`。repo-health は `verdict=pass`（fail_grade=0、new=0、worsened=0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。
- `sh scripts/dev/check-doc-links.sh` → exit 0。
- `sh agent-docs/progress/2026-11-21-strategy-v1-1/userns-skip/check-skip.sh` → exit 0（9 件が ignored のまま。上の節）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変更 path は `crates/celeris-credentiald/tests/injection_peer_uid.rs`・
  `agent-docs/adr/2026-11-21-userns-tests-explicit-skip.md`・`agent-docs/progress/2026-11-21-strategy-v1-1/peer-uid-probe.md` の 3 つだけ。

## 未解決（この unit では直さない。人の決定のとおり）

- 試験 9 の本体（dac・broker の `PEER-UID` の行）は未確認。sandbox では sudo が使えない。上の command を operator が host で走らせる。
- U2: release gate（`scripts/selfdeploy/release.sh`）は `CELERIS_LAUNCHER_TESTS=require` を立てず、`PEER-UID` の行を gate の log に残さない。
  sudo が使えない gate の host では試験 9 は return して PASS と数えられる。operator が別 task として起票する。
- U3（Project.request を実 prompt に出す経路）は子 2 で扱う。U4（cluster job の wait で終わった run の decisions）は operator が別 task として起票する。
- U5（本番での確認）は子 1 が main に入った後の release が昇格されてから行う。

## 提案

- `run_probe_child` は子を起こす command を引数に取るので、U2 を直す task で gate の log に `PEER-UID` の行を残すときも、この関数の
  `Err` の文（子を起こした command・exit・stdout・stderr の全体）をそのまま使える。
