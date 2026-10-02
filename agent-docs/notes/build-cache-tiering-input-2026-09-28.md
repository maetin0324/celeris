# 入力メモ: ビルドキャッシュの 2 層化（人の設計方針、2026-09-28）

背景: ルートディスク（ローカル LVM 252G）が、Celeris の run と実装エージェントの cargo target で繰り返し満杯になった（2026-09-26〜28 に 4 回）。ホームは TrueNAS の NFS（1GbE）。人の方針は次のとおり（原文の要旨。設計 ADR はこれに沿う）。

## 方針
- **2 層構造**: NFS = source / worktree / cold cache（容量層）。ローカル NVMe = active Cargo target（burst buffer）+ hot sccache（性能層）。
- **target/ は保存対象ではなく scratch**: 消えても再生成できる。ビルド中に target/ を NFS へ非同期 write-back するのは避ける（Cargo / rustc が書き換え続けるファイルと競合し、整合した snapshot にならない。大量の小ファイルを結局 NFS へ流すことになる）。**active target → NVMe だけ、finished target → 基本削除**。
- **再利用価値は sccache に集約**: sccache は「入力 + compiler version + flags + source → hash → compiled result」なので write-back cache として扱いやすい。
- **sccache は L1（NVMe）/ L2（NFS）の階層キャッシュ**: compile request → L1 lookup（hit → return）→ miss → L2 lookup（hit → L1 へ promote → return）→ miss → rustc compile → L1 へ保存 → **非同期で L2 へ保存**。NFS 書き込みを critical path から外す（NFS が 20 ms 止まっても rustc は待たない）。background flusher は **帯域制限 20〜30 MB/s**。
- **L2（NFS）は 1 entry = 1 ファイルの immutable object**: `/cache/celeris/sccache/ab/ab14c8….zst` のような content-addressed storage。write は tmp file → write → fsync/close → rename(hash.zst)。複数 worker が同じ entry を同時に書いても扱いやすい。NFS 上の 100 万個の小ファイルを避ける。
- **容量の目安（256GB の場合）**: OS / home 等 ~80GB、`/scratch/celeris` 最大 150GB（`active-target/` ~100GB、`sccache-L1/` ~40GB）。NFS 側 `/NFS/celeris-cache/sccache-L2/`（圧縮 immutable objects）。
- **watermark**: target pool > 120GB（または 100GB）→ finished task の target を LRU 削除。sccache L1 > 40GB → cold entries を L2 へ退避（L2 に無いものだけ）。
- **Celeris は semantic cache manager になれる**（単純な LRU より賢く）: RUNNING の target → NVMe 絶対保持、WAITING → 当面保持、COMPLETED → 早期 GC、FAILED だが再実行予定 → 少し長く保持。同じ repo で近い commit（task A = abc123、task B = abc128）の target は保持する、といったヒューリスティック。
- **挙動**: task start → local target allocation → cargo check/build → L1 lookup → miss → L2 lookup → miss → compile → L1 store → async L2 write → task finish → target = reclaimable → NVMe high watermark → completed task target delete → 古い L1 cache delete。
- NFS を build filesystem として使わず、cold object store として使う（TrueNAS + 1GbE でも理にかなう）。

## Celeris への当てはめ（Fable の補足）
- 対象となる cargo 実行: Celeris の worker / WU / 統合 WU / reviewer の checks、`release.sh` のゲート（現在 `/var/lib/celeris/release-build/.cargo-target`）、Fable 配下の実装エージェント（`.claude/worktrees/agent-*`、現在は `CARGO_TARGET_DIR=/var/lib/celeris/build-cache/cargo/agent-platform-<name>` を手で指定）。
- 現在の物理構成: ローカルはルート LVM 1 本（`/`、252G。DB `/var/lib/celeris/celeris.sqlite3` も同居）。NVMe の別ボリュームは無い（Proxmox 側で拡張または `mp1` 追加は人の判断）。NFS は `/home/rmaeda`（6.4T）。
- 直近の Phase F5-fix（Opus、進行中）が「WU ごとの `CARGO_TARGET_DIR` + WU 終端で削除」を入れる。本設計はそれを包含する（WU ごとの target は scratch pool の 1 entry になる）。
- sccache は未導入（`which sccache` で確認すること）。sccache 自体に L2 階層は無い。候補: (a) sccache の local disk cache（`SCCACHE_DIR`）を L1 とし、Celeris が L1 → L2 の flusher と「task start 時の L2 → L1 の promote（同じ repo/commit の entry）」を持つ、(b) sccache の webdav backend に対して Celeris が L1/L2 を実装した小さな cache server を loopback で立てる（真の階層キャッシュ。miss 時に L2 を見る）。ADR で比較して決める。
