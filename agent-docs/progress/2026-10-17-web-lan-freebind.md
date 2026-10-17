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
