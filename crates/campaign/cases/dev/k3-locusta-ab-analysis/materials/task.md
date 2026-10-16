# K3: Locusta 低並列対策 A/B の原票からの再分析

入力は job 43223 の原票 tgz（取得手順は case.json）。条件 L_base / L_A / L_B / L_AB / U、転送 size {4 KiB, 1 MiB, 4 MiB}、3 反復。
`ior_results/ior_result_0.json` から条件×size×op の帯域を読み、L_A/L_base・L_B/L_base・L_AB/U を求めて表にし、効果を報告する。
統計量は「反復ごとの比の平均」に固定する（case 条件。暫定、人の確認待ち）。分母（反復数・有効セル数）を表に書く。
既存の集計や過去の報告は渡さない。原票から自分で計算すること。
