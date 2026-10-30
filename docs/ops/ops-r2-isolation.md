# ops-r2 の入力隔離: 評価器の別 uid（L1）と runtime の mount 許可表（L2）、人の準備手順

---
tasks: [01M66JYQ249SZHNXK0AEVTZNEC]
---

設計は [ADR 2026-10-30-ops-r2-isolation-principal](../../agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md)（人の決定 iso-boundary: 評価器専用の別 uid）。
この文書は、道具の使い方と、人が 1 回実行する準備手順を書く。**準備（§4）が終わるまで、正式試行の preflight は L1 の確認で必ず止まる**
（同じ uid では `EVAL_ROOT` の private を開けてしまうため）。本番 daemon・本番 DB・`~/.config/celeris` はこの手順では変えない。

## 1. 境界は 2 層

| 層 | 何で塞ぐか | 塞ぐもの |
|---|---|---|
| L1 | 評価器専用 uid `celeris-eval` が持つ 0700 の dir（`EVAL_ROOT`） | private grader（private.json・pristine 試験・隠し受入 command・期待値） |
| L2 | 試行の主体を、許可した path だけを mount した bwrap（`--unshare-pid`・新しい `/proc`）で起動する | 旧解答・他試行の出力・repo の clone（git に残った private を含む）・人の `~/.claude`・本番 KB |

主体と包み方:

- A の直接 agent: `Sandbox::runtime_args`（`crates/campaign/src/harness_exec.rs`。A の実起動と同じ関数）。
- B の測定 daemon（CoS run・worker run は daemon の子なので同じ namespace を継ぐ）と、実験専用 operator session: `scripts/campaign/ops-r2-sandbox.sh`。
- 評価器: `celeris-eval` uid で `EVAL_ROOT` から読む。試行の出力は drop box で受け取り、判定 JSON だけを outbox に返す。

同じ uid の chmod・mode 000・プロンプトでの禁止は、どちらの層にも数えない。

## 2. 道具の使い方

### 2.1 `celeris-campaign probe-read`（runtime の中から読めるかを確かめる）

```sh
celeris-campaign probe-read --principal <名前> --deny <path> [--deny <path> ...] \
  [--allow <許可する mount source> ...] [--out <probe.json>]
```

- 各 `--deny` の path を open して 1 byte 読み、親 dir を readdir する。`/proc/self/mountinfo` の source が `--allow` の外にあれば記録する。
- 出力は schema `celeris.runtime-probe/1` の JSON（stdout。`--out` があれば file にも）。読めた・一覧できた・許可外の source があれば exit 1。
- 主体の包み方の中で走らせる。例: `ops-r2-sandbox.sh ... -- <probe binary> probe-read --principal B --deny ...`。
- principal が `l1` か `l1:` で始まる probe は、人の uid の process から包まずに開いた L1 の確認として扱われる。

### 2.2 `scripts/campaign/ops-r2-sandbox.sh`（B・operator の L2）

```sh
sh scripts/campaign/ops-r2-sandbox.sh --state <試行の state> --release <release dir> \
  --account <測定専用 account dir> [--ro <追加の ro path> ...] [--print-args] -- <command> [args...]
```

- mount は ro の `/usr` `/bin` `/sbin` `/lib` `/lib64` `/etc`・release dir・`--ro`、rw の `--state` と account dir、tmpfs の `/tmp`、新しい `/proc` と `/dev` だけ。
  HOME は `--state/home`、環境変数は PATH と HOME だけ。network は共有する。
- `--print-args` は実行と同じ bwrap 引数を 1 行 1 引数で出し、bwrap を起動しない（preflight もこれで許可表を得る）。
- 人の `~/.claude`・`~/.config/celeris`・本番 state・workspaces・repo clone と、それらの祖先を指定すると exit 2。
  `projects/` か `memory/` を持つ account dir も exit 2（過去の transcript を持つ account は渡せない）。
- 試行ごとの state と HOME は `scripts/campaign/ops-r1-measure-config.sh` が作る。

### 2.3 `celeris-campaign preflight --deny-list`（deny_readable で止める）

正式試行（`--measure-config` 指定）では常に走り、1 件でも残れば exit 非 0 で有料実行・task 作成を 0 件で止める（fail-closed）。

```sh
celeris-campaign preflight ... \
  --deny-list <file> --eval-root <EVAL_ROOT> \
  [--sandbox-script scripts/campaign/ops-r2-sandbox.sh] [--probe-program <celeris-campaign>] \
  --b-state <dir> --b-release <dir> --b-account <dir> [--b-ro <path> ...] \
  --operator-state <dir> --operator-release <dir> --operator-account <dir> [--operator-ro <path> ...]
```

- `--deny-list` は 1 行 1 絶対 path（`#` 行と空行は無視、glob は展開しない）。承認票の deny list を実在の path に展開して書く。
  dir より中の file を書く（親を一覧できるだけでも hit になるため）。`--eval-root` の下の path を 1 件以上含める。
- 主体は A・B（A 以外の arm が有効なとき）・operator（正式試行では常に）・L1。missing は次の形で出る。
  - `deny_readable:<主体>:<path>`（読めた・親を一覧できた）
  - `deny_readable:<主体>:mount:<source>`（許可表の外の filesystem が見えた）
  - `deny_readable:<主体>:probe:<理由>`（probe が走らない。bwrap 不可など）
- `measure-launcher.sh` は env file の `DENY_LIST`・`EVAL_ROOT`・`SANDBOX_SCRIPT`・`PROBE_PROGRAM`・`B_STATE`・`B_RELEASE`・`B_ACCOUNT`・
  `OPERATOR_STATE`・`OPERATOR_RELEASE`・`OPERATOR_ACCOUNT` を同じ引数で渡す。

### 2.4 `check-ops-r1-leak.py --runtime-probe`（漏洩検査の可読判定を probe で置き換える）

```sh
python3 -I scripts/campaign/check-ops-r1-leak.py --private <private.json> --scan <dir> \
  --deny-readable <path> ... --runtime-probe <B の probe.json> --runtime-probe <L1 の probe.json> --out <json>
```

- probe に載った path は host の mode ではなく probe の結果で判定する。runtime と L1 の両方で拒否された path だけ `same_uid` の limit を外す。
- どれかの probe で読めた path、許可外の mount source、`ok=false` の probe は `deny_readable` の hit（exit 1）。詳細は [ops-r1 漏洩検査](ops-r1-leak-check.md)。

## 3. 実 bwrap の試験は userns が使えない場所では skip になる

`crates/campaign/tests/runtime_probe.rs` の実 bwrap 試験（`real_bwrap_probe_cannot_read_outside_a_allowlist`・`preflight_principals_probe_with_their_real_wrapping`）は
`CELERIS_USERNS_TESTS` が `1` か `require` のときだけ走る（ADR-0126）。未指定では本体を実行せず、stderr に
`skip: ... requires CELERIS_USERNS_TESTS=1 (or require)` を出して return する（nextest は passed と数える）。
userns が使えない Celeris worker の sandbox では、この試験は skip になる。実際に走らせるには、bwrap と user namespace が使える shell で:

```sh
CELERIS_USERNS_TESTS=require cargo nextest run -p celeris-campaign --test runtime_probe --success-output immediate
```

`require` は namespace が作れなければ失敗にする（`1` は作れなければ skip）。確認: 6 passed で、出力に `skip:` の行が無いこと。

## 4. 人が実行する準備手順

sudo が要る。値は例で、`EVAL_ROOT=/local/celeris/eval`、drop box `/local/celeris/eval-inbox`、outbox `/local/celeris/eval-outbox`、人の uid は `rmaeda`。

### 4.1 `celeris-eval` の uid・group を作る

```sh
sudo groupadd --system celeris-eval
sudo useradd --system --gid celeris-eval --home-dir /local/celeris/eval --no-create-home \
  --shell /usr/sbin/nologin celeris-eval
```

確認: `id celeris-eval` が uid・gid を出し、`id rmaeda` の groups に `celeris-eval` が無いこと。
（`sudo -iu` で入る案（§4.4 案 2）を選ぶなら、shell は `/bin/bash` にする。）

### 4.2 `EVAL_ROOT`（0700）・drop box（1733）・outbox（0755）を作る

```sh
sudo install -d -o celeris-eval -g celeris-eval -m 0700 /local/celeris/eval
sudo install -d -o celeris-eval -g celeris-eval -m 0700 /local/celeris/eval/private /local/celeris/eval/work /local/celeris/eval/out
sudo install -d -o celeris-eval -g celeris-eval -m 1733 /local/celeris/eval-inbox
sudo install -d -o celeris-eval -g celeris-eval -m 0755 /local/celeris/eval-outbox
```

確認:

```sh
stat -c '%U %a %n' /local/celeris/eval /local/celeris/eval-inbox /local/celeris/eval-outbox
# celeris-eval 700 /local/celeris/eval
# celeris-eval 1733 /local/celeris/eval-inbox
# celeris-eval 755 /local/celeris/eval-outbox
ls /local/celeris/eval        # rmaeda から: Permission denied
ls /local/celeris/eval-inbox  # rmaeda から: Permission denied（書けるが一覧できない）
getfacl -p /local/celeris/eval | grep -v '^#'   # ACL の追加エントリが無いこと
```

### 4.3 採点 wrapper と採点 binary を置く（`celeris-eval` 持ち、人は書けない）

```sh
sudo install -d -o celeris-eval -g celeris-eval -m 0755 /local/celeris/eval-bin
sudo install -o celeris-eval -g celeris-eval -m 0555 <release の celeris-campaign> /local/celeris/eval-bin/celeris-campaign
sudo install -o celeris-eval -g celeris-eval -m 0555 scripts/campaign/check-ops-r1-leak.py /local/celeris/eval-bin/check-ops-r1-leak.py
sudo install -o celeris-eval -g celeris-eval -m 0555 <採点 wrapper> /local/celeris/eval-bin/grade
```

採点 wrapper は、drop box の tgz を sha256 で照合して `work/` に展開し、`celeris-campaign` を `--grader-root /local/celeris/eval/private` で走らせ、
判定 JSON を `check-ops-r1-leak.py --private <EVAL_ROOT の private.json> --scan <判定 JSON>` に通してから outbox に 0644 で置く（ADR D4）。
wrapper 自体はこの repo にまだ無い（§6）。

確認: `stat -c '%U %a %n' /local/celeris/eval-bin /local/celeris/eval-bin/*` がすべて `celeris-eval` で、`test -w /local/celeris/eval-bin/grade || echo ok` が `ok`。

### 4.4 評価器の起動方法（どちらかを選ぶ）

- 案 1（狭い sudo 規則）: `sudo visudo -f /etc/sudoers.d/celeris-eval` で 1 行だけ書く。

  ```
  rmaeda ALL=(celeris-eval) NOPASSWD: /local/celeris/eval-bin/grade
  ```

  起動: `sudo -u celeris-eval /local/celeris/eval-bin/grade <引数>`。
  確認: `sudo -l -U rmaeda` の celeris-eval の行が `/local/celeris/eval-bin/grade` だけであること、`sudo -n -u celeris-eval /bin/cat /local/celeris/eval/private/...` が拒否されること。
- 案 2（`sudo -iu`）: sudoers は変えず、人が `sudo -iu celeris-eval` で入った shell で `/local/celeris/eval-bin/grade` を実行する。
  人の対話が毎回要るが、自動の経路は作らない。確認: その shell で `id -un` が `celeris-eval`。

案 1 では人の uid の process（bwrap の外の普段の operator session を含む）が `grade` を起動できる。`grade` が返すのは漏洩検査を通した判定 JSON だけなので、private は出ない。
試行の主体（A・B・operator）は bwrap の中で動く。bwrap は no_new_privs を立てるので、中から sudo の setuid は効かないはずだが、実機では未確認。
準備後に `sh scripts/campaign/ops-r2-sandbox.sh ... -- /usr/bin/sudo -n -u celeris-eval /local/celeris/eval-bin/grade` が失敗することを確かめる。

### 4.5 正式 case の private を `EVAL_ROOT` へ移す

`--grader-root` は `<root>/final/<case>/private.json` の配置を読むので、同じ配置で置く。

```sh
case=<正式試行の case id>
sudo install -d -o celeris-eval -g celeris-eval -m 0700 /local/celeris/eval/private/final /local/celeris/eval/private/final/$case
sudo install -o celeris-eval -g celeris-eval -m 0400 \
  crates/campaign/fixtures/grader-private/final/$case/private.json /local/celeris/eval/private/final/$case/private.json
sudo -u celeris-eval sha256sum /local/celeris/eval/private/final/$case/private.json   # 承認票の sha256 と一致すること
```

repo の HEAD から消すか（ADR D4）は、K2 については人の決定待ち（ADR「人に決めてもらう点」1）。消しても git の履歴と既存の clone には残るので、
それらは L2 で塞ぎ、deny list に clone の `crates/campaign/fixtures/grader-private/final/<case>/private.json` を載せる。

### 4.6 測定専用 Claude account dir に login する

B の測定 daemon 用と実験専用 operator session 用に、新しい dir を 1 つずつ作り、人が 1 回 login する。人が普段使う account dir と既存の credential は変えない・消さない。

```sh
install -d -m 0700 /local/celeris/measure-accounts/b /local/celeris/measure-accounts/operator
CLAUDE_CONFIG_DIR=/local/celeris/measure-accounts/b claude   # /login を済ませて終了
CLAUDE_CONFIG_DIR=/local/celeris/measure-accounts/operator claude
```

確認: 各 dir に `.credentials.json` があり、`projects/` と `memory/` が無いこと
（`sh scripts/campaign/ops-r2-sandbox.sh --state <空の dir> --release <release> --account <dir> --print-args -- true` が exit 0。projects があれば exit 2）。
login 後に試し起動すると `projects/` ができるので、できたら消してから使う。

### 4.7 人の uid から `EVAL_ROOT` の private を開けないことを確かめる

```sh
cat /local/celeris/eval/private/final/<case>/private.json        # Permission denied
celeris-campaign probe-read --principal l1 \
  --deny /local/celeris/eval/private/final/<case>/private.json --out l1.json; echo "exit=$?"
```

期待: `exit=0`、`l1.json` の該当 path が `readable: false`。exit 1 なら L1 が成り立っていない（持ち主・mode・ACL・group を §4.1〜4.2 の確認で見直す）。
その後、正式試行の deny list で `celeris-campaign preflight --deny-list ... --eval-root /local/celeris/eval ...` を実行し、
`deny_readable` の missing が 0 件であることを確かめる。

## 5. 本番への影響

本番 daemon・本番 DB・`~/.config/celeris`・本番の release は変えない。測定 daemon の config は試行ごとに `ops-r1-measure-config.sh` が作る。
§4 で増えるのは uid・group 1 つ、`/local/celeris/eval*` の dir、測定専用 account dir、（案 1 なら）sudoers の 1 行。

## 6. 残っていること

- 採点 wrapper（§4.3 の `grade`）はまだ無い。今の採点は人の uid で `--grader-root` を読む。
- release dir に repo の source（fixtures）が入っていないことは未確認（ADR「残る経路と限界」）。
- network namespace は共有するので、bwrap の中から認証の要らない loopback の endpoint（health 等）は読める。
