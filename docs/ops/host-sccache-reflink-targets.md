# host の sccache と `/local` の reflink target への切り替え

---
tasks: [01M3YD2Z585N1YCBZK4AH8QXR0]
---

この手順は本番 host の管理者が実行する。Celeris の run は Proxmox、container の mount、user unit、本番 config、daemon を変更しない。設計は [ADR-0129](../../agent-docs/adr/0129-host-sccache-reflink-targets.md)、scratch の置き場は ADR-0136（`0136-local-hot-data-layout.md`、main 未取り込み）、Cargo の実測は [実験記録](../../agent-docs/progress/2026-10-02-host-sccache-reflink/reflink-target-experiment.md)、リリースの切り替えは [selfdeploy](selfdeploy.md) を参照する。以下の `CTID`、`pve/data`、容量、host の mount path は実機に合わせて置き換える。

## 1. Proxmox の btrfs volume を `/local` に見せる

Proxmox **host** で、LVM-thin pool と未使用の volume 名、容量を `lvs -a -o lv_name,vg_name,lv_size,pool_lv,lv_attr` で確認する。既存データがない新規 LV に限り、例えば次を実行する。`mkfs.btrfs` は指定した LV の内容を消すため、直前に `lsblk -f /dev/pve/celeris-local` で対象を再確認する。

```bash
lvcreate -V 300G -T pve/data -n celeris-local
lsblk -f /dev/pve/celeris-local
mkfs.btrfs -L celeris-local /dev/pve/celeris-local
mkdir -p /mnt/celeris-local
blkid /dev/pve/celeris-local
```

host の `/etc/fstab` に、`blkid` で得た UUID を使って `UUID=<UUID> /mnt/celeris-local btrfs defaults 0 0` を追加し、`mount /mnt/celeris-local` と `findmnt -no SOURCE,FSTYPE,TARGET /mnt/celeris-local` で btrfs の mount を確認する。container の停止時間を確保して `pct stop <CTID>`、空いている mount point 番号を `pct config <CTID>` で確認し、例えば `pct set <CTID> -mp0 /mnt/celeris-local,mp=/local`、`pct start <CTID>` とする。既存の `mp0` は上書きしない。host で `pct config <CTID>`、container で `findmnt -no SOURCE,FSTYPE,TARGET /local` を実行し、`/local` が btrfs であることを確認する。container の Celeris 実行ユーザーで `test -w /local` も確認し、必要な所有権を host 側から設定する。

container では `ioctl(FICLONE/FICLONERANGE)` が `EPERM` になるため、`cp --reflink=always` の成否を確認に使わない。`cp -a --reflink=auto` は GNU coreutils 9.5 のこの環境で `copy_file_range(2)` に落ち、btrfs の extent を共有する。container の Celeris 実行ユーザーで、**空き容量と `filefrag`（e2fsprogs）の導入を確認してから**同じ `/local` 内に試験ファイルを作る:

```bash
cp --version | head -1                     # この環境は GNU coreutils 9.5
findmnt -no SOURCE,FSTYPE,TARGET /local    # FSTYPE=btrfs
probe=$(mktemp -d /local/reflink-check.XXXXXX)
dd if=/dev/urandom of="$probe/source" bs=1M count=512 status=progress
sync
df -B1 --output=used /local | tail -1     # コピー前の使用量を記録
cp -a --reflink=auto "$probe/source" "$probe/copied"
sync
df -B1 --output=used /local | tail -1     # コピー後の増分を比較
filefrag -v "$probe/source" "$probe/copied"  # extent の flags に shared があるか確認
cmp "$probe/source" "$probe/copied"
rm -rf -- "$probe"
```

512 MiB の実体増加に対し `df` の増分が小さく、`filefrag` で **両ファイルの extent に `shared`** が見えれば共有できている。`df` は他の書き込みの影響を受けるので、判定が曖昧なら静かな時間帯に再試行する。`du` は共有の証明にならない。Celeris の実装も `cp -a --reflink=auto` 後に FIEMAP で共有を確認し、共有を確認できなければコピーを捨てて空の target に戻る。

## 2. host 管理の Cargo に sccache を設定する

以下は **container 内の Celeris 実行ユーザー**が管理する host 側 Cargo 設定であり、Celeris のサービス設定ではない。`sccache` と `nc` を導入し、`command -v sccache nc` で両方を確認する（例: 配布パッケージ、または `cargo install sccache --locked`）。`nc` が無いと wrapper は server を使わず素の rustc に戻る。`/local` が mount され、書き込めることを確認してから設定する。

```bash
install -d -m 755 "$HOME/.local/bin" "$HOME/.config/systemd/user" "$HOME/.cargo"
install -m 755 scripts/host-sccache/rustc-wrapper.sh "$HOME/.local/bin/rustc-wrapper.sh"
install -m 644 scripts/host-sccache/sccache.service "$HOME/.config/systemd/user/sccache.service"
mkdir -p /local/sccache  # ADR-0136 の Celeris 管理 tree（/local/celeris/{data,state}）の外の host 管理 cache
test -w /local/sccache
systemctl --user daemon-reload
systemctl --user enable --now sccache.service
systemctl --user is-active sccache.service
```

unit の `ExecStart=%h/.local/bin/sccache` に合わせ、`command -v sccache` が別の path なら unit の `ExecStart` を実際の絶対 path に直してから `daemon-reload` する。unit の `SCCACHE_DIR=/local/sccache`（ADR-0136 の Celeris 管理 tree の外）と容量上限 `SCCACHE_CACHE_SIZE=20G` も実機に合わせる。`SCCACHE_CACHE_SIZE` は ADR-0136 が切替直前に要求する `/local` の 30GiB 以上の空きを侵さない値にする。`systemctl --user status sccache.service` と `sccache --show-stats` で起動を確認する。

既存の `~/.cargo/config.toml` をバックアップして `[build]` に次を追加する。既存の `[build]` があれば同じ節にキーを足し、重複節は作らない。`~` は展開されないため、**実際の絶対 path** を書く。[設定例](../../scripts/host-sccache/cargo-config.toml.example) も参照する。

```toml
[build]
rustc-wrapper = "/home/<user>/.local/bin/rustc-wrapper.sh"
```

小さな Cargo project で `cargo check -v` を実行し、`sccache --show-stats` の実行・hit/miss 数が増えることを確認する。次に `systemctl --user stop sccache.service`、同じ project の Rust source を更新して `cargo check -v` を再実行し、server が無くても build が成功することを確認し、`systemctl --user start sccache.service` で戻す。wrapper は到達不能時に最初の rustc 引数を直接 `exec` する。worker が独自の `CARGO_HOME` を使う場合は、その sandbox から Cargo config と wrapper が読めるかも確認する。server が届かなくても権限を広げず rustc に戻す。

## 3. Celeris の scratch と旧 cache を切り替える

scratch の正本の置き場は ADR-0136（`0136-local-hot-data-layout.md`、main 未取り込み） が `/local/celeris/data/scratch` と定める。

最初に [selfdeploy の release と verify](selfdeploy.md) を済ませ、新版を昇格できる状態にする。停止する前に進行中の run と release build がないこと、`~/.local/celeris/current/bin/celerisctl scratch status --json`、`du -sh /var/lib/celeris/scratch/{targets,cache-l1} 2>/dev/null`、`df -h /local` を記録する。既存の scratch 全体と config は戻すときまで保存する。

daemon を停止し、`/local` が mount されたことを再確認して `/local/celeris/data/scratch` を作る。`seed_reflink` は既定で false。§1 の共有確認が通った後、切り替え時に次のように **true を明示**する。有効でも Celeris は seed build 前に実際の pool 内でコピーと FIEMAP shared の probe を行い、共有できなければ seed を作らず空の target から build する。不可の理由は `scratch: seed refresh disabled; pool cannot share extents` ログで確認する。`mount` を指定すると `/local` が無い起動では従来の既定 scratch dir（本番では `/var/lib/celeris/scratch`）へ戻り、理由をログに出す（`/local` 上に同名ディレクトリは作らない）。`mount` を省略して `dir` だけを指定するとこの保護は働かない。

```toml
[scratch]
dir = "/local/celeris/data/scratch"
mount = "/local"
seed_reflink = true
```

既存 target はどちらかを選ぶ。**捨てる場合**は旧 `/var/lib/celeris/scratch/targets` をしばらく保存し、新 pool にはコピーしない。新しい owner は seed ができるまで空から build される。**移す場合**は daemon と release build を止めた状態で、`rsync -a /var/lib/celeris/scratch/targets/ /local/celeris/data/scratch/targets/` とし、`lease.json` の mtime と owner の `target/` まで含めて移す。`rsync -a --dry-run --checksum /var/lib/celeris/scratch/targets/ /local/celeris/data/scratch/targets/` の出力が空であることを確認する。異なる filesystem 間なのでこの移行コピー自体は reflink ではない。旧 scratch はすぐ消さず、戻し先として残す。seed は `targets/` の外にあり、旧 target を手で seed として流用しない。

新版の [selfdeploy 昇格手順](selfdeploy.md) に従い daemon を差し替え、`celerisctl scratch status --json` と `findmnt -no FSTYPE,TARGET /local` で pool の使用と空き容量を確かめる。登録済みの local Cargo repo の seed は起動直後の housekeeping が main の commit から自動で初回作成する。`/local/celeris/data/scratch/seeds/<repo-key>/current/manifest.json` と `current/target/`、daemon の `scratch: seed switched` ログを確認する。main が進むか rustc・`[scratch.cargo]` が変わると更新され、容量不足なら旧 seed を保持して保留する。新しい task/WU の `targets/<owner>/target-origin.json` と `filefrag -v` で seed 由来と extent 共有を確認する。旧 owner は上書きされない。

新版への昇格と scratch の切り替えを確認後、旧サービスを止めて無効化する:

```bash
systemctl --user disable --now celeris-sccache.service celeris-scratch-cache.service
systemctl --user is-enabled celeris-sccache.service celeris-scratch-cache.service
systemctl --user is-active celeris-sccache.service celeris-scratch-cache.service
```

後ろの二つは `disabled` / `inactive` を確認するコマンドで、非 0 終了が正常な場合もある。旧 `~/.config/systemd/user/` の unit ファイルを削除して `systemctl --user daemon-reload` する。`scratch status --json` の旧 `sccache` / `cache` 欄が `null` で、旧サービスと token の参照元が無いことを確認してから、**旧 scratch 内だけ**の `/var/lib/celeris/scratch/cache-l1`、`sccache-l1`、`cache-server.token` を手で削除する。`/local/sccache` は host 管理の新 cache なので削除しない。[旧 cache の後始末](sccache-l1.md) も参照する。

## 4. `/local` がまだ無い場合

`[scratch] dir` と `mount` を切り替えず、現行の `/var/lib/celeris/scratch` のまま運用する。host の sccache を先に導入するなら unit の `SCCACHE_DIR` を既存のローカルディスク上の別 path に直すか、unit を起動せず wrapper の素の rustc への戻りを使う。`/local` を指定済みでも `mount = "/local"` があれば mount 不在時は既定 scratch に戻るが、切り替え前に `findmnt /local` と書き込み権限を確認する。`/local` が無いまま同名ディレクトリに scratch を作らない。

## 5. 戻し方

問題が出たら新規 run と release build を止め、daemon を停止する。config の `[scratch] dir` を旧 `/var/lib/celeris/scratch` に戻し、`mount` を外す。旧 scratch を保存したままなら、その `targets/` と lease を再利用できる。新版のまま戻す場合も `seed_reflink = false` にすれば seed コピーを止め、空または既存 target で build できる。`celerisctl scratch status --json` と run の `CARGO_TARGET_DIR` が旧 pool を示すことを確認する。

daemon の版も戻す必要があれば [selfdeploy の `rollback.sh`](selfdeploy.md) を人が実行する。DB restore の要否は同手順の schema 検査で決める。旧版が Celeris 管理の cache server を必要とする場合だけ、保存した旧 unit と設定を復元して有効化し、`systemctl --user is-active` で確認する。host の `~/.cargo/config.toml` の `rustc-wrapper` と `sccache.service` は独立しており、個別に外せる。`/local` の LV と mount は scratch・sccache の参照が無くなったことを確認するまで消さない。

## 実測から見た期待値

[実験記録](../../agent-docs/progress/2026-10-02-host-sccache-reflink/reflink-target-experiment.md)と[ADR-0129](../../agent-docs/adr/0129-host-sccache-reflink-targets.md)では、28 MiB の seed を ext 系 filesystem 上で実体コピーしたとき、同じ source・別 target は `Compiling` 0 行、0.03 秒、`df` +27.6 MiB だった。別 source でも mtime を保てば 0 行、0.03 秒、+27.6 MiB。source の mtime を更新すると path 依存の 2 crate が再 build され 0.23 秒。空からの build は 5 crate、0.74 秒だった。これらは小さな Cargo workspace の結果であり、btrfs の節約量ではない。別途、人が container の `/local` で 512 MiB を `copy_file_range` / coreutils 9.5 の `cp -a --reflink=auto` で写したときは `df` 増分 0 MiB、`filefrag` は `shared` を示した。実際の worktree で path 依存 crate の mtime が新しければ再 build は残る。
