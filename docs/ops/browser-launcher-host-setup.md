---
tasks: [01M3WW2RBB9QW9NPN862TZEK9P]
---
# browser launcher の host 準備手順（root が行う）

[ADR-0115](../adr/0115-browser-ptrace-owner-ns-launcher.md)「ホスト側の準備」と [ADR-0116](../adr/0116-browser-launcher-implementation.md) の launcher を host に置く手順。**すべて root（初期 user namespace）で人が行う。** エージェントと daemon は root 操作をしない。

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
```

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

daemon の user（UID 1001）で、**初期 user namespace の host 上**のリポジトリの task branch の worktree から実行する（本番 DB には触れない試験）。実行前に `cat /proc/self/uid_map` が `0 0 4294967295` であることを確認する。`1001 0 1` のような入れ子 namespace の test runner では host 側 Chrome の `/proc` が見えず、この実 process 証明はできない。

socket だけが active でも service の起動成功は保証されない。管理者は socket activation 後、次を確認する。`ActiveState=failed` や `activating (auto-restart)` の場合は試験の前に service の起動失敗を直す。

```sh
systemctl status celeris-browser-launcher.service --no-pager -l
systemctl show celeris-browser-launcher.service -p ActiveState -p Result -p ExecMainStatus -p NRestarts
sudo journalctl -u celeris-browser-launcher.service -b --no-pager -n 80
# journal が示す config の所有権、state_dir/session_root の所有権・0700、実行ファイルの配置、
# socket activation の失敗を修正し、ActiveState=active を確認する。
```

journal が `No such file or directory (os error 2)` なら、まず手順 4 の `session_root`（`/var/lib/celeris-browser/sessions`）と `launcher.toml` の各 path の有無を確かめる。

daemon の user で実行する:

```sh
CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture
```

`require` のため、`celeris-browser`・socket・subuid 範囲のどれかが欠けていれば skip でなく失敗する。確かめる内容（ADR-0116 D7）:

1. Chrome の user namespace owner（`NS_GET_OWNER_UID`、launcher の観測）が `S`（subuid の先頭）、その親 namespace の owner が `B` で、どちらも 1001 でない（ADR-0116 D3 付記）。`uid_map` / `gid_map` に 1001 が無い。1001 からは Chrome の `/proc/<pid>/ns/user` も開けない。
2. UID 1001 の別 process からの `PTRACE_ATTACH` / `strace -p` と `/proc/<pid>/environ`・`mem` の読取りが拒否される（正の対照も併記される）。
3. `verify_isolation` が `Ok`。

結果（コマンド・exit code・`uid_map`・owner）は `docs/progress/phase-browser-4.md` に記録する。既存経路の確認として `cargo test -p task-worker --test browser_runtime_isolated` も通ること。

## 戻し方

```sh
systemctl disable --now celeris-browser-launcher.socket celeris-browser-launcher.service
rm /etc/systemd/system/celeris-browser-launcher.{socket,service} && systemctl daemon-reload
```

daemon の設定が `runtime = "daemon"`（既定）なら他に影響しない。`runtime = "launcher"` のまま launcher を止めると browser run は fail closed で失敗する（ADR-0116 D5）。
