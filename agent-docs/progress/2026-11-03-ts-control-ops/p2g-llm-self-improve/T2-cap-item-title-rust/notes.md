# T2-cap-item-title（旧 Rust の経路）の記録

- 変更: `crates/task-core/src/chat/triage.rs` に `CosTriageItemReport::digest_title`（`summary` が 80 code point を超えたら先頭 80 ＋ `…`、空白だけなら `title()` のまま）と定数 `COS_DIGEST_TITLE_MAX_CHARS` を足した。`crates/task-core/src/chat/digest.rs` の `cos_digest_message` は `### ` の見出しと card の `title` にこれを使う。`title()` は変えていない（digest 以外の fallback・launch・review の出力を変えないため）。
- 試験: `triage_tests.rs` に `digest_title_caps_long_summaries_at_80_code_points` を 1 件足した（80 文字はそのまま、81 文字超は切る、絵文字を含む多 byte、空白だけ、本文と card の両方）。trial の前に `cargo nextest run -p task-core -E 'test(/digest/)'` で 11 件 pass。
- patch は 1 回だけ作った（+47/-1、3 file）。
- trial は 1 回（`trial.jsonl` の 1 行）で ok=true、計 533.0 秒。段ごとの秒: scope 0.02・check 33.3・clippy 54.6・test-build 137.7・test-run 91.5・build 63.1・release-build 116.1・score 30.3。
- 落ちた段はない。直したことはない。
- 採点の file（spec.json・spec.cases.jsonl・inputs.jsonl）は読んでいない。
