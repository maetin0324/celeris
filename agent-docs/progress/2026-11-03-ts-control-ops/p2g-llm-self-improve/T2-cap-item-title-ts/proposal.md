# T2-cap-item-title（TS の経路）の改善案

- 何を: 項目の `summary` が 80 code point を超えたら、digest の `### ` 見出しと card の `title` を「先頭 80 code point + `…`」にする。80 以下と、空白だけのときの `<source_kind>:<source_key>` は今のまま。
- どこを: `control-ts/src/domains/cos_digest.ts` の `title`。`digestMessage` は見出しと card の両方で `title` の戻り値を使っているので、`title` 1 か所で両方に効く。
- なぜ: 長い見出しで画面が崩れるため。長さは `string.length`（UTF-16）でなく code point で数え、サロゲートペアを割らない。
- どう確かめるか: `cos_digest.test.ts` に境界（80・81 文字、絵文字、空白だけ）の試験を足し、`trial.sh --path ts` で旧 fixture の差分 0・期待差分の一致と compare→canary→rollback を確かめる。
- 採点の file（spec.json・spec.cases.jsonl・inputs.jsonl）は読まずに実装した。
