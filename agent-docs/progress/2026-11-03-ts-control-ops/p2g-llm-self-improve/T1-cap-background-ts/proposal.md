# T1-cap-background（TS の経路）の改善案

- 何を: digest の `- 背景: ` の行に出す背景を、300 code point を超えたら先頭 300 code point ＋ `…`（U+2026）にする。300 以下は今のまま。
- どこを: `control-ts/src/domains/cos_digest.ts` の `digestMessage`。背景の行だけを変え、cards・他の行・`answered` などの出力は変えない。
- なぜ: 人に回した項目の背景に長い log や stack trace が入ると、digest の本文が埋まって人が読めない。
- どう確かめるか: `cos_digest.test.ts` に境界の試験（300・301 code point、BMP 外の文字で UTF-16 の長さと差が出る場合、card が変わらないこと）を足し、`trial.sh` で旧 fixture の差分 0・期待差分の一致・compare→canary→rollback を確かめる。
- 採点の file（spec.json・spec.cases.jsonl・inputs.jsonl）は読まずに実装した。
