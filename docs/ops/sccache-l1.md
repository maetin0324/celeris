# sccache L1 の導入と有効化（ADR-0075 D4、Phase G2）

Celeris の run（Task 単位の run、WU の run と checks、統合の検査、reviewer の checks）は、sccache の server が応答するときだけ
`RUSTC_WRAPPER` などを受け取る。server が無ければ素の cargo で動く（scratch の `CARGO_TARGET_DIR` と
`CARGO_INCREMENTAL=0` / `CARGO_PROFILE_DEV_DEBUG=line-tables-only` は常に与える）。以下は**人が 1 回だけ**行う。

## 前提

- G2 を含む release が**昇格済み**（`~/.local/celeris/current/bin/celerisctl scratch env --server` が通る版）。
  `celeris-sccache.service` は `current` の `celerisctl` から env を取る。
- `[scratch.sccache]` / `[scratch.cargo]` は**書かなくてよい**（既定: 有効、port 4236、binary
  `~/.local/celeris/tools/sccache/bin/sccache`、incremental 無効、`line-tables-only`）。変えるときも、この節を知る release が
  昇格した**後**に `config.toml` に足す（旧い版は未知のキーで起動に失敗し、N-1 と rollback が壊れる。ADR-0075 D7）。

## 手順

1. バイナリを置く（どちらか）。
   - `scripts/scratch/setup-sccache.sh --from ~/.cargo/bin/sccache`（既に同じ版〈`tools/sccache/VERSION`〉が入っているとき。ネットワークに出ない）
   - `scripts/scratch/setup-sccache.sh`（`cargo install sccache --locked --version <VERSION> --root ~/.local/celeris/tools/sccache`）
2. unit を置く: `scripts/selfdeploy/install-units.sh`（`celeris-sccache.service` を `~/.config/systemd/user/` に置いて daemon-reload。起動はしない）。
3. 有効化: `systemctl --user enable --now celeris-sccache.service`。
4. 確認:
   - `~/.local/celeris/current/bin/celerisctl scratch status` に `sccache L1 /var/lib/celeris/scratch/sccache-l1 (ready)` と
     `hits … / misses …` の行が出る。
   - `ss -ltn | grep 4236` に `127.0.0.1:4236`。
   - daemon の起動ログ（次の昇格以降）に `sccache L1 wired into cargo runs`。run の `runs/<run_id>/request.json` の
     `cargo_target_dir` は従来どおり scratch の target。

## 仕組みと注意

- `RUSTC_WRAPPER` は本物の sccache ではなく、Celeris が生成する `/var/lib/celeris/scratch/bin/sccache`（shell）。sccache 0.18 は
  `CARGO_` で始まる env を全て Rust の cache key に入れるので、owner ごとに違う `CARGO_TARGET_DIR` を rustc の env から外してから
  本物を exec する（外さないと owner をまたいだ hit が 0 になる。phase-G.md の U1）。ファイル名が `sccache` なのは、cc-rs が
  `RUSTC_WRAPPER` の名前を見て C/C++ にも同じ wrapper を使うため。
- server は run の中から起こさない（`celeris@` の cgroup で昇格の巻き添えになる）。server が落ちていても run は素の cargo で動くが、
  「応答を確かめた直後に落ちた」run の中では sccache の client が自分で server を起こしうる（同じ env で起こすので中身は同じ。
  その server は run の cgroup に入るので、気づいたら `celeris-sccache.service` を起こし直す）。
- 人が自分で使う sccache（既定 port 4226）とは port で分かれる。`SCCACHE_IDLE_TIMEOUT=0`（止まらない）。
- L1 の上限は `[scratch] l1_max_gb`（既定 40）。sccache 自身が LRU で管理する（G3 で Celeris の cache server に置き換える）。
- 止める: `systemctl --user disable --now celeris-sccache.service`（run は次から素の cargo）。L1 を捨てる: 止めてから
  `rm -rf /var/lib/celeris/scratch/sccache-l1`。
