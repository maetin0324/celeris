---
title: "prune-scale: 本番 release-build target fixture"
tasks: ["01M4JJNVBZNG9T10SVGR249EC1"]
status: in_progress
updated: 2026-10-10
---
# 本番相当の prune 試験

採取時刻 2026-10-10 09:36Z。対象は `/local/celeris/data/scratch/targets/release-build/target/debug/deps`、marker は `.rustc_info.json` mtime 由来の 08:40:45Z。TSV は binary 199 行を記録し、古い workspace executable は 11 個・102,502,400 allocated bytes（約 0.095 GiB）。これは 08:40Z の過去実測（381 個・108 GiB）とは別時点であり、同一母集団として扱わない。

`bash scripts/selfdeploy/tests/release_prune_production_scale.sh`: exit 0。出力は stale workspace binary 0.095 GiB、縮尺 fixture reclaimed 0.113 GiB production-equivalent。差は最小 512-byte block への切り上げと追加 safety fixture を含む集計による。registry dependency rlib と新しい binary の保持を確認。

この実行では 8 release 蓄積 assertion は未実装であり、過去の 381/108 GiB 時点からの本番相当削減試験としては未完了。
