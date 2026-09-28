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
- L1 の上限は `[scratch] l1_max_gb`（既定 40）。sccache 自身が LRU で管理する（G3 の cache server を使うときは下の「L2」の節）。
- 止める: `systemctl --user disable --now celeris-sccache.service`（run は次から素の cargo）。L1 を捨てる: 止めてから
  `rm -rf /var/lib/celeris/scratch/sccache-l1`。

## L2（Phase G3、ADR-0075 D5 (b)）: Celeris の階層 cache server

sccache の server を webdav backend として `celeris cache-server`（`celeris-scratch-cache.service`）に向ける。cache server は
L1 = `/var/lib/celeris/scratch/cache-l1`（ローカル）、L2 = `~/.local/celeris/cache/sccache-l2`（NFS、`<k0k1>/<key>.zst` の immutable
object）を持つ。GET は L1 → L2 → miss（L2 hit は L1 へ promote）、PUT は L1 に書いてすぐ返し、L2 へは flusher が 25 MB/s で
非同期に書く。NFS が止まっても L1 だけで動く（3 回連続の失敗で L2 を切り離し、バックオフの後に復帰）。

### 前提

- G3 を含む release が**昇格済み**（`~/.local/celeris/current/bin/celeris cache-server` がある版）。L1 の手順（上）が済んでいる。
- `[scratch.l2]` / `[scratch.cache_server]` は**書かなくてよい**（既定: L2 有効、`~/.local/celeris/cache/sccache-l2`、300 GB、25 MB/s、
  GET の L2 は 500 ms で打ち切り、cache server は `127.0.0.1:4237`、token は `/var/lib/celeris/scratch/cache-server.token`〈初回に自動生成〉）。
  変えるときも、この節を知る release が昇格した**後**に足す（ADR-0075 D7）。

### 手順（人が 1 回）

1. unit を置く: `scripts/selfdeploy/install-units.sh`（`celeris-scratch-cache.service` を置いて daemon-reload。起動はしない）。
2. cache server を有効化: `systemctl --user enable --now celeris-scratch-cache.service`。
3. 確認: `curl -s http://127.0.0.1:4237/healthz` → `ok`、`journalctl --user -u celeris-scratch-cache.service` に
   `scratch cache server listening on 127.0.0.1 port=4237`。
4. sccache の server を webdav に切り替える: `systemctl --user restart celeris-sccache.service`（backend は起動時に決まる）。
   `journalctl --user -u celeris-sccache.service` の先頭に `# celeris: sccache backend = webdav http://127.0.0.1:4237`（cache server が
   応答しなければ `backend = disk …` で G2 のまま動く）。
5. `~/.local/celeris/current/bin/celerisctl scratch status` に
   - `cache server http://127.0.0.1:4237 (ready) · sccache backend webdav`
   - `gets … · L1 hit …% · L2 hit …% · miss …% · promotes … · puts …`
   - `L2 /home/rmaeda/.local/celeris/cache/sccache-l2 (ok) …`（`l2_bytes` は起動 60 s 後の初回走査から）
   - `flush queue … (…, oldest … s) · last flush … · cap 25 MB/s`
   GUI のデーモン画面の scratch の行に「· L1 hit …% · L2 hit …% · flush 遅延 … s」。

### 仕組みと注意

- sccache 0.18 は backend が起動時に応答しないと**起動に失敗する**。そのため `celerisctl scratch env --server` は cache server の
  `/healthz` を確かめ、応答が無ければ local disk（`sccache-l1/`）で起動させる。選んだ方は `/var/lib/celeris/scratch/bin/sccache-server.mode`。
- sccache は backend の応答を timeout なしで待つ。webdav で動いている sccache に対して cache server が応答しない（止まった・hang）と、
  dispatcher は次の run から `RUSTC_WRAPPER` を与えない（素の cargo。`scratch status` の sccache 行が `unavailable: sccache uses the webdav
  backend but the cache server … does not answer /healthz`）。cache server を起こし直せば次の run から戻る。cache server を長く止めるなら
  `systemctl --user restart celeris-sccache.service` で disk に戻す。
- 昇格で cache server は再起動しない（protocol を変えた版だけ人が `systemctl --user restart celeris-scratch-cache.service`、その後
  `celeris-sccache.service` も再起動）。止めると未 flush の key は `cache-l1/.pending` に残り、次の起動で積み直す。
- `cache-l1/` と `sccache-l1/`（G2 の disk cache、fallback 用）は別の dir。どちらも `l1_max_gb`（40）までだが、使われるのは一方だけ。
  L1 を捨てる: cache server を止めてから `rm -rf /var/lib/celeris/scratch/cache-l1`（L2 から promote し直す）。
- L2 の GC は 1 日 1 回、300 GB を超えた分を mtime の古い順に消す。L2 を捨てる: cache server を止めてから
  `rm -rf ~/.local/celeris/cache/sccache-l2`。
- 毎 run 約 30 crate（workspace のメンバーと `OUT_DIR` 依存）は owner ごとに key が変わり、約 100 MB が L2 に書かれて再利用されない
  （phase-G.md の G3 checkpoint 4。LRU で回収される）。
