---
title: docs 再構成 — task-worker の docs パス参照の追従（refs-worker）
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
status: done
updated: 2026-10-02
---
# refs-worker: task-worker の prompt・試験・check 指針を新配置に合わせる

ADR-0128（`agent-docs/adr/0128-docs-layout.md`）と `scripts/dev/docs-layout.tsv` に合わせて、
`crates/task-worker/` に出る docs のパスと記録規則を直した。削除したファイルは無い。

## 変えたもの

- planner の check 指針（`claude_code/prompt.rs` の `PLANNER_CHECK_GUIDANCE`）:
  - 差分範囲 check で記録として除外するパスを `agent-docs/progress/`・`agent-docs/adr/` に変更。
  - 記録の規則（D3・D5）を 1 項目で追加: 新しい ADR は `agent-docs/adr/YYYY-MM-DD-<slug>.md`（番号の新設なし）、
    進捗は task ごとの `agent-docs/progress/YYYY-MM-DD-<slug>.md`、並列 unit は `…/<slug>/<unit key>.md`、
    `agent-docs/PROGRESS.md` には追記しない。
  - land・close-out・final verify の葉の check に `check-doc-links.sh`・`check-adr-numbers.sh`・
    `progress-index.sh --check` を含める項目を追加（D7）。
  - 試験規則の参照を `docs/testing.md` → `agent-docs/guides/testing.md`。
- plan / review prompt の `DESIGN §5.6`・`§6`・`§5.7` の言及を ADR-0007 へ（DESIGN.md は D8 で削除済み）。
- 秘書の前置き（`preamble.rs`）の「範囲指定に含める記録の置き場所」を `agent-docs/adr/`・`agent-docs/progress/` に変更。
- コメント: `DESIGN 原則 N` → `ADR-0001 D2 原則 N`（原則の表は ADR-0001 D2 にある）、`DESIGN §5.x` は ADR か
  `docs/SPEC.md §3.7` へ。旧パス（`docs/PROGRESS.md`・`docs/adr/0054|0063`・`docs/llm-source.md`・`docs/knowledge.md`・
  `docs/ops/browser-isolated-runtime-subuid.md`・`docs/progress/phase-G.md`）を新パスへ。
- 文言試験 `planner_prompt_has_the_check_writing_section` に新しい 3 項目を追加し、旧文言を追従。
- `protocol.rs` の doc comment 変更に伴い `docs/protocol/worker-protocol.schema.json` を `UPDATE_SCHEMA=1` で再生成
  （description 2 か所だけの差分）。`docs/protocol/` の場所は変えていない。

## 証拠

- `sh scripts/dev/check-doc-links.sh crates/task-worker` → `check-doc-links: ok`、exit 0（変更前は 13 件の壊れた参照で exit 1）。
- `cargo test -p task-worker --lib --no-run` → exit 0。
- lib 試験の絞り込み: claude_code 97 passed / preamble 30 / protocol 11（1 ignored）/ aider 9（1 ignored）/ codex 82 / acp 28、いずれも 0 failed。
- `cargo fmt -p task-worker -- --check` → exit 0、`cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0。

## 未解決

- `docs/protocol/worker-protocol.md`・`docs/api/v1/gui-api.md` の本文に DESIGN への言及が残る（範囲外。cleanup-api で扱う）。
