---
tasks: [01M4JBAJ5TWEC6PFQ5PV73Y2QD]
---

# release-build target の共有を考慮した上限判定

## 変更

- `sd_release_prune_size` は btrfs 上で `btrfs filesystem du -s --raw` が成功すれば Exclusive + Set shared を使う。
- btrfs が無い・失敗・出力不正なら inode ごとに `st_blocks * 512` を数える。hardlink と sparse の見かけの領域を重複計上しない。
- 非 btrfs host では reflink extent の共有量を把握できず、共有分は重複計上され得る。判断方式と限界は ADR の D3 付記に記載。
- 回帰試験に 16 MiB sparse file、btrfs の成功出力 fixture、btrfs unavailable 条件を追加。既存の上限 1 byte による再作成試験も維持。

## 検査

- `bash scripts/selfdeploy/tests/release_prune_stale_test_binaries.sh` — exit 0。sparse file がある target は 1 MiB 上限でも作り直されず、既存の 1 byte 上限では作り直される。
- `sd_release_prune_size` 本体（関数開始から閉じ括弧まで）に `st_size` の文字列がない。
