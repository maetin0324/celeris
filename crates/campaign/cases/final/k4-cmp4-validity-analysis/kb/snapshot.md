# KB snapshot（運用の一般事実だけ）

- Sirius の会計群は gen/NBBG。1 vnode/host 配置。計算ノードで build せず login node で build する。
- IOR の JSON 結果は `summary`（operation/bwMeanMIB）と `tests[0].Results`（access/bwMiB）を持つ。欄名は MIB と MiB が混在する。
- 実験の実効設定（fsync・checkRead・transport）は結果 JSON と実効設定 log から読む。報告の文言は証拠にならない。
- job の成否は `qstat -x` の Exit_status で確かめる。job ID の無い数値は証拠にならない。

解答・過去の結果・既知欠陥の一覧はこの snapshot に含めない。
