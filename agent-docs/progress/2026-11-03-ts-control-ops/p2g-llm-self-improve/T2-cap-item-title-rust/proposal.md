# T2-cap-item-title（旧 Rust の経路）の改善案

- 何を: 受信箱の項目の見出し（`summary`）が 80 code point を超えたら、先頭 80 code point ＋ `…`（U+2026）に切る。空白だけのときの `<source_kind>:<source_key>` と 80 以下は変えない。
- どこを: `crates/task-core/src/chat/triage.rs` に `CosTriageItemReport::digest_title`（`title()` を `chars()` で数えて切る）を足し、`crates/task-core/src/chat/digest.rs` の `cos_digest_message` で `### ` の見出しと card の `title` の両方にこれを使う。`title()` 自体は変えない（fallback・launch・review も `title()` を使うので、digest 以外の出力を変えないため）。
- なぜ: 長い見出しで digest の本文と chat の card が崩れる。code point で数えるのは課題文の指定（UTF-16 長ではない）。
- どう確かめるか: task-core に単体試験（80・81 code point、多 byte 文字、空白だけ）を足し、`trial.sh --path rust` で check・clippy・試験・build・採点を通す。
- 採点の file（spec.json・spec.cases.jsonl・inputs.jsonl）は読んでいない。
