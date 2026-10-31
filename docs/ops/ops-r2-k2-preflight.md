---
tasks: [01M669QB069GND2ZKA1T97QPY9]
---
# ops-r2 K2 の開始前チェック: 隔離 preflight・漏洩検査・operator principal・session-map・停止規則

K2（`k2-bf-xattr-restart-campaign`）を ops-r2 の条件で正式に試行する前に、operator（実験専用 session か人）がこの順で 1 回ずつ確かめる。
1 項目でも期待と違えば試行を始めない。設計は [ADR 2026-10-30-ops-r2-isolation-principal](../../agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md)。
各道具の詳しい使い方は [入力隔離](ops-r2-isolation.md)・[ops-r1 漏洩検査](ops-r1-leak-check.md)・[CoS 受付の記録](ops-r1-intake.md)・
[実験専用 operator session](ops-r2-operator-session.md)・[測定 daemon](ops-r1-measure-daemon.md)・[pilot runbook](mlsys-main-4-pilot-runbook.md) にある。

このチェックは LLM・実 HPC 負荷を使わない。本番 daemon・本番 DB・`~/.config/celeris` は変えない。
2026-10-31 に operator が §1〜§4 を実機で実行し、§1 は合格、§2〜§4 は旧い手順の不備（方針版が固定の 6、B の account の置き方、manifest の欠け、
deny list の引数上限、NFS 上の claude、漏洩検査の canary と L1 の mount）で止まった。この版はその不備を直した手順で、新しい手順で §2〜§6 を通した記録はまだ無い（未確認）。
やり直しの順序は [進捗](../../agent-docs/progress/2026-10-31-ops-r2-preflight-fixes/docs.md) の「本番への移行」にある。

以下の変数は例。`CC` は固定した `celeris-campaign`、`CLAUDE`・`CLAUDE_SHA` は §2.1 の local の claude copy とその sha256。`P` は試行の証跡 dir（試行ごとに新しい dir）、`S` は B の試行 state、`O` は operator session の state、
`R` は固定した release dir、`EVAL_ROOT=/var/lib/celeris-eval`、`case=k2-bf-xattr-restart-campaign`。

## 0. 人の準備が済んでいる

[入力隔離 §4](ops-r2-isolation.md#4-人が実行する準備手順) の 4.1〜4.7 を人が済ませている。加えて次が済んでいる。

| 項目 | 確かめ方 | 期待 |
|---|---|---|
| K2 の新しい private（人の決定 k2-private-in-git = reauthor。ADR 付記 2026-10-30「K2 の private の作り直し」） | 承認票の「K2 の新しい private の sha256」欄。作成は [入力隔離 §4.5](ops-r2-isolation.md#45-正式-case-の新しい-private-を-repo-の外で作り-eval_root-に置く) | 人（または評価器 uid の shell）が repo の外で作り、`$EVAL_ROOT/private/final/$case/private.json` に `celeris-eval` 持ち 0400 で置き、§4.5 の書式確認が `ok`、sha256 を人が承認票に書いている。欄が空なら始めない |
| K2 の `case.json` の `private_ref.sha256` | `python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["private_ref"]["sha256"])' crates/campaign/cases/final/$case/case.json` | 承認票の新しい private の sha256 と同じ。違えば grader の読込が `private file hash mismatch` で止まるので始めない（2026-10-30 の時点では承認票の値が無く、一致しない。値が出たら [入力隔離 §4.5](ops-r2-isolation.md) の `ops-r2-set-private-sha.py` で替えて commit する） |
| K2 の `case.json` の `grade_case_ref.sha256` | `python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["grade_case_ref"]["sha256"])' crates/campaign/cases/final/$case/case.json` | 承認票の grade-case の sha256（評価器が `$EVAL_ROOT/grade-cases` に `gen-grade-case` で作ったもの）と同じで、repo の旧 grade-case の値 `f84089bcf161…190aca593` ではない。旧値のままなら始めない（[入力隔離 §4.5](ops-r2-isolation.md)） |
| 採点 wrapper の install | `sha256sum /var/lib/celeris-eval-bin/grade scripts/campaign/ops-r2-grade.py` | 2 つの sha256 が同じ（install は [入力隔離 §4.3](ops-r2-isolation.md)） |
| 承認票の欄（試行数・上限・model・account・停止条件・期限・固定依頼文の sha256・K2 の新しい private の sha256・deny list・実験専用 session の id・評価器の起動方法） | 承認票を開く | すべて人が埋めている。operator が空欄を埋めていない（K4 の逸脱を繰り返さない） |
| 評価器の uid | `id celeris-eval`、`id -nG` | uid がある。人の uid の groups に `celeris-eval` が無い |
| 測定専用 account dir（B 用・operator 用の 2 つ） | `ls -A <dir>` | `.credentials.json` があり、`projects/`・`memory/` が無い。1 dir = 1 account で、B の測定 daemon の pool には B 用の 1 つだけを載せる（§2.2）。A・operator の dir は pool に入れない |
| 測定専用の local の claude copy | `"$CLAUDE" --version`、`sha256sum "$CLAUDE"` | 版と sha256 が承認票と同じ（§2.1）。NFS 上の claude を測定に使わない |

## 1. release dir に private が入っていない

試行の主体は release dir を ro で見る（L2 の許可表の中）。ここに private の写しがあると deny list と mountinfo では見つからない。

```sh
sha256sum "$EVAL_ROOT/private/final/$case/private.json"   # 評価器の uid（celeris-eval の shell）で。承認票の「K2 の新しい private の sha256」と一致
grep -rlaF false_claim_traps "$R" | wc -l                  # 0
find "$R" -path '*grader-private*' | wc -l                 # 0
```

照合に使うのは承認票の新しい private の sha256 だけ。repo と git の履歴にある旧 private（漏洩済みの旧解答）の sha256 とは比べない。

`grep -rlaF grader-private "$R"` が `notes.md`・`notes.json`・`promote.log` だけを返すのは、release の変更記録に path 名が出るためで問題ない。
2026-10-30 に `/local/celeris/state/releases` の 4 件で確かめた結果: `false_claim_traps` 0 件、K2 の旧 private の hidden command の先頭 60 byte 0 件（新しい private は repo に入らないので release にも入らない）、
`grader-private` の文字列は notes と promote.log だけ（release に `celeris-campaign` の binary は入っていない）。

## 2. 測定専用の local claude と、測定 daemon の config

### 2.1 local の claude copy

NFS 上の claude を A・B・operator の包みの中に見せると、NFS の mount が許可表の外の source になり、preflight が `deny_readable:A:mount:...` で止まる
（許可表に NFS を足さない。他の利用者の書き換えを受け、許可表の外の木を見せるため。ADR 2026-10-31-ops-r2-preflight-fixes D4）。
測定で使う claude は local の固定 copy だけにする。写した後は書き換えない。K2 ops-r2 の manifest が pin するのは次の copy。

```sh
CLAUDE=/local/celeris/data/measure/ops-r2-k2-p1/bin/claude-2.1.303   # 版を上げるときは新しい dir に写し、承認票と manifest を人が改める
"$CLAUDE" --version                                                   # 2.1.303（承認票の版と同じ）
sha256sum "$CLAUDE"                                                   # 375988f457756524234695cf2d8f64ace99c5f68682e47ea24862a843c191450（manifest の claude_code_sha256、承認票の値）
```

版か sha256 が承認票と違えば始めずに人へ返す。この sha256 を `CLAUDE_SHA` として §3 の `--claude-sha256` に渡す。

### 2.2 config を作る

方針版は引数で決める。`--policy-version` は必須で、承認票の方針版（K2 ops-r2 では 10）を渡す。固定の既定値は無い。
`--skills-src`（既定 `config/skills`）の `cos-inbox-triage` の front matter version と違えば script が止まる。

B の account は `--b-account-dir`（`.credentials.json` を直に持つ 1 つの測定専用 dir）で渡す。script が `$S/pool/` を作り、b の basename の symlink を 1 つだけ置いて
`[accounts] claude_dir` をその pool にする（Celeris は claude_dir の各 subdir を 1 account として読む。b そのものを渡すと backups・cache 等が account と見なされる）。
a と operator の dir は渡せない。旧引数 `--claude-accounts` は使えない（usage で止まる）。

```sh
sh scripts/campaign/ops-r1-measure-config.sh --state "$S" --release-dir "$R" --models <cos-opus|all-sonnet> \
  --policy-version 10 --b-account-dir /local/celeris/measure-accounts/b --claude-bin "$CLAUDE" \
  --clusters-from ~/.config/celeris/config.toml --out "$S/daemon.toml"
grep -c 'principal = "human:' "$S/daemon.toml"            # 0
grep -c 'principal = "operator:ops-r1"' "$S/daemon.toml"  # 1
grep -A1 '^\[cos.triage\]' "$S/daemon.toml"               # policy_version = "10"
ls -A "$S/pool"                                           # b だけ（1 件）
grep -A4 '^\[maintenance.stagnation\]' "$S/daemon.toml"   # 4 閾値がすべて 0
```

期待: token は `operator:ops-r1` だけ。`--state` は試行ごとに新しい dir（他試行の `ws/` を見せない）。`--out` は bwrap の中の daemon が読むので `$S` の下に置く。
生成した config は langmem・gc を無効、`[maintenance.stagnation]` の閾値を 0、`[memory]`・`[llm_proxy]`・`[[mcp.listeners]]`・`[[cron.seed]]` なしにする。
試行と無関係に LLM を呼ぶ常駐の経路はこれで切る。CoS の受信箱 triage だけは config で切れない（§7-7）。

## 3. 隔離 preflight（deny_readable が 0 件）

deny list は 1 行 1 絶対 path の file で、承認票の deny list を実在の path に展開して書く。件数に上限は無い（9000 件を超える全件でよい。
bwrap の引数に並べず、file のまま包みの中へ渡す）。最低限、次を入れる。

- `$EVAL_ROOT/private/final/$case/private.json`
- 人の repo clone と Celeris の作業場所の clone の `crates/campaign/fixtures/grader-private/final/$case/private.json`（漏洩済みの旧 private。旧解答として塞ぐ）
  （`/home/rmaeda/workspace/agent-platform/...` と `/local/celeris/data/workspaces/*/repos/agent-platform/...` を展開した path）
- 同じ clone の `crates/campaign/fixtures/grade-cases/final/$case/` の中の file（`$case.json`・`hidden/.grader/` の試験。旧 private から作った漏洩済みの grade-case）
- 過去の campaign の out と試行の artifacts の中の file（K4 の `artifacts/ops-r1/` 等）
- 本番 KB の root の中の file、`~/.claude` と `~/.config/celeris` の中の file、測定用以外の account dir の中の file

dir ではなく中の file を書く（親を一覧できるだけで hit になる）。deny list は絶対 path でない行・NUL を含む行があると入力の誤りで止まる。
`sha256sum "$P/deny.list"` を承認票の deny list の欄と照合し、steps に残す。

K2 ops-r2 の manifest は `crates/campaign/fixtures/frozen-manifests/ops-r2-k2.json`（契約 revision `main-4-ops-r2`）。
`preflight` だけがこれを受ける（`run`・`dry-run` 等の投入系は拒否する）。api-url・token-env・task-template・materials-src・kb-src・canaries・cap-env は要らない。
`--repo-src`（K2 の初期 repo）・`--inputs-src`（外部入力）・`--measure-config`・`--normal-config` は実物を渡す。

```sh
"$CC" preflight --manifest crates/campaign/fixtures/frozen-manifests/ops-r2-k2.json \
  --repo-src <K2 の初期 repo> --inputs-src <外部入力の dir> \
  --measure-config "$S/daemon.toml" --normal-config ~/.config/celeris/config.toml \
  --a-program "$CLAUDE" --claude "$CLAUDE" --claude-sha256 "$CLAUDE_SHA" \
  --deny-list "$P/deny.list" --eval-root "$EVAL_ROOT" \
  --sandbox-script scripts/campaign/ops-r2-sandbox.sh --probe-program "$CC" \
  --b-state "$S" --b-release "$R" --b-account /local/celeris/measure-accounts/b --b-ro "$(dirname "$CLAUDE")" \
  --operator-state "$O" --operator-release "$R" --operator-account <operator 用 account dir> --operator-ro "$(dirname "$CLAUDE")"
echo "exit=$?"
```

期待: `exit=0` で、`missing` が 0 件（特に `deny_readable:` で始まる行が 0 件）。
`--claude` と `--claude-sha256` は組で、片方だけだと `deny_readable:claude:...` で止まる。copy の sha256 が違っても止まる。
`--b-ro`・`--operator-ro` は実際に包みへ渡す `--ro` と同じにする（preflight は同じ許可表で probe する）。
B の account は pool の symlink ではなく b の実 path（`ops-r2-sandbox.sh --account` に渡す値）を渡す。
`scripts/campaign/measure-launcher.sh preflight` は `--claude`・`--claude-sha256` を渡す欄が無いので、ops-r2 の preflight には使わず、上の command を直接実行する。
出た行の読み方は [入力隔離 §2.3](ops-r2-isolation.md#23-celeris-campaign-preflight---deny-listdeny_readable-で止める)。
`deny_readable:L1:...:ancestor_owned_by_run_uid` は `EVAL_ROOT` を人の uid が持つ dir の下に置いたとき、`owned_by_run_uid` は人の準備が済んでいないときに出る。
`graders` の missing は、`case.json` の `private_ref.sha256`・`grade_case_ref.sha256` が manifest の `grader_refs`（承認票の値）と違うときに出る（§0 の 2 行）。

正式試行（`--measure-config` 指定）では、この検査を外す env 変数・flag は無い。1 件でも残れば preflight は有料実行・task 作成を 0 件で止める。
この command を `--repo-src` 等の実物で通した実績はまだ無い（未確認）。

## 4. 漏洩検査（runtime の probe と L1 の probe を渡す）

B の包み方の中から probe を取り、L1 は人の uid から包まずに取る。

B の probe には preflight が渡すのと同じ `--allow` を付ける。`--allow` は host の mount の source（device 名）の許可表で、包みに bind する path
（`/usr` `/bin` `/sbin` `/lib` `/lib64` `/etc` `/dev`・`$R`・`$S`・B の account dir・`--ro` の path）が載っている mount の source と、`tmpfs`・`proc`・`devpts`・`mqueue`。
`--allow` が無いと包みの中の mount がすべて `mounts_outside_allow` に出る（2026-10-31 の実機では 6 件）。

```sh
ALLOW="tmpfs proc devpts mqueue"
for d in /usr /bin /sbin /lib /lib64 /etc /dev "$R" "$S" /local/celeris/measure-accounts/b "$(dirname "$CLAUDE")" "$CC" "$P/deny.list"; do
  ALLOW="$ALLOW $(findmnt -n -o SOURCE --target "$d" | sed 's/\[.*//')"
done
set --; for a in $ALLOW; do set -- "$@" --allow "$a"; done

sh scripts/campaign/ops-r2-sandbox.sh --state "$S" --release "$R" --account /local/celeris/measure-accounts/b \
  --ro "$CC" --ro "$P/deny.list" --ro "$(dirname "$CLAUDE")" -- \
  "$CC" probe-read --principal B --deny-readable-file "$P/deny.list" "$@" --out "$S/probe-B.json"   # exit 0

printf '%s\n' "$EVAL_ROOT/private/final/$case/private.json" > "$P/l1-deny.list"
"$CC" probe-read --principal l1 --deny-readable-file "$P/l1-deny.list" --out "$P/probe-l1.json"   # exit 0

python3 -I scripts/campaign/check-ops-r1-leak.py --private "$EVAL_ROOT/private/final/$case/private.json" \
  --scan <試行の出力の写し> --deny-readable-file "$P/deny.list" --eval-root "$EVAL_ROOT" \
  --runtime-probe "$S/probe-B.json" --runtime-probe "$P/probe-l1.json" --out <outbox>/leak-pre.json; echo "exit=$?"
```

`findmnt` の source の取り方は preflight（`allow_sources`）と同じ考え方の手書きで、実機で通した記録は無い（未確認）。許可表の食い違いで B の probe が
`mounts_outside_allow` を出したら、先に §3 の preflight の B の結果（同じ許可表で判定する）と見比べる。

期待: probe 2 本が exit 0、漏洩検査が `exit=0`・`pass: true`。

- **L1 の `mounts_outside_allow` は hit にしない。** L1 の probe は包まずに走るので host の mount が並ぶ（2026-10-31 の実機では 19 件）。
  漏洩検査はその件数を `l1_mounts_outside_allow` に残すだけで `deny_readable` の hit にしない。B の probe（包みの中）の `mounts_outside_allow` と `ok=false` は hit にする。
- **same_uid に canary は要らない。** `--eval-root` を渡すと、次の 4 条件がすべて揃う path は `same_uid_limit: false`・`same_uid_cleared_by: runtime_probe_and_l1` になる。
  (1) B の probe の全部がその path を持ち `readable`・`listable` とも false、(2) B の probe の全部が `ok=true` で `mounts_outside_allow` が空、
  (3) L1 の probe が渡され、その全 path が `readable`・`listable` とも false、(4) path が `EVAL_ROOT` の下なら L1 の probe にその path があり `owned_by_run_uid` が空。
  1 つでも欠ければ same_uid の limit が残り、canary が無いので `same_uid_without_canary` の hit で exit 1。probe に載っていない path、L1 の probe なしも通らない。
  `--eval-root` を渡さないと ops-r1 の規則（canary 必須）のまま。K2 の新しい private に canary を足すと sha256 が承認票と合わなくなるため、canary は置かない。
  bwrap の外の人の uid の process（普段の operator・人の shell）が読める点はこの規則で外さない（承認票と §5 で扱う）。
- `--deny-readable-file` は 1 行 1 絶対 path の file で、件数に上限は無い（旧 `--deny-readable <path>` は入力の誤りで止まる）。結果に file の sha256 と件数が出る。

漏洩検査は `--private` に `EVAL_ROOT` の private を読むので、評価器の uid で走らせる（[入力隔離 §4.4](ops-r2-isolation.md#44-評価器の起動方法どちらかを選ぶ) の案 2 の shell。
案 1 では採点 wrapper `grade` が判定 JSON について行う）。人の uid から private を開けないのが正しい状態で、人の uid で走らせると入力の誤りで止まる。
採点の判定 JSON は wrapper が drop box の tgz を sha256 で照合・展開して採点し、漏洩検査を通してから outbox に置く。
試行の出力そのものを `--scan` に渡す上の検査は wrapper に含めず、評価器の shell（`sudo -u celeris-eval /bin/bash`）で手で行う（残す）。
`check-ops-r1-leak.py` の probe の欄（`errno` は整数）は `celeris.runtime-probe/1` のまま。

bwrap の中から評価器の sudo が効かないことも確かめる（[入力隔離 §4.4](ops-r2-isolation.md#44-評価器の起動方法どちらかを選ぶ)。2026-10-31 の実機で exit 1、`no new privileges`）。

```sh
sh scripts/campaign/ops-r2-sandbox.sh --state "$S" --release "$R" --account /local/celeris/measure-accounts/b -- \
  /usr/bin/sudo -n -u celeris-eval /var/lib/celeris-eval-bin/grade; echo "exit=$?"   # 0 以外
```

### 4.1 sirius への接続（B の包みの中から）

K2 の B は sirius で計測する。測定 daemon は bwrap の中で動き、HOME は `$S/home`、許可表に `~/.ssh` も ssh の control master の socket も無い。
bwrap の中から sirius に届くかは未確認。試行の前に operator が確かめ、結果を steps に記録する。

```sh
sh scripts/campaign/ops-r2-sandbox.sh --state "$S" --release "$R" --account /local/celeris/measure-accounts/b -- \
  /usr/bin/ssh -o BatchMode=yes -o ConnectTimeout=10 <sirius の host 名> 'echo ok; command -v qsub qstat'; echo "exit=$?"
```

期待: `exit=0`、`ok` と qsub・qstat の path が出る。exit が 0 でない、または qsub・qstat が出なければ、**試行を始めず、測定 daemon も起動せず、exit code と stderr の 1 行目を添えて人に返す**。
operator は許可表に ssh の鍵・config・socket を足さない（credential を試行の主体に見せるかは人が決める）。`[[clusters]]` の ssh 設定（`--clusters-from` で写した値）が
`$S/home` の下の config を要るなら、その形で届くかも同じ command で見える。

## 5. operator principal と出典

- `--source` の file（`approved_request`）に、承認の `decision_id`・承認の時刻 `said_at`・承認票の依頼文 sha256・`decided_by`・`approved_by` を書く。
  `sha256sum <固定依頼文>` が `request_sha256` と一致すること。
- `ops-r1-intake --relay` は operator の token（`$S/token`）で呼ぶ。人の token を環境変数に置かない。
- 試行区間に、人の普段の session が測定 daemon を操作しない（照合で見つかればその試行は正式にしない。ADR D5）。
- 停止・回復を operator が自分で決めたときは、human-op の `decided_by=external_operator` とし、`note` に委任元の出典を書く。

確かめ方: relay の後で `$P/intake.json` の `executed_by`・`relayed_by` が `operator:ops-r1`、`decided_by`・`approved_by` が `human:<name>`。
記録が書かれない（exit 1）なら、理由は [CoS 受付の記録](ops-r1-intake.md) の「拒否する場合」を見る。

## 6. 測定 daemon の起動と、実験専用 operator session・session-map の準備

測定 daemon は B の包み（`ops-r2-sandbox.sh`）の中で起動する。§3 の preflight と §4 の漏洩検査が通った後、§4.1 が通った後にだけ行う。

```sh
sh scripts/campaign/ops-r2-sandbox.sh --state "$S" --release "$R" --account /local/celeris/measure-accounts/b \
  --ro "$(dirname "$CLAUDE")" -- "$R/bin/celeris" --config "$S/daemon.toml" --release <sha12> &
curl -s http://127.0.0.1:<測定 daemon の port>/api/v1/health   # cos_policy_version が "10"、release が sha12
```

`cos_policy_version` が承認票の方針版と違えば §7-6 で止める。この起動の形（包みの中で release の `bin/celeris` が `--config` を読む）を実機で通した記録は無い（未確認）。


```sh
sh scripts/campaign/ops-r2-session-prep.sh --evidence "$P/operator-session" --trials "$P/trials.json" \
  --claude-version <版> --model <model> --account-dir <operator 用 account dir> --policy-version <方針版> \
  --claude-bin "$CLAUDE" --campaign-bin "$CC" --celerisctl <固定した celerisctl> \
  --token-file "$S/token" --api-url http://127.0.0.1:<測定 daemon の port>
(cd "$P/operator-session" && sha256sum -c SHA256SUMS)   # すべて OK
```

期待: `ticket.json`（session id・版・model・account・方針版・sha256・`transcript_expected`）、`session-map.base.json`、`session-trials.json`、
`launch.sh`、`claude-config/settings.json`（`cleanupPeriodDays: 3650`、`Agent`・`Task` の拒否）がある。
`trials.json` の各試行に `trial_key`・`condition_id`・`request_ref` と prep・observe の区間がある。
承認票の実験専用 session の id が `ticket.json` の session id と一致する。

試行中は `bin/ops-r2-poll.sh` が状態を見て記録し、operator の LLM session は poll が 0 以外で返ったときだけ判断する。
operator の介入は 1 件ずつ開始と終了のある `human-op record --actor-kind external_operator` にし、人の能動時間は
`bin/ops-r2-human-span.sh begin|end` で人が記録した区間だけを数える（[実験専用 operator session §2](ops-r2-operator-session.md#2-試行の間-poll-と記録)）。
初回の起動で `transcript_expected` と実物の path を照合する（未確認）。

## 7. 停止規則の確認

[pilot runbook §3](mlsys-main-4-pilot-runbook.md#3-停止条件) の 9 項目に、ops-r2 では次を足す。どれかに当たったら、その試行の後に進まず、
`sh scripts/campaign/measure-launcher.sh stop --env <env>` で測定 daemon を止め、事実を進捗に書いて人に返す。

1. 隔離 preflight に `deny_readable:` が 1 件でも出た（§3）、または漏洩検査が exit 0 以外（§4）。
2. 試行区間の operator principal の変更操作に、human-ops の op 行（実験専用 session の id）と対応しないものがあった（ADR D5）。
3. 人の普段の session が試行区間に測定 daemon を操作した。
4. `ops-r2-session-map.py final` が exit 2（終了の無い介入、他試行にかかる介入、trial_key・condition_id の不一致）。
5. `ops-r2-poll.sh` が exit 2（取得か記録の失敗）で、記録を後から足せない。
6. 測定 daemon の `cos_policy_version`（`/api/v1/health`）が承認票の方針版（K2 ops-r2 は 10）と違う。config を作り直すのではなく、止めて人に返す。
7. 試行と無関係な起点の CoS run（停滞の finding・cron の定期見直し等）が測定 daemon に現れた。その試行は正式な試行にせず、消費を別に記録して人に返す。
   B の受信箱の triage（試行の task から出た待ちへの CoS の応答）は B の処理の一部として試行に計上する。
8. B の包みの中から sirius に届かない（§4.1）、または local の claude copy の版・sha256 が承認票と違う（§2.1）。

K2 は cluster job を使う。試行後も job が終端していなければ、pilot runbook §3 の 8 どおり人が `cluster-cancel.jsonl` の手順で消す（自動では消さない）。

## 8. 記録を残す

`$P` に `deny.list`（sha256 付き）・`l1-deny.list`・preflight の出力・`probe-*.json`・`leak-pre.json`・`intake.json`・`operator-session/` を残し、sha256 を進捗に書く。
取れなかった値（消費・人の時間）は null と理由（`not_recorded`・`transcript_missing`・`usage_missing:<件数>`）で残し、0 で埋めない。
