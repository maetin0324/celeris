---
tasks: [01M3WW2RBB9QW9NPN862TZEK9P]
---
# browser launcher の host 準備手順（root が行う）

[ADR-0115](../../agent-docs/adr/0115-browser-ptrace-owner-ns-launcher.md)「ホスト側の準備」と [ADR-0116](../../agent-docs/adr/0116-browser-launcher-implementation.md) の launcher を host に置く手順。**すべて root（初期 user namespace）で人が行う。** エージェントと daemon は root 操作をしない。

この手順を終えても機密能力（`CredentialInjection` / `IdentityRestore`）は解放しない。daemon の既定は `runtime = "daemon"` のまま変わらない。

## 記号

| 記号 | 意味 | この host での値 |
|---|---|---|
| `D` | daemon の user（host UID/GID 1001） | `rmaeda`（1001:1001） |
| `B` | 専用 user/group `celeris-browser` の UID/GID | 手順 1 で `id celeris-browser` を見て決まる |
| `S` | `celeris-browser` の subuid/subgid の先頭（数 65536） | 手順 2 で決める（例 `296608`） |

## 0. 前提

1. **初期 namespace の root であること**: `cat /proc/self/uid_map` が `0 0 4294967295`。
   container・入れ子 userns（例 `1001 1001 1` だけ、`newuidmap` の所有者が `nobody`）で作業しない。subuid の範囲が親 map に入らず `newuidmap` が EPERM になる。
2. `bwrap`、`google-chrome`（または設定で指す Chrome）、`agent-browser`、`celeris-browser-sandboxd` / `celeris-browser-egress` を root 所有の path（`/usr/bin`、`/usr/local/libexec/celeris` など）に置ける。`ProtectHome=true` のため `/home` 下の binary は使えない。

## 1. 専用 user/group を作る

```sh
groupadd --system celeris-browser
useradd --system --gid celeris-browser --no-create-home \
  --home-dir /var/lib/celeris-browser --shell /usr/sbin/nologin celeris-browser
passwd -l celeris-browser
```

確認（どれも期待どおりでなければ先へ進まない）:

```sh
id celeris-browser                        # groups= に celeris-browser だけ。1001 / rmaeda を含まない
id rmaeda                                 # groups= に celeris-browser を含まない
getent passwd celeris-browser | cut -d: -f7   # /usr/sbin/nologin
getent group celeris-browser              # 末尾（メンバー欄）が空
```

## 2. subuid / subgid の新規範囲

既存の割当てと重ならない 65536 個を `celeris-browser` に与える。**`rmaeda:165536:65536` を流用しない**（ADR-0115）。

```sh
cat /etc/subuid /etc/subgid
# 2026-10-02 時点の開発 host: rmaed:100000:65536 / rmaeda:165536:65536 / chatgpt-rdc:231072:65536
# → 空いている先頭は 296608（= 231072 + 65536）。host の実際の値から計算し直すこと。
S=296608
usermod --add-subuids ${S}-$((S+65535)) --add-subgids ${S}-$((S+65535)) celeris-browser
grep '^celeris-browser:' /etc/subuid /etc/subgid
```

重なりの確認（出力が `ok` であること）:

```sh
python3 - <<'PY'
for f in ("/etc/subuid", "/etc/subgid"):
    r = []
    for l in open(f):
        n, s, c = l.strip().split(":"); r.append((int(s), int(s) + int(c), n))
    r.sort()
    bad = [(a, b) for a, b in zip(r, r[1:]) if a[1] > b[0]]
    print(f, "ok" if not bad else bad)
PY
```

親 map の確認: 初期 namespace（手順 0）なら `0 0 4294967295` で `S..S+65535` と `B` を含む。入れ子の host では親の `uid_map` / `gid_map` が両方を含まなければこの host は使えない。

## 3. newuidmap / newgidmap の権限

```sh
ls -l /usr/bin/newuidmap /usr/bin/newgidmap   # -rwsr-xr-x root root（setuid root）か
getcap /usr/bin/newuidmap /usr/bin/newgidmap  # または cap_setuid / cap_setgid の file capability
stat -c '%U:%G %a' /usr/bin /usr/bin/newuidmap /usr/bin/newgidmap   # root:root、group/other に書込みなし
```

`celeris-browser` として 2 map が張れることを先に確かめる（launcher と同じ形 `0→B, 1000→S`）:

```sh
B=$(id -u celeris-browser)
runuser -u celeris-browser -- unshare --user sh -c 'sleep 5' & sleep 1
P=$(pgrep -n -u celeris-browser sleep)
runuser -u celeris-browser -- newuidmap $P 0 $B 1 1000 $S 1 && cat /proc/$P/uid_map
echo deny > /proc/$P/setgroups 2>/dev/null; runuser -u celeris-browser -- newgidmap $P 0 $B 1 1000 $S 1 && cat /proc/$P/gid_map
```

期待: `uid_map` が `0 B 1` と `1000 S 1` の 2 行（`1001` は現れない）。どちらかが EPERM なら手順 2・3 を見直す。

## 4. launcher の配置（root 所有）

release の `bin/` から launcher を root 所有でコピーする。daemon の `~/.local/celeris/releases/` を直接指さない（daemon が書き換えられるため）。

```sh
install -d -o root -g root -m 0755 /usr/local/libexec/celeris /etc/celeris-browser
install -o root -g root -m 0755 <release>/bin/celeris-browser-launcher     /usr/local/libexec/celeris/
install -o root -g root -m 0755 <release>/bin/celeris-browser-sandboxd     /usr/local/libexec/celeris/
install -o root -g root -m 0755 <release>/bin/celeris-browser-egress       /usr/local/libexec/celeris/
# agent-browser、bwrap、Chrome も root 所有の固定 path に配置し、その path を下の設定に記す。
```

固定設定 `/etc/celeris-browser/launcher.toml`（root:root 0644。daemon の設定からは変えられない、ADR-0116 D5）を作る。次は `BackendConfig` が要求する全項目で、実際に配置した `bwrap`・Chrome・agent-browser の絶対 path と resolver に置き換える。`session_root` は `state_dir` と別の私有 dir にする。IPC の上限は launcher に組み込まれた `LauncherLimits::default()` が適用され、TOML の項目ではない。

```toml
socket = "/run/celeris-browser/launcher.sock"
state_dir = "/var/lib/celeris-browser"
session_root = "/var/lib/celeris-browser/sessions"
allowed_uids = [1001]
bwrap = "/usr/bin/bwrap"
sandboxd = "/usr/local/libexec/celeris/celeris-browser-sandboxd"
egress = "/usr/local/libexec/celeris/celeris-browser-egress"
chrome = "/usr/bin/google-chrome"
agent_browser = "/usr/local/libexec/celeris/agent-browser"
resolver = "1.1.1.1"
# 本番 config ではこの欄を省略する（既定 off）。試験専用 launcher config のみ:
# test_loopback_allow = ["127.0.0.1:17730"]
```

`test_loopback_allow` は試験専用の任意欄で、省略時は off。明示する場合も `127.0.0.1:<port>` の完全一致のみを許し、port は 1〜65535（53/853 は不可）。launcher は試験許可を session policy に渡すとき task の `allowed_domains` との共通部分だけを使うため、設定した port 以外の private address や IP literal の拒否動作は変わらない。試験 harness が許可する loopback ページの port をここに列挙する。

**本番用 `/etc/celeris-browser/launcher.toml` にはこの欄を設定しない。** launcher は有効な設定を本番 config path、socket (`/run/celeris-browser/launcher.sock`)、state dir (`/var/lib/celeris-browser`) またはその配下・祖先で検出すると listen 前に起動を拒否する。socket activation の socket path も検査する。さらに本番 daemon は本番 config/DB/token の判定が fail-closed となり、launcher の `hello` が試験許可を申告した場合や hello に答えない旧 launcher では session を開始しない。試験用 daemon・DB・token・launcher socket は本番と完全に分離する。

試験用 config の例（試験専用の一時 path でのみ使用）:

```toml
test_loopback_allow = ["127.0.0.1:17730"]
```

試験許可が有効な launcher は stderr に `test-only loopback egress enabled: 127.0.0.1:17730 (not for production)` を出す。拒否記録は `<state_dir>/sessions/<session_id>/egress-denied.jsonl` にあり、1 行ごとに `kind`、`host`、`port`、UTC の `at`、`session_id` を記録する。要求本文、header、DNS 応答、上流エラーは記録しない。最大 128 件で、超過後は `truncated` を 1 件記録する。実機確認台本は当該 session 記録を `egress-denied.json` に集約する。

`resolver` は host で使用を許可する DNS resolver に置き換える。値が合っていることを確認してから unit を起動する。

```sh
chown root:root /etc/celeris-browser/launcher.toml && chmod 0644 /etc/celeris-browser/launcher.toml
install -d -o celeris-browser -g celeris-browser -m 0700 /var/lib/celeris-browser/sessions
```

`session_root` が無いと launcher は起動直後に exit 1 する（旧版の journal は `celeris-browser-launcher: No such file or directory (os error 2)` だけを出す）。`install -d` は `useradd` の後・unit 起動の前に必ず実行し、`ls -ld /var/lib/celeris-browser/sessions` が `drwx------ celeris-browser celeris-browser` であることを確かめる。2026-10-02 以降の launcher は、`session_root` が `state_dir` の直下で未作成なら自分で 0700 で作り、起動失敗の journal には対象の path を出す。

## 5. socket・状態 dir の所有と mode

| path | 所有 | mode | 作り手 |
|---|---|---|---|
| `/run/celeris-browser/` | root:root | 0755 | socket unit の `DirectoryMode` |
| `/run/celeris-browser/launcher.sock` | rmaeda:rmaeda | 0600 | socket unit（`SocketUser`/`SocketMode`）。daemon だけが connect できる |
| `/var/lib/celeris-browser/` | celeris-browser:celeris-browser | 0700 | service の `StateDirectory` |
| `/var/lib/celeris-browser/sessions/<id>/`（Chrome の `/session`） | `S`:`S` | 0700 | launcher が作る。launcher の記録とは別の dir |
| `/usr/local/libexec/celeris/`、`/etc/celeris-browser/` | root:root | 0755 / 0644 | 手順 4 |

socket・状態 dir・helper の path は Chrome の mount に bind しない（launcher が検査する、ADR-0116 D4）。

## 6. unit の配置と起動

リポジトリの `deploy/systemd/celeris-browser-launcher.socket` と `.service` を system unit として置く。daemon の user 名が `rmaeda` でない host では `.socket` の `SocketUser` / `SocketGroup` を書き換える。

```sh
install -o root -g root -m 0644 deploy/systemd/celeris-browser-launcher.socket  /etc/systemd/system/
install -o root -g root -m 0644 deploy/systemd/celeris-browser-launcher.service /etc/systemd/system/
systemd-analyze verify /etc/systemd/system/celeris-browser-launcher.{socket,service}
systemctl daemon-reload
systemctl enable --now celeris-browser-launcher.socket
systemctl status celeris-browser-launcher.socket
ls -l /run/celeris-browser/launcher.sock          # srw------- rmaeda rmaeda
```

service は最初の接続で socket activation により起動する。起動後の確認:

```sh
systemctl show celeris-browser-launcher.service -p User -p Group -p KillMode -p NoNewPrivileges
# User=celeris-browser / Group=celeris-browser / KillMode=control-group / NoNewPrivileges=no
ps -o user,group,pid,cmd -C celeris-browser-launcher
```

launcher protocol が `hello` を追加で必要とする daemon を導入するときは、daemon と同じ release の `celeris-browser-launcher` binary も `/usr/local/libexec/celeris/` に配置してから起動する。

`NoNewPrivileges` を付けないのは setuid の `newuidmap` を使うため（ADR-0115 ホスト側の準備 4）。Chrome 側の `no_new_privs` と capability 全落としは bwrap が付ける。

## 7. daemon UID 1001 が celeris-browser として実行できないことの確認

daemon の user（`rmaeda`）で実行し、**すべて失敗する**ことを確かめる:

```sh
sudo -n -u celeris-browser true; echo "sudo exit=$?"              # 非 0
su -s /bin/sh celeris-browser -c true </dev/null; echo "su exit=$?"   # 非 0（login 不可）
runuser -u celeris-browser -- true; echo "runuser exit=$?"         # 非 0（root 専用）
systemctl start celeris-browser-launcher.service; echo "start exit=$?"  # 非 0（polkit が拒否）
systemd-run --uid=celeris-browser true; echo "systemd-run exit=$?" # 非 0
test -w /usr/local/libexec/celeris/celeris-browser-launcher || echo "launcher not writable: ok"
test -w /etc/celeris-browser/launcher.toml || echo "config not writable: ok"
test -w /etc/systemd/system/celeris-browser-launcher.service || echo "unit not writable: ok"
test -r /var/lib/celeris-browser || echo "state dir not readable: ok"
```

root で補助の確認:

```sh
grep -rn 'celeris-browser' /etc/sudoers /etc/sudoers.d/ 2>/dev/null   # 何も出ない
getcap -r /usr/local/libexec/celeris 2>/dev/null                       # 何も出ない（host CAP_SYS_PTRACE を与えない）
```

## 8. 準備後に流す試験

daemon の user（UID 1001）で、**launcher と同じ host PID namespace を見られる通常シェル**からリポジトリの task branch の worktree で実行する（本番 DB には触れない試験）。実行前に `cat /proc/self/uid_map` と `readlink /proc/self/ns/pid` を記録する。LXC host では初期 user namespace の `0 0 4294967295` にならず、通常シェルの map が複数行になることがある。worker の db_guard 内の `1001 1001 1` や `1001 0 1` のような単独 map では host 側 Chrome の `/proc` が見えず、この実 process 証明はできない。

socket だけが active でも service の起動成功は保証されない。管理者は socket activation 後、次を確認する。`ActiveState=failed` や `activating (auto-restart)` の場合は試験の前に service の起動失敗を直す。

```sh
systemctl status celeris-browser-launcher.service --no-pager -l
systemctl show celeris-browser-launcher.service -p ActiveState -p Result -p ExecMainStatus -p NRestarts
sudo journalctl -u celeris-browser-launcher.service -b --no-pager -n 80
# journal が示す config の所有権、state_dir/session_root の所有権・0700、実行ファイルの配置、
# socket activation の失敗を修正し、ActiveState=active を確認する。
```

journal が `No such file or directory (os error 2)` なら、まず手順 4 の `session_root`（`/var/lib/celeris-browser/sessions`）と `launcher.toml` の各 path の有無を確かめる。

`runtime relay did not become ready` なら、更新した launcher と sandboxd の組を配置したうえで journal の `start <session>: start bwrap:` 行を確認する。launcher は bwrap の終了状態と、bwrap/sandboxd の制御済み診断行を最大 4 行記録する。Chrome と action の stderr は機密を含み得るため記録せず、sandboxd は起動失敗の errno と終了状態だけを報告する。egress は relay の READY 後に初めて起動するため、この段階の失敗には関与しない。

daemon の user で実行する:

```sh
CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture
```

`require` のため、`celeris-browser`・socket・subuid 範囲のどれかが欠けていれば skip でなく失敗する。確かめる内容（ADR-0116 D7）:

1. Chrome の user namespace owner（`NS_GET_OWNER_UID`、launcher の観測）が `S`（subuid の先頭）、その親 namespace の owner が `B` で、どちらも 1001 でない（ADR-0116 D3 付記）。`uid_map` / `gid_map` に 1001 が無い。1001 からは Chrome の `/proc/<pid>/ns/user` も開けない。
2. UID 1001 の別 process からの `PTRACE_ATTACH` / `strace -p` と `/proc/<pid>/environ`・`mem` の読取りが拒否される（正の対照も併記される）。
3. `verify_isolation` が `Ok`。

結果（コマンド・exit code・`uid_map`・owner）は `agent-docs/progress/phase-browser-4.md` に記録する。既存経路の確認として `cargo test -p task-worker --test browser_runtime_isolated` も通ること。

## 戻し方

```sh
systemctl disable --now celeris-browser-launcher.socket celeris-browser-launcher.service
rm /etc/systemd/system/celeris-browser-launcher.{socket,service} && systemctl daemon-reload
```

daemon の設定が `runtime = "daemon"`（既定）なら他に影響しない。`runtime = "launcher"` のまま launcher を止めると browser run は fail closed で失敗する（ADR-0116 D5）。

## launcher protocol 8 への更新（screenshot・download の受け渡し）

launcher protocol 8（ADR [credential username / post-login read](../../agent-docs/adr/2026-10-09-browser-credential-username-and-post-login-read.md)
付記 2026-10-10e）で、launcher runtime の screenshot・download の file が run の `browser/output/` に届き、agent が読める（PDF は
`download-<hex>.pdf` の名前でも置かれる）。**daemon の release と launcher の再 build・差し替えの両方**が要る。action runner
（`browser_action.py`）も launcher に埋め込まれているので、daemon だけの更新では足りない。

- daemon だけが新しい（launcher が protocol 7 以下）: browser session は動くが、screenshot・download は固定理由
  `browser_launcher_protocol_artifacts_required` で失敗する（run の progress `browser.artifact: …` と shim の応答に出る）。
- launcher だけが新しい: 旧 daemon は新しい要求を送らないので従来どおり（screenshot・download は旧来の失敗のまま）。

手順は [browser-credential-login-v5.md](browser-credential-login-v5.md) の「2. launcher の再 build と差し替え」と同じ（root、
稼働 session が無いことを確かめ、既存 binary を退避して hash を記録する）。退避名だけ変える。

```sh
W=<配送された agent-platform worktree（昇格した sha の checkout）>
SHA12=<昇格した sha12>
L=/usr/local/libexec/celeris/celeris-browser-launcher
cd "$W"
git rev-parse HEAD
CARGO_TARGET_DIR=/local/celeris/data/scratch/launcher-$SHA12-target cargo build --release -p task-worker --bin celeris-browser-launcher
sha256sum /local/celeris/data/scratch/launcher-$SHA12-target/release/celeris-browser-launcher "$L"
install -o root -g root -m 0755 "$L" "$L.pre-artifacts-v8"
pgrep -u celeris-browser -a
systemctl stop celeris-browser-launcher.socket celeris-browser-launcher.service
install -o root -g root -m 0755 /local/celeris/data/scratch/launcher-$SHA12-target/release/celeris-browser-launcher "$L"
sha256sum "$L"
systemctl start celeris-browser-launcher.socket
systemctl status celeris-browser-launcher.socket --no-pager
rm -rf /local/celeris/data/scratch/launcher-$SHA12-target
```

確認（daemon の実行 user）:

1. `celerisctl browser doctor` の `launcher` が OK で、`protocol 8` と出る。
2. read_origins 内の PDF を download する browser task を 1 件流し、run の `browser/output/` に `download-<hex>.bin` と同じ中身の
   `download-<hex>.pdf` があること、task の artifacts に `download-<hex>.bin` が載ることを確かめる。
3. run の events・progress に file の中身や URL の query・cookie が出ていないことを確かめる（出るのは `browser.download: success` と
   生成名だけ）。

戻し方: `install -o root -g root -m 0755 "$L.pre-artifacts-v8" "$L"` の後に socket を再起動する。daemon は protocol 7 を見て
screenshot・download を理由付きで失敗させる（他の操作・credential login は動く）。Live View の frame は protocol 9 の予定。
