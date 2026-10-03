---
tasks: [01M3ZFJ2DZ5TZPAFKACX45JNF4]
---
# launcher の実 session で本番 admission の表を取る手順（人が行う）

Attested task（01M3WV4BFJ71J9ZWJ020MP2Z4K）の証跡 `ADMISSION[real-session]`（許可/拒否の対応表、[ADR-0138](../../agent-docs/adr/0138-browser-prod-admission-confidential-release.md) D-L） を host で取り直す手順。daemon 側の launcher の身元確認は、`SO_PEERCRED`（socket 起動では systemd の uid 0 になる）から応答の `SCM_CREDENTIALS` に替えた（[ADR-0116 付記 D-P](../../agent-docs/adr/0116-browser-launcher-implementation.md)）。

**host の launcher は全 task の試験が共有している。** 入れ替えは証跡を取る間だけにし、終わったら必ず手順 5 で main の版に戻す。入れ替えている間、main の版を前提にした他 task の `browser_launcher_ptrace` 試験は `start real launcher session: Protocol` で落ちる。

前提: [browser-launcher-host-setup.md](browser-launcher-host-setup.md) の準備（user `celeris-browser`、subuid、unit、`/etc/celeris-browser/launcher.toml`）が済んでいること。

## 記号

| 記号 | 意味 |
|---|---|
| `W` | この修正を含む task branch の worktree（例 `/var/lib/celeris/workspaces/01M3ZFJ2DZ5TZPAFKACX45JNF4/repos/agent-platform`） |
| `L` | 配置先 `/usr/local/libexec/celeris/celeris-browser-launcher` |
| `T` | ビルドの target dir（`W` で `cargo metadata --format-version 1 --no-deps` の `target_directory`） |

## 1. 証跡用の launcher を作る（daemon の user、root 不要）

launcher の binary と protocol（v3）は Attested task branch（47f350eb）から変えていない。修正は daemon 側の client だけなので、試験は `W` の版で動かす。launcher も同じ tree から作る。

```sh
cd "$W"
git rev-parse --short=12 HEAD                    # 記録する
cargo build --release -p task-worker --bin celeris-browser-launcher
T=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
sha256sum "$T/release/celeris-browser-launcher"  # 記録する
```

## 2. 今の launcher を退避する（root）

```sh
L=/usr/local/libexec/celeris/celeris-browser-launcher
sha256sum "$L"                                   # 戻したあとに同じ値になることを確かめる
install -o root -g root -m 0755 "$L" "$L.pre-evidence"
```

## 3. 入れ替える（root）

socket と service を両方止めてから置く（止めている間の接続で古い binary が起動しないように）。

```sh
pgrep -u celeris-browser -a                      # 動いている session が無いこと（あれば終わるのを待つ）
systemctl stop celeris-browser-launcher.socket celeris-browser-launcher.service
install -o root -g root -m 0755 "$T/release/celeris-browser-launcher" "$L"
sha256sum "$L"                                   # 手順 1 の値と同じ
systemctl start celeris-browser-launcher.socket
systemctl status celeris-browser-launcher.socket --no-pager
```

## 4. 台本を走らせる（daemon の user、通常の host shell）

launcher と同じ pid namespace・`celeris-browser` の UID が写る user namespace の shell で動かす（`cat /proc/self/uid_map` を記録する。worker の db_guard 内のような単独 map では UID が 65534 に見えて前提が成り立たない）。

```sh
cd "$W"
sh crates/task-worker/scripts/launcher-admission-evidence.sh /tmp/launcher-admission-evidence.log
echo "exit=$?"
systemctl show celeris-browser-launcher.service -p MainPID   # 下の responder の pid と比べる
```

期待（`/tmp/launcher-admission-evidence.log`）:

- `started: ... responder=Some(SenderCred { pid: <MainPID>, uid: <celeris-browser の UID>, gid: ... })`。pid は `MainPID` と一致し、uid は `id -u celeris-browser`。
- `ADMISSION[real-session]` の行があり、launcher-proof の行が allow、他が deny。
- `PTRACE_ATTACH ... errno=Some(1)`、`strace ... Operation not permitted`。
- `test result: ok. 6 passed`、最終行 `EXIT: 0`。

`responder=None` や uid が違うときは表を作らずに失敗する（fail closed）。手順 5 で戻してから原因を調べる。

log は Attested task の成果物ディレクトリへ写し、取得日時・`W` の HEAD・binary の sha256 と一緒に残す。TOTP 等の秘密は書かない。

## 5. main の版に戻す（root、必ず行う）

```sh
pgrep -u celeris-browser -a                      # 試験の session が残っていないこと
systemctl stop celeris-browser-launcher.socket celeris-browser-launcher.service
install -o root -g root -m 0755 "$L.pre-evidence" "$L"
sha256sum "$L"                                   # 手順 2 で記録した値と同じ
rm "$L.pre-evidence"
systemctl start celeris-browser-launcher.socket
systemctl status celeris-browser-launcher.socket --no-pager
```

退避が無い・壊れているときは、main の worktree で `cargo build --release -p task-worker --bin celeris-browser-launcher` を作り、同じ `install` で置く。

戻ったことの確認（daemon の user、main の worktree）:

```sh
cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture
```

`start real launcher session: Protocol` が出ないこと（main の client と host の launcher の版が合っている）。

## 6. 後で

Attested task の branch が main に入り、main の launcher が protocol v3 になったら、host の launcher を main の版として入れ替える（そのときは手順 5 の退避・戻しは不要）。
