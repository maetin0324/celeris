---
title: K2 ops-r2 の試行開始の残り（2）— 漏洩検査の公開 archive・submodule 込みの種・試行 A の起動形・B の案件 repo と起動
tasks: [01M69YXTFWZT56XCZFY2PCPZMB, 01M69YZVW2T5ASBYQYZKPN7Y85]
status: done
started: 2026-10-31
completed: 2026-10-31
updated: 2026-10-31
---

# K2 ops-r2 の試行開始の残り（2）

完了日: 2026-10-31

親 task 01M69YXTFWZT56XCZFY2PCPZMB の子 3 件はすべて done。実装と試験は終わったが、新しい手順で §2〜§6 を実機で通した記録はまだ無い。
operator が下の「本番への移行」で §2〜§6 をやり直すまで、K2 ops-r2 の正式試行は始めない。
方針は ADR [2026-10-31-ops-r2-preflight-fixes](../adr/2026-10-31-ops-r2-preflight-fixes.md) の付記
D-public・D-seed2・D-trial-a2・D-bwork・D-bssh・D-bstart に書いた。手順書は [ops-r2-k2-preflight.md](../../docs/ops/ops-r2-k2-preflight.md)。

## 子 task の結果

| 子 task | 親の項目 | 結果 | 詳細 |
|---|---|---|---|
| 01M69YZVW27JDDR1115KFCD6NH（leak-seed） | 1・2 | done | [leak-seed.md](2026-10-31-ops-r2-trial-start-2/leak-seed.md) |
| 01M69YZVW2AA2QPYS5KKAN20SR（trial-a-command） | 3 | done | [trial-a-command.md](2026-10-31-ops-r2-trial-start-2/trial-a-command.md) |
| 01M69YZVW2BBP82R6ZY45BMW3J（b-workspace） | 4・5・6 | done | [b-workspace.md](2026-10-31-ops-r2-trial-start-2/b-workspace.md) |

- 項目 1（漏洩検査）: `check-ops-r1-leak.py --public-archive <tar> <sha256>` が、公開の初期 tar と submodule の tar に元から現れる private の文字列を検査から外す。
  結果には外した数と長さだけを書く。private の sha256・禁止 hash の検査は外さない。
- 項目 2（種）: `ops-r2-seed.sh make --archive <公開 tar> --case <case.json> --assets <asset store> --out <tar>` が submodule 5 件の中身を入れた種を作る。
  実 asset で作った種の sha256 は 233d38cbbc390cd9374287b9eb618e715c4088e4e858f4eb179d1ed6e08b4d60（2 回作って一致）。展開した木は採点器の `--initial-tree` の組み方で作った木と差が無かった。
- 項目 3（試行 A）: `ops-r2-trial-a-command.sh` が K4 の A と同じ権限の形（`--permission-mode bypassPermissions --disallowedTools Task`）と必須の `--session-id` を持つ 1 行を出す。
  `@`・`:` を含む socket path を quote して受ける。
- 項目 4（B の案件 repo）: `ops-r1-measure-config.sh` が `setup/project.json` に workspace `<state>/work/benchfs` を書く。案件の primary repo は種類 `dir` になる。
- 項目 5（preflight の ssh config）: `celeris-campaign preflight` に `--b-ssh-config`・`--operator-ssh-config` を足した。
- 項目 6（B の起動）: `ops-r2-b-daemon-command.sh` が、種の展開の後に測定 daemon を exec する 1 行を出す（起動はしない）。

commit（件名。基準は `git merge-base HEAD main`、子 3 件の分は main に入っている）:
- ops-r2: 漏洩検査が公開の初期 archive の needle を外し、種に submodule の中身を入れる
- 進捗: leak-seed の bundle 名から hash 様の表記を外す
- 進捗: leak-seed の提出時の検査結果
- ops-r2: 試行 A の起動 command に K4 の A と同じ権限の形と session id を出し、@・: を含む path を quote して受ける
- ops-r2: B の案件 repo を種の作業ツリーにし、preflight に --ssh-config を足し、B の daemon の起動 command と試験を足す
- 進捗: b-workspace の提出時の検査結果
- 進捗: ops-r2 trial start 2 のまとめと本番への移行（この task）

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `bash scripts/dev/test-affected.sh` → exit 0。この task の変更は進捗ファイル 1 件だけで、出力は「試験対象の変更なし（docs のみ等）」と check-doc-links の self-test ok。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → ok。
- 子 3 件の試験の結果は各 WU の進捗にある（例: `ops_r2_b_start` 3 passed、`CELERIS_USERNS_TESTS=1` の実 bwrap 1 passed、`run-ops-r1-leak-tests.sh` 61 tests OK）。

## 未解決事項
- 新しい手順で §2〜§6 を実機で通していない（この task は本番・実機の操作をしない）。
- 本物の release の `bin/celeris` が B の包みの中で `--config` を読んで起動するかは未確認（試験の daemon は偽物）。
- B の CoS が `dir` の repo を選んで起票するか、`dir` の repo の task で delivery が何をするかは未確認。
- 専用 ssh config の `UserKnownHostsFile` は B の包みの中から読めない。master socket 経由なら host key を照合し直さないので通る見込みだが未確認（§4.1 で分かる）。
- 評価器の uid（celeris-eval）が `$P/k2-initial.tar` と asset store の submodule tar を読めるかは未確認。読めなければ §4 の漏洩検査が exit 2 で止まる。
- `--disallowedTools Task` だけで claude 2.1.303 の subagent（道具名 `Agent`）が止まるかは未確認（K4 と同じ値）。

## 本番への移行（operator が §2〜§6 をやり直す。人が実行する手順）

前提:
- この branch が main に入り、その main から作った release と `celeris-campaign` を使う（`--b-ssh-config` と新しい `make` は旧い binary・script に無い）。
  `CC` は main の clone で `cargo build --release -p celeris-campaign` した binary を `/local/celeris/data/measure-bin/` に写したもので、`sha256sum "$CC"` を steps に残す。
  script（`scripts/campaign/*.sh`・`*.py`）は同じ main の clone のものを使い、その clone を cwd にする。
- 手順書 §0 の人の準備と §1 が済み、承認票の欄が人の手で埋まっている。
- 本番 daemon・本番 DB・`~/.config/celeris`・`/local/celeris/state/releases`・sirius の設定は変えない。旧い state（`ops-r2-k2-p1`・`ops-r2-k2-p2`）と証跡は消さず、使わない。

変数（例。値は承認票と手順書の冒頭に合わせる）:

```sh
case=k2-bf-xattr-restart-campaign
EVAL_ROOT=/var/lib/celeris-eval
R=<固定した release dir>                                   # bin/celeris と manifest.json がある
CLAUDE=/local/celeris/data/measure/ops-r2-k2-p1/bin/claude-2.1.303
BASE=/local/celeris/data/measure/ops-r2-k2-p3              # 新しい dir（既存なら p4 等にする）
S=$BASE/b; S_A=$BASE/a; O=$BASE/operator; P=$BASE/evidence
mkdir -p "$S" "$S_A" "$O" "$P"                             # 4 つとも空の新しい dir
MUX=/run/user/1001/ssh-mux-rmaeda@130.158.241.2:22         # 人が開いた sirius の master socket
SSHCFG_DIR=/local/celeris/data/measure-bin/ssh
DL=/local/celeris/data/measure-bin/deny-$case.list
```

### 移行 1（§2.1・§2.2）: claude の copy と config、release store の写し

```sh
"$CLAUDE" --version                 # 2.1.303
CLAUDE_SHA=$(sha256sum "$CLAUDE" | cut -d' ' -f1); echo "$CLAUDE_SHA"   # 375988f457756524234695cf2d8f64ace99c5f68682e47ea24862a843c191450（承認票と同じ）
sh scripts/campaign/ops-r1-measure-config.sh --state "$S" --release-dir "$R" --models <承認票の値> \
  --policy-version 10 --b-account-dir /local/celeris/measure-accounts/b --claude-bin "$CLAUDE" \
  --clusters-from ~/.config/celeris/config.toml --out "$S/daemon.toml"; echo "exit=$?"   # exit=0
SHA12=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["sha12"])' "$R/manifest.json")
readlink "$S/release-store/current"                 # releases/$SHA12
readlink "$S/release-store/releases/$SHA12"         # $R
grep -A2 '^\[selfdeploy\]' "$S/daemon.toml"         # releases_dir = "$S/release-store/releases"
grep -A1 '^\[cos.triage\]' "$S/daemon.toml"         # policy_version = "10"
ls -A "$S/pool"                                     # b だけ
grep -c 'principal = "human:' "$S/daemon.toml"      # 0
python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["workspace"])' "$S/setup/project.json"
                                                    # {'kind': 'local', 'path': '$S/work/benchfs'}
ls /local/celeris/state/releases > "$P/prod-releases.after"   # 生成の前に取った一覧と diff して差が無い
```

`manifest.json` に `sha12` の欄が無ければ `basename "$(readlink -f "$R")"` の 12 桁を使う。
`$KB0` は手順書 §2.2 のとおり作り、`diff -r "$KB0" "$S/kb"` が exit 0 を確かめる。どれかが期待と違えば止め、出力を steps に残して人に返す。

### 移行 2（§6.1）: 種を作り直し、A・B の state に置く

```sh
CASE_JSON=crates/campaign/cases/final/$case/case.json
ASSETS=/local/celeris/data/campaign-assets/k2
INIT_COMMIT=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["initial"]["commit"])' "$CASE_JSON")
ARCHIVE_SHA=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["initial"]["archive_sha256"])' "$CASE_JSON")
git clone -q "$ASSETS"/benchfs-*.bundle "$P/benchfs-src"
git -C "$P/benchfs-src" archive --format=tar -o "$P/k2-initial.tar" "$INIT_COMMIT"
sha256sum "$P/k2-initial.tar"                       # $ARCHIVE_SHA と同じ
SEED_SHA=$(sh scripts/campaign/ops-r2-seed.sh make --archive "$P/k2-initial.tar" --case "$CASE_JSON" --assets "$ASSETS" --out "$P/benchfs.tar")
echo "$SEED_SHA"                                    # 233d38cbbc390cd9374287b9eb618e715c4088e4e858f4eb179d1ed6e08b4d60
for X in "$S_A" "$S"; do
  sh scripts/campaign/ops-r2-seed.sh place --seed "$P/benchfs.tar" --sha256 "$SEED_SHA" --state "$X"
  mkdir -p "$X/bin" && cp scripts/campaign/ops-r2-seed.sh "$X/bin/"
done
sha256sum "$S_A/seed/benchfs.tar" "$S/seed/benchfs.tar"   # 2 行とも $SEED_SHA
tar -tf "$P/benchfs.tar" | grep -c '^lib/pluvio/.'       # 1 以上（submodule の中身が入っている）
PUB=(--public-archive "$P/k2-initial.tar" "$ARCHIVE_SHA")
while read -r h f; do case $f in submodules/*.tar) PUB+=(--public-archive "$ASSETS/$f" "$h") ;; esac; done < "$ASSETS/SHA256SUMS"
echo "${#PUB[@]}"                                   # 18（初期 tar 1 件と submodule の tar 5 件、各 3 語）
```

`make` が exit 1 なら（公開 tar・submodule tar の sha256 不一致、path の重複）止めて人に返す。
`$SEED_SHA` が上の値と違えば、asset か case.json が 2026-10-31 と違う。止めて人に返す。
評価器の uid が読めることを確かめる: `sudo -u celeris-eval test -r "$P/k2-initial.tar" && sudo -u celeris-eval test -r "$ASSETS/SHA256SUMS"; echo "exit=$?"`（exit=0）。
読めなければ `$P` の dir の権限を人が直す（private の dir の権限は変えない）。

### 移行 3（§4.1）: B の包みの中から sirius へ

`$SSHCFG_DIR/config` は sirius だけを書き、`ControlPath` に `$MUX`、`UserKnownHostsFile` に `$SSHCFG_DIR` の下の file を書く。人の uid 持ちで group・other に書き込み権が無いこと（`stat -c '%U %a' "$SSHCFG_DIR/config"` → `rmaeda 600` か `644`）。

```sh
sh scripts/campaign/ops-r2-sandbox.sh --state "$S" --release "$R" --account /local/celeris/measure-accounts/b \
  --ro "$(dirname "$CLAUDE")" --ssh-config "$SSHCFG_DIR/config" --socket "$MUX" --print-args \
  | grep -n 'ssh_config\|ssh-mux\|measure-bin/ssh$'     # /etc/ssh/ssh_config の bind と socket の bind だけ。$SSHCFG_DIR の dir そのものは出ない
sh scripts/campaign/ops-r2-sandbox.sh --state "$S" --release "$R" --account /local/celeris/measure-accounts/b \
  --ssh-config "$SSHCFG_DIR/config" --socket "$MUX" -- \
  /usr/bin/ssh -o BatchMode=yes -o ConnectTimeout=10 sirius 'echo ok; command -v qsub qstat'; echo "exit=$?"
```

期待: `exit=0`、`ok` と qsub・qstat の path。`Host key verification failed` なら known_hosts が包みから読めない（未解決事項の 3 つ目）。
exit が 0 でなければ移行 4 以降に進まず、exit code と stderr の 1 行目を人に返す（鍵・config・socket を許可表に足すかは人が決める）。

### 移行 4（§3）: 隔離 preflight

```sh
sha256sum "$DL"; cp "$DL" "$P/deny.list"            # 承認票の deny list の sha256 と同じ
"$CC" preflight --manifest crates/campaign/fixtures/frozen-manifests/ops-r2-k2.json \
  --repo-src <K2 の初期 repo> --inputs-src <外部入力の dir> --kb-src "$KB0" \
  --measure-config "$S/daemon.toml" --normal-config ~/.config/celeris/config.toml \
  --a-program "$CLAUDE" --claude "$CLAUDE" --claude-sha256 "$CLAUDE_SHA" \
  --deny-list "$DL" --eval-root "$EVAL_ROOT" \
  --sandbox-script scripts/campaign/ops-r2-sandbox.sh --probe-program "$CC" \
  --b-state "$S" --b-release "$R" --b-account /local/celeris/measure-accounts/b --b-ro "$(dirname "$CLAUDE")" \
  --b-ssh-config "$SSHCFG_DIR/config" --b-socket "$MUX" \
  --operator-state "$O" --operator-release "$R" --operator-account <operator 用 account dir> --operator-ro "$(dirname "$CLAUDE")" \
  > "$P/preflight.out" 2>&1; echo "exit=$?"
grep -c '^deny_readable:' "$P/preflight.out"        # 0
```

期待: `exit=0`、`missing` 0 件。`--b-ro "$SSHCFG_DIR"` は渡さない（旧い形。B の daemon の包みには config の file だけを見せる）。
`--b-ro`・`--b-ssh-config`・`--b-socket` は移行 6 の daemon の起動と同じ値にする。exit が 0 でなければ `$P/preflight.out` を steps に残して止める。

### 移行 5（§4）: probe と漏洩検査

手順書 §4 の snippet で `ALLOW` を組み、B の probe（`--ssh-config "$SSHCFG_DIR/config" --socket "$MUX"` 付き）と L1 の probe を取る。

```sh
sh scripts/campaign/ops-r2-sandbox.sh --state "$S" --release "$R" --account /local/celeris/measure-accounts/b \
  --ro "$CC" --ro "$DL" --ro "$(dirname "$CLAUDE")" --ssh-config "$SSHCFG_DIR/config" --socket "$MUX" -- \
  "$CC" probe-read --principal B --deny-readable-file "$DL" "$@" --out "$S/probe-B.json"; echo "exit=$?"   # exit=0
printf '%s\n' "$EVAL_ROOT/private/final/$case/private.json" > "$P/l1-deny.list"
"$CC" probe-read --principal l1 --deny-readable-file "$P/l1-deny.list" --out "$P/probe-l1.json"; echo "exit=$?"   # exit=0
```

漏洩検査は評価器の uid の shell（`sudo -u celeris-eval /bin/bash`）で、上の `PUB` を同じ shell で組み直してから走らせる。

```sh
python3 -I scripts/campaign/check-ops-r1-leak.py --private "$EVAL_ROOT/private/final/$case/private.json" \
  --scan "$S_A/seed" --scan "$S/seed" "${PUB[@]}" --deny-readable-file "$DL" --eval-root "$EVAL_ROOT" \
  --run-uid "$(id -u rmaeda)" --run-gid "$(id -g rmaeda)" \
  --runtime-probe "$S/probe-B.json" --runtime-probe "$P/probe-l1.json" --out <outbox>/leak-pre.json; echo "exit=$?"
python3 -I -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d["pass"], d["checked"]["public_archive"])' <outbox>/leak-pre.json
```

期待: `exit=0`、`True`。`checked.public_archive` の `excluded_count`・`excluded_lengths` と `warning` を steps に残す。
`warning` が `no_private_strings_left` か `most_private_strings_removed` なら、続けるかを人が承認票で決める（operator は決めない）。
続けて手順書 §4 の最後の command で、包みの中から評価器の sudo が効かないこと（exit が 0 以外）を確かめる。

### 移行 6（§6）: 測定 daemon の起動と案件の repo

移行 1〜5 がすべて通った後にだけ行う。`$S/work/benchfs` が無いか空であること（`ls -A "$S/work/benchfs" 2>/dev/null | wc -l` → 0）。

```sh
sh scripts/campaign/ops-r2-b-daemon-command.sh --state "$S" --release "$R" --claude-dir "$(dirname "$CLAUDE")" \
  --account /local/celeris/measure-accounts/b --ssh-config "$SSHCFG_DIR/config" --socket "$MUX" --seed-sha256 "$SEED_SHA" \
  > "$P/b-daemon-command.sh"; echo "exit=$?"         # exit=0、1 行
# 人が別の端末で: sh "$P/b-daemon-command.sh"
curl -s http://127.0.0.1:<port>/api/v1/health      # cos_policy_version が "10"、release が $SHA12
ls "$S/work/benchfs/lib/pluvio" | head -1          # 空でない（種が展開された）
test ! -e "$S/work/benchfs/.git" && echo no-git    # no-git
```

health の後、手順書 §6 と [測定 daemon](../../docs/ops/ops-r1-measure-daemon.md) のとおり `-d @"$S/setup/project.json"` で `POST /api/v1/projects` し、案件を確かめる。

```sh
curl -s -H "Authorization: Bearer $(cat "$S/token")" http://127.0.0.1:<port>/api/v1/projects/<id>/repos
# 1 件、name が benchfs、kind が dir、primary、location.path が $S/work/benchfs
```

期待と違えば、promotion gate の exit 4（`current does not exist`）なら移行 1 の release store の写しを、展開の失敗なら移行 2 の種を見直す。
どちらでも同じ state で 2 回目の起動はしない（script が拒む）。新しい `BASE` で移行 1 からやり直す。
health の `cos_policy_version` が "10" でなければ手順書 §7-6 で止める。

### 移行の後
- 手順書 §5（operator principal と出典）と §6 の session prep を行い、§6.2 で A の command を出す。A は移行 2 と同じ `$SEED_SHA` と、`$P/a-session-id` に残した UUID を使う。
- 手順書 §8 のとおり `$P` の証跡と sha256 を新しい進捗ファイルに残す。手順書の冒頭の「新しい手順で §2〜§6 を通した記録はまだ無い」は、通した後にその進捗を指す文に改める。

## 提案
- B の probe の `--allow` を `ops-r2-sandbox.sh --print-args` と mountinfo から作る script を足す（今は手書きの snippet。docs WU の提案と同じ）。
- 試行 A の `--disallowedTools` を B の worker と同じ `Agent,Task,Workflow` に揃えるかを、K2 の承認票で人が決める。
