# ops-r2 の入力隔離: 評価器の別 uid（L1）と runtime の mount 許可表（L2）、人の準備手順

---
tasks: [01M66JYQ249SZHNXK0AEVTZNEC]
---

設計は [ADR 2026-10-30-ops-r2-isolation-principal](../../agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md)（人の決定 iso-boundary: 評価器専用の別 uid）。
この文書は、道具の使い方と、人が 1 回実行する準備手順を書く。**準備（§4）が終わるまで、正式試行の preflight は L1 の確認で必ず止まる**
（人の uid が `EVAL_ROOT` か祖先を持っている、または書けるため。§2.3）。本番 daemon・本番 DB・`~/.config/celeris` はこの手順では変えない。
K2 の試行前に通す確認の順序は [K2 の開始前チェック](ops-r2-k2-preflight.md) にまとめた。

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
L1 は open の拒否では判定しない。`EVAL_ROOT` から deny path までの持ち主の uid が run の uid でも 0 でもなく、
run の uid がそれらと祖先に chmod・rename できないことで判定する（§2.3）。

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
  - `deny_readable:L1:<path>:<理由> <要素> (uid <n>, mode <o>)`（L1 の持ち主の判定で止まった。下の表）
- L1 は、probe で開けなくても、`--eval-root` の下の deny path について lstat で次を確かめる（`crates/campaign/src/l1_owner.rs`）。
  run の uid は preflight を走らせた process の実効 uid と所属 group（`/proc/self/status`）。1 つでも満たさなければ止める。

  | 対象 | 止める理由（`<理由>`） |
  |---|---|
  | `EVAL_ROOT` から path までの各要素 | 持ち主が run の uid（`owned_by_run_uid`。持ち主は chmod で読めるように戻せる）、持ち主が 0（`owned_by_root`）、run の uid から書ける（`writable_by_run_uid`。sticky でも止める）、symlink（`symlink`） |
  | `EVAL_ROOT` の祖先（`/` まで） | 持ち主が run の uid（`ancestor_owned_by_run_uid`。rename で `EVAL_ROOT` を差し替えられる）、run の uid から書ける（`ancestor_writable_by_run_uid`。sticky の dir は他人の entry を rename できないので可）、symlink（`symlink`） |
  | stat できない | `EVAL_ROOT` と祖先は止める（`stat_failed`）。`EVAL_ROOT` より下で ENOENT・EACCES になった所では歩みを止めて通す（親が上の条件を満たすので、run の uid はそこに作ることも覗くこともできない） |
  | `--eval-root` の外の deny path | `not_under_eval_root` |

  書けるかは、持ち主・group・その他のうち run の uid に当てはまる 1 組の bit で決める。run の uid が 0 なら常に書けるとみなすので、root で走らせた preflight は L1 で必ず止まる。
  同じ uid の dir と file を chmod 000 にした `EVAL_ROOT` は、open が EACCES でも `owned_by_run_uid` で止まる。
- 迂回の env 変数・CLI flag は無い。`celeris.runtime-probe/1` の JSON の形は変えていない（持ち主の判定は probe の外で行う）。
- `measure-launcher.sh` は env file の `DENY_LIST`・`EVAL_ROOT`・`SANDBOX_SCRIPT`・`PROBE_PROGRAM`・`B_STATE`・`B_RELEASE`・`B_ACCOUNT`・
  `OPERATOR_STATE`・`OPERATOR_RELEASE`・`OPERATOR_ACCOUNT` を同じ引数で渡す。

### 2.4 `check-ops-r1-leak.py --runtime-probe`（漏洩検査の可読判定を probe で置き換える）

```sh
python3 -I scripts/campaign/check-ops-r1-leak.py --private <private.json> --scan <dir> \
  --deny-readable <path> ... --runtime-probe <B の probe.json> --runtime-probe <L1 の probe.json> --out <json>
```

- probe に載った path は host の mode ではなく probe の結果で判定する。
- `same_uid` の limit を外す（`same_uid_limit: false`、`same_uid_cleared_by: runtime_probe_and_l1`）のは、runtime と L1 の両方の probe で拒否され、
  かつ `owned_by_run_uid`（host の stat で、path と祖先のうち run の uid が持ち主のもの）が空の path だけ。
  `owned_by_run_uid` が空でなければ、両方の probe が拒否でも `same_uid` の limit を残し、`--canary` が無ければ `same_uid_without_canary` の hit（exit 1）になる。
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

sudo が要る。値は例で、`EVAL_ROOT=/var/lib/celeris-eval`、drop box `/var/lib/celeris-eval-inbox`、outbox `/var/lib/celeris-eval-outbox`、人の uid は `rmaeda`。

### 4.1 `celeris-eval` の uid・group を作る

```sh
sudo groupadd --system celeris-eval
sudo useradd --system --gid celeris-eval --home-dir /var/lib/celeris-eval --no-create-home \
  --shell /usr/sbin/nologin celeris-eval
```

確認: `id celeris-eval` が uid・gid を出し、`id rmaeda` の groups に `celeris-eval` が無いこと。
shell は `/usr/sbin/nologin` のままにする。評価器の uid の shell に入るときは login shell を使わず `sudo -u celeris-eval /bin/bash` で入る（§4.4 案 2・§4.5 も同じ入り方）。

### 4.2 `EVAL_ROOT`（0700）・drop box（1733）・outbox（0755）を作る

`EVAL_ROOT` は、root が持ち人の uid が書けない祖先の下に、symlink を経ない実 path で置く（§2.3 の L1 の条件）。
`/local/celeris` の下は使わない。`/local` と `/local/celeris` は人の uid（rmaeda）が持つので、人の uid は `EVAL_ROOT` を rename で差し替えられ、
preflight は `ancestor_owned_by_run_uid` で止まる。drop box・outbox・採点 wrapper の dir も同じ理由で `/var/lib` の下に置く
（祖先を人の uid が持つと、§4.4 の sudo 規則が指す `grade` を人の uid が差し替えられる）。

```sh
sudo install -d -o celeris-eval -g celeris-eval -m 0700 /var/lib/celeris-eval
sudo install -d -o celeris-eval -g celeris-eval -m 0700 /var/lib/celeris-eval/private /var/lib/celeris-eval/work /var/lib/celeris-eval/out
sudo install -d -o celeris-eval -g celeris-eval -m 1733 /var/lib/celeris-eval-inbox
sudo install -d -o celeris-eval -g celeris-eval -m 0755 /var/lib/celeris-eval-outbox
```

確認:

```sh
stat -c '%U %a %n' /var/lib/celeris-eval /var/lib/celeris-eval-inbox /var/lib/celeris-eval-outbox
# celeris-eval 700 /var/lib/celeris-eval
# celeris-eval 1733 /var/lib/celeris-eval-inbox
# celeris-eval 755 /var/lib/celeris-eval-outbox
stat -c '%U %a %n' / /var /var/lib   # どれも root が持ち、rmaeda が書けない（755 など）こと
ls /var/lib/celeris-eval        # rmaeda から: Permission denied
ls /var/lib/celeris-eval-inbox  # rmaeda から: Permission denied（書けるが一覧できない）
getfacl -p /var/lib/celeris-eval | grep -v '^#'   # ACL の追加エントリが無いこと
```

### 4.3 採点 wrapper と採点 binary を置く（`celeris-eval` 持ち、人は書けない）

```sh
sudo install -d -o celeris-eval -g celeris-eval -m 0755 /var/lib/celeris-eval-bin
sudo install -o celeris-eval -g celeris-eval -m 0555 <release の celeris-campaign> /var/lib/celeris-eval-bin/celeris-campaign
sudo install -o celeris-eval -g celeris-eval -m 0555 scripts/campaign/check-ops-r1-leak.py /var/lib/celeris-eval-bin/check-ops-r1-leak.py
sudo install -o celeris-eval -g celeris-eval -m 0555 scripts/campaign/ops-r2-grade.py /var/lib/celeris-eval-bin/grade
```

採点 wrapper は、drop box の tgz を sha256 で照合して `work/` に展開し、`celeris-campaign` を `--grader-root /var/lib/celeris-eval/private` で走らせ、
判定 JSON を `check-ops-r1-leak.py --private <EVAL_ROOT の private.json> --scan <判定 JSON>` に通してから outbox に 0644 で置く（ADR D4）。
wrapper は `scripts/campaign/ops-r2-grade.py`（試験は `scripts/campaign/test_ops_r2_grade.py`）。起動は
`grade --tgz <drop box の tgz> --sha256 <その sha256> --case <case id> -- <celeris-campaign の引数>` で、引数の `{work}` は展開先、`{verdict}` は判定 JSON の書き先に置き換わる（`{verdict}` は必須）。
`--grader-root` は wrapper が付け、引数に `--grader-root`・`..`・`work/` の外の絶対 path は書けない。stdout に出すのは outbox の判定 JSON の path だけで、
失敗は短い理由だけを返す。`celeris-campaign` と漏洩検査の出力は `EVAL_ROOT/work/` の中にだけ残す。漏洩検査が通らなければ outbox に何も置かない。
起動した uid が `EVAL_ROOT` の持ち主でない、または `EVAL_ROOT` が group/other に開いていれば止まる。
wrapper の sha256 を書く欄は承認票に足さない。install 元は repo の commit で決まるので、開始前チェック（`ops-r2-k2-preflight.md` §0）で
`/var/lib/celeris-eval-bin/grade` と repo の `scripts/campaign/ops-r2-grade.py` の sha256 が一致することを確かめる。

確認: `stat -c '%U %a %n' /var/lib/celeris-eval-bin /var/lib/celeris-eval-bin/*` がすべて `celeris-eval` で、`test -w /var/lib/celeris-eval-bin/grade || echo ok` が `ok`。

### 4.4 評価器の起動方法（どちらかを選ぶ）

- 案 1（狭い sudo 規則）: `sudo visudo -f /etc/sudoers.d/celeris-eval` で 1 行だけ書く。

  ```
  rmaeda ALL=(celeris-eval) NOPASSWD: /var/lib/celeris-eval-bin/grade
  ```

  起動: `sudo -u celeris-eval /var/lib/celeris-eval-bin/grade <引数>`。
  確認: `sudo -l -U rmaeda` の celeris-eval の行が `/var/lib/celeris-eval-bin/grade` だけであること、`sudo -n -u celeris-eval /bin/cat /var/lib/celeris-eval/private/...` が拒否されること。
- 案 2（評価器の shell）: sudoers は変えず、人が `sudo -u celeris-eval /bin/bash` で入った shell で `/var/lib/celeris-eval-bin/grade` を実行する。
  人の対話が毎回要るが、自動の経路は作らない。確認: その shell で `id -un` が `celeris-eval`。

案 1 では人の uid の process（bwrap の外の普段の operator session を含む）が `grade` を起動できる。`grade` が返すのは漏洩検査を通した判定 JSON だけなので、private は出ない。
試行の主体（A・B・operator）は bwrap の中で動く。bwrap は no_new_privs を立てるので、中から sudo の setuid は効かないはずだが、実機では未確認。
準備後に `sh scripts/campaign/ops-r2-sandbox.sh ... -- /usr/bin/sudo -n -u celeris-eval /var/lib/celeris-eval-bin/grade` が失敗することを確かめる。

### 4.5 正式 case の新しい private を repo の外で作り `EVAL_ROOT` に置く

K2 の private は作り直す（人の決定 k2-private-in-git = reauthor、ADR 付記 2026-10-30「K2 の private の作り直し」）。
repo にある K2 の private は git の履歴と clone に残る漏洩済みの旧解答で、採点にも照合にも使わない。

- 作る主体: 人、または人が `sudo -u celeris-eval /bin/bash` で入った評価器 uid の shell。Celeris の worker・CoS・外部 operator は新しい private を見ない・作らない。
- 置き場所: `/var/lib/celeris-eval/private/final/<case>/private.json`（持ち主 `celeris-eval`、0400）。repo の作業ツリー・Celeris の作業場所・KB・artifacts・`/tmp` には置かない。commit しない。
- `--grader-root /var/lib/celeris-eval/private` は `<root>/final/<case>/private.json` を読むので、この配置にする。

```sh
case=k2-bf-xattr-restart-campaign
sudo install -d -o celeris-eval -g celeris-eval -m 0700 /var/lib/celeris-eval/private/final /var/lib/celeris-eval/private/final/$case
sudo -u celeris-eval /bin/bash            # 以下は celeris-eval の shell
umask 077
case=k2-bf-xattr-restart-campaign
f=/var/lib/celeris-eval/private/final/$case/private.json
vi "$f"                          # 新しい private をここで直接書く（書式は下）
chmod 0400 "$f"
```

書式は grader の読込（`crates/campaign/src/cases.rs` の `CasePrivate` と `CasePrivate::validate`）に合わせる。

| 欄 | 条件 |
|---|---|
| `schema` | `"celeris.campaign-case-private/1"` |
| `case_id`・`split` | case の id（`k2-bf-xattr-restart-campaign`）と `"final"` |
| `hidden_command` | 空でない文字列（隠し受入 command） |
| `baseline_command` | 任意。文字列 |
| `known_defects` | 1 件以上。各要素は `{id, description}` だけ |
| `false_claim_traps` | 1 件以上。各要素は `{id, claim, why_wrong}` だけ |
| `expected` | 参照値・再計算の仕様（任意の JSON） |

上の 8 欄以外の key は読込で拒否される。`known_defects` と `false_claim_traps` の `id` は合わせて重複しない。
同じ shell で次を実行し、`ok` だけが出ることを確かめる（中身は出さない）。

```sh
python3 -I - "$f" "$case" <<'PY'
import json, sys
p, cid = sys.argv[1], sys.argv[2]
d = json.load(open(p))
req = {"schema", "case_id", "split", "hidden_command", "known_defects", "false_claim_traps", "expected"}
assert req <= d.keys() <= req | {"baseline_command"}, "keys"
assert d["schema"] == "celeris.campaign-case-private/1", "schema"
assert d["case_id"] == cid and d["split"] == "final", "case_id/split"
assert isinstance(d["hidden_command"], str) and d["hidden_command"].strip(), "hidden_command"
assert isinstance(d.get("baseline_command", ""), str), "baseline_command"
kd, ft = d["known_defects"], d["false_claim_traps"]
assert kd and ft, "real case needs known_defects and false_claim_traps"
assert all(isinstance(x, dict) and set(x) == {"id", "description"} and all(isinstance(v, str) for v in x.values()) for x in kd), "known_defects"
assert all(isinstance(x, dict) and set(x) == {"id", "claim", "why_wrong"} and all(isinstance(v, str) for v in x.values()) for x in ft), "false_claim_traps"
ids = [x["id"] for x in kd + ft]
assert len(ids) == len(set(ids)), "duplicate defect/trap id"
print("ok")
PY
stat -c '%U %a' "$f"             # celeris-eval 400
sha256sum "$f"                   # この値を人が承認票の「K2 の新しい private の sha256」欄に書く
```

sha256 の記録先は承認票（人が書く。operator は書かない）。開始前チェック（`ops-r2-k2-preflight.md` §0・§1）と採点の前の照合は、承認票のこの値とだけ比べる。
新しい private の中身・sha256 を Celeris の task・進捗・KB には書かない（承認票の sha256 だけを試行の証跡で参照する）。

`CaseSet::load` は公開部 `case.json` の `private_ref.sha256` と private の実 file の sha256 も照合する。repo の K2 の `case.json` は
旧 private の値のままなので、このままでは `--grader-root /var/lib/celeris-eval/private` で `private file hash mismatch` になる。
正式試行の前に、人が承認票の値を渡して `python3 -I scripts/campaign/ops-r2-set-private-sha.py --case k2-bf-xattr-restart-campaign --sha256 <承認票の値>` を実行し、
`case.json` の `private_ref.sha256` だけを替えて commit する（script は private を読まない。漏洩済みの旧値 `d02126077a47…0694760` は拒否する）。
frozen manifest が固定する `materials_sha256` は `private_ref` を除いた公開部から作るので、manifest の再 freeze は要らない（script が公開部の不変を確かめ、変われば止まる）。
値の反映は承認票の値が出てから行う（2026-10-30 の時点では未実施）。

旧 private の repo の path は、漏洩済みの旧解答として deny list に残す。git の履歴と既存の clone にも残るので L2 で塞ぎ、
deny list に clone の `crates/campaign/fixtures/grader-private/final/<case>/private.json` を載せる（その中身は採点に使わない）。

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
cat /var/lib/celeris-eval/private/final/<case>/private.json        # Permission denied
celeris-campaign probe-read --principal l1 \
  --deny /var/lib/celeris-eval/private/final/<case>/private.json --out l1.json; echo "exit=$?"
```

期待: `exit=0`、`l1.json` の該当 path が `readable: false`。exit 1 なら L1 が成り立っていない（持ち主・mode・ACL・group を §4.1〜4.2 の確認で見直す）。
probe の拒否だけでは L1 の確認にならない（同じ uid の chmod 000 でも拒否になる）。
その後、正式試行の deny list で `celeris-campaign preflight --deny-list ... --eval-root /var/lib/celeris-eval ...` を実行し、
`deny_readable` の missing が 0 件であることを確かめる。`deny_readable:L1:<path>:<理由> ...` が出たら、§2.3 の表の理由と、
示された要素の uid・mode を直す。

## 5. 本番への影響

本番 daemon・本番 DB・`~/.config/celeris`・本番の release は変えない。測定 daemon の config は試行ごとに `ops-r1-measure-config.sh` が作る。
§4 で増えるのは uid・group 1 つ、`/var/lib/celeris-eval*` の dir、測定専用 account dir、（案 1 なら）sudoers の 1 行。

## 6. 残っていること

- 採点 wrapper（§4.3）の試験は、試験の uid が持つ `EVAL_ROOT` で行い、uid の不一致は `CELERIS_EVAL_TEST_EUID` で注入する。実 uid（`celeris-eval`）での起動と sudo 規則は人の準備の後に §4.4・§4.7 で確かめる。
- release dir に repo の source（fixtures）が入っていないことは未確認（ADR「残る経路と限界」）。
- network namespace は共有するので、bwrap の中から認証の要らない loopback の endpoint（health 等）は読める。
- 別 uid（`celeris-eval`）が持つ `EVAL_ROOT` の中に実在する private で L1 が通る e2e は無い（試験は別の uid を作れない）。
  通る規則は `l1_owner` の単体試験で run の uid を注入して確かめ、実 binary の通過は host にある他 uid 持ちの dir（中の path は存在しない）で確かめている。
  実 uid での確認は人の準備の後に §4.7 で行う。
- K2 の新しい private は人が作る（§4.5、未実施）。repo の K2 の `case.json` の `private_ref.sha256` は旧 private の値のままで、
  新しい private を `--grader-root` で読むと `private file hash mismatch` になる。替える手順は §4.5 の `ops-r2-set-private-sha.py`。承認票の値での反映は人の値待ち。
