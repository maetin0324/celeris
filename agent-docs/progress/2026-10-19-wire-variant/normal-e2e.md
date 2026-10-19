---
task: 01M5AC7DNXVSZE0B214Z2GZ75N
tasks: [01M5AC7DNXVSZE0B214Z2GZ75N]
title: 通常経路の無LLM variant回帰試験（normal-e2e）
work_unit: normal-e2e
status: done
updated: 2026-10-19
---
# normal-e2e

## 変更（crates/celeris/tests/variant_propagation.rs のみ。本体コード不変）
`campaign_wiring_e2e` と同じ一時 DB・loopback 専用 daemon（task-api）・fake のみの config。userns 不要。spec-on / spec-off は別の一時 DB（別所有環境）で、org 準備はその DB だけ。本番と同じ `build_dispatcher` の tick で確認。
- 正（spec-off / spec-on 各 1 件）: 通常 POST 201 → GET の hash・org pin → 冪等再送 200 → content 違い 409・pin 脱落 4xx（task 増えない）→ 子・孫が祖先の hash/pin を継ぐ → 祖先と矛盾する子は拒否 → DB 再 open で同じ解決 → dispatcher tick で `Assigned`（spec-off は単一 coding 課）、blocked でなく、共有 org 不変。
- 負: POST 後の org drift は run 起動なし＋blocked＋理由 event／pin 外の assignee では run 起動なし／daemon 観測値なしは run 起動なし（fail closed）。
- 実 LLM 不可: config の全 `kind` が `fake`、claude/codex/anthropic の語が無いことを assert。

## 証拠
- `cargo test -p celeris --test variant_propagation` → 5 passed, 0 failed
- `cargo clippy -p celeris --all-targets -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0
- `bash scripts/dev/test-parallel.sh` は統合（verify-record / integrate-verify）で実行。

## 未解決事項・提案
- **重要**: daemon 本体（crates/celeris/src）は `Dispatcher::set_variant_observation` を呼んでいない。現状、実 daemon では variant 付き task の run は常に "daemon instance id / config sha256 are not available" で blocked になる（fail closed）。試験は observation を手で渡して正の経路を確認した。daemon 起動時に instance id と config sha256 を渡す配線が別途要る（本 WU は celeris/src を変更しないので未実施。後続の葉で）。
- retry / continuation / repair の dispatch_one 通しは task-dispatch の `variant_pin` 試験（13 件）が担当。ここでは通常 daemon 境界のみ。
