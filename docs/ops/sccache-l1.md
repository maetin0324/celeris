# sccache / cache server は廃止（ADR-0129）

sccache の差し込み（L1、`celeris-sccache.service`）と Celeris 自前の階層 cache server（L2、`celeris-scratch-cache.service`）は
Celeris から撤去した。経緯と設計は [ADR-0129](../adr/0129-host-sccache-reflink-targets.md) を参照。compiler wrapper を使うかどうかは
host 管理者が `~/.cargo/config.toml` の `[build] rustc-wrapper` で決める（ADR-0129 §2）。Celeris は `RUSTC_WRAPPER` /
`RUSTC_WORKSPACE_WRAPPER` / `SCCACHE_*` を差し込みも除去もせず、host から継いだ値をそのまま run の子プロセスへ渡す。

`/local` の btrfs、host の sccache、scratch の移行と戻し方は[切り替え手順](host-sccache-reflink-targets.md)を参照。

`scratch status` / `GET /api/v1/metrics/scratch` の `sccache` / `cache` 欄は型を残すが常に `null`（旧 client を壊さないため）。

## 人が行う後始末（本番 host、1 回だけ）

昇格済みの release がこの変更を含んだ後に行う。

1. unit を止めて無効化する:
   ```
   systemctl --user disable --now celeris-sccache.service celeris-scratch-cache.service
   ```
2. unit ファイルを消す（`~/.config/systemd/user/celeris-sccache.service`、
   `~/.config/systemd/user/celeris-scratch-cache.service`）、`systemctl --user daemon-reload`。
3. 旧 scratch の `sccache-l1/`・`cache-l1/`・token（`<scratch>/cache-server.token` など）を消す前に、参照元が無いことを確かめる:
   - `systemctl --user list-units` に `celeris-sccache` / `celeris-scratch-cache` が残っていない。
   - `~/.local/celeris/current/bin/celerisctl scratch status --json` の `sccache` / `cache` が `null`。
   - 直近の run の `runs/<run_id>/request.json` に `RUSTC_WRAPPER` / `SCCACHE_*` を Celeris が差し込んだ形跡が無い
     （host の `~/.cargo/config.toml` 由来の値は残ってよい）。
4. 確かめたら手で削除する（Celeris は自動削除しない）: `rm -rf <scratch>/sccache-l1 <scratch>/cache-l1 <scratch>/cache-server.token`。
   `[scratch]` 配下に残る旧 `l2` の NFS cache（`~/.local/celeris/cache/sccache-l2` など）も同様に確かめてから消す。

`config.toml` に残る `[scratch.sccache]` / `[scratch.cache_server]` / `[scratch.l2]` は起動を止めず、読み飛ばして一度警告する
だけでよい（新しい config には書かない）。
