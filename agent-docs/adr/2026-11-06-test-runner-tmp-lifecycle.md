---
tasks: [01M6S9KKWT7EM7FP5CCG1Q8DH0]
---
# 試験 runner の一時領域の寿命管理（test-affected.sh・test-parallel.sh）

状態: 採用（設計）。実装は後続の unit が行う。この ADR は実装より先に commit する。

## 背景
`scripts/dev/test-affected.sh` は crate の試験の前に `/tmp/celeris-test-affected.XXXXXX` を作り、`TMPDIR` をその下の `tmp` にする。
`scripts/dev/test-parallel.sh` も `/tmp/celeris-test-parallel.XXXXXX` を同じように作る。どちらも後片付けは bash の `trap … EXIT` だけである。
2026-11-06 に tmpfs の `/tmp`（63G）が 93.6% まで埋まった。この ADR を書いた時点（2026-11-06 21 時台、JST）では、
`/tmp/celeris-test-affected.*` が 27 件、`/tmp/celeris-test-parallel.*` が 15 件残り、合わせて 29G あった（`du -sch`）。
中身は試験の残り物だけではない。cargo の build 中の `cc*.o`・`cc*.res` と、test-affected の `script-bundle` も含む。
`TMPDIR` を export した後に cargo が build するため、rustc・linker の一時 file も同じ dir に入る。

## 観測（`bash scripts/dev/tests/test-runner-tmp-repro.sh`、bash 5.2.37、2026-11-06）
小さな git fixture にこの checkout の runner 2 本の写しを置いて動かす。偽 cargo は `nextest run` の中で FIFO に ready を書き、release を読むまで止まる。
runner は専用の session で起こす（pgid = runner の pid）。比べたのは、通常終了と、TERM・KILL・INT・HUP を親だけ・group 全体へ送る場合である。
2 本の runner で同じ結果になった。

| 終わり方 | rc | trap | 一時 dir | 子（偽 cargo） |
|---|---|---|---|---|
| 通常終了 | 0 | 走る | 消える | — |
| TERM・HUP を親だけ | 143・129 | 走る | 消える | 多くは子の終わりを待つ。待たずに trap が走り、子の使用中に `TMPDIR` を消すことがある（後述） |
| INT を親だけ | 0（130 も許容） | 走る | 消える | bash は子の終了を待ち、子が INT で死ななければ script を**続けて** rc=0 で終わる |
| TERM・INT・HUP を group へ | 143・130・129 | 走る | 消える | 子も signal で死ぬ |
| KILL を親だけ | 137 | 走らない | **残る** | 孤児として生き続け、消えなかった `TMPDIR` に書き続ける（`child_tmp=ok`） |
| KILL を group へ | 137 | 走らない | **残る** | 子も死ぬ |

- EXIT trap を持つ bash 5.2 は TERM・INT・HUP を捕捉する（`/proc/<pid>/status` の SigCgt で確認した）。このため、一時 dir が残るのは SIGKILL の 4 case だけだった。
- 親だけへの TERM・HUP では、child_tmp=fail（子が使う `TMPDIR` を trap が先に消した）が 18 case 中 0〜3 件出た（この ADR を書くまでの 14 回の実行）。
  原因は、bash が子の `wait` に入る前に signal が届くと、その場で trap を走らせることだと推測している（bash の内部は読んでいない）。
  試験はこの case で ok と fail の両方を許し、末尾に fail の件数を出す。
- test-parallel.sh の `tests left N entries in TMPDIR (removed on exit)` は、削除の**前**に出る診断である。通常終了では、警告が出た後に dir が消えることを確かめた。
  削除できなかったことを示す警告ではない。削除できなかったときは、別の行 `could not remove …` が出る。

## 本番での発生経路
確定（コードを読んで確かめた）:
- 決定的 check の timeout（`crates/task-worker/src/workspace.rs` の `LocalWorkspace::exec`）は、TERM を送らない。
  `process_group::kill_group_and_descendants` で group と子孫へ直接 SIGKILL を送る。
  exec の future が drop されたときも、`GroupKillGuard` が同じ SIGKILL を送る。
  check の cmd が `bash scripts/dev/test-affected.sh` なら、上の表の「KILL を group へ」に当たり、一時 dir が残る。
  exec ごとの `TMPDIR`（`runs/exec-tmp-*`）は `Drop` で消える。しかし test-affected は一時 dir を `/tmp` に固定で作るので、その回収から外れる。
- worker run の打ち切り（`subprocess.rs`・`claude_code.rs` の `kill_now`）は、`send_signal_to_group` で harness の process group へ SIGTERM を送り、`kill_grace` 後に SIGKILL を送る。
  `reap_after_terminal` が `process_group::sweep` を呼ぶ経路も、SIGTERM の後で SIGKILL を送る。
  cargo の試験が `kill_grace` 内に終わらなければ、SIGKILL で一時 dir が残る。
- release gate（`scripts/selfdeploy/release.sh` の `run_step cargo-test`）は test-parallel.sh を前景で走らせ、時間上限を持たない。
  release.sh 自身が止められない限り、signal は届かない。

推測（確かめていない）:
- claude-code の Bash 道具が起こす command が、harness の process group に入るかは確認していない。
  入らない場合、`killpg` は届かない。run の終わりに harness が自分の道具の子を殺すときの signal の種類も、確認していない。
- test-parallel.sh の残骸 15 件が、どの経路で残ったかは特定していない。候補は、release.sh を人か systemd が止めた場合と、check の cmd で test-parallel を走らせて timeout した場合である。
- 残骸の mtime と、check の timeout・run の打ち切りの event は照合していない。

## 決定

### D1. 配置と path の予算
- Unix socket の sun_path は 108 byte（NUL を除くと 107）である。`/tmp`（tmpfs）・`/local/celeris/data/scratch`（btrfs）・run の `TMPDIR` のどこでも、107 byte の path を bind できることを実測した。
- 試験が `TMPDIR` の下に作る path のうち、既知で最長のものは 50 byte である。内訳は、`tempfile::tempdir()` の `/.tmpXXXXXX` が 11 byte、`/run/celeris-credentiald/injection.sock` が 39 byte。
  `TMPDIR` の予算は 107 − 50 = 57 byte とし、そこから 10 byte の余裕を引いた **47 byte 以下**とする。
  runner は起動時に長さを検査し、予算を超える候補は使わない。
- run の `TMPDIR`（`<workspace>/runs/<run_id>/tmp`）は、この run で 93 byte あった。予算を超えるので、置き場所にも fallback にもしない。
  これが 2 本の runner が `/tmp` 直下を使っていた理由である。
- 置き場所の候補は、上から順に次の 3 つとする。最初に検査を通ったものを使う。
  1. `$CELERIS_TEST_TMP_ROOT`（明示された root。予算と所有者の検査は同じ）。
  2. `/local/celeris/data/scratch/tr-<uid>`（scratch LVM、btrfs）。親の dir があり、書けるときだけ使う。`TMPDIR` は `…/tr-1001/XXXXXX/t` の 45 byte。
  3. `/tmp/celeris-tr-<uid>`（tmpfs）。`TMPDIR` は 29 byte。

候補の比較:

| | run TMPDIR | scratch LVM | /tmp（tmpfs） |
|---|---|---|---|
| path 長 | 93 byte（予算外） | 45 byte | 29 byte |
| 容量 | workspace と同じ disk | 大きい。cargo target と同じ thin pool なので、disk guard（ADR-0074）の監視下 | RAM と共有の 63G。29G の残骸で 46% を占め、93.6% まで埋まった |
| 回収 | run 終了で worker が消す | 次の起動で回収（D3） | 次の起動で回収（D3）。reboot で消える |
| sandbox からの可視性 | 見える | 見える（`CARGO_TARGET_DIR` がここにあり、cargo が書く。決定的 check の namespace からも見えるはずだが、直接は確かめていない） | 見える（この run から host の残骸 42 件が見えた） |
| Celeris 外の host | — | 無い（fallback へ） | 常にある |

scratch を優先する理由は、build の一時 file と試験の残り物が RAM を使わないことである。
scratch が無い host（開発者の手元）でも動くよう、`/tmp` への fallback を残す。fallback するときは stderr に 1 行出す。
fallback を消すと、Celeris 外の host で runner が動かなくなる。

### D2. 作成と owner metadata
- root は自分の UID が所有する 0700 の dir とする。symlink は使わない（`lstat` で確かめる）。
  root が存在して条件に合わないときは、その候補を使わず次へ進み、理由を stderr に出す。root を作り直したり、消したりはしない。
- 1 回の実行の dir は、`R/.init-XXXXXX` として 0700 で作る。中に `owner` file を書く。
  `owner` の内容は、`uid`・`pid`・`starttime`（`/proc/<pid>/stat` の 22 欄）・`sid`・`boot_id`（`/proc/sys/kernel/random/boot_id`）・`runner` 名・作成時刻である。
  `owner` は一時 file を書いてから `mv` で置く。その後で dir を `R/XXXXXX` へ rename して公開する。公開された dir には、必ず `owner` がある。
- runner は `R/XXXXXX/lock` を開き、`flock` を取ったまま fd を子へ継がせる（CLOEXEC を付けない）。
  cargo・nextest・試験 process は、fd を閉じない限りこの lock を持ち続ける。
- `TMPDIR` は `R/XXXXXX/t` とする。

### D3. 稼働中の保護と次回起動時の回収
- runner は起動時と終了時に `R` を走査し、終わった実行の dir を回収する。並行に走る掃除は `R/.sweep.lock` の `flock -n` で 1 つにする。取れなければ走査を飛ばす。
- 1 つの dir を「終わった」と判定するのは、次の 3 つがすべて成り立つときだけである。
  1. その dir の `lock` を `flock -n` で取れる（lock を継いだ process がもう居ない）。
  2. `owner` の `boot_id` が今の boot と違う。または、`pid` の process が無いか、その `starttime` が記録と違う（PID 再利用の判定）。
  3. `/proc/*/stat` を走査して、`owner` の `sid` を持つ process が無い。fd を閉じる子（`closefrom` する daemon）への備えである。
     runner は cargo を自分の session で起こす（D4）。
- `/proc` が読めない（hidepid 等）か、判定に要る欄が欠ける場合は「生きている」とみなして残す。
- 回収は、lock を持ったまま `R/.trash-XXXXXX` へ rename してから `rm -rf --one-file-system` する。rename の後で、別の runner がその dir を使うことはない。
- 回収の対象は、`R` 直下の実 dir（symlink でない）で、自分の UID が所有するものだけとする。それ以外は数だけを stderr に出し、触らない。
- `owner` の無い `R/.init-*`（初期化の途中で死んだ残り）は、作成から 1 時間を過ぎ、中身が `owner` の書きかけだけのときに限り回収する。それ以外は数を出して残す。
- SIGKILL は捕捉できないので、SIGKILL の後始末は、すべて次の起動時の回収に任せる。
  決定的 check の timeout（SIGKILL）で残った dir は、次に同じ UID で runner が起動したときに回収される。
  このとき孤児の子が残っていれば、D3 の 1・3 で保護される。

### D4. signal の扱い
- runner は cargo を `setsid` で別 session に起こし、その pid を覚える。TERM・INT・HUP の trap では次の順に処理する。
  1. 子の session へ同じ signal を送る。
  2. 子の終わりを最大 30 秒待つ。cargo の正常な停止を待つ保険の時間である。
  3. 子が終わったら dir を消す。終わらなければ dir を残し、D3 の回収に任せる。
  4. 128+N で終わる。
- 観測した 2 つの問題は、この手順で直る。1 つは、trap が子の使用中に `TMPDIR` を消すこと。もう 1 つは、INT を受けた後に script が rc=0 で続くこと。
- EXIT trap は、通常終了の後始末（dir を消す）だけを担う。

### D5. 両 runner への適用と限界
- D1〜D4 は、`scripts/dev/` の共通の helper（source して使う 1 file）に置く。test-affected.sh の crate 段と test-parallel.sh は、どちらもこの helper を使う。
  両 runner の一時 dir の作り方と後片付けは同じ形なので、共通化できる。test-affected の `changed` file は `${TMPDIR:-/tmp}` の下に置かれ、exec の `TMPDIR` と一緒に消えるので、変えない。
- test-parallel.sh の leftovers 警告は残す。上記のとおり削除前の診断であり、summary の `tmp_leftovers` に写る。
- 限界:
  - fd を閉じ、別 session へ抜け出した子孫は、D3 の 1・3 では見つからない。この場合、pid と starttime の判定だけになる。このような子孫は、使用中の dir を消されることがある。
  - 次の起動が無い host では、dir が残る。

### D6. 旧い残骸は人の手順で消す
- `/tmp/celeris-test-affected.*`・`/tmp/celeris-test-parallel.*` には owner metadata が無い。持ち主の生死を判定できないので、自動では消さない。新しい helper も、これらを走査しない。
- 人は次の手順で消す（本番 host の操作なので、agent は実行しない）。
  1. 一覧と量を確かめる: `ls -ld /tmp/celeris-test-affected.* /tmp/celeris-test-parallel.*` と `du -sch /tmp/celeris-test-affected.* /tmp/celeris-test-parallel.*`。
  2. 使っている process が無いことを確かめる: `for d in /tmp/celeris-test-affected.* /tmp/celeris-test-parallel.*; do ls -l /proc/[0-9]*/cwd /proc/[0-9]*/fd 2>/dev/null | grep -F "$d" && echo "in use: $d"; done`。
     `in use:` の行が出た dir は消さない。
  3. 所有者が自分であることを確かめてから消す: `find /tmp -maxdepth 1 \( -name 'celeris-test-affected.*' -o -name 'celeris-test-parallel.*' \) -user "$(id -u)" -type d -mmin +120 -exec sh -c 'chmod -R u+w "$1"; rm -rf --one-file-system "$1"' _ {} \;`。
  4. `df -h /tmp` で使用率が下がったことを確かめる。
- 新しい helper が入った後も、旧い prefix の dir が増えるなら、旧い runner（main に追従していない branch か release）が動いている。

## 帰結
- 通常終了と捕捉できる signal では、子の終了を待ってから dir を消す。SIGKILL では dir が残るが、次の起動時に回収される。
- build の一時 file と試験の残り物は、scratch LVM に置く。scratch の無い host では `/tmp` に置く。
- 回収は、PID 再利用・再起動・初期化の途中・並行する掃除・symlink・別 UID のどれでも、判定できなければ残す方に倒す。

## 実装済みの付記（2026-11-07）
実装は `scripts/dev/lib/test-tmp-lifecycle.sh` にあり、`scripts/dev/test-affected.sh` と `scripts/dev/test-parallel.sh` が使う。D1〜D5 の決定（TMPDIR 47 byte 以下、候補 3 つの順、owner metadata と flock、次回起動時の回収、setsid と 30 秒待ち）と実装は一致している。SIGKILL の後始末は次回起動時の回収である。D6 の旧い残骸は人の手順のままで、自動回収は実装していない。

## 範囲外
- daemon 側の check の timeout を、SIGTERM → SIGKILL の 2 段に変えること（`workspace.rs`）。別の判断として task の提案に書く。
- js_step（web・control-ts の段）の一時 file。これらは `TMPDIR` の下に作られ、exec の `TMPDIR` と一緒に消える。
