---
title: "健全性v2 R6 write-set: 決定 r6-write-set に従い slices.json の R6 の write set を広げる"
tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 R6 write-set

完了日: 2026-11-15

結論: `agent-docs/progress/2026-11-07-health-v2-b0/slices.json` の R6 の `write_set` に正規表現を 4 つ足した。
slices.json の差分は追加の 4 行だけで、R6 の他の欄と他の slice は変えていない。理由と決定の経緯は
[ADR 2026-11-07 の付記 r6-write-set](../../adr/2026-11-07-health-v2-baseline.md) に書いた。

## 決定
- 決定 r6-write-set の答えは a（推奨どおり）。答えたのは運用者で、人ではない。人が 2026-11-15 に
  「R6が止まっているのにも対処してください」と指示し、運用者がそれを受けて a を選んだ。

## 足した正規表現
- `^crates/[a-z-]+/src/[a-z0-9_/]+\.rs$`（数字を含む src の Rust file）
- `^crates/[a-z-]+/(tests/[a-z0-9_/]+)\.rs$`（数字を含む tests の Rust file）
- `^docs/api/v1/[a-z0-9_.-]+\.json$`（生成した API schema）
- `^web/api/generated/schema\.json$`（web の生成 schema）

B0 の旧い 2 つの Rust の正規表現は消さずに残した。新しい 2 つは旧い 2 つを含むので、許す範囲は旧い方を消した場合と同じ。
残した理由は、受け入れ条件「差分が R6 の write_set への追加だけ」を満たし、B0 の範囲をそのまま読めるようにするため。

## 証拠（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py agent-docs/progress/2026-11-07-health-v2-b0/slices.json`
  → exit 0、「slices-check: ok（工程 11、capability 41、base 5923a76c9aa0）」。
- 試しの path 62 件（`docs/api/v1/api-v1.schema.json`・`docs/api/v1/event.schema.json`・`web/api/generated/schema.json`
  と、`git ls-files` の crates の src・tests にある数字を含む Rust file 59 件）を `--write-set R6` に通した:
  変更後の slices.json → exit 0（「write-set R6: ok」）。変更前の slices.json → exit 1（62 件すべて範囲外）。
- 意図しない path（`docs/api/v1/task-counts.openapi.yaml`・`web/api/generated/other.json`・`crates/task-api/examples/x.rs`）は
  変更後も範囲外（exit 1、3 件）。
- 変更前と変更後の slices.json を JSON として読み比べた: R6 の `write_set` 以外の全欄と他の 10 工程が一致し、
  変更前の `write_set` の 9 つはすべて変更後に残る。追加は上の 4 つだけ。
- `git diff --stat` → slices.json は「4 insertions(+)」だけ。
- この unit の commit は slices.json・ADR の付記・この進捗 file の 1 commit で、他の変更を混ぜていない。

## 未解決
- 無し。範囲を使う変更（console-api・test-dedupe ほか）は各 unit が行う。

## 提案
- 無し。
