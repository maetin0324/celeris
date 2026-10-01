---
tasks: [01M3QGRC542ZC23996DNCTHZF5]
---
# 別 host UID（subuid）での browser isolated runtime の確認手順

ADR-0087 の同一 UID runtime を、subuid が使える host で別 host UID として実証する手順。人が実行する。
この手順が通るまで identity 復元などの機密能力は解放しない。

## 前提の確認

1. `grep "^$USER:" /etc/subuid /etc/subgid` に割当てがある（例 `165536:65536`）。
2. 親 user namespace の範囲内であること: `cat /proc/self/uid_map` が `0 0 4294967295`（初期 namespace）、または割当て範囲を含む。
   開発 host（2026-09-29）は `0 100000 1001 / 1001 1001 1 / 1002 101002 64534` で、`165536` が範囲外のため EPERM。
3. `unshare --user --map-auto --map-user=1 --map-group=1 id` が exit 0。
4. `newuidmap` / `newgidmap` が setuid または file capability 付き（`ls -l /usr/bin/newuidmap`）。

## 確認

1. このリポジトリで `cargo test -p task-worker --test browser_runtime_isolated -- --nocapture` が 4 passed であること（同一 UID の基線）。
2. runtime を別 UID で起こす: bwrap を `unshare --user --map-auto` で包むか、setuid でない bwrap に対し
   `newuidmap <pid> 1000 <subuid の先頭> 1` を controller が実行する形で起動し、`/proc/<child-pid>/status` の Uid が
   daemon の UID と異なる（subuid 範囲内）ことを確認する。
3. `RuntimeFacts.runtime_uid != host_uid` で `verify_isolation` が `Ok` になることを確かめる。`SameUid` 以外の違反が出たら不可。
4. 別 UID の browser process に daemon UID の別 process から `cat /proc/<pid>/environ` や `gdb -p` が拒否されることを確認する
   （同一 UID では通ってしまうのが本手順の目的）。
5. 結果（コマンド・exit code・uid_map）を `docs/progress/phase-browser-4.md` の P4-A 行に追記し、機密解放は別の ADR で判断する。
