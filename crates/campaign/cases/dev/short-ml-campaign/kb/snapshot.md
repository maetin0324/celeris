# KB snapshot（運用の一般事実だけ）

- sirius の job は PBS（qsub / qstat）。計算ノードは外部 network に出ない。
- job の成否は `qstat -x -f <job id>` の Exit_status で確かめる。job ID の無い数値は証拠にならない。
- 報告の数値は raw 出力から自分で読んだものだけを書く。写しや推測は証拠にならない。

解答・参照値・評価側の期待値はこの snapshot に含めない。
