---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: complete
title: browser_e2e credential broker socket path flake
updated: 2026-10-12
---
# browser_e2e credential broker socket path flake

## 原因

`browser_e2e` は broker runtime を `TestEnv` の一時ディレクトリ配下に作っていた。今回の長い run `TMPDIR` では `control.sock` までのパスが 157 byte となり、Linux の `sockaddr_un.sun_path` 上限（108 byte）を超えた。`UnixListener::bind` が `ENAMETOOLONG` で失敗し、`ipc::serve` は `Error::Io` を返していた。serve の終了を readiness 待ちで検出する診断を残した。

## 修正と回帰試験

broker runtime を `/tmp` の短い一時ディレクトリへ移し、daemon の所有者検査に必要な mode `0700` を明示した。root/config/data fixture は既存の `TestEnv` 内に維持した。回帰試験 `credential_broker_runtime_sockets_fit_unix_path_limit_with_long_tmpdir` は control/resolve/injection の各 socket path が 108 byte 未満で、実際に bind されたことを確認する。

## 検証

- `TMPDIR=/local/celeris/data/workspaces/01M4MEEKX485VD5SWKX0P30D4Y/runs/01M4Q0CP7P500RPHJKKWWQKV1G/tmp` の下で計算した旧 control socket path は 157 byte（上限超過）。
- `cargo test -p task-api --test browser_e2e --no-run`: exit 0。
- `cargo test -p task-api --test browser_e2e`: exit 0、5 passed（4 flaky test + 回帰試験）。
- `/dev/shm` と `/run/user/1001` はこの環境で read-only だったため、mode `0700` を設定した `/tmp` の短い test runtime を使った。
