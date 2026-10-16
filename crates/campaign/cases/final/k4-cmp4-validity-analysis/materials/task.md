# K4: 4 FS 比較の有効性分析

入力は cmp4 v3 の N=2 原票 tgz（job 43706）。FS（BenchFS/CHFS/GekkoFS/UnifyFS）×workload ごとの帯域・IOPS と、
セルが有効か無効か（理由つき）を原票から決め、有効セル数／全セル数の分母つきの表と報告を作る。
実効設定が要求（fsync など）を満たさないセル、job が正常終了していないセルは有効に数えない。欠測は分母に残す。
