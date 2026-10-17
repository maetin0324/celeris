---
title: celeris-web-lan.socket に FreeBind=yes（再起動時の LAN 入口落ち）
tasks: [01M553KCPZQ5PBB1HVY2NVMCM3]
status: done
updated: 2026-10-17
---
# celeris-web-lan.socket に FreeBind=yes

2026-10-17 のホスト再起動で `celeris-web-lan.socket`（192.168.1.103:7721）が起動時に
`Cannot assign requested address` で failed になった。運用者は本番に drop-in
`~/.config/systemd/user/celeris-web-lan.socket.d/freebind.conf`（`[Socket] FreeBind=yes`）を置いて直した。
雛形 `deploy/systemd/celeris-web-lan.socket` に `FreeBind=yes` を入れ、
`scripts/selfdeploy/tests/install_units_hot_dir.sh` で雛形と設置結果を検査する。

## 本番への移行（人が実行）

雛形を入れ直すと FreeBind が二重になるので、入れ直しと同時に drop-in を消す。

```bash
bash ~/workspace/agent-platform/scripts/selfdeploy/install-units.sh --replace-existing
rm ~/.config/systemd/user/celeris-web-lan.socket.d/freebind.conf
rmdir ~/.config/systemd/user/celeris-web-lan.socket.d   # 空なら
systemctl --user daemon-reload
systemctl --user cat celeris-web-lan.socket | grep -c '^FreeBind=yes'   # 1 であること
systemctl --user restart celeris-web-lan.socket
```

## 検査（attempt 2）
- 前回の test-parallel 失敗は `a_arm_hardening real_bwrap_hides_everything_outside_the_workspace`。sandbox で `bwrap --proc /proc` が EPERM のため、`bwrap_usable()` の探査に `--proc /proc` を足して skip させた（FreeBind とは無関係）。
- `bash scripts/dev/test-parallel.sh` exit 0（5717 pass / 0 fail）、clippy exit 0、fmt exit 0、`bash scripts/selfdeploy/tests/install_units_hot_dir.sh` ok。

## 配送修復（2026-10-17）

prepare.log の cargo-test は 5689 pass / 28 fail（userns=true）。代表例は
`browser_runtime_isolated::probe_inside_runtime_cannot_reach_host_sockets_or_network` の
`InitNotReady` だった。`InitNotReady` に既存の安全な起動診断（bwrap の終了状態と
`bwrap:` / `sandboxd:` の許可済み行）を付け、sandbox 外で同試験を再実行すると、
`bwrap: Can't mount proc on /newroot/proc: Operation not permitted` と判明した。
`unshare -Ur true` は成功するため、userns の preflight だけでは検出できない。

これは FreeBind の設定とは独立した検証ホストの proc mount 制約であり、今回の変更で
リリース可能になったとは判断しない。隔離を弱めたり release gate の試験を skip したりせず、
運用者がホストの namespace / proc mount 制約を確認した後に、同じ release 設定で再検証する。
本 run では本番設定・サービス・リリースを変更していない。

検証結果: `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` は sandbox 外で exit 0
（5717 pass / 0 fail / 15 ignored、userns=null）。`cargo clippy --workspace -- -D warnings`、
`cargo fmt --all -- --check`、`bash scripts/selfdeploy/tests/install_units_hot_dir.sh` も exit 0。
sandbox 内での全体試験は fixture 起動等の環境制約で複数失敗し、sandbox 外での再検証へ切り替えた。
通常設定の合格は、userns 有効の release gate の合格を意味しない。
ログはこのタスクの artifacts に `test-parallel-host.log`、`clippy.log`、`fmt.log`、
`install-units.log`、`runtime-probe.log` として保存した。
