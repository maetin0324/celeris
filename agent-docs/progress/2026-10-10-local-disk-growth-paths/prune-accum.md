---
tasks: [01M4JKZVZW1KKR13B6TR9MASW4]
---
# 8 release 積み上げの縮尺試験

`bash scripts/selfdeploy/tests/release_prune_production_scale.sh` を引数なしで実行した。committed inventory の workspace executable 199 個を 1 世代とし、fallocate の 4 KiB 実 block で縮尺 1/1024 に組み立てた。A は刈らずに累積、B は毎 release の開始時に前回 build marker で prune し、追いつきは A の最終状態を一度 prune した。registry 由来 `rlib/.d` は保持される。

```text
RESULT no_prune_bins=597 no_prune_gib=162.945 catchup_reclaimed_gib=149.367 prune_max_gib=67.902 prune_final_gib=27.156 prune_bound_gib=67.902 nonincreasing=1 limit_recreate_without_prune=1 limit_recreate_with_prune=0 deps_kept=1
```

| release | A: 刈らない累積 (GiB) | B: 毎回刈る (GiB) |
| 1 | 67.902 | 67.902 |
| 2 | 81.484 | 27.172 |
| 3 | 95.062 | 27.168 |
| 4 | 108.633 | 27.156 |
| 5 | 122.215 | 27.160 |
| 6 | 135.797 | 27.172 |
| 7 | 149.375 | 27.168 |
| 8 | 162.945 | 27.156 |

全数値条件を満たした。刈らない場合は 597 binary・162.945 GiB まで増え、追いつきで 149.367 GiB 回収した。毎回刈る場合の最大量は 67.902 GiB で理論上限以下、最終量は 27.156 GiB。上限超過では刈らない系列だけ target 再作成が発生し、刈る系列は発生しない。
