---
title: S6 m-cosd-a — 旧 Rust 経路 cos-digest の warm 試行（前半）
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: running
updated: 2026-11-03
---
# S6 m-cosd-a — 旧 Rust 経路 cos-digest の warm 試行（前半）

unit m-cosd-a。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-10・S5-11 に従う。コードと script は変えない。IEEE Access・K2・K4 の release・state dir・config には触れない。

## 編集が cos-ts と同じ論理であること（測る前に確認）
結論: 同じ。変える判定は「`reason` が `edit-activate-baseline-<nonce>` を含む run には command を 1 つも出さない」で、両方とも当たらない分岐である。期待する出力は、どちらも編集前と同じ（全 fixture・golden で command 列が変わらない）。違いは無かったので測定に進んだ。

`scripts/dev/edit-activate-baseline.sh` の編集の定義（`apply_edit` の python、396〜411 行）:
```python
elif scenario == "cos-ts":
    # cos-digest と同じ論理: reason が nonce 入りの印を含む run には command を出さない（当たらない分岐）
    anchor = '  const { text, cards } = digestMessage(input.run_id, input.run_state, input.reason, input.items);\n'
    add = (f'  // edit-activate-baseline probe {nonce}: a branch that is never taken\n'
           f'  if (input.reason !== null && input.reason.includes("edit-activate-baseline-{nonce}")) {{\n'
           '    return { commands: [] };\n'
           '  }\n')
elif scenario == "cos-digest":
    anchor = '    let (text, cards) = cos_digest_message(\n'
    add = (f'    // edit-activate-baseline probe {nonce}: a branch that is never taken\n'
           '    if input\n'
           '        .reason\n'
           '        .as_deref()\n'
           f'        .is_some_and(|r| r.contains("edit-activate-baseline-{nonce}"))\n'
           '    {\n'
           '        return Vec::new();\n'
           '    }\n')
```

突き合わせ:

| 項目 | cos-ts（`control-ts/src/domains/cos_digest.ts`） | cos-digest（`crates/task-core/src/chat/digest.rs`） |
|---|---|---|
| 入れる関数 | `decide(input: CosDigestInput): CosDigestOutput`（370 行） | `cos_digest_decide(input: &CosDigestInput) -> Vec<CosDigestCommand>`（268 行） |
| 入れる位置 | 関数の先頭、`digestMessage(...)` を呼ぶ行の直前 | 関数の先頭、`cos_digest_message(` を呼ぶ行の直前 |
| 入力の型 | `reason: string \| null`（63 行） | `pub reason: Option<String>`（32 行） |
| 条件 | `reason !== null` かつ `reason.includes("edit-activate-baseline-<nonce>")` | `reason.as_deref().is_some_and(\|r\| r.contains("edit-activate-baseline-<nonce>"))` |
| 当たったときの出力 | `{ commands: [] }`（`CosDigestOutput` の command 列が空） | `Vec::new()`（command 列が空） |
| `reason` が無いとき | 当たらない（`null`） | 当たらない（`None`） |
| nonce | 試行ごとの 8 桁の 16 進数（同じ `apply_edit` が作る） | 同じ |

- `null`／`None` の扱い・部分文字列の判定・空の command 列の 3 点が一致する。TS の出力は `{commands}` の object、Rust は列そのものだが、境界の変換（S4-2）で同じ command 列に当たる。
- fixture・golden・試験の `reason` に `edit-activate-baseline-` を含むものは無い（nonce は試行ごとに乱数）。どちらの経路でも分岐は当たらず、出力は編集前と同じになる。
- 検証責任の揃え方（Rust 側は `-p task-core -p task-dispatch` の 6 段、TS 側は ts-verify の 10 段）は [cos-ts-scenario](cos-ts-scenario.md) のとおりで、ここでは変えない。
