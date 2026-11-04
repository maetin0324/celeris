# T1-cap-background（旧 Rust の経路）の改善案

- 何を: digest 本文の `- 背景: ` の行で、`decision.background` が 300 code point を超えたら先頭 300 code point ＋ `…`（U+2026）にする。300 以下は今のまま。
- どこを: `crates/task-core/src/chat/digest.rs` の `cos_digest_message`。切り詰めは新しい純関数 `clip_background`（`char_indices().nth(300)` で byte 位置を求める）に置く。cards・他の行は触らない。
- なぜ: 長い log・stack trace が背景にそのまま入ると、人が読む digest が埋まるため。
- どう確かめるか: 単体試験 `digest_background_is_clipped_at_300_code_points`（300 ちょうど・301 以上・astral 文字・短い文字列）と、`scripts/dev/llm-improve/trial.sh --path rust` の採点。
- 採点の file（spec.json・spec.cases.jsonl・inputs.jsonl）は読んでいない（`ls` で名前を見ただけ）。
